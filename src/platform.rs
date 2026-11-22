use crate::model::Status;
#[cfg(target_os = "macos")]
use crate::safe;
use serde::Serialize;
use std::{env, path::Path};

#[cfg(any(target_os = "linux", test))]
#[cfg_attr(
    not(target_os = "linux"),
    allow(dead_code, reason = "Exercise Linux parser fixtures on other hosts")
)]
mod linux;
#[cfg(windows)]
pub(crate) mod windows;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct System {
    pub(crate) status: Status,
    pub(crate) os: &'static str,
    pub(crate) process_arch: &'static str,
    native_arch: Option<String>,
    os_version: Option<String>,
    pub(crate) execution: &'static str,
    pub(crate) privilege: &'static str,
    pub(crate) cwd_access: &'static str,
    agent_shell: &'static str,
    login_shell_hint: Option<String>,
    process_path_entries: Option<usize>,
    process_pathext_present: bool,
    user_path_entries: Option<usize>,
    machine_path_entries: Option<usize>,
    pub(crate) developer: Option<Developer>,
    pub(crate) ssh_indicator: bool,
    pub(crate) ci_indicator: bool,
    virtualization_indicator: bool,
    #[cfg(target_os = "linux")]
    linux: linux::Context,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Hardware {
    pub(crate) status: Status,
    visible_logical_cpus: Option<u64>,
    pub(crate) effective_cpus: Option<f64>,
    visible_memory_bytes: Option<u64>,
    pub(crate) effective_memory_bytes: Option<u64>,
    pub(crate) storage_available_bytes: Option<u64>,
    pub(crate) gpu_evidence: &'static str,
    #[cfg(target_os = "linux")]
    cgroup: linux::Limits,
    resource_source: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Developer {
    pub(crate) kind: &'static str,
    source: &'static str,
    path: Option<String>,
    selected_system_directory: Option<String>,
    swift_binary_observed: bool,
}

impl System {
    /// The process runs over SSH or inside a container/WSL boundary, so facts about
    /// local harness configuration describe this host, not the user's desktop.
    pub(crate) fn remote_or_isolated(&self) -> bool {
        self.ssh_indicator
            || !matches!(
                self.execution,
                "native" | "rosetta" | "wow64_or_emulated" | "linux_process" | "unknown"
            )
    }
}

fn present(key: &str) -> bool {
    env::var_os(key).is_some_and(|s| !s.is_empty())
}

pub(crate) fn system(cwd: &Path) -> System {
    #[cfg(target_os = "linux")]
    let linux = linux::context();
    #[cfg(target_os = "linux")]
    let execution = linux.execution();
    #[cfg(target_os = "macos")]
    let execution = if mac_integer(c"sysctl.proc_translated") == Some(1) {
        "rosetta"
    } else {
        "native"
    };
    #[cfg(windows)]
    let execution = windows::execution();
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    let execution = "unknown";
    #[cfg(windows)]
    let (user_path_entries, machine_path_entries) = windows::secondary_path_counts();
    #[cfg(not(windows))]
    let (user_path_entries, machine_path_entries) = (None, None);
    let shell = env::var_os("SHELL").and_then(|v| {
        let value = Path::new(&v).file_name()?.to_str()?;
        [
            "bash", "zsh", "fish", "dash", "sh", "ksh", "tcsh", "csh", "nu", "pwsh", "xonsh",
        ]
        .into_iter()
        .find(|s| *s == value)
        .map(str::to_owned)
    });
    let developer = None;
    let virtualization_indicator = virtualization();
    System {
        status: Status::Available,
        os: env::consts::OS,
        process_arch: env::consts::ARCH,
        native_arch: os_arch(execution),
        os_version: os_version(),
        execution,
        privilege: privilege(),
        cwd_access: access(cwd),
        agent_shell: "unknown",
        login_shell_hint: shell,
        process_path_entries: env::var_os("PATH").map(|p| env::split_paths(&p).take(97).count()),
        process_pathext_present: cfg!(windows) && present("PATHEXT"),
        user_path_entries,
        machine_path_entries,
        developer,
        ssh_indicator: ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
            .iter()
            .any(|s| present(s)),
        ci_indicator: ["CI", "GITHUB_ACTIONS", "GITLAB_CI", "TF_BUILD", "BUILDKITE"]
            .iter()
            .any(|s| present(s)),
        virtualization_indicator,
        #[cfg(target_os = "linux")]
        linux,
    }
}

fn virtualization() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac_integer(c"kern.hv_vmm_present") == Some(1)
    }
    #[cfg(target_os = "linux")]
    {
        linux::virtualization()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

fn os_arch(execution: &str) -> Option<String> {
    #[cfg(windows)]
    {
        let _ = execution;
        windows::native_arch().map(str::to_owned)
    }
    #[cfg(target_os = "macos")]
    {
        Some(
            if execution == "rosetta" {
                "aarch64"
            } else {
                env::consts::ARCH
            }
            .to_owned(),
        )
    }
    #[cfg(target_os = "linux")]
    {
        let _ = execution;
        linux::uname_machine()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = execution;
        None
    }
}

fn os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        mac_text(c"kern.osproductversion")
    }
    #[cfg(target_os = "linux")]
    {
        linux::version()
    }
    #[cfg(windows)]
    {
        windows::version()
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn privilege() -> &'static str {
    #[cfg(unix)]
    {
        if unsafe { libc::geteuid() } == 0 {
            "effective_uid_zero"
        } else {
            "standard"
        }
    }
    #[cfg(windows)]
    {
        windows::privilege()
    }
    #[cfg(not(any(unix, windows)))]
    {
        "unknown"
    }
}

#[cfg(unix)]
fn c_path(path: &Path) -> Option<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes()).ok()
}

