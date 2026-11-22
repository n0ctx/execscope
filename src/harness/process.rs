//! Bounded, read-only view of the ExecScope process's ancestors.
//!
//! Only the executable path and the first few argv entries are kept, and they are
//! used solely for matching against signatures; they are never copied into reports
//! because arguments can carry secrets.

use std::path::PathBuf;

pub(crate) const MAX_ANCESTORS: usize = 16;
// Windows exposes no other process's arguments without reading its memory.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
const MAX_ARGS: usize = 6;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
const MAX_ARG_BYTES: usize = 4096;

#[derive(Debug, Clone, Default)]
pub(crate) struct Ancestor {
    /// Executable image path reported by the OS, when readable.
    pub(crate) exe: Option<PathBuf>,
    /// argv[0..6]; empty when the OS does not expose arguments for this process.
    pub(crate) args: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Ancestry {
    /// Nearest parent first.
    pub(crate) chain: Vec<Ancestor>,
    /// The platform offered no readable process table (for example, no `/proc`).
    pub(crate) unavailable: bool,
    /// The walk stopped before the root: an unreadable ancestor (for example,
    /// `/proc` hidepid), a parent outside a PID namespace, or the depth cap.
    pub(crate) incomplete: bool,
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn bounded_args(args: impl Iterator<Item = String>) -> Vec<String> {
    args.take(MAX_ARGS)
        .filter(|a| a.len() <= MAX_ARG_BYTES)
        .collect()
}

#[cfg(target_os = "macos")]
pub(crate) fn ancestry() -> Ancestry {
    let mut chain = Vec::new();
    let mut pid = unsafe { libc::getppid() };
    let mut args = vec![0u8; mac_argmax()];
    let mut incomplete = false;
    // PID 1 is included: inside a container a harness can be the init process.
    while pid >= 1 {
        if chain.len() == MAX_ANCESTORS {
            incomplete = true;
            break;
        }
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let read = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        // `proc_pidinfo` refuses processes of other users (such as root-owned `login`
        // in every Terminal session); `KERN_PROC_PID` still gives their parent.
        let Some(ppid) = (read == size)
            .then_some(info.pbi_ppid as libc::pid_t)
            .or_else(|| mac_ppid(pid))
        else {
            incomplete = !chain.is_empty();
            break;
        };
        let mut buf = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        let exe = (len > 0)
            .then(|| {
                use std::os::unix::ffi::OsStrExt;
                PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len as usize]))
            })
            .filter(|p| p.is_absolute());
        chain.push(Ancestor {
            exe,
            args: mac_args(pid, &mut args),
        });
        if pid == 1 {
            break;
        }
        pid = ppid;
    }
    Ancestry {
        unavailable: chain.is_empty() && unsafe { libc::getppid() } > 1,
        incomplete,
        chain,
    }
}

/// Parent id from `KERN_PROC_PID`. libc does not bind Darwin's `kinfo_proc`, so the
/// field is read at its fixed offset; the reply length guards the layout.
#[cfg(target_os = "macos")]
fn mac_ppid(pid: libc::pid_t) -> Option<libc::pid_t> {
    const KINFO_PROC_SIZE: usize = 648;
    const E_PPID_OFFSET: usize = 560;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    let mut buf = [0u8; KINFO_PROC_SIZE];
    let mut len = buf.len();
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if !ok || len != KINFO_PROC_SIZE {
        return None;
    }
    let bytes = buf.get(E_PPID_OFFSET..E_PPID_OFFSET + 4)?;
    Some(libc::pid_t::from_ne_bytes(bytes.try_into().ok()?))
}

/// `KERN_ARGMAX` bounds a process's argument and environment area.
#[cfg(target_os = "macos")]
fn mac_argmax() -> usize {
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let mut value: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut value).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    usize::try_from(value)
        .ok()
        .filter(|v| ok && *v > 0)
        .unwrap_or(256 * 1024)
        .min(4 * 1024 * 1024)
}

#[cfg(target_os = "macos")]
fn mac_args(pid: libc::pid_t, buf: &mut [u8]) -> Vec<String> {
    // KERN_PROCARGS2: argc (i32), exec path, NUL padding, then argv strings.
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut len = buf.len();
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    if !ok || len < 4 || len > buf.len() {
        return vec![];
    }
    parse_procargs2(&buf[..len])
}

