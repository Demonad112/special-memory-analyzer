//! Free space and volume root of a path (the destination, or a recovery folder).

use std::path::Path;

pub(crate) struct VolumeInfo {
    pub free_bytes: u64,
    /// `C:\` or `\\server\share`.
    pub root: String,
}

/// For the nearest part of `path` that exists. `None` off Windows or when Windows can't tell.
#[cfg(windows)]
pub(crate) fn volume_info(path: &Path) -> Option<VolumeInfo> {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Prefix};

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            directory_name: *const u16,
            free_bytes_available: *mut u64,
            total_number_of_bytes: *mut u64,
            total_number_of_free_bytes: *mut u64,
        ) -> i32;
    }

    let probe = path.ancestors().find(|ancestor| ancestor.exists())?;
    let wide: Vec<u16> = probe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let (mut free, mut total, mut total_free) = (0_u64, 0_u64, 0_u64);
    // SAFETY: `wide` is NUL-terminated and the out pointers are valid for the call.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
    if ok == 0 {
        return None;
    }
    let root = match std::path::absolute(probe).ok()?.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => format!("{}:\\", letter as char),
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
                "\\\\{}\\{}",
                server.to_string_lossy(),
                share.to_string_lossy()
            ),
            _ => return None,
        },
        _ => return None,
    };
    Some(VolumeInfo {
        free_bytes: free,
        root,
    })
}

#[cfg(not(windows))]
pub(crate) fn volume_info(_path: &Path) -> Option<VolumeInfo> {
    None
}
