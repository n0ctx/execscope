use crate::{
    model::{self, Detail, Request, Sample, Scope, Status},
    network, platform, project, safe, tools,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const LOCAL_BUDGET: Duration = Duration::from_millis(650);
const BRIEF_TOOLS_BUDGET: Duration = Duration::from_millis(1050);
const FULL_TOOLS_BUDGET: Duration = Duration::from_millis(1850);

#[derive(Serialize)]
struct Cwd {
    value: String,
    source: &'static str,
    status: Status,
}

#[derive(Serialize)]
struct Snapshot<'a> {
    schema_version: &'static str,
    sampled_at: String,
    scope: Scope,
    detail: Detail,
    environment_source: &'static str,
    cwd: Cwd,
    overall_status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Sample<platform::System>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<Sample<project::Project>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hardware: Option<Sample<platform::Hardware>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    network: Option<Sample<network::Passive>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_network: Option<Sample<network::Active>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    tools: BTreeMap<&'static str, tools::Fact>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<&'static str>,
    #[serde(skip)]
    requested: Option<&'a [model::Tool]>,
}

type CacheEntry<T> = (PathBuf, bool, Instant, Sample<T>);
struct Cache<T> {
    values: Mutex<VecDeque<CacheEntry<T>>>,
}
impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self {
            values: Mutex::new(VecDeque::new()),
        }
    }
}
impl<T: Clone> Cache<T> {
    fn sample(
        &self,
        key: &Path,
        deep: bool,
        ttl: Duration,
        refresh: bool,
        get: impl FnOnce() -> T,
        valid: impl FnOnce(&T) -> bool,
    ) -> Sample<T> {
        if !refresh
            && let Some((_, _, _, data)) = self
                .values
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .find(|(p, d, at, _)| p == key && *d == deep && at.elapsed() < ttl)
        {
            let mut data = data.clone();
            data.cached = true;
            return data;
        }
        let data = Sample::new(get());
        if valid(&data.data) {
            let mut values = self.values.lock().unwrap_or_else(|e| e.into_inner());
            values.retain(|(p, d, _, _)| p != key || *d != deep);
            values.push_front((key.to_owned(), deep, Instant::now(), data.clone()));
            values.truncate(16);
        }
        data
    }
}

#[derive(Default)]
struct Caches {
    hardware: Cache<platform::Hardware>,
    passive: Cache<network::Passive>,
    probes: Arc<tools::ProbeCache>,
}

pub(crate) struct Engine {
    caches: Option<Arc<Caches>>,
    requests: tokio::sync::Semaphore,
    // A bounded blocking pool prevents hostile mounts from piling up work after a timeout.
    slots: Arc<tokio::sync::Semaphore>,
}

impl Engine {
    pub(crate) fn new(persistent: bool) -> Self {
        Self {
            caches: persistent.then(|| Arc::new(Caches::default())),
            requests: tokio::sync::Semaphore::new(3),
            slots: Arc::new(tokio::sync::Semaphore::new(8)),
        }
    }

