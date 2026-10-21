//! The deletion log: one JSON object per line in `deletions.jsonl`, appended for every item a
//! bulk delete tried, so a technician can show what was removed, when, and by whom.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub const LOG_FILE: &str = "deletions.jsonl";
const ROTATED_FILE: &str = "deletions.1.jsonl";
/// The log is moved aside (replacing the previous copy) once it reaches this size.
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// `2026-09-30 06:15:02 UTC`.
    pub time: String,
    pub time_ms: u64,
    pub user: String,
    pub elevated: bool,
    pub path: String,
    pub size: u64,
    /// "recycle", "permanent", or "restore" for an item put back from the Recycle Bin.
    pub method: String,
    /// "done", "inUse", "accessDenied", "notFound", "skipped" or "failed".
    pub result: String,
    pub detail: Option<String>,
    /// Groups the entries of one bulk delete, so the last one can be restored. Absent in logs
    /// from before restore existed, and on the entries a restore itself writes.
    #[serde(default)]
    pub run: Option<String>,
}

/// Appends `entries` to `<dir>/deletions.jsonl`, creating the folder, and rotates a full log.
pub fn append(dir: &Path, entries: &[LogEntry]) -> io::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    let file = dir.join(LOG_FILE);
    if fs::metadata(&file).is_ok_and(|meta| meta.len() >= MAX_BYTES) {
        // A failed rotation must not lose the new entries: keep appending to the big file.
        let _ = fs::remove_file(dir.join(ROTATED_FILE));
        let _ = fs::rename(&file, dir.join(ROTATED_FILE));
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

/// Every entry still on disk, oldest first: the rotated copy, then the current log. A line that
/// does not parse is skipped, so one bad line never hides the rest.
pub fn read_all(dir: &Path) -> Vec<LogEntry> {
    [ROTATED_FILE, LOG_FILE]
        .iter()
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
            fs::metadata(temp.path().join(ROTATED_FILE)).unwrap().len(),
            MAX_BYTES
        );
        let text = fs::read_to_string(temp.path().join(LOG_FILE)).unwrap();
        assert_eq!(text.lines().count(), 1);
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
        fs::write(temp.path().join(ROTATED_FILE), format!("{old}\n")).unwrap();
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
