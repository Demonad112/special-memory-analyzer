//! The privileged half of keeping metadata, in the spirit of Robocopy's `/COPY:SOU` and `/B`:
//! permissions (DACL), owner and group, audit rules (SACL), and alternate data streams
//! (`Zone.Identifier`, ...). Windows-only; elsewhere these are no-ops.
//!
//! What can be kept depends on who runs the app: an administrator holds the backup, restore and
//! security privileges and can keep everything; a standard user keeps permissions and streams
//! where the folder allows it, and gets a warning for the rest.

use crate::meta::PreserveOptions;
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Which of the privileges that matter for copying security were switched on for this process.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Privileges {
    /// `SeBackupPrivilege`: read any file whatever its ACL says.
    pub backup: bool,
    /// `SeRestorePrivilege`: write any owner and permissions.
    pub restore: bool,
    /// `SeSecurityPrivilege`: read and write audit rules.
    pub security: bool,
    /// `SeTakeOwnershipPrivilege`.
    pub take_ownership: bool,
}

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Lets copies use the process token's backup, restore and security privileges. The app calls
/// this when a record run starts; until then (scans, unit tests) nothing is switched on, so
/// access checks behave as they do for any program.
pub fn enable_privileges() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// The privileges in use: none until [`enable_privileges`], then every one the token holds.
pub fn privileges() -> Privileges {
    if ENABLED.load(Ordering::Relaxed) {
        imp::privileges()
    } else {
        Privileges::default()
    }
}

/// Opens a source file for reading; with the backup privilege this reads files whose ACL denies
/// the caller (Robocopy `/B`).
pub fn open_source(path: &Path) -> io::Result<File> {
    imp::open_source(path)
}

/// Copies the alternate data streams of `source` onto `destination`. Fails with a sentence when
/// the source has some and the destination drive can't hold them.
pub fn copy_streams(source: &Path, destination: &Path) -> io::Result<()> {
    imp::copy_streams(source, destination)
}

/// Copies permissions, owner and audit rules as far as `options` ask. One sentence per part that
/// could not be kept.
pub fn copy_security(source: &Path, destination: &Path, options: &PreserveOptions) -> Vec<String> {
    imp::copy_security(source, destination, options)
}

#[cfg(windows)]
mod imp {
    use super::{PreserveOptions, Privileges};
    use crate::meta::extended_path;
    use std::ffi::OsString;
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_NOT_ALL_ASSIGNED, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, SetKernelObjectSecurity, LUID_AND_ATTRIBUTES,
        SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindFirstStreamW, FindNextStreamW, GetVolumeInformationW, WIN32_FIND_STREAM_DATA,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_PERSISTENT_ACLS: u32 = 0x8;
    const FILE_NAMED_STREAMS: u32 = 0x0004_0000;

    const WRITE_DAC: u32 = 0x0004_0000;
    const WRITE_OWNER: u32 = 0x0008_0000;
    const ACCESS_SYSTEM_SECURITY: u32 = 0x0100_0000;
    const OWNER: u32 = 0x1;
    const GROUP: u32 = 0x2;
    const DACL: u32 = 0x4;
    const SACL: u32 = 0x8;

    fn wide(path: &Path) -> Vec<u16> {
        let mut text: Vec<u16> = extended_path(path).as_os_str().encode_wide().collect();
        text.push(0);
        text
    }

