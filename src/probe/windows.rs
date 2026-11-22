//! Native launch policy for pinned, cwd-independent Windows identity probes.
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io::Read,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{
        Authorization::{ConvertStringSidToSidW, GetSecurityInfo, SE_FILE_OBJECT},
        *,
    },
    Storage::FileSystem::*,
    System::{
        JobObjects::*,
        Pipes::{CreatePipe, PeekNamedPipe},
        SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW},
        Threading::*,
    },
};

// Published at https://nodejs.org/dist/v24.22.3/SHASUMS256.txt, win-x64/node.exe.
// Exact hashes also exclude another vendor-signed Node binary with different behavior.
const NODE_X64_V24_22_3: [u8; 32] = [
    0x86, 0xfc, 0x47, 0x85, 0x32, 0x7d, 0xb5, 0xf5, 0xc7, 0x55, 0x8d, 0xc6, 0x15, 0x6c, 0x54, 0x3b,
    0x43, 0x3c, 0x31, 0x3e, 0x81, 0x3a, 0x22, 0x2e, 0x08, 0x80, 0x86, 0x22, 0x2a, 0xb4, 0xaf, 0xa6,
];
const MAX_IMAGE: u64 = 192 * 1024 * 1024;
const MAX_OUTPUT: usize = 4096;

struct Cancellation(Arc<AtomicBool>);
impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub(super) async fn identity(path: &Path, timeout: Duration) -> Result<&'static str, &'static str> {
    let start = Instant::now();
    let cancelled = Cancellation(Arc::new(AtomicBool::new(false)));
    let flag = cancelled.0.clone();
    let path = path.to_owned();
    tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            let check = || {
                if start.elapsed() >= timeout || flag.load(Ordering::Relaxed) {
                    Err("timed_out")
                } else {
                    Ok(())
                }
            };
            verify(&path, &check)
        }),
    )
    .await
    .map_err(|_| "timed_out")?
    .map_err(|_| "probe_failed")?
}

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain([0]).collect()
}