fn access(cwd: &Path) -> &'static str {
    #[cfg(unix)]
    {
        let Some(path) = c_path(cwd) else {
            return "unknown";
        };
        let result = unsafe {
            libc::faccessat(
                libc::AT_FDCWD,
                path.as_ptr(),
                libc::W_OK | libc::X_OK,
                libc::AT_EACCESS,
            )
        };
        if result == 0 {
            "writable_estimate"
        } else {
            match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::EROFS | libc::EACCES | libc::EPERM) => "not_writable_estimate",
                _ => "unknown",
            }
        }
    }
    #[cfg(windows)]
    {
        windows::directory_access(cwd)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = cwd;
        "unknown"
    }
}

#[cfg(target_os = "macos")]
fn mac_read(name: &std::ffi::CStr) -> Option<([u8; 128], usize)> {
    let mut bytes = [0u8; 128];
    let mut len = bytes.len();
    (unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            bytes.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0
        && len <= bytes.len())
    .then_some((bytes, len))
}
#[cfg(target_os = "macos")]
fn mac_text(name: &std::ffi::CStr) -> Option<String> {
    let (bytes, size) = mac_read(name)?;
    let value = std::str::from_utf8(bytes.get(..size)?)
        .ok()?
        .trim_end_matches('\0');
    safe::token(value, 64)
}
#[cfg(target_os = "macos")]
fn mac_integer(name: &std::ffi::CStr) -> Option<u64> {
    let (bytes, size) = mac_read(name)?;
    match size {
        4 => Some(u64::from(u32::from_ne_bytes(bytes[..4].try_into().ok()?))),
        8 => Some(u64::from_ne_bytes(bytes[..8].try_into().ok()?)),
        _ => None,
    }
}

#[cfg(any(target_os = "macos", test))]
fn classify_developer(path: &Path) -> (&'static str, bool) {
    let swift = path.join("Toolchains/XcodeDefault.xctoolchain/usr/bin/swift");
    if swift.is_file() && path.join("Platforms/MacOSX.platform").is_dir() {
        ("xcode", true)
    } else if path.join("usr/bin/clang").is_file() && path.join("SDKs").is_dir() {
        ("command_line_tools", path.join("usr/bin/swift").is_file())
    } else {
        ("incomplete_or_inaccessible", false)
    }
}

#[cfg(target_os = "macos")]
pub(crate) async fn mac_developer(cwd: &Path) -> (Developer, Option<std::path::PathBuf>) {
    use std::{fs, path::PathBuf, time::Duration};
    // Apple's fixed read-only query resolves a default installation without starting
    // a developer tool or asking xcrun to install or update command line tools.
    let query = if cwd.is_dir() {
        crate::probe::run(
            Path::new("/usr/bin/xcode-select"),
            &["-p"],
            cwd,
            Duration::from_millis(125),
        )
        .await
        .ok()
        .and_then(|v| String::from_utf8(v).ok())
        .map(|v| PathBuf::from(v.trim()))
        .filter(|p| p.is_absolute() && p.is_dir())
    } else {
        None
    };
    let system = fs::read_link("/var/db/xcode_select_link")
        .or_else(|_| fs::read_link("/var/db/xcode-select-link"))
        .ok()
        .map(|p| {
            if p.is_absolute() {
                p
            } else {
                Path::new("/var/db").join(p)
            }
        });
    let fallback = [
        "/Applications/Xcode.app/Contents/Developer",
        "/Library/Developer/CommandLineTools",
    ]
    .into_iter()
    .find(|p| classify_developer(Path::new(p)).0 != "incomplete_or_inaccessible");
    let system = query.clone().or(system);
    let (path, source) =
        if let Some(override_dir) = env::var_os("DEVELOPER_DIR").filter(|s| !s.is_empty()) {
            (Some(PathBuf::from(override_dir)), "DEVELOPER_DIR")
        } else if system.is_some() {
            (
                system.clone(),
                if query.is_some() {
                    "xcode_select_print_path"
                } else {
                    "xcode_select_link"
                },
            )
        } else {
            (
                fallback.map(PathBuf::from),
                "conventional_install_not_confirmed_selected",
            )
        };
    let path = path.map(|p| {
        if p.extension().is_some_and(|e| e == "app") {
            p.join("Contents/Developer")
        } else {
            p
        }
    });
    let (kind, swift_binary_observed) = path
        .as_deref()
        .filter(|p| p.is_absolute())
        .map(classify_developer)
        .unwrap_or(("unknown", false));
    let selected = path
        .clone()
        .filter(|p| p.is_absolute() && source != "conventional_install_not_confirmed_selected");
    (
        Developer {
            kind,
            source,
            path: path.as_deref().map(safe::path),
            selected_system_directory: system.as_deref().map(safe::path),
            swift_binary_observed,
        },
        selected,
    )
}

