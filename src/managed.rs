//! Inspect manager metadata without asking a manager to activate, trust, sync, or install.
mod selectors;

use crate::{project::Requirement, safe};
use serde::Serialize;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Direct,
    Wrapper,
    WindowsAlias,
    Foreign,
    Rustup,
    Corepack,
    Mise,
    Asdf,
    Pyenv,
    Volta,
    PythonEnvironment,
    XcodeStub,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Managed {
    #[serde(serialize_with = "opt_path")]
    pub(crate) selected: Option<PathBuf>,
    #[serde(serialize_with = "paths")]
    pub(crate) alternatives: Vec<PathBuf>,
    pub(crate) blocker: Option<&'static str>,
    pub(crate) source: &'static str,
    pub(crate) evidence: Vec<&'static str>,
    pub(crate) requested: Option<String>,
    pub(crate) discovered_version: Option<String>,
    pub(crate) version_source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    uv_project: Option<selectors::Context>,
}
fn opt_path<S: serde::Serializer>(p: &Option<PathBuf>, s: S) -> Result<S::Ok, S::Error> {
    match p {
        Some(p) => s.serialize_some(&safe::path(p)),
        None => s.serialize_none(),
    }
}
fn paths<S: serde::Serializer>(p: &[PathBuf], s: S) -> Result<S::Ok, S::Error> {
    p.iter()
        .map(|p| safe::path(p))
        .collect::<Vec<_>>()
        .serialize(s)
}

pub(crate) fn classify(name: &str, path: &Path, target: Option<&Path>) -> Kind {
    let contains = |p: &Path, needle: &str| {
        p.to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase()
            .contains(needle)
    };
    let has = |needle| contains(path, needle) || target.is_some_and(|t| contains(t, needle));
    if has("/windowsapps/")
        && path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("exe"))
    {
        return Kind::WindowsAlias;
    }
    if cfg!(target_os = "macos")
        && matches!(name, "git" | "swift" | "clang" | "gcc")
        && target.unwrap_or(path).parent() == Some(Path::new("/usr/bin"))
    {
        return Kind::XcodeStub;
    }
    if matches!(name, "rustc" | "cargo")
        && (has("/.cargo/bin/")
            || target
                .and_then(Path::file_stem)
                .is_some_and(|s| s == "rustup"))
    {
        return Kind::Rustup;
    }
    if matches!(name, "npm" | "pnpm" | "yarn") && has("/corepack/") {
        return Kind::Corepack;
    }
    for (needle, k) in [
        ("/mise/shims/", Kind::Mise),
        ("/asdf/shims/", Kind::Asdf),
        ("/pyenv/shims/", Kind::Pyenv),
        ("/.pyenv/shims/", Kind::Pyenv),
        ("/volta/bin/", Kind::Volta),
        ("/.volta/bin/", Kind::Volta),
    ] {
        if has(needle) {
            return k;
        }
    }
    for (key, subpath, kind) in [
        ("MISE_DATA_DIR", "shims", Kind::Mise),
        ("ASDF_DATA_DIR", "shims", Kind::Asdf),
        ("PYENV_ROOT", "shims", Kind::Pyenv),
        ("VOLTA_HOME", "bin", Kind::Volta),
    ] {
        if env_dir(key).is_some_and(|p| path.starts_with(p.join(subpath))) {
            return kind;
        }
    }
    if target.unwrap_or(path).extension().is_some_and(|extension| {
        ["cmd", "bat", "ps1", "js", "cjs", "sh", "py"]
            .iter()
            .any(|e| extension.eq_ignore_ascii_case(e))
    }) {
        return Kind::Wrapper;
    }
    if name == "python"
        && path
            .file_stem()
            .is_some_and(|s| s.eq_ignore_ascii_case("py"))
    {
        return Kind::Wrapper;
    }
    if name == "python"
        && (["VIRTUAL_ENV", "CONDA_PREFIX"]
            .iter()
            .any(|key| env_dir(key).is_some_and(|p| path.starts_with(p)))
            || path
                .parent()
                .and_then(Path::parent)
                .is_some_and(|root| root.join("pyvenv.cfg").is_file()))
    {
        return Kind::PythonEnvironment;
    }
    Kind::Direct
}

