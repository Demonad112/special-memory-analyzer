//! What a copy keeps besides the bytes, in the spirit of Robocopy's `/COPY:DAT` and `/DCOPY:DAT`:
//! created, accessed and modified times, and the file attributes (read-only, hidden, system,
//! archive, ...). Applied after the data is in place; a failure here is a warning on the row, not
//! a failed copy. Windows-only; elsewhere only the modified time is kept (by the copy itself).

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Which parts of a file's metadata to carry over. Stored runs without it keep both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PreserveOptions {
    /// Created, accessed and modified times (files, and folders Deep Server Analyzer creates).
    pub timestamps: bool,
    /// Read-only, hidden, system, archive, not-indexed and temporary flags.
    pub attributes: bool,
    /// Permissions (the DACL, with its "inheritance blocked" flag).
    pub acl: bool,
    /// Owner and primary group. Setting someone else as owner needs the restore privilege.
    pub owner: bool,
    /// Audit rules (the SACL). Needs the security privilege, so off unless asked for.
    pub audit: bool,
    /// Alternate data streams such as `Zone.Identifier`.
    pub streams: bool,
}

impl Default for PreserveOptions {
    fn default() -> Self {
        Self {
            timestamps: true,
            attributes: true,
            acl: true,
            owner: true,
            audit: false,
            streams: true,
        }
    }
}

/// The metadata read from a source file or folder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceMeta {
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    /// Win32 attribute bits; 0 where the platform has none.
    pub attributes: u32,
}

/// Reads the times and attributes of `path`.
pub fn read(path: &Path) -> io::Result<SourceMeta> {
    let meta = std::fs::metadata(path)?;
    Ok(SourceMeta {
        created: meta.created().ok(),
        accessed: meta.accessed().ok(),
        modified: meta.modified().ok(),
        attributes: attributes_of(&meta),
    })
}

#[cfg(windows)]
fn attributes_of(meta: &std::fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}

#[cfg(not(windows))]
fn attributes_of(_meta: &std::fs::Metadata) -> u32 {
    0
}

/// Attribute bits that are copied. Reparse, compressed, encrypted, sparse and offline describe how
/// the source is stored, not what the file is, so they stay off the copy.
pub const COPIED_ATTRIBUTES: u32 = 0x1 | 0x2 | 0x4 | 0x20 | 0x100 | 0x2000;

/// `\\?\` (or `\\?\UNC\`) form of an absolute path, so the Win32 calls accept 260+ characters.
/// Relative and already-prefixed paths are returned as they are.
pub fn extended_path(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if text.starts_with(r"\\?\") {
        return path.to_path_buf();
    }
    if let Some(rest) = text.strip_prefix(r"\\") {
        return PathBuf::from(format!(r"\\?\UNC\{rest}"));
    }
    let bytes = text.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        return PathBuf::from(format!(r"\\?\{}", text.replace('/', r"\")));
    }
    path.to_path_buf()
}

/// Clears the read-only flag so an existing destination can be replaced. Best effort.
pub fn clear_read_only(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut permissions = meta.permissions();
        if permissions.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(path, permissions);
        }
    }
}

/// Puts `wanted` (read from `source`) on `destination` as far as `options` ask: streams first
/// (writing them touches the file), then dates and attributes, permissions and owner last so a
/// restrictive ACL cannot get in the way. Returns one sentence per part that could not be kept.
pub fn apply(
    source: &Path,
    destination: &Path,
    wanted: &SourceMeta,
    options: &PreserveOptions,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if options.streams {
        if let Err(error) = crate::secure::copy_streams(source, destination) {
            warnings.push(format!("Alternate data streams were not kept: {error}"));
        }
    }
    if options.timestamps {
        if let Err(error) = set_times(destination, wanted) {
            warnings.push(format!("Dates were not kept: {error}"));
        }
    }
    if options.attributes {
        if let Err(error) = set_attributes(destination, wanted.attributes) {
            warnings.push(format!("Attributes were not kept: {error}"));
        }
    }
    warnings.extend(crate::secure::copy_security(source, destination, options));
    warnings
}

