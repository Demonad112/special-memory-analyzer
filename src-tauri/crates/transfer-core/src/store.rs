//! The files of one run folder, and the list of runs.

use crate::{
    now_ms, ItemKind, ItemResult, ItemStatus, Result, RunSummary, RunTotals, TransferError,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

const SUMMARY_FILE: &str = "run.json";
const PREFLIGHT_FILE: &str = "preflight.json";
const MANIFEST_FILE: &str = "manifest.jsonl";
const RESULTS_FILE: &str = "results.jsonl";
const AUDIT_FILE: &str = "audit.jsonl";

/// How a run step ended, in `audit.jsonl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LogStatus {
    Started,
    Succeeded,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone)]
pub struct RunStore {
    dir: PathBuf,
}

impl RunStore {
    /// A new, empty run folder named after the current time (`20260930-061502-123`).
    pub fn create(root: &Path) -> Result<Self> {
        fs::create_dir_all(root)?;
        let base = run_id_for(now_ms());
        for attempt in 0..1000 {
            let id = if attempt == 0 {
                base.clone()
            } else {
                format!("{base}-{attempt}")
            };
            let dir = root.join(&id);
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(Self { dir }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(TransferError::BadRun(
            "Could not create a run folder".to_owned(),
        ))
    }

    /// An existing run. `id` must be a plain folder name under `root`.
    pub fn open(root: &Path, id: &str) -> Result<Self> {
        if id.is_empty() || id.contains(['/', '\\', ':']) || id.starts_with('.') {
            return Err(TransferError::BadRun(format!("Not a run id: {id}")));
        }
        let dir = root.join(id);
        if !dir.join(SUMMARY_FILE).is_file() {
            return Err(TransferError::BadRun(format!("Run {id} was not found")));
        }
        Ok(Self { dir })
    }

    pub fn id(&self) -> String {
        self.dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn summary(&self) -> Result<RunSummary> {
        read_json(&self.dir.join(SUMMARY_FILE))
    }

    pub fn save_summary(&self, summary: &RunSummary) -> Result<()> {
        write_json(&self.dir.join(SUMMARY_FILE), summary)
    }

    pub fn preflight<T: DeserializeOwned>(&self) -> Result<T> {
        read_json(&self.dir.join(PREFLIGHT_FILE))
    }

    pub fn save_preflight<T: Serialize>(&self, preflight: &T) -> Result<()> {
        write_json(&self.dir.join(PREFLIGHT_FILE), preflight)
    }

    pub fn manifest_writer(&self) -> Result<JsonlWriter> {
        JsonlWriter::create(&self.dir.join(MANIFEST_FILE), false)
    }

    /// Manifest rows in the order they were written.
    pub fn manifest<T: DeserializeOwned>(&self) -> Result<impl Iterator<Item = Result<T>>> {
        read_jsonl(&self.dir.join(MANIFEST_FILE))
    }

    /// Appends to `results.jsonl`, flushing each row so a crash loses at most the file in flight.
    pub fn results_writer(&self) -> Result<JsonlWriter> {
        JsonlWriter::create(&self.dir.join(RESULTS_FILE), true)
    }

    /// The latest outcome per relative path.
    pub fn latest_results(&self) -> Result<BTreeMap<String, ItemResult>> {
        let path = self.dir.join(RESULTS_FILE);
        let mut latest = BTreeMap::new();
        if !path.is_file() {
            return Ok(latest);
        }
        for row in read_jsonl::<ItemResult>(&path)? {
            let row = row?;
            latest.insert(row.relative_path.clone(), row);
        }
        Ok(latest)
    }

    /// Recounts copied/skipped/not-copied from the results; planned counts are kept.
    pub fn refresh_totals(&self, summary: &mut RunSummary) -> Result<()> {
        summary.totals = totals_from(&summary.totals, self.latest_results()?.values());
        Ok(())
    }

    /// Adds a line to `audit.jsonl`. Audit problems never stop a run.
    pub fn audit(
        &self,
        action: &str,
        status: LogStatus,
        message: impl Into<String>,
        details: &[(&str, serde_json::Value)],
    ) {
        let details: serde_json::Map<String, serde_json::Value> = details
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect();
        let event = serde_json::json!({
            "atMs": now_ms(),
            "action": action,
            "status": status,
            "message": message.into(),
            "details": details,
        });
        if let Ok(mut writer) = JsonlWriter::create(&self.dir.join(AUDIT_FILE), true) {
            let _ = writer.write(&event);
        }
    }
}

/// Planned counts from `planned`, outcome counts from `results`.
pub fn totals_from<'a>(
    planned: &RunTotals,
    results: impl Iterator<Item = &'a ItemResult>,
) -> RunTotals {
    let mut totals = RunTotals {
        planned_files: planned.planned_files,
        planned_bytes: planned.planned_bytes,
        folders: planned.folders,
        excluded: planned.excluded,
        ..RunTotals::default()
    };
    for item in results {
        // Folders count only when they could not be copied (an unreadable folder).
        match item.status {
            ItemStatus::Copied | ItemStatus::Arrived if item.kind == ItemKind::File => {
                totals.copied += 1;
                totals.copied_bytes += item.size;
            }
            ItemStatus::SkippedIdentical if item.kind == ItemKind::File => {
                totals.skipped_identical += 1
            }
            ItemStatus::Copied | ItemStatus::Arrived | ItemStatus::SkippedIdentical => {}
            ItemStatus::NotCopied => {
                totals.not_copied += 1;
                totals.not_copied_bytes += item.size;
            }
        }
    }
    totals
}

/// Every run under `root`, newest first. Folders that are not runs are skipped.
pub fn list_runs(root: &Path) -> Vec<RunSummary> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut runs: Vec<RunSummary> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| read_json(&entry.path().join(SUMMARY_FILE)).ok())
        .collect();
    runs.sort_by(|left, right| {
        right
            .created_at_ms
            .cmp(&left.created_at_ms)
            .then(right.id.cmp(&left.id))
    });
    runs
}

