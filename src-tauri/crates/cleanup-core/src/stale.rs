//! The stale data report: how much of a scan has not been modified in N years, per first-level
//! folder under the scan root. Everything comes from the scan's metadata; no file is opened.
//!
//! The modified time is the rule (D-14). The last-access time is an optional extra filter, never
//! the default, because Windows often does not keep it up to date ([`last_access_updates`]).

use scan_core::{Kind, Node, NodeId, Tree};
use serde::Serialize;

/// A year as 365.25 days, so a threshold of N years does not drift with leap years.
pub const YEAR_MS: u64 = 31_557_600_000;
pub const DEFAULT_YEARS: u32 = 3;
pub const YEAR_CHOICES: [u32; 6] = [1, 2, 3, 5, 7, 10];
/// 1980-01-01 UTC. An earlier time is unset or wrong (the FAT epoch), not old data.
pub const EARLIEST_MS: u64 = 315_532_800_000;
/// Stale files listed per folder when a row is opened.
pub const FILES_LIMIT: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub years: u32,
    /// Also require the last-access time to be older than the threshold.
    pub not_opened: bool,
}

impl Options {
    fn cutoff(self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(u64::from(self.years) * YEAR_MS)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Stale,
    Fresh,
    /// Missing, in the future, or before 1980: counted apart, never as stale.
    Unknown,
}

fn known(time: Option<u64>, now_ms: u64) -> Option<u64> {
    time.filter(|&time| (EARLIEST_MS..=now_ms).contains(&time))
}

/// Whether a file with these times is stale at `now_ms`. Exactly N years old is stale.
pub fn verdict(
    modified_ms: Option<u64>,
    accessed_ms: Option<u64>,
    now_ms: u64,
    options: Options,
) -> Verdict {
    let cutoff = options.cutoff(now_ms);
    let Some(modified) = known(modified_ms, now_ms) else {
        return Verdict::Unknown;
    };
    if modified > cutoff {
        return Verdict::Fresh;
    }
    if !options.not_opened {
        return Verdict::Stale;
    }
    match known(accessed_ms, now_ms) {
        None => Verdict::Unknown,
        Some(accessed) if accessed > cutoff => Verdict::Fresh,
        Some(_) => Verdict::Stale,
    }
}

/// One first-level folder (or the files directly in the root) and its stale part.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleGroup {
    /// The folder's id in the scan; the root's id (0) for the files directly in the root.
    pub id: NodeId,
    pub name: String,
    pub is_folder: bool,
    pub stale_files: u64,
    /// Space on disk of the stale files.
    pub stale_bytes: u64,
    /// Space on disk of the whole folder (or of the root's own files).
    pub folder_bytes: u64,
    /// The most recent modified time among the stale files.
    pub newest_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleReport {
    pub years: u32,
    pub not_opened: bool,
    pub now_ms: u64,
    /// Groups with stale files, most stale space first.
    pub groups: Vec<StaleGroup>,
    pub stale_files: u64,
    pub stale_bytes: u64,
    /// Files whose date is missing, in the future or before 1980.
    pub unknown_files: u64,
    pub unknown_bytes: u64,
    /// Online-only files: not on this disk, so not counted.
    pub cloud_files: u64,
    /// Space on disk of the whole scan.
    pub total_bytes: u64,
    pub last_access: LastAccess,
}

#[derive(Default)]
struct Totals {
    stale_files: u64,
    stale_bytes: u64,
    unknown_files: u64,
    unknown_bytes: u64,
    cloud_files: u64,
    newest_ms: Option<u64>,
}

impl Totals {
    fn add(&mut self, node: &Node, now_ms: u64, options: Options) {
        if node.kind != Kind::File {
            return;
        }
        if node.cloud_files > 0 {
            self.cloud_files += 1;
            return;
        }
        match verdict(node.modified_ms, node.accessed_ms, now_ms, options) {
            Verdict::Stale => {
                self.stale_files += 1;
                self.stale_bytes += node.disk;
                self.newest_ms = self.newest_ms.max(node.modified_ms);
            }
            Verdict::Unknown => {
                self.unknown_files += 1;
                self.unknown_bytes += node.disk;
            }
            Verdict::Fresh => {}
        }
    }
}

/// Every live file at or below `id`, links never entered.
fn files_below(tree: &Tree, id: NodeId, mut each: impl FnMut(NodeId, &Node)) {
    let mut stack = vec![id];
    while let Some(at) = stack.pop() {
        for child in tree.live_children(at) {
            let node = tree.node(child);
            match node.kind {
                Kind::Dir => stack.push(child),
                Kind::File => each(child, node),
                Kind::Link => {}
            }
        }
    }
}

/// The stale report for `tree` at `now_ms`.
pub fn analyze(tree: &Tree, now_ms: u64, options: Options, last_access: LastAccess) -> StaleReport {
    let mut groups = Vec::new();
    let mut all = Totals::default();
    let mut root_files = Totals::default();
    let mut root_bytes = 0;
    for child in tree.live_children(0) {
        let node = tree.node(child);
        match node.kind {
            Kind::File => {
                root_files.add(node, now_ms, options);
                root_bytes += node.disk;
            }
            Kind::Dir => {
                let mut totals = Totals::default();
                files_below(tree, child, |_, file| totals.add(file, now_ms, options));
                merge(&mut all, &totals);
                if totals.stale_files > 0 {
                    groups.push(StaleGroup {
                        id: child,
                        name: node.name.clone(),
                        is_folder: true,
                        stale_files: totals.stale_files,
                        stale_bytes: totals.stale_bytes,
                        folder_bytes: node.disk,
                        newest_ms: totals.newest_ms,
                    });
                }
            }
            Kind::Link => {}
        }
    }
    merge(&mut all, &root_files);
    if root_files.stale_files > 0 {
        groups.push(StaleGroup {
            id: 0,
            name: ROOT_FILES.into(),
            is_folder: false,
            stale_files: root_files.stale_files,
            stale_bytes: root_files.stale_bytes,
            folder_bytes: root_bytes,
            newest_ms: root_files.newest_ms,
        });
    }
    groups.sort_by(|a, b| {
        b.stale_bytes
            .cmp(&a.stale_bytes)
            .then_with(|| a.name.cmp(&b.name))
    });
    StaleReport {
        years: options.years,
        not_opened: options.not_opened,
        now_ms,
        groups,
        stale_files: all.stale_files,
        stale_bytes: all.stale_bytes,
        unknown_files: all.unknown_files,
        unknown_bytes: all.unknown_bytes,
        cloud_files: all.cloud_files,
        total_bytes: tree.root().disk,
        last_access,
    }
}

/// The name of the group of files directly in the scan root.
pub const ROOT_FILES: &str = "Files in the scanned folder itself";

fn merge(into: &mut Totals, from: &Totals) {
    into.stale_files += from.stale_files;
    into.stale_bytes += from.stale_bytes;
    into.unknown_files += from.unknown_files;
    into.unknown_bytes += from.unknown_bytes;
    into.cloud_files += from.cloud_files;
    into.newest_ms = into.newest_ms.max(from.newest_ms);
}

/// A stale file, for the list under a group.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleFile {
    pub id: NodeId,
    /// The path below the scan root.
    pub path: String,
    pub size: u64,
    pub modified_ms: Option<u64>,
    pub accessed_ms: Option<u64>,
}

