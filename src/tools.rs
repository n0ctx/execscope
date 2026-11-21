use crate::{
    lookup::{Candidate, Chain, Search},
    managed::{self, Kind, Managed},
    model::{Status, Tool},
    probe::{self, Probe},
    project::{Project, Requirement},
    safe,
    versions::{self, Comparison},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::task::JoinSet;

const PROBE_LIMIT: Duration = Duration::from_millis(300);
const CACHE_LIMIT: usize = 128;

type ProbeKey = (&'static str, PathBuf, PathBuf, Stamp);
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Stamp(u64, Option<std::time::SystemTime>, u64, u64, u64, u64);

impl Stamp {
    fn new(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(Self(
                m.len(),
                m.modified().ok(),
                m.dev(),
                m.ino(),
                m.ctime() as u64,
                m.ctime_nsec() as u64,
            ))
        }
        #[cfg(not(unix))]
        {
            Some(Self(m.len(), m.modified().ok(), 0, 0, 0, 0))
        }
    }
}

#[derive(Default)]
pub(crate) struct ProbeCache {
    entries: Mutex<HashMap<ProbeKey, (Instant, String, Probe)>>,
}

impl ProbeCache {
    async fn verify(
        &self,
        name: &'static str,
        path: &Path,
        cwd: &Path,
        refresh: bool,
    ) -> (Probe, String, bool) {
        let key = Stamp::new(path).map(|stamp| (name, path.to_owned(), cwd.to_owned(), stamp));
        if let Some(key) = &key
            && !refresh
            && let Some((_, at, probe)) = self
                .entries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(key)
                .filter(|(seen, _, _)| seen.elapsed() < Duration::from_secs(30))
        {
            return (probe.clone(), at.clone(), true);
        }
        let probe = probe::verify(name, path, cwd, PROBE_LIMIT).await;
        let at = crate::model::now();
        // Project/PATH/manager selection is resolved again on every request; only a
        // positive identity of an unchanged concrete executable is cached.
        if let Some(key) = key
            && probe.status == Status::Available
            && Stamp::new(path).as_ref() == Some(&key.3)
        {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if entries.len() >= CACHE_LIMIT {
                entries.clear();
            }
            entries.insert(key, (Instant::now(), at.clone(), probe.clone()));
        }
        (probe, at, false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Ready,
    Blocked,
    Unverified,
}
impl Outcome {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Blocked => "blocked",
            Self::Unverified => "unverified",
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct Fact {
    pub(crate) status: Status,
    pub(crate) stage: &'static str,
    pub(crate) selection_context: &'static str,
    pub(crate) source: &'static str,
    pub(crate) reason_code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) verified_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) discovered_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) selected_invocation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) invocation_name: Option<String>,
    pub(crate) comparison: Comparison,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) applicable_requirement: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) declarations: Vec<Requirement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chain: Option<Chain>,
    #[serde(skip_serializing_if = "Option::is_none")]
    managed: Option<Managed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe: Option<Probe>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe_sampled_at: Option<String>,
    #[serde(skip_serializing_if = "crate::model::is_false")]
    probe_cached: bool,
    #[serde(skip)]
    pub(crate) outcome: Outcome,
}

#[derive(Debug, Serialize)]
pub(crate) struct Preflight<'a> {
    pub(crate) status: Outcome,
    requested: &'a [Tool],
    pub(crate) requirements: Vec<Item<'a>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Item<'a> {
    pub(crate) name: &'static str,
    pub(crate) outcome: Outcome,
    #[serde(flatten)]
    pub(crate) evidence: &'a Fact,
}

pub(crate) fn preflight<'a>(
    facts: &'a BTreeMap<&'static str, Fact>,
    requested: &'a [Tool],
) -> Preflight<'a> {
    let requirements: Vec<_> = requested
        .iter()
        .map(|&Tool(name)| {
            let evidence = &facts[name];
            Item {
                name,
                outcome: evidence.outcome,
                evidence,
            }
        })
        .collect();
    let status = if requirements
        .iter()
        .any(|req| req.outcome == Outcome::Blocked)
    {
        Outcome::Blocked
    } else if requirements.iter().all(|req| req.outcome == Outcome::Ready) {
        Outcome::Ready
    } else {
        Outcome::Unverified
    };
    Preflight {
        status,
        requested,
        requirements,
    }
}