fn directory(get: unsafe extern "system" fn(*mut u16, u32) -> u32) -> Option<PathBuf> {
    let mut buf = [0u16; 512];
    let size = unsafe { get(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (size > 0 && size < buf.len()).then(|| PathBuf::from(OsString::from_wide(&buf[..size])))
}

fn own(raw: HANDLE) -> Result<OwnedHandle, &'static str> {
    if raw == INVALID_HANDLE_VALUE || raw.is_null() {
        Err("windows_handle_unavailable")
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
}

fn verify(
    path: &Path,
    check: &impl Fn() -> Result<(), &'static str>,
) -> Result<&'static str, &'static str> {
    let path = path.canonicalize().map_err(|_| "candidate_unavailable")?;
    let system = directory(GetSystemDirectoryW).ok_or("system_directory_unavailable")?;
    let windows = directory(GetWindowsDirectoryW).ok_or("system_directory_unavailable")?;
    let drive = windows
        .ancestors()
        .last()
        .ok_or("system_directory_unavailable")?;
    let root = drive
        .join("Program Files")
        .canonicalize()
        .map_err(|_| "installation_directory_unverified")?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| "outside_trusted_install_roots")?;
    if !crate::platform::windows::local(&path) || !node_install_location(relative) {
        return Err("outside_trusted_install_roots");
    }
    // Pin ancestors from the drive down. No handle shares DELETE access, so
    // an ancestor already checked cannot be renamed out from under a later
    // open. Directory handles do not lock the *entries*; the DACL must deny
    // untrusted principals both replacement and writes to the DLL-search dir.
    let ancestors: Vec<_> = path
        .parent()
        .ok_or("installation_directory_unverified")?
        .ancestors()
        .take(13)
        .collect();
    if ancestors.len() > 12 {
        return Err("installation_directory_unverified");
    }
    let mut install = Vec::new();
    for ancestor in ancestors.into_iter().rev() {
        check()?;
        let file = open_locked(ancestor, READ_CONTROL | FILE_READ_ATTRIBUTES, true)?;
        if ancestor.parent().is_none() && !ntfs(&file) {
            return Err("installation_filesystem_unverified");
        }
        if !protected(&file, ancestor.parent().is_none()) {
            return Err("installation_acl_unverified");
        }
        install.push(file);
    }
    // The child working directory and the system DLL directory must also be
    // regular protected directories. Keep these handles open through the launch.
    for dir in [&windows, &system] {
        let handle = open_locked(dir, READ_CONTROL | FILE_READ_ATTRIBUTES, true)?;
        if !protected(&handle, false) {
            return Err("system_directory_acl_unverified");
        }
        install.push(handle);
    }
    let bin = open_locked(&path, READ_CONTROL | FILE_GENERIC_READ, false)?;
    if !protected(&bin, false) {
        return Err("installation_acl_unverified");
    }
    let mut image = File::from(bin);
    let digest = hash(&mut image, check)?;
    if digest != NODE_X64_V24_22_3 {
        return Err("release_digest_not_allowlisted");
    }
    check()?;
    let output = launch(&path, &system, &windows, check)?;
    if output
        .strip_suffix(b"\r\n")
        .or_else(|| output.strip_suffix(b"\n"))
        == Some(b"v24.22.3")
    {
        Ok("24.22.3")
    } else {
        Err("unrecognized_version_output")
    }
}

fn node_install_location(relative: &Path) -> bool {
    let mut parts = relative.iter();
    parts
        .next()
        .is_some_and(|x| x.eq_ignore_ascii_case("nodejs"))
        && parts
            .next()
            .is_some_and(|x| x.eq_ignore_ascii_case("node.exe"))
        && parts.next().is_none()
}

fn hash(
    file: &mut File,
    check: &impl Fn() -> Result<(), &'static str>,
) -> Result<[u8; 32], &'static str> {
    let len = file.metadata().map_err(|_| "candidate_unavailable")?.len();
    if len == 0 || len > MAX_IMAGE {
        return Err("image_length_unverified");
    }
    let mut hasher = Sha256::new();
    let mut chunk = vec![0u8; 128 * 1024];
    let mut total = 0u64;
    loop {
        check()?;
        let count = file.read(&mut chunk).map_err(|_| "image_read_failed")?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > len {
            return Err("image_length_unverified");
        }
        hasher.update(&chunk[..count]);
    }
    if total != len {
        return Err("image_length_unverified");
    }
    Ok(hasher.finalize().into())
}

fn open_locked(path: &Path, rights: u32, directory: bool) -> Result<OwnedHandle, &'static str> {
    let flags = FILE_FLAG_OPEN_REPARSE_POINT
        | if directory {
            FILE_FLAG_BACKUP_SEMANTICS
        } else {
            0
        };
    let share = if directory {
        FILE_SHARE_READ | FILE_SHARE_WRITE
    } else {
        FILE_SHARE_READ
    };
    let handle = own(unsafe {
        CreateFileW(
            wide(path).as_ptr(),
            rights,
            share,
            null(),
            OPEN_EXISTING,
            flags,
            null_mut(),
        )
    })?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) } == 0
        || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        return Err("installation_path_reparse_or_inaccessible");
    }
    Ok(handle)
}

struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

fn ntfs(handle: &OwnedHandle) -> bool {
    let mut fs = [0u16; 16];
    // Verify the filesystem whose share modes and ACL rules the launch relies on.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            handle.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    };
    ok != 0 && fs[..5] == [b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0]
}