    fn wide_str(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    pub fn privileges() -> Privileges {
        static GRANTED: OnceLock<Privileges> = OnceLock::new();
        *GRANTED.get_or_init(|| {
            let mut token: HANDLE = std::ptr::null_mut();
            // SAFETY: `token` is a valid out pointer; the handle is closed below.
            let opened = unsafe {
                OpenProcessToken(
                    GetCurrentProcess(),
                    TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                    &mut token,
                )
            };
            if opened == 0 {
                return Privileges::default();
            }
            let enable = |name: &str| -> bool {
                let name = wide_str(name);
                // SAFETY: zeroed LUID/TOKEN_PRIVILEGES are valid plain-data values.
                let mut luid = unsafe { std::mem::zeroed() };
                // SAFETY: `name` is NUL-terminated; `luid` is a valid out pointer.
                if unsafe { LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) } == 0
                {
                    return false;
                }
                let state = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [LUID_AND_ATTRIBUTES {
                        Luid: luid,
                        Attributes: SE_PRIVILEGE_ENABLED,
                    }],
                };
                // SAFETY: `state` outlives the call; the previous-state outputs are not wanted.
                let adjusted = unsafe {
                    AdjustTokenPrivileges(
                        token,
                        0,
                        &state,
                        0,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                // Success is reported even when the token doesn't hold the privilege.
                // SAFETY: reads the calling thread's last error.
                adjusted != 0 && unsafe { GetLastError() } != ERROR_NOT_ALL_ASSIGNED
            };
            let granted = Privileges {
                backup: enable("SeBackupPrivilege"),
                restore: enable("SeRestorePrivilege"),
                security: enable("SeSecurityPrivilege"),
                take_ownership: enable("SeTakeOwnershipPrivilege"),
            };
            // SAFETY: `token` came from OpenProcessToken.
            unsafe { CloseHandle(token) };
            granted
        })
    }

    pub fn open_source(path: &Path) -> io::Result<File> {
        // A plain open first, so a file another program holds open is still reported as in use
        // (backup rights would read it mid-write). Only an access-denied open is retried with
        // backup semantics, like Robocopy /ZB.
        match File::open(path) {
            Err(error) if error.raw_os_error() == Some(5) && super::privileges().backup => {
                OpenOptions::new()
                    .read(true)
                    .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                    .open(path)
            }
            other => other,
        }
    }

    /// `C:\` or `\\server\share\` of an absolute path.
    fn volume_root(path: &Path) -> Option<Vec<u16>> {
        let text = path.as_os_str().to_string_lossy().into_owned();
        let text = text
            .strip_prefix(r"\\?\UNC\")
            .map(|rest| format!(r"\\{rest}"))
            .or_else(|| text.strip_prefix(r"\\?\").map(str::to_owned))
            .unwrap_or(text);
        let root = if let Some(rest) = text.strip_prefix(r"\\") {
            let mut parts = rest.split('\\');
            format!(r"\\{}\{}\", parts.next()?, parts.next()?)
        } else if text.as_bytes().get(1) == Some(&b':') {
            format!(r"{}\", &text[..2])
        } else {
            return None;
        };
        Some(wide_str(&root))
    }

    /// The file system flags of the drive holding `path`, when they can be read.
    fn volume_flags(path: &Path) -> Option<u32> {
        let root = volume_root(path)?;
        let mut flags = 0u32;
        // SAFETY: `root` is NUL-terminated; unwanted outputs are null with size 0.
        let ok = unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut flags,
                std::ptr::null_mut(),
                0,
            )
        };
        (ok != 0).then_some(flags)
    }

    /// Names of the non-default streams of `path`, as `:name` (without the `:$DATA` type).
    fn stream_names(path: &Path) -> io::Result<Vec<String>> {
        let name = wide(path);
        // SAFETY: zeroed WIN32_FIND_STREAM_DATA is valid plain data.
        let mut data: WIN32_FIND_STREAM_DATA = unsafe { std::mem::zeroed() };
        // SAFETY: `name` is NUL-terminated; `data` is the buffer for level 0 (standard).
        let handle = unsafe {
            FindFirstStreamW(
                name.as_ptr(),
                0,
                (&mut data as *mut WIN32_FIND_STREAM_DATA).cast(),
                0,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            let error = io::Error::last_os_error();
            // 38 = no more streams; 1 and 87 = a drive that has none (FAT, exFAT, some shares).
            return match error.raw_os_error() {
                Some(1 | 38 | 87) => Ok(Vec::new()),
                _ => Err(error),
            };
        }
        let mut names = Vec::new();
        loop {
            let length = data.cStreamName.iter().position(|&c| c == 0).unwrap_or(0);
            let full = OsString::from_wide(&data.cStreamName[..length])
                .to_string_lossy()
                .into_owned();
            // ":name:$DATA"; the unnamed data stream is "::$DATA".
            if let Some(stream) = full
                .strip_prefix(':')
                .and_then(|rest| rest.strip_suffix(":$DATA"))
                .filter(|stream| !stream.is_empty())
            {
                names.push(stream.to_owned());
            }
            // SAFETY: `handle` is a live find handle and `data` the same buffer as above.
            if unsafe { FindNextStreamW(handle, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast()) }
                == 0
            {
                break;
            }
        }
        // SAFETY: `handle` came from FindFirstStreamW.
        unsafe { FindClose(handle) };
        Ok(names)
    }

    pub fn copy_streams(source: &Path, destination: &Path) -> io::Result<()> {
        let names = stream_names(source)?;
        if names.is_empty() {
            return Ok(());
        }
        if volume_flags(destination).is_some_and(|flags| flags & FILE_NAMED_STREAMS == 0) {
            return Err(io::Error::other(
                "the destination drive cannot hold alternate data streams",
            ));
        }
        for name in names {
            let from = extended_path(source);
            let to = extended_path(destination);
            let mut from_stream = from.into_os_string();
            from_stream.push(format!(":{name}"));
            let mut to_stream = to.into_os_string();
            to_stream.push(format!(":{name}"));
            let mut input = OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(&from_stream)?;
            let mut output = File::create(&to_stream)?;
            io::copy(&mut input, &mut output)?;
        }
        Ok(())
    }

    pub fn copy_security(
        source: &Path,
        destination: &Path,
        options: &PreserveOptions,
    ) -> Vec<String> {
        let mut warnings = Vec::new();
        let held = super::privileges();
        let mut info = 0u32;
        if options.acl {
            info |= DACL;
        }
        if options.owner {
            info |= OWNER | GROUP;
        }
        if options.audit {
            if held.security {
                info |= SACL;
            } else {
                warnings.push(
                    "Audit rules were not kept: it needs the Administrator (security) privilege."
                        .to_owned(),
                );
            }
        }
        if info == 0 {
            return warnings;
        }
        if volume_flags(destination).is_some_and(|flags| flags & FILE_PERSISTENT_ACLS == 0) {
            warnings.push(
                "Permissions and owner were not kept: the destination drive has none (FAT or exFAT)."
                    .to_owned(),
            );
            return warnings;
        }

        let from = wide(source);
        let mut owner = std::ptr::null_mut();
        let mut group = std::ptr::null_mut();
        let mut dacl = std::ptr::null_mut();
        let mut sacl = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: `from` is NUL-terminated; every output pointer is valid. The returned
        // descriptor owns the memory `owner`..`sacl` point into and is freed below.
        let code = unsafe {
            GetNamedSecurityInfoW(
                from.as_ptr(),
                SE_FILE_OBJECT,
                info,
                &mut owner,
                &mut group,
                &mut dacl,
                &mut sacl,
                &mut descriptor,
            )
        };
        if code != 0 {
            warnings.push(format!(
                "Permissions and owner were not kept: the source's could not be read ({}).",
                io::Error::from_raw_os_error(code as i32)
            ));
            return warnings;
        }

        // The source descriptor is applied as it is (its "inheritance blocked" bits included)
        // through the file handle. That, unlike SetNamedSecurityInfoW, does not re-propagate
        // inherited entries to everything below a folder. Permissions and audit rules go first and
        // the owner separately, so an owner that can't be set doesn't lose the permissions.
        let _ = (owner, group, dacl, sacl);
        let mut steps: Vec<(u32, u32, &str)> = Vec::new();
        if info & (DACL | SACL) != 0 {
            let access = WRITE_DAC
                | if info & SACL != 0 {
                    ACCESS_SYSTEM_SECURITY
                } else {
                    0
                };
            steps.push((info & (DACL | SACL), access, "Permissions or audit rules"));
        }
        if info & (OWNER | GROUP) != 0 {
            let note = if held.restore || held.take_ownership {
                "Owner"
            } else {
                "Owner (run as Administrator to keep it)"
            };
            steps.push((info & (OWNER | GROUP), WRITE_OWNER, note));
        }
        for (part, access, what) in steps {
            let result = OpenOptions::new()
                .access_mode(access)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(extended_path(destination))
                .and_then(|file| {
                    // SAFETY: the handle is open for the duration of the call and `descriptor`
                    // is the live descriptor from above.
                    let ok =
                        unsafe { SetKernelObjectSecurity(file.as_raw_handle(), part, descriptor) };
                    if ok == 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            if let Err(error) = result {
                warnings.push(format!("{what} not kept: {error}"));
            }
        }
        // SAFETY: `descriptor` was allocated by GetNamedSecurityInfoW for LocalFree.
        unsafe { LocalFree(descriptor) };
        warnings
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{PreserveOptions, Privileges};
    use std::fs::File;
    use std::io;
    use std::path::Path;

    pub fn privileges() -> Privileges {
        Privileges::default()
    }

    pub fn open_source(path: &Path) -> io::Result<File> {
        File::open(path)
    }

    pub fn copy_streams(_source: &Path, _destination: &Path) -> io::Result<()> {
        Ok(())
    }

    pub fn copy_security(
        _source: &Path,
        _destination: &Path,
        _options: &PreserveOptions,
    ) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ds-secure-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn alternate_streams_are_copied() {
        let dir = temp("ads");
        let source = dir.join("s.txt");
        let target = dir.join("t.txt");
        std::fs::write(&source, b"main").unwrap();
        std::fs::write(&target, b"main").unwrap();
        let mut stream = source.clone().into_os_string();
        stream.push(":Zone.Identifier");
        std::fs::write(&stream, b"[ZoneTransfer]\r\nZoneId=3\r\n").unwrap();

        copy_streams(&source, &target).unwrap();

        let mut copied = target.clone().into_os_string();
        copied.push(":Zone.Identifier");
        assert_eq!(
            std::fs::read(&copied).unwrap(),
            b"[ZoneTransfer]\r\nZoneId=3\r\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_without_streams_is_fine() {
        let dir = temp("nostream");
        let source = dir.join("s.txt");
        let target = dir.join("t.txt");
        std::fs::write(&source, b"x").unwrap();
        std::fs::write(&target, b"x").unwrap();
        copy_streams(&source, &target).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The DACL and owner of `path` as SDDL text.
    fn sddl(path: &Path) -> String {
        use windows_sys::Win32::Security::Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
            SE_FILE_OBJECT,
        };
        let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated name and out pointers; freed below.
        let code = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                0x1 | 0x4,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(code, 0);
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: `descriptor` is live; `text` is freed below.
        let ok = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                1,
                0x1 | 0x4,
                &mut text,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(ok, 0);
        // SAFETY: `text` is NUL-terminated.
        let length = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
        let result = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
        // SAFETY: both were allocated by the calls above for LocalFree.
        unsafe {
            LocalFree(text.cast());
            LocalFree(descriptor);
        }
        result
    }

    #[test]
    fn a_custom_protected_acl_is_copied_exactly() {
        use std::process::Command;
        let dir = temp("acl");
        let source = dir.join("s.txt");
        let target = dir.join("t.txt");
        std::fs::write(&source, b"x").unwrap();
        std::fs::write(&target, b"x").unwrap();
        // Everyone read-only, inheritance cut: nothing like what the new file inherits.
        let status = Command::new("icacls")
            .arg(&source)
            .args([
                "/inheritance:r",
                "/grant:r",
                "*S-1-1-0:(R)",
                "*S-1-5-18:(F)",
            ])
            .arg("/grant:r")
            .arg(format!("{}:(F)", std::env::var("USERNAME").unwrap()))
            .output()
            .unwrap()
            .status;
        assert!(status.success());
        assert_ne!(sddl(&source), sddl(&target));

        let warnings = copy_security(&source, &target, &PreserveOptions::default());

        assert!(warnings.is_empty(), "{warnings:?}");
        // The "auto-inherited" marker (AI) only records that inheritance ran once; the entries,
        // owner and the blocked-inheritance flag (P) are what must match.
        assert_eq!(sddl(&source).replace("D:PAI", "D:P"), sddl(&target));
        assert!(sddl(&target).contains("D:P"), "inheritance block was lost");
        let _ = Command::new("icacls")
            .arg(&dir)
            .args(["/reset", "/t", "/q"])
            .output();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn audit_rules_without_the_privilege_are_a_warning() {
        let dir = temp("audit");
        let source = dir.join("s.txt");
        let target = dir.join("t.txt");
        std::fs::write(&source, b"x").unwrap();
        std::fs::write(&target, b"x").unwrap();
        let options = PreserveOptions {
            audit: true,
            ..PreserveOptions::default()
        };

        let warnings = copy_security(&source, &target, &options);

        if !privileges().security {
            assert!(
                warnings.iter().any(|w| w.contains("Audit rules")),
                "{warnings:?}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