fn env_dir(key: &str) -> Option<PathBuf> {
    env::var_os(key).map(PathBuf::from).filter(|p| {
        #[cfg(windows)]
        if !crate::platform::windows::local(p) {
            return false;
        }
        p.is_absolute() && p.as_os_str().len() <= 4096
    })
}
fn home() -> Option<PathBuf> {
    env_dir(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
}
fn store(key: &str, sub: &str) -> Option<PathBuf> {
    env_dir(key).or_else(|| home().map(|h| h.join(sub)))
}
fn executable(p: &Path) -> bool {
    crate::lookup::executable(
        p,
        if cfg!(windows) { "tool.exe" } else { "tool" },
        cfg!(windows),
    )
}
fn file(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}
fn component(s: &str) -> Option<&str> {
    (!s.is_empty()
        && s.len() <= 96
        && s.bytes().next()?.is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')))
    .then_some(s)
}
fn declaration<'a>(reqs: &'a [Requirement], kind: &str) -> Option<&'a str> {
    reqs.iter()
        .find(|r| r.kind == kind && r.effective)
        .map(|r| r.value.as_str())
}

pub(crate) fn enrich(
    name: &str,
    cwd: &Path,
    kind: Option<Kind>,
    reqs: &[Requirement],
    project_roots: &[&Path],
    python_root: Option<&Path>,
) -> Managed {
    let mut r = Managed {
        selected: None,
        alternatives: vec![],
        blocker: None,
        source: "bounded_static_evidence",
        evidence: vec![],
        requested: None,
        discovered_version: None,
        version_source: None,
        uv_project: None,
    };
    let dirs = std::iter::once(cwd)
        .chain(project_roots.iter().copied())
        .take(24);
    if matches!(name, "rustc" | "cargo")
        && let Some(root) = store("RUSTUP_HOME", ".rustup")
    {
        let explicit = env::var("RUSTUP_TOOLCHAIN").ok();
        if rustup(
            &mut r,
            &root,
            cwd,
            explicit.as_deref(),
            name,
            kind == Some(Kind::Rustup),
        ) {
            r.source = "rustup_selected_toolchain";
        }
    }
    if name == "python" {
        if kind == Some(Kind::PythonEnvironment) {
            r.source = "python_environment_path";
            r.note("selected_environment_not_replaced_by_base_interpreter");
        }
        for (key, source) in [
            ("VIRTUAL_ENV", "VIRTUAL_ENV_present"),
            ("CONDA_PREFIX", "CONDA_PREFIX_present"),
        ] {
            if let Some(root) = env_dir(key) {
                r.note(source);
                python_env(&mut r, &root, "active_env_candidate");
            }
        }
        if let Some(root) = python_root {
            let configured = env::var_os("UV_PROJECT_ENVIRONMENT").filter(|s| s.len() <= 4096);
            let no_project =
                env::var("UV_NO_PROJECT").is_ok_and(|s| matches!(s.as_str(), "1" | "true"));
            let context = selectors::uv_project(root, configured.as_deref(), no_project);
            if let Some(path) = &context.executable {
                r.alternate(path.clone());
            }
            r.uv_project = Some(context);
        }
        if let Ok(python) = env::var("UV_PYTHON") {
            if let Some(path) = env_dir("UV_PYTHON") {
                r.note("UV_PYTHON_path_present");
                r.alternate(path);
            } else if !python.is_empty() {
                r.note("UV_PYTHON_version_request_present");
            }
        }
        for dir in dirs.clone() {
            let venv = dir.join(".venv");
            if venv.is_dir() {
                python_env(
                    &mut r,
                    &venv,
                    if dir.join("uv.lock").is_file() {
                        "uv_project_venv_candidate"
                    } else {
                        "project_venv_candidate"
                    },
                );
            }
        }
        let pyenv_version = env::var("PYENV_VERSION").ok();
        if let Some(pyenv_root) = store("PYENV_ROOT", ".pyenv") {
            let global = safe::read(&pyenv_root.join("version"), 4096);
            let wanted = pyenv_version
                .as_deref()
                .or_else(|| declaration(reqs, "python_pin"))
                .or(global.as_deref().map(str::trim));
            if let Some(v) = wanted.and_then(component) {
                let candidate = pyenv_root
                    .join("versions")
                    .join(v)
                    .join("bin")
                    .join(file(name));
                r.observe(
                    candidate,
                    kind == Some(Kind::Pyenv),
                    "pyenv_version_selection",
                    v,
                );
            }
        }
        if let Some(uv) = store("UV_PYTHON_INSTALL_DIR", ".local/share/uv/python") {
            add_installed(&mut r, &uv, "bin", name, "uv_python_store_observed");
        }
    }
    if name == "node" {
        if let Some(root) = store("NVM_DIR", ".nvm") {
            add_installed(
                &mut r,
                &root.join("versions/node"),
                "bin",
                name,
                "nvm_installed_node_not_shell_selection",
            );
        }
        if let Some(root) = env_dir("FNM_MULTISHELL_PATH") {
            r.alternate(root.join("bin").join(file(name)));
            r.note("fnm_multishell_path_present");
        }
        if let Some(root) = store("FNM_DIR", ".local/share/fnm") {
            add_installed(
                &mut r,
                &root.join("node-versions"),
                "installation/bin",
                name,
                "fnm_installed_node",
            );
        }
        if let Some(root) = store("VOLTA_HOME", ".volta") {
            let image = root.join("tools/image/node");
            let default = safe::read(&root.join("tools/user/platform.json"), 24 * 1024)
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
            let pin = declaration(reqs, "volta")
                .or_else(|| default.as_ref().and_then(|v| v["node"]["runtime"].as_str()));
            if let Some(pin) = pin.and_then(component) {
                r.observe(
                    image.join(pin).join("bin").join(file(name)),
                    kind == Some(Kind::Volta),
                    "volta_project_or_default",
                    pin,
                );
            }
            add_installed(&mut r, &image, "bin", name, "volta_installed_node");
        }
    }
    if let Some(root) = store("ASDF_DATA_DIR", ".asdf") {
        let tool = if name == "node" {
            "nodejs"
        } else if name == "go" {
            "golang"
        } else {
            name
        };
        if kind == Some(Kind::Asdf) || declaration(reqs, "asdf").is_some() {
            let key = format!("ASDF_{}_VERSION", tool.to_ascii_uppercase());
            let filename =
                env::var("ASDF_TOOL_VERSIONS_FILENAME").unwrap_or_else(|_| ".tool-versions".into());
            let request = if let Ok(value) = env::var(key) {
                Ok(Some(value))
            } else {
                selectors::asdf_file(cwd, home().as_deref(), &filename, tool)
            };
            match request {
                Ok(Some(pin)) => match selectors::asdf(&root, tool, name, &pin) {
                    Ok((path, version)) => r.observe(
                        path,
                        kind == Some(Kind::Asdf),
                        "asdf_project_or_environment_selection",
                        &version,
                    ),
                    Err(reason) => r.note(reason),
                },
                Ok(None) => r.note("asdf_no_configured_selector_observed"),
                Err(reason) => r.note(reason),
            }
        }
    }
    if let Some(root) = store("MISE_DATA_DIR", ".local/share/mise") {
        let tool = if name == "rustc" || name == "cargo" {
            "rust"
        } else {
            name
        };
        if kind == Some(Kind::Mise) || declaration(reqs, "mise").is_some() {
            let key = format!("MISE_{}_VERSION", tool.to_ascii_uppercase());
            let global = env_dir("MISE_CONFIG_DIR")
                .or_else(|| env_dir("XDG_CONFIG_HOME").map(|p| p.join("mise")))
                .or_else(|| home().map(|p| p.join(".config/mise")));
            let request = if let Ok(value) = env::var(key) {
                Ok(Some((value, "mise_environment_tool_version")))
            } else if [
                "MISE_GLOBAL_CONFIG_FILE",
                "MISE_OVERRIDE_CONFIG_FILENAMES",
                "MISE_OVERRIDE_TOOL_VERSIONS_FILENAMES",
                "MISE_CEILING_PATHS",
                "MISE_IGNORED_CONFIG_PATHS",
            ]
            .iter()
            .any(|k| env::var_os(k).is_some())
            {
                Err("mise_config_discovery_override_unverified")
            } else {
                selectors::mise_request(
                    cwd,
                    global.as_deref(),
                    &env::var("MISE_ENV").unwrap_or_default(),
                    tool,
                )
            };
            if let Ok(Some((pin, pin_source))) = &request {
                match selectors::mise_install(&root, tool, name, pin) {
                    Ok((path, version)) => {
                        r.alternate(path);
                        r.note("mise_matching_install_observed");
                        r.note(pin_source);
                        if kind == Some(Kind::Mise) {
                            r.source = "mise_selector_metadata";
                            r.requested = Some(crate::project::constraint(pin));
                            r.record_version(
                                &version,
                                "mise_matching_install_not_verified_selected",
                            );
                            r.note("mise_dynamic_selection_not_replayed");
                        }
                    }
                    Err(reason) => r.note(reason),
                }
            } else {
                r.note(
                    request
                        .err()
                        .unwrap_or("mise_no_supported_selector_observed"),
                );
            }
        }
    }
    if matches!(name, "npm" | "pnpm" | "yarn") {
        for d in dirs {
            r.alternate(d.join("node_modules/.bin").join(if cfg!(windows) {
                format!("{name}.cmd")
            } else {
                name.into()
            }));
        }
        if kind == Some(Kind::Corepack) {
            r.source = "corepack_static_cache";
            let dir = env_dir("COREPACK_HOME").or_else(|| {
                if cfg!(windows) {
                    env_dir("LOCALAPPDATA").map(|d| d.join("node/corepack"))
                } else {
                    env_dir("XDG_CACHE_HOME")
                        .map(|d| d.join("node/corepack"))
                        .or_else(|| home().map(|h| h.join(".cache/node/corepack")))
                }
            });
            let pin =
                declaration(reqs, "manager_pin").filter(|v| semver::Version::parse(v).is_ok());
            if let (Some(root), Some(pin)) = (dir, pin) {
                corepack(&mut r, &root, name, pin);
            } else {
                r.note("corepack_pin_or_cache_location_unverified");
            }
        }
    }
    #[cfg(windows)]
    windows_alternates(&mut r, name);
    r
}

