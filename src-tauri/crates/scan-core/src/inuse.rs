//! Files another program holds open: a quick open probe for every file, and the names of the
//! programs behind the first few through the Restart Manager.
//!
//! The probe opens the file for read with full sharing, which is what a copy will hit: a sharing
//! or lock violation means another program holds it without sharing. It never opens a link
//! (reparse points are opened as themselves, and links and online-only files are not opened at
//! all, so nothing is downloaded from the cloud) and never reads a byte. The Windows calls are
//! behind `#[cfg(windows)]`; elsewhere every file is "not checked".

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Windows error codes for a file another program holds.
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_LOCK_VIOLATION: i32 = 33;

/// Restart Manager is slow per call; readiness asks for at most this many files (D-08).
pub const HOLDER_CAP: usize = 500;
/// And spends at most this long on them (D-08).
pub const HOLDER_DEADLINE: Duration = Duration::from_secs(30);

/// What the open probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum InUse {
    Free,
    /// Another program holds it open without sharing, or has a byte range locked.
    InUse,
    /// The open failed for another reason (access denied, gone, ...), with the Windows code.
    Unreadable {
        code: i32,
    },
    /// Not opened: a link, an online-only file, or not a Windows system.
    NotChecked,
}

impl InUse {
    /// The probe's result for a failed open with this OS error code.
    pub fn from_error_code(code: Option<i32>) -> Self {
        match code {
            Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) => Self::InUse,
            Some(code) => Self::Unreadable { code },
            None => Self::Unreadable { code: -1 },
        }
    }
}

/// Probes one file. See the module notes for what it never opens.
pub fn probe(path: &Path) -> InUse {
    imp::probe(path)
}

/// A program holding a file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Holder {
    /// The program's name as Restart Manager gives it (`Microsoft Outlook`, `OUTLOOK.EXE`).
    pub app: String,
    pub pid: u32,
}

/// Who holds one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "result")]
pub enum Holders {
    /// The programs Restart Manager named (empty when it found none, for example a program on
    /// another machine holding a local file through a share).
    Found { holders: Vec<Holder> },
    /// The file is on a share: only the server knows who holds it.
    Remote,
    /// Past the cap or the deadline, so not asked.
    NotChecked,
    /// Restart Manager failed with this Windows code.
    Failed { code: i32 },
    /// Not on this system (Restart Manager needs Windows).
    NotSupported,
}

/// The holders of each path, in the same order: one Restart Manager session per file, for at
/// most `cap` local files and until `deadline` has passed; the rest are [`Holders::NotChecked`].
/// Paths on a share are [`Holders::Remote`] and cost nothing.
pub fn holders(paths: &[PathBuf], cap: usize, deadline: Duration) -> Vec<Holders> {
    holders_with(paths, cap, deadline, Instant::now, imp::holders_of)
}

/// [`holders`] with the clock and the per-file lookup passed in.
fn holders_with(
    paths: &[PathBuf],
    cap: usize,
    deadline: Duration,
    now: impl Fn() -> Instant,
    mut lookup: impl FnMut(&Path) -> Holders,
) -> Vec<Holders> {
    let started = now();
    let mut asked = 0;
    paths
        .iter()
        .map(|path| {
            if is_remote(path) {
                Holders::Remote
            } else if asked >= cap || now().duration_since(started) >= deadline {
                Holders::NotChecked
            } else {
                asked += 1;
                lookup(path)
            }
        })
        .collect()
}

/// A `\\server\share` path (also as `\\?\UNC\...`). A `\\?\C:\` path is local.
pub fn is_remote(path: &Path) -> bool {
    let text = path.to_string_lossy().replace('/', "\\");
    let upper = text.to_ascii_uppercase();
    if upper.starts_with(r"\\?\UNC\") {
        return true;
    }
    if upper.starts_with(r"\\?\") || upper.starts_with(r"\\.\") {
        return false;
    }
    text.starts_with(r"\\")
}

