//! Putting back the last bulk delete: the items it sent to the Recycle Bin are found by their
//! original path and moved back. Nothing is ever overwritten.

use crate::log::LogEntry;
use crate::protect::Protected;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// One item in the Recycle Bin: where it came from, where it is kept now, and when it went in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinItem {
    pub original: PathBuf,
    pub stored: PathBuf,
    pub deleted_ms: u64,
}

/// Reads the `$I` file Windows writes beside each recycled item: a version (1 or 2), the size,
/// the deletion time as a FILETIME, then the original path in UTF-16. Returns the path and the
/// deletion time in Unix milliseconds.
pub fn parse_bin_info(bytes: &[u8]) -> Option<(String, u64)> {
    const FILETIME_TO_UNIX_MS: u64 = 11_644_473_600_000;
    let word = |at: usize, len: usize| bytes.get(at..at + len);
    let number = |at: usize| word(at, 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
    let (path_at, max_units) = match number(0)? {
        1 => (24, usize::MAX),
        2 => {
            let units = word(24, 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))?;
            (28, units as usize)
        }
        _ => return None,
    };
    let deleted_ms = (number(16)? / 10_000).checked_sub(FILETIME_TO_UNIX_MS)?;
    let units: Vec<u16> = bytes
        .get(path_at..)?
        .as_chunks::<2>()
        .0
        .iter()
        .take(max_units)
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|&unit| unit != 0)
        .collect();
    if units.is_empty() {
        return None;
    }
    Some((String::from_utf16_lossy(&units), deleted_ms))
}

/// The items in one bin folder (`$Recycle.Bin\<SID>`): every `$I…` file that still has its
/// matching `$R…` data. Anything unreadable is left out.
pub fn bin_items_in(dir: &Path) -> Vec<BinItem> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rest = name
                .strip_prefix("$I")
                .or_else(|| name.strip_prefix("$i"))?;
            let stored = ["$R", "$r"]
                .iter()
                .map(|prefix| dir.join(format!("{prefix}{rest}")))
                .find(|path| fs::symlink_metadata(path).is_ok())?;
            let (original, deleted_ms) = parse_bin_info(&fs::read(entry.path()).ok()?)?;
            Some(BinItem {
                original: PathBuf::from(original),
                stored,
                deleted_ms,
            })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RestoreOutcome {
    Restored,
    /// Something is at the original path now, so it was left as it is.
    Exists,
    /// Not in the Recycle Bin any more (emptied, or restored by hand).
    NotInBin,
    /// Deleted permanently: there is nothing to restore.
    Permanent,
    Failed,
}

impl RestoreOutcome {
    pub fn code(self) -> &'static str {
        match self {
            RestoreOutcome::Restored => "restored",
            RestoreOutcome::Exists => "exists",
            RestoreOutcome::NotInBin => "notInBin",
            RestoreOutcome::Permanent => "permanent",
            RestoreOutcome::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    pub path: String,
    pub size: u64,
    pub outcome: RestoreOutcome,
    pub message: Option<String>,
}

/// What the last bulk delete did, for the "Restore last cleanup" button.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    /// `2026-09-30 06:15:02 UTC`.
    pub time: String,
    /// Items that were deleted.
    pub items: u64,
    /// Of those, the ones sent to the Recycle Bin.
    pub recycled: u64,
}

/// The items the most recent bulk delete removed. Entries without a run id (older logs, and the
/// ones a restore writes) are not part of any run; items that were not deleted are left out.
pub fn last_run(entries: &[LogEntry]) -> Vec<&LogEntry> {
    let Some(newest) = entries
        .iter()
        .filter(|entry| entry.run.is_some() && entry.method != "restore")
        .max_by_key(|entry| entry.time_ms)
    else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|entry| entry.run == newest.run && entry.result == "done")
        .collect()
}

pub fn summary(entries: &[LogEntry]) -> Option<LastRun> {
    let run = last_run(entries);
    let first = run.first()?;
    Some(LastRun {
        time: first.time.clone(),
        items: run.len() as u64,
        recycled: run.iter().filter(|entry| entry.method == "recycle").count() as u64,
    })
}

