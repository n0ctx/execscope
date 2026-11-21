use crate::safe;
use std::{
    ffi::{OsString, c_void},
    mem::{size_of, zeroed},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_WRITE_PROTECT, GetLastError, INVALID_HANDLE_VALUE,
    },
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW},
    System::{
        JobObjects::*,
        Registry::*,
        SystemInformation::*,
        Threading::{GetCurrentProcess, IsWow64Process2, OpenProcessToken},
    },
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn architectures() -> (Option<&'static str>, Option<bool>) {
    let mut process = 0;
    let mut native = 0;
    if unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, &mut native) } == 0 {
        return (None, None);
    }
    let arch = match native {
        0x8664 => Some("x86_64"),
        0xaa64 => Some("aarch64"),
        0x014c => Some("x86"),
        _ => None,
    };
    (arch, Some(process != 0))
}
pub(super) fn native_arch() -> Option<&'static str> {
    architectures().0
}
pub(super) fn execution() -> &'static str {
    match architectures().1 {
        Some(true) => "wow64_or_emulated",
        Some(false) => "native",
        None => "unknown",
    }
}

pub(super) fn version() -> Option<String> {
    let key = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
    let build = text(HKEY_LOCAL_MACHINE, key, "CurrentBuildNumber")?;
    let build = safe::token(build.to_str()?, 16)?;
    let release =
        text(HKEY_LOCAL_MACHINE, key, "DisplayVersion").and_then(|v| safe::token(v.to_str()?, 16));
    Some(release.map_or_else(|| build.clone(), |r| format!("{r} build {build}")))
}

pub(super) fn privilege() -> &'static str {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return "unknown";
    }
    let mut elevated: TOKEN_ELEVATION = unsafe { zeroed() };
    let mut returned = 0;
    let success = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevated as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    unsafe {
        CloseHandle(token);
    }
    if success == 0 {
        "unknown"
    } else if elevated.TokenIsElevated != 0 {
        "elevated"
    } else {
        "standard"
    }
}

pub(crate) fn text(root: HKEY, key: &str, name: &str) -> Option<OsString> {
    let mut buf = vec![0u16; 8192];
    let mut bytes = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            root,
            wide(key).as_ptr(),
            wide(name).as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if ok != 0 || !bytes.is_multiple_of(2) || bytes as usize > buf.len() * 2 {
        return None;
    }
    buf.truncate(bytes as usize / 2);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    if buf.contains(&0) {
        return None;
    }
    Some(OsString::from_wide(&buf))
}

pub(crate) fn paths() -> (Option<OsString>, Option<OsString>) {
    (
        text(HKEY_CURRENT_USER, "Environment", "PATH"),
        text(
            HKEY_LOCAL_MACHINE,
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            "PATH",
        ),
    )
}
pub(super) fn secondary_path_counts() -> (Option<usize>, Option<usize>) {
    let (user, machine) = paths();
    let count = |s: OsString| std::env::split_paths(&s).take(97).count();
    (user.map(count), machine.map(count))
}

pub(crate) fn local(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                let root = [u16::from(drive), u16::from(b':'), u16::from(b'\\'), 0];
                // Fixed and RAM drives only. Mapped network drives, UNC paths,
                // opaque device names, and removable media are not probed.
                matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 3 | 6)
            }
            _ => false,
        },
        _ => false,
    }
}

