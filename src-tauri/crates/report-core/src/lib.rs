//! Saved reports: Compare, Migration readiness and Disk Cleanup results are saved as JSON files here; Record runs
//! are already saved by `transfer-core` and are listed and exported alongside them.
//!
//! Every report becomes a [`Document`] (title, summary tiles, tables), which is what the HTML
//! and CSV exports are made from.

pub mod applog;
pub mod branding;
mod doc;
pub mod explorer;
mod header;
mod kinds;
mod recovery;

pub use doc::{Branding, Document, Table, Tile, Tone, JOB_LABELS};
pub use explorer::Explorer;
pub use kinds::{
    CleanReport, CleanedItem, CompareReport, FileItem, FolderItem, MissingFile, Removed, TypeItem,
    UsageReport,
};
pub use migrate_core::ReadinessReport;

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use transfer_core::store::{list_runs, RunStore};
use transfer_core::{format_utc, machine_name, now_ms, user_name};

/// Overrides where reports are saved (tests, portable layouts).
pub const REPORTS_DIR_ENV: &str = "DEEP_SERVER_ANALYZER_REPORTS_DIR";
/// Overrides where the deletion log is kept (tests, portable layouts).
pub const LOGS_DIR_ENV: &str = "DEEP_SERVER_ANALYZER_LOGS_DIR";
const JOB_FILE: &str = "job.json";

/// `%LOCALAPPDATA%\Deep Server Analyzer\Reports`, next to the Records folder.
pub fn reports_root() -> PathBuf {
    if let Some(dir) = std::env::var_os(REPORTS_DIR_ENV).filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
    transfer_core::records_root()
        .parent()
        .map(|parent| parent.join("Reports"))
        .unwrap_or_else(|| PathBuf::from("Reports"))
}

/// Where the deletion log lives: `Logs` next to `Reports` (`LOGS_DIR_ENV` overrides it).
pub fn logs_root() -> PathBuf {
    if let Some(dir) = std::env::var_os(LOGS_DIR_ENV).filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
    reports_root()
        .parent()
        .map(|parent| parent.join("Logs"))
        .unwrap_or_else(|| PathBuf::from("Logs"))
}

/// Who the work was for, typed on the Compare, Record and Disk Cleanup pages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobInfo {
    pub client: String,
    pub ticket: String,
    pub technician: String,
}

impl JobInfo {
    pub fn is_empty(&self) -> bool {
        self.client.trim().is_empty()
            && self.ticket.trim().is_empty()
            && self.technician.trim().is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReportKind {
    Compare,
    Record,
    DiskUsage,
    Cleanup,
    Readiness,
}

/// A saved Compare or Disk Cleanup report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub machine: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub job: JobInfo,
    pub body: Body,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "data")]
pub enum Body {
    Compare(CompareReport),
    DiskUsage(UsageReport),
    Cleanup(CleanReport),
    Readiness(ReadinessReport),
}

impl Body {
    pub fn kind(&self) -> ReportKind {
        match self {
            Body::Compare(_) => ReportKind::Compare,
            Body::DiskUsage(_) => ReportKind::DiskUsage,
            Body::Cleanup(_) => ReportKind::Cleanup,
            Body::Readiness(_) => ReportKind::Readiness,
        }
    }
}

/// One row on the Reports page.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub kind: ReportKind,
    pub created_at_ms: u64,
    /// "Compare", "Copy for me", "Disk usage", "Quick cleanup", "Migration readiness".
    pub title: String,
    /// What it was about: the folders or the drive.
    pub subject: String,
    /// One line with the result: "3 files missing (1.2 GB)", "Freed 4.1 GB".
    pub headline: String,
    /// Something went wrong or is missing; the row shows in red.
    pub problem: bool,
    pub job: JobInfo,
}

/// Saves a new report and returns it with its id.
pub fn save(root: &Path, job: JobInfo, body: Body) -> Result<Saved, String> {
    fs::create_dir_all(root).map_err(|err| err.to_string())?;
    let created_at_ms = now_ms();
    let stamp: String = format_utc(created_at_ms)
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let prefix = match body.kind() {
        ReportKind::Compare => "compare",
        ReportKind::DiskUsage => "usage",
        ReportKind::Readiness => "readiness",
        ReportKind::Cleanup | ReportKind::Record => "cleanup",
    };
    let base = format!("{prefix}-{}-{:03}", stamp, created_at_ms % 1000);
    let mut id = base.clone();
    for attempt in 1.. {
        if !root.join(format!("{id}.json")).exists() {
            break;
        }
        id = format!("{base}-{attempt}");
    }
    let saved = Saved {
        id,
        created_at_ms,
        machine: machine_name(),
        user: user_name(),
        job,
        body,
    };
    write(root, &saved)?;
    Ok(saved)
}