fn protected(handle: &OwnedHandle, drive_root: bool) -> bool {
    let mut raw = Local(null_mut());
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut installer = Local(null_mut());
    // NT SERVICE\TrustedInstaller is a Windows system principal, not the invoking user.
    let trusted_installer = wide("S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464");
    if unsafe { ConvertStringSidToSidW(trusted_installer.as_ptr(), &mut installer.0) } == 0 {
        return false;
    }
    let trusted = |sid| unsafe {
        IsValidSid(sid) != 0
            && (IsWellKnownSid(sid, WinLocalSystemSid) != 0
                || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
                || EqualSid(sid, installer.0) != 0)
    };
    if unsafe {
        GetSecurityInfo(
            handle.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut raw.0,
        )
    } != 0
        || dacl.is_null()
        || owner.is_null()
        || !trusted(owner)
    {
        return false;
    }
    // A drive may grant users creation of unrelated objects. It must not
    // grant replacement of existing children or changes to the security descriptor.
    let mutation = DELETE
        | WRITE_DAC
        | WRITE_OWNER
        | GENERIC_WRITE
        | GENERIC_ALL
        | FILE_DELETE_CHILD
        | if drive_root {
            0
        } else {
            FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_WRITE_EA | FILE_WRITE_ATTRIBUTES
        };
    let mut info: ACL_SIZE_INFORMATION = unsafe { zeroed() };
    if unsafe {
        GetAclInformation(
            dacl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
        || info.AceCount > 128
    {
        return false;
    }
    for i in 0..info.AceCount {
        let mut entry: *mut c_void = null_mut();
        if unsafe { GetAce(dacl, i, &mut entry) } == 0 || entry.is_null() {
            return false;
        }
        let header = unsafe { &*entry.cast::<ACE_HEADER>() };
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        match header.AceType {
            0 => {
                // ACCESS_ALLOWED_ACE_TYPE
                if usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>() + 4 {
                    return false;
                }
                let ace = entry.cast::<ACCESS_ALLOWED_ACE>();
                let sid = unsafe { &raw mut (*ace).SidStart }.cast::<c_void>();
                if unsafe { (*ace).Mask } & mutation != 0 && !trusted(sid) {
                    return false;
                }
            }
            // Denials cannot add mutation rights. We do not assert that a
            // trusted principal can access the file; the real launch proves that.
            1 => (), // ACCESS_DENIED_ACE_TYPE
            _ => return false,
        }
    }
    true
}

struct Attributes {
    storage: Vec<usize>,
    ptr: LPPROC_THREAD_ATTRIBUTE_LIST,
}
impl Attributes {
    fn new() -> Result<Self, &'static str> {
        let mut size = 0;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 3, 0, &mut size);
        }
        if size == 0 || size > 32768 {
            return Err("launch_attributes_unavailable");
        }
        let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
        let ptr = storage.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(ptr, 3, 0, &mut size) } == 0 {
            return Err("launch_attributes_unavailable");
        }
        Ok(Self { storage, ptr })
    }
    fn apply<T>(&self, key: u32, handles: &[T]) -> Result<(), &'static str> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.ptr,
                0,
                key as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err("launch_attributes_unavailable");
        }
        Ok(())
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.ptr);
        }
        let _ = self.storage.len();
    }
}

