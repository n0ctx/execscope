use crate::{model::Status, safe};
use serde::Serialize;
use std::{io::Read, path::Path, time::Duration};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Probe {
    pub(crate) version: Option<String>,
    #[cfg(windows)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file_version: Option<String>,
    pub(crate) status: Status,
    pub(crate) reason: &'static str,
    pub(crate) source: &'static str,
}

fn unknown(reason: &'static str) -> Probe {
    Probe {
        version: None,
        #[cfg(windows)]
        file_version: None,
        status: Status::Unknown,
        reason,
        source: "probe_policy",
    }
}

#[cfg(unix)]
pub(crate) async fn verify(name: &str, target: &Path, cwd: &Path, timeout: Duration) -> Probe {
    let Some(spec) = spec(name) else {
        return Probe {
            status: Status::Unsupported,
            ..unknown("no_read_only_identity_probe")
        };
    };
    let Some(root) = trust_root(name, target) else {
        return unknown("outside_trusted_install_roots");
    };
    if !trusted_chain(target, root) {
        return unknown("installation_ownership_unverified");
    }
    let Some(mut file) = safe::open_regular(target) else {
        return unknown("candidate_unavailable");
    };
    let mut bytes = [0; 512];
    let Ok(n) = file.read(&mut bytes) else {
        return unknown("candidate_unavailable");
    };
    if !native_image(&bytes[..n]) {
        return unknown("wrapper_or_foreign_image");
    }
    let result = run(target, spec.args, cwd, timeout).await;
    match result {
        Ok(output) => match version_from(name, &output) {
            Some(version) => Probe {
                version: Some(version),
                status: Status::Available,
                reason: "identity_verified",
                source: "verified_executable",
            },
            None => unknown("unrecognized_version_output"),
        },
        Err("timed_out") => Probe {
            status: Status::Timeout,
            ..unknown("timed_out")
        },
        Err(reason) => unknown(reason),
    }
}

#[cfg(windows)]
pub(crate) async fn verify(_name: &str, target: &Path, _cwd: &Path, _timeout: Duration) -> Probe {
    let Some(mut file) = safe::open_regular(target) else {
        return unknown("candidate_unavailable");
    };
    let mut bytes = [0; 512];
    let Ok(n) = file.read(&mut bytes) else {
        return unknown("candidate_unavailable");
    };
    if !native_image(&bytes[..n]) {
        return unknown("wrapper_or_foreign_image");
    }
    // A PE resource establishes a file version, not safe execution in the
    // requested directory. Do not start a child with an unverified DLL search.
    Probe {
        file_version: crate::platform::windows::file_version(target),
        status: Status::Unsupported,
        source: "native_pe_file_metadata",
        ..unknown("native_image_observed_safe_launch_unavailable")
    }
}

#[cfg(unix)]
struct Spec {
    args: &'static [&'static str],
}
#[cfg(unix)]
fn spec(name: &str) -> Option<Spec> {
    let args: &[&str] = match name {
        "python" => &["-I", "-S", "-B", "--version"],
        "java" | "javac" | "dotnet" => return None, // JVM and dotnet host setup varies by distribution.
        "go" => &["version"],
        "git" | "node" | "bun" | "rustc" | "cargo" | "swift" | "gcc" | "clang" | "cmake" | "uv"
        | "docker" => &["--version"],
        _ => return None, // Never run a package-manager or interpreter launcher wrapper.
    };
    Some(Spec { args })
}

