use super::*;

#[derive(Debug, Serialize, Clone)]
pub(super) struct Context {
    pub(super) source: &'static str,
    pub(super) state: &'static str,
    #[serde(serialize_with = "opt_path")]
    pub(super) executable: Option<PathBuf>,
    pub(super) version: Option<String>,
}

pub(super) fn uv_project(
    root: &Path,
    configured: Option<&std::ffi::OsStr>,
    discovery_disabled: bool,
) -> Context {
    let (locator, source) = match configured.filter(|s| !s.is_empty()) {
        Some(path) => (root.join(path), "UV_PROJECT_ENVIRONMENT"),
        None => (root.join(".venv"), "uv_default_project_environment"),
    };
    let mut context = Context {
        source,
        state: "project_environment_not_created",
        executable: None,
        version: None,
    };
    if discovery_disabled {
        context.state = "uv_project_discovery_disabled";
        return context;
    }
    #[cfg(windows)]
    if !crate::platform::windows::local(&locator) {
        context.state = "project_environment_not_local";
        return context;
    }
    let Some(cfg) = safe::read(&locator.join("pyvenv.cfg"), 8192) else {
        if locator.exists() {
            context.state = "project_environment_metadata_unverified";
        }
        return context;
    };
    let path = locator.join(if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    });
    if !executable(&path) {
        context.state = "project_interpreter_missing_or_non_executable";
        return context;
    }
    context.state = "existing_project_interpreter_candidate";
    context.executable = Some(path);
    context.version = pyvenv_version(&cfg);
    context
}

pub(super) fn pyvenv_version(raw: &str) -> Option<String> {
    raw.lines().take(48).find_map(|line| {
        let (key, value) = line.split_once('=')?;
        let value = value.trim();
        if !matches!(key.trim(), "version" | "version_info") || value.len() > 32 {
            return None;
        }
        let version = semver::Version::parse(value).ok()?;
        (version.pre.is_empty() && version.build.is_empty()).then(|| version.to_string())
    })
}

// ASDF uses the first installed requested version. `system` is a dispatch to
// the next PATH layer, not an installation in the ASDF store.
pub(super) fn asdf(
    root: &Path,
    tool: &str,
    exe: &str,
    request: &str,
) -> Result<(PathBuf, String), &'static str> {
    let mut versions = request.split_ascii_whitespace();
    for item in versions.by_ref().take(8) {
        if item == "system" {
            return Err("asdf_system_fallback_unverified");
        }
        let item = component(item).ok_or("asdf_nonstandard_selector_unverified")?;
        let path = root
            .join("installs")
            .join(tool)
            .join(item)
            .join("bin")
            .join(file(exe));
        if executable(&path) {
            return Ok((path, item.to_owned()));
        }
        if let Err(error) = fs::metadata(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err("asdf_installation_inaccessible");
        }
    }
    Err(if versions.next().is_some() {
        "asdf_selector_limit_reached"
    } else {
        "asdf_installed_target_not_observed"
    })
}

pub(super) fn asdf_file(
    cwd: &Path,
    home: Option<&Path>,
    filename: &str,
    tool: &str,
) -> Result<Option<String>, &'static str> {
    if filename.is_empty()
        || filename.len() > 96
        || matches!(filename, "." | "..")
        || !filename
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err("asdf_versions_filename_unverified");
    }
    let find = |directory: &Path| -> Result<Option<String>, &'static str> {
        let path = directory.join(filename);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Ok(meta) if meta.is_file() => (),
            _ => return Err("asdf_versions_file_unverified"),
        }
        let raw = safe::read(&path, 24 * 1024).ok_or("asdf_versions_file_unverified")?;
        Ok(raw.lines().find_map(|line| {
            let (name, rest) = line.trim_start().split_once(char::is_whitespace)?;
            (name == tool).then(|| rest.split('#').next().unwrap_or_default().trim().to_owned())
        }))
    };
    for directory in cwd.ancestors().take(32) {
        if let Some(value) = find(directory)? {
            return Ok(Some(value));
        }
        if Some(directory) == home {
            return Ok(None);
        }
    }
    home.map(find).transpose().map(Option::flatten)
}