#[cfg(any(target_os = "macos", test))]
fn parse_procargs2(buf: &[u8]) -> Vec<String> {
    let Some(argc) = buf
        .get(..4)
        .map(|b| i32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
    else {
        return vec![];
    };
    let mut rest = &buf[4..];
    // Skip the exec path and its NUL padding.
    let Some(end) = rest.iter().position(|b| *b == 0) else {
        return vec![];
    };
    rest = &rest[end..];
    let start = rest.iter().position(|b| *b != 0).unwrap_or(rest.len());
    rest = &rest[start..];
    bounded_args(
        rest.split(|b| *b == 0)
            .take(argc.clamp(0, MAX_ARGS as i32) as usize)
            .filter_map(|s| String::from_utf8(s.to_vec()).ok()),
    )
}

#[cfg(target_os = "linux")]
pub(crate) fn ancestry() -> Ancestry {
    use std::path::Path;
    let proc = Path::new("/proc");
    if !proc.join("self/stat").is_file() {
        return Ancestry {
            chain: vec![],
            unavailable: true,
            incomplete: false,
        };
    }
    let mut chain = Vec::new();
    let mut pid = i64::from(std::os::unix::process::parent_id());
    // A parent outside this PID namespace shows as 0.
    let mut incomplete = pid == 0;
    // PID 1 is included: inside a container a harness can be the init process.
    while pid >= 1 {
        if chain.len() == MAX_ANCESTORS {
            incomplete = true;
            break;
        }
        let dir = proc.join(pid.to_string());
        let Some(ppid) = crate::safe::read(&dir.join("stat"), 4096)
            .as_deref()
            .and_then(linux_ppid)
        else {
            incomplete = true;
            break;
        };
        let exe = std::fs::read_link(dir.join("exe"))
            .ok()
            .map(without_deleted_suffix)
            .filter(|p| p.is_absolute());
        let args = std::fs::File::open(dir.join("cmdline"))
            .ok()
            .and_then(|f| {
                use std::io::Read;
                let mut bytes = Vec::new();
                f.take(16 * 1024).read_to_end(&mut bytes).ok()?;
                Some(bytes)
            })
            .map(|bytes| {
                bounded_args(
                    bytes
                        .split(|b| *b == 0)
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| String::from_utf8(s.to_vec()).ok()),
                )
            })
            .unwrap_or_default();
        chain.push(Ancestor { exe, args });
        if pid == 1 {
            break;
        }
        if ppid == 0 {
            incomplete = true;
            break;
        }
        pid = ppid;
    }
    Ancestry {
        unavailable: false,
        incomplete,
        chain,
    }
}

/// The kernel marks a replaced or removed executable as `<path> (deleted)`; an updated
/// interpreter is still the same interpreter.
#[cfg(any(target_os = "linux", test))]
fn without_deleted_suffix(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|p| p.strip_suffix(" (deleted)")) {
        Some(stripped) => PathBuf::from(stripped),
        None => path,
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_ppid(stat: &str) -> Option<i64> {
    // The command name may contain spaces or parentheses; fields resume after the last ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(windows)]
pub(crate) fn ancestry() -> Ancestry {
    use std::{collections::HashMap, os::windows::ffi::OsStringExt};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                GetCurrentProcessId, GetProcessTimes, OpenProcess,
                PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
            },
        },
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Ancestry {
            chain: vec![],
            unavailable: true,
            incomplete: false,
        };
    }
    let mut parents = HashMap::new();
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while more && parents.len() < 65_536 {
        parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
        more = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    // Parent ids can be reused after the parent exits: require each ancestor to have
    // started before the process it supposedly launched.
    let created = |pid: u32| -> Option<(u64, Option<PathBuf>)> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
        let time = (unsafe { GetProcessTimes(handle, &mut c, &mut e, &mut k, &mut u) } != 0)
            .then(|| (u64::from(c.dwHighDateTime) << 32) | u64::from(c.dwLowDateTime));
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        let path = (unsafe { QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len) }
            != 0)
            .then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..len as usize])));
        unsafe { CloseHandle(handle) };
        Some((time?, path))
    };
    let mut chain = Vec::new();
    let mut child = unsafe { GetCurrentProcessId() };
    let mut child_time = created(child).map_or(u64::MAX, |(t, _)| t);
    let mut incomplete = false;
    // A parent that already exited (absent, or its id reused by a newer process) ends
    // the chain; one that exists but cannot be opened leaves it incomplete.
    while let Some(&parent) = parents.get(&child) {
        if parent == 0 || parent == child {
            break;
        }
        if chain.len() == MAX_ANCESTORS {
            incomplete = true;
            break;
        }
        let Some((time, exe)) = created(parent) else {
            incomplete = true;
            break;
        };
        if time > child_time {
            break;
        }
        chain.push(Ancestor { exe, args: vec![] });
        child = parent;
        child_time = time;
    }
    Ancestry {
        unavailable: false,
        incomplete,
        chain,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub(crate) fn ancestry() -> Ancestry {
    Ancestry {
        chain: vec![],
        unavailable: true,
        incomplete: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procargs2_and_proc_stat_parsing() {
        let mut buf = 2i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/usr/bin/node\0\0\0node\0/opt/x/cli.js\0--secret=1\0HOME=/x\0");
        assert_eq!(parse_procargs2(&buf), ["node", "/opt/x/cli.js"]);
        assert!(parse_procargs2(b"\x01").is_empty());
        assert_eq!(linux_ppid("123 (a) b) c) S 456 1 1"), Some(456));
        assert_eq!(linux_ppid("garbage"), None);
        assert_eq!(
            without_deleted_suffix(PathBuf::from("/usr/bin/node (deleted)")),
            PathBuf::from("/usr/bin/node")
        );
        assert_eq!(
            without_deleted_suffix(PathBuf::from("/usr/bin/node")),
            PathBuf::from("/usr/bin/node")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_parent_lookup_matches_the_os() {
        let me = std::process::id() as libc::pid_t;
        assert_eq!(mac_ppid(me), Some(unsafe { libc::getppid() }));
        assert_eq!(mac_ppid(1), Some(0));
        assert_eq!(mac_ppid(i32::MAX), None);
    }

    #[test]
    fn live_ancestry_is_bounded() {
        let ancestry = ancestry();
        assert!(ancestry.chain.len() <= MAX_ANCESTORS);
        assert!(ancestry.chain.iter().all(|a| a.args.len() <= MAX_ARGS));
    }
}
