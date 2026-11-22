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

const SECRET_MARKERS: &[&str] = &[
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
    "sk-ant-",
];

fn secret_like(lower: &str) -> bool {
    SECRET_MARKERS.iter().any(|p| lower.contains(p))
}

/// Prefixes of widely used credential formats, each followed by a long key body.
const KEY_PREFIXES: &[&str] = &[
    "sk-",
    "sk_live_",
    "sk_test_",
    "rk_live_",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "hf_",
    "npm_",
    "shpat_",
    "AIza",
    "AKIA",
    "eyJ",
];

/// Credential-shaped text: a known key prefix at a word start followed by 16+ key
/// characters with at least two digits and four letters; a 32+ character run mixing
/// upper case, lower case and digits; or a 32+ character hexadecimal run.
fn key_like(value: &str) -> bool {
    let key_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let prefixed = KEY_PREFIXES.iter().any(|prefix| {
        value.match_indices(prefix).any(|(at, _)| {
            let boundary = !value[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric());
            let body: String = value[at + prefix.len()..]
                .chars()
                .take_while(|c| key_char(*c))
                .collect();
            boundary
                && body.len() >= 16
                && body.bytes().filter(u8::is_ascii_digit).count() >= 2
                && body.bytes().filter(u8::is_ascii_alphabetic).count() >= 4
        })
    });
    prefixed
        || value
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|run| run.len() >= 32)
            .any(|run| {
                let digits = run.bytes().any(|b| b.is_ascii_digit());
                let mixed = run.bytes().any(|b| b.is_ascii_uppercase())
                    && run.bytes().any(|b| b.is_ascii_lowercase());
                digits && (mixed || run.bytes().all(|b| b.is_ascii_hexdigit()))
            })
}

/// Leading components of a home or system path written without its separator, with
/// the number of components from which the text is treated as a path: `Users/alice`
/// names a person, while `var/log-parser` is an ordinary two-part name.
const PATH_ROOTS: &[(&str, usize)] = &[
    ("users", 2),
    ("home", 3),
    ("root", 2),
    ("private", 3),
    ("var", 3),
    ("tmp", 3),
    ("mnt", 3),
    ("media", 3),
    ("volumes", 3),
    ("opt", 3),
    ("usr", 3),
    ("etc", 3),
];

/// Package-spec schemes that are written `scheme:name@version`, not `user:password@host`.
const PACKAGE_SCHEMES: &[&str] = &[
    "npm", "jsr", "git", "github", "gitlab", "pypi", "file", "link",
];

fn ipv4(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether `@` at `at` introduces a host: `user@host:path` remotes, `user@host/path`,
/// e-mail and `user[:password]@domain` addresses, IP addresses and `localhost`.
/// `npm:@scope/pkg`, `pkg@1.2.3`, `pkg@1.0.0:beta` and `plugin@market` are not hosts.
fn host_after(value: &str, at: usize) -> bool {
    let before = &value[..at];
    let rest = &value[at + 1..];
    let host = &rest[..rest.find(['/', ':']).unwrap_or(rest.len())];
    let user = before.rsplit(['/', ':']).next().unwrap_or("");
    let letters = host.bytes().any(|b| b.is_ascii_alphabetic());
    let remote = letters
        && rest
            .find(':')
            .is_some_and(|colon| !rest[..colon].contains('/'));
    let pathed = !user.is_empty() && rest.contains('/');
    let domain = host.contains('.')
        && host
            .split('.')
            .next()
            .is_some_and(|label| label.bytes().any(|b| b.is_ascii_alphabetic()))
        && host
            .rsplit('.')
            .next()
            .is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()));
    // `alice:s3cr3t@host`: a password before the `@`, unless it is `npm:pkg@1.2.3`.
    let password = before
        .rsplit('/')
        .next()
        .and_then(|seg| seg.split_once(':'))
        .is_some_and(|(scheme, secret)| {
            !scheme.is_empty() && !secret.is_empty() && !PACKAGE_SCHEMES.contains(&scheme)
        });
    remote || pathed || domain || password || ipv4(host) || host.eq_ignore_ascii_case("localhost")
}