pub(crate) fn file_version(path: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    let p: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let size = unsafe { GetFileVersionInfoSizeW(p.as_ptr(), ptr::null_mut()) };
    if size == 0 || size > 64 * 1024 {
        return None;
    }
    let mut data = vec![0u32; (size as usize).div_ceil(4)];
    if unsafe { GetFileVersionInfoW(p.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let mut value = ptr::null_mut();
    let mut len = 0;
    let key = [u16::from(b'\\'), 0];
    if unsafe { VerQueryValueW(data.as_ptr().cast(), key.as_ptr(), &mut value, &mut len) } == 0
        || len < size_of::<VS_FIXEDFILEINFO>() as u32
        || value.is_null()
    {
        return None;
    }
    let info = unsafe { value.cast::<VS_FIXEDFILEINFO>().read_unaligned() };
    if info.dwSignature != 0xfeef04bd {
        return None;
    }
    let a = info.dwFileVersionMS;
    let b = info.dwFileVersionLS;
    Some(format!(
        "{}.{}.{}.{}",
        a >> 16,
        a & 0xffff,
        b >> 16,
        b & 0xffff
    ))
}

pub(super) fn directory_access(path: &Path) -> &'static str {
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ADD_FILE, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TRAVERSE, OPEN_EXISTING,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_ADD_FILE | FILE_TRAVERSE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if handle != INVALID_HANDLE_VALUE {
        unsafe { CloseHandle(handle) };
        "writable_estimate"
    } else if matches!(
        unsafe { GetLastError() },
        ERROR_ACCESS_DENIED | ERROR_WRITE_PROTECT
    ) {
        "not_writable_estimate"
    } else {
        "unknown"
    }
}

pub(super) fn storage(path: &Path) -> Option<u64> {
    let p: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut bytes = 0;
    (unsafe { GetDiskFreeSpaceExW(p.as_ptr(), &mut bytes, ptr::null_mut(), ptr::null_mut()) } != 0)
        .then_some(bytes)
}

type HardwareFacts = (
    Option<u64>,
    Option<u64>,
    Option<f64>,
    Option<u64>,
    &'static str,
);
pub(super) fn hardware(available: Option<u64>, deeper: bool) -> HardwareFacts {
    let mut status: MEMORYSTATUSEX = unsafe { zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    let memory = (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullTotalPhys);
    let mut system: SYSTEM_INFO = unsafe { zeroed() };
    unsafe { GetNativeSystemInfo(&mut system) };
    let logical = (system.dwNumberOfProcessors > 0).then(|| u64::from(system.dwNumberOfProcessors));
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    let mut cpu: JOBOBJECT_CPU_RATE_CONTROL_INFORMATION = unsafe { zeroed() };
    let mut effective_memory = memory;
    let mut effective_cpu = available.or(logical).map(|n| n as f64);
    if job(JobObjectExtendedLimitInformation, &mut limits) {
        for (flag, bound) in [
            (JOB_OBJECT_LIMIT_JOB_MEMORY, limits.JobMemoryLimit),
            (JOB_OBJECT_LIMIT_PROCESS_MEMORY, limits.ProcessMemoryLimit),
        ] {
            if limits.BasicLimitInformation.LimitFlags & flag != 0 && bound > 0 {
                effective_memory =
                    Some(effective_memory.map_or(bound as u64, |n| n.min(bound as u64)));
            }
        }
    }
    if job(JobObjectCpuRateControlInformation, &mut cpu)
        && cpu.ControlFlags
            & (JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP)
            == (JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP)
    {
        let rate = unsafe { cpu.Anonymous.CpuRate };
        if (1..=10_000).contains(&rate)
            && let Some(count) = logical
        {
            let quota = count as f64 * f64::from(rate) / 10_000.0;
            effective_cpu = Some(effective_cpu.map_or(quota, |n| n.min(quota)));
        }
    }
    (
        logical,
        memory,
        effective_cpu,
        effective_memory,
        if deeper {
            display_evidence()
        } else {
            "not_requested"
        },
    )
}

fn display_evidence() -> &'static str {
    use windows_sys::Win32::Devices::Display::{
        GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS,
    };
    let (mut paths, mut modes) = (0, 0);
    match unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes) } {
        0 if paths > 0 => "active_display_path_observed_compute_unverified",
        0 => "display_path_not_observed",
        _ => "windows_display_evidence_inaccessible",
    }
}

fn job<T>(class: JOBOBJECTINFOCLASS, value: &mut T) -> bool {
    unsafe {
        QueryInformationJobObject(
            ptr::null_mut(),
            class,
            (value as *mut T).cast::<c_void>(),
            size_of::<T>() as u32,
            ptr::null_mut(),
        ) != 0
    }
}
