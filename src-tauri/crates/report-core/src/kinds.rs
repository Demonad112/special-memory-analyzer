//! What each kind of report holds, its row on the Reports page, and its export layout.

use crate::doc::{Document, Section, Share, Table, Tile, Tone, Verdict};
use crate::explorer::{self, Explorer};
use crate::recovery::{Options, Plan};
use crate::{
    format_bytes, format_count, plural, record_job, Body, Entry, JobInfo, ReportKind, Saved,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;
use transfer_core::store::RunStore;
use transfer_core::{
    format_local, ItemResult, ItemStatus, RunState, RunSummary, TransferMode, VerifyLevel,
};

/// A finished compare: totals and the missing files.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CompareReport {
    pub source: String,
    pub destination: String,
    pub source_size: u64,
    pub source_files: u64,
    pub destination_size: u64,
    pub destination_files: u64,
    pub missing: u64,
    pub missing_bytes: u64,
    pub different: u64,
    pub extra: u64,
    /// Folders on either side that could not be read completely.
    pub unreadable: u64,
    pub elapsed_ms: u64,
    /// Relative to the source; may be cut short (see [`CompareReport::missing`]).
    pub missing_paths: Vec<MissingFile>,
}

/// A source file the destination lacks. Reports saved before sizes were kept hold bare path
/// strings; they load with a size of 0.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "MissingRepr")]
pub struct MissingFile {
    /// Relative to the source; an empty folder ends with a separator.
    pub path: String,
    pub size: u64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MissingRepr {
    Path(String),
    Full {
        path: String,
        #[serde(default)]
        size: u64,
    },
}

impl From<MissingRepr> for MissingFile {
    fn from(repr: MissingRepr) -> Self {
        match repr {
            MissingRepr::Path(path) => Self { path, size: 0 },
            MissingRepr::Full { path, size } => Self { path, size },
        }
    }
}

/// A Disk Cleanup scan, and what was deleted from it afterwards.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageReport {
    pub path: String,
    /// Space on disk; online-only files are not counted.
    pub size: u64,
    pub files: u64,
    pub dirs: u64,
    pub unreadable: u64,
    pub cloud_files: u64,
    pub cloud_bytes: u64,
    pub drive_total: Option<u64>,
    pub drive_free: Option<u64>,
    pub elapsed_ms: u64,
    pub top_folders: Vec<FolderItem>,
    pub largest_files: Vec<FileItem>,
    pub types: Vec<TypeItem>,
    pub removed: Vec<Removed>,
    /// The pruned size tree and the biggest files per type, for the report's explorer. Absent in
    /// reports saved before it existed.
    pub explorer: Option<Explorer>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FolderItem {
    pub name: String,
    pub size: u64,
    pub files: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileItem {
    pub path: String,
    pub size: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TypeItem {
    pub extension: String,
    pub size: u64,
    pub files: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Removed {
    pub path: String,
    pub size: u64,
    /// Deleted for good rather than moved to the Recycle Bin.
    pub permanent: bool,
}

/// A quick cleanup run (Temp files, browser caches, ...).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CleanReport {
    pub items: Vec<CleanedItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CleanedItem {
    pub title: String,
    pub freed: u64,
    pub removed_files: u64,
    /// Files left because they were in use or too new.
    pub skipped_files: u64,
    pub error: Option<String>,
}

impl CleanReport {
    pub fn freed(&self) -> u64 {
        self.items.iter().map(|item| item.freed).sum()
    }
}

pub(crate) fn entry(saved: &Saved) -> Entry {
    let (title, subject, headline, problem) = match &saved.body {
        Body::Compare(report) => (
            "Compare".to_string(),
            format!("{} → {}", report.source, report.destination),
            if report.missing > 0 {
                format!(
                    "{} missing ({})",
                    plural(report.missing, "file"),
                    format_bytes(report.missing_bytes)
                )
            } else {
                "Nothing missing".to_string()
            },
            report.missing > 0,
        ),
        Body::DiskUsage(report) => {
            let removed: u64 = report.removed.iter().map(|item| item.size).sum();
            let mut headline = format!("{} used", format_bytes(report.size));
            if removed > 0 {
                headline.push_str(&format!(" · {} removed", format_bytes(removed)));
            }
            (
                "Disk usage".to_string(),
                report.path.clone(),
                headline,
                false,
            )
        }
        Body::Cleanup(report) => (
            "Quick cleanup".to_string(),
            report
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            format!("Freed {}", format_bytes(report.freed())),
            false,
        ),
    };
    Entry {
        id: saved.id.clone(),
        kind: saved.body.kind(),
        created_at_ms: saved.created_at_ms,
        title,
        subject,
        headline,
        problem,
        job: saved.job.clone(),
    }
}

fn record_title(run: &RunSummary) -> &'static str {
    match run.settings.mode {
        TransferMode::Copy => "Copy for me",
        TransferMode::Watch => "Watched copy",
    }
}

pub(crate) fn record_entry(run: &RunSummary, job: JobInfo) -> Entry {
    let totals = &run.totals;
    let (headline, problem) = if totals.not_copied > 0 {
        (
            format!(
                "{} not copied ({})",
                plural(totals.not_copied, "file"),
                format_bytes(totals.not_copied_bytes)
            ),
            true,
        )
    } else {
        match run.state {
            RunState::Completed => (
                format!("All {} there", plural(totals.planned_files, "file")),
                false,
            ),
            RunState::Failed => (run.error.clone().unwrap_or_else(|| "Failed".into()), true),
            RunState::Cancelled => ("Cancelled".into(), true),
            _ => ("Not finished".into(), false),
        }
    };
    Entry {
        id: run.id.clone(),
        kind: ReportKind::Record,
        created_at_ms: run.created_at_ms,
        title: record_title(run).into(),
        subject: format!("{} → {}", run.settings.source, run.settings.destination),
        headline,
        problem,
        job,
    }
}

fn meta(job: &JobInfo, at_ms: u64, machine: &str, user: &str) -> Vec<(String, String)> {
    let mut meta = Vec::new();
    for (label, value) in [
        ("Client", &job.client),
        ("Ticket", &job.ticket),
        ("Technician", &job.technician),
    ] {
        if !value.trim().is_empty() {
            meta.push((label.to_string(), value.trim().to_string()));
        }
    }
    meta.push(("Date".into(), format_local(at_ms)));
    meta.push(("Computer".into(), machine.to_string()));
    if job.technician.trim().is_empty() && !user.is_empty() {
        meta.push(("Windows user".into(), user.to_string()));
    }
    meta
}

fn tile(label: &str, value: String, detail: String, tone: Tone) -> Tile {
    Tile {
        label: label.into(),
        value,
        detail,
        tone,
    }
}

fn seconds(ms: u64) -> String {
    format!("{:.1} s", ms as f64 / 1000.0)
}

pub(crate) fn document(saved: &Saved) -> Document {
    let meta = meta(&saved.job, saved.created_at_ms, &saved.machine, &saved.user);
    match &saved.body {
        Body::Compare(report) => {
            let listed = report.missing_paths.len() as u64;
            let mut notes = Vec::new();
            if report.unreadable > 0 {
                notes.push(format!(
                    "{} could not be read completely; sizes there may be low.",
                    plural(report.unreadable, "folder")
                ));
            }
            let plan = Plan::new(
                &report.source,
                &report.destination,
                &report.missing_paths,
                Options::default(),
            );
            let intro = format!(
                "Open PowerShell on a PC that can reach both folders, paste the command and press Enter. It copies {} again{}.",
                plan.as_ref().map_or_else(
                    || "the missing files".to_string(),
                    |plan| plural(plan.files, "missing file")
                ),
                if listed < report.missing {
                    format!(" (only the {} listed here)", format_count(listed))
                } else {
                    String::new()
                }
            );
            Document {
                title: "Compare".into(),
                subtitle: format!("{} → {}", report.source, report.destination),
                meta,
                verdict: Some(if report.missing > 0 {
                    Verdict::Bad
                } else {
                    Verdict::Good
                }),
                tiles: vec![
                    tile(
                        "Source",
                        format_bytes(report.source_size),
                        plural(report.source_files, "file"),
                        Tone::Plain,
                    ),
                    tile(
                        "Destination",
                        format_bytes(report.destination_size),
                        plural(report.destination_files, "file"),
                        Tone::Plain,
                    ),
                    if report.missing > 0 {
                        tile(
                            "Missing at destination",
                            plural(report.missing, "file"),
                            format!("{} not copied", format_bytes(report.missing_bytes)),
                            Tone::Bad,
                        )
                    } else {
                        tile(
                            "Missing at destination",
                            "None".into(),
                            "Every file is there".into(),
                            Tone::Good,
                        )
                    },
                    tile(
                        "Other differences",
                        plural(report.different, "size change"),
                        format!("{} only at destination", plural(report.extra, "file")),
                        Tone::Plain,
                    ),
                ],
                notes,
                share: (report.source_files > 0).then(|| {
                    let there = report.source_files.saturating_sub(report.missing);
                    Share {
                        label: "Source files found at the destination".into(),
                        detail: format!(
                            "{} of {}",
                            format_count(there),
                            format_count(report.source_files)
                        ),
                        percent: there as f64 * 100.0 / report.source_files as f64,
                        tone: if report.missing == 0 {
                            Tone::Good
                        } else {
                            Tone::Plain
                        },
                    }
                }),
                tables: vec![Table {
                    heading: "Missing at destination".into(),
                    columns: vec!["Path (relative to the source)".into(), "Size".into()],
                    numeric: vec![false, true],
                    rows: report
                        .missing_paths
                        .iter()
                        .map(|file| vec![file.path.clone(), format_bytes(file.size)])
                        .collect(),
                    size_col: Some(1),
                    row_bytes: report.missing_paths.iter().map(|file| file.size).collect(),
                    footnote: if listed < report.missing {
                        format!(
                            "The first {} of {} are listed.",
                            format_count(listed),
                            format_count(report.missing)
                        )
                    } else {
                        format!("Compared in {}.", seconds(report.elapsed_ms))
                    },
                    ..Table::default()
                }
                .toned(Tone::Bad)],
                sections: plan
                    .iter()
                    .map(|plan| plan.section(intro.clone()))
                    .collect(),
                data: plan.as_ref().map(|plan| json!({ "rec": plan.data() })),
                ..Document::default()
            }
        }
        Body::DiskUsage(report) => {
            let removed: u64 = report.removed.iter().map(|item| item.size).sum();
            let mut notes = Vec::new();
            if report.unreadable > 0 {
                notes.push(format!(
                    "{} could not be read, so the totals may be low.",
                    plural(report.unreadable, "folder")
                ));
            }
            if report.cloud_files > 0 {
                notes.push(format!(
                    "{} ({}) are online-only and take no space on this PC; they are not counted.",
                    plural(report.cloud_files, "cloud file"),
                    format_bytes(report.cloud_bytes)
                ));
            }
            let size_row = |name: &str, size: u64, total: u64| -> Vec<String> {
                let share = if total > 0 {
                    format!("{:.1}%", size as f64 * 100.0 / total as f64)
                } else {
                    String::new()
                };
                vec![name.to_string(), format_bytes(size), share]
            };
            let mut tables = vec![
                Table {
                    heading: "Largest files".into(),
                    columns: vec!["File".into(), "Size".into(), "Share".into()],
                    numeric: vec![false, true, true],
                    rows: report
                        .largest_files
                        .iter()
                        .map(|file| size_row(&file.path, file.size, report.size))
                        .collect(),
                    size_col: Some(1),
                    row_bytes: report.largest_files.iter().map(|file| file.size).collect(),
                    ..Table::default()
                },
                Table {
                    heading: "Biggest folders".into(),
                    columns: vec!["Folder".into(), "Size".into(), "Share".into()],
                    numeric: vec![false, true, true],
                    rows: report
                        .top_folders
                        .iter()
                        .map(|folder| size_row(&folder.name, folder.size, report.size))
                        .collect(),
                    size_col: Some(1),
                    row_bytes: report
                        .top_folders
                        .iter()
                        .map(|folder| folder.size)
                        .collect(),
                    ..Table::default()
                },
                Table {
                    heading: "File types".into(),
                    columns: vec!["Type".into(), "Size".into(), "Share".into()],
                    numeric: vec![false, true, true],
                    rows: report
                        .types
                        .iter()
                        .map(|kind| {
                            let name = if kind.extension.is_empty() {
                                format!("No extension ({})", plural(kind.files, "file"))
                            } else {
                                format!(".{} ({})", kind.extension, plural(kind.files, "file"))
                            };
                            size_row(&name, kind.size, report.size)
                        })
                        .collect(),
                    size_col: Some(1),
                    row_bytes: report.types.iter().map(|kind| kind.size).collect(),
                    ..Table::default()
                },
            ];
            if !report.removed.is_empty() {
                tables.insert(
                    0,
                    Table {
                        heading: "Removed".into(),
                        columns: vec!["Item".into(), "Size".into(), "How".into()],
                        numeric: vec![false, true, false],
                        rows: report
                            .removed
                            .iter()
                            .map(|item| {
                                vec![
                                    item.path.clone(),
                                    format_bytes(item.size),
                                    if item.permanent {
                                        "Deleted".into()
                                    } else {
                                        "Recycle Bin".into()
                                    },
                                ]
                            })
                            .collect(),
                        size_col: Some(1),
                        row_bytes: report.removed.iter().map(|item| item.size).collect(),
                        ..Table::default()
                    },
                );
            }
            let explorer_on = report
                .explorer
                .as_ref()
                .is_some_and(|explorer| explorer.nodes.len() > 1);
            let explorer_data = report.explorer.as_ref().filter(|_| explorer_on).map(
                |explorer| serde_json::json!({ "x": explorer::page_data(explorer, &report.path) }),
            );
            let mut tiles = vec![
                tile(
                    "Used",
                    format_bytes(report.size),
                    format!(
                        "{} · {}",
                        plural(report.files, "file"),
                        plural(report.dirs, "folder")
                    ),
                    Tone::Plain,
                ),
                tile(
                    "Removed",
                    format_bytes(removed),
                    plural(report.removed.len() as u64, "item"),
                    if removed > 0 { Tone::Good } else { Tone::Plain },
                ),
            ];
            if let (Some(total), Some(free)) = (report.drive_total, report.drive_free) {
                tiles.insert(
                    1,
                    tile(
                        "Free space at scan",
                        format_bytes(free),
                        format!("of {}", format_bytes(total)),
                        Tone::Plain,
                    ),
                );
            }
            Document {
                title: "Disk usage".into(),
                subtitle: report.path.clone(),
                meta,
                verdict: (removed > 0).then_some(Verdict::Good),
                tiles,
                notes,
                share: report
                    .drive_total
                    .filter(|total| *total > 0)
                    .map(|total| Share {
                        label: "Space this folder takes on the drive".into(),
                        detail: format!("{} of {}", format_bytes(report.size), format_bytes(total)),
                        percent: (report.size as f64 * 100.0 / total as f64).min(100.0),
                        tone: Tone::Plain,
                    }),
                tables,
                explorer: explorer_on,
                data: explorer_data,
                ..Document::default()
            }
        }
        Body::Cleanup(report) => Document {
            title: "Quick cleanup".into(),
            subtitle: format!("Freed {}", format_bytes(report.freed())),
            meta,
            verdict: Some(Verdict::Good),
            tiles: vec![tile(
                "Freed",
                format_bytes(report.freed()),
                plural(
                    report.items.iter().map(|item| item.removed_files).sum(),
                    "file",
                ) + " removed",
                Tone::Good,
            )],
            notes: vec!["Files in use or changed in the last day are left in place.".into()],
            share: (!report.items.is_empty()).then(|| {
                let total = report.items.len() as u64;
                let clean = report
                    .items
                    .iter()
                    .filter(|item| item.error.is_none())
                    .count() as u64;
                Share {
                    label: "Locations cleaned without error".into(),
                    detail: format!("{} of {}", format_count(clean), format_count(total)),
                    percent: clean as f64 * 100.0 / total as f64,
                    tone: if clean == total {
                        Tone::Good
                    } else {
                        Tone::Warn
                    },
                }
            }),
            tables: vec![Table {
                heading: "Cleaned".into(),
                columns: vec![
                    "Item".into(),
                    "Freed".into(),
                    "Files removed".into(),
                    "Files left".into(),
                ],
                numeric: vec![false, true, true, true],
                rows: report
                    .items
                    .iter()
                    .map(|item| {
                        vec![
                            match &item.error {
                                Some(error) => format!("{} ({error})", item.title),
                                None => item.title.clone(),
                            },
                            format_bytes(item.freed),
                            format_count(item.removed_files),
                            format_count(item.skipped_files),
                        ]
                    })
                    .collect(),
                size_col: Some(1),
                row_bytes: report.items.iter().map(|item| item.freed).collect(),
                ..Table::default()
            }],
            ..Document::default()
        },
    }
}

pub(crate) fn record_document(records: &Path, id: &str) -> Result<Document, String> {
    let store = RunStore::open(records, id).map_err(|err| err.to_string())?;
    let run = store.summary().map_err(|err| err.to_string())?;
    let results = store.latest_results().map_err(|err| err.to_string())?;
    let failed_items: Vec<_> = results
        .values()
        .filter(|item| item.status == ItemStatus::NotCopied)
        .collect();
    let row_bytes: Vec<u64> = failed_items.iter().map(|item| item.size).collect();
    let not_copied: Vec<Vec<String>> = failed_items
        .iter()
        .map(|item| {
            let reason = item
                .reason
                .map(|reason| reason.title())
                .unwrap_or("Other error");
            vec![
                item.relative_path.clone(),
                format_bytes(item.size),
                reason.to_string(),
                item.message.clone().unwrap_or_default(),
            ]
        })
        .collect();
    let verify = run.settings.verify;
    let arrived_items: Vec<_> = results
        .values()
        .filter(|item| item.status != ItemStatus::NotCopied)
        .collect();
    let arrived_rows: Vec<Vec<String>> = arrived_items
        .iter()
        .map(|item| {
            vec![
                item.relative_path.clone(),
                format_bytes(item.size),
                check_label(item, verify).into(),
            ]
        })
        .collect();
    let arrived_table = Table {
        heading: "Arrived".into(),
        columns: vec![
            "Path (relative to the source)".into(),
            "Size".into(),
            "Check".into(),
        ],
        numeric: vec![false, true, false],
        rows: arrived_rows,
        size_col: Some(1),
        row_bytes: arrived_items.iter().map(|item| item.size).collect(),
        ..Table::default()
    }
    .toned(Tone::Good);
    // Every file with its status: the CSV holds the whole record, not just the failures.
    let all_items: Vec<_> = results.values().collect();
    let csv_table = Table {
        heading: "All files".into(),
        columns: vec![
            "Path (relative to the source)".into(),
            "Size".into(),
            "Status".into(),
            "Check or reason".into(),
            "Details".into(),
        ],
        numeric: vec![false, true, false, false, false],
        rows: all_items
            .iter()
            .map(|item| {
                let (status, note) = if item.status == ItemStatus::NotCopied {
                    let reason = item.reason.map_or("Other error", |reason| reason.title());
                    ("Not copied", reason)
                } else {
                    (status_label(item.status), check_label(item, verify))
                };
                vec![
                    item.relative_path.clone(),
                    format_bytes(item.size),
                    status.into(),
                    note.into(),
                    item.message.clone().unwrap_or_default(),
                ]
            })
            .collect(),
        size_col: Some(1),
        row_bytes: all_items.iter().map(|item| item.size).collect(),
        ..Table::default()
    };
    let arrived_count = arrived_items.len();
    let totals = &run.totals;
    let job = record_job(&store);
    let arrived = match run.settings.mode {
        TransferMode::Copy => "Copied",
        TransferMode::Watch => "Arrived",
    };
    let mut notes = Vec::new();
    if let Some(error) = &run.error {
        notes.push(format!("The record stopped: {error}"));
    }
    if run.settings.ignore_junk {
        notes.push(
            "System and temp files (Thumbs.db, desktop.ini, ~$ lock files, .tmp) were ignored."
                .into(),
        );
    }
    Ok(Document {
        title: record_title(&run).into(),
        subtitle: format!("{} → {}", run.settings.source, run.settings.destination),
        meta: meta(&job, run.created_at_ms, &run.machine, &run.user),
        verdict: Some(record_verdict(&run)),
        tiles: vec![
            tile(
                "Source",
                format_bytes(totals.planned_bytes),
                plural(totals.planned_files, "file"),
                Tone::Plain,
            ),
            tile(
                arrived,
                plural(totals.copied + totals.skipped_identical, "file"),
                format!(
                    "{} · {} already there",
                    format_bytes(totals.copied_bytes),
                    format_count(totals.skipped_identical)
                ),
                Tone::Good,
            ),
            if totals.not_copied > 0 {
                tile(
                    "Not copied",
                    plural(totals.not_copied, "file"),
                    format_bytes(totals.not_copied_bytes),
                    Tone::Bad,
                )
            } else {
                tile(
                    "Not copied",
                    "None".into(),
                    "Every file made it".into(),
                    Tone::Good,
                )
            },
        ],
        notes,
        share: (totals.planned_files > 0).then(|| {
            let there = (totals.copied + totals.skipped_identical).min(totals.planned_files);
            Share {
                label: format!("Source files {}", arrived.to_lowercase()),
                detail: format!(
                    "{} of {}",
                    format_count(there),
                    format_count(totals.planned_files)
                ),
                percent: there as f64 * 100.0 / totals.planned_files as f64,
                tone: if totals.not_copied == 0 {
                    Tone::Good
                } else {
                    Tone::Plain
                },
            }
        }),
        tables: vec![Table {
            heading: "Not copied".into(),
            columns: vec![
                "Path (relative to the source)".into(),
                "Size".into(),
                "Reason".into(),
                "Details".into(),
            ],
            numeric: vec![false, true, false, false],
            rows: not_copied,
            size_col: Some(1),
            row_bytes,
            ..Table::default()
        }
        .toned(Tone::Bad)],
        csv_table: Some(csv_table),
        sections: vec![Section::Details {
            summary: format!("Arrived ({})", format_count(arrived_count as u64)),
            open: false,
            body: vec![Section::Table(arrived_table)],
        }],
        ..Document::default()
    })
}

fn status_label(status: ItemStatus) -> &'static str {
    match status {
        ItemStatus::Copied => "Copied",
        ItemStatus::Arrived => "Arrived",
        ItemStatus::SkippedIdentical => "Already there",
        ItemStatus::NotCopied => "Not copied",
    }
}

/// How an arrived file was checked. A hash is shown only when both sides were hashed and agree;
/// a differing hash is a `NotCopied` item with its own reason, never listed as arrived.
fn check_label(item: &ItemResult, verify: VerifyLevel) -> &'static str {
    match item.status {
        ItemStatus::SkippedIdentical => "Already there",
        _ if verify == VerifyLevel::Hash
            && item.source_hash.is_some()
            && item.source_hash == item.dest_hash =>
        {
            "Content verified (BLAKE3)"
        }
        _ => "Size + date match",
    }
}

/// "All clear" only when the run finished and every file made it: a run that failed or was
/// cancelled can have nothing in the not-copied list and still be unfinished.
fn record_verdict(run: &RunSummary) -> Verdict {
    let unfinished = matches!(run.state, RunState::Failed | RunState::Cancelled);
    if unfinished || run.error.is_some() || run.totals.not_copied > 0 {
        Verdict::Bad
    } else {
        Verdict::Good
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(body: Body) -> Saved {
        Saved {
            id: "x".into(),
            created_at_ms: 0,
            machine: "PC1".into(),
            user: "sam".into(),
            job: JobInfo {
                client: "Acme".into(),
                ticket: "T-42".into(),
                technician: String::new(),
            },
            body,
        }
    }

    #[test]
    fn compare_rows_flag_missing_files() {
        let report = CompareReport {
            source: r"C:\Data".into(),
            destination: r"E:\Data".into(),
            missing: 3,
            missing_bytes: 2048,
            missing_paths: vec![
                MissingFile {
                    path: "a.txt".into(),
                    size: 1024,
                },
                MissingFile {
                    path: r"sub\b.txt".into(),
                    size: 0,
                },
            ],
            ..CompareReport::default()
        };
        let saved = saved(Body::Compare(report));
        let row = entry(&saved);
        assert!(row.problem);
        assert_eq!(row.headline, "3 files missing (2.00 KB)");
        let doc = document(&saved);
        assert_eq!(doc.tables[0].rows.len(), 2);
        assert_eq!(doc.tables[0].rows[0][1], "1.00 KB");
        assert_eq!(doc.tables[0].row_bytes, vec![1024, 0]);
        assert!(doc.csv().contains("a.txt,1.00 KB,1024"));
        let Section::Recovery(rec) = &doc.sections[0] else {
            panic!("a recovery block")
        };
        assert!(rec.script.contains(r"robocopy 'C:\Data' 'E:\Data' 'a.txt'"));
        assert!(rec
            .script
            .contains(r"robocopy 'C:\Data\sub' 'E:\Data\sub' 'b.txt'"));
        assert!(doc.data.as_ref().unwrap()["rec"]["dirs"].is_array());
        assert_eq!(doc.tables[0].footnote, "The first 2 of 3 are listed.");
        assert_eq!(doc.tiles[2].tone, Tone::Bad);
        assert!(doc.meta.contains(&("Ticket".into(), "T-42".into())));
        assert!(doc.meta.contains(&("Windows user".into(), "sam".into())));
    }

    #[test]
    fn compare_html_has_the_recovery_block_and_stays_inside_the_csp() {
        let report = CompareReport {
            source: r"C:\Data".into(),
            destination: r"E:\Data".into(),
            missing: 2,
            missing_paths: vec![
                MissingFile {
                    path: r"app\node_modules\x.js".into(),
                    size: 5,
                },
                MissingFile {
                    path: r"it's\</script>.txt".into(),
                    size: 1,
                },
            ],
            ..CompareReport::default()
        };
        let html = document(&saved(Body::Compare(report))).html("Acme", "now");
        for needle in [
            "Copy the missing files again",
            "Show the full command",
            "Download .ps1",
            "data-bit=\"0\"",
            "node_modules: 1 file",
            "Copy pass #2 still didn't work",
            "id=\"rec-pass2\"",
        ] {
            assert!(html.contains(needle), "missing {needle}");
        }
        assert!(!html.contains("</script>.txt"), "island must escape <");
        assert_eq!(
            html.matches("<script").count(),
            2,
            "island and page script only"
        );
    }

    #[test]
    fn reports_saved_before_sizes_existed_still_load() {
        let old = r#"{"source":"C:\\A","destination":"D:\\B","missing":2,"missingPaths":["x.txt","d\\y.txt"]}"#;
        let report: CompareReport = serde_json::from_str(old).unwrap();
        assert_eq!(report.missing_paths.len(), 2);
        assert_eq!(report.missing_paths[1].path, r"d\y.txt");
        assert_eq!(report.missing_paths[1].size, 0);
        let new = r#"{"missingPaths":[{"path":"x.txt","size":7},{"path":"y.txt"}]}"#;
        let report: CompareReport = serde_json::from_str(new).unwrap();
        assert_eq!(report.missing_paths[0].size, 7);
        assert_eq!(report.missing_paths[1].size, 0);
        let again: CompareReport =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert_eq!(again, report);
    }

    #[test]
    fn usage_lists_removed_items_first() {
        let report = UsageReport {
            path: r"C:\".into(),
            size: 1000,
            removed: vec![Removed {
                path: r"C:\old.iso".into(),
                size: 400,
                permanent: false,
            }],
            ..UsageReport::default()
        };
        let saved = saved(Body::DiskUsage(report));
        assert_eq!(
            entry(&saved).headline,
            "1000 bytes used · 400 bytes removed"
        );
        let doc = document(&saved);
        assert_eq!(doc.tables[0].heading, "Removed");
        assert_eq!(doc.tables[0].rows[0][2], "Recycle Bin");
    }

    #[test]
    fn usage_report_with_an_explorer_gets_the_page_and_old_reports_do_not() {
        use crate::explorer::{ExplorerNode, KIND_FILE};
        let mut report = UsageReport {
            path: r"C:\".into(),
            size: 10,
            ..UsageReport::default()
        };
        // A report saved before the explorer existed: no explorer, no page, same tables.
        let old: UsageReport = serde_json::from_str(r#"{"path":"C:\\","size":10}"#).unwrap();
        assert!(old.explorer.is_none());
        let doc = document(&saved(Body::DiskUsage(old)));
        assert!(!doc.explorer && doc.data.is_none());

        // An empty scan has nothing to explore.
        report.explorer = Some(Explorer::default());
        assert!(!document(&saved(Body::DiskUsage(report.clone()))).explorer);

        report.explorer = Some(Explorer {
            nodes: vec![
                ExplorerNode {
                    name: r"C:\".into(),
                    disk: 10,
                    ..ExplorerNode::default()
                },
                ExplorerNode {
                    name: "a.mp4".into(),
                    disk: 10,
                    kind: KIND_FILE,
                    ..ExplorerNode::default()
                },
            ],
            ..Explorer::default()
        });
        let doc = document(&saved(Body::DiskUsage(report)));
        assert!(doc.explorer);
        let data = doc.data.expect("data island");
        assert_eq!(data["x"]["root"], r"C:\");
        assert_eq!(data["x"]["nodes"][1]["n"], "a.mp4");
        // The flat tables are still there for readers without scripts.
        assert_eq!(doc.tables[0].heading, "Largest files");
    }

    /// The explorer page for the script tests in `src/lib/reportExplorer.test.ts`: names that
    /// would break markup, folders three levels deep, a folded row and four file types.
    /// `UPDATE_FIXTURES=1 cargo test -p report-core explorer_fixture` rewrites it.
    #[test]
    fn explorer_fixture_matches_what_the_page_renders() {
        use crate::explorer::{ExplorerNode, TypeFiles, KIND_DIR, KIND_FILE, KIND_MORE};
        let node = |name: &str, disk: u64, files: u64, kind: u8, parent: u32| ExplorerNode {
            name: name.into(),
            disk,
            files,
            kind,
            parent,
        };
        let hostile = "</script><b>x</b>.txt";
        let explorer = Explorer {
            nodes: vec![
                node(r"C:\", 1000, 10, KIND_DIR, 0),
                node("Users", 600, 6, KIND_DIR, 0),
                node("Windows", 300, 3, KIND_DIR, 0),
                node("pagefile.sys", 90, 1, KIND_FILE, 0),
                node("(2 smaller items)", 10, 2, KIND_MORE, 0),
                node("Alice", 500, 5, KIND_DIR, 1),
                node(hostile, 100, 1, KIND_FILE, 1),
                node("Movies", 400, 1, KIND_DIR, 5),
                node("notes.txt", 100, 1, KIND_FILE, 5),
                node("clip.mp4", 400, 1, KIND_FILE, 7),
                node("system.dll", 300, 1, KIND_FILE, 2),
            ],
            types: vec![
                TypeItem {
                    extension: "mp4".into(),
                    size: 400,
                    files: 1,
                },
                TypeItem {
                    extension: "dll".into(),
                    size: 300,
                    files: 1,
                },
                TypeItem {
                    extension: "txt".into(),
                    size: 200,
                    files: 2,
                },
                TypeItem {
                    extension: "sys".into(),
                    size: 90,
                    files: 1,
                },
            ],
            type_files: vec![
                TypeFiles {
                    extension: "mp4".into(),
                    files: vec![(r"Users\Alice\Movies\clip.mp4".into(), 400)],
                },
                TypeFiles {
                    extension: "dll".into(),
                    files: vec![(r"Windows\system.dll".into(), 300)],
                },
                TypeFiles {
                    extension: "txt".into(),
                    files: vec![
                        (format!(r"Users\{hostile}"), 100),
                        (r"Users\Alice\notes.txt".into(), 100),
                    ],
                },
                TypeFiles {
                    extension: "sys".into(),
                    files: vec![("pagefile.sys".into(), 90)],
                },
            ],
            types_more: 0,
            scanned_items: 4000,
        };
        let report = UsageReport {
            path: r"C:\".into(),
            size: 1000,
            files: 10,
            dirs: 4,
            explorer: Some(explorer),
            ..UsageReport::default()
        };
        let html = document(&saved(Body::DiskUsage(report))).html("Fixture", "fixed date");
        // The hostile name did not open a script or element: ours, the explorer's and the island.
        assert_eq!(html.matches("<script").count(), 3);
        assert!(!html.contains("<b>x"));

        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/explorer.html");
        if std::env::var_os("UPDATE_FIXTURES").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &html).unwrap();
        }
        let committed = std::fs::read_to_string(&path)
            .expect("tests/fixtures/explorer.html (run with UPDATE_FIXTURES=1)")
            .replace("\r\n", "\n");
        // Only the explorer's own parts: the shared script may change without touching these.
        let part = |page: &str, from: &str, to: &str| -> String {
            let start = page.find(from).expect(from);
            let end = page[start..].find(to).expect(to) + start;
            page[start..end].to_string()
        };
        for (from, to) in [
            ("<script type=\"application/json\" id=\"d\">", "</script>"),
            ("<script>\n(function(){\nvar host=", "</script>"),
        ] {
            assert_eq!(
                part(&committed, from, to),
                part(&html, from, to),
                "fixture is stale: run UPDATE_FIXTURES=1 cargo test -p report-core explorer_fixture"
            );
        }
    }

    #[test]
    fn cleanup_adds_up_what_was_freed() {
        let report = CleanReport {
            items: vec![
                CleanedItem {
                    title: "Temporary files".into(),
                    freed: 1024,
                    removed_files: 3,
                    ..CleanedItem::default()
                },
                CleanedItem {
                    title: "Browser caches".into(),
                    freed: 1024,
                    ..CleanedItem::default()
                },
            ],
        };
        let saved = saved(Body::Cleanup(report));
        let row = entry(&saved);
        assert_eq!(row.headline, "Freed 2.00 KB");
        assert_eq!(row.subject, "Temporary files, Browser caches");
        assert_eq!(document(&saved).tables[0].rows.len(), 2);
        assert_eq!(document(&saved).verdict, Some(Verdict::Good));
        let share = document(&saved).share.expect("a bar");
        assert_eq!(share.detail, "2 of 2");
        assert_eq!(share.tone, Tone::Good);
    }

    #[test]
    fn cleanup_bar_turns_amber_when_a_location_failed() {
        let report = CleanReport {
            items: vec![
                CleanedItem::default(),
                CleanedItem {
                    error: Some("Access denied".into()),
                    ..CleanedItem::default()
                },
            ],
        };
        let share = document(&saved(Body::Cleanup(report))).share.unwrap();
        assert_eq!(share.detail, "1 of 2");
        assert_eq!(share.percent, 50.0);
        assert_eq!(share.tone, Tone::Warn);
    }

    #[test]
    fn disk_usage_bar_shows_the_share_of_the_drive_and_is_absent_without_one() {
        let mut report = UsageReport {
            path: r"C:\Users".into(),
            size: 250,
            drive_total: Some(1000),
            ..UsageReport::default()
        };
        let share = document(&saved(Body::DiskUsage(report.clone())))
            .share
            .expect("a bar");
        assert_eq!(share.percent, 25.0);
        report.drive_total = None;
        assert!(document(&saved(Body::DiskUsage(report))).share.is_none());
    }

    #[test]
    fn every_report_kind_exports_raw_bytes_in_the_csv() {
        let compare = CompareReport {
            missing: 1,
            missing_paths: vec![MissingFile {
                path: "a".into(),
                size: 7,
            }],
            ..CompareReport::default()
        };
        let usage = UsageReport {
            largest_files: vec![FileItem {
                path: "f".into(),
                size: 9,
            }],
            ..UsageReport::default()
        };
        let clean = CleanReport {
            items: vec![CleanedItem {
                title: "Temp".into(),
                freed: 11,
                ..CleanedItem::default()
            }],
        };
        for (body, bytes) in [
            (Body::Compare(compare), "7"),
            (Body::DiskUsage(usage), "9"),
            (Body::Cleanup(clean), "11"),
        ] {
            let csv = document(&saved(body)).csv();
            let header = csv.lines().next().unwrap();
            assert!(header.contains("Bytes"), "no Bytes column in {header}");
            assert!(
                csv.lines().any(|line| line.ends_with(&format!(",{bytes}"))
                    || line.contains(&format!(",{bytes},"))),
                "raw {bytes} missing in {csv}"
            );
        }
    }

    #[test]
    fn check_label_follows_verify_level_and_hashes() {
        let mut item = ItemResult::done("a.txt", 1, ItemStatus::Copied);
        assert_eq!(
            check_label(&item, VerifyLevel::SizeAndTime),
            "Size + date match"
        );
        // Hash level but nothing hashed (e.g. a file that was already there): no claim.
        assert_eq!(check_label(&item, VerifyLevel::Hash), "Size + date match");
        item.source_hash = Some("abc".into());
        item.dest_hash = Some("abc".into());
        assert_eq!(
            check_label(&item, VerifyLevel::Hash),
            "Content verified (BLAKE3)"
        );
        // A size-and-time run never claims content was verified.
        assert_eq!(
            check_label(&item, VerifyLevel::SizeAndTime),
            "Size + date match"
        );
        item.dest_hash = Some("def".into());
        assert_eq!(check_label(&item, VerifyLevel::Hash), "Size + date match");
        let same = ItemResult::done("b.txt", 1, ItemStatus::SkippedIdentical);
        assert_eq!(check_label(&same, VerifyLevel::Hash), "Already there");
        assert_eq!(status_label(ItemStatus::SkippedIdentical), "Already there");
    }

    #[test]
    fn record_verdict_matrix() {
        use transfer_core::{
            ConflictPolicy, RunTotals, TransferMode, TransferSettings, VerifyLevel,
        };
        let run = |state: RunState, not_copied: u64, error: Option<&str>| RunSummary {
            id: "r".into(),
            settings: TransferSettings {
                source: "a".into(),
                destination: "b".into(),
                mode: TransferMode::Copy,
                verify: VerifyLevel::SizeAndTime,
                conflict: ConflictPolicy::Skip,
                ignore_junk: false,
                download_cloud: false,
                preserve: Default::default(),
            },
            state,
            created_at_ms: 0,
            started_at_ms: None,
            finished_at_ms: None,
            machine: String::new(),
            user: String::new(),
            totals: RunTotals {
                not_copied,
                ..RunTotals::default()
            },
            error: error.map(str::to_string),
        };
        assert_eq!(
            record_verdict(&run(RunState::Completed, 0, None)),
            Verdict::Good
        );
        assert_eq!(
            record_verdict(&run(RunState::Watching, 0, None)),
            Verdict::Good
        );
        assert_eq!(
            record_verdict(&run(RunState::Completed, 2, None)),
            Verdict::Bad
        );
        // The bug this fixes: a failed or cancelled run with nothing listed used to read "All clear".
        assert_eq!(
            record_verdict(&run(RunState::Failed, 0, None)),
            Verdict::Bad
        );
        assert_eq!(
            record_verdict(&run(RunState::Cancelled, 0, None)),
            Verdict::Bad
        );
        assert_eq!(
            record_verdict(&run(RunState::Completed, 0, Some("disk full"))),
            Verdict::Bad
        );
    }
}