impl Managed {
    fn note(&mut self, evidence: &'static str) {
        if !self.evidence.contains(&evidence) {
            self.evidence.push(evidence);
        }
    }
    fn alternate(&mut self, path: PathBuf) {
        if self.alternatives.len() < 12 && !self.alternatives.contains(&path) && executable(&path) {
            self.alternatives.push(path);
        }
    }
    fn observe(&mut self, path: PathBuf, governing: bool, source: &'static str, pin: &str) {
        if !executable(&path) {
            if governing {
                self.note("declared_manager_target_not_observed");
            }
            return;
        }
        self.alternate(path.clone());
        self.note(source);
        if governing {
            self.selected = fs::canonicalize(path).ok();
            self.source = source;
            self.requested = Some(crate::project::constraint(pin));
            self.record_version(pin, "selected_manager_directory");
        }
    }

    fn record_version(&mut self, version: &str, source: &'static str) {
        let version = version.strip_prefix('v').unwrap_or(version);
        if version.len() <= 32
            && semver::Version::parse(version).is_ok_and(|v| v.pre.is_empty() && v.build.is_empty())
        {
            self.discovered_version = Some(version.to_owned());
            self.version_source = Some(source);
        }
    }
}

pub(crate) fn enrich_selected(
    r: &mut Managed,
    name: &str,
    entry: &Path,
    target: Option<&Path>,
    kind: Kind,
) {
    if name == "python"
        && kind == Kind::PythonEnvironment
        && let Some(root) = entry.parent().and_then(Path::parent)
        && let Some(config) = safe::read(&root.join("pyvenv.cfg"), 8 * 1024)
        && let Some(version) = selectors::pyvenv_version(&config)
    {
        r.record_version(&version, "selected_pyvenv_cfg");
    }
    if name != "node" || kind != Kind::Direct {
        return;
    }
    let Some(target) = target else {
        return;
    };
    for (key, fallback, prefix, source) in [
        ("NVM_DIR", ".nvm", "versions/node", "nvm_node_path"),
        (
            "FNM_DIR",
            ".local/share/fnm",
            "node-versions",
            "fnm_node_path",
        ),
        (
            "VOLTA_HOME",
            ".volta",
            "tools/image/node",
            "volta_node_path",
        ),
        (
            "ASDF_DATA_DIR",
            ".asdf",
            "installs/nodejs",
            "asdf_node_path",
        ),
        (
            "MISE_DATA_DIR",
            ".local/share/mise",
            "installs/node",
            "mise_node_path",
        ),
    ] {
        let Some(base) = store(key, fallback).and_then(|p| p.join(prefix).canonicalize().ok())
        else {
            continue;
        };
        let Ok(relative) = target.strip_prefix(&base) else {
            continue;
        };
        if let Some(version) = relative
            .components()
            .next()
            .and_then(|part| part.as_os_str().to_str())
        {
            r.record_version(version, source);
            if r.discovered_version.is_some() {
                r.note(source);
                return;
            }
        }
    }
}