pub(crate) fn relevant(
    project: Option<&Project>,
    full: bool,
    all: bool,
    requires: &[Tool],
) -> Vec<&'static str> {
    let mut names = Vec::new();
    for &Tool(name) in requires {
        push(&mut names, name);
    }
    if all {
        for name in crate::model::TOOLS {
            push(&mut names, name);
        }
    }
    if let Some(project) = project {
        push(&mut names, "git");
        for stack in &project.stacks {
            match *stack {
                "node" | "js" | "javascript" | "typescript" | "nodejs" => {
                    push(&mut names, "node");
                    let manager = project
                        .package_manager
                        .as_deref()
                        .and_then(|s| s.split('@').next())
                        .and_then(|s| {
                            ["npm", "yarn", "pnpm", "bun"]
                                .into_iter()
                                .find(|name| *name == s)
                        })
                        .unwrap_or("npm");
                    push(&mut names, manager);
                }
                "python" => {
                    push(&mut names, "python");
                    push(&mut names, "uv");
                    if full {
                        push(&mut names, "pip");
                    }
                }
                "rust" => {
                    push(&mut names, "cargo");
                    push(&mut names, "rustc");
                }
                "go" => push(&mut names, "go"),
                "swift" => push(&mut names, "swift"),
                "java" | "jvm" | "kotlin" => {
                    push(&mut names, "java");
                    if full {
                        push(&mut names, "javac");
                    }
                }
                "dotnet" | ".net" => push(&mut names, "dotnet"),
                _ => (),
            }
        }
        for name in crate::model::TOOLS {
            if project.requirements(name).next().is_some() {
                push(&mut names, name);
            }
        }
    }
    names
}

fn push(names: &mut Vec<&'static str>, name: &'static str) {
    if !names.contains(&name) {
        names.push(name);
    }
}

pub(crate) struct Collection<'a> {
    pub(crate) project: Option<&'a Project>,
    pub(crate) project_incomplete: bool,
    pub(crate) cwd: &'a Path,
    #[cfg(target_os = "macos")]
    pub(crate) developer_dir: Option<&'a Path>,
    pub(crate) slots: std::sync::Arc<tokio::sync::Semaphore>,
    pub(crate) refresh: bool,
    pub(crate) budget: Duration,
}

#[derive(Clone)]
struct Job {
    name: &'static str,
    search: Search,
    project: Option<std::sync::Arc<Project>>,
    project_incomplete: bool,
    cwd: PathBuf,
    #[cfg(target_os = "macos")]
    developer_dir: Option<PathBuf>,
    slots: std::sync::Arc<tokio::sync::Semaphore>,
    cache: Option<std::sync::Arc<ProbeCache>>,
    refresh: bool,
}

pub(crate) async fn collect(
    names: &[&'static str],
    config: Collection<'_>,
    cache: Option<std::sync::Arc<ProbeCache>>,
) -> BTreeMap<&'static str, Fact> {
    let Collection {
        project,
        project_incomplete,
        cwd,
        #[cfg(target_os = "macos")]
        developer_dir,
        slots,
        refresh,
        budget,
    } = config;
    let job = Job {
        name: "",
        search: Search::process(),
        project: project.cloned().map(std::sync::Arc::new),
        project_incomplete,
        cwd: cwd.to_owned(),
        #[cfg(target_os = "macos")]
        developer_dir: developer_dir.map(Path::to_owned),
        slots,
        cache,
        refresh,
    };
    let mut pending = names.iter().copied();
    let mut tasks = JoinSet::new();
    let mut result = BTreeMap::new();
    let spawn = |tasks: &mut JoinSet<_>, name| {
        let mut job = job.clone();
        job.name = name;
        tasks.spawn(async move { (name, resolve(job).await) });
    };
    for name in pending.by_ref().take(5) {
        spawn(&mut tasks, name);
    }
    let until = tokio::time::Instant::now() + budget;
    loop {
        match tokio::time::timeout_at(until, tasks.join_next()).await {
            Ok(Some(Ok((name, fact)))) => {
                result.insert(name, fact);
            }
            Ok(Some(Err(_))) => (),
            Ok(None) | Err(_) => break,
        }
        if let Some(name) = pending.next() {
            spawn(&mut tasks, name);
        }
    }
    tasks.shutdown().await;
    for name in names {
        result.entry(name).or_insert_with(timeout_fact);
    }
    result
}