/// The largest stale files of one group (see [`StaleGroup::id`]), at most `limit`.
pub fn stale_files(
    tree: &Tree,
    group: NodeId,
    now_ms: u64,
    options: Options,
    limit: usize,
) -> Vec<StaleFile> {
    let mut files = Vec::new();
    let mut keep = |id: NodeId, node: &Node| {
        if node.cloud_files == 0
            && verdict(node.modified_ms, node.accessed_ms, now_ms, options) == Verdict::Stale
        {
            files.push((id, node.disk));
        }
    };
    if group == 0 {
        for child in tree.live_children(0) {
            let node = tree.node(child);
            if node.kind == Kind::File {
                keep(child, node);
            }
        }
    } else if tree.is_live(group) && tree.parent(group) == Some(0) {
        files_below(tree, group, keep);
    }
    files.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    files.truncate(limit);
    let root = tree.path(0);
    files
        .into_iter()
        .map(|(id, size)| {
            let node = tree.node(id);
            let path = tree.path(id);
            StaleFile {
                id,
                path: path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                size,
                modified_ms: node.modified_ms,
                accessed_ms: node.accessed_ms,
            }
        })
        .collect()
}

/// Whether Windows updates last-access times on NTFS (`NtfsDisableLastAccessUpdate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LastAccess {
    Kept,
    Off,
    /// Not Windows, or the setting could not be read.
    Unknown,
}

/// Reads the setting from its registry DWORD. Bit 0 set means updates are off; the high bit only
/// says whether Windows or the user manages it (`0x80000003` is "system managed, off").
pub fn last_access_from(value: Option<u32>) -> LastAccess {
    match value {
        None => LastAccess::Unknown,
        Some(value) if value & 1 == 1 => LastAccess::Off,
        Some(_) => LastAccess::Kept,
    }
}