/// Replaces a saved report (Disk Cleanup adds removed items to its scan report).
pub fn write(root: &Path, saved: &Saved) -> Result<(), String> {
    let path = report_path(root, &saved.id)?;
    // A disk usage report with an explorer is a few MB; skip the indentation for those.
    let big = matches!(&saved.body, Body::DiskUsage(report) if report.explorer.is_some());
    let json = if big {
        serde_json::to_vec(saved)
    } else {
        serde_json::to_vec_pretty(saved)
    }
    .map_err(|err| err.to_string())?;
    transfer_core::store::write_atomic(&path, &json)
        .map_err(|err| format!("Could not save the report: {err}"))
}

pub fn load(root: &Path, id: &str) -> Result<Saved, String> {
    let text = fs::read(report_path(root, id)?)
        .map_err(|_| "That report is no longer there.".to_string())?;
    let mut saved: Saved = serde_json::from_slice(&text)
        .map_err(|err| format!("The report could not be read: {err}"))?;
    if saved.id.is_empty() {
        saved.id = id.to_string();
    }
    Ok(saved)
}

fn report_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || id.contains(['/', '\\', ':', '.']) {
        return Err(format!("Not a report id: {id}"));
    }
    Ok(root.join(format!("{id}.json")))
}

/// Saves the job details next to a record run.
pub fn save_record_job(records: &Path, run_id: &str, job: &JobInfo) -> Result<(), String> {
    let store = RunStore::open(records, run_id).map_err(|err| err.to_string())?;
    let json = serde_json::to_vec_pretty(job).map_err(|err| err.to_string())?;
    fs::write(store.dir().join(JOB_FILE), json).map_err(|err| err.to_string())
}

pub fn record_job(store: &RunStore) -> JobInfo {
    fs::read(store.dir().join(JOB_FILE))
        .ok()
        .and_then(|text| serde_json::from_slice(&text).ok())
        .unwrap_or_default()
}