/// A configuration-declared name or identifier (model, provider, MCP server, Skill)
/// that is safe to print: letters/digits in any script plus `._-:@/+[]#^~`, bounded
/// length, no URL, filesystem path, remote address, e-mail or IP address, or
/// credential-looking content.
pub(crate) fn identifier(input: &str, max_chars: usize) -> Option<String> {
    let value = input.trim();
    let address = value
        .match_indices('@')
        .any(|(at, _)| host_after(value, at))
        || value.split(['/', ':', '@']).any(ipv4);
    let segments: Vec<&str> = value.split('/').collect();
    let rooted = PATH_ROOTS
        .iter()
        .any(|(root, min)| segments.len() >= *min && segments[0].eq_ignore_ascii_case(root));
    let drive = value.as_bytes().get(1) == Some(&b':')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic);
    (!value.is_empty()
        && value.chars().count() <= max_chars
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || "._-:@/+[]#^~".contains(c))
        // `~` also starts `~user/...` home paths, so a leading tilde is never printed.
        && !value.starts_with(['/', '.', '~'])
        && !value.contains("..")
        && !value.contains(":/")
        && !address
        && !rooted
        && !drive
        && !key_like(value))
    .then(|| value.to_owned())
}

pub(crate) fn path(path: &Path) -> String {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut name = path.to_string_lossy().into_owned();
    // Windows canonical paths carry a verbatim prefix. Its `?` would trip the query-string
    // guard below and hide every absolute path behind a redaction.
    #[cfg(windows)]
    {
        if let Some(rest) = name.strip_prefix(r"\\?\UNC\") {
            name = format!(r"\\{rest}");
        } else if let Some(rest) = name.strip_prefix(r"\\?\") {
            name = rest.to_owned();
        }
    }
    let lower = name.to_ascii_lowercase();
    if name.len() > 768
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        || name.contains("://")
        || lower.contains("%3a%2f%2f")
        || name.contains('?')
        || secret_like(&lower)
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
        && let Ok(relative) = Path::new(name.as_str()).strip_prefix(home)
    {
        return if relative.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", relative.to_string_lossy())
        };
    }
    name
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
        assert_eq!(
            identifier(" qwen3.8-27b ", 96).as_deref(),
            Some("qwen3.8-27b")
        );
        assert_eq!(identifier("代码审查", 96).as_deref(), Some("代码审查"));
        for ok in [
            "npm:@scope/pkg",
            "fmt@market",
            "anthropic/claude-x",
            "git:github.com/o/r",
            "opus[1m]",
            "npm:pkg@1.2.3",
            "openrouter/anthropic/claude-x",
            "task-runner-for-everything-2",
            "disk-usage-analyzer-v2-pro",
            "hf_transfer",
            "Users",
            "owner/repo#main",
            "pkg@^1.2.3",
            "pkg@~1.2",
            "pkg@1.0.0:beta",
            "pkg@1.0.alpha",
            "home/assistant",
            "var/log-parser",
            "ghp_helper",
            "owner/sk-agent-toolkit-v2-extra",
            "@scope/pkg@latest",
            "claude-opus-4-5@20251101",
            "sk-ant-abc",
        ] {
            assert!(identifier(ok, 96).is_some(), "{ok}");
        }
        for bad in [
            "a b",
            "x;rm",
            "https://h",
            "sk-ant-api03-Abc123Def456Ghi789",
            "",
            "\u{202e}x",
            "/Users/alice/x",
            "../../x.ts",
            "~/ext",
            ".ssh/id_rsa",
            "git@github.com:alice/private",
            "git:git@github.com:alice/repo@v1",
            "C:/Users/x",
            "alice:s3cr3t@example.com",
            "alice@example.com",
            "git@gh:alice/private",
            "alice@mac-mini:code/repo",
            "Users/alice/secret-project",
            "home/alice/x",
            "sk-or-v1-0123456789abcdef0123",
            "sk-0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKL",
            "glpat-0123456789abcdefghij",
            "hf_0123456789abcdefghijkl",
            "AIzaSyA0123456789abcdefghij",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig",
            "key:Zx9Qw8Er7Ty6Ui5Op4As3Df2Gh1Jk0LmNb",
            "~anthropic/claude-latest",
            "alice:s3cr3t@mac-mini",
            "user@host/path/repo",
            "user@192.168.1.5",
            "user@localhost",
            "10.0.0.1",
            "192.168.1.5:8080",
            "0123456789abcdef0123456789abcdef",
            "sk_live_0123456789abcdefABCD",
            "npm_ab12cd34ef56gh78ij90",
            "shpat_0123456789abcdef0123",
            "C:Users",
            "Users/alice",
            "glm-4.6@z.ai",
        ] {
            assert_eq!(identifier(bad, 96), None, "{bad}");
        }
        assert_eq!(identifier(&"a".repeat(65), 64), None);
    }

    // Canonical Windows paths arrive with a verbatim prefix; it must not read as a query string.
    #[cfg(windows)]
    #[test]
    fn verbatim_prefix_is_not_a_redaction() {
        assert_eq!(path(Path::new(r"\\?\C:\work\app")), r"C:\work\app");
        assert_eq!(
            path(Path::new(r"\\?\UNC\server\share\app")),
            r"\\server\share\app"
        );
    }
}