type Pin = (String, &'static str);

pub(super) fn mise_request(
    cwd: &Path,
    global: Option<&Path>,
    envs: &str,
    tool: &str,
) -> Result<Option<Pin>, &'static str> {
    let envs: Vec<_> = envs.split(',').filter(|s| !s.is_empty()).take(9).collect();
    if envs.len() > 8 || !envs.iter().all(|s| component(s).is_some()) {
        return Err("mise_environment_selector_unverified");
    }
    let layer = |dir: &Path, names: Vec<String>, source| -> Result<Option<Pin>, &'static str> {
        for name in names {
            let file = dir.join(name);
            match fs::symlink_metadata(&file) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Ok(meta) if meta.is_file() => (),
                _ => return Err("mise_config_file_unverified"),
            };
            let canonical = file
                .canonicalize()
                .ok()
                .filter(|p| p.starts_with(dir))
                .ok_or("mise_config_file_unverified")?;
            let table = safe::read(&canonical, 48 * 1024)
                .and_then(|s| s.parse::<toml::Table>().ok())
                .ok_or("mise_config_file_unverified")?;
            let Some(tools) = table.get("tools").and_then(toml::Value::as_table) else {
                continue;
            };
            if tools
                .keys()
                .any(|key| key.rsplit(':').next() == Some(tool) && key != tool)
            {
                return Err("mise_explicit_backend_unverified");
            }
            if let Some(pin) = tools.get(tool) {
                let value = pin
                    .as_str()
                    .ok_or("mise_configured_tool_options_unverified")?;
                return Ok(Some((value.to_owned(), source)));
            }
        }
        Ok(None)
    };
    // Discovery rules from any ancestor can change which nearer files load.
    for dir in cwd.ancestors().take(32) {
        for name in [".miserc.toml", ".miserc.local.toml", ".config/miserc.toml"] {
            match fs::symlink_metadata(dir.join(name)) {
                Ok(meta) if meta.is_file() || meta.is_symlink() => {
                    return Err("mise_config_discovery_override_unverified");
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => return Err("mise_config_discovery_inaccessible"),
            }
        }
    }
    if let Some(dir) = global.and_then(|p| p.canonicalize().ok()) {
        let files = envs
            .iter()
            .rev()
            .map(|e| format!("config.{e}.override.toml"))
            .chain(["config.override.toml".into()])
            .collect();
        if let Some(pin) = layer(&dir, files, "mise_global_override")? {
            return Ok(Some(pin));
        }
    }
    for dir in cwd.ancestors().take(32) {
        let mut files = Vec::new();
        for env in envs.iter().rev() {
            files.extend([
                format!(".mise.{env}.local.toml"),
                format!("mise.{env}.local.toml"),
            ]);
        }
        files.extend([".mise.local.toml".into(), "mise.local.toml".into()]);
        for env in envs.iter().rev() {
            files.extend([format!(".mise.{env}.toml"), format!("mise.{env}.toml")]);
        }
        files.extend(
            [
                ".mise.toml",
                "mise.toml",
                "mise/config.toml",
                ".mise/config.toml",
                ".config/mise.toml",
                ".config/mise/config.toml",
            ]
            .map(str::to_owned),
        );
        if let Some(pin) = layer(dir, files, "mise_project_config")? {
            return Ok(Some(pin));
        }
        if ["mise/conf.d", ".mise/conf.d", ".config/mise/conf.d"]
            .iter()
            .any(|s| dir.join(s).is_dir())
        {
            return Err("mise_config_fragments_unverified");
        }
        let asdf_key = if tool == "node" {
            "nodejs"
        } else if tool == "go" {
            "golang"
        } else {
            tool
        };
        if let Some(pin) = asdf_file(dir, Some(dir), ".tool-versions", asdf_key)? {
            return Ok(Some((pin, "mise_tool_versions_config")));
        }
    }
    if let Some(dir) = global.and_then(|p| p.canonicalize().ok()) {
        let mut files: Vec<_> = envs
            .iter()
            .rev()
            .map(|e| format!("config.{e}.local.toml"))
            .collect();
        files.push("config.local.toml".into());
        files.extend(envs.iter().rev().map(|e| format!("config.{e}.toml")));
        files.extend(["config.toml".into(), "mise.toml".into()]);
        return layer(&dir, files, "mise_global_config");
    }
    Ok(None)
}

