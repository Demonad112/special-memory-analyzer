//! The duplicate finder: files in a scan with identical content, found in three passes that each
//! read more than the last (same size, then the first and last 64 KiB, then a full BLAKE3 hash),
//! and the checks a delete of duplicates must pass before anything goes to [`crate::bulk::run`].
//!
//! Only plain files are read. Links, online-only (OneDrive) placeholders, names that are not
//! valid Unicode and hard-linked names are never opened: reading a placeholder downloads it, a
//! link points somewhere else, and two names of one hard-linked file are the same data, not two
//! copies of it.

use crate::pathsafe::is_reparse_point;
use scan_core::hash::{hash_file, hash_head_tail, HEAD_TAIL_BYTES};
use scan_core::{CancelToken, Kind, Node, NodeId, Tree};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
/// The smallest file looked at unless the user picks another size (D-12).
pub const DEFAULT_MIN_SIZE: u64 = MIB;
/// The minimum sizes the panel offers.
pub const MIN_SIZE_CHOICES: [u64; 4] = [64 * KIB, MIB, 10 * MIB, 100 * MIB];

/// One file of a duplicate group, as the scan and the search saw it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub id: NodeId,
    pub name: String,
    /// The folder the file is in.
    pub folder: String,
    #[serde(skip)]
    pub path: PathBuf,
    pub modified_ms: Option<u64>,
    /// Space on disk for this name.
    pub disk: u64,
}

/// Files with the same content.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// Position in [`Found::groups`] when the search finished; stable until the next search.
    pub id: u32,
    /// Length of each file in bytes.
    pub size: u64,
    #[serde(skip)]
    pub hash: String,
    /// Space freed by keeping one copy: all copies but the largest on disk.
    pub reclaimable: u64,
    /// Oldest first (by modified time; unknown times last), then by path.
    pub members: Vec<Member>,
}

/// Files the search left out, and why.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    /// Online-only placeholders: never read.
    pub cloud: u64,
    /// Names of a hard-linked file: the same data, not a copy.
    pub hard_linked: u64,
    /// Files that could not be read while hashing.
    pub unreadable: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub min_size: u64,
    /// Largest [`Group::reclaimable`] first.
    pub groups: Vec<Group>,
    pub skipped: Skipped,
}

impl Found {
    pub fn files(&self) -> u64 {
        self.groups
            .iter()
            .map(|group| group.members.len() as u64)
            .sum()
    }

    pub fn reclaimable(&self) -> u64 {
        self.groups.iter().map(|group| group.reclaimable).sum()
    }

    /// Drops deleted files from their groups; a group left with one file is no longer a group.
    pub fn forget(&mut self, gone: &HashSet<NodeId>) {
        for group in &mut self.groups {
            group.members.retain(|member| !gone.contains(&member.id));
            group.reclaimable = reclaimable(&group.members);
        }
        self.groups.retain(|group| group.members.len() > 1);
        self.groups
            .sort_by(|a, b| b.reclaimable.cmp(&a.reclaimable).then(a.id.cmp(&b.id)));
    }
}

/// Which pass the search is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    /// Listing same-size files from the scan.
    Sizes,
    /// Reading the first and last 64 KiB of each.
    Ends,
    /// Reading whole files.
    Full,
    Done,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub stage: Stage,
    /// Files finished in this stage, and how many it has.
    pub done: u64,
    pub total: u64,
    /// Bytes read so far, all stages.
    pub bytes: u64,
}

/// Counters the search updates and the command reads every 100 ms.
#[derive(Debug, Default)]
pub struct Counters {
    stage: AtomicU8,
    done: AtomicU64,
    total: AtomicU64,
    bytes: AtomicU64,
}

impl Counters {
    pub fn progress(&self) -> Progress {
        Progress {
            stage: match self.stage.load(Ordering::Relaxed) {
                0 => Stage::Sizes,
                1 => Stage::Ends,
                2 => Stage::Full,
                _ => Stage::Done,
            },
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
        }
    }

