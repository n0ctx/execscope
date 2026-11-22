use crate::safe;
use serde::Serialize;
use std::{
    env,
    path::{Component, Path, PathBuf},
};

fn read(path: &str, max: usize) -> Option<String> {
    safe::read(Path::new(path), max)
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Context {
    wsl_indicator: bool,
    interop_indicator: bool,
    append_windows_path: Option<bool>,
    windows_mount_in_path: bool,
    container_indicator: Option<&'static str>,
    kubernetes_indicator: bool,
    pid_namespace_differs_from_pid1: Option<bool>,
}

impl Context {
    pub(super) fn execution(&self) -> &'static str {
        match (self.wsl_indicator, self.container_indicator.is_some()) {
            (true, true) => "wsl_container",
            (true, false) => "wsl",
            (false, true) => "container",
            (false, false) => "linux_process",
        }
    }
}

pub(super) fn context() -> Context {
    let release = read("/proc/sys/kernel/osrelease", 256).unwrap_or_default();
    let cgroup = read("/proc/self/cgroup", 16 * 1024).unwrap_or_default();
    let mut context = detect(
        &release,
        &cgroup,
        read("/etc/wsl.conf", 16 * 1024).as_deref(),
        env::var_os("WSL_INTEROP").is_some(),
        Path::new("/.dockerenv").is_file(),
        Path::new("/run/.containerenv").is_file(),
    );
    context.kubernetes_indicator |= env::var_os("KUBERNETES_SERVICE_HOST").is_some();
    if context.kubernetes_indicator && context.container_indicator.is_none() {
        context.container_indicator = Some("kubernetes_indicator");
    }
    context.windows_mount_in_path = context.wsl_indicator
        && env::var_os("PATH").is_some_and(|path| {
            env::split_paths(&path).take(96).any(|p| {
                p.strip_prefix("/mnt")
                    .ok()
                    .and_then(|r| r.components().next())
                    .is_some_and(|c| c.as_os_str().as_encoded_bytes().len() == 1)
            })
        });
    context.pid_namespace_differs_from_pid1 = std::fs::read_link("/proc/self/ns/pid")
        .ok()
        .zip(std::fs::read_link("/proc/1/ns/pid").ok())
        .map(|(a, b)| a != b);
    context
}

fn detect(
    release: &str,
    cgroup: &str,
    config: Option<&str>,
    interop: bool,
    docker: bool,
    podman: bool,
) -> Context {
    let wsl_indicator = interop || release.to_ascii_lowercase().contains("microsoft");
    let kubernetes_indicator = cgroup.contains("kubepods");
    let container_indicator = if docker || cgroup.contains("docker") {
        Some("docker_marker")
    } else if podman || cgroup.contains("libpod") {
        Some("podman_marker")
    } else if kubernetes_indicator {
        Some("kubernetes_cgroup")
    } else if cgroup.contains("lxc") {
        Some("lxc_cgroup")
    } else {
        None
    };
    let append_windows_path = if wsl_indicator {
        let mut section = "";
        config.and_then(|s| {
            s.lines().take(200).find_map(|line| {
                let line = line.split(['#', ';']).next()?.trim();
                if let Some(name) = line.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
                    section = name.trim();
                    return None;
                }
                let (key, value) = line.split_once('=')?;
                if section == "interop" && key.trim() == "appendWindowsPath" {
                    value.trim().parse().ok()
                } else {
                    None
                }
            })
        })
    } else {
        None
    };
    Context {
        wsl_indicator,
        interop_indicator: interop,
        append_windows_path,
        windows_mount_in_path: false,
        container_indicator,
        kubernetes_indicator,
        pid_namespace_differs_from_pid1: None,
    }
}

pub(super) fn virtualization() -> bool {
    read("/sys/hypervisor/type", 128)
        .is_some_and(|s| matches!(s.trim(), "xen" | "kvm" | "microsoft"))
        || read("/sys/class/dmi/id/sys_vendor", 128).is_some_and(|s| {
            [
                "VMware",
                "QEMU",
                "Microsoft Corporation",
                "Xen",
                "innotek GmbH",
                "Amazon EC2",
            ]
            .contains(&s.trim())
        })
}

