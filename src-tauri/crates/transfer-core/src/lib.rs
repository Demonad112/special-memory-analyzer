//! Record a copy: Deep Server Analyzer copies a folder (or watches another tool copy it), and lists every
//! file that did not make it with the reason, so it can be retried or copied elsewhere.
//! The folder walk comes from `scan-core`.
//!
//! A run lives in its own folder under [`records_root`] (see [`store::RunStore`]):
//! - `run.json`: settings, state and totals
//! - `manifest.jsonl`: every source entry, written by [`prepare::prepare`]
//! - `preflight.json`: what the pre-flight check found
//! - `results.jsonl`: one row per file outcome, appended; the last row for a path wins
//! - `audit.jsonl`: run events
//!
//! Runs are plain files so they survive a restart and can be reopened.

pub mod copy;
mod csv;
pub mod meta;
pub mod prepare;
pub mod reason;
pub mod recovery;
pub mod run;
pub mod secure;
pub mod share;
pub mod store;
mod volume;
mod walk;
pub mod watch;

pub use copy::{ConflictPolicy, CopyOptions, VerifyLevel};
pub use csv::csv_cell;
pub use meta::PreserveOptions;
pub use reason::{classify, FailureReason, Side};
pub use scan_core::CancelToken;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Overrides where runs are stored (tests, portable layouts).
pub const RECORDS_DIR_ENV: &str = "DEEP_SERVER_ANALYZER_RECORDS_DIR";