    fn enter(&self, stage: Stage, total: u64) {
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.stage.store(stage as u8, Ordering::Relaxed);
    }

    fn step(&self, bytes: u64) {
        self.done.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }
}

/// Why a scanned file is or is not read by the search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eligible {
    Yes,
    /// Not a plain file, a bad name, or smaller than the minimum: not counted anywhere.
    No,
    Cloud,
    HardLinked,
}

/// Decides from the scan alone whether `node` may be read. Nothing here touches the disk.
pub fn eligible(node: &Node, min_size: u64) -> Eligible {
    if node.kind != Kind::File || node.lossy_name || node.size < min_size.max(1) {
        Eligible::No
    } else if node.cloud_files > 0 {
        Eligible::Cloud
    } else if node.hard_linked {
        Eligible::HardLinked
    } else {
        Eligible::Yes
    }
}

/// Same-size files from the scan, sorted into sets. Taken under the tree lock; the reads that
/// follow need no tree.
#[derive(Clone, Debug, Default)]
pub struct Candidates {
    pub min_size: u64,
    /// Files of one length per set; sets of one file are dropped.
    pub sets: Vec<(u64, Vec<Member>)>,
    pub skipped: Skipped,
}

/// Pass 1: the live files of `tree` of at least `min_size` bytes that share a length with
/// another. Reads nothing from the disk.
pub fn collect(tree: &Tree, min_size: u64) -> Candidates {
    let mut skipped = Skipped::default();
    let mut by_size: HashMap<u64, Vec<NodeId>> = HashMap::new();
    let mut stack = vec![0];
    while let Some(at) = stack.pop() {
        for child in tree.live_children(at) {
            let node = tree.node(child);
            if node.kind == Kind::Dir {
                stack.push(child);
                continue;
            }
            match eligible(node, min_size) {
                Eligible::Yes => by_size.entry(node.size).or_default().push(child),
                Eligible::Cloud => skipped.cloud += 1,
                Eligible::HardLinked => skipped.hard_linked += 1,
                Eligible::No => {}
            }
        }
    }
    let sets = by_size
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|(size, ids)| (size, ids.iter().map(|&id| member(tree, id)).collect()))
        .collect();
    Candidates {
        min_size,
        sets,
        skipped,
    }
}

fn member(tree: &Tree, id: NodeId) -> Member {
    let node = tree.node(id);
    let path = tree.path(id);
    Member {
        id,
        name: node.name.clone(),
        folder: path
            .parent()
            .map(|folder| folder.display().to_string())
            .unwrap_or_default(),
        path,
        modified_ms: node.modified_ms,
        disk: node.disk,
    }
}

/// Passes 2 and 3: reads the ends of each candidate, then whole files where the ends match.
/// `None` when cancelled.
pub fn search(candidates: Candidates, cancel: &CancelToken, counters: &Counters) -> Option<Found> {
    let Candidates {
        min_size,
        sets,
        mut skipped,
    } = candidates;

    counters.enter(
        Stage::Ends,
        sets.iter().map(|(_, set)| set.len() as u64).sum(),
    );
    let mut by_ends: Vec<(u64, Vec<Member>)> = Vec::new();
    for (size, set) in sets {
        let mut buckets: HashMap<String, Vec<Member>> = HashMap::new();
        for member in set {
            if cancel.is_cancelled() {
                return None;
            }
            match hash_head_tail(&member.path, HEAD_TAIL_BYTES) {
                Ok(hash) => buckets.entry(hash).or_default().push(member),
                Err(_) => skipped.unreadable += 1,
            }
            counters.step(size.min(HEAD_TAIL_BYTES * 2));
        }
        by_ends.extend(
            buckets
                .into_values()
                .filter(|set| set.len() > 1)
                .map(|set| (size, set)),
        );
    }

    counters.enter(
        Stage::Full,
        by_ends.iter().map(|(_, set)| set.len() as u64).sum(),
    );
    let mut groups = Vec::new();
    for (size, set) in by_ends {
        let mut buckets: HashMap<String, Vec<Member>> = HashMap::new();
        for member in set {
            match hash_file(&member.path, cancel) {
                Ok(hash) => buckets.entry(hash).or_default().push(member),
                Err(_) if cancel.is_cancelled() => return None,
                Err(_) => skipped.unreadable += 1,
            }
            counters.step(size);
        }
        for (hash, mut members) in buckets.into_iter().filter(|(_, set)| set.len() > 1) {
            members.sort_by(|a, b| {
                oldest_first(a.modified_ms, b.modified_ms).then_with(|| a.path.cmp(&b.path))
            });
            groups.push(Group {
                id: 0,
                size,
                hash,
                reclaimable: reclaimable(&members),
                members,
            });
        }
    }
    if cancel.is_cancelled() {
        return None;
    }
    groups.sort_by(|a, b| {
        b.reclaimable
            .cmp(&a.reclaimable)
            .then_with(|| a.members[0].path.cmp(&b.members[0].path))
    });
    for (at, group) in groups.iter_mut().enumerate() {
        group.id = u32::try_from(at).unwrap_or(u32::MAX);
    }
    counters.enter(Stage::Done, 0);
    Some(Found {
        min_size,
        groups,
        skipped,
    })
}