fn run_id_for(ms: u64) -> String {
    // "2026-09-30 06:15:02 UTC" → "20260930-061502-123"
    let text = crate::format_utc(ms);
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    format!("{}-{}-{:03}", &digits[..8], &digits[8..14], ms % 1000)
}

pub struct JsonlWriter {
    writer: BufWriter<File>,
    flush_each: bool,
}

impl JsonlWriter {
    fn create(path: &Path, append: bool) -> Result<Self> {
        let file = if append {
            OpenOptions::new().create(true).append(true).open(path)?
        } else {
            File::create(path)?
        };
        Ok(Self {
            writer: BufWriter::new(file),
            flush_each: append,
        })
    }

    pub fn write<T: Serialize>(&mut self, row: &T) -> Result<()> {
        serde_json::to_writer(&mut self.writer, row).map_err(std::io::Error::other)?;
        self.writer.write_all(b"\n")?;
        if self.flush_each {
            self.writer.flush()?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }
}

fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<impl Iterator<Item = Result<T>>> {
    let reader = BufReader::new(File::open(path)?);
    Ok(reader.lines().filter_map(|line| match line {
        Ok(line) if line.trim().is_empty() => None,
        Ok(line) => Some(
            serde_json::from_str(&line)
                .map_err(|error| TransferError::BadRun(format!("Bad row in run file: {error}"))),
        ),
        Err(error) => Some(Err(error.into())),
    }))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map_err(|error| TransferError::BadRun(format!("{}: {error}", path.display())))
}

/// Written to a `.tmp` file and renamed, so a crash never leaves half a file.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use crate::FailureReason;

    #[test]
    fn run_ids_sort_by_time_and_do_not_collide() {
        assert_eq!(run_id_for(1_790_000_000_123), "20260921-141320-123");
        let dir = TempDir::new("store-ids");
        let first = RunStore::create(&dir.path("runs")).unwrap();
        let second = RunStore::create(&dir.path("runs")).unwrap();
        assert_ne!(first.id(), second.id());
    }

    fn stored_run(root: &Path, created_at_ms: u64) -> String {
        let store = RunStore::create(root).unwrap();
        let summary = RunSummary {
            id: store.id(),
            settings: crate::TransferSettings {
                source: "C:\\src".to_owned(),
                destination: "D:\\dst".to_owned(),
                mode: crate::TransferMode::Copy,
                verify: crate::VerifyLevel::SizeAndTime,
                conflict: crate::ConflictPolicy::Skip,
                ignore_junk: false,
                download_cloud: false,
                preserve: Default::default(),
            },
            state: crate::RunState::Completed,
            created_at_ms,
            started_at_ms: None,
            finished_at_ms: None,
            machine: String::new(),
            user: String::new(),
            totals: RunTotals::default(),
            error: None,
        };
        store.save_summary(&summary).unwrap();
        store.id()
    }

    #[test]
    fn runs_are_listed_newest_first_and_never_pruned() {
        let dir = TempDir::new("store-list");
        let root = dir.path("runs");
        let old = stored_run(&root, 1_000);
        let new = stored_run(&root, 2_000);
        fs::create_dir_all(root.join("not-a-run")).unwrap();
        let ids: Vec<String> = list_runs(&root).into_iter().map(|run| run.id).collect();
        assert_eq!(ids, [new, old]);
    }

    #[test]
    fn open_rejects_ids_that_leave_the_root() {
        let dir = TempDir::new("store-open");
        for id in ["", "..", "../x", "a\\b", "C:x"] {
            assert!(RunStore::open(&dir.path("runs"), id).is_err(), "{id}");
        }
    }

    #[test]
    fn the_latest_result_per_path_wins() {
        let dir = TempDir::new("store-results");
        let store = RunStore::create(&dir.path("runs")).unwrap();
        let mut writer = store.results_writer().unwrap();
        writer
            .write(&ItemResult::not_copied(
                "a.txt",
                ItemKind::File,
                4,
                FailureReason::FileLocked,
                None,
                "in use",
            ))
            .unwrap();
        writer
            .write(&ItemResult::done("b.txt", 2, ItemStatus::Copied))
            .unwrap();
        writer
            .write(&ItemResult::done("a.txt", 4, ItemStatus::Copied))
            .unwrap();
        writer.finish().unwrap();

        let latest = store.latest_results().unwrap();
        assert_eq!(latest.len(), 2);
        assert_eq!(latest["a.txt"].status, ItemStatus::Copied);
        let totals = totals_from(&RunTotals::default(), latest.values());
        assert_eq!(
            (totals.copied, totals.copied_bytes, totals.not_copied),
            (2, 6, 0)
        );
    }
}