async fn resolve(job: Job) -> Fact {
    let Job {
        name,
        ref cwd,
        ref cache,
        refresh,
        project_incomplete,
        ..
    } = job;
    let Ok(permit) = job.slots.clone().acquire_owned().await else {
        return timeout_fact();
    };
    let work = job.clone();
    let Ok((chain, managed, declarations)) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let declarations: Vec<_> = work
            .project
            .as_ref()
            .map(|p| p.requirements(name).cloned().collect())
            .unwrap_or_default();
        let roots: Vec<_> = work
            .project
            .as_ref()
            .map(|p| p.roots().collect())
            .unwrap_or_default();
        let chain = work.search.find(name, &work.cwd);
        let selected = chain.candidates.iter().find(|c| c.selected);
        let kind = selected.map(|c| c.kind);
        let mut managed = managed::enrich(name, &work.cwd, kind, &declarations, &roots);
        if let Some(candidate) = selected {
            managed::enrich_selected(
                &mut managed,
                name,
                &candidate.path,
                candidate.target.as_deref(),
                candidate.kind,
            );
        }
        #[cfg(target_os = "macos")]
        if kind == Some(Kind::XcodeStub) {
            managed::xcode(&mut managed, name, work.developer_dir.as_deref());
        }
        (chain, managed, declarations)
    })
    .await
    else {
        return timeout_fact();
    };
    let selected = chain.candidates.iter().find(|candidate| candidate.selected);
    let constraints = applicable(name, selected.map(|c| c.kind), &declarations);
    let selection = choose(selected, &managed);
    let mut probe = None;
    let mut probe_sampled_at = None;
    let mut probe_cached = false;
    if let Some(path) = selection.target {
        let Ok(_permit) = job.slots.clone().acquire_owned().await else {
            return timeout_fact();
        };
        if let Some(cache) = cache {
            let p = cache.verify(name, path, cwd, refresh).await;
            probe = Some(p.0);
            probe_sampled_at = Some(p.1);
            probe_cached = p.2;
        } else {
            probe = Some(probe::verify(name, path, cwd, PROBE_LIMIT).await);
            probe_sampled_at = Some(crate::model::now());
        }
    }
    let verified = probe.as_ref().filter(|p| p.status == Status::Available);
    let version = verified.and_then(|p| p.version.as_deref());
    let (comparison, applicable_requirement) = assess(version, &constraints);
    let (outcome, status, reason_code) = if let Some(blocker) = selection.blocker {
        (Outcome::Blocked, Status::Unknown, blocker)
    } else if let Some(verified) = verified {
        match comparison {
            Comparison::Mismatch => (Outcome::Blocked, Status::Available, "version_mismatch"),
            Comparison::Unverified => (
                Outcome::Unverified,
                Status::Available,
                "version_requirement_unverified",
            ),
            _ if project_incomplete && name != "git" => (
                Outcome::Unverified,
                Status::Available,
                "project_evidence_incomplete",
            ),
            _ if chain.earlier_selection_unverified => (
                Outcome::Unverified,
                Status::Available,
                "path_lookup_incomplete",
            ),
            _ => (Outcome::Ready, Status::Available, verified.reason),
        }
    } else if selected.is_none() {
        if job.search.unavailable || chain.inaccessible || chain.limited {
            (
                Outcome::Unverified,
                Status::Unknown,
                "path_lookup_incomplete",
            )
        } else if !managed.alternatives.is_empty()
            || chain.candidates.iter().any(|c| c.kind == Kind::Foreign)
        {
            (Outcome::Blocked, Status::NotFound, "runtime_not_selected")
        } else if !chain.candidates.is_empty() {
            (Outcome::Blocked, Status::NotFound, "not_executable")
        } else {
            (Outcome::Blocked, Status::NotFound, "not_found_in_context")
        }
    } else if let Some(probe) = &probe {
        (Outcome::Unverified, probe.status, probe.reason)
    } else if selected.is_some_and(|c| c.kind == Kind::PythonEnvironment) {
        (
            Outcome::Unverified,
            Status::Unknown,
            "selected_python_environment_not_probed",
        )
    } else {
        (
            Outcome::Unverified,
            Status::Unknown,
            "managed_or_wrapper_not_verified",
        )
    };
    let stage = if verified.is_some() {
        "verified"
    } else if selected.is_some() {
        "selected"
    } else if !chain.candidates.is_empty() || !managed.alternatives.is_empty() {
        "discovered"
    } else if !declarations.is_empty() {
        "declared"
    } else {
        "not_found"
    };
    let source = verified.map(|p| p.source).unwrap_or(selection.source);
    let discovered_version = managed.discovered_version.clone();
    #[cfg(windows)]
    let discovered_version =
        discovered_version.or_else(|| probe.as_ref().and_then(|p| p.file_version.clone()));
    Fact {
        status,
        stage,
        source,
        selection_context: selection.source,
        reason_code,
        verified_version: version.map(str::to_owned),
        discovered_version,
        selected_invocation: selected.map(|c| safe::path(&c.path)),
        invocation_name: selected.map(|c| c.invocation.clone()),
        comparison,
        applicable_requirement,
        declarations,
        chain: Some(chain),
        managed: Some(managed),
        probe,
        probe_sampled_at,
        probe_cached,
        outcome,
    }
}