/// `%LOCALAPPDATA%\Deep Server Analyzer\Records`.
/// In portable mode (a `portable.txt` next to the exe): `<exe folder>\Data\Records`.
pub fn records_root() -> PathBuf {
    if let Some(dir) = std::env::var_os(RECORDS_DIR_ENV).filter(|value| !value.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(data) = scan_core::portable_data_dir() {
        return data.join("Records");
    }
    std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Deep Server Analyzer")
        .join("Records")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferMode {
    /// Deep Server Analyzer copies the files.
    Copy,
    /// Another tool copies; Deep Server Analyzer checks what arrives against the source.
    Watch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSettings {
    pub source: String,
    pub destination: String,
    pub mode: TransferMode,
    pub verify: VerifyLevel,
    pub conflict: ConflictPolicy,
    /// Leave out system and temp files (`Thumbs.db`, `desktop.ini`, `~$` Office lock files, ...).
    #[serde(default)]
    pub ignore_junk: bool,
    /// Download OneDrive online-only files to copy them. Off: they are listed as not copied.
    #[serde(default)]
    pub download_cloud: bool,
    /// Dates and attributes to carry over to the copy.
    #[serde(default)]
    pub preserve: PreserveOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    /// Scanned and pre-flight checked; nothing copied yet.
    Prepared,
    Running,
    Watching,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTotals {
    /// Files in the source.
    pub planned_files: u64,
    pub planned_bytes: u64,
    pub folders: u64,
    /// Links and junctions, which are not followed or copied.
    pub excluded: u64,
    pub copied: u64,
    pub copied_bytes: u64,
    pub skipped_identical: u64,
    pub not_copied: u64,
    pub not_copied_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: String,
    pub settings: TransferSettings,
    pub state: RunState,
    pub created_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub machine: String,
    pub user: String,
    pub totals: RunTotals,
    /// Set when the run stopped on an error that is not about one file.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    File,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemStatus {
    /// Copied and verified by Deep Server Analyzer.
    Copied,
    /// Watch mode: arrived at the destination and matches the source.
    Arrived,
    SkippedIdentical,
    NotCopied,
}

/// The outcome for one file (or an unreadable folder).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemResult {
    /// Relative to the source and destination roots, with `/` separators.
    pub relative_path: String,
    pub kind: ItemKind,
    pub size: u64,
    pub status: ItemStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<FailureReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest_hash: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    /// Watch mode: the reason is Deep Server Analyzer's best guess, since it did not do the copy.
    #[serde(default)]
    pub inferred: bool,
    /// Whether "Copy to recovery folder" can work: the source must still be readable.
    #[serde(default)]
    pub recoverable: bool,
    pub at_ms: u64,
}

impl ItemResult {
    pub fn done(relative_path: impl Into<String>, size: u64, status: ItemStatus) -> Self {
        Self {
            relative_path: relative_path.into(),
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
            at_ms: now_ms(),
        }
    }

    pub fn not_copied(
        relative_path: impl Into<String>,
        kind: ItemKind,
        size: u64,
        reason: FailureReason,
        side: Option<Side>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            reason: Some(reason),
            side,
            message: Some(message.into()),
            kind,
            recoverable: recovery::recoverable(kind, reason, side),
            ..Self::done(relative_path, size, ItemStatus::NotCopied)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Scanning,
    Copying,
    Watching,
    Finishing,
    Done,
}

/// Counters for the summary strip. Rate and time left are worked out by the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    pub run_id: String,
    pub phase: Phase,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub copied: u64,
    pub skipped: u64,
    pub not_copied: u64,
    pub current: Option<String>,
}

/// Receives progress and item outcomes while a run works. The app forwards them as events.
pub trait TransferSink {
    fn progress(&mut self, progress: &TransferProgress);
    fn item(&mut self, item: &ItemResult);
}

/// A sink that drops everything (tests, background checks).
pub struct NullSink;

impl TransferSink for NullSink {
    fn progress(&mut self, _progress: &TransferProgress) {}
    fn item(&mut self, _item: &ItemResult) {}
}

#[derive(Debug)]
pub enum TransferError {
    Io(std::io::Error),
    InvalidPath(String),
    /// The run folder is missing or its files cannot be read.
    BadRun(String),
    Cancelled,
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransferError::Io(error) => write!(f, "{error}"),
            TransferError::InvalidPath(message) | TransferError::BadRun(message) => {
                f.write_str(message)
            }
            TransferError::Cancelled => f.write_str("Cancelled"),
        }
    }
}

impl std::error::Error for TransferError {}

impl From<std::io::Error> for TransferError {
    fn from(error: std::io::Error) -> Self {
        TransferError::Io(error)
    }
}

pub type Result<T> = std::result::Result<T, TransferError>;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

/// `2026-09-30 06:15:02 UTC`.
pub fn format_utc(ms: u64) -> String {
    let secs = ms / 1000;
    let (hour, minute, second) = (secs / 3600 % 24, secs / 60 % 60, secs % 60);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let days = (secs / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

/// `2026-10-01 16:24:00 (UTC-06:00)`: the machine's local time, for reports read by people.
pub fn format_local(ms: u64) -> String {
    use chrono::{Local, TimeZone};
    let offset = Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|time| time.offset().local_minus_utc())
        .unwrap_or_default();
    format_with_offset(ms, offset)
}

/// [`format_local`] with an explicit UTC offset in seconds.
pub fn format_with_offset(ms: u64, offset_secs: i32) -> String {
    let shifted = (ms as i64)
        .saturating_add(i64::from(offset_secs) * 1000)
        .max(0) as u64;
    let text = format_utc(shifted);
    let local = text.trim_end_matches(" UTC");
    let sign = if offset_secs < 0 { '-' } else { '+' };
    let minutes = offset_secs.unsigned_abs() / 60;
    format!("{local} (UTC{sign}{:02}:{:02})", minutes / 60, minutes % 60)
}

fn env_or(names: &[&str], fallback: &str) -> String {
    names
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_else(|| fallback.to_owned())
}

pub fn machine_name() -> String {
    env_or(&["COMPUTERNAME", "HOSTNAME"], "unknown")
}

pub fn user_name() -> String {
    env_or(&["USERNAME", "USER"], "unknown")
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    #[cfg(windows)]
    use std::path::Path;
    use std::path::PathBuf;

    /// A temp folder removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(label: &str) -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!("analyzer-{label}-{stamp}"));
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        pub fn path(&self, relative: &str) -> PathBuf {
            self.0.join(relative)
        }

        pub fn write(&self, relative: &str, content: &[u8]) -> PathBuf {
            let path = self.path(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Denies `rights` to Everyone on a folder with icacls; removed on drop.
    #[cfg(windows)]
    pub struct DenyGuard(PathBuf);

    #[cfg(windows)]
    impl DenyGuard {
        pub fn new(path: &Path, rights: &str) -> Self {
            let status = std::process::Command::new("icacls")
                .arg(path)
                .args(["/deny", &format!("*S-1-1-0:{rights}")])
                .output()
                .expect("icacls should run")
                .status;
            assert!(status.success(), "icacls /deny failed");
            Self(path.to_path_buf())
        }
    }

    #[cfg(windows)]
    impl Drop for DenyGuard {
        fn drop(&mut self) {
            let _ = std::process::Command::new("icacls")
                .arg(&self.0)
                .args(["/remove:d", "*S-1-1-0"])
                .output();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_times() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_utc(951_782_400_000), "2000-02-29 00:00:00 UTC");
        assert_eq!(format_utc(1_790_000_000_123), "2026-09-21 14:13:20 UTC");
        assert_eq!(
            format_with_offset(1_790_000_000_123, -6 * 3600),
            "2026-09-21 08:13:20 (UTC-06:00)"
        );
        assert_eq!(
            format_with_offset(1_790_000_000_123, 5 * 3600 + 1800),
            "2026-09-21 19:43:20 (UTC+05:30)"
        );
        assert!(format_local(1_790_000_000_123).starts_with("2026-09-2"));
    }

    #[test]
    fn item_results_skip_empty_fields_in_json() {
        let item = ItemResult::done("a/b.txt", 3, ItemStatus::Copied);
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["relativePath"], "a/b.txt");
        assert_eq!(json["status"], "copied");
        assert!(json.get("reason").is_none());
    }
}
