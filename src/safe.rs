use serde::Serializer;
use std::{fs::OpenOptions, io::Read, path::Path};

// Nonblocking open prevents a replacement FIFO from stalling a reader. Stat the
// opened file, not the originally observed directory entry.
pub(crate) fn open_regular(path: &Path) -> Option<std::fs::File> {
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = options.open(path).ok()?;
    file.metadata().ok()?.is_file().then_some(file)
}

pub(crate) fn read(path: &Path, max: usize) -> Option<String> {
    let file = open_regular(path)?;
    if file.metadata().ok()?.len() > max as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > max {
        return None;
    }
    String::from_utf8(bytes).ok()
}

pub(crate) fn token(input: &str, max: usize) -> Option<String> {
    let value = input.trim();
    (!value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_+*<>=~^|,! *".contains(&b)))
    .then(|| value.to_owned())
}

pub(crate) fn path(path: &Path) -> String {
    let name = path.to_string_lossy();
    let lower = name.to_ascii_lowercase();
    if name.len() > 768
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        || name.contains("://")
        || lower.contains("%3a%2f%2f")
        || name.contains('?')
        || [
            "password=",
            "passwd=",
            "token=",
            "api_key=",
            "secret=",
            "authorization=",
            "bearer ",
            "cookie=",
            "xoxb-",
            "ghp_",
            "github_pat_",
            "sk-live-",
            "sk-proj-",
        ]
        .iter()
        .any(|p| lower.contains(p))
    {
        return "<redacted_path>".to_owned();
    }
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    if let Some(home) = home
        .as_deref()
        .map(Path::new)
        .filter(|p| p.is_absolute() && p.parent().is_some())
        && let Ok(relative) = path.strip_prefix(home)
    {
        return if relative.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", relative.to_string_lossy())
        };
    }
    name.into_owned()
}

pub(crate) fn serialize_path<S: Serializer>(
    value: &Path,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path(value))
}

pub(crate) fn brief_path(value: &str) -> String {
    if value
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\\')
    {
        serde_json::to_string(value).expect("string serialization cannot fail")
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_and_path_filtering() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("file");
        std::fs::write(&p, "12345").unwrap();
        assert_eq!(read(&p, 5).as_deref(), Some("12345"));
        assert_eq!(read(&p, 4), None);
        assert_eq!(read(d.path(), 100), None);
        assert_eq!(path(Path::new("/cache/token=dummy")), "<redacted_path>");
        assert_eq!(path(Path::new("/cache/\x1b[31m")), "<redacted_path>");
        assert_eq!(brief_path("/code/中文 space"), "\"/code/中文 space\"");
    }
}