#[cfg(target_os = "linux")]
pub(super) fn uname_machine() -> Option<String> {
    let mut data: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut data) } != 0 {
        return None;
    }
    let raw = unsafe { std::ffi::CStr::from_ptr(data.machine.as_ptr()) };
    raw.to_str().ok().and_then(|s| safe::token(s, 24))
}

pub(super) fn version() -> Option<String> {
    let text =
        read("/etc/os-release", 16 * 1024).or_else(|| read("/usr/lib/os-release", 16 * 1024))?;
    let field = |key| {
        text.lines().find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k == key)
                .then(|| v.trim_matches(['\'', '"']))
                .and_then(|s| safe::token(s, 40))
        })
    };
    match (field("ID"), field("VERSION_ID")) {
        (Some(name), Some(version)) => Some(format!("{name} {version}")),
        (name, _) => name,
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub(super) struct Limits {
    cpu_quota: Option<f64>,
    cpuset_cpus: Option<u64>,
    affinity_cpus: Option<u64>,
    memory_limit_bytes: Option<u64>,
    hierarchy: &'static str,
}

pub(super) type HardwareFacts = (
    Option<u64>,
    Option<u64>,
    Option<f64>,
    Option<u64>,
    Limits,
    &'static str,
);

pub(super) fn hardware(available: Option<u64>, deeper: bool) -> HardwareFacts {
    let logical = read("/sys/devices/system/cpu/online", 4096)
        .and_then(|s| cpuset(s.trim()))
        .or(available);
    let mem = read("/proc/meminfo", 16 * 1024).and_then(|s| {
        s.lines().find_map(|l| {
            l.strip_prefix("MemTotal:").and_then(|v| {
                v.split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()?
                    .checked_mul(1024)
            })
        })
    });
    let cgroup = read("/proc/self/cgroup", 32 * 1024).unwrap_or_default();
    let mounts = read("/proc/self/mountinfo", 384 * 1024).unwrap_or_default();
    let mut limits = cgroup_limits(&cgroup, &mounts, |p| safe::read(p, 4096));
    limits.affinity_cpus = read("/proc/self/status", 48 * 1024).and_then(|s| {
        s.lines().find_map(|l| {
            l.strip_prefix("Cpus_allowed_list:")
                .and_then(|v| cpuset(v.trim()))
        })
    });
    let eff_cpu = [
        available.map(|n| n as f64),
        limits.cpu_quota,
        limits.cpuset_cpus.map(|n| n as f64),
        limits.affinity_cpus.map(|n| n as f64),
    ]
    .into_iter()
    .flatten()
    .reduce(f64::min);
    let eff_mem = [mem, limits.memory_limit_bytes].into_iter().flatten().min();
    let gpu = if !deeper {
        "not_requested"
    } else if Path::new("/dev/nvidiactl").exists()
        && (0..8).any(|i| Path::new(&format!("/dev/nvidia{i}")).exists())
    {
        if read("/proc/driver/nvidia/version", 2048).is_some_and(|s| s.starts_with("NVRM version:"))
        {
            "nvidia_driver_and_nodes_observed_cuda_unverified"
        } else {
            "nvidia_nodes_observed_cuda_unverified"
        }
    } else if Path::new("/dev/dxg").exists() {
        "wsl_dxg_node_observed"
    } else if (128..136).any(|i| Path::new(&format!("/dev/dri/renderD{i}")).exists()) {
        "drm_render_node_observed"
    } else {
        "device_not_observed"
    };
    (logical, mem, eff_cpu, eff_mem, limits, gpu)
}

fn cpuset(text: &str) -> Option<u64> {
    let mut ranges = Vec::<(u32, u32)>::new();
    for range in text.split(',').take(257) {
        let (start, end) = range.split_once('-').unwrap_or((range, range));
        let (start, end) = (start.parse().ok()?, end.parse().ok()?);
        if start > end || ranges.len() == 256 {
            return None;
        }
        ranges.push((start, end));
    }
    ranges.sort_unstable();
    let mut total = 0u64;
    let mut previous = None;
    for (a, b) in ranges {
        let start = previous.map_or(u64::from(a), |end: u64| (end + 1).max(u64::from(a)));
        if start <= u64::from(b) {
            total += u64::from(b) - start + 1;
        }
        previous = Some(previous.map_or(u64::from(b), |end| end.max(u64::from(b))));
    }
    Some(total)
}

fn cgroup_limits(groups: &str, mounts: &str, read: impl Fn(&Path) -> Option<String>) -> Limits {
    let mut result = Limits {
        hierarchy: "not_observed",
        ..Limits::default()
    };
    let mut seen = [false; 2];
    for group in groups.lines().take(64) {
        let mut parts = group.splitn(3, ':');
        let (Some(_), Some(controllers), Some(group_path)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let v2 = controllers.is_empty();
        let path = Path::new(group_path);
        if !strict_absolute(path) {
            continue;
        }
        let selected = mounts
            .lines()
            .take(1024)
            .filter_map(|line| mount_for(line, path, controllers))
            .max_by_key(|(root, _, _, _)| root.components().count());
        let Some((root, mount, cpu, mem)) = selected else {
            continue;
        };
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let member = mount.join(relative);
        let cpus = v2 || controllers.split(',').any(|c| c == "cpuset");
        seen[usize::from(v2)] = true;
        for level in member
            .ancestors()
            .take(32)
            .take_while(|p| p.starts_with(&mount))
        {
            let file = |name: &str| read(&level.join(name));
            if cpu {
                let raw = if v2 {
                    file("cpu.max").and_then(|t| {
                        let mut w = t.split_whitespace();
                        Some((
                            w.next()?.parse::<u64>().ok()?,
                            w.next()?.parse::<u64>().ok()?,
                        ))
                    })
                } else {
                    file("cpu.cfs_quota_us")
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .zip(file("cpu.cfs_period_us").and_then(|s| s.trim().parse::<u64>().ok()))
                };
                if let Some((q, p)) = raw.filter(|(q, p)| *q > 0 && *p > 0) {
                    let n = q as f64 / p as f64;
                    result.cpu_quota = Some(result.cpu_quota.map_or(n, |x| x.min(n)));
                }
            }
            if mem
                && let Some(n) = file(if v2 {
                    "memory.max"
                } else {
                    "memory.limit_in_bytes"
                })
                .and_then(|s| s.trim().parse::<u64>().ok())
                .filter(|n| *n < (1 << 60))
            {
                result.memory_limit_bytes = Some(result.memory_limit_bytes.map_or(n, |x| x.min(n)));
            }
            if cpus
                && let Some(n) = file(if v2 {
                    "cpuset.cpus.effective"
                } else {
                    "cpuset.effective_cpus"
                })
                .or_else(|| file("cpuset.cpus"))
                .and_then(|s| cpuset(s.trim()))
            {
                result.cpuset_cpus = Some(result.cpuset_cpus.map_or(n, |x| x.min(n)));
            }
        }
    }
    result.hierarchy = match seen {
        [true, true] => "hybrid",
        [true, false] => "v1",
        [false, true] => "v2",
        _ => "not_observed",
    };
    result
}

fn mount_for(
    line: &str,
    group: &Path,
    controllers: &str,
) -> Option<(PathBuf, PathBuf, bool, bool)> {
    let (pre, post) = line.split_once(" - ")?;
    let fields: Vec<_> = pre.split_whitespace().take(5).collect();
    let mut post = post.split_whitespace();
    let fs = post.next()?;
    let mount_options = post.nth(1)?;
    let v2 = fs == "cgroup2" && controllers.is_empty();
    let is_controller = |key| {
        controllers.split(',').any(|s| s == key) && mount_options.split(',').any(|s| s == key)
    };
    let (cpu, mem) = (v2 || is_controller("cpu"), v2 || is_controller("memory"));
    if !v2 && (fs != "cgroup" || !(cpu || mem || is_controller("cpuset"))) {
        return None;
    }
    let decode = |s: &str| {
        PathBuf::from(
            s.replace("\\040", " ")
                .replace("\\011", "\t")
                .replace("\\012", "\n")
                .replace("\\134", "\\"),
        )
    };
    let root = decode(fields.get(3)?);
    let mount = decode(fields.get(4)?);
    if !strict_absolute(&root) || !strict_absolute(&mount) || !group.starts_with(&root) {
        return None;
    }
    Some((root, mount, cpu, mem))
}

fn strict_absolute(path: &Path) -> bool {
    path.is_absolute() && !path.components().any(|c| c == Component::ParentDir)
}

#[cfg(test)]
mod tests {
    use super::*;
    // These fixtures use POSIX absolute paths; Windows Path does not treat them as absolute.
    #[cfg(unix)]
    #[test]
    fn resolves_v2_child_and_parent_limits_at_mount_root() {
        let mounts = "21 20 0:2 /tenant /sys/fs/cgroup rw - cgroup2 cgroup rw";
        let limits = cgroup_limits("0::/tenant/app", mounts, |p| {
            Some(
                match p.to_str()? {
                    "/sys/fs/cgroup/app/cpu.max" => "max 100000",
                    "/sys/fs/cgroup/cpu.max" => "150000 100000",
                    "/sys/fs/cgroup/app/memory.max" => "209715200",
                    "/sys/fs/cgroup/memory.max" => "1073741824",
                    "/sys/fs/cgroup/app/cpuset.cpus.effective" => "0-1,3,3-4",
                    _ => return None,
                }
                .into(),
            )
        });
        assert_eq!(limits.hierarchy, "v2");
        assert_eq!(limits.cpu_quota, Some(1.5));
        assert_eq!(limits.memory_limit_bytes, Some(209715200));
        assert_eq!(limits.cpuset_cpus, Some(4));
    }
    #[cfg(unix)]
    #[test]
    fn v1_split_mounts_and_unlimited_values() {
        let mounts = "10 1 0:10 / /c/cpu rw - cgroup cgroup rw,cpu,cpuacct\n11 1 0:11 / /c/mem rw - cgroup cgroup rw,memory\n12 1 0:12 / /c/set rw - cgroup cgroup rw,cpuset";
        let limits = cgroup_limits(
            "2:cpu,cpuacct:/app\n3:memory:/app\n4:cpuset:/app",
            mounts,
            |p| {
                Some(
                    match p.to_str()? {
                        "/c/cpu/app/cpu.cfs_quota_us" => "25000",
                        "/c/cpu/app/cpu.cfs_period_us" => "100000",
                        "/c/mem/app/memory.limit_in_bytes" => "9223372036854771712",
                        "/c/mem/memory.limit_in_bytes" => "104857600",
                        "/c/set/app/cpuset.cpus" => "3-4",
                        _ => return None,
                    }
                    .into(),
                )
            },
        );
        assert_eq!(limits.hierarchy, "v1");
        assert_eq!(limits.cpu_quota, Some(0.25));
        assert_eq!(limits.memory_limit_bytes, Some(104857600));
        assert_eq!(limits.cpuset_cpus, Some(2));
    }
    #[test]
    fn wsl_and_container_are_independent_indicators() {
        let ctx = detect(
            "6.4-microsoft-standard-WSL2",
            "0::/kubepods/unit",
            Some("[network]\nappendWindowsPath=true\n[interop]\nappendWindowsPath = false"),
            false,
            false,
            false,
        );
        assert_eq!(ctx.execution(), "wsl_container");
        assert_eq!(ctx.append_windows_path, Some(false));
        assert!(ctx.kubernetes_indicator);
        assert_eq!(
            detect("6.1.1", "0::/", None, false, true, false).execution(),
            "container"
        );
    }
}
