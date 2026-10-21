//! One folder level for the scanner: names, kinds, logical size, size on disk, modified time.
//!
//! On Windows a single `NtQueryDirectoryFile` call returns all of that (plus the file id) for a
//! whole batch of entries, so there is no per-file system call. The allocated size is what the disk
//! really spends: whole clusters, NTFS-compressed bytes, nothing for sparse holes. Elsewhere,
//! or when a file system refuses that call, a plain `read_dir` listing is used with an estimate of the size on disk.

use crate::scan::Kind;
use std::fs;
use std::io;
use std::path::Path;

/// One entry in a folder.
pub(crate) struct Entry {
    pub name: String,
    pub kind: Kind,
    /// Logical size (what Explorer shows as "Size"). 0 for folders and links.
    pub size: u64,
    /// Space the file takes on the disk. 0 for folders, links and online-only cloud files.
    pub disk: u64,
    pub modified_ms: Option<u64>,
    /// An online-only cloud placeholder (OneDrive and similar).
    pub cloud: bool,
    /// Volume-unique file id (0 when the file system has none). Two entries with the same id and
    /// size are hard links to one file.
    pub file_id: u64,
}

/// What a folder listing found. `error` is set when reading stopped early; `entries` then holds
/// what was read before that.
pub(crate) struct Listing {
    pub entries: Vec<Entry>,
    pub error: Option<io::Error>,
}

pub(crate) fn list_dir(path: &Path) -> io::Result<Listing> {
    #[cfg(windows)]
    {
        match nt::list_dir(path) {
            Err(nt::Failure::Unsupported) => {}
            Err(nt::Failure::Io(err)) => return Err(err),
            Ok(listing) => return Ok(listing),
        }
    }
    list_dir_std(path)
}

fn list_dir_std(path: &Path) -> io::Result<Listing> {
    let mut listing = Listing {
        entries: Vec::new(),
        error: None,
    };
    for entry in fs::read_dir(path)? {
        // On Windows the entry's type and metadata come from the directory listing itself
        // (FindNextFileW), so this costs no extra system call per file.
        let entry_info = entry.and_then(|entry| {
            let file_type = entry.file_type()?;
            Ok((entry, file_type))
        });
        let (entry, file_type) = match entry_info {
            Ok(info) => info,
            Err(err) => {
                listing.error.get_or_insert(err);
                continue;
            }
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_symlink() {
            listing.entries.push(plain(name, Kind::Link));
        } else if file_type.is_dir() {
            listing.entries.push(plain(name, Kind::Dir));
        } else {
            match entry.metadata() {
                Ok(metadata) => {
                    let cloud = std_is_cloud(&metadata);
                    let size = metadata.len();
                    listing.entries.push(Entry {
                        name,
                        kind: Kind::File,
                        size,
                        disk: if cloud { 0 } else { std_disk(&metadata) },
                        modified_ms: metadata
                            .modified()
                            .ok()
                            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX)),
                        cloud,
                        file_id: 0,
                    });
                }
                Err(err) => {
                    listing.error.get_or_insert(err);
                }
            }
        }
    }
    Ok(listing)
}

fn plain(name: String, kind: Kind) -> Entry {
    Entry {
        name,
        kind,
        size: 0,
        disk: 0,
        modified_ms: None,
        cloud: false,
        file_id: 0,
    }
}

#[cfg(unix)]
fn std_disk(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.blocks() * 512
}

/// Without the real allocation: whole 4 KiB clusters, the usual NTFS size.
#[cfg(not(unix))]
fn std_disk(metadata: &fs::Metadata) -> u64 {
    metadata.len().div_ceil(4096) * 4096
}

#[cfg(windows)]
fn std_is_cloud(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & nt::CLOUD_ATTRIBUTES != 0
}