// Only describe the common layout. A config selector with templates, aliases,
// a plugin/backend, overlays, or multiple matching installations is not resolved
// from directory names alone.
pub(super) fn mise_install(
    root: &Path,
    tool: &str,
    exe: &str,
    request: &str,
) -> Result<(PathBuf, String), &'static str> {
    let pin = request.strip_prefix("prefix:").unwrap_or(request);
    let pin = pin.strip_prefix('v').unwrap_or(pin);
    let parts: Vec<_> = pin.split('.').collect();
    if parts.is_empty()
        || parts.len() > 3
        || parts
            .iter()
            .any(|s| s.is_empty() || s.len() > 5 || !s.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err("mise_non_numeric_selector_unverified");
    }
    let installs = root.join("installs").join(tool);
    let entries = fs::read_dir(installs).map_err(|_| "mise_install_store_unavailable")?;
    let mut found = None;
    for (index, entry) in entries.enumerate() {
        if index >= 64 {
            return Err("mise_install_store_limit_reached");
        }
        let entry = entry.map_err(|_| "mise_install_store_inaccessible")?;
        let basename = entry.file_name();
        let Some(raw) = basename.to_str() else {
            continue;
        };
        let Ok(version) = semver::Version::parse(raw.strip_prefix('v').unwrap_or(raw)) else {
            continue;
        };
        if !version.pre.is_empty() || !version.build.is_empty() {
            continue;
        }
        let numeric = [version.major, version.minor, version.patch];
        if !parts
            .iter()
            .zip(numeric)
            .all(|(requested, installed)| requested.parse::<u64>() == Ok(installed))
        {
            continue;
        }
        let path = entry.path().join("bin").join(file(exe));
        if !executable(&path) {
            continue;
        }
        if found.is_some() {
            return Err("mise_multiple_installed_matches");
        }
        found = Some((path, version.to_string()));
    }
    found.ok_or("mise_matching_install_not_observed")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn install(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "not an executable payload").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn asdf_uses_first_installed_fallback_and_distinguishes_system() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(
            root.join(".fixture-versions"),
            "nodejs 20.0.0 22.1.0 # fallback\n",
        )
        .unwrap();
        let request = asdf_file(root, Some(root), ".fixture-versions", "nodejs")
            .unwrap()
            .unwrap();
        let node = root.join("installs/nodejs/22.1.0/bin").join(file("node"));
        install(&node);
        assert_eq!(
            asdf(root, "nodejs", "node", &request).unwrap(),
            (node, "22.1.0".into())
        );
        assert_eq!(
            asdf(root, "nodejs", "node", "20.0.0 system 22.1.0"),
            Err("asdf_system_fallback_unverified")
        );
    }

    #[test]
    fn mise_numeric_prefix_is_unique_or_not_selected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let node = root.join("installs/node/24.3.0/bin").join(file("node"));
        install(&node);
        assert_eq!(
            mise_install(root, "node", "node", "24").unwrap(),
            (node, "24.3.0".into())
        );
        install(&root.join("installs/node/24.9.0/bin").join(file("node")));
        assert_eq!(
            mise_install(root, "node", "node", "prefix:24"),
            Err("mise_multiple_installed_matches")
        );
        assert_eq!(
            mise_install(root, "node", "node", "latest"),
            Err("mise_non_numeric_selector_unverified")
        );
    }

    #[test]
    fn mise_global_override_and_project_depth_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let cwd = root.join("nested");
        let global = root.join("global");
        fs::create_dir(&cwd).unwrap();
        fs::create_dir(&global).unwrap();
        fs::write(root.join("mise.local.toml"), "[tools]\nnode = '20'\n").unwrap();
        fs::write(cwd.join("mise.toml"), "[tools]\nnode = '22'\n").unwrap();
        fs::write(cwd.join("mise.ci.toml"), "[tools]\nnode = '24'\n").unwrap();
        assert_eq!(
            mise_request(&cwd, Some(&global), "ci", "node").unwrap(),
            Some(("24".into(), "mise_project_config"))
        );
        fs::write(
            global.join("config.override.toml"),
            "[tools]\nnode = '26'\n",
        )
        .unwrap();
        assert_eq!(
            mise_request(&cwd, Some(&global), "ci", "node").unwrap(),
            Some(("26".into(), "mise_global_override"))
        );
    }

    #[test]
    fn uv_relative_env_is_bound_to_python_root_and_not_process_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let configured = Some(std::ffi::OsStr::new(".uv-env"));
        let env = root.join(".uv-env");
        let exe = env.join(if cfg!(windows) {
            "Scripts/python.exe"
        } else {
            "bin/python"
        });
        assert_eq!(
            uv_project(root, configured, false).state,
            "project_environment_not_created"
        );
        fs::create_dir(&env).unwrap();
        fs::write(env.join("pyvenv.cfg"), "version_info = 3.12.8\n").unwrap();
        assert_eq!(
            uv_project(root, configured, false).state,
            "project_interpreter_missing_or_non_executable"
        );
        install(&exe);
        let context = uv_project(root, configured, false);
        assert_eq!(context.executable, Some(exe));
        assert_eq!(context.version.as_deref(), Some("3.12.8"));
        assert_eq!(
            uv_project(root, configured, true).state,
            "uv_project_discovery_disabled"
        );
    }
}
