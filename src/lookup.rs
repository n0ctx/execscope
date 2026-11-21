use crate::{
    managed::{self, Kind},
    safe,
};
use serde::Serialize;
use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

const PATH_LIMIT: usize = 96;
const CANDIDATE_LIMIT: usize = 10;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Candidate {
    #[serde(serialize_with = "safe::serialize_path")]
    pub(crate) path: PathBuf,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "optional_path"
    )]
    pub(crate) target: Option<PathBuf>,
    pub(crate) invocation: String,
    pub(crate) kind: Kind,
    pub(crate) executable: bool,
    pub(crate) source: &'static str,
    #[serde(skip_serializing_if = "crate::model::is_false")]
    pub(crate) selected: bool,
}

fn optional_path<S: serde::Serializer>(
    value: &Option<PathBuf>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(p) => serializer.serialize_some(&safe::path(p)),
        None => serializer.serialize_none(),
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Search {
    dirs: Vec<PathBuf>,
    extensions: Vec<String>,
    limited: bool,
    pub(crate) unavailable: bool,
    windows: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Chain {
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) entries_checked: usize,
    pub(crate) limited: bool,
    pub(crate) inaccessible: bool,
    pub(crate) earlier_selection_unverified: bool,
}

impl Search {
    pub(crate) fn process() -> Self {
        Self::new(
            env::var_os("PATH").as_deref(),
            env::var("PATHEXT").ok().as_deref(),
            cfg!(windows),
        )
    }

    fn new(path: Option<&OsStr>, pathext: Option<&str>, windows: bool) -> Self {
        let mut dirs: Vec<_> = path
            .map(|value| env::split_paths(value).take(PATH_LIMIT + 1).collect())
            .unwrap_or_default();
        let mut limited = dirs.len() > PATH_LIMIT;
        dirs.truncate(PATH_LIMIT);
        let mut extensions = vec![String::new()];
        if windows {
            for ext in pathext.unwrap_or(".COM;.EXE;.BAT;.CMD").split(';').take(25) {
                let ext = ext.trim();
                if ext.starts_with('.')
                    && (2..=9).contains(&ext.len())
                    && ext[1..].bytes().all(|b| b.is_ascii_alphanumeric())
                {
                    let ext = ext.to_ascii_lowercase();
                    if !extensions.contains(&ext) {
                        extensions.push(ext);
                    }
                }
            }
            limited |= pathext.is_some_and(|ext| ext.split(';').take(26).count() > 25);
        }
        Self {
            dirs,
            extensions,
            limited,
            unavailable: path.is_none(),
            windows,
        }
    }

    pub(crate) fn find(&self, name: &str, cwd: &Path) -> Chain {
        let invocations: &[&str] = match name {
            "python" if self.windows => &["python", "python3", "py"],
            "python" => &["python", "python3"],
            "pip" => &["pip", "pip3"],
            _ => &[name],
        };
        let mut chain = Chain {
            candidates: vec![],
            entries_checked: self.dirs.len(),
            limited: self.limited,
            inaccessible: false,
            earlier_selection_unverified: false,
        };
        let mut selected = false;
        for invocation in invocations {
            for dir in &self.dirs {
                let dir = if dir.is_absolute() {
                    dir.clone()
                } else {
                    cwd.join(dir)
                };
                #[cfg(windows)]
                if !crate::platform::windows::local(&dir) {
                    chain.inaccessible = true;
                    chain.earlier_selection_unverified |= !selected;
                    continue;
                }
                for ext in &self.extensions {
                    let file = format!("{invocation}{ext}");
                    let path = dir.join(&file);
                    if chain.candidates.iter().any(|cand| cand.path == path) {
                        continue;
                    }
                    let entry = match fs::symlink_metadata(&path) {
                        Ok(entry) => entry,
                        Err(e) => {
                            if !matches!(
                                e.kind(),
                                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                            ) {
                                chain.inaccessible = true;
                                chain.earlier_selection_unverified |= !selected;
                            }
                            continue;
                        }
                    };
                    if !(entry.is_file()
                        || entry.is_symlink()
                        || (self.windows && is_alias_location(&path)))
                    {
                        continue;
                    }
                    if chain.candidates.len() == CANDIDATE_LIMIT {
                        chain.limited = true;
                        return chain;
                    }
                    let target = fs::canonicalize(&path).ok();
                    #[cfg(windows)]
                    let target = target.filter(|p| crate::platform::windows::local(p));
                    let kind = if self.windows && is_alias_location(&path) {
                        Kind::WindowsAlias
                    } else {
                        managed::classify(name, &path, target.as_deref())
                    };
                    let executable =
                        kind == Kind::WindowsAlias || executable(&path, &file, self.windows);
                    let is_selected = !selected && executable;
                    selected |= is_selected;
                    chain.candidates.push(Candidate {
                        path,
                        target,
                        invocation: file,
                        kind,
                        executable,
                        selected: is_selected,
                        source: "process_path",
                    });
                }
                // In WSL these are not native Linux invocations. Record only as secondary evidence.
                #[cfg(target_os = "linux")]
                if dir.starts_with("/mnt") {
                    let path = dir.join(format!("{invocation}.exe"));
                    if chain.candidates.len() < CANDIDATE_LIMIT && path.is_file() {
                        chain.candidates.push(Candidate {
                            target: fs::canonicalize(&path).ok(),
                            path,
                            invocation: format!("{invocation}.exe"),
                            kind: Kind::Foreign,
                            executable: false,
                            selected: false,
                            source: "windows_mount",
                        });
                    }
                }
            }
        }
        chain
    }
}

fn is_alias_location(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        && path
            .components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case("WindowsApps"))
}