    pub(crate) async fn inspect(&self, request: &Request) -> Result<String, &'static str> {
        request.validate()?;
        let _request = self
            .requests
            .try_acquire()
            .map_err(|_| "inspection is busy; retry after an in-flight request finishes")?;
        let source = if request.cwd.is_some() {
            "explicit"
        } else {
            "process"
        };
        let input = request.cwd.clone();
        let cwd = blocking(&self.slots, move || directory(input))
            .await
            .ok_or("working directory lookup timed out or inspection is busy")??;
        let deep = request.detail == Detail::Full;
        let refresh = request.refresh;
        let scope = request.scope;
        let requested = request.requires.as_deref();
        let needs_project = matches!(scope, Scope::Auto | Scope::Dev | Scope::All)
            || requested.is_some_and(|r| !r.is_empty());
        let needs_system = matches!(scope, Scope::Auto | Scope::System | Scope::Dev | Scope::All);
        let needs_hardware = matches!(scope, Scope::Auto | Scope::Hardware | Scope::All);
        let needs_network = matches!(scope, Scope::Auto | Scope::Network | Scope::All);
        let deep_hardware = matches!(scope, Scope::Hardware | Scope::All);
        let (mut system, project, hardware, network, developer) = tokio::join!(
            maybe_blocking(needs_system, &self.slots, {
                let cwd = cwd.clone();
                move || Sample::new(platform::system(&cwd))
            }),
            maybe_blocking(needs_project, &self.slots, {
                let cwd = cwd.clone();
                move || Sample::new(project::inspect(&cwd, deep))
            }),
            maybe_blocking(needs_hardware, &self.slots, {
                let cwd = cwd.clone();
                let cache = self.caches.clone();
                move || {
                    let get = || platform::hardware(&cwd, deep_hardware);
                    if let Some(cache) = cache {
                        cache.hardware.sample(
                            &cwd,
                            deep_hardware,
                            Duration::from_secs(10),
                            refresh,
                            get,
                            |v| v.status == Status::Available,
                        )
                    } else {
                        Sample::new(get())
                    }
                }
            }),
            maybe_blocking(needs_network, &self.slots, {
                let cache = self.caches.clone();
                move || {
                    if let Some(cache) = cache {
                        cache.passive.sample(
                            Path::new(""),
                            false,
                            Duration::from_secs(4),
                            refresh,
                            network::passive,
                            |v| v.status == Status::Available,
                        )
                    } else {
                        Sample::new(network::passive())
                    }
                }
            }),
            async {
                #[cfg(target_os = "macos")]
                if needs_system
                    || requested.is_some_and(|r| {
                        r.iter()
                            .any(|t| matches!(t.0, "git" | "swift" | "gcc" | "clang"))
                    })
                {
                    return Some(platform::mac_developer(&cwd).await);
                }
                None::<(platform::Developer, Option<PathBuf>)>
            },
        );
        let _developer_dir = developer.and_then(|(facts, path)| {
            if let Some(system) = &mut system {
                system.data.developer = Some(facts);
            }
            path
        });
        let relevant_project = if matches!(scope, Scope::Auto | Scope::Dev | Scope::All) {
            project.as_ref().map(|p| &p.data)
        } else {
            None
        };
        let names = tools::relevant(
            relevant_project,
            deep,
            scope == Scope::All,
            requested.unwrap_or_default(),
        );
        let collect_tools = tools::collect(
            &names,
            tools::Collection {
                project: project.as_ref().map(|p| &p.data),
                project_incomplete: needs_project
                    && project
                        .as_ref()
                        .is_none_or(|p| p.data.status == Status::Unknown),
                cwd: &cwd,
                #[cfg(target_os = "macos")]
                developer_dir: _developer_dir.as_deref(),
                slots: Arc::clone(&self.slots),
                refresh,
                budget: if deep || scope == Scope::All {
                    FULL_TOOLS_BUDGET
                } else {
                    BRIEF_TOOLS_BUDGET
                },
            },
            self.caches.as_ref().map(|c| Arc::clone(&c.probes)),
        );
        let (facts, active) = tokio::join!(collect_tools, async {
            if scope == Scope::Network {
                // If passive collection failed, do not try a direct route blindly.
                Some(Sample::new(
                    network::active(
                        network
                            .as_ref()
                            .is_none_or(|n| n.data.proxy == "configured"),
                    )
                    .await,
                ))
            } else {
                None
            }
        });
        let mut warnings = Vec::new();
        for (attempted, status, warning) in [
            (
                needs_system,
                system.as_ref().map(|m| m.data.status),
                "system_incomplete",
            ),
            (
                needs_project,
                project.as_ref().map(|m| m.data.status),
                "project_incomplete",
            ),
            (
                needs_hardware,
                hardware.as_ref().map(|m| m.data.status),
                "hardware_incomplete",
            ),
            (
                needs_network,
                network.as_ref().map(|m| m.data.status),
                "passive_network_incomplete",
            ),
            (
                scope == Scope::Network,
                active.as_ref().map(|m| m.data.status),
                "network_target_check_incomplete",
            ),
        ] {
            if attempted && !matches!(status, Some(Status::Available | Status::NotFound)) {
                warnings.push(warning);
            }
        }
        if facts.values().any(|f| f.status == Status::Timeout) {
            warnings.push("tool_resolution_timed_out");
        }
        if facts
            .values()
            .any(|f| f.outcome == tools::Outcome::Unverified)
        {
            warnings.push("tool_resolution_incomplete");
        }
        let snapshot = Snapshot {
            schema_version: "1",
            sampled_at: model::now(),
            scope,
            detail: request.detail,
            environment_source: "local_process",
            cwd: Cwd {
                value: safe::path(&cwd),
                source,
                status: Status::Available,
            },
            overall_status: if warnings.is_empty() { "ok" } else { "partial" },
            system,
            project,
            hardware,
            network,
            active_network: active,
            tools: facts,
            warnings,
            requested,
        };
        if deep {
            #[derive(Serialize)]
            struct Full<'a> {
                #[serde(flatten)]
                snapshot: &'a Snapshot<'a>,
                #[serde(skip_serializing_if = "Option::is_none")]
                preflight: Option<tools::Preflight<'a>>,
            }
            serde_json::to_string_pretty(&Full {
                snapshot: &snapshot,
                preflight: requested.map(|r| tools::preflight(&snapshot.tools, r)),
            })
            .map_err(|_| "could not render the environment report")
        } else {
            Ok(brief(&snapshot))
        }
    }
}

