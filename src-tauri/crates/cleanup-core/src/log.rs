//! The deletion log: one JSON object per line in `deletions.jsonl`, appended for every item a
//! bulk delete tried, so a technician can show what was removed, when, and by whom.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub const LOG_FILE: &str = "deletions.jsonl";
/// How many full logs are kept next to the current one, as `deletions.1.jsonl` (newest) to
/// `deletions.5.jsonl` (oldest): about 30 MB of history in all.
const ROTATIONS: u32 = 5;
/// The log is moved aside once it reaches this size.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;

fn rotated_name(n: u32) -> String {
    format!("deletions.{n}.jsonl")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// `2026-09-30 06:15:02 UTC`.
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub time_ms: u64,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub elevated: bool,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: u64,
    /// "recycle", "permanent", or "restore" for an item put back from the Recycle Bin.
    #[serde(default)]
    pub method: String,
    /// "done", "inUse", "accessDenied", "notFound", "skipped" or "failed".
    #[serde(default)]
    pub result: String,
    pub detail: Option<String>,
    /// Groups the entries of one bulk delete, so the last one can be restored. Absent in logs
    /// from before restore existed, and on the entries a restore itself writes.
    #[serde(default)]
    pub run: Option<String>,
}

/// Moves the full log to `deletions.1.jsonl`, shifting the older copies up one place. The oldest
/// copy ages out. Stops at the first copy that cannot move, so no kept entry is overwritten.
fn rotate(dir: &Path, file: &Path) -> io::Result<()> {
    let _ = fs::remove_file(dir.join(rotated_name(ROTATIONS)));
    for n in (1..ROTATIONS).rev() {
        let from = dir.join(rotated_name(n));
        if from.exists() {
            fs::rename(&from, dir.join(rotated_name(n + 1)))?;
        }
    }
    fs::rename(file, dir.join(rotated_name(1)))
}

/// Appends `entries` to `<dir>/deletions.jsonl`, creating the folder, and rotates a full log.
pub fn append(dir: &Path, entries: &[LogEntry]) -> io::Result<()> {
    append_limited(dir, entries, MAX_BYTES)
}

fn append_limited(dir: &Path, entries: &[LogEntry], max_bytes: u64) -> io::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    let file = dir.join(LOG_FILE);
    if fs::metadata(&file).is_ok_and(|meta| meta.len() >= max_bytes) {
        // A failed rotation must not lose the new entries: keep appending to the big file.
        let _ = rotate(dir, &file);
    }
    let mut text = String::new();
    for entry in entries {
        text.push_str(&serde_json::to_string(entry).map_err(io::Error::other)?);
        text.push('\n');
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?
        .write_all(text.as_bytes())
}

