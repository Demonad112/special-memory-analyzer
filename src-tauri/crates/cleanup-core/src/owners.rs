//! Space by owner: who owns the files of a scan. The owner of each file is read on demand (D-15)
//! with [`scan_core::security`], a few files at a time. Reading an owner needs READ_CONTROL only,
//! so no file data is opened and online-only (OneDrive) files are included: their owner is
//! metadata, and they count 0 bytes on disk, as in the scan.
//!
//! The pass keeps one owner index per file ([`OwnerIndex`]). Tables and filtered folder views are
//! built from that index and the live tree, so deletes after the pass are reflected without
//! reading anything again.

use crate::Row;
use scan_core::security::{self, Account, SidNames};
use scan_core::{CancelToken, Kind, NodeId, Tree};
use serde::Serialize;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Index into [`OwnerIndex::owners`] for a node that is not a file the pass looked at.
const NONE: u32 = u32::MAX;
/// The "could not read the owner" entry, always first.
pub const UNREADABLE: u32 = 0;
/// Owners listed in the saved report and top folders per owner.
pub const TOP_FOLDERS: usize = 3;
/// Reader threads. Each read is one open with READ_CONTROL; more threads mostly wait on disk.
const MAX_THREADS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OwnerKind {
    /// The account name was found.
    Known,
    /// A SID with no account (a deleted user, a domain that is gone): shown as the SID.
    NotFound,
    /// The owner could not be read (access denied, no owner on the item).
    Unreadable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Owner {
    pub sid: Option<String>,
    /// `DOMAIN\name`, `SID (account not found)` or `Unknown (no access)`.
    pub name: String,
    pub kind: OwnerKind,
}

/// The owner of every file of a scan, by node id.
#[derive(Clone, Debug)]
pub struct OwnerIndex {
    pub owners: Vec<Owner>,
    of: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerFolder {
    /// The first-level folder (or the scan root) the space is in.
    pub id: NodeId,
    pub name: String,
    pub disk: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerRow {
    pub index: u32,
    pub sid: Option<String>,
    pub name: String,
    pub kind: OwnerKind,
    pub files: u64,
    /// Space on disk of the owner's files.
    pub disk: u64,
    /// Largest first, at most [`TOP_FOLDERS`].
    pub top_folders: Vec<OwnerFolder>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerTable {
    /// Space on disk and files of the whole scan, for the share column.
    pub total_disk: u64,
    pub total_files: u64,
    /// Most space first. Owners with no live file left are omitted.
    pub rows: Vec<OwnerRow>,
}

/// Counters the pass updates and the command reads every 100 ms.
#[derive(Debug, Default)]
pub struct Counters {
    done: AtomicU64,
    total: AtomicU64,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

impl Counters {
    pub fn progress(&self) -> Progress {
        Progress {
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
        }
    }
}

/// The system reader: the owner from the security descriptor, without following a link.
pub fn read_system(path: &Path) -> io::Result<Option<String>> {
    security::read(path, security::What::Owner).map(|descriptor| descriptor.owner)
}

fn unreadable() -> Owner {
    Owner {
        sid: None,
        name: "Unknown (no access)".into(),
        kind: OwnerKind::Unreadable,
    }
}

/// The files an owner pass will read, taken from the scan under the tree lock. The reads that
/// follow hold no lock and fetch paths a batch at a time.
#[derive(Clone, Debug, Default)]
pub struct Targets {
    /// Nodes in the tree when the files were taken.
    nodes: usize,
    files: Vec<NodeId>,
}

impl Targets {
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// The live files of `tree`. Reads nothing from the disk.
pub fn collect(tree: &Tree) -> Targets {
    let mut files = Vec::new();
    let mut stack = vec![0];
    while let Some(at) = stack.pop() {
        for child in tree.live_children(at) {
            match tree.node(child).kind {
                Kind::Dir => stack.push(child),
                Kind::File => files.push(child),
                Kind::Link => {}
            }
        }
    }
    Targets {
        nodes: tree.len(),
        files,
    }
}

/// Files per batch of paths.
const BATCH: usize = 256;

/// Reads the owner of every target. `paths` gives the paths for a batch of node ids, in order,
/// `None` for a node with no path to open (removed since, or a name that is not valid Unicode);
/// it is called from the reader threads, so it may lock the tree. `read` gives the owner SID of a path (`Ok(None)` when the
/// item has no owner). `None` when cancelled.
pub fn read_all(
    targets: &Targets,
    paths: &(dyn Fn(&[NodeId]) -> Vec<Option<PathBuf>> + Sync),
    read: &(dyn Fn(&Path) -> io::Result<Option<String>> + Sync),
    names: &SidNames,
    cancel: &CancelToken,
    counters: &Counters,
) -> Option<OwnerIndex> {
    let files = &targets.files;
    counters.done.store(0, Ordering::Relaxed);
    counters.total.store(files.len() as u64, Ordering::Relaxed);

    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(1, MAX_THREADS);
    let parts: Vec<Vec<(NodeId, Option<String>)>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut part = Vec::new();
                    loop {
                        let from = next.fetch_add(BATCH, Ordering::Relaxed);
                        if from >= files.len() || cancel.is_cancelled() {
                            break;
                        }
                        let batch = &files[from..(from + BATCH).min(files.len())];
                        let batch_paths = paths(batch);
                        for (&id, path) in batch.iter().zip(batch_paths) {
                            if cancel.is_cancelled() {
                                break;
                            }
                            let owner = path.and_then(|path| read(&path).ok().flatten());
                            part.push((id, owner));
                            counters.done.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    part
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap_or_default())
            .collect()
    });
    if cancel.is_cancelled() {
        return None;
    }

    let mut owners = vec![unreadable()];
    let mut by_sid: HashMap<String, u32> = HashMap::new();
    let mut of = vec![NONE; targets.nodes];
    for (id, sid) in parts.into_iter().flatten() {
        let index = match sid {
            None => UNREADABLE,
            Some(sid) => *by_sid.entry(sid.to_ascii_uppercase()).or_insert_with(|| {
                let account = names.lookup(&sid, None);
                owners.push(Owner {
                    name: account.display(&sid),
                    kind: match account {
                        Account::NotFound => OwnerKind::NotFound,
                        _ => OwnerKind::Known,
                    },
                    sid: Some(sid),
                });
                u32::try_from(owners.len() - 1).unwrap_or(UNREADABLE)
            }),
        };
        of[id as usize] = index;
    }
    Some(OwnerIndex { owners, of })
}

impl OwnerIndex {
    fn owner_of(&self, id: NodeId) -> Option<u32> {
        self.of.get(id as usize).copied().filter(|&at| at != NONE)
    }

    /// The owner table for the files still in `tree`.
    pub fn table(&self, tree: &Tree) -> OwnerTable {
        // Per owner: files, disk, and disk per first-level folder.
        let mut sums: Vec<(u64, u64, HashMap<NodeId, u64>)> =
            self.owners.iter().map(|_| (0, 0, HashMap::new())).collect();
        let mut table = OwnerTable::default();
        // (folder, the first-level folder it is under)
        let mut stack = vec![(0, 0)];
        while let Some((at, group)) = stack.pop() {
            for child in tree.live_children(at) {
                let node = tree.node(child);
                match node.kind {
                    Kind::Dir => stack.push((child, if at == 0 { child } else { group })),
                    Kind::File => {
                        let Some(owner) = self.owner_of(child) else {
                            continue;
                        };
                        let sum = &mut sums[owner as usize];
                        sum.0 += 1;
                        sum.1 += node.disk;
                        *sum.2.entry(group).or_default() += node.disk;
                        table.total_files += 1;
                        table.total_disk += node.disk;
                    }
                    Kind::Link => {}
                }
            }
        }
        for (index, (files, disk, folders)) in sums.into_iter().enumerate() {
            if files == 0 {
                continue;
            }
            let mut top: Vec<(NodeId, u64)> = folders.into_iter().collect();
            top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let owner = &self.owners[index];
            table.rows.push(OwnerRow {
                index: u32::try_from(index).unwrap_or(UNREADABLE),
                sid: owner.sid.clone(),
                name: owner.name.clone(),
                kind: owner.kind,
                files,
                disk,
                top_folders: top
                    .into_iter()
                    .take(TOP_FOLDERS)
                    .map(|(id, disk)| OwnerFolder {
                        id,
                        name: tree.path(id).display().to_string(),
                        disk,
                    })
                    .collect(),
            });
        }
        table
            .rows
            .sort_by(|a, b| b.disk.cmp(&a.disk).then_with(|| a.name.cmp(&b.name)));
        table
    }

    /// What is directly inside folder `id`, counting only the files `owner` owns: a folder row
    /// carries that owner's space below it, and folders with none are left out.
    pub fn rows_for(&self, tree: &Tree, id: NodeId, owner: u32) -> Vec<Row> {
        if !tree.is_live(id) {
            return Vec::new();
        }
        let mut rows = Vec::new();
        for child in tree.live_children(id) {
            let node = tree.node(child);
            let mut row = Row::new(tree, child);
            match node.kind {
                Kind::File => {
                    if self.owner_of(child) != Some(owner) {
                        continue;
                    }
                }
                Kind::Link => continue,
                Kind::Dir => {
                    let mut sum = Sum::default();
                    self.sum_below(tree, child, owner, &mut sum);
                    if sum.files == 0 {
                        continue;
                    }
                    row.size = sum.disk;
                    row.logical = sum.logical;
                    row.files = sum.files;
                    row.cloud_files = sum.cloud_files;
                    row.cloud_bytes = sum.cloud_bytes;
                    // The folder's own error and folder count describe the whole folder.
                    row.dirs = 0;
                    row.errors = 0;
                    row.error = None;
                }
            }
            rows.push(row);
        }
        rows.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        rows
    }

    fn sum_below(&self, tree: &Tree, folder: NodeId, owner: u32, sum: &mut Sum) {
        let mut stack = vec![folder];
        while let Some(at) = stack.pop() {
            for child in tree.live_children(at) {
                let node = tree.node(child);
                match node.kind {
                    Kind::Dir => stack.push(child),
                    Kind::File if self.owner_of(child) == Some(owner) => {
                        sum.files += 1;
                        sum.disk += node.disk;
                        sum.logical += node.size;
                        sum.cloud_files += u64::from(node.cloud_files > 0);
                        sum.cloud_bytes += node.cloud_bytes;
                    }
                    _ => {}
                }
            }
        }
    }
}

#[derive(Default)]
struct Sum {
    files: u64,
    disk: u64,
    logical: u64,
    cloud_files: u64,
    cloud_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use scan_core::{scan, ScanState};
    use std::fs;

    const ALICE: &str = "S-1-5-21-1-2-3-1001";
    const BOB: &str = "S-1-5-21-1-2-3-1002";
    const GONE: &str = "S-1-5-21-1-2-3-1003";

    fn names() -> SidNames {
        SidNames::with_resolver(|sid, _| match sid {
            ALICE => Account::Found {
                name: "alice".into(),
                domain: "CORP".into(),
            },
            BOB => Account::Found {
                name: "bob".into(),
                domain: String::new(),
            },
            _ => Account::NotFound,
        })
    }

    /// Owner by file name prefix; `deny_*` cannot be read, `none_*` has no owner.
    fn reader(path: &Path) -> io::Result<Option<String>> {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with("alice") {
            Ok(Some(ALICE.into()))
        } else if name.starts_with("bob") {
            Ok(Some(BOB.into()))
        } else if name.starts_with("gone") {
            Ok(Some(GONE.into()))
        } else if name.starts_with("none") {
            Ok(None)
        } else {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        }
    }

    /// [`collect`] and [`read_all`] with paths taken from `tree` (no lock to take here).
    fn run(
        tree: &Tree,
        read: &(dyn Fn(&Path) -> io::Result<Option<String>> + Sync),
        names: &SidNames,
        cancel: &CancelToken,
        counters: &Counters,
    ) -> Option<OwnerIndex> {
        let paths = |ids: &[NodeId]| -> Vec<Option<PathBuf>> {
            ids.iter()
                .map(|&id| (!tree.node(id).lossy_name).then(|| tree.path(id)))
                .collect()
        };
        read_all(&collect(tree), &paths, read, names, cancel, counters)
    }

    fn put(root: &Path, name: &str, len: usize) {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7u8; len]).unwrap();
    }

    fn tree_and_index() -> (tempfile::TempDir, Tree, OwnerIndex) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        put(root, "alice-root.bin", 10_000);
        put(root, "a/alice-1.bin", 40_000);
        put(root, "a/deep/alice-2.bin", 30_000);
        put(root, "a/bob-1.bin", 5_000);
        put(root, "b/alice-3.bin", 20_000);
        put(root, "b/bob-2.bin", 60_000);
        put(root, "c/alice-4.bin", 1_000);
        put(root, "c/alice-5.bin", 2_000);
        put(root, "c/gone-1.bin", 3_000);
        put(root, "c/none-1.bin", 4_000);
        put(root, "c/deny-1.bin", 5_000);
        let tree = scan(root, &ScanState::default()).unwrap();
        let index = run(
            &tree,
            &reader,
            &names(),
            &CancelToken::default(),
            &Counters::default(),
        )
        .unwrap();
        (temp, tree, index)
    }

    fn row<'a>(table: &'a OwnerTable, name: &str) -> &'a OwnerRow {
        table
            .rows
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("no row {name}: {:?}", table.rows))
    }

    #[test]
    fn totals_and_names_per_owner() {
        let (_temp, tree, index) = tree_and_index();
        let table = index.table(&tree);
        assert_eq!(table.total_files, 11);
        assert_eq!(table.total_disk, tree.root().disk);
        assert_eq!(
            table.rows.iter().map(|row| row.files).sum::<u64>(),
            table.total_files
        );
        assert_eq!(
            table.rows.iter().map(|row| row.disk).sum::<u64>(),
            table.total_disk
        );
        assert_eq!(row(&table, "CORP\\alice").files, 6);
        assert_eq!(row(&table, "bob").files, 2);
        let gone = row(&table, &format!("{GONE} (account not found)"));
        assert_eq!((gone.files, gone.kind), (1, OwnerKind::NotFound));
        assert_eq!(gone.sid.as_deref(), Some(GONE));
    }

    #[test]
    fn unreadable_and_ownerless_files_share_one_bucket() {
        let (_temp, tree, index) = tree_and_index();
        let table = index.table(&tree);
        let unknown = row(&table, "Unknown (no access)");
        assert_eq!(unknown.files, 2);
        assert_eq!(unknown.kind, OwnerKind::Unreadable);
        assert_eq!(unknown.sid, None);
        assert_eq!(
            index.owners[UNREADABLE as usize].kind,
            OwnerKind::Unreadable
        );
    }

    #[test]
    fn rows_are_largest_first_and_top_folders_are_first_level() {
        let (_temp, tree, index) = tree_and_index();
        let table = index.table(&tree);
        let disks: Vec<u64> = table.rows.iter().map(|row| row.disk).collect();
        assert!(disks.windows(2).all(|pair| pair[0] >= pair[1]), "{disks:?}");

        let alice = row(&table, "CORP\\alice");
        // a (incl. deep), b, c and the root's own file: only three are kept.
        assert_eq!(alice.top_folders.len(), TOP_FOLDERS);
        let top: Vec<&str> = alice
            .top_folders
            .iter()
            .map(|folder| {
                Path::new(&folder.name)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
            })
            .collect();
        assert_eq!(top[0], "a", "{top:?}");
        assert!(alice
            .top_folders
            .windows(2)
            .all(|pair| pair[0].disk >= pair[1].disk));
        let a = alice.top_folders[0].id;
        assert_eq!(tree.node(a).name, "a");
        assert!(alice.top_folders[0].disk > alice.top_folders[1].disk);
    }

    #[test]
    fn root_level_files_are_grouped_under_the_root() {
        let temp = tempfile::tempdir().unwrap();
        put(temp.path(), "alice-1.bin", 5_000);
        let tree = scan(temp.path(), &ScanState::default()).unwrap();
        let index = run(
            &tree,
            &reader,
            &names(),
            &CancelToken::default(),
            &Counters::default(),
        )
        .unwrap();
        let table = index.table(&tree);
        assert_eq!(table.rows[0].top_folders[0].id, 0);
    }

    #[test]
    fn deleted_files_leave_the_table() {
        let (_temp, mut tree, index) = tree_and_index();
        let b = tree.child_by_name(0, "b").unwrap();
        let before = index.table(&tree);
        assert!(tree.remove(b));
        let after = index.table(&tree);
        assert_eq!(after.total_files, before.total_files - 2);
        assert_eq!(row(&after, "bob").files, 1);
        assert_eq!(after.total_disk, tree.root().disk);
    }

    #[test]
    fn owner_filter_shows_only_that_owners_space() {
        let (_temp, tree, index) = tree_and_index();
        let table = index.table(&tree);
        let alice = row(&table, "CORP\\alice").index;
        let bob = row(&table, "bob").index;

        let rows = index.rows_for(&tree, 0, alice);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert!(names.contains(&"alice-root.bin"));
        assert!(names.contains(&"a") && names.contains(&"b") && names.contains(&"c"));
        let a = rows.iter().find(|row| row.name == "a").unwrap();
        assert_eq!(a.files, 2, "alice-1 and deep/alice-2, not bob-1");
        let full = Row::new(&tree, a.id);
        assert!(a.size < full.size);

        let rows = index.rows_for(&tree, 0, bob);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert!(!names.contains(&"alice-root.bin"));
        assert!(!names.contains(&"c"), "bob owns nothing in c");
        assert_eq!(rows.iter().map(|row| row.files).sum::<u64>(), 2);
        // Largest first.
        assert_eq!(names[0], "b");
    }

    #[test]
    fn a_cancelled_pass_gives_nothing() {
        let temp = tempfile::tempdir().unwrap();
        put(temp.path(), "alice-1.bin", 5_000);
        let tree = scan(temp.path(), &ScanState::default()).unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(run(&tree, &reader, &names(), &cancel, &Counters::default()).is_none());
    }

    #[test]
    fn progress_counts_every_file_and_names_are_looked_up_once() {
        let calls = std::sync::Arc::new(AtomicU64::new(0));
        let seen = calls.clone();
        let names = SidNames::with_resolver(move |_, _| {
            seen.fetch_add(1, Ordering::Relaxed);
            Account::NotSupported
        });
        let (temp, tree, _) = tree_and_index();
        let counters = Counters::default();
        let index = run(&tree, &reader, &names, &CancelToken::default(), &counters).unwrap();
        let progress = counters.progress();
        assert_eq!((progress.done, progress.total), (11, 11));
        // Alice, Bob and GONE, however many files each owns.
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        // A lookup that failed still shows the SID.
        assert!(index.owners.iter().any(|owner| owner.name == ALICE));
        drop(temp);
    }

    #[cfg(windows)]
    #[test]
    fn files_made_by_the_test_are_owned_by_the_current_user() {
        let temp = tempfile::tempdir().unwrap();
        put(temp.path(), "mine.bin", 1_000);
        let tree = scan(temp.path(), &ScanState::default()).unwrap();
        let index = run(
            &tree,
            &read_system,
            &SidNames::default(),
            &CancelToken::default(),
            &Counters::default(),
        )
        .unwrap();
        let table = index.table(&tree);
        assert_eq!(table.rows.len(), 1, "{:?}", table.rows);
        let owner = &table.rows[0];
        let sid = owner.sid.as_deref().expect("the owner was read");
        // The file's owner is the account that created it or, for an elevated token, the
        // Administrators group; either way a real SID that resolves to a name.
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert_ne!(owner.kind, OwnerKind::Unreadable);
        assert_eq!(owner.files, 1);
    }
}