/// [`collect`] then [`search`], for callers that can hold the tree throughout.
pub fn find(
    tree: &Tree,
    min_size: u64,
    cancel: &CancelToken,
    counters: &Counters,
) -> Option<Found> {
    counters.enter(Stage::Sizes, 0);
    search(collect(tree, min_size), cancel, counters)
}

fn oldest_first(a: Option<u64>, b: Option<u64>) -> std::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(&b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

fn reclaimable(members: &[Member]) -> u64 {
    let total: u64 = members.iter().map(|member| member.disk).sum();
    let largest = members.iter().map(|member| member.disk).max().unwrap_or(0);
    total - largest
}

/// The ids "Select all but the oldest in each group" ticks: every member but the first, since
/// members are sorted oldest first.
pub fn all_but_oldest(found: &Found) -> Vec<NodeId> {
    found
        .groups
        .iter()
        .flat_map(|group| group.members.iter().skip(1).map(|member| member.id))
        .collect()
}

/// Checks a delete of `ids` against the search before anything is removed (D-12):
///
/// - every id is a file of a group;
/// - each group keeps at least one file that is not in `ids`, whatever the webview sent;
/// - one kept file of each group is still a plain file with the group's content (re-hashed now);
/// - each file to delete still has the size and modified time the search saw.
///
/// Returns the ids to hand to the delete pipeline, in the order given and without repeats.
pub fn check_delete(
    found: &Found,
    ids: &[NodeId],
    cancel: &CancelToken,
) -> Result<Vec<NodeId>, String> {
    let index: HashMap<NodeId, usize> = found
        .groups
        .iter()
        .enumerate()
        .flat_map(|(at, group)| group.members.iter().map(move |member| (member.id, at)))
        .collect();
    let mut seen = HashSet::new();
    let mut picked = Vec::new();
    let mut touched: Vec<usize> = Vec::new();
    for &id in ids {
        if !seen.insert(id) {
            continue;
        }
        let at = *index.get(&id).ok_or_else(|| {
            "A selected file is not in the duplicate list any more. Find duplicates again."
                .to_string()
        })?;
        if !touched.contains(&at) {
            touched.push(at);
        }
        picked.push(id);
    }
    if picked.is_empty() {
        return Err("Nothing is selected.".into());
    }
    for &at in &touched {
        let group = &found.groups[at];
        let kept: Vec<&Member> = group
            .members
            .iter()
            .filter(|member| !seen.contains(&member.id))
            .collect();
        if kept.is_empty() {
            return Err(format!(
                "Every copy of {} is selected. Keep at least one copy in each group; nothing was deleted.",
                group.members[0].name
            ));
        }
        if cancel.is_cancelled() {
            return Err("Cancelled".into());
        }
        if !kept.iter().any(|member| still_holds(member, group, cancel)) {
            return Err(format!(
                "The copy of {} you are keeping changed or is gone since the search, so nothing was deleted. Find duplicates again.",
                group.members[0].name
            ));
        }
        for member in group
            .members
            .iter()
            .filter(|member| seen.contains(&member.id))
        {
            if !unchanged(&member.path, group.size, member.modified_ms) {
                return Err(format!(
                    "{} changed since the search, so nothing was deleted. Find duplicates again.",
                    member.path.display()
                ));
            }
        }
    }
    Ok(picked)
}

/// The kept file is still a plain file of the group's size and content.
fn still_holds(member: &Member, group: &Group, cancel: &CancelToken) -> bool {
    !is_reparse_point(&member.path)
        && fs::symlink_metadata(&member.path)
            .is_ok_and(|meta| meta.is_file() && meta.len() == group.size)
        && hash_file(&member.path, cancel).is_ok_and(|hash| hash == group.hash)
}

/// The file to delete is still a plain file with the size and modified time the scan saw.
fn unchanged(path: &Path, size: u64, modified_ms: Option<u64>) -> bool {
    if is_reparse_point(path) {
        return false;
    }
    fs::symlink_metadata(path).is_ok_and(|meta| {
        meta.is_file() && meta.len() == size && epoch_ms(meta.modified()) == modified_ms
    })
}

/// A file time as ms since 1970, the way the scan reads it.
fn epoch_ms(time: std::io::Result<std::time::SystemTime>) -> Option<u64> {
    time.ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use scan_core::{scan, ScanState};

    fn write(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn content(seed: u8, len: usize) -> Vec<u8> {
        (0..len)
            .map(|at| (at as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    fn search(root: &Path, min_size: u64) -> (Tree, Found) {
        let tree = scan(root, &ScanState::default()).unwrap();
        let found = find(
            &tree,
            min_size,
            &CancelToken::default(),
            &Counters::default(),
        )
        .unwrap();
        (tree, found)
    }

    fn names(found: &Found) -> Vec<Vec<String>> {
        found
            .groups
            .iter()
            .map(|group| {
                let mut names: Vec<String> = group
                    .members
                    .iter()
                    .map(|member| member.name.clone())
                    .collect();
                names.sort();
                names
            })
            .collect()
    }

    #[test]
    fn three_equal_files_make_one_group() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = content(1, 300_000);
        write(temp.path(), "a.iso", &bytes);
        write(temp.path(), "x/b.iso", &bytes);
        write(temp.path(), "y/z/c.iso", &bytes);
        write(temp.path(), "other.iso", &content(2, 300_000));

        let (_, found) = search(temp.path(), 64 * KIB);
        assert_eq!(names(&found), [["a.iso", "b.iso", "c.iso"]]);
        let group = &found.groups[0];
        assert_eq!(group.size, 300_000);
        let disk: u64 = group.members.iter().map(|member| member.disk).sum();
        let largest = group
            .members
            .iter()
            .map(|member| member.disk)
            .max()
            .unwrap();
        assert_eq!(group.reclaimable, disk - largest);
        assert_eq!(found.files(), 3);
    }

    #[test]
    fn same_size_with_different_content_makes_no_group() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "a.bin", &content(1, 100_000));
        write(temp.path(), "b.bin", &content(2, 100_000));
        let (_, found) = search(temp.path(), 64 * KIB);
        assert!(found.groups.is_empty());
    }

    #[test]
    fn equal_ends_with_a_different_middle_make_no_group() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = content(1, 400_000);
        let mut middle = bytes.clone();
        middle[200_000] ^= 0xFF;
        write(temp.path(), "a.bin", &bytes);
        write(temp.path(), "b.bin", &middle);
        let counters = Counters::default();
        let tree = scan(temp.path(), &ScanState::default()).unwrap();
        let found = find(&tree, 64 * KIB, &CancelToken::default(), &counters).unwrap();
        assert!(found.groups.is_empty());
        // Both reached the full pass: the ends alone could not tell them apart.
        assert_eq!(counters.progress().stage, Stage::Done);
        assert_eq!(
            counters.progress().bytes,
            2 * (2 * HEAD_TAIL_BYTES) + 2 * 400_000
        );
    }

    #[test]
    fn files_under_the_minimum_are_not_read() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = content(1, 100_000);
        write(temp.path(), "a.bin", &bytes);
        write(temp.path(), "b.bin", &bytes);
        let (_, found) = search(temp.path(), MIB);
        assert!(found.groups.is_empty());
        let (_, found) = search(temp.path(), 64 * KIB);
        assert_eq!(found.groups.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_and_links_make_no_group() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = content(1, 100_000);
        let original = write(temp.path(), "a/original.bin", &bytes);
        fs::hard_link(&original, temp.path().join("hard.bin")).unwrap();
        std::os::unix::fs::symlink(&original, temp.path().join("soft.bin")).unwrap();

        let (_, found) = search(temp.path(), 64 * KIB);
        assert!(found.groups.is_empty(), "{:?}", names(&found));
        assert_eq!(found.skipped.hard_linked, 2);
    }

    #[test]
    fn online_only_links_and_bad_names_are_never_eligible() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "a.bin", &content(1, 100_000));
        let tree = scan(temp.path(), &ScanState::default()).unwrap();
        let file = tree.node(tree.child_by_name(0, "a.bin").unwrap()).clone();
        assert_eq!(eligible(&file, 64 * KIB), Eligible::Yes);

        let mut cloud = file.clone();
        cloud.cloud_files = 1;
        assert_eq!(eligible(&cloud, 64 * KIB), Eligible::Cloud);
        let mut link = file.clone();
        link.kind = Kind::Link;
        assert_eq!(eligible(&link, 64 * KIB), Eligible::No);
        let mut lossy = file.clone();
        lossy.lossy_name = true;
        assert_eq!(eligible(&lossy, 64 * KIB), Eligible::No);
        let mut linked = file.clone();
        linked.hard_linked = true;
        assert_eq!(eligible(&linked, 64 * KIB), Eligible::HardLinked);
        let mut empty = file;
        empty.size = 0;
        assert_eq!(
            eligible(&empty, 0),
            Eligible::No,
            "empty files are never duplicates"
        );
    }

    #[test]
    fn a_cancelled_search_returns_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = content(1, 100_000);
        write(temp.path(), "a.bin", &bytes);
        write(temp.path(), "b.bin", &bytes);
        let tree = scan(temp.path(), &ScanState::default()).unwrap();
        let cancel = CancelToken::default();
        cancel.cancel();
        assert!(find(&tree, 64 * KIB, &cancel, &Counters::default()).is_none());
    }

    #[test]
    fn members_are_oldest_first_and_all_but_oldest_skips_the_first() {
        use std::time::{Duration, UNIX_EPOCH};
        let temp = tempfile::tempdir().unwrap();
        let bytes = content(1, 100_000);
        for (name, ms) in [
            ("new.bin", 1_600_000_000_000),
            ("old.bin", 1_400_000_000_000),
        ] {
            let path = write(temp.path(), name, &bytes);
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_millis(ms))
                .unwrap();
        }
        let (tree, found) = search(temp.path(), 64 * KIB);
        let members: Vec<&str> = found.groups[0]
            .members
            .iter()
            .map(|member| member.name.as_str())
            .collect();
        assert_eq!(members, ["old.bin", "new.bin"]);
        assert_eq!(
            all_but_oldest(&found),
            [tree.child_by_name(0, "new.bin").unwrap()]
        );
    }

    fn ids(found: &Found) -> Vec<NodeId> {
        found.groups[0]
            .members
            .iter()
            .map(|member| member.id)
            .collect()
    }

    #[test]
    fn deleting_every_copy_of_a_group_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = content(1, 100_000);
        write(temp.path(), "a.bin", &bytes);
        write(temp.path(), "b.bin", &bytes);
        write(temp.path(), "c.bin", &bytes);
        let (_, found) = search(temp.path(), 64 * KIB);

        let every = ids(&found);
        let err = check_delete(&found, &every, &CancelToken::default()).unwrap_err();
        assert!(err.contains("Every copy"), "{err}");
        // Repeats do not hide a missing keeper either.
        let doubled: Vec<NodeId> = every.iter().chain(every.iter()).copied().collect();
        assert!(check_delete(&found, &doubled, &CancelToken::default()).is_err());

        let two = check_delete(&found, &every[1..], &CancelToken::default()).unwrap();
        assert_eq!(two, &every[1..]);
    }

    #[test]
    fn a_changed_kept_copy_stops_the_delete() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = content(1, 100_000);
        write(temp.path(), "a.bin", &bytes);
        write(temp.path(), "b.bin", &bytes);
        let (_, found) = search(temp.path(), 64 * KIB);
        let [_, drop] = ids(&found)[..] else { panic!() };
        let kept = &found.groups[0].members[0].path;

        // Same size, different content: only the re-hash notices.
        let mut changed = bytes.clone();
        changed[50_000] ^= 0xFF;
        fs::write(kept, &changed).unwrap();
        let err = check_delete(&found, &[drop], &CancelToken::default()).unwrap_err();
        assert!(err.contains("keeping changed"), "{err}");

        fs::remove_file(kept).unwrap();
        assert!(check_delete(&found, &[drop], &CancelToken::default()).is_err());
    }

    #[test]
    fn a_changed_copy_to_delete_stops_the_delete() {
        use std::time::{Duration, UNIX_EPOCH};
        let temp = tempfile::tempdir().unwrap();
        let bytes = content(1, 100_000);
        write(temp.path(), "a.bin", &bytes);
        write(temp.path(), "b.bin", &bytes);
        let (_, found) = search(temp.path(), 64 * KIB);
        let drop = &found.groups[0].members[1];

        fs::File::options()
            .write(true)
            .open(&drop.path)
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_millis(1_000_000_000_000))
            .unwrap();
        let err = check_delete(&found, &[drop.id], &CancelToken::default()).unwrap_err();
        assert!(err.contains("changed since the search"), "{err}");
    }

    #[test]
    fn ids_outside_the_search_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = content(1, 100_000);
        write(temp.path(), "a.bin", &bytes);
        write(temp.path(), "b.bin", &bytes);
        let (_, found) = search(temp.path(), 64 * KIB);
        assert!(check_delete(&found, &[0], &CancelToken::default()).is_err());
        assert!(check_delete(&found, &[], &CancelToken::default()).is_err());
    }

    #[test]
    fn forgetting_deleted_files_drops_groups_of_one() {
        let temp = tempfile::tempdir().unwrap();
        let one = content(1, 100_000);
        let two = content(2, 200_000);
        for name in ["a1", "a2", "a3"] {
            write(temp.path(), name, &one);
        }
        for name in ["b1", "b2"] {
            write(temp.path(), name, &two);
        }
        let (_, mut found) = search(temp.path(), 64 * KIB);
        assert_eq!(found.groups.len(), 2);
        let gone: HashSet<NodeId> = found
            .groups
            .iter()
            .flat_map(|group| group.members.iter().skip(1).map(|member| member.id))
            .collect();
        let one_left = found
            .groups
            .iter()
            .find(|g| g.size == 100_000)
            .unwrap()
            .members[2]
            .id;
        let gone: HashSet<NodeId> = gone.into_iter().filter(|&id| id != one_left).collect();
        found.forget(&gone);
        assert_eq!(found.groups.len(), 1);
        assert_eq!(found.groups[0].members.len(), 2);
        assert_eq!(
            found.groups[0].reclaimable,
            found.groups[0].members[1]
                .disk
                .min(found.groups[0].members[0].disk)
        );
    }
}