#[cfg(windows)]
mod imp {
    use super::{Holder, Holders, InUse};
    use crate::list::wide_path;
    use std::fs::{self, OpenOptions};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;
    use std::ptr;
    use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    use windows_sys::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
        RM_PROCESS_INFO,
    };

    const ATTR_REPARSE_POINT: u32 = 0x400;
    /// Offline, recall-on-open and recall-on-data-access: the data is in the cloud.
    const CLOUD_ATTRIBUTES: u32 = 0x1000 | 0x4_0000 | 0x40_0000;

    pub fn probe(path: &Path) -> InUse {
        // The attributes come from the directory entry; nothing is opened yet.
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_attributes() & (ATTR_REPARSE_POINT | CLOUD_ATTRIBUTES) != 0 => {
                return InUse::NotChecked
            }
            Ok(meta) if !meta.is_file() => return InUse::NotChecked,
            Ok(_) => {}
            Err(error) => return InUse::from_error_code(error.raw_os_error()),
        }
        // No backup semantics: a plain open is what the copy does, and it must not bypass ACLs.
        let opened = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path);
        match opened {
            // Closed at once; nothing is read.
            Ok(_file) => InUse::Free,
            Err(error) => InUse::from_error_code(error.raw_os_error()),
        }
    }

    /// Ends the session on every path out of [`holders_of`].
    struct Session(u32);

    impl Drop for Session {
        fn drop(&mut self) {
            // SAFETY: the handle came from RmStartSession and is ended once.
            unsafe { RmEndSession(self.0) };
        }
    }

    fn code(error: u32) -> Holders {
        Holders::Failed {
            code: i32::try_from(error).unwrap_or(i32::MAX),
        }
    }

    pub fn holders_of(path: &Path) -> Holders {
        let mut handle = 0u32;
        let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
        // SAFETY: `handle` and `key` are valid out buffers of the documented sizes.
        let started = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
        if started != ERROR_SUCCESS {
            return code(started);
        }
        let session = Session(handle);
        let wide = wide_path(path);
        let names = [wide.as_ptr()];
        // SAFETY: one NUL-terminated name that outlives the call; no apps or services.
        let registered = unsafe {
            RmRegisterResources(session.0, 1, names.as_ptr(), 0, ptr::null(), 0, ptr::null())
        };
        if registered != ERROR_SUCCESS {
            return code(registered);
        }
        let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
        loop {
            let mut needed = 0u32;
            let mut count = u32::try_from(infos.len()).unwrap_or(u32::MAX);
            let mut reasons = 0u32;
            // SAFETY: `infos` holds `count` entries and outlives the call.
            let listed = unsafe {
                RmGetList(
                    session.0,
                    &mut needed,
                    &mut count,
                    infos.as_mut_ptr(),
                    &mut reasons,
                )
            };
            if listed == ERROR_SUCCESS {
                infos.truncate(count as usize);
                break;
            }
            if listed != ERROR_MORE_DATA || needed as usize <= infos.len() {
                return code(listed);
            }
            // SAFETY: RM_PROCESS_INFO is plain data; all zeroes is a valid value.
            infos = vec![unsafe { std::mem::zeroed() }; needed as usize];
        }
        let holders = infos
            .iter()
            .map(|info| {
                let name = &info.strAppName;
                let length = name
                    .iter()
                    .position(|&unit| unit == 0)
                    .unwrap_or(name.len());
                Holder {
                    app: String::from_utf16_lossy(&name[..length]),
                    pid: info.Process.dwProcessId,
                }
            })
            .collect();
        Holders::Found { holders }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Holders, InUse};
    use std::path::Path;

    pub fn probe(_path: &Path) -> InUse {
        InUse::NotChecked
    }

    pub fn holders_of(_path: &Path) -> Holders {
        Holders::NotSupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn sharing_and_lock_violations_mean_in_use() {
        assert_eq!(InUse::from_error_code(Some(32)), InUse::InUse);
        assert_eq!(InUse::from_error_code(Some(33)), InUse::InUse);
        assert_eq!(
            InUse::from_error_code(Some(5)),
            InUse::Unreadable { code: 5 }
        );
        assert_eq!(InUse::from_error_code(None), InUse::Unreadable { code: -1 });
    }

    #[test]
    fn share_paths_are_remote() {
        for remote in [
            r"\\fs01\Finance\a.pst",
            r"\\fs01\C$\Users",
            r"\\?\UNC\fs01\Finance\a.pst",
            r"\\?\unc\fs01\x",
            "//fs01/share/x",
        ] {
            assert!(is_remote(Path::new(remote)), "{remote}");
        }
        for local in [
            r"C:\Data\a.pst",
            r"\\?\C:\Data\a.pst",
            r"\\.\C:\x",
            "relative\\a",
        ] {
            assert!(!is_remote(Path::new(local)), "{local}");
        }
    }

    #[test]
    fn holders_skip_shares_and_stop_at_the_cap() {
        let paths: Vec<PathBuf> = [r"C:\a", r"\\fs01\s\b", r"C:\c", r"C:\d"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let mut asked = Vec::new();
        let found = holders_with(&paths, 2, Duration::from_secs(30), Instant::now, |path| {
            asked.push(path.to_path_buf());
            Holders::Found {
                holders: vec![Holder {
                    app: "OUTLOOK.EXE".to_owned(),
                    pid: 4120,
                }],
            }
        });
        assert_eq!(asked, vec![PathBuf::from(r"C:\a"), PathBuf::from(r"C:\c")]);
        assert!(matches!(found[0], Holders::Found { .. }));
        assert_eq!(found[1], Holders::Remote);
        assert!(matches!(found[2], Holders::Found { .. }));
        assert_eq!(found[3], Holders::NotChecked);
    }

    #[test]
    fn holders_stop_at_the_deadline() {
        let paths: Vec<PathBuf> = (0..4).map(|i| PathBuf::from(format!(r"C:\f{i}"))).collect();
        let start = Instant::now();
        // Each lookup takes 10 s on this clock.
        let tick = Cell::new(0u64);
        let now = || start + Duration::from_secs(tick.get());
        let found = holders_with(&paths, HOLDER_CAP, HOLDER_DEADLINE, now, |_| {
            tick.set(tick.get() + 10);
            Holders::Found {
                holders: Vec::new(),
            }
        });
        let checked = found
            .iter()
            .filter(|result| **result != Holders::NotChecked)
            .count();
        assert_eq!(checked, 3, "{found:?}");
        assert_eq!(found[3], Holders::NotChecked);
    }

    #[cfg(not(windows))]
    #[test]
    fn nothing_is_checked_off_windows() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(probe(&file), InUse::NotChecked);
        assert_eq!(
            holders(&[file], HOLDER_CAP, HOLDER_DEADLINE),
            vec![Holders::NotSupported]
        );
    }

    #[cfg(windows)]
    mod windows {
        use super::super::*;
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;

        #[test]
        fn a_file_held_without_sharing_is_in_use_and_held_by_this_process() {
            let dir = tempfile::tempdir().unwrap();
            let held = dir.path().join("held.pst");
            let free = dir.path().join("free.txt");
            std::fs::write(&held, b"mail").unwrap();
            std::fs::write(&free, b"text").unwrap();
            let _lock = OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0)
                .open(&held)
                .unwrap();

            assert_eq!(probe(&held), InUse::InUse);
            assert_eq!(probe(&free), InUse::Free);

            let found = holders(&[held, free], HOLDER_CAP, HOLDER_DEADLINE);
            let Holders::Found { holders } = &found[0] else {
                panic!("{found:?}")
            };
            assert!(
                holders
                    .iter()
                    .any(|holder| holder.pid == std::process::id()),
                "{holders:?}"
            );
            assert_eq!(
                found[1],
                Holders::Found {
                    holders: Vec::new()
                }
            );
        }

        #[test]
        fn folders_and_junctions_are_not_opened() {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("target");
            std::fs::create_dir(&target).unwrap();
            let link = dir.path().join("link");
            let output = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&target)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");

            assert_eq!(probe(&target), InUse::NotChecked);
            assert_eq!(probe(&link), InUse::NotChecked);
            std::fs::remove_dir(&link).unwrap();
        }

        #[test]
        fn a_missing_file_is_unreadable() {
            let dir = tempfile::tempdir().unwrap();
            assert!(matches!(
                probe(&dir.path().join("gone.txt")),
                InUse::Unreadable { .. }
            ));
        }
    }
}
