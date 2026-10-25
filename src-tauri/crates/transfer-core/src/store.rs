//! The files of one run folder, and the list of runs.

use crate::{
    now_ms, ItemKind, ItemResult, ItemStatus, Result, RunSummary, RunTotals, TransferError,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
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
    Warning,
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
        read_jsonl(self, &self.dir.join(MANIFEST_FILE))
    }

    /// Appends to `results.jsonl`, flushing each row so a crash loses at most the file in flight.
    pub fn results_writer(&self) -> Result<JsonlWriter> {
        JsonlWriter::create(&self.dir.join(RESULTS_FILE), true)
    }

    /// The latest outcome per relative path, every row kept. Only for callers that need whole
    /// rows for everything; the others below hold a fraction of this.
    pub fn latest_results(&self) -> Result<BTreeMap<String, ItemResult>> {
        self.latest_results_where(|_| true)
    }

    /// The latest outcome per relative path, keeping only the rows `keep` accepts. The first
    /// pass remembers where each path's last row is (16 bytes of key and a row number per path),
    /// the second keeps the wanted ones, so memory follows the selection, not the run.
    pub fn latest_results_where(
        &self,
        keep: impl Fn(&ItemResult) -> bool,
    ) -> Result<BTreeMap<String, ItemResult>> {
        let mut latest = BTreeMap::new();
        let Some(rows) = self.result_rows()? else {
            return Ok(latest);
        };
        let mut last: HashMap<PathKey, u64> = HashMap::new();
        for (number, row) in (0_u64..).zip(rows) {
            last.insert(PathKey::of(&row?.relative_path), number);
        }
        let Some(rows) = self.result_rows()? else {
            return Ok(latest);
        };
        for (number, row) in (0_u64..).zip(rows) {
            let row = row?;
            if last.get(&PathKey::of(&row.relative_path)) == Some(&number) && keep(&row) {
                latest.insert(row.relative_path.clone(), row);
            }
        }
        Ok(latest)
    }

    /// Fingerprints of every path that has a result row.
    pub fn result_keys(&self) -> Result<HashSet<PathKey>> {
        let mut keys = HashSet::new();
        if let Some(rows) = self.result_rows()? {
            for row in rows {
                keys.insert(PathKey::of(&row?.relative_path));
            }
        }
        Ok(keys)
    }

    /// Totals over the latest outcome per path: counts from `planned`, the rest from the results.
    /// Streams the file and keeps 32 bytes per path instead of the rows.
    pub fn latest_totals(&self, planned: &RunTotals) -> Result<RunTotals> {
        let mut latest: HashMap<PathKey, Counted> = HashMap::new();
        if let Some(rows) = self.result_rows()? {
            for row in rows {
                let row = row?;
                latest.insert(PathKey::of(&row.relative_path), Counted::of(&row));
            }
        }
        let mut tally = Tally::new(planned);
        for counted in latest.values() {
            tally.add(counted.kind, counted.status, counted.size);
        }
        Ok(tally.totals)
    }

    /// The rows of `results.jsonl` in the order written, or `None` before the file exists.
    fn result_rows(&self) -> Result<Option<impl Iterator<Item = Result<ItemResult>>>> {
        let path = self.dir.join(RESULTS_FILE);
        if !path.is_file() {
            return Ok(None);
        }
        read_jsonl::<ItemResult>(self, &path).map(Some)
    }

    /// Recounts copied/skipped/not-copied from the results; planned counts are kept.
    pub fn refresh_totals(&self, summary: &mut RunSummary) -> Result<()> {
        summary.totals = self.latest_totals(&summary.totals)?;
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

/// A fingerprint of a relative path, 128 bits of its BLAKE3 hash: enough to tell paths apart
/// without keeping them. Collisions are not a practical concern at this size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathKey([u8; 16]);

impl PathKey {
    pub fn of(path: &str) -> Self {
        let hash = blake3::hash(path.as_bytes());
        let mut key = [0; 16];
        key.copy_from_slice(&hash.as_bytes()[..16]);
        Self(key)
    }
}

/// What the totals need from a result row.
#[derive(Clone, Copy)]
struct Counted {
    kind: ItemKind,
    status: ItemStatus,
    size: u64,
}

impl Counted {
    fn of(item: &ItemResult) -> Self {
        Self {
            kind: item.kind,
            status: item.status,
            size: item.size,
        }
    }
}

struct Tally {
    totals: RunTotals,
}

impl Tally {
    fn new(planned: &RunTotals) -> Self {
        Self {
            totals: RunTotals {
                planned_files: planned.planned_files,
                planned_bytes: planned.planned_bytes,
                folders: planned.folders,
                excluded: planned.excluded,
                changed: planned.changed,
                new: planned.new,
                retried: planned.retried,
                unchanged: planned.unchanged,
                changed_at_destination: planned.changed_at_destination,
                destination_only: planned.destination_only,
                ..RunTotals::default()
            },
        }
    }

    fn add(&mut self, kind: ItemKind, status: ItemStatus, size: u64) {
        let totals = &mut self.totals;
        // Folders count only when they could not be copied (an unreadable folder).
        match status {
            ItemStatus::Copied | ItemStatus::Arrived if kind == ItemKind::File => {
                totals.copied += 1;
                totals.copied_bytes += size;
            }
            ItemStatus::SkippedIdentical if kind == ItemKind::File => totals.skipped_identical += 1,
            ItemStatus::Copied | ItemStatus::Arrived | ItemStatus::SkippedIdentical => {}
            ItemStatus::NotCopied => {
                totals.not_copied += 1;
                totals.not_copied_bytes += size;
            }
        }
    }
}

/// Planned counts from `planned`, outcome counts from `results`.
pub fn totals_from<'a>(
    planned: &RunTotals,
    results: impl Iterator<Item = &'a ItemResult>,
) -> RunTotals {
    let mut tally = Tally::new(planned);
    for item in results {
        tally.add(item.kind, item.status, item.size);
    }
    tally.totals
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
            drop_torn_tail(path);
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

/// Rows of a `.jsonl` file. A line that does not parse is an error, except when it is the last
/// non-empty line: that is a write torn by a crash, so it is skipped and noted in the audit log.
fn read_jsonl<T: DeserializeOwned>(
    store: &RunStore,
    path: &Path,
) -> Result<impl Iterator<Item = Result<T>>> {
    let mut lines = BufReader::new(File::open(path)?).lines().peekable();
    let store = store.clone();
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(std::iter::from_fn(move || loop {
        let line = match lines.next()? {
            Ok(line) => line,
            Err(error) => return Some(Err(error.into())),
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(&line) {
            Ok(row) => return Some(Ok(row)),
            Err(error) => {
                // Blank lines after it do not make it a middle line.
                while matches!(lines.peek(), Some(Ok(next)) if next.trim().is_empty()) {
                    lines.next();
                }
                if lines.peek().is_some() {
                    return Some(Err(TransferError::BadRun(format!(
                        "Bad row in run file: {error}"
                    ))));
                }
                store.audit(
                    "read-run-file",
                    LogStatus::Warning,
                    format!("Ignored an incomplete last row in {file}"),
                    &[("file", file.clone().into())],
                );
                return None;
            }
        }
    }))
}

/// Before appending, cuts off a last row that a crash left half written, so the new rows do not
/// turn it into a bad row in the middle. A complete row that only lacks its newline is kept.
fn drop_torn_tail(path: &Path) {
    use std::io::{Read, Seek, SeekFrom};
    const TAIL: u64 = 1 << 20;
    let Ok(mut file) = OpenOptions::new().read(true).write(true).open(path) else {
        return;
    };
    let Ok(len) = file.metadata().map(|meta| meta.len()) else {
        return;
    };
    let start = len.saturating_sub(TAIL);
    let mut tail = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut tail).is_err() {
        return;
    }
    if tail.last().is_none_or(|&byte| byte == b'\n') {
        return;
    }
    let row_start = tail
        .iter()
        .rposition(|&byte| byte == b'\n')
        .map(|at| at + 1)
        .or((start == 0).then_some(0));
    match row_start {
        Some(at) if serde_json::from_slice::<serde_json::Value>(&tail[at..]).is_err() => {
            let _ = file.set_len(start + at as u64);
        }
        _ => {
            let _ = file
                .seek(SeekFrom::End(0))
                .and_then(|_| file.write_all(b"\n"));
        }
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map_err(|error| TransferError::BadRun(format!("{}: {error}", path.display())))
}

/// Writes `bytes` to a `.tmp` file next to `path`, flushes it to disk and renames it over
/// `path`, so a crash or power loss never leaves a half-written or empty file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Written to a `.tmp` file and renamed, so a crash never leaves half a file.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    write_atomic(path, text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use crate::FailureReason;

    #[test]
    fn write_atomic_replaces_the_file_and_leaves_no_tmp_behind() {
        let dir = TempDir::new("store-atomic");
        let path = dir.path("out.json");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert!(!dir.path("out.json.tmp").exists());
    }

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
            previous_run_id: None,
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

    fn good_rows(store: &RunStore) {
        let mut writer = store.results_writer().unwrap();
        writer
            .write(&ItemResult::done("a.txt", 1, ItemStatus::Copied))
            .unwrap();
        writer
            .write(&ItemResult::done("b.txt", 2, ItemStatus::Copied))
            .unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn a_torn_last_row_is_skipped_and_noted_in_the_audit_log() {
        let dir = TempDir::new("store-torn");
        let store = RunStore::create(&dir.path("runs")).unwrap();
        good_rows(&store);
        let path = store.dir().join(RESULTS_FILE);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(br#"{"relativePath":"c.txt","sta"#).unwrap();
        drop(file);

        assert_eq!(store.latest_results().unwrap().len(), 2);
        let audit = fs::read_to_string(store.dir().join(AUDIT_FILE)).unwrap();
        assert!(audit.contains("warning"), "{audit}");

        // Appending cuts the torn row off, so it never becomes a bad row in the middle.
        let mut writer = store.results_writer().unwrap();
        writer
            .write(&ItemResult::done("d.txt", 4, ItemStatus::Copied))
            .unwrap();
        writer.finish().unwrap();
        assert_eq!(store.latest_results().unwrap().len(), 3);
    }

    #[test]
    fn a_bad_row_in_the_middle_is_still_an_error() {
        let dir = TempDir::new("store-bad-middle");
        let store = RunStore::create(&dir.path("runs")).unwrap();
        good_rows(&store);
        let path = store.dir().join(RESULTS_FILE);
        let text = fs::read_to_string(&path).unwrap();
        let (first, rest) = text.split_once('\n').unwrap();
        fs::write(&path, format!("{first}\n{{\"torn\n{rest}")).unwrap();

        assert!(store.latest_results().is_err());
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

    const BENCH_ROWS: u64 = 1_000_000;

    /// Writes `BENCH_ROWS` rows (a few repeated, as after a retry) straight to disk, so building
    /// the file does not show up in the measurements.
    fn bench_store(dir: &TempDir) -> RunStore {
        let store = RunStore::create(&dir.path("runs")).unwrap();
        let mut file = BufWriter::new(File::create(store.dir().join(RESULTS_FILE)).unwrap());
        for n in 0..BENCH_ROWS {
            let path = format!("Projects/client-{}/drawings/sheet-{n:07}.dwg", n % 500);
            let mut row = if n % 50 == 0 {
                ItemResult::not_copied(
                    path,
                    ItemKind::File,
                    n,
                    FailureReason::FileLocked,
                    None,
                    "The file is open in another program.",
                )
            } else {
                ItemResult::done(path, n, ItemStatus::Copied)
            };
            row.source_hash = Some(format!("{n:064x}"));
            row.dest_hash = row.source_hash.clone();
            serde_json::to_writer(&mut file, &row).unwrap();
            file.write_all(b"\n").unwrap();
            if n % 100 == 20 {
                // A retry that finished an earlier failure (every other failed row).
                let earlier = n - 20;
                let fixed = ItemResult::done(
                    format!(
                        "Projects/client-{}/drawings/sheet-{earlier:07}.dwg",
                        earlier % 500
                    ),
                    earlier,
                    ItemStatus::Copied,
                );
                serde_json::to_writer(&mut file, &fixed).unwrap();
                file.write_all(b"\n").unwrap();
            }
        }
        file.flush().unwrap();
        store
    }

    /// Peak resident memory of this process in MB, where Linux reports it.
    fn peak_mb() -> Option<f64> {
        let status = fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
        let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb / 1024.0)
    }

    fn report(name: &str, started: std::time::Instant, base: Option<f64>, totals: &RunTotals) {
        let peak = peak_mb().zip(base).map(|(peak, base)| peak - base);
        println!(
            "{name}: {BENCH_ROWS} rows, {:?}, peak +{:.0} MB, copied {} not copied {}",
            started.elapsed(),
            peak.unwrap_or(f64::NAN),
            totals.copied,
            totals.not_copied
        );
    }

    /// Run each benchmark alone so the peak is its own:
    /// `cargo test -p transfer-core --release totals_benchmark_map -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark: writes a 1M-row results file"]
    fn totals_benchmark_map() {
        let dir = TempDir::new("store-bench-map");
        let store = bench_store(&dir);
        let base = peak_mb();
        let started = std::time::Instant::now();
        let totals = totals_from(
            &RunTotals::default(),
            store.latest_results().unwrap().values(),
        );
        report("latest_results map", started, base, &totals);
    }

    fn mixed_rows(store: &RunStore) {
        let locked = |path: &str, size| {
            ItemResult::not_copied(
                path,
                ItemKind::File,
                size,
                FailureReason::FileLocked,
                None,
                "in use",
            )
        };
        let mut writer = store.results_writer().unwrap();
        for row in [
            locked("a.txt", 4),
            ItemResult::done("b.txt", 2, ItemStatus::Copied),
            ItemResult::done("c.txt", 8, ItemStatus::SkippedIdentical),
            locked("d.txt", 16),
            ItemResult::not_copied(
                "Unreadable",
                ItemKind::Folder,
                0,
                FailureReason::AccessDenied,
                None,
                "no",
            ),
            ItemResult::done("Fine", 0, ItemStatus::Copied),
            // Later rows win: a.txt was fixed by a retry, b.txt then failed.
            ItemResult::done("a.txt", 4, ItemStatus::Copied),
            locked("b.txt", 2),
            ItemResult::done("e.txt", 32, ItemStatus::Arrived),
        ] {
            writer.write(&row).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn streamed_totals_match_the_totals_of_the_latest_rows() {
        let dir = TempDir::new("store-totals");
        let store = RunStore::create(&dir.path("runs")).unwrap();
        mixed_rows(&store);
        let planned = RunTotals {
            planned_files: 7,
            planned_bytes: 99,
            folders: 2,
            excluded: 1,
            ..RunTotals::default()
        };

        let latest = store.latest_results().unwrap();
        assert_eq!(latest.len(), 7);
        let expected = totals_from(&planned, latest.values());
        assert_eq!(store.latest_totals(&planned).unwrap(), expected);
        assert_eq!(
            (expected.copied, expected.copied_bytes, expected.not_copied),
            (3, 36, 3)
        );
        assert_eq!((expected.planned_files, expected.excluded), (7, 1));
    }

    #[test]
    fn a_run_saved_before_delta_sync_loads_with_no_chain_and_zero_delta_counts() {
        let dir = TempDir::new("store-old-run");
        let root = dir.path("runs");
        let id = stored_run(&root, 1_000);
        // run.json as 1.2.0 wrote it: no previousRunId, none of the delta counters.
        let path = root.join(&id).join(SUMMARY_FILE);
        let mut json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("previousRunId");
        let totals = json["totals"].as_object_mut().unwrap();
        for key in [
            "changed",
            "new",
            "retried",
            "unchanged",
            "changedAtDestination",
            "destinationOnly",
        ] {
            totals.remove(key);
        }
        fs::write(&path, json.to_string()).unwrap();

        let summary = RunStore::open(&root, &id).unwrap().summary().unwrap();
        assert_eq!(summary.previous_run_id, None);
        assert_eq!(summary.totals, RunTotals::default());
        assert_eq!(list_runs(&root).len(), 1);
    }

    #[test]
    fn the_chain_and_delta_counts_survive_a_save_and_recounting_the_results() {
        let dir = TempDir::new("store-delta-fields");
        let store = RunStore::create(&dir.path("runs")).unwrap();
        mixed_rows(&store);
        let planned = RunTotals {
            planned_files: 7,
            changed: 1,
            new: 2,
            retried: 3,
            unchanged: 4,
            changed_at_destination: 5,
            destination_only: 6,
            ..RunTotals::default()
        };
        let mut summary = RunSummary {
            previous_run_id: Some("20261021-180200-000".to_owned()),
            totals: planned.clone(),
            ..RunStore::open(&dir.path("runs"), &stored_run(&dir.path("runs"), 5))
                .unwrap()
                .summary()
                .unwrap()
        };
        store.refresh_totals(&mut summary).unwrap();
        assert_eq!(
            (
                summary.totals.changed,
                summary.totals.new,
                summary.totals.retried,
                summary.totals.unchanged,
                summary.totals.changed_at_destination,
                summary.totals.destination_only
            ),
            (1, 2, 3, 4, 5, 6),
            "the copy outcomes are recounted, the plan is kept"
        );
        assert_eq!(summary.totals.copied, 3);

        store.save_summary(&summary).unwrap();
        let back = store.summary().unwrap();
        assert_eq!(back.previous_run_id.as_deref(), Some("20261021-180200-000"));
        assert_eq!(back.totals, summary.totals);
    }

    #[test]
    fn selecting_rows_never_resurrects_a_path_whose_latest_row_does_not_match() {
        let dir = TempDir::new("store-where");
        let store = RunStore::create(&dir.path("runs")).unwrap();
        mixed_rows(&store);

        let not_copied = store
            .latest_results_where(|row| row.status == ItemStatus::NotCopied)
            .unwrap();
        // a.txt failed first, but its latest row is a success.
        let paths: Vec<&str> = not_copied.keys().map(String::as_str).collect();
        assert_eq!(paths, ["Unreadable", "b.txt", "d.txt"]);
        let everything: Vec<ItemResult> = store
            .latest_results()
            .unwrap()
            .into_values()
            .filter(|row| row.status == ItemStatus::NotCopied)
            .collect();
        assert_eq!(not_copied.into_values().collect::<Vec<_>>(), everything);
    }

    #[test]
    fn result_keys_cover_every_path_once_and_none_before_the_file_exists() {
        let dir = TempDir::new("store-keys");
        let store = RunStore::create(&dir.path("runs")).unwrap();
        assert!(store.result_keys().unwrap().is_empty());
        assert_eq!(
            store.latest_totals(&RunTotals::default()).unwrap(),
            RunTotals::default()
        );
        mixed_rows(&store);
        let keys = store.result_keys().unwrap();
        assert_eq!(keys.len(), 7);
        assert!(keys.contains(&PathKey::of("a.txt")));
        assert!(!keys.contains(&PathKey::of("A.TXT")));
    }

    /// `cargo test -p transfer-core --release totals_benchmark_stream -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark: writes a 1M-row results file"]
    fn totals_benchmark_stream() {
        let dir = TempDir::new("store-bench-stream");
        let store = bench_store(&dir);
        let base = peak_mb();
        let started = std::time::Instant::now();
        let totals = store.latest_totals(&RunTotals::default()).unwrap();
        report("latest_totals stream", started, base, &totals);
    }
}