pub(crate) fn hardware(cwd: &Path, deeper: bool) -> Hardware {
    let available = std::thread::available_parallelism()
        .ok()
        .map(|n| n.get() as u64);
    #[cfg(target_os = "linux")]
    let (
        visible_logical_cpus,
        visible_memory_bytes,
        effective_cpus,
        effective_memory_bytes,
        cgroup,
        gpu,
    ) = linux::hardware(available, deeper);
    #[cfg(target_os = "macos")]
    let (visible_logical_cpus, visible_memory_bytes, effective_cpus, effective_memory_bytes, gpu) = {
        let cpus = mac_integer(c"hw.logicalcpu").or(available);
        let memory = mac_integer(c"hw.memsize");
        let gpu = if deeper { metal() } else { "not_requested" };
        (
            cpus,
            memory,
            available.or(cpus).map(|n| n as f64),
            memory,
            gpu,
        )
    };
    #[cfg(windows)]
    let (visible_logical_cpus, visible_memory_bytes, effective_cpus, effective_memory_bytes, gpu) =
        windows::hardware(available, deeper);
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let (visible_logical_cpus, visible_memory_bytes, effective_cpus, effective_memory_bytes, gpu) = (
        available,
        None,
        available.map(|n| n as f64),
        None,
        "unsupported",
    );
    let storage_available_bytes = deeper.then(|| storage(cwd)).flatten();
    Hardware {
        status: if effective_cpus.is_some() && effective_memory_bytes.is_some() {
            Status::Available
        } else {
            Status::Unknown
        },
        visible_logical_cpus,
        effective_cpus,
        visible_memory_bytes,
        effective_memory_bytes,
        storage_available_bytes,
        gpu_evidence: gpu,
        resource_source: if cfg!(target_os = "linux") {
            "visible_os_affinity_and_cgroup"
        } else if cfg!(windows) {
            "system_and_readable_job_limits"
        } else {
            "visible_os"
        },
        #[cfg(target_os = "linux")]
        cgroup,
    }
}

#[cfg(target_os = "macos")]
fn metal() -> &'static str {
    use std::ffi::c_void;
    #[link(name = "Metal", kind = "framework")]
    unsafe extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut c_void;
    }
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_release(object: *mut c_void);
    }
    unsafe {
        let device = MTLCreateSystemDefaultDevice();
        if device.is_null() {
            "metal_device_not_observed"
        } else {
            objc_release(device);
            "metal_device_available"
        }
    }
}

fn storage(cwd: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        let path = c_path(cwd)?;
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
            return None;
        }
        // POSIX sizes and block-count typedefs differ among targets.
        fn number(value: impl TryInto<u64>) -> Option<u64> {
            value.try_into().ok()
        }
        Some(number(stats.f_bavail)?.saturating_mul(number(stats.f_frsize)?))
    }
    #[cfg(windows)]
    {
        windows::storage(cwd)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = cwd;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn developer_directory_is_classified_by_usable_layout_not_name() {
        let temp = tempfile::tempdir().unwrap();
        let fake = temp.path().join("Xcode.app");
        std::fs::create_dir(&fake).unwrap();
        assert_eq!(
            classify_developer(&fake),
            ("incomplete_or_inaccessible", false)
        );
        std::fs::create_dir_all(fake.join("SDKs")).unwrap();
        std::fs::create_dir_all(fake.join("usr/bin")).unwrap();
        std::fs::write(fake.join("usr/bin/clang"), "").unwrap();
        assert_eq!(classify_developer(&fake), ("command_line_tools", false));
        std::fs::create_dir_all(fake.join("Platforms/MacOSX.platform")).unwrap();
        let tools = fake.join("Toolchains/XcodeDefault.xctoolchain/usr/bin");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::write(tools.join("swift"), "").unwrap();
        assert_eq!(classify_developer(&fake), ("xcode", true));
    }
}