/// This PC's last-access setting.
pub fn last_access_updates() -> LastAccess {
    last_access_from(registry_value())
}

#[cfg(windows)]
fn registry_value() -> Option<u32> {
    use std::ffi::c_void;
    const HKEY_LOCAL_MACHINE: isize = -2_147_483_646; // 0x80000002 sign-extended, as Windows defines it
    const RRF_RT_REG_DWORD: u32 = 0x10;
    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(
            key: isize,
            sub_key: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            size: *mut u32,
        ) -> i32;
    }
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() };
    let sub_key = wide(r"SYSTEM\CurrentControlSet\Control\FileSystem");
    let name = wide("NtfsDisableLastAccessUpdate");
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both strings are NUL-terminated and outlive the call; `data` holds the 4 bytes
    // `size` says, and RRF_RT_REG_DWORD refuses any other value type.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub_key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    (status == 0).then_some(data)
}

#[cfg(not(windows))]
fn registry_value() -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use scan_core::{scan, ScanState};
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    /// 2026-10-24, the fixed "now" of these tests.
    const NOW: u64 = 1_792_800_000_000;
    const THREE: Options = Options {
        years: 3,
        not_opened: false,
    };
    const THREE_UNOPENED: Options = Options {
        years: 3,
        not_opened: true,
    };

    fn years_ago(years: u64) -> u64 {
        NOW - years * YEAR_MS
    }

    #[test]
    fn exactly_the_threshold_is_stale_and_a_millisecond_less_is_not() {
        let cutoff = years_ago(3);
        assert_eq!(verdict(Some(cutoff), None, NOW, THREE), Verdict::Stale);
        assert_eq!(verdict(Some(cutoff + 1), None, NOW, THREE), Verdict::Fresh);
        assert_eq!(verdict(Some(cutoff - 1), None, NOW, THREE), Verdict::Stale);
        for years in YEAR_CHOICES {
            let options = Options {
                years,
                not_opened: false,
            };
            let at = years_ago(u64::from(years));
            assert_eq!(verdict(Some(at), None, NOW, options), Verdict::Stale);
            assert_eq!(verdict(Some(at + 1), None, NOW, options), Verdict::Fresh);
        }
    }

    #[test]
    fn future_missing_and_pre_1980_dates_are_unknown_not_stale() {
        assert_eq!(verdict(None, None, NOW, THREE), Verdict::Unknown);
        assert_eq!(verdict(Some(NOW + 1), None, NOW, THREE), Verdict::Unknown);
        assert_eq!(
            verdict(Some(EARLIEST_MS - 1), None, NOW, THREE),
            Verdict::Unknown
        );
        assert_eq!(verdict(Some(EARLIEST_MS), None, NOW, THREE), Verdict::Stale);
        assert_eq!(verdict(Some(NOW), None, NOW, THREE), Verdict::Fresh);
    }

    #[test]
    fn the_access_filter_is_off_by_default_and_only_narrows_when_on() {
        let old = Some(years_ago(5));
        let opened_lately = Some(NOW - 1000);
        // Off: the access time does not matter.
        assert_eq!(verdict(old, opened_lately, NOW, THREE), Verdict::Stale);
        assert_eq!(verdict(old, None, NOW, THREE), Verdict::Stale);
        // On: opened within the threshold is not stale, an old open is, a missing one is unknown.
        assert_eq!(
            verdict(old, opened_lately, NOW, THREE_UNOPENED),
            Verdict::Fresh
        );
        assert_eq!(verdict(old, old, NOW, THREE_UNOPENED), Verdict::Stale);
        assert_eq!(verdict(old, None, NOW, THREE_UNOPENED), Verdict::Unknown);
        assert_eq!(
            verdict(old, Some(NOW + 5), NOW, THREE_UNOPENED),
            Verdict::Unknown
        );
        // A recent change is fresh whatever the access time says.
        assert_eq!(
            verdict(Some(NOW - 1000), old, NOW, THREE_UNOPENED),
            Verdict::Fresh
        );
    }

    fn write(root: &Path, name: &str, len: usize, modified: u64, accessed: u64) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![1_u8; len]).unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(UNIX_EPOCH + Duration::from_millis(modified))
                    .set_accessed(UNIX_EPOCH + Duration::from_millis(accessed)),
            )
            .unwrap();
    }

    #[test]
    fn groups_by_first_level_folder_with_the_root_files_apart() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (old, new) = (years_ago(4), NOW - 1000);
        write(root, "Finance/2019/a.xlsx", 50_000, old, old);
        write(root, "Finance/2019/deep/b.xlsx", 30_000, old - 1000, old);
        write(root, "Finance/current.xlsx", 20_000, new, new);
        write(root, "HR/fresh.docx", 10_000, new, new);
        write(root, "Archive/c.zip", 90_000, years_ago(8), new);
        write(root, "readme.txt", 5_000, old, old);
        write(root, "future.txt", 5_000, NOW + YEAR_MS, NOW);
        let tree = scan(root, &ScanState::default()).unwrap();
        let disk = |path: &[&str]| {
            let id = path
                .iter()
                .fold(0, |at, name| tree.child_by_name(at, name).unwrap());
            tree.node(id).disk
        };

        let report = analyze(&tree, NOW, THREE, LastAccess::Unknown);
        let names: Vec<&str> = report.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            ["Archive", "Finance", ROOT_FILES],
            "HR has nothing stale"
        );
        let finance = &report.groups[1];
        assert_eq!(finance.id, tree.child_by_name(0, "Finance").unwrap());
        assert_eq!(finance.stale_files, 2);
        assert_eq!(
            finance.stale_bytes,
            disk(&["Finance", "2019", "a.xlsx"]) + disk(&["Finance", "2019", "deep", "b.xlsx"])
        );
        assert_eq!(finance.folder_bytes, disk(&["Finance"]));
        assert_eq!(finance.newest_ms, Some(old));
        let loose = &report.groups[2];
        assert_eq!(
            (loose.id, loose.is_folder, loose.stale_files),
            (0, false, 1)
        );
        assert_eq!(report.stale_files, 4);
        assert_eq!(report.unknown_files, 1, "the file dated in the future");

        // "Also not opened": the archive was opened lately, so it drops out.
        let unopened = analyze(&tree, NOW, THREE_UNOPENED, LastAccess::Kept);
        let names: Vec<&str> = unopened.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Finance", ROOT_FILES]);
        assert_eq!(unopened.last_access, LastAccess::Kept);
    }

    #[test]
    fn a_groups_files_are_listed_largest_first_and_only_stale_ones() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (old, new) = (years_ago(4), NOW - 1000);
        write(root, "Finance/small.xlsx", 10_000, old, old);
        write(root, "Finance/sub/big.xlsx", 80_000, old, old);
        write(root, "Finance/new.xlsx", 90_000, new, new);
        write(root, "loose.txt", 5_000, old, old);
        let tree = scan(root, &ScanState::default()).unwrap();
        let finance = tree.child_by_name(0, "Finance").unwrap();

        let files = stale_files(&tree, finance, NOW, THREE, FILES_LIMIT);
        let paths: Vec<String> = files.iter().map(|f| f.path.replace('\\', "/")).collect();
        assert_eq!(paths, ["Finance/sub/big.xlsx", "Finance/small.xlsx"]);
        assert_eq!(stale_files(&tree, finance, NOW, THREE, 1).len(), 1);

        let loose = stale_files(&tree, 0, NOW, THREE, FILES_LIMIT);
        assert_eq!(loose.len(), 1);
        assert_eq!(loose[0].path, "loose.txt");
        // Only first-level folders are groups.
        let sub = tree.child_by_name(finance, "sub").unwrap();
        assert!(stale_files(&tree, sub, NOW, THREE, FILES_LIMIT).is_empty());
    }

    #[test]
    fn online_only_files_are_counted_apart_and_never_listed() {
        let temp = tempfile::tempdir().unwrap();
        let old = years_ago(4);
        write(temp.path(), "a.txt", 5_000, old, old);
        let tree = scan(temp.path(), &ScanState::default()).unwrap();
        let mut cloud = tree.node(tree.child_by_name(0, "a.txt").unwrap()).clone();
        cloud.cloud_files = 1;
        let mut totals = Totals::default();
        totals.add(&cloud, NOW, THREE);
        assert_eq!((totals.cloud_files, totals.stale_files), (1, 0));
    }

    #[test]
    fn the_last_access_setting_reads_bit_zero() {
        assert_eq!(last_access_from(None), LastAccess::Unknown);
        assert_eq!(last_access_from(Some(0)), LastAccess::Kept);
        assert_eq!(last_access_from(Some(1)), LastAccess::Off);
        assert_eq!(last_access_from(Some(0x8000_0002)), LastAccess::Kept);
        assert_eq!(last_access_from(Some(0x8000_0003)), LastAccess::Off);
        assert_eq!(last_access_from(Some(0x8000_0001)), LastAccess::Off);
    }

    #[cfg(windows)]
    #[test]
    fn the_last_access_setting_is_read_on_windows() {
        // Every supported Windows has the value; it decides between Kept and Off.
        assert_ne!(last_access_updates(), LastAccess::Unknown);
    }
}