/// Every entry still on disk, oldest first: the rotated copies from the oldest, then the current
/// log. A line that does not parse is skipped, so one bad line never hides the rest.
pub fn read_all(dir: &Path) -> Vec<LogEntry> {
    (1..=ROTATIONS)
        .rev()
        .map(rotated_name)
        .chain([LOG_FILE.to_owned()])
        .filter_map(|name| fs::read_to_string(dir.join(name)).ok())
        .flat_map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<LogEntry>(line).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> LogEntry {
        LogEntry {
            time: "2026-10-10 09:00:00 UTC".into(),
            time_ms: 1,
            user: "tech".into(),
            elevated: true,
            path: path.into(),
            size: 42,
            method: "recycle".into(),
            result: "done".into(),
            detail: None,
            run: None,
        }
    }

    #[test]
    fn a_log_line_with_only_a_path_and_time_parses() {
        let entry: LogEntry = serde_json::from_str(r#"{"path":"C:\\a","timeMs":5}"#).unwrap();
        assert_eq!(entry.path, "C:\\a");
        assert_eq!(entry.time_ms, 5);
        assert!(entry.user.is_empty() && !entry.elevated && entry.run.is_none());
    }

    #[test]
    fn appends_one_json_line_per_entry_and_keeps_odd_names_intact() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Logs");
        let hostile = "C:\\a\"b\nc\\<x>.txt";
        append(&dir, &[entry("one"), entry(hostile)]).unwrap();
        append(&dir, &[entry("three")]).unwrap();

        let text = fs::read_to_string(dir.join(LOG_FILE)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        let parsed: LogEntry = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(parsed.path, hostile);
        assert!(parsed.elevated);
    }

    #[test]
    fn a_full_log_is_rotated_and_the_new_entry_lands_in_a_fresh_file() {
        let temp = tempfile::tempdir().unwrap();
        let big = vec![b'x'; MAX_BYTES as usize];
        fs::write(temp.path().join(LOG_FILE), &big).unwrap();
        append(temp.path(), &[entry("fresh")]).unwrap();

        assert_eq!(
            fs::metadata(temp.path().join(rotated_name(1)))
                .unwrap()
                .len(),
            MAX_BYTES
        );
        let text = fs::read_to_string(temp.path().join(LOG_FILE)).unwrap();
        assert_eq!(text.lines().count(), 1);
    }

    fn counted(n: u32) -> LogEntry {
        entry(&format!("item-{n}"))
    }

    #[test]
    fn five_rotated_logs_are_kept_and_read_oldest_first() {
        let temp = tempfile::tempdir().unwrap();
        // One entry per file: every append after the first finds a full log and rotates.
        for n in 0..8 {
            append_limited(temp.path(), &[counted(n)], 1).unwrap();
        }

        for kept in 1..=ROTATIONS {
            assert!(temp.path().join(rotated_name(kept)).exists(), "{kept}");
        }
        assert!(!temp.path().join(rotated_name(ROTATIONS + 1)).exists());
        // Items 0 and 1 aged out of the five rotated files; 2..=6 are rotated, 7 is current.
        let paths: Vec<String> = read_all(temp.path())
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        assert_eq!(
            paths,
            ["item-2", "item-3", "item-4", "item-5", "item-6", "item-7"]
        );
    }

    #[test]
    fn a_rotation_that_cannot_shift_keeps_every_entry() {
        let temp = tempfile::tempdir().unwrap();
        let line = |n: u32| format!("{}\n", serde_json::to_string(&counted(n)).unwrap());
        fs::write(temp.path().join(rotated_name(4)), line(0)).unwrap();
        fs::write(temp.path().join(LOG_FILE), line(1)).unwrap();
        // A folder in the oldest slot cannot be replaced, so deletions.4 cannot move up.
        let blocker = temp.path().join(rotated_name(ROTATIONS));
        fs::create_dir(&blocker).unwrap();
        fs::write(blocker.join("keep"), b"x").unwrap();
        append_limited(temp.path(), &[counted(2)], 1).unwrap();

        let paths: Vec<String> = read_all(temp.path())
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        assert_eq!(paths, ["item-0", "item-1", "item-2"]);
    }

    #[test]
    fn old_lines_without_a_run_id_still_parse_and_bad_lines_are_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let old = r#"{"time":"t","timeMs":5,"user":"u","elevated":false,"path":"C:\\a","size":1,"method":"recycle","result":"done","detail":null}"#;
        let new = serde_json::to_string(&LogEntry {
            run: Some("r1".into()),
            ..entry("two")
        })
        .unwrap();
        fs::write(temp.path().join(rotated_name(1)), format!("{old}\n")).unwrap();
        fs::write(temp.path().join(LOG_FILE), format!("not json\n{new}\n")).unwrap();

        let all = read_all(temp.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].run, None);
        assert_eq!(all[1].run.as_deref(), Some("r1"));
        assert!(read_all(&temp.path().join("missing")).is_empty());
    }

    #[test]
    fn a_log_folder_that_cannot_be_created_is_an_error_not_a_panic() {
        // `cleanup_delete_many` turns this error into the `logError` the page shows.
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("Logs");
        fs::write(&blocker, b"a file where the folder should be").unwrap();
        assert!(append(&blocker, &[entry("one")]).is_err());
    }

    #[test]
    fn nothing_to_log_creates_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Logs");
        append(&dir, &[]).unwrap();
        assert!(!dir.exists());
    }
}