/// Every report and record, newest first.
pub fn list(reports: &Path, records: &Path) -> Vec<Entry> {
    let mut entries: Vec<Entry> = fs::read_dir(reports)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".json")?;
            header::read(&report_path(reports, id).ok()?, id).ok()
        })
        .map(kinds::header_entry)
        .collect();
    for run in list_runs(records) {
        let job = RunStore::open(records, &run.id)
            .map(|store| record_job(&store))
            .unwrap_or_default();
        entries.push(kinds::record_entry(&run, job));
    }
    entries.sort_by(|a, b| {
        b.created_at_ms
            .cmp(&a.created_at_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    entries
}

/// Deletes a report, or a record run with all its files.
pub fn delete(reports: &Path, records: &Path, kind: ReportKind, id: &str) -> Result<(), String> {
    match kind {
        ReportKind::Record => {
            let store = RunStore::open(records, id).map_err(|err| err.to_string())?;
            fs::remove_dir_all(store.dir()).map_err(|err| format!("Could not delete it: {err}"))
        }
        _ => fs::remove_file(report_path(reports, id)?)
            .map_err(|err| format!("Could not delete it: {err}")),
    }
}

/// Deletes saved reports (Compare, Disk usage, Quick cleanup) last written more than `days` days
/// before `now_ms` and returns how many went. Record runs live in their own folder and are never
/// touched: they are client proof. `days == 0` keeps everything.
pub fn prune(reports: &Path, days: u32, now_ms: u64) -> u64 {
    if days == 0 {
        return 0;
    }
    let cutoff = now_ms.saturating_sub(u64::from(days) * 24 * 60 * 60 * 1000);
    let mut removed = 0;
    for entry in fs::read_dir(reports).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let written = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|age| age.as_millis() as u64);
        if written.is_some_and(|ms| ms < cutoff) && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The report as a document, ready for [`Document::html`] or [`Document::csv`].
pub fn document(
    reports: &Path,
    records: &Path,
    kind: ReportKind,
    id: &str,
) -> Result<Document, String> {
    match kind {
        ReportKind::Record => kinds::record_document(records, id),
        _ => Ok(kinds::document(&load(reports, id)?)),
    }
}

/// Size as Explorer shows it, matching the app: "1.25 GB", "940 KB", "12 bytes".
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["bytes", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} {}", if bytes == 1 { "byte" } else { "bytes" });
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let digits = if value >= 100.0 {
        0
    } else if value >= 10.0 {
        1
    } else {
        2
    };
    format!("{value:.digits$} {}", UNITS[unit])
}

/// "1,234,567".
pub fn format_count(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// "1 file", "2,048 files".
pub fn plural(count: u64, one: &str) -> String {
    format!(
        "{} {one}{}",
        format_count(count),
        if count == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_with_only_a_body_loads_and_takes_its_id_from_the_file_name() {
        let temp = tempfile::tempdir().unwrap();
        let body = serde_json::to_value(usage()).unwrap();
        let text = serde_json::json!({ "body": body }).to_string();
        fs::write(temp.path().join("old-report.json"), text).unwrap();
        let saved = load(temp.path(), "old-report").unwrap();
        assert_eq!(saved.id, "old-report");
        assert_eq!(saved.created_at_ms, 0);
        assert!(saved.machine.is_empty());
        assert!(saved.job.is_empty());
    }

    fn usage() -> Body {
        Body::DiskUsage(UsageReport {
            path: r"C:\".into(),
            size: 2048,
            ..UsageReport::default()
        })
    }

    #[test]
    fn list_rows_match_the_rows_built_from_fully_loaded_reports() {
        let temp = tempfile::tempdir().unwrap();
        let reports = temp.path().join("Reports");
        let job = JobInfo {
            client: "Acme".into(),
            ticket: "T-1".into(),
            technician: "Pat".into(),
        };
        let compare = Body::Compare(CompareReport {
            source: r"D:\a".into(),
            destination: r"E:\a".into(),
            missing: 2,
            missing_bytes: 3 << 20,
            missing_paths: vec![MissingFile {
                path: "x.txt".into(),
                size: 5,
            }],
            ..CompareReport::default()
        });
        let usage = Body::DiskUsage(UsageReport {
            path: r"C:\".into(),
            size: 9 << 30,
            removed: vec![
                Removed {
                    path: r"C:\old.iso".into(),
                    size: 4 << 20,
                    permanent: false,
                },
                Removed {
                    path: r"C:\old2.iso".into(),
                    size: 1 << 20,
                    permanent: true,
                },
            ],
            explorer: Some(crate::explorer::Explorer::default()),
            ..UsageReport::default()
        });
        let clean = Body::Cleanup(CleanReport {
            items: vec![
                CleanedItem {
                    title: "Temp files".into(),
                    freed: 1000,
                    ..CleanedItem::default()
                },
                CleanedItem {
                    title: "Browser caches".into(),
                    freed: 2000,
                    ..CleanedItem::default()
                },
            ],
        });
        let readiness = Body::Readiness(ReadinessReport {
            source: r"F:\Finance".into(),
            destination_root: r"\\nas\finance".into(),
            name_problems: 2,
            links: 1,
            problems: vec![migrate_core::Problem {
                path: "CON.txt".into(),
                rule: "reserved".into(),
                blocker: true,
                ..migrate_core::Problem::default()
            }],
            ..ReadinessReport::default()
        });
        let mut ids = Vec::new();
        for body in [compare, usage, clean, readiness] {
            ids.push(save(&reports, job.clone(), body).unwrap().id);
        }
        // An old report with only a body takes its id from the file name.
        let old = serde_json::json!({ "body": serde_json::to_value(self::usage()).unwrap() });
        fs::write(reports.join("old-report.json"), old.to_string()).unwrap();
        ids.push("old-report".into());
        fs::write(reports.join("broken.json"), "{ not json").unwrap();
        // A kind this build does not know (what a reverted build sees for a readiness report) is
        // left out of the list, and the rest still lists.
        let unknown = r#"{"id":"x","createdAtMs":1,"body":{"kind":"hologram","data":{}}}"#;
        fs::write(reports.join("future-1.json"), unknown).unwrap();

        let listed = list(&reports, &temp.path().join("Records"));
        assert_eq!(listed.len(), ids.len());
        for id in ids {
            let expected = kinds::entry(&load(&reports, &id).unwrap());
            let row = listed.iter().find(|row| row.id == id).unwrap();
            assert_eq!(row, &expected);
        }
    }

    /// `cargo test -p report-core --release list_benchmark_twenty -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark: writes 100 MB of reports"]
    fn list_benchmark_twenty_reports_of_five_megabytes() {
        let temp = tempfile::tempdir().unwrap();
        let reports = temp.path().join("Reports");
        let nodes: Vec<crate::explorer::ExplorerNode> = (0..90_000u32)
            .map(|n| crate::explorer::ExplorerNode {
                name: format!("folder-{n}.bin"),
                disk: u64::from(n) * 4096,
                files: 1,
                kind: crate::explorer::KIND_FILE,
                parent: n / 8,
            })
            .collect();
        for n in 0..20 {
            let body = Body::DiskUsage(UsageReport {
                path: format!(r"D:\Share{n}"),
                size: 2048 + n,
                explorer: Some(crate::explorer::Explorer {
                    nodes: nodes.clone(),
                    ..Default::default()
                }),
                ..UsageReport::default()
            });
            save(&reports, JobInfo::default(), body).unwrap();
        }
        let bytes: u64 = fs::read_dir(&reports)
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum();
        let records = temp.path().join("Records");
        let started = std::time::Instant::now();
        let rows = list(&reports, &records);
        let elapsed = started.elapsed();
        assert_eq!(rows.len(), 20);
        println!(
            "list: 20 reports, {:.1} MB total, {:?}",
            bytes as f64 / 1e6,
            elapsed
        );
    }

    #[test]
    fn prune_removes_only_old_report_files_and_keeps_zero_as_forever() {
        let temp = tempfile::tempdir().unwrap();
        let reports = temp.path().join("Reports");
        let saved = save(&reports, JobInfo::default(), usage()).unwrap();
        fs::write(reports.join("notes.txt"), "keep").unwrap();
        let day = 24 * 60 * 60 * 1000;

        assert_eq!(prune(&reports, 0, now_ms() + 400 * day), 0);
        assert_eq!(prune(&reports, 30, now_ms()), 0, "new files stay");
        assert_eq!(prune(&reports, 30, now_ms() + 31 * day), 1);
        assert!(load(&reports, &saved.id).is_err());
        assert!(reports.join("notes.txt").exists());
        assert_eq!(prune(&temp.path().join("missing"), 30, now_ms()), 0);
    }

    #[test]
    fn saves_lists_loads_and_deletes() {
        let temp = tempfile::tempdir().unwrap();
        let reports = temp.path().join("Reports");
        let records = temp.path().join("Records");
        let job = JobInfo {
            client: "Acme".into(),
            ..JobInfo::default()
        };
        let first = save(&reports, job.clone(), usage()).unwrap();
        let second = save(&reports, JobInfo::default(), usage()).unwrap();
        assert_ne!(first.id, second.id);
        assert!(first.id.starts_with("usage-"));

        let entries = list(&reports, &records);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].job, job);
        assert_eq!(entries[0].kind, ReportKind::DiskUsage);
        assert_eq!(load(&reports, &first.id).unwrap(), first);

        delete(&reports, &records, ReportKind::DiskUsage, &first.id).unwrap();
        assert_eq!(list(&reports, &records).len(), 1);
        assert!(load(&reports, &first.id).is_err());
    }

    #[test]
    fn records_are_listed_with_their_job_and_exported() {
        use transfer_core::{
            ConflictPolicy, RunState, RunSummary, RunTotals, TransferMode, TransferSettings,
            VerifyLevel,
        };
        let temp = tempfile::tempdir().unwrap();
        let records = temp.path().join("Records");
        let store = RunStore::create(&records).unwrap();
        store
            .save_summary(&RunSummary {
                id: store.id(),
                settings: TransferSettings {
                    source: r"C:\src".into(),
                    destination: r"D:\dst".into(),
                    mode: TransferMode::Watch,
                    verify: VerifyLevel::SizeAndTime,
                    conflict: ConflictPolicy::Skip,
                    ignore_junk: false,
                    download_cloud: false,
                    preserve: Default::default(),
                },
                state: RunState::Completed,
                created_at_ms: 1,
                started_at_ms: None,
                finished_at_ms: None,
                machine: "PC1".into(),
                user: "sam".into(),
                totals: RunTotals {
                    planned_files: 4,
                    not_copied: 1,
                    not_copied_bytes: 10,
                    ..RunTotals::default()
                },
                error: None,
            })
            .unwrap();
        let job = JobInfo {
            client: "Acme".into(),
            ticket: "T-9".into(),
            technician: "Sam".into(),
        };
        save_record_job(&records, &store.id(), &job).unwrap();

        let entries = list(&temp.path().join("Reports"), &records);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, ReportKind::Record);
        assert_eq!(entries[0].title, "Watched copy");
        assert_eq!(entries[0].headline, "1 file not copied (10 bytes)");
        assert!(entries[0].problem);
        assert_eq!(entries[0].job, job);

        let doc = document(temp.path(), &records, ReportKind::Record, &store.id()).unwrap();
        assert!(doc.meta.contains(&("Client".into(), "Acme".into())));
        // The labels the report kinds use are the ones the "Prepared for" block picks out.
        assert_eq!(
            doc.job_fields(),
            vec![("Client", "Acme"), ("Ticket", "T-9"), ("Technician", "Sam")]
        );
        assert_eq!(doc.tiles[1].label, "Arrived");

        delete(temp.path(), &records, ReportKind::Record, &store.id()).unwrap();
        assert!(!store.dir().exists());
    }

    #[test]
    fn record_csv_carries_raw_bytes_for_every_file() {
        use transfer_core::{
            ConflictPolicy, ItemKind, ItemResult, ItemStatus, RunState, RunSummary, RunTotals,
            TransferMode, TransferSettings, VerifyLevel,
        };
        let temp = tempfile::tempdir().unwrap();
        let records = temp.path().join("Records");
        let store = RunStore::create(&records).unwrap();
        store
            .save_summary(&RunSummary {
                id: store.id(),
                settings: TransferSettings {
                    source: r"C:\src".into(),
                    destination: r"D:\dst".into(),
                    mode: TransferMode::Watch,
                    verify: VerifyLevel::SizeAndTime,
                    conflict: ConflictPolicy::Skip,
                    ignore_junk: false,
                    download_cloud: false,
                    preserve: Default::default(),
                },
                state: RunState::Completed,
                created_at_ms: 1,
                started_at_ms: None,
                finished_at_ms: None,
                machine: "PC1".into(),
                user: "sam".into(),
                totals: RunTotals {
                    planned_files: 2,
                    not_copied: 1,
                    not_copied_bytes: 77,
                    ..RunTotals::default()
                },
                error: None,
            })
            .unwrap();
        let item = |path: &str, size: u64, status: ItemStatus| ItemResult {
            relative_path: path.into(),
            kind: ItemKind::File,
            size,
            status,
            reason: None,
            side: None,
            os_code: None,
            message: None,
            source_hash: None,
            dest_hash: None,
            attempts: 1,
            inferred: false,
            recoverable: false,
            at_ms: 1,
        };
        let mut writer = store.results_writer().unwrap();
        writer
            .write(&item("ok.txt", 1234, ItemStatus::Arrived))
            .unwrap();
        writer
            .write(&item("bad.txt", 77, ItemStatus::NotCopied))
            .unwrap();
        writer.finish().unwrap();

        let doc = document(temp.path(), &records, ReportKind::Record, &store.id()).unwrap();
        let csv = doc.csv();
        let header = csv.lines().next().unwrap();
        assert!(header.contains("Bytes"), "no Bytes column in {header}");
        for bytes in ["1234", "77"] {
            assert!(
                csv.lines().any(|line| line.contains(&format!(",{bytes}"))),
                "raw {bytes} missing in {csv}"
            );
        }
    }

    #[test]
    fn ids_cannot_leave_the_folder() {
        let temp = tempfile::tempdir().unwrap();
        assert!(load(temp.path(), r"..\secret").is_err());
        assert!(load(temp.path(), "a/b").is_err());
        assert!(delete(temp.path(), temp.path(), ReportKind::Compare, "x.json").is_err());
        assert!(delete(temp.path(), temp.path(), ReportKind::Record, "..").is_err());
    }

    #[test]
    fn numbers_read_like_the_app() {
        assert_eq!(format_bytes(1), "1 byte");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(225 * 1024 * 1024 * 1024), "225 GB");
        assert_eq!(format_count(1_163_241), "1,163,241");
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(2048, "file"), "2,048 files");
    }
}