async fn maybe_blocking<T: Send + 'static>(
    needed: bool,
    slots: &Arc<tokio::sync::Semaphore>,
    run: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    if needed {
        blocking(slots, run).await
    } else {
        None
    }
}

async fn blocking<T: Send + 'static>(
    slots: &Arc<tokio::sync::Semaphore>,
    run: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let permit = slots.clone().try_acquire_owned().ok()?;
    let work = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        run()
    });
    tokio::time::timeout(LOCAL_BUDGET, work).await.ok()?.ok()
}

fn directory(input: Option<PathBuf>) -> Result<PathBuf, &'static str> {
    let directory = match input {
        Some(p) if p.as_os_str().is_empty() || p.as_os_str().len() > 4096 => {
            return Err("cwd must be a nonempty directory path of at most 4096 bytes");
        }
        Some(p) if p.is_absolute() => p,
        relative => {
            let base =
                std::env::current_dir().map_err(|_| "process working directory is unavailable")?;
            relative.map_or(base.clone(), |p| base.join(p))
        }
    };
    #[cfg(windows)]
    if !platform::windows::local(&directory) {
        return Err("cwd must be on a fixed local drive");
    }
    let directory =
        std::fs::canonicalize(directory).map_err(|_| "cwd is missing or inaccessible")?;
    #[cfg(windows)]
    if !platform::windows::local(&directory) {
        return Err("cwd must resolve to a fixed local drive");
    }
    if !directory.is_dir() {
        return Err("cwd must be a directory");
    }
    Ok(directory)
}