#[cfg(not(windows))]
fn std_is_cloud(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
mod nt {
    use super::{plain, Entry, Listing};
    use crate::scan::Kind;
    use std::ffi::{c_void, OsString};
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;
    use std::ptr;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetCompressedFileSizeW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    const FILE_LIST_DIRECTORY: u32 = 0x1;
    const SYNCHRONIZE: u32 = 0x10_0000;
    /// `FILE_INFORMATION_CLASS::FileIdFullDirectoryInformation`.
    const FILE_ID_FULL_DIRECTORY_INFORMATION: i32 = 38;
    const STATUS_NO_MORE_FILES: i32 = 0x8000_0006_u32 as i32;
    const STATUS_NO_SUCH_FILE: i32 = 0xC000_000F_u32 as i32;
    const STATUS_NOT_IMPLEMENTED: i32 = 0xC000_0002_u32 as i32;
    const STATUS_INVALID_INFO_CLASS: i32 = 0xC000_0003_u32 as i32;
    const STATUS_NOT_SUPPORTED: i32 = 0xC000_00BB_u32 as i32;

    const ATTR_SPARSE: u32 = 0x200;
    const ATTR_DIRECTORY: u32 = 0x10;
    const ATTR_REPARSE_POINT: u32 = 0x400;
    const ATTR_COMPRESSED: u32 = 0x800;
    /// Offline, recall-on-open and recall-on-data-access: the data is in the cloud.
    pub const CLOUD_ATTRIBUTES: u32 = 0x1000 | 0x4_0000 | 0x40_0000;
    const TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const TAG_SYMLINK: u32 = 0xA000_000C;
    /// Windows Overlay Filter: a compressed file whose directory entry shows no allocation.
    const TAG_WOF: u32 = 0x8000_0017;

    /// Files up to this size may live in the MFT record and really take no space of their own.
    const SMALL_FILE: u64 = 4096;
    /// Offsets inside `FILE_ID_FULL_DIR_INFORMATION`.
    const HEADER: usize = 80;

    #[repr(C)]
    struct IoStatusBlock {
        status: isize,
        information: usize,
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryDirectoryFile(
            file: HANDLE,
            event: HANDLE,
            apc_routine: *mut c_void,
            apc_context: *mut c_void,
            io_status: *mut IoStatusBlock,
            information: *mut c_void,
            length: u32,
            class: i32,
            return_single_entry: u8,
            file_name: *mut c_void,
            restart_scan: u8,
        ) -> i32;
        fn RtlNtStatusToDosError(status: i32) -> u32;
    }

    pub(super) enum Failure {
        /// This file system doesn't answer the call; use the plain listing.
        Unsupported,
        Io(io::Error),
    }

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateFileW and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// The path as UTF-16 with a trailing NUL. Very long paths get the `\\?\` prefix.
    fn wide(path: &Path) -> Vec<u16> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        let verbatim: Vec<u16> = r"\\?\".encode_utf16().collect();
        if wide.len() >= 240 && !wide.starts_with(&verbatim) {
            let unc: Vec<u16> = r"\\".encode_utf16().collect();
            if wide.starts_with(&unc) {
                let mut full: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
                full.extend_from_slice(&wide[2..]);
                wide = full;
            } else if wide.get(1) == Some(&u16::from(b':')) {
                let mut full = verbatim;
                full.extend_from_slice(&wide);
                wide = full;
            }
        }
        wide.push(0);
        wide
    }

    fn read_u32(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    }

    fn read_u64(bytes: &[u8], at: usize) -> u64 {
        let mut raw = [0u8; 8];
        raw.copy_from_slice(&bytes[at..at + 8]);
        u64::from_le_bytes(raw)
    }

    /// FILETIME (100 ns since 1601) to ms since 1970.
    fn filetime_ms(filetime: u64) -> Option<u64> {
        const EPOCH_DIFF_MS: u64 = 11_644_473_600_000;
        (filetime / 10_000).checked_sub(EPOCH_DIFF_MS)
    }

    fn compressed_size(path: &Path) -> Option<u64> {
        let wide = wide(path);
        let mut high = 0u32;
        // SAFETY: `wide` is NUL-terminated and `high` is a valid out pointer.
        let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high) };
        if low == u32::MAX && io::Error::last_os_error().raw_os_error() != Some(0) {
            return None;
        }
        Some((u64::from(high) << 32) | u64::from(low))
    }

    pub(super) fn list_dir(path: &Path) -> Result<Listing, Failure> {
        let wide_path = wide(path);
        // SAFETY: `wide_path` is NUL-terminated; the other arguments are plain values.
        let raw = unsafe {
            CreateFileW(
                wide_path.as_ptr(),
                FILE_LIST_DIRECTORY | SYNCHRONIZE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(Failure::Io(io::Error::last_os_error()));
        }
        let handle = Handle(raw);

        let mut listing = Listing {
            entries: Vec::new(),
            error: None,
        };
        // u64 elements keep the buffer 8-byte aligned.
        let mut buffer = vec![0u64; 32 * 1024];
        let mut first = true;
        loop {
            let mut io_status = IoStatusBlock {
                status: 0,
                information: 0,
            };
            // SAFETY: the buffer and status block outlive the call; the length is the buffer's.
            let status = unsafe {
                NtQueryDirectoryFile(
                    handle.0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut io_status,
                    buffer.as_mut_ptr().cast(),
                    u32::try_from(buffer.len() * 8).unwrap_or(u32::MAX),
                    FILE_ID_FULL_DIRECTORY_INFORMATION,
                    0,
                    ptr::null_mut(),
                    u8::from(first),
                )
            };
            if status == STATUS_NO_MORE_FILES || status == STATUS_NO_SUCH_FILE {
                break;
            }
            if status < 0 {
                if first
                    && matches!(
                        status,
                        STATUS_NOT_IMPLEMENTED | STATUS_INVALID_INFO_CLASS | STATUS_NOT_SUPPORTED
                    )
                {
                    return Err(Failure::Unsupported);
                }
                // SAFETY: plain conversion of a status code.
                let code = unsafe { RtlNtStatusToDosError(status) };
                let err = io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX));
                if first {
                    return Err(Failure::Io(err));
                }
                listing.error = Some(err);
                break;
            }
            first = false;

            let used = io_status.information.min(buffer.len() * 8);
            // SAFETY: `used` bytes of the u64 buffer were just written by the call.
            let bytes = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), used) };
            parse(bytes, path, &mut listing.entries);
        }
        Ok(listing)
    }

    /// Reads every record in one batch the call returned.
    fn parse(bytes: &[u8], dir: &Path, out: &mut Vec<Entry>) {
        let mut at = 0usize;
        loop {
            if at + HEADER > bytes.len() {
                return;
            }
            let next = read_u32(bytes, at) as usize;
            let name_len = read_u32(bytes, at + 60) as usize;
            if at + HEADER + name_len > bytes.len() {
                return;
            }
            let units: Vec<u16> = bytes[at + HEADER..at + HEADER + name_len]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect();
            let name = OsString::from_wide(&units).to_string_lossy().into_owned();

            if name != "." && name != ".." {
                let attrs = read_u32(bytes, at + 56);
                let reparse_tag = if attrs & ATTR_REPARSE_POINT != 0 {
                    read_u32(bytes, at + 64)
                } else {
                    0
                };
                let is_link = matches!(reparse_tag, TAG_MOUNT_POINT | TAG_SYMLINK);
                if is_link {
                    out.push(plain(name, Kind::Link));
                } else if attrs & ATTR_DIRECTORY != 0 {
                    let mut entry = plain(name, Kind::Dir);
                    entry.cloud = false;
                    out.push(entry);
                } else {
                    out.push(file_entry(bytes, at, name, attrs, reparse_tag, dir));
                }
            }
            if next == 0 {
                return;
            }
            at += next;
        }
    }

    fn file_entry(
        bytes: &[u8],
        at: usize,
        name: String,
        attrs: u32,
        reparse_tag: u32,
        dir: &Path,
    ) -> Entry {
        let size = read_u64(bytes, at + 40);
        let mut disk = read_u64(bytes, at + 48);
        let cloud = attrs & CLOUD_ATTRIBUTES != 0;
        if cloud {
            disk = 0;
        } else if disk == 0
            && (size > SMALL_FILE
                || attrs & (ATTR_SPARSE | ATTR_COMPRESSED) != 0
                || reparse_tag == TAG_WOF)
        {
            // The listing shows no allocation for compressed, sparse and WOF files: ask for it.
            if let Some(real) = compressed_size(&dir.join(&name)) {
                disk = real;
            }
        }
        Entry {
            name,
            kind: Kind::File,
            size,
            disk,
            modified_ms: filetime_ms(read_u64(bytes, at + 24)),
            cloud,
            file_id: read_u64(bytes, at + 72),
        }
    }
}
