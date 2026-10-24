//! What each kind of report holds, its row on the Reports page, and its export layout.

use crate::doc::{Document, Section, Share, Table, Tile, Tone, Verdict};
use crate::explorer::{self, Explorer};
use crate::header::{BodyHeader, Header};
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

#[cfg(test)]
pub(crate) fn entry(saved: &Saved) -> Entry {
    header_entry(Header::of(saved))
}

pub(crate) fn header_entry(header: Header) -> Entry {
    let (kind, title, subject, headline, problem) = match header.body {
        BodyHeader::Compare(report) => (
            ReportKind::Compare,
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
        BodyHeader::DiskUsage(report) => {
            let removed: u64 = report.removed.iter().map(|item| item.size).sum();
            let mut headline = format!("{} used", format_bytes(report.size));
            if removed > 0 {
                headline.push_str(&format!(" · {} removed", format_bytes(removed)));
            }
            (
                ReportKind::DiskUsage,
                "Disk usage".to_string(),
                report.path,
                headline,
                false,
            )
        }
        BodyHeader::Cleanup(report) => (
            ReportKind::Cleanup,
            "Quick cleanup".to_string(),
            report
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            format!(
                "Freed {}",
                format_bytes(report.items.iter().map(|item| item.freed).sum())
            ),
            false,
        ),
    };
    Entry {
        id: header.id,
        kind,
        created_at_ms: header.created_at_ms,
        title,
        subject,
        headline,
        problem,
        job: header.job,
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
        Body::Compare(report) => compare::document(report, meta),
        Body::DiskUsage(report) => usage::disk_usage(report, meta),
        Body::Cleanup(report) => usage::cleanup(report, meta),
    }
}

mod compare;
mod record;
#[cfg(test)]
mod tests;
mod usage;

#[cfg(test)]
use record::{check_label, record_verdict, status_label};
pub(crate) use record::{record_document, record_entry};