fn brief(snapshot: &Snapshot<'_>) -> String {
    let mut lines = Vec::new();
    if let Some(system) = &snapshot.system {
        lines.push(platform_brief::system(&system.data));
    }
    let mut cwd = format!(
        "cwd={} source={}",
        safe::brief_path(&snapshot.cwd.value),
        snapshot.cwd.source
    );
    if let Some(p) = &snapshot.project {
        if !p.data.stacks.is_empty() {
            cwd.push_str(&format!(" project={}", p.data.stacks.join("+")));
        }
        if let Some(manager) = &p.data.package_manager {
            cwd.push_str(&format!(" manager={manager}"));
        }
    }
    lines.push(cwd);
    if !snapshot.tools.is_empty() {
        let items = snapshot
            .tools
            .iter()
            .filter(|(name, _)| {
                !snapshot
                    .requested
                    .is_some_and(|r| r.iter().any(|t| t.0 == **name))
            })
            .map(|(name, f)| format!("{name}:{}", tool_label(name, f, false)))
            .collect::<Vec<_>>();
        if !items.is_empty() {
            lines.push(format!("tools={}", items.join(" ")));
        }
    }
    if let Some(requires) = snapshot.requested {
        let pf = tools::preflight(&snapshot.tools, requires);
        let reqs = pf
            .requirements
            .iter()
            .map(|item| {
                format!(
                    "{}:{}",
                    item.name,
                    tool_label(item.name, item.evidence, true)
                )
            })
            .collect::<Vec<_>>();
        lines.push(if reqs.is_empty() {
            "preflight=ready(no_requirements)".to_owned()
        } else {
            format!("preflight={} items={}", pf.status.label(), reqs.join(","))
        });
    }
    let mut context = Vec::new();
    if let Some(network) = &snapshot.network {
        context.push(format!(
            "proxy={} dns={}",
            network.data.proxy, network.data.dns
        ));
        if network.data.vpn == "tunnel_interface_observed" {
            context.push("tunnel=observed".into());
        }
    }
    if let Some(hardware) = &snapshot.hardware {
        context.push(platform_brief::hardware(&hardware.data));
    }
    if !context.is_empty() {
        lines.push(context.join(" "));
    }
    if let Some(active) = &snapshot.active_network {
        lines.push(format!(
            "example.com dns={} https={} route={}",
            active.data.dns.result, active.data.https.result, active.data.route
        ));
    }
    if !snapshot.warnings.is_empty() {
        lines.push(format!("partial={}", snapshot.warnings.join(",")));
    }
    lines.join("\n")
}

fn tool_label(name: &str, f: &tools::Fact, preflight: bool) -> String {
    let version = f.verified_version.as_deref().unwrap_or("verified");
    let mut text = match f.outcome {
        tools::Outcome::Ready if preflight => format!("ready({version})"),
        tools::Outcome::Ready => version.to_owned(),
        tools::Outcome::Blocked if f.reason_code == "version_mismatch" => {
            format!("blocked(version_mismatch,{version})")
        }
        tools::Outcome::Blocked => format!("blocked({})", f.reason_code),
        tools::Outcome::Unverified => match (&f.verified_version, &f.discovered_version) {
            (Some(v), _) => format!("unverified({},verified={v})", f.reason_code),
            (None, Some(v)) => format!("unverified({},observed={v})", f.reason_code),
            _ => format!("unverified({})", f.reason_code),
        },
    };
    if f.outcome != tools::Outcome::Ready
        && let Some(requirement) = &f.applicable_requirement
    {
        text.push_str(&format!("[want={}]", requirement.replace(' ', "")));
    }
    if let Some(invocation) = &f.invocation_name
        && invocation.strip_suffix(".exe").unwrap_or(invocation) != name
    {
        text.push_str(&format!("[run={invocation}]"));
    }
    text
}

mod platform_brief {
    use super::*;
    pub(super) fn system(data: &platform::System) -> String {
        let mut text = format!(
            "os={}/{} exec={} shell=unknown access={}",
            data.os, data.process_arch, data.execution, data.privilege
        );
        if data.cwd_access == "not_writable_estimate" {
            text.push_str(" cwd_write=denied_estimate");
        }
        if data.ssh_indicator {
            text.push_str(" ssh=indicator");
        }
        if data.ci_indicator {
            text.push_str(" ci=indicator");
        }
        if let Some(dev) = &data.developer
            && dev.kind != "unknown"
        {
            text.push_str(&format!(" developer={}", dev.kind));
        }
        text
    }
    pub(super) fn hardware(data: &platform::Hardware) -> String {
        let cpus = data
            .effective_cpus
            .map(|v| {
                format!("{v:.2}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_owned()
            })
            .unwrap_or_else(|| "unknown".to_owned());
        let mem = data
            .effective_memory_bytes
            .map(|v| format!("{}MiB", v / (1024 * 1024)))
            .unwrap_or_else(|| "unknown".to_owned());
        let mut text = format!("cpu={cpus} ram={mem}");
        if let Some(bytes) = data.storage_available_bytes {
            text.push_str(&format!(" disk_free={}MiB", bytes / (1024 * 1024)));
        }
        if data.gpu_evidence != "not_requested" {
            text.push_str(&format!(" gpu={}", data.gpu_evidence));
        }
        text
    }
}