/// Windows paths are not case-sensitive and the log may hold either slash.
fn key(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// Puts each item of the last run back where it was, using `bin` (what the Recycle Bin holds).
/// One item failing does not stop the rest. Nothing is put back into a place `protected` keeps:
/// the log is a user-writable file, so a line in it is not proof the item came from there.
pub fn restore_last(
    entries: &[LogEntry],
    bin: &[BinItem],
    protected: &Protected,
) -> Vec<RestoreResult> {
    last_run(entries)
        .into_iter()
        .map(|entry| {
            let (outcome, message) = restore_one(entry, bin, protected);
            RestoreResult {
                path: entry.path.clone(),
                size: entry.size,
                outcome,
                message,
            }
        })
        .collect()
}

fn restore_one(
    entry: &LogEntry,
    bin: &[BinItem],
    protected: &Protected,
) -> (RestoreOutcome, Option<String>) {
    if entry.method != "recycle" {
        return (
            RestoreOutcome::Permanent,
            Some("It was deleted permanently, so it cannot be restored.".into()),
        );
    }
    // The run logged its time after the deletes finished, so the right copy went in at or
    // before it; a later delete of the same path belongs to some other run.
    const CLOCK_SLACK_MS: u64 = 2_000;
    let wanted = key(&entry.path);
    let Some(found) = bin
        .iter()
        .filter(|item| {
            key(&item.original.to_string_lossy()) == wanted
                && item.deleted_ms <= entry.time_ms + CLOCK_SLACK_MS
        })
        .max_by_key(|item| item.deleted_ms)
    else {
        return (
            RestoreOutcome::NotInBin,
            Some("It is not in the Recycle Bin any more.".into()),
        );
    };

    let target = Path::new(&entry.path);
    if let Err(reason) = protected.check(target, &drive_root(&entry.path)) {
        return (
            RestoreOutcome::Failed,
            Some(format!("It was not put back. {reason}")),
        );
    }
    if fs::symlink_metadata(target).is_ok() {
        return (
            RestoreOutcome::Exists,
            Some("Something is already there, so it was not replaced.".into()),
        );
    }
    if let Some(parent) = target.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            return (
                RestoreOutcome::Failed,
                Some(format!("Could not recreate {}: {err}", parent.display())),
            );
        }
    }
    match fs::rename(&found.stored, target) {
        Ok(()) => (RestoreOutcome::Restored, None),
        Err(err) => (
            RestoreOutcome::Failed,
            Some(format!("Could not move it back: {err}")),
        ),
    }
}