#[cfg(unix)]
fn trust_root<'a>(name: &str, path: &'a Path) -> Option<&'a Path> {
    let nodes: &[&str] = match name {
        "git" => &["git"],
        "python" => &["python", "python3"],
        "node" => &["node"],
        "bun" => &["bun"],
        "rustc" => &["rustc"],
        "cargo" => &["cargo"],
        "go" => &["go"],
        "swift" => &["swift", "swift-driver", "swift-frontend"],
        "gcc" => &["gcc"],
        "clang" => &["clang"],
        "cmake" => &["cmake"],
        "uv" => &["uv"],
        "docker" => &["docker"],
        _ => return None,
    };
    let filename = path.file_name()?.to_str()?;
    let target_name = filename
        .strip_prefix("x86_64-linux-gnu-")
        .or_else(|| filename.strip_prefix("aarch64-linux-gnu-"))
        .unwrap_or(filename);
    if !nodes.contains(&target_name)
        && !(name == "python"
            && filename
                .strip_prefix("python3.")
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())))
        && !(matches!(name, "gcc" | "clang")
            && target_name
                .strip_prefix(name)
                .and_then(|v| v.strip_prefix('-'))
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())))
    {
        return None;
    }
    let parent = path.parent()?;
    if ["/usr/bin", "/bin", "/usr/sbin", "/sbin"].contains(&parent.to_str()?) {
        // Apple developer stubs may present an installer (including the Git stub).
        if cfg!(target_os = "macos") && matches!(name, "git" | "swift" | "clang" | "gcc") {
            return None;
        }
        return Some(parent);
    }
    #[cfg(target_os = "linux")]
    if (name == "go"
        && (parent == Path::new("/usr/local/go/bin") || versioned_system_bin(parent, "go-")))
        || (name == "clang" && versioned_system_bin(parent, "llvm-"))
    {
        return Some(parent);
    }
    #[cfg(target_os = "macos")]
    {
        if matches!(name, "git" | "swift" | "clang" | "gcc")
                && ["/Library/Developer/CommandLineTools/usr/bin", "/Applications/Xcode.app/Contents/Developer/usr/bin", "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin"].contains(&parent.to_str()?) {
                    return Some(parent);
                }
        // Canonical path must be a bottle for this tool, not merely below Homebrew's prefix.
        for root in [
            Path::new("/opt/homebrew/Cellar"),
            Path::new("/usr/local/Cellar"),
        ] {
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let mut components = relative.components();
            let formula = components.next()?.as_os_str().to_str()?;
            let release = components.next()?.as_os_str().to_str()?;
            if formula_matches(name, formula)
                && release.len() < 64
                && release.bytes().next()?.is_ascii_digit()
            {
                let store = root.join(formula).join(release);
                let receipt = safe::read(&store.join("INSTALL_RECEIPT.json"), 32 * 1024)
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
                if receipt.is_some_and(|r| r["poured_from_bottle"].as_bool() == Some(true)) {
                    return path.ancestors().find(|ancestor| *ancestor == store);
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn versioned_system_bin(bin: &Path, prefix: &str) -> bool {
    bin.file_name().is_some_and(|s| s == "bin")
        && bin.parent().is_some_and(|release| {
            release.parent() == Some(Path::new("/usr/lib"))
                && release
                    .file_name()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.strip_prefix(prefix))
                    .is_some_and(|s| {
                        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                    })
        })
}

#[cfg(target_os = "macos")]
fn formula_matches(name: &str, formula: &str) -> bool {
    let base = if name == "rustc" || name == "cargo" {
        "rust"
    } else {
        name
    };
    let base = if base == "clang" { "llvm" } else { base };
    formula == base
        || formula
            .strip_prefix(base)
            .and_then(|s| s.strip_prefix('@'))
            .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
}

#[cfg(unix)]
fn trusted_chain(path: &Path, root: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let local_store =
        root.starts_with("/opt/homebrew/Cellar") || root.starts_with("/usr/local/Cellar");
    let owner = if local_store {
        // The Homebrew installer makes the Cellar writable by its installing user.
        let cellar = root
            .ancestors()
            .find(|p| p.file_name().is_some_and(|s| s == "Cellar"));
        let Some(owner) = cellar
            .and_then(|p| std::fs::symlink_metadata(p).ok())
            .map(|m| m.uid())
        else {
            return false;
        };
        owner
    } else {
        0
    };
    path.ancestors().take(30).all(|p| {
        std::fs::symlink_metadata(p).ok().is_some_and(|m| {
            !m.file_type().is_symlink()
                && (m.uid() == 0 || m.uid() == owner)
                && (m.mode() & 0o022 == 0
                    || local_store && m.uid() == owner && m.mode() & 0o002 == 0
                    || cfg!(target_os = "macos")
                        && p == Path::new("/Applications")
                        && m.uid() == 0
                        && m.gid() == 80
                        && m.mode() & 0o002 == 0)
        })
    })
}

fn native_image(bytes: &[u8]) -> bool {
    #[cfg(target_os = "linux")]
    {
        if bytes.len() < 20 || &bytes[..4] != b"\x7fELF" || bytes[4] != 2 || bytes[5] != 1 {
            return false;
        }
        let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
        (cfg!(target_arch = "x86_64") && machine == 62)
            || (cfg!(target_arch = "aarch64") && machine == 183)
    }
    #[cfg(target_os = "macos")]
    {
        let Some(header) = bytes.get(..8) else {
            return false;
        };
        let kind: [u8; 4] = header[..4].try_into().unwrap();
        let machine = |cpu| {
            (cfg!(target_arch = "aarch64") && cpu == 0x0100000c)
                || (cfg!(target_arch = "x86_64") && cpu == 0x01000007)
        };
        match kind {
            [0xcf, 0xfa, 0xed, 0xfe] => {
                machine(u32::from_le_bytes(header[4..8].try_into().unwrap()))
            }
            [0xca, 0xfe, 0xba, 0xbe | 0xbf] => {
                let size = if kind[3] == 0xbf { 32 } else { 20 };
                let count = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
                (1..=12).contains(&count)
                    && bytes[8..]
                        .chunks_exact(size)
                        .take(count)
                        .any(|arch| machine(u32::from_be_bytes(arch[..4].try_into().unwrap())))
            }
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        let Some(offset) = bytes.get(60..64) else {
            return false;
        };
        let offset = u32::from_le_bytes(offset.try_into().unwrap()) as usize;
        let Some(pe) = offset.checked_add(6).and_then(|end| bytes.get(offset..end)) else {
            return false;
        };
        bytes.starts_with(b"MZ")
            && pe.starts_with(b"PE\0\0")
            && matches!(
                (std::env::consts::ARCH, u16::from_le_bytes([pe[4], pe[5]])),
                ("x86_64", 0x8664) | ("aarch64", 0xaa64)
            )
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = bytes;
        false
    }
}

#[cfg(unix)]
pub(crate) async fn run(
    path: &Path,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> Result<Vec<u8>, &'static str> {
    use std::process::Stdio;
    use tokio::{io::AsyncReadExt, process::Command};
    struct Group(i32);
    impl Drop for Group {
        fn drop(&mut self) {
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
    }
    let mut command = Command::new(path);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("LC_ALL", "C")
        .env("GOTOOLCHAIN", "local")
        .env("GOENV", "off")
        .env("GOPROXY", "off")
        .env("GOSUMDB", "off")
        .env("DOCKER_CONFIG", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command.spawn().map_err(|_| "launch_denied_or_failed")?;
    let pid = child
        .id()
        .and_then(|p| i32::try_from(p).ok())
        .ok_or("launch_failed")?;
    let group = Group(pid);
    let mut stdout = child.stdout.take().ok_or("output_unavailable")?.take(4097);
    let deadline = tokio::time::Instant::now() + timeout;
    let result = tokio::time::timeout_at(deadline, async {
        let mut bytes = Vec::new();
        stdout
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "output_unavailable")?;
        if bytes.len() > 4096 {
            return Err("output_limit_exceeded");
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| "timed_out")?;
    let bytes = result?;
    let status = tokio::time::timeout_at(deadline, child.wait())
        .await
        .map_err(|_| "timed_out")?
        .map_err(|_| "probe_failed")?;
    // Keep the group guard live until the child is reaped. Approved probes do not
    // create a separate session; a timeout drops the guard and then the child.
    std::mem::forget(group);
    if !status.success() {
        return Err("probe_nonzero_exit");
    }
    Ok(bytes)
}

#[cfg(unix)]
fn version_from(name: &str, bytes: &[u8]) -> Option<String> {
    if bytes.len() > 4096 {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return None;
    }
    let line = text.lines().next()?.trim();
    if line.len() > 240 {
        return None;
    }
    let suffix = match name {
        "git" => line.strip_prefix("git version "),
        "python" => line.strip_prefix("Python "),
        "go" => line.strip_prefix("go version go"),
        "rustc" => line.strip_prefix("rustc "),
        "cargo" => line.strip_prefix("cargo "),
        "uv" => line.strip_prefix("uv "),
        "cmake" => line.strip_prefix("cmake version "),
        "docker" => line.strip_prefix("Docker version "),
        "swift" => line
            .strip_prefix("Apple Swift version ")
            .or_else(|| line.strip_prefix("Swift version ")),
        "clang" => line
            .strip_prefix("Apple clang version ")
            .or_else(|| line.strip_prefix("clang version ")),
        "gcc"
            if line.starts_with("gcc ")
                || line.starts_with("gcc-")
                || line.starts_with("x86_64-linux-gnu-gcc")
                || line.starts_with("aarch64-linux-gnu-gcc") =>
        {
            line.split_once(") ").map(|(_, v)| v)
        }
        "node" => line.strip_prefix('v'),
        "bun" => Some(line),
        _ => None,
    }?;
    let value = suffix
        .split_ascii_whitespace()
        .next()?
        .trim_end_matches(',');
    if value.len() > 32 || !value.bytes().next()?.is_ascii_digit() {
        return None;
    }
    if semver::Version::parse(value).is_ok() {
        return Some(value.to_owned());
    }
    let parts = value
        .split('.')
        .take(4)
        .take_while(|part| {
            !part.is_empty() && part.len() < 5 && part.bytes().all(|b| b.is_ascii_digit())
        })
        .take(3)
        .collect::<Vec<_>>();
    if parts.len() >= 2 {
        Some(parts.join("."))
    } else {
        None
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn rejects_raw_huge_spoofed_and_ansi_output() {
        assert_eq!(
            version_from("git", b"git version 2.42.3 (Apple Git-17)\n"),
            Some("2.42.3".into())
        );
        assert_eq!(version_from("node", b"v24.1.0\n"), Some("24.1.0".into()));
        assert_eq!(version_from("git", b"ignore this 2.42.3"), None);
        assert_eq!(version_from("git", b"git version \x1b[31m2.42.3"), None);
        assert_eq!(version_from("git", &[b'a'; 4097]), None);
    }
    #[tokio::test]
    async fn cwd_scripts_are_not_probed_and_timeout_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("node");
        std::fs::write(&script, "#!/bin/sh\ntouch marker\n").unwrap();
        assert_eq!(
            verify("node", &script, temp.path(), Duration::from_millis(100))
                .await
                .status,
            Status::Unknown
        );
        assert!(!temp.path().join("marker").exists());
        let start = std::time::Instant::now();
        assert_eq!(
            run(
                Path::new("/bin/sleep"),
                &["10"],
                temp.path(),
                Duration::from_millis(30)
            )
            .await
            .unwrap_err(),
            "timed_out"
        );
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn deadline_cleans_up_a_group_with_children() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path();
        let error = run(
            Path::new("/bin/sh"),
            &[
                "-c",
                "/bin/sleep 0.25 & wait; printf done > descendant-finished",
            ],
            work,
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert_eq!(error, "timed_out");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!work.join("descendant-finished").exists());
    }
}