// Separate actual manager selection from declarations applicable to the prerequisite.
fn applicable<'a>(
    name: &str,
    kind: Option<Kind>,
    declarations: &'a [Requirement],
) -> Vec<&'a Requirement> {
    let all = || declarations.iter().filter(|r| r.effective);
    let mut out = all()
        .filter(|r| matches!(r.kind, "constraint" | "manager_pin" | "sdk_selector"))
        .collect::<Vec<_>>();
    let pins: &[&str] = match kind {
        Some(Kind::Asdf) => &["asdf"],
        Some(Kind::Mise) => &["mise", "asdf"],
        Some(Kind::Pyenv) => &["python_pin"],
        Some(Kind::Volta) => &["volta"],
        Some(Kind::Rustup) => &[],
        _ if name == "node" => &["node_pin", "fnm_pin", "volta", "mise", "asdf"],
        _ if name == "python" => &["python_pin"],
        _ if matches!(name, "rustc" | "cargo") => &["rustup"],
        _ => &[],
    };
    if let Some(pin) = pins.iter().find_map(|pin| all().find(|r| r.kind == *pin)) {
        out.push(pin);
    }
    out
}

fn assess(version: Option<&str>, constraints: &[&Requirement]) -> (Comparison, Option<String>) {
    let mut state = (Comparison::NoConstraint, None);
    for req in constraints {
        let check = if req.kind == "sdk_selector" {
            Comparison::Unverified
        } else {
            let syntax = match req.source {
                "package.json:engines" => versions::Syntax::Npm,
                "pyproject.toml:requires-python" => versions::Syntax::Python,
                _ => versions::Syntax::Pin,
            };
            versions::compare(version, Some(&req.value), syntax)
        };
        if check == Comparison::Mismatch {
            return (check, Some(req.value.clone()));
        }
        if state.0 == Comparison::NoConstraint || check == Comparison::Unverified {
            state = (check, Some(req.value.clone()));
        }
    }
    state
}

struct Selection<'a> {
    target: Option<&'a Path>,
    source: &'static str,
    blocker: Option<&'static str>,
}
fn choose<'a>(selected: Option<&'a Candidate>, managed: &'a Managed) -> Selection<'a> {
    let Some(found) = selected else {
        return Selection {
            target: None,
            source: "process_path_lookup",
            blocker: None,
        };
    };
    let (target, source, blocker) = match found.kind {
        Kind::WindowsAlias => (None, "windows_app_alias", Some("app_alias_only")),
        Kind::Foreign => (None, "foreign_binary", Some("runtime_not_selected")),
        Kind::Direct => (found.target.as_deref(), "process_path", None),
        Kind::Wrapper => (None, "process_path_wrapper", None),
        _ => (managed.selected.as_deref(), managed.source, managed.blocker),
    };
    Selection {
        target,
        source,
        blocker,
    }
}

fn timeout_fact() -> Fact {
    Fact {
        status: Status::Timeout,
        stage: "not_checked",
        selection_context: "inspection_budget",
        source: "inspection_budget",
        reason_code: "deadline_exhausted",
        verified_version: None,
        discovered_version: None,
        selected_invocation: None,
        invocation_name: None,
        comparison: Comparison::Unverified,
        applicable_requirement: None,
        declarations: vec![],
        chain: None,
        managed: None,
        probe: None,
        probe_sampled_at: None,
        probe_cached: false,
        outcome: Outcome::Unverified,
    }
}