fn launch(
    path: &Path,
    system: &Path,
    windows: &Path,
    check: &impl Fn() -> Result<(), &'static str>,
) -> Result<Vec<u8>, &'static str> {
    let job = own(unsafe { CreateJobObjectW(null(), null()) })?;
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags =
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err("process_containment_unavailable");
    }
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let (mut reader, mut writer) = (null_mut(), null_mut());
    if unsafe { CreatePipe(&mut reader, &mut writer, &security, MAX_OUTPUT as u32 * 2) } == 0 {
        return Err("output_unavailable");
    }
    let (reader, writer) = (own(reader)?, own(writer)?);
    let sink = own(unsafe {
        CreateFileW(
            wide(r"\\.\NUL").as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &security,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    })?;
    let handles = [writer.as_raw_handle(), sink.as_raw_handle()];
    let jobs = [job.as_raw_handle()];
    let attributes = Attributes::new()?;
    attributes.apply(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &handles)?;
    attributes.apply(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;
    // WinBase.h: reject remote and low-integrity images and prefer System32.
    let image_policy = [(1u64 << 52) | (1u64 << 56) | (1u64 << 60)];
    attributes.apply(PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY, &image_policy)?;
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[1];
    startup.StartupInfo.hStdOutput = handles[0];
    startup.StartupInfo.hStdError = handles[1];
    startup.lpAttributeList = attributes.ptr;
    let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
    let command = wide("node.exe --version");
    let mut command = command;
    let mut root = OsString::from("SystemRoot=");
    root.push(windows);
    let mut env = wide(root);
    env.push(0);
    check()?;
    // The application path, DLL search working directory and environment do not
    // use the inspected project. JOB_LIST attaches before any instruction runs.
    if unsafe {
        CreateProcessW(
            wide(path).as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            env.as_ptr().cast(),
            wide(system).as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err("launch_denied_or_failed");
    }
    let process = own(info.hProcess)?;
    let _thread = own(info.hThread)?;
    drop(writer);
    let mut output = Vec::new();
    let mut buf = [0; 1024];
    loop {
        check()?;
        let mut available = 0;
        let pipe = unsafe {
            PeekNamedPipe(
                reader.as_raw_handle(),
                null_mut(),
                0,
                null_mut(),
                &mut available,
                null_mut(),
            )
        };
        if pipe == 0 && unsafe { GetLastError() } != ERROR_BROKEN_PIPE {
            return Err("output_unavailable");
        }
        if available > 0 {
            let cap = (available as usize).min(buf.len());
            let mut count = 0;
            if unsafe {
                ReadFile(
                    reader.as_raw_handle(),
                    buf.as_mut_ptr(),
                    cap as u32,
                    &mut count,
                    null_mut(),
                )
            } == 0
            {
                return Err("output_unavailable");
            }
            output.extend_from_slice(&buf[..count as usize]);
            if output.len() > MAX_OUTPUT {
                return Err("output_limit_exceeded");
            }
        } else {
            match unsafe { WaitForSingleObject(process.as_raw_handle(), 5) } {
                WAIT_OBJECT_0 if pipe == 0 => break,
                WAIT_OBJECT_0 | WAIT_TIMEOUT => (),
                _ => return Err("probe_failed"),
            }
        }
    }
    let mut status = u32::MAX;
    if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut status) } == 0 || status != 0 {
        return Err("probe_nonzero_exit");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_share_mode_pins_file_and_ancestor_names() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("directory");
        std::fs::create_dir(&dir).unwrap();
        let image = dir.join("test.txt");
        std::fs::write(&image, b"fixture").unwrap();
        let dir_lock = open_locked(&dir, READ_CONTROL | FILE_READ_ATTRIBUTES, true).unwrap();
        if std::fs::rename(&dir, tmp.path().join("other-directory")).is_ok() {
            // Some hosted volumes do not enforce share-mode rename locks; the rest of this
            // test would then describe the volume instead of the probe's open_locked.
            eprintln!("skipping: this volume does not enforce share-mode rename locks");
            return;
        }
        let file_lock = open_locked(&image, READ_CONTROL | FILE_GENERIC_READ, false).unwrap();
        assert!(std::fs::rename(&image, dir.join("other.txt")).is_err());
        assert!(std::fs::write(&image, b"another value").is_err());
        drop(file_lock);
        std::fs::rename(&image, dir.join("other.txt")).unwrap();
        drop(dir_lock);
        std::fs::rename(&dir, tmp.path().join("other-directory")).unwrap();
    }

    #[test]
    fn node_layout_is_exact() {
        assert!(node_install_location(Path::new(r"nodejs\NoDe.ExE")));
        assert!(!node_install_location(Path::new(r"nodejs\assets\node.exe")));
        assert!(!node_install_location(Path::new(r"nodejs\node.cmd")));
        assert!(!node_install_location(Path::new(r"elsewhere\node.exe")));
    }
}