/// `C:\` for `C:\a\b`, `\\host\share` for a share path: the widest folder a restore may land in.
fn drive_root(path: &str) -> PathBuf {
    let path = path.replace('/', "\\");
    if let Some(unc) = path.strip_prefix("\\\\") {
        let mut parts = unc.split('\\');
        return PathBuf::from(format!(
            "\\\\{}\\{}",
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default()
        ));
    }
    match path.split_once('\\') {
        Some((drive, _)) => PathBuf::from(format!("{drive}\\")),
        // Not a Windows path (the tests run elsewhere too): its filesystem root.
        None => Path::new(&path)
            .ancestors()
            .last()
            .map_or_else(|| PathBuf::from(&path), Path::to_path_buf),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_v2(path: &str, filetime: u64) -> Vec<u8> {
        let units: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let mut bytes = Vec::new();
        bytes.extend(2u64.to_le_bytes());
        bytes.extend(123u64.to_le_bytes());
        bytes.extend(filetime.to_le_bytes());
        bytes.extend((units.len() as u32).to_le_bytes());
        bytes.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
        bytes
    }

    // 2026-10-10 09:00:00 UTC in 100 ns ticks since 1601.
    const FILETIME: u64 = (1_791_622_800_000 + 11_644_473_600_000) * 10_000;

    #[test]
    fn bin_info_is_read_in_both_formats_and_rejects_junk() {
        let path = "C:\\Users\\Sam\\Ünï\\a.txt";
        assert_eq!(
            parse_bin_info(&info_v2(path, FILETIME)),
            Some((path.into(), 1_791_622_800_000))
        );

        let mut v1 = Vec::new();
        v1.extend(1u64.to_le_bytes());
        v1.extend(5u64.to_le_bytes());
        v1.extend(FILETIME.to_le_bytes());
        let mut field = [0u8; 520];
        for (i, unit) in path.encode_utf16().enumerate() {
            field[i * 2..i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
        }
        v1.extend(field);
        assert_eq!(parse_bin_info(&v1).map(|info| info.0), Some(path.into()));

        assert_eq!(parse_bin_info(b"short"), None);
        assert_eq!(parse_bin_info(&[9u8; 64]), None);
        assert_eq!(parse_bin_info(&info_v2("", FILETIME)), None);
    }

    #[test]
    fn a_bin_folder_lists_only_items_that_still_have_their_data() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path();
        fs::write(bin.join("$IAAA.txt"), info_v2("C:\\x\\one.txt", FILETIME)).unwrap();
        fs::write(bin.join("$RAAA.txt"), b"data").unwrap();
        fs::write(bin.join("$IBBB"), info_v2("C:\\x\\gone", FILETIME)).unwrap();

        let items = bin_items_in(bin);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].original, PathBuf::from("C:\\x\\one.txt"));
        assert_eq!(items[0].stored, bin.join("$RAAA.txt"));
        assert!(bin_items_in(&bin.join("missing")).is_empty());
    }

    fn entry(path: &Path, run: Option<&str>, time_ms: u64, method: &str, result: &str) -> LogEntry {
        LogEntry {
            time: "2026-10-10 09:00:00 UTC".into(),
            time_ms,
            user: "tech".into(),
            elevated: true,
            path: path.display().to_string(),
            size: 7,
            method: method.into(),
            result: result.into(),
            detail: None,
            run: run.map(Into::into),
        }
    }

    fn in_bin(bin: &Path, name: &str, original: &Path, deleted_ms: u64) -> BinItem {
        let stored = bin.join(name);
        fs::write(&stored, name).unwrap();
        BinItem {
            original: original.to_path_buf(),
            stored,
            deleted_ms,
        }
    }

    #[test]
    fn only_the_newest_run_counts_and_only_items_that_were_deleted() {
        let p = Path::new("x");
        let entries = [
            entry(p, Some("old"), 100, "recycle", "done"),
            entry(p, Some("new"), 300, "recycle", "done"),
            entry(p, Some("new"), 300, "recycle", "inUse"),
            entry(p, Some("new"), 300, "permanent", "done"),
            entry(p, None, 900, "restore", "restored"),
            entry(p, None, 950, "recycle", "done"),
        ];
        let run = last_run(&entries);
        assert_eq!(run.len(), 2);
        let said = summary(&entries).unwrap();
        assert_eq!((said.items, said.recycled), (2, 1));
        assert!(last_run(&entries[4..]).is_empty());
        assert!(summary(&[]).is_none());
    }

    #[test]
    fn restores_each_item_and_never_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        let home = temp.path().join("home");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&home).unwrap();

        let back = home.join("deep/er/back.txt");
        let taken = home.join("taken.txt");
        let lost = home.join("lost.txt");
        let perm = home.join("perm.txt");
        fs::write(&taken, "new file").unwrap();
        let items = [
            in_bin(&bin, "$R1", &back, 1_000),
            in_bin(&bin, "$R2", &taken, 1_000),
        ];
        let entries = [
            entry(&back, Some("r"), 1_500, "recycle", "done"),
            entry(&taken, Some("r"), 1_500, "recycle", "done"),
            entry(&lost, Some("r"), 1_500, "recycle", "done"),
            entry(&perm, Some("r"), 1_500, "permanent", "done"),
        ];

        let results = restore_last(&entries, &items, &Protected::default());
        let outcomes: Vec<RestoreOutcome> = results.iter().map(|r| r.outcome).collect();
        assert_eq!(
            outcomes,
            [
                RestoreOutcome::Restored,
                RestoreOutcome::Exists,
                RestoreOutcome::NotInBin,
                RestoreOutcome::Permanent,
            ]
        );
        assert_eq!(fs::read_to_string(&back).unwrap(), "$R1");
        assert!(!bin.join("$R1").exists());
        // The file that was already there is untouched and the bin copy is kept.
        assert_eq!(fs::read_to_string(&taken).unwrap(), "new file");
        assert!(bin.join("$R2").exists());

        // Running it again finds nothing left to put back.
        let again = restore_last(&entries, &items, &Protected::default());
        assert_eq!(again[0].outcome, RestoreOutcome::Exists);
    }

    #[test]
    fn picks_the_copy_from_this_run_when_the_path_went_in_twice() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let path = temp.path().join("again.txt");
        let items = [
            in_bin(&bin, "$Rold", &path, 1_000),
            in_bin(&bin, "$Rthis", &path, 5_000),
            in_bin(&bin, "$Rlater", &path, 99_000),
        ];
        let entries = [entry(&path, Some("r"), 5_400, "recycle", "done")];

        let results = restore_last(&entries, &items, &Protected::default());
        assert_eq!(results[0].outcome, RestoreOutcome::Restored);
        assert_eq!(fs::read_to_string(&path).unwrap(), "$Rthis");
    }

    #[test]
    fn paths_match_without_regard_to_case_or_slash_direction() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let path = temp.path().join("Mixed.TXT");
        let shouted = in_bin(
            &bin,
            "$R9",
            Path::new(&path.display().to_string().to_uppercase()),
            10,
        );
        let results = restore_last(
            &[entry(&path, Some("r"), 10, "recycle", "done")],
            &[shouted],
            &Protected::default(),
        );
        assert_eq!(results[0].outcome, RestoreOutcome::Restored);
    }

    #[test]
    fn a_target_in_a_protected_folder_is_refused_and_nothing_is_moved_or_created() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        let system = temp.path().join("Windows");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&system).unwrap();
        let planted = system.join("System32").join("evil.dll");
        let items = [in_bin(&bin, "$Revil", &planted, 1_000)];
        let entries = [entry(&planted, Some("r"), 1_200, "recycle", "done")];
        let protected = Protected::new(&[&system.to_string_lossy()], &[], &[]);

        let results = restore_last(&entries, &items, &protected);
        assert_eq!(results[0].outcome, RestoreOutcome::Failed);
        assert!(results[0]
            .message
            .as_deref()
            .unwrap()
            .contains("won't delete"));
        assert!(items[0].stored.exists());
        assert!(!planted.parent().unwrap().exists());
    }

    #[test]
    fn the_widest_folder_a_restore_may_land_in_is_the_drive_or_share() {
        assert_eq!(drive_root(r"C:\a\b.txt"), PathBuf::from(r"C:\"));
        assert_eq!(drive_root("d:/x/y"), PathBuf::from(r"d:\"));
        assert_eq!(
            drive_root(r"\\srv\share\dir\f"),
            PathBuf::from(r"\\srv\share")
        );
    }
}