#[cfg(windows)]
fn set_times(destination: &Path, source: &SourceMeta) -> io::Result<()> {
    use std::fs::{FileTimes, OpenOptions};
    use std::os::windows::fs::{FileTimesExt, OpenOptionsExt};

    const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    // Backup semantics lets the same call open a folder.
    let file = OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(extended_path(destination))?;
    let mut times = FileTimes::new();
    if let Some(time) = source.created {
        times = times.set_created(time);
    }
    if let Some(time) = source.accessed {
        times = times.set_accessed(time);
    }
    if let Some(time) = source.modified {
        times = times.set_modified(time);
    }
    file.set_times(times)
}

#[cfg(not(windows))]
fn set_times(destination: &Path, source: &SourceMeta) -> io::Result<()> {
    use std::fs::{FileTimes, OpenOptions};
    let file = OpenOptions::new().write(true).open(destination)?;
    let mut times = FileTimes::new();
    if let Some(time) = source.accessed {
        times = times.set_accessed(time);
    }
    if let Some(time) = source.modified {
        times = times.set_modified(time);
    }
    file.set_times(times)
}

#[cfg(windows)]
fn set_attributes(destination: &Path, source: u32) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_NORMAL, INVALID_FILE_ATTRIBUTES,
    };

    let mut wide: Vec<u16> = extended_path(destination)
        .as_os_str()
        .encode_wide()
        .collect();
    wide.push(0);
    // SAFETY: `wide` is NUL-terminated and outlives both calls.
    let current = unsafe { GetFileAttributesW(wide.as_ptr()) };
    if current == INVALID_FILE_ATTRIBUTES {
        return Err(io::Error::last_os_error());
    }
    // Keep what describes the destination's own storage (directory, compressed, ...).
    let mut wanted = (current & !COPIED_ATTRIBUTES) | (source & COPIED_ATTRIBUTES);
    if wanted == current {
        return Ok(());
    }
    if wanted == 0 {
        wanted = FILE_ATTRIBUTE_NORMAL;
    }
    // SAFETY: as above.
    if unsafe { SetFileAttributesW(wide.as_ptr(), wanted) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn set_attributes(_destination: &Path, _source: u32) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_path_prefixes_absolute_paths_once() {
        assert_eq!(
            extended_path(Path::new(r"C:\a\b")),
            PathBuf::from(r"\\?\C:\a\b")
        );
        assert_eq!(
            extended_path(Path::new(r"\\srv\share\a")),
            PathBuf::from(r"\\?\UNC\srv\share\a")
        );
        assert_eq!(
            extended_path(Path::new(r"\\?\C:\a")),
            PathBuf::from(r"\\?\C:\a")
        );
        assert_eq!(extended_path(Path::new("rel/a")), PathBuf::from("rel/a"));
    }

    #[test]
    fn defaults_keep_everything_and_old_records_load() {
        let from_empty: PreserveOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(from_empty, PreserveOptions::default());
        assert!(from_empty.timestamps && from_empty.attributes);
    }

    #[cfg(windows)]
    #[test]
    fn created_accessed_modified_and_attributes_round_trip() {
        use std::os::windows::fs::MetadataExt;
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("ds-meta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("s.txt");
        let target = dir.join("t.txt");
        std::fs::write(&source, b"x").unwrap();
        std::fs::write(&target, b"x").unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_300_000_000);
        let mut wanted = read(&source).unwrap();
        wanted.created = Some(old);
        wanted.accessed = Some(old + Duration::from_secs(60));
        wanted.modified = Some(old + Duration::from_secs(120));
        wanted.attributes = 0x2 | 0x1;

        let warnings = apply(&source, &target, &wanted, &PreserveOptions::default());

        assert!(warnings.is_empty(), "{warnings:?}");
        let got = std::fs::metadata(&target).unwrap();
        assert_eq!(got.created().unwrap(), old);
        assert_eq!(got.modified().unwrap(), old + Duration::from_secs(120));
        assert_eq!(got.file_attributes() & 0x3, 0x3);
        clear_read_only(&target);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
