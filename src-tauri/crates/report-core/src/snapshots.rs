//! Scan history: a small snapshot of every completed Disk Cleanup scan, kept under
//! `Reports\snapshots\<key>\<timestamp>.json` (D-16), plus the growth diff between two snapshots
//! and the "full in about N days" forecast (D-17).
//!
//! The folder sits inside the reports folder, which is on the never-delete list; `prune` only
//! touches top-level `*.json` files, so it never reaches it. Files are written atomically and a
//! torn or unreadable one is skipped when listing.

use scan_core::{Kind, Tree};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use transfer_core::store::write_atomic;

pub const SNAPSHOTS_DIR: &str = "snapshots";
/// Newest snapshots kept per root.
pub const KEEP_PER_ROOT: usize = 60;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Nothing older than two years is kept.
pub const MAX_AGE_MS: u64 = 2 * 365 * DAY_MS;
/// Folders are recorded down to this depth below the root.
pub const MAX_DEPTH: usize = 3;
/// A folder must hold at least this share of the root (per mille): 0.5%.
const MIN_SHARE_PER_MILLE: u64 = 5;
/// Folders kept in one snapshot.
pub const MAX_FOLDERS: usize = 2000;

/// The forecast looks at this many days of snapshots, needs this many points over this many days,
/// and is hidden when the drive is more than this far from full.
pub const FORECAST_WINDOW_DAYS: u64 = 90;
pub const FORECAST_MIN_POINTS: usize = 3;
pub const FORECAST_MIN_SPAN_DAYS: f64 = 14.0;
pub const FORECAST_MAX_DAYS: f64 = 5.0 * 365.0;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SnapFolder {
    /// Path below the root, with `\` between names.
    pub path: String,
    /// Space on disk.
    pub size: u64,
    pub files: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Snapshot {
    /// The canonical scan root.
    pub root: String,
    pub taken_ms: u64,
    pub drive_total: Option<u64>,
    pub drive_free: Option<u64>,
    /// Space on disk, files and folders of the whole root.
    pub size: u64,
    pub files: u64,
    pub dirs: u64,
    /// Largest first.
    pub folders: Vec<SnapFolder>,
}

impl Snapshot {
    /// Drive space in use, when both numbers were known.
    pub fn drive_used(&self) -> Option<u64> {
        Some(self.drive_total?.saturating_sub(self.drive_free?))
    }

    /// The snapshot of a scan: root totals and the folders to [`MAX_DEPTH`] that hold at least
    /// 0.5% of the root, at most [`MAX_FOLDERS`] of them (the largest).
    pub fn from_tree(
        root: &str,
        tree: &Tree,
        drive: (Option<u64>, Option<u64>),
        taken_ms: u64,
    ) -> Self {
        Self::capped(root, tree, drive, taken_ms, MAX_FOLDERS)
    }

    fn capped(
        root: &str,
        tree: &Tree,
        drive: (Option<u64>, Option<u64>),
        taken_ms: u64,
        max_folders: usize,
    ) -> Self {
        let top = tree.root();
        let mut folders = Vec::new();
        let mut stack = vec![(0, 0usize, String::new())];
        while let Some((at, depth, path)) = stack.pop() {
            if depth == MAX_DEPTH {
                continue;
            }
            for child in tree.live_children(at) {
                let node = tree.node(child);
                if node.kind != Kind::Dir
                    || node.lossy_name
                    || node.disk.saturating_mul(1000) < top.disk.saturating_mul(MIN_SHARE_PER_MILLE)
                    || node.disk == 0
                {
                    continue;
                }
                let here = if path.is_empty() {
                    node.name.clone()
                } else {
                    format!("{path}\\{}", node.name)
                };
                folders.push(SnapFolder {
                    path: here.clone(),
                    size: node.disk,
                    files: node.files,
                });
                stack.push((child, depth + 1, here));
            }
        }
        folders.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
        folders.truncate(max_folders);
        Self {
            root: root.to_string(),
            taken_ms,
            drive_total: drive.0,
            drive_free: drive.1,
            size: top.disk,
            files: top.files,
            dirs: top.dirs,
            folders,
        }
    }
}

/// The folder name for a root: hex SHA-256 of its canonical path (D-16).
pub fn key(root: &str) -> String {
    Sha256::digest(root.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_key(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn snapshots_root(reports: &Path) -> PathBuf {
    reports.join(SNAPSHOTS_DIR)
}

fn root_dir(reports: &Path, root: &str) -> PathBuf {
    snapshots_root(reports).join(key(root))
}

/// Saves `snapshot`, then applies the retention rules to its root. Returns the file written.
pub fn save(reports: &Path, snapshot: &Snapshot, now_ms: u64) -> io::Result<PathBuf> {
    let dir = root_dir(reports, &snapshot.root);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{:016}.json", snapshot.taken_ms));
    let text = serde_json::to_string(snapshot).map_err(io::Error::other)?;
    write_atomic(&path, text.as_bytes())?;
    retain(&dir, now_ms);
    Ok(path)
}

/// The snapshot files of a folder as (timestamp, path), newest first. Only `<digits>.json`.
fn files(dir: &Path) -> Vec<(u64, PathBuf)> {
    let mut found: Vec<(u64, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                return None;
            }
            let at = path.file_stem()?.to_str()?.parse::<u64>().ok()?;
            Some((at, path))
        })
        .collect();
    found.sort_by_key(|file| std::cmp::Reverse(file.0));
    found
}

/// Keeps the newest [`KEEP_PER_ROOT`] files that are at most [`MAX_AGE_MS`] old; returns how
/// many were removed.
fn retain(dir: &Path, now_ms: u64) -> u64 {
    let cutoff = now_ms.saturating_sub(MAX_AGE_MS);
    let mut removed = 0;
    for (index, (at, path)) in files(dir).into_iter().enumerate() {
        if (index >= KEEP_PER_ROOT || at < cutoff) && fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Every readable snapshot of `root`, oldest first. A torn or unreadable file is skipped.
pub fn list(reports: &Path, root: &str) -> Vec<Snapshot> {
    let mut found: Vec<Snapshot> = files(&root_dir(reports, root))
        .into_iter()
        .filter_map(|(_, path)| {
            let snapshot: Snapshot = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
            (snapshot.root == root).then_some(snapshot)
        })
        .collect();
    found.sort_by_key(|snapshot| snapshot.taken_ms);
    found
}

/// One root that has history, for Settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootHistory {
    pub key: String,
    pub root: String,
    pub count: u64,
    pub first_ms: u64,
    pub last_ms: u64,
}

/// The roots with at least one readable snapshot, most recent first.
pub fn roots(reports: &Path) -> Vec<RootHistory> {
    let mut found = Vec::new();
    for entry in fs::read_dir(snapshots_root(reports))
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_key(&name) || !entry.path().is_dir() {
            continue;
        }
        let snapshots: Vec<Snapshot> = files(&entry.path())
            .into_iter()
            .filter_map(|(_, path)| serde_json::from_slice(&fs::read(path).ok()?).ok())
            .collect();
        let (Some(newest), Some(oldest)) = (
            snapshots.iter().max_by_key(|s: &&Snapshot| s.taken_ms),
            snapshots.iter().min_by_key(|s: &&Snapshot| s.taken_ms),
        ) else {
            continue;
        };
        found.push(RootHistory {
            key: name,
            root: newest.root.clone(),
            count: snapshots.len() as u64,
            first_ms: oldest.taken_ms,
            last_ms: newest.taken_ms,
        });
    }
    found.sort_by(|a, b| b.last_ms.cmp(&a.last_ms).then_with(|| a.root.cmp(&b.root)));
    found
}

/// Deletes the history of one root (`key` from [`roots`]) or of every root. Only folders named
/// by a 64-digit key inside `snapshots` are touched. Returns how many snapshot files went.
pub fn delete(reports: &Path, key: Option<&str>) -> Result<u64, String> {
    let base = snapshots_root(reports);
    let mut removed = 0;
    for entry in fs::read_dir(&base).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_key(&name) || key.is_some_and(|wanted| !wanted.eq_ignore_ascii_case(&name)) {
            continue;
        }
        removed += fs::read_dir(entry.path()).map_or(0, |dir| {
            dir.flatten().filter(|e| e.path().is_file()).count() as u64
        });
        fs::remove_dir_all(entry.path()).map_err(|err| err.to_string())?;
    }
    Ok(removed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Change {
    Grown,
    Shrunk,
    /// Not in the earlier snapshot: new, or it was below 0.5% of the root then.
    New,
    /// Not in the later snapshot: gone, or it fell below 0.5% of the root.
    Removed,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthRow {
    pub path: String,
    pub change: Change,
    pub before: u64,
    pub after: u64,
    pub delta: i64,
    /// Of the earlier size; `None` for a new folder.
    pub percent: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Growth {
    pub from_ms: u64,
    pub to_ms: u64,
    pub before: u64,
    pub after: u64,
    pub delta: i64,
    /// Folders that changed, largest change (either way) first; unchanged ones are left out.
    pub rows: Vec<GrowthRow>,
}

fn signed(after: u64, before: u64) -> i64 {
    i64::try_from(i128::from(after) - i128::from(before)).unwrap_or(if after > before {
        i64::MAX
    } else {
        i64::MIN
    })
}

/// What changed between two snapshots of the same root. Paths match without regard to case.
pub fn growth(earlier: &Snapshot, later: &Snapshot) -> Growth {
    let old: HashMap<String, u64> = earlier
        .folders
        .iter()
        .map(|f| (f.path.to_lowercase(), f.size))
        .collect();
    let new: HashMap<String, u64> = later
        .folders
        .iter()
        .map(|f| (f.path.to_lowercase(), f.size))
        .collect();
    let mut rows = Vec::new();
    for folder in &later.folders {
        let before = old.get(&folder.path.to_lowercase()).copied();
        let after = folder.size;
        let (change, before_size) = match before {
            None => (Change::New, 0),
            Some(before) if after > before => (Change::Grown, before),
            Some(before) if after < before => (Change::Shrunk, before),
            Some(_) => continue,
        };
        rows.push(GrowthRow {
            path: folder.path.clone(),
            change,
            before: before_size,
            after,
            delta: signed(after, before_size),
            percent: (before_size > 0)
                .then(|| signed(after, before_size) as f64 * 100.0 / before_size as f64),
        });
    }
    for folder in &earlier.folders {
        if !new.contains_key(&folder.path.to_lowercase()) {
            rows.push(GrowthRow {
                path: folder.path.clone(),
                change: Change::Removed,
                before: folder.size,
                after: 0,
                delta: signed(0, folder.size),
                percent: Some(-100.0),
            });
        }
    }
    rows.sort_by(|a, b| {
        b.delta
            .unsigned_abs()
            .cmp(&a.delta.unsigned_abs())
            .then_with(|| a.path.cmp(&b.path))
    });
    Growth {
        from_ms: earlier.taken_ms,
        to_ms: later.taken_ms,
        before: earlier.size,
        after: later.size,
        delta: signed(later.size, earlier.size),
        rows,
    }
}

/// What the forecast line says.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Forecast {
    /// Fewer than 3 snapshots, or they span less than 14 days.
    NotEnoughHistory,
    /// The trend is flat or shrinking.
    NotGrowing,
    /// More than 5 years away: not shown.
    Hidden,
    #[serde(rename_all = "camelCase")]
    Full {
        /// Whole days from `now` until the line reaches the drive's size (0 when already there).
        days: u64,
        /// Growth per day on the fitted line.
        per_day: u64,
        /// Days between the first and last snapshot used, and how many there were.
        window_days: u64,
        points: u64,
    },
}

/// Least-squares line over the drive space in use (D-17): snapshots of the last 90 days that know
/// the drive's size, at least 3 spanning at least 14 days, and a positive slope. The drive's size
/// is the newest snapshot's. `snapshots` need not be sorted.
pub fn forecast(snapshots: &[Snapshot], now_ms: u64) -> Forecast {
    let since = now_ms.saturating_sub(FORECAST_WINDOW_DAYS * DAY_MS);
    let mut points: Vec<(u64, u64, u64)> = snapshots
        .iter()
        .filter(|s| s.taken_ms >= since && s.taken_ms <= now_ms)
        .filter_map(|s| Some((s.taken_ms, s.drive_used()?, s.drive_total?)))
        .collect();
    points.sort_by_key(|point| point.0);
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Forecast::NotEnoughHistory;
    };
    let span_days = (last.0 - first.0) as f64 / DAY_MS as f64;
    if points.len() < FORECAST_MIN_POINTS || span_days < FORECAST_MIN_SPAN_DAYS {
        return Forecast::NotEnoughHistory;
    }
    // Days since the first point keep the numbers small.
    let origin = first.0;
    let xs: Vec<f64> = points
        .iter()
        .map(|p| (p.0 - origin) as f64 / DAY_MS as f64)
        .collect();
    let ys: Vec<f64> = points.iter().map(|p| p.1 as f64).collect();
    let n = xs.len() as f64;
    let (mean_x, mean_y) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let sxx: f64 = xs.iter().map(|x| (x - mean_x).powi(2)).sum();
    let sxy: f64 = xs
        .iter()
        .zip(&ys)
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum();
    let slope = sxy / sxx;
    if slope.is_nan() || slope <= 0.0 {
        return Forecast::NotGrowing;
    }
    let intercept = mean_y - slope * mean_x;
    let full_at = (last.2 as f64 - intercept) / slope;
    let days = (full_at - (now_ms - origin) as f64 / DAY_MS as f64).max(0.0);
    if days > FORECAST_MAX_DAYS {
        return Forecast::Hidden;
    }
    Forecast::Full {
        days: days.round() as u64,
        per_day: slope.round() as u64,
        window_days: span_days.round() as u64,
        points: points.len() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scan_core::{scan, ScanState};

    const ROOT: &str = r"D:\Shares";

    fn snap(taken_ms: u64, folders: &[(&str, u64)]) -> Snapshot {
        Snapshot {
            root: ROOT.into(),
            taken_ms,
            size: folders.iter().map(|f| f.1).sum(),
            folders: folders
                .iter()
                .map(|&(path, size)| SnapFolder {
                    path: path.into(),
                    size,
                    files: 1,
                })
                .collect(),
            ..Snapshot::default()
        }
    }

    fn drive(taken_ms: u64, used: u64) -> Snapshot {
        Snapshot {
            drive_total: Some(1_000_000),
            drive_free: Some(1_000_000 - used),
            ..snap(taken_ms, &[])
        }
    }

    const NOW: u64 = 1_800_000_000_000;

    #[test]
    fn a_saved_snapshot_loads_and_is_listed_oldest_first() {
        let temp = tempfile::tempdir().unwrap();
        save(temp.path(), &snap(NOW - 1000, &[("a", 5)]), NOW).unwrap();
        save(temp.path(), &snap(NOW - 3000, &[("a", 3)]), NOW).unwrap();
        let other = Snapshot {
            root: r"D:\Other".into(),
            ..snap(NOW - 2000, &[])
        };
        save(temp.path(), &other, NOW).unwrap();
        let listed = list(temp.path(), ROOT);
        assert_eq!(
            listed.iter().map(|s| s.taken_ms).collect::<Vec<_>>(),
            [NOW - 3000, NOW - 1000]
        );
        assert_eq!(listed[1].folders[0].size, 5);
        assert_eq!(list(temp.path(), r"D:\Other").len(), 1);
        assert_eq!(roots(temp.path()).len(), 2);
    }

    #[test]
    fn writes_are_atomic_and_a_torn_file_is_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let path = save(temp.path(), &snap(NOW - 2000, &[("a", 3)]), NOW).unwrap();
        let dir = path.parent().unwrap();
        let leftovers = |dir: &Path| {
            fs::read_dir(dir)
                .unwrap()
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "tmp"))
                .count()
        };
        assert_eq!(leftovers(dir), 0, "no .tmp left behind");

        // A power cut mid-write of a newer file, and a file that is not ours.
        fs::write(
            dir.join(format!("{:016}.json", NOW - 1000)),
            r#"{"root":"D:\\Sha"#,
        )
        .unwrap();
        fs::write(dir.join("notes.json"), "{}").unwrap();
        fs::write(dir.join(format!("{:016}.json.tmp", NOW)), "{").unwrap();
        let listed = list(temp.path(), ROOT);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].taken_ms, NOW - 2000);
        assert_eq!(roots(temp.path())[0].count, 1);
    }

    #[test]
    fn retention_keeps_the_newest_sixty_and_nothing_older_than_two_years() {
        let temp = tempfile::tempdir().unwrap();
        for i in 0..65u64 {
            save(temp.path(), &snap(NOW - i * DAY_MS, &[]), NOW).unwrap();
        }
        let listed = list(temp.path(), ROOT);
        assert_eq!(listed.len(), KEEP_PER_ROOT);
        assert_eq!(listed.last().unwrap().taken_ms, NOW);
        assert_eq!(
            listed[0].taken_ms,
            NOW - 59 * DAY_MS,
            "the five oldest went"
        );

        // Two years and a day old: gone at the next save, whatever the count.
        let old = snap(NOW - MAX_AGE_MS - DAY_MS, &[]);
        save(temp.path(), &old, NOW).unwrap();
        assert!(list(temp.path(), ROOT)
            .iter()
            .all(|s| s.taken_ms != old.taken_ms));

        // Exactly two years old is kept (when there is room).
        let fresh = tempfile::tempdir().unwrap();
        let edge = snap(NOW - MAX_AGE_MS, &[]);
        save(fresh.path(), &edge, NOW).unwrap();
        assert_eq!(list(fresh.path(), ROOT).len(), 1);
    }

    #[test]
    fn prune_leaves_the_snapshots_folder_alone() {
        let temp = tempfile::tempdir().unwrap();
        let reports = temp.path();
        let path = save(reports, &snap(NOW - 1000, &[("a", 1)]), NOW).unwrap();
        fs::write(reports.join("old-report.json"), "{}").unwrap();
        // Far in the future, so every file is "older than 1 day".
        let later = crate::now_ms() + 400 * DAY_MS;
        assert_eq!(crate::prune(reports, 1, later), 1);
        assert!(!reports.join("old-report.json").exists());
        assert!(path.exists(), "the snapshot survived prune");
        assert_eq!(list(reports, ROOT).len(), 1);
    }

    #[test]
    fn deleting_history_takes_one_root_or_all_and_only_snapshot_folders() {
        let temp = tempfile::tempdir().unwrap();
        let reports = temp.path();
        save(reports, &snap(NOW - 2000, &[]), NOW).unwrap();
        save(reports, &snap(NOW - 1000, &[]), NOW).unwrap();
        let other = Snapshot {
            root: r"D:\Other".into(),
            ..snap(NOW, &[])
        };
        save(reports, &other, NOW).unwrap();
        fs::write(reports.join("keep.json"), "{}").unwrap();
        fs::create_dir_all(snapshots_root(reports).join("not-a-key")).unwrap();

        assert_eq!(delete(reports, Some(&key(ROOT))), Ok(2));
        assert!(list(reports, ROOT).is_empty());
        assert_eq!(list(reports, r"D:\Other").len(), 1);
        assert_eq!(delete(reports, Some("../..")), Ok(0));
        assert_eq!(delete(reports, None), Ok(1));
        assert!(roots(reports).is_empty());
        assert!(reports.join("keep.json").exists());
        assert!(snapshots_root(reports).join("not-a-key").exists());
    }

    #[test]
    fn growth_finds_grown_shrunk_new_and_removed_folders() {
        let before = snap(
            1,
            &[
                ("a", 100),
                ("b", 200),
                ("c", 50),
                ("gone", 80),
                ("same", 10),
            ],
        );
        let after = snap(
            2,
            &[
                ("A", 160),
                ("b", 150),
                ("c", 50),
                ("fresh", 70),
                ("same", 10),
            ],
        );
        let growth = growth(&before, &after);
        assert_eq!((growth.from_ms, growth.to_ms), (1, 2));
        assert_eq!(growth.delta, after.size as i64 - before.size as i64);
        let rows: Vec<(&str, Change, i64)> = growth
            .rows
            .iter()
            .map(|r| (r.path.as_str(), r.change, r.delta))
            .collect();
        assert_eq!(
            rows,
            [
                ("gone", Change::Removed, -80),
                ("fresh", Change::New, 70),
                ("A", Change::Grown, 60),
                ("b", Change::Shrunk, -50),
            ]
        );
        assert_eq!(growth.rows[2].percent, Some(60.0), "case does not matter");
        assert_eq!(growth.rows[1].percent, None);
        assert_eq!(growth.rows[3].percent, Some(-25.0));
    }

    #[test]
    fn a_snapshot_keeps_folders_to_depth_three_that_hold_half_a_percent() {
        let temp = tempfile::tempdir().unwrap();
        let put = |name: &str, len: usize| {
            let path = temp.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, vec![1u8; len]).unwrap();
        };
        put("big/x/y/z/deep.bin", 4_000_000);
        put("big/other.bin", 4_000_000);
        put("small/tiny.bin", 1_000);
        let tree = scan(temp.path(), &ScanState::default()).unwrap();
        let snapshot = Snapshot::from_tree("T", &tree, (Some(10), Some(4)), 7);
        let paths: Vec<&str> = snapshot.folders.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["big", r"big\x", r"big\x\y"], "{paths:?}");
        assert_eq!(snapshot.drive_used(), Some(6));
        assert_eq!(snapshot.size, tree.root().disk);
        assert_eq!(snapshot.files, 3);
    }

    #[test]
    fn a_snapshot_keeps_only_the_largest_folders_over_the_cap() {
        let temp = tempfile::tempdir().unwrap();
        for (name, len) in [("a", 3_000), ("b", 2_000), ("c", 1_000)] {
            let path = temp.path().join(format!("{name}/f.bin"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, vec![1u8; len]).unwrap();
        }
        let tree = scan(temp.path(), &ScanState::default()).unwrap();
        let snapshot = Snapshot::capped("T", &tree, (None, None), 7, 2);
        let paths: Vec<&str> = snapshot.folders.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a", "b"]);
        assert_eq!(
            Snapshot::from_tree("T", &tree, (None, None), 7)
                .folders
                .len(),
            3
        );
    }

    #[test]
    fn old_snapshot_files_with_missing_fields_still_load() {
        let snapshot: Snapshot = serde_json::from_str(r#"{"root":"D:\\x","takenMs":5}"#).unwrap();
        assert_eq!(snapshot.folders, []);
        assert_eq!(snapshot.drive_used(), None);
    }

    #[test]
    fn forecast_needs_three_points_over_two_weeks() {
        assert_eq!(forecast(&[], NOW), Forecast::NotEnoughHistory);
        let two = [drive(NOW - 30 * DAY_MS, 100), drive(NOW, 200)];
        assert_eq!(forecast(&two, NOW), Forecast::NotEnoughHistory);
        // Three points, but only 10 days.
        let short: Vec<Snapshot> = [10, 5, 0]
            .iter()
            .map(|d| drive(NOW - d * DAY_MS, 100 + (10 - d) * 10))
            .collect();
        assert_eq!(forecast(&short, NOW), Forecast::NotEnoughHistory);
        // Snapshots without the drive's size do not count.
        let blind: Vec<Snapshot> = [30, 15, 0]
            .iter()
            .map(|d| snap(NOW - d * DAY_MS, &[]))
            .collect();
        assert_eq!(forecast(&blind, NOW), Forecast::NotEnoughHistory);
        // Points older than 90 days are ignored.
        let stale: Vec<Snapshot> = [200, 150, 120]
            .iter()
            .map(|d| drive(NOW - d * DAY_MS, 1000))
            .collect();
        assert_eq!(forecast(&stale, NOW), Forecast::NotEnoughHistory);
    }

    #[test]
    fn forecast_says_not_growing_for_flat_or_shrinking_use() {
        let flat: Vec<Snapshot> = [60, 30, 0]
            .iter()
            .map(|d| drive(NOW - d * DAY_MS, 500))
            .collect();
        assert_eq!(forecast(&flat, NOW), Forecast::NotGrowing);
        let shrinking: Vec<Snapshot> = [60, 30, 0]
            .iter()
            .map(|d| drive(NOW - d * DAY_MS, 500 - (60 - d) * 2))
            .collect();
        assert_eq!(forecast(&shrinking, NOW), Forecast::NotGrowing);
    }

    #[test]
    fn forecast_on_a_linear_series_is_within_a_day() {
        // 10_000 units a day; 600_000 used now of 1_000_000: 40 days left.
        let series: Vec<Snapshot> = [60, 40, 20, 0]
            .iter()
            .map(|d| drive(NOW - d * DAY_MS, 600_000 - d * 10_000))
            .collect();
        let Forecast::Full {
            days,
            per_day,
            window_days,
            points,
        } = forecast(&series, NOW)
        else {
            panic!("no forecast")
        };
        assert!(days.abs_diff(40) <= 1, "{days}");
        assert_eq!(per_day, 10_000);
        assert_eq!((window_days, points), (60, 4));
        // Order does not matter.
        let mut reversed = series.clone();
        reversed.reverse();
        assert_eq!(forecast(&reversed, NOW), forecast(&series, NOW));
    }

    #[test]
    fn forecast_is_hidden_beyond_five_years_and_zero_when_already_full() {
        // 1 unit a day against 1_000_000 of room: centuries.
        let slow: Vec<Snapshot> = [60, 30, 0]
            .iter()
            .map(|d| drive(NOW - d * DAY_MS, 1000 - d))
            .collect();
        assert_eq!(forecast(&slow, NOW), Forecast::Hidden);
        let full: Vec<Snapshot> = [60, 30, 0]
            .iter()
            .map(|d| drive(NOW - d * DAY_MS, 1_000_000 - d * 10))
            .collect();
        assert!(matches!(
            forecast(&full, NOW),
            Forecast::Full { days: 0, .. }
        ));
    }
}
