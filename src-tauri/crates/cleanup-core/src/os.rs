use serde::Serialize;
use std::fs;
use std::path::Path;

/// A local drive for the drive tiles.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    /// "C:\".
    pub path: String,
    /// The volume label, or "Local Disk" / "USB Drive" when it has none.
    pub label: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
}

/// Fixed and removable drives with media. Network and CD drives are left out: they can take
/// seconds to answer.
#[cfg(windows)]
pub fn drives() -> Vec<Drive> {
    use win::*;
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;

    let mut buffer = [0u16; 512];
    // SAFETY: the buffer length is passed and the call writes at most that many units.
    let len = unsafe { GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr()) };
    let len = (len as usize).min(buffer.len());
    buffer[..len]
        .split(|&unit| unit == 0)
        .filter(|root| !root.is_empty())
        .filter_map(|root| {
            let wide: Vec<u16> = root.iter().copied().chain([0]).collect();
            // SAFETY: `wide` is a null-terminated root path such as "C:\".
            let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
            if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
                return None;
            }
            let (mut total, mut free, mut caller_free) = (0u64, 0u64, 0u64);
            // SAFETY: valid root path and out pointers; fails (0) for a drive with no media.
            let ok = unsafe {
                GetDiskFreeSpaceExW(wide.as_ptr(), &mut caller_free, &mut total, &mut free)
            };
            if ok == 0 || total == 0 {
                return None;
            }
            let mut name = [0u16; 261];
            // SAFETY: the name buffer length is passed; the optional outputs are null.
            let named = unsafe {
                GetVolumeInformationW(
                    wide.as_ptr(),
                    name.as_mut_ptr(),
                    name.len() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                )
            };
            let end = name
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(name.len());
            let label = if named != 0 {
                String::from_utf16_lossy(&name[..end])
            } else {
                String::new()
            };
            let removable = kind == DRIVE_REMOVABLE;
            Some(Drive {
                path: String::from_utf16_lossy(root),
                label: if label.is_empty() {
                    (if removable { "USB Drive" } else { "Local Disk" }).into()
                } else {
                    label
                },
                total,
                free,
                removable,
            })
        })
        .collect()
}

#[cfg(not(windows))]
pub fn drives() -> Vec<Drive> {
    Vec::new()
}