fn add_installed(r: &mut Managed, root: &Path, bin: &str, name: &str, source: &'static str) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries
        .take(25)
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    if entries.len() > 24 {
        r.note("manager_store_enumeration_limited");
        return;
    }
    entries.sort();
    let prior = r.alternatives.len();
    for dir in entries {
        r.alternate(dir.join(bin).join(file(name)));
    }
    if r.alternatives.len() > prior {
        r.note(source);
    }
}

fn python_env(r: &mut Managed, root: &Path, source: &'static str) {
    r.note(source);
    if root.join("pyvenv.cfg").is_file() {
        r.note("pyvenv_cfg_present");
    }
    let candidate = root.join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    });
    if executable(&candidate) {
        r.alternate(candidate);
    } else if cfg!(windows) && executable(&root.join("python.exe")) {
        r.alternate(root.join("python.exe"));
    } else {
        r.note("environment_interpreter_missing_or_non_executable");
    }
}

fn rustup(
    r: &mut Managed,
    home: &Path,
    cwd: &Path,
    override_pin: Option<&str>,
    name: &str,
    governing: bool,
) -> bool {
    let settings = safe::read(&home.join("settings.toml"), 32 * 1024)
        .and_then(|s| s.parse::<toml::Table>().ok());
    let overrides = settings
        .as_ref()
        .and_then(|s| s.get("overrides"))
        .and_then(toml::Value::as_table);
    let mut pin = override_pin.map(str::to_owned);
    if pin.is_some() {
        r.note("RUSTUP_TOOLCHAIN_override");
    }
    if pin.is_none() {
        for dir in cwd.ancestors().take(32) {
            if let Some(candidate) = overrides
                .and_then(|o| o.get(dir.to_str()?))
                .and_then(toml::Value::as_str)
            {
                pin = Some(candidate.to_owned());
                r.note("rustup_directory_override");
                break;
            }
            for filename in ["rust-toolchain", "rust-toolchain.toml"] {
                let path = dir.join(filename);
                if matches!(fs::symlink_metadata(&path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
                {
                    continue;
                }
                let Some(raw) = fs::canonicalize(path)
                    .ok()
                    .filter(|p| p.starts_with(dir))
                    .and_then(|p| safe::read(&p, 24 * 1024))
                else {
                    r.note("rustup_project_file_unreadable");
                    return false;
                };
                if let Some(toolchain) = raw
                    .parse::<toml::Table>()
                    .ok()
                    .and_then(|mut t| t.remove("toolchain"))
                {
                    if let Some(path) = toolchain.get("path").and_then(toml::Value::as_str) {
                        if !Path::new(path).is_absolute() {
                            r.note("rustup_relative_path_override_unverified");
                            return false;
                        }
                        let selected = Path::new(path).join("bin").join(file(name));
                        #[cfg(windows)]
                        if !crate::platform::windows::local(&selected) {
                            r.note("rustup_path_override_not_local");
                            return false;
                        }
                        if executable(&selected) {
                            r.alternate(selected.clone());
                            if governing {
                                r.selected = selected.canonicalize().ok();
                                r.source = "rustup_local_path_override";
                            }
                            r.note("rustup_local_path_override_observed");
                        } else {
                            r.note("rustup_path_override_target_not_observed");
                        }
                        return false;
                    }
                    pin = toolchain
                        .get("channel")
                        .and_then(toml::Value::as_str)
                        .map(str::to_owned);
                    if toolchain.get("components").is_some() || toolchain.get("targets").is_some() {
                        r.note("rustup_may_reconcile_declared_components_or_targets");
                    }
                } else if filename == "rust-toolchain" {
                    pin = component(raw.trim()).map(str::to_owned);
                }
                if pin.is_none() {
                    r.note("rustup_project_selector_unverified");
                    return false;
                }
                r.note("rustup_project_toolchain_file");
                break;
            }
            if pin.is_some() {
                break;
            }
        }
    }
    let pin = pin.or_else(|| {
        settings
            .as_ref()
            .and_then(|s| s.get("default_toolchain"))
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
    });
    let Some(pin) = pin.as_deref().and_then(component) else {
        r.note("rustup_channel_unresolved");
        return false;
    };
    r.requested = Some(crate::project::constraint(pin));
    let host = settings
        .as_ref()
        .and_then(|s| s.get("default_host_triple"))
        .and_then(toml::Value::as_str)
        .and_then(component)
        .or(option_env!("EXECSCOPE_BUILD_TARGET"))
        .or(match (env::consts::ARCH, env::consts::OS) {
            ("aarch64", "macos") => Some("aarch64-apple-darwin"),
            ("x86_64", "macos") => Some("x86_64-apple-darwin"),
            ("x86_64", "linux") => Some("x86_64-unknown-linux-gnu"),
            ("aarch64", "linux") => Some("aarch64-unknown-linux-gnu"),
            ("x86_64", "windows") => Some("x86_64-pc-windows-msvc"),
            _ => None,
        });
    let store = home.join("toolchains");
    let literal = store.join(pin);
    let toolchain = if literal.is_dir() || host.is_some_and(|h| pin.ends_with(h)) {
        literal
    } else if let Some(host) = host {
        store.join(format!("{pin}-{host}"))
    } else {
        return false;
    };
    let tool = toolchain.join("bin").join(file(name));
    if governing && store.is_dir() && !executable(&tool) {
        r.blocker = Some(if toolchain.is_dir() {
            "required_toolchain_component_unavailable"
        } else {
            "required_runtime_unavailable"
        });
        r.note("rustup_selected_target_missing");
    } else {
        r.observe(tool, governing, "rustup_selected_toolchain", pin);
    }
    governing
}

fn corepack(r: &mut Managed, root: &Path, name: &str, pin: &str) {
    let cached = root.join("v1").join(name).join(pin);
    let marker = safe::read(&cached.join(".corepack"), 16 * 1024)
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    if marker.is_some_and(|m| m["locator"]["name"].as_str() == Some(name)) {
        r.requested = Some(pin.to_owned());
        r.record_version(pin, "corepack_cache_marker");
        r.note("corepack_declared_version_cached_not_launched");
    } else if !cached.exists() && root.is_dir() {
        // Downloads remain forbidden; an inaccessible cache is not affirmative evidence of absence.
        if matches!(fs::symlink_metadata(&cached), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        {
            r.blocker = Some("corepack_not_cached");
            r.note("corepack_declared_version_not_observed_in_local_cache");
        }
    } else {
        r.note("corepack_cache_metadata_unverified");
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn xcode(r: &mut Managed, name: &str, root: Option<&Path>) {
    let Some(dir) = root else {
        r.note("developer_directory_not_observed");
        return;
    };
    let first = dir
        .join("Toolchains/XcodeDefault.xctoolchain/usr/bin")
        .join(name);
    let other = dir.join("usr/bin").join(name);
    let p = if executable(&first) { first } else { other };
    if executable(&p) {
        r.alternate(p.clone());
        r.note("developer_tool_binary_observed");
        r.selected = p.canonicalize().ok();
        r.source = "apple_developer_directory";
    } else {
        r.note("developer_tool_binary_not_observed");
    }
}

#[cfg(windows)]
fn windows_alternates(r: &mut Managed, name: &str) {
    let (user, machine) = crate::platform::windows::paths();
    for (value, source) in [
        (user, "windows_user_path_alternative"),
        (machine, "windows_machine_path_alternative"),
    ] {
        if let Some(raw) = value {
            for dir in env::split_paths(&raw)
                .take(64)
                .filter(|p| p.is_absolute() && crate::platform::windows::local(p))
            {
                let prev = r.alternatives.len();
                r.alternate(dir.join(file(name)));
                if r.alternatives.len() > prev {
                    r.note(source);
                }
            }
        }
    }
    if name == "python"
        && let Some(local) = env_dir("LOCALAPPDATA")
    {
        add_installed(
            r,
            &local.join("Programs/Python"),
            "",
            name,
            "windows_python_install_alternative",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn new() -> Managed {
        Managed {
            selected: None,
            alternatives: vec![],
            blocker: None,
            source: "fixture",
            evidence: vec![],
            requested: None,
            discovered_version: None,
            version_source: None,
            uv_project: None,
        }
    }
    #[test]
    fn aliases_proxies_wrappers_and_real_symlinks_are_distinct() {
        assert_eq!(
            classify(
                "python",
                Path::new("C:/Users/u/AppData/Local/Microsoft/WindowsApps/python.exe"),
                None
            ),
            Kind::WindowsAlias
        );
        assert_eq!(
            classify(
                "pnpm",
                Path::new("/bin/pnpm"),
                Some(Path::new("/node_modules/corepack/dist/pnpm.js"))
            ),
            Kind::Corepack
        );
        assert_eq!(
            classify(
                "node",
                Path::new("/bin/node"),
                Some(Path::new("/opt/Cellar/node/24/bin/node"))
            ),
            Kind::Direct
        );
        assert_eq!(
            classify("node", Path::new("/opt/asdf/shims/node"), None),
            Kind::Asdf
        );
    }
    #[test]
    fn rustup_missing_component_nearest_override_and_env_precedence() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("ru");
        let work = root.join("work");
        let child = work.join("sub");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(home.join("toolchains/1.92-test-host")).unwrap();
        fs::write(work.join("rust-toolchain"), "1.90").unwrap();
        fs::write(
            child.join("rust-toolchain.toml"),
            "[toolchain]\nchannel='1.91'",
        )
        .unwrap();
        fs::write(
            home.join("settings.toml"),
            "default_host_triple='test-host'\n",
        )
        .unwrap();
        let mut r = new();
        assert!(rustup(&mut r, &home, &child, None, "cargo", true));
        assert_eq!(r.requested.as_deref(), Some("1.91"));
        assert_eq!(r.blocker, Some("required_runtime_unavailable"));
        let mut r = new();
        assert!(rustup(&mut r, &home, &child, Some("1.92"), "cargo", true));
        assert_eq!(r.requested.as_deref(), Some("1.92"));
        assert_eq!(r.blocker, Some("required_toolchain_component_unavailable"));
    }
    #[test]
    fn rustup_absolute_path_override_is_not_a_default_channel() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let project = root.join("project");
        let custom = root.join("custom");
        fs::create_dir(&project).unwrap();
        let tool = custom.join("bin").join(file("rustc"));
        fs::create_dir_all(tool.parent().unwrap()).unwrap();
        fs::write(&tool, "fixture; must not launch").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let config = project.join("rust-toolchain.toml");
        let value = toml::Value::String(custom.to_string_lossy().to_string());
        fs::write(&config, format!("[toolchain]\npath = {value}\n")).unwrap();
        let mut r = new();
        rustup(&mut r, &root, &project, None, "rustc", true);
        assert_eq!(r.selected, Some(tool.canonicalize().unwrap()));
        assert_eq!(r.source, "rustup_local_path_override");
        fs::write(&config, "[toolchain]\npath = 'relative'\n").unwrap();
        let mut r = new();
        rustup(&mut r, &root, &project, None, "rustc", true);
        assert!(r.selected.is_none());
        assert!(
            r.evidence
                .contains(&"rustup_relative_path_override_unverified")
        );
    }

    #[test]
    fn corepack_local_cache_transition_does_not_launch_anything() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let mut r = new();
        corepack(&mut r, root, "pnpm", "10.0.2");
        assert_eq!(r.blocker, Some("corepack_not_cached"));
        let cached = root.join("v1/pnpm/10.0.2");
        fs::create_dir_all(&cached).unwrap();
        fs::write(
            cached.join(".corepack"),
            r#"{"locator":{"name":"pnpm","reference":"10.0.2"}}"#,
        )
        .unwrap();
        let mut r = new();
        corepack(&mut r, root, "pnpm", "10.0.2");
        assert_eq!(r.discovered_version.as_deref(), Some("10.0.2"));
        assert!(r.blocker.is_none());
        assert!(r.selected.is_none());
    }
    #[test]
    fn unactivated_and_broken_venvs_stay_separate_from_path() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let venv = root.join(".venv");
        fs::create_dir(&venv).unwrap();
        fs::write(venv.join("pyvenv.cfg"), "home=/missing").unwrap();
        let mut r = new();
        python_env(&mut r, &venv, "project_venv_candidate");
        assert!(
            r.evidence
                .contains(&"environment_interpreter_missing_or_non_executable")
        );
        assert!(r.selected.is_none());
        let path = venv.join(if cfg!(windows) {
            "Scripts/python.exe"
        } else {
            "bin/python"
        });
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "placeholder").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut r = new();
        python_env(&mut r, &venv, "uv_project_venv_candidate");
        assert_eq!(r.alternatives, vec![path]);
        assert!(r.selected.is_none());
    }
}