pub(crate) fn executable(path: &Path, name: &str, windows: bool) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    if windows {
        return name.contains('.');
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::CString::new(path.as_os_str().as_bytes()).is_ok_and(|p| unsafe {
            libc::faccessat(libc::AT_FDCWD, p.as_ptr(), libc::X_OK, libc::AT_EACCESS) == 0
        })
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_layers_do_not_borrow_interactive_or_user_path() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("GUI PATH 中文");
        let alt = root.path().join("terminal");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&alt).unwrap();
        fs::write(first.join("node.exe"), b"native file placeholder").unwrap();
        fs::write(first.join("node.cmd"), b"wrapper").unwrap();
        fs::write(alt.join("node.exe"), b"terminal-only").unwrap();
        let path = env::join_paths([&first]).unwrap();
        let chain = Search::new(Some(&path), Some(".CMD;.EXE"), true).find("node", root.path());
        assert_eq!(chain.candidates.len(), 2);
        assert!(chain.candidates[0].selected);
        assert_eq!(chain.candidates[0].invocation, "node.cmd");
        assert!(!chain.candidates[1].selected);
        assert!(chain.candidates.iter().all(|c| !c.path.starts_with(&alt)));
    }

    #[test]
    fn windows_alias_does_not_promote_real_install() {
        let root = tempfile::tempdir().unwrap();
        let aliases = root.path().join("WindowsApps");
        let alternate = root.path().join("Python");
        for dir in [&aliases, &alternate] {
            fs::create_dir(dir).unwrap();
            fs::write(dir.join("python.exe"), "").unwrap();
        }
        let path = env::join_paths([&aliases, &alternate]).unwrap();
        let chain = Search::new(Some(&path), None, true).find("python", root.path());
        assert_eq!(chain.candidates[0].kind, Kind::WindowsAlias);
        assert!(chain.candidates[0].selected);
        assert!(!chain.candidates[1].selected);
    }

    #[cfg(unix)]
    #[test]
    fn path_reorder_and_permissions_change_selection() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let one = root.path().join("one");
        let two = root.path().join("two");
        for dir in [&one, &two] {
            fs::create_dir(dir).unwrap();
            fs::write(dir.join("node"), "data").unwrap();
            fs::set_permissions(dir.join("node"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let find = |dirs: [&Path; 2]| {
            Search::new(Some(&env::join_paths(dirs).unwrap()), None, false)
                .find("node", root.path())
        };
        assert_eq!(find([&one, &two]).candidates[0].path, one.join("node"));
        assert_eq!(find([&two, &one]).candidates[0].path, two.join("node"));
        fs::set_permissions(two.join("node"), fs::Permissions::from_mode(0o644)).unwrap();
        let chain = find([&two, &one]);
        assert!(!chain.candidates[0].selected);
        assert!(chain.candidates[1].selected);
    }
}