/// Moves a file or folder to the Recycle Bin. If it is too big for the bin, Windows asks before
/// deleting it for good.
#[cfg(windows)]
pub fn recycle(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use win::*;
    const FO_DELETE: u32 = 3;
    const FOF_SILENT: u16 = 0x4;
    const FOF_NOCONFIRMATION: u16 = 0x10;
    const FOF_ALLOWUNDO: u16 = 0x40;
    const FOF_NOERRORUI: u16 = 0x400;
    const FOF_WANTNUKEWARNING: u16 = 0x4000;

    // The source list ends with two nulls.
    let from: Vec<u16> = path.as_os_str().encode_wide().chain([0, 0]).collect();
    let mut operation = ShFileOpStructW {
        hwnd: std::ptr::null_mut(),
        func: FO_DELETE,
        from: from.as_ptr(),
        to: std::ptr::null(),
        flags: FOF_ALLOWUNDO
            | FOF_NOCONFIRMATION
            | FOF_SILENT
            | FOF_NOERRORUI
            | FOF_WANTNUKEWARNING,
        any_aborted: 0,
        name_mappings: std::ptr::null_mut(),
        progress_title: std::ptr::null(),
    };
    // SAFETY: `operation` is fully initialised and `from` outlives the call.
    let code = unsafe { SHFileOperationW(&mut operation) };
    if operation.any_aborted != 0 {
        return Err("Cancelled. Nothing more was deleted.".into());
    }
    if code != 0 {
        return Err(format!(
            "Windows could not move {} to the Recycle Bin (code {code:#x}). It may be open in another program.",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn recycle(_path: &Path) -> Result<(), String> {
    Err("The Recycle Bin is only on Windows.".into())
}

/// Deletes a file or folder for good.
pub fn delete_permanently(path: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;
    // A junction or symlink to a folder is removed itself, never what it points to.
    let result = if metadata.is_dir() {
        fs::remove_dir_all(path)
    } else if metadata.file_type().is_symlink() && fs::remove_file(path).is_err() {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    };
    result.map_err(|err| format!("Could not delete {}: {err}", path.display()))
}

/// What is in the current user's Recycle Bin on every drive. Other users' bin folders are not
/// read: a restore must only move back what this account recycled.
#[cfg(windows)]
pub fn recycle_bin_items() -> Result<Vec<crate::BinItem>, String> {
    let sid = current_sid()?;
    Ok(drives()
        .iter()
        .map(|drive| std::path::PathBuf::from(format!("{}$Recycle.Bin", drive.path)).join(&sid))
        .flat_map(|dir| crate::restore::bin_items_in(&dir))
        .collect())
}

/// The SID of the user this process runs as, from `whoami /user`.
#[cfg(windows)]
fn current_sid() -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = std::process::Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|err| format!("Could not find your Recycle Bin: {err}"))?;
    sid_from_whoami(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| "Could not find your Recycle Bin: no user SID.".to_string())
}

/// The `S-1-…` field of `"DOMAIN\user","S-1-5-21-…"`.
#[cfg_attr(not(windows), allow(dead_code))]
fn sid_from_whoami(output: &str) -> Option<String> {
    output
        .split(',')
        .map(|field| field.trim().trim_matches('"'))
        .find(|field| {
            field.starts_with("S-1-")
                && field.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        .map(str::to_string)
}

#[cfg(not(windows))]
pub fn recycle_bin_items() -> Result<Vec<crate::BinItem>, String> {
    Err("The Recycle Bin is only on Windows.".into())
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    #[repr(C)]
    pub struct ShFileOpStructW {
        pub hwnd: *mut c_void,
        pub func: u32,
        pub from: *const u16,
        pub to: *const u16,
        pub flags: u16,
        pub any_aborted: i32,
        pub name_mappings: *mut c_void,
        pub progress_title: *const u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetLogicalDriveStringsW(len: u32, buffer: *mut u16) -> u32;
        pub fn GetDriveTypeW(root: *const u16) -> u32;
        pub fn GetDiskFreeSpaceExW(
            root: *const u16,
            caller_free: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> i32;
        pub fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_component: *mut u32,
            flags: *mut u32,
            fs_name: *mut u16,
            fs_name_len: u32,
        ) -> i32;
    }

    #[repr(C)]
    pub struct ShQueryRbInfo {
        pub size_of: u32,
        pub size: i64,
        pub items: i64,
    }

    #[link(name = "shell32")]
    extern "system" {
        pub fn SHFileOperationW(operation: *mut ShFileOpStructW) -> i32;
        pub fn SHQueryRecycleBinW(root: *const u16, info: *mut ShQueryRbInfo) -> i32;
        pub fn SHEmptyRecycleBinW(hwnd: *mut c_void, root: *const u16, flags: u32) -> i32;
        pub fn IsUserAnAdmin() -> i32;
    }
}

/// Bytes and items in the Recycle Bin on every drive.
#[cfg(windows)]
pub fn recycle_bin_size() -> crate::quick::Cleaned {
    let mut info = win::ShQueryRbInfo {
        size_of: std::mem::size_of::<win::ShQueryRbInfo>() as u32,
        size: 0,
        items: 0,
    };
    // SAFETY: a null root means every drive; `info` is initialised with its size.
    let result = unsafe { win::SHQueryRecycleBinW(std::ptr::null(), &mut info) };
    if result != 0 {
        return crate::quick::Cleaned::default();
    }
    crate::quick::Cleaned {
        freed: u64::try_from(info.size).unwrap_or(0),
        removed: u64::try_from(info.items).unwrap_or(0),
        skipped: 0,
    }
}

#[cfg(not(windows))]
pub fn recycle_bin_size() -> crate::quick::Cleaned {
    crate::quick::Cleaned::default()
}

/// Empties the Recycle Bin on every drive, without Windows' own prompt or sound.
#[cfg(windows)]
pub fn empty_recycle_bin() -> Result<(), String> {
    const SHERB_NOCONFIRMATION: u32 = 0x1;
    const SHERB_NOPROGRESSUI: u32 = 0x2;
    const SHERB_NOSOUND: u32 = 0x4;
    // Returned when the bin is already empty.
    const E_UNEXPECTED: i32 = 0x8000_FFFF_u32 as i32;
    // SAFETY: null window and root (every drive); flags only.
    let result = unsafe {
        win::SHEmptyRecycleBinW(
            std::ptr::null_mut(),
            std::ptr::null(),
            SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
        )
    };
    if result == 0 || result == E_UNEXPECTED {
        Ok(())
    } else {
        Err(format!(
            "Windows could not empty the Recycle Bin (code {result:#x})."
        ))
    }
}

#[cfg(not(windows))]
pub fn empty_recycle_bin() -> Result<(), String> {
    Err("The Recycle Bin is only on Windows.".into())
}

/// True when Deep Server Analyzer runs as administrator.
#[cfg(windows)]
pub fn is_elevated() -> bool {
    // SAFETY: no arguments.
    unsafe { win::IsUserAnAdmin() != 0 }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_permanently_removes_files_and_folders() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("old");
        fs::create_dir_all(folder.join("inner")).unwrap();
        fs::write(folder.join("inner/a.bin"), b"x").unwrap();
        let file = temp.path().join("b.bin");
        fs::write(&file, b"y").unwrap();

        delete_permanently(&folder).unwrap();
        delete_permanently(&file).unwrap();
        assert!(!folder.exists() && !file.exists());
        assert!(delete_permanently(&file).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn recycle_moves_a_file_out_and_drives_include_the_system_drive() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("recycle-me.txt");
        fs::write(&file, b"z").unwrap();
        recycle(&file).unwrap();
        assert!(!file.exists());

        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let drives = drives();
        let drive = drives
            .iter()
            .find(|drive| drive.path.eq_ignore_ascii_case(&format!("{system}\\")))
            .expect("system drive listed");
        assert!(drive.total >= drive.free && drive.total > 0);
    }
}

#[cfg(test)]
mod sid_tests {
    use super::sid_from_whoami;

    #[test]
    fn the_sid_is_read_from_whoami_csv_and_nothing_else_is_accepted() {
        assert_eq!(
            sid_from_whoami("\"SERVER\\Tech User\",\"S-1-5-21-111-222-333-1001\"\r\n").as_deref(),
            Some("S-1-5-21-111-222-333-1001")
        );
        assert_eq!(sid_from_whoami("ERROR: Access denied"), None);
        assert_eq!(sid_from_whoami("\"a\",\"..\\S-1-5\""), None);
        assert_eq!(sid_from_whoami(""), None);
    }
}
