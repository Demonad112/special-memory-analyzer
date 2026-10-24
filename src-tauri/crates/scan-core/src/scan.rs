use crate::list::{list_dir, Entry};
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Index of a node in a [`Tree`]. The root is always 0.
pub type NodeId = u32;

/// Shared cancel flag. Clones point at the same flag.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Live counters for one scan. The scan updates them; another thread reads [`ScanState::progress`].
#[derive(Debug, Default)]
pub struct ScanState {
    cancel: CancelToken,
    files: AtomicU64,
    dirs: AtomicU64,
    bytes: AtomicU64,
    current: Mutex<String>,
    ignore_junk: bool,
}

impl ScanState {
    pub fn new(cancel: CancelToken) -> Self {
        Self {
            cancel,
            ..Self::default()
        }
    }

    /// Leaves out system and temp files (see [`is_junk`]), as if they weren't there.
    pub fn ignoring_junk(mut self, ignore: bool) -> Self {
        self.ignore_junk = ignore;
        self
    }

    pub fn progress(&self) -> ScanProgress {
        ScanProgress {
            files: self.files.load(Ordering::Relaxed),
            dirs: self.dirs.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            current: self
                .current
                .lock()
                .map(|current| current.clone())
                .unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    /// The folder being read most recently.
    pub current: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    File,
    Dir,
    /// A symlink or junction. Never followed, counted as size 0.
    Link,
}

/// One file, folder or link. Folder totals include everything below them.
#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub kind: Kind,
    /// Logical size in bytes (what Explorer shows as "Size").
    pub size: u64,
    /// Space on the disk in bytes: whole clusters, compressed and sparse files at their real
    /// cost, hard links counted once, online-only cloud files 0. This is the number to use when
    /// asking "how much can I free"; [`Node::size`] is the number to use when comparing copies.
    pub disk: u64,
    /// Files at or below this node (1 for a file).
    pub files: u64,
    /// Folders below this node, not counting itself.
    pub dirs: u64,
    /// Cloud-only placeholders (OneDrive "online-only") at or below this node. Their size is the
    /// logical size; the data is not on the disk.
    pub cloud_files: u64,
    /// Bytes of those placeholders: counted in [`Node::size`] but not stored on this disk.
    pub cloud_bytes: u64,
    /// Folders at or below this node that could not be read completely.
    pub errors: u64,
    /// Why this folder could not be read completely.
    pub error: Option<String>,
    /// The Windows error code behind [`Node::error`], when there is one.
    pub error_code: Option<i32>,
    /// Files: last modified time, in ms since 1970.
    pub modified_ms: Option<u64>,
    /// Files: last access time, in ms since 1970, from the same listing as the modified time.
    /// Windows often does not keep it up to date (`NtfsDisableLastAccessUpdate`), so it is only
    /// ever an extra filter, never the main rule.
    pub accessed_ms: Option<u64>,
    /// Files: another name for the same data exists in this scan (hard link). Only the first name
    /// keeps its [`Node::disk`].
    pub hard_linked: bool,
    /// The name is not valid Unicode (NTFS allows unpaired surrogates), so [`Node::name`] holds
    /// replacement characters and a path built from it would point at a different, missing item.
    /// Such a folder is not entered.
    pub lossy_name: bool,
    first_child: NodeId,
    child_count: u32,
    parent: NodeId,
    /// Deleted after the scan (see [`Tree::remove`]); left out of every listing.
    removed: bool,
}

impl Node {
    fn new(name: String, kind: Kind) -> Self {
        Self {
            name,
            kind,
            size: 0,
            disk: 0,
            files: 0,
            dirs: 0,
            cloud_files: 0,
            cloud_bytes: 0,
            errors: 0,
            error: None,
            error_code: None,
            modified_ms: None,
            accessed_ms: None,
            hard_linked: false,
            lossy_name: false,
            first_child: 0,
            child_count: 0,
            parent: 0,
            removed: false,
        }
    }

    /// Ids of this node's children, largest first.
    pub fn children(&self) -> Range<NodeId> {
        self.first_child..self.first_child + self.child_count
    }

    pub fn has_children(&self) -> bool {
        self.child_count > 0
    }
}

/// A scanned folder as a flat arena. Children of a node are contiguous and sorted largest first.
#[derive(Clone, Debug)]
pub struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    pub fn root(&self) -> &Node {
        &self.nodes[0]
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The child of `id` with exactly this name.
    pub fn child_by_name(&self, id: NodeId, name: &str) -> Option<NodeId> {
        self.node(id)
            .children()
            .find(|&child| self.node(child).name == name)
    }

    /// The folder holding `id`; `None` for the root.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        (id != 0).then(|| self.node(id).parent)
    }

    /// Children of `id` that were not removed, largest first as scanned.
    pub fn live_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.node(id)
            .children()
            .filter(|&child| !self.node(child).removed)
    }

    /// True when `id` exists and neither it nor a folder above it was removed.
    pub fn is_live(&self, id: NodeId) -> bool {
        if id as usize >= self.nodes.len() {
            return false;
        }
        let mut current = Some(id);
        while let Some(at) = current {
            if self.node(at).removed {
                return false;
            }
            current = self.parent(at);
        }
        true
    }

    /// Full path of `id`. The root's name is the scanned folder's path.
    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut names = Vec::new();
        let mut current = id;
        while let Some(parent) = self.parent(current) {
            names.push(&self.node(current).name);
            current = parent;
        }
        let mut path = PathBuf::from(&self.root().name);
        path.extend(names.into_iter().rev());
        path
    }

    /// Marks a deleted item as gone and takes it out of the totals of every folder above it.
    /// False when `id` is the root or already gone.
    pub fn remove(&mut self, id: NodeId) -> bool {
        if id == 0 || !self.is_live(id) {
            return false;
        }
        let node = &mut self.nodes[id as usize];
        node.removed = true;
        let (size, disk, files, cloud_files, cloud_bytes, errors) = (
            node.size,
            node.disk,
            node.files,
            node.cloud_files,
            node.cloud_bytes,
            node.errors,
        );
        let dirs = node.dirs + u64::from(node.kind == Kind::Dir);
        let mut current = id;
        while let Some(parent) = self.parent(current) {
            let above = &mut self.nodes[parent as usize];
            above.size = above.size.saturating_sub(size);
            above.disk = above.disk.saturating_sub(disk);
            above.files = above.files.saturating_sub(files);
            above.dirs = above.dirs.saturating_sub(dirs);
            above.cloud_files = above.cloud_files.saturating_sub(cloud_files);
            above.cloud_bytes = above.cloud_bytes.saturating_sub(cloud_bytes);
            above.errors = above.errors.saturating_sub(errors);
            current = parent;
        }
        true
    }
}

#[derive(Debug)]
pub enum ScanError {
    NotFound(PathBuf),
    NotADirectory(PathBuf),
    Unreadable { path: PathBuf, message: String },
    Cancelled,
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(path) => write!(f, "{} does not exist.", path.display()),
            Self::NotADirectory(path) => write!(f, "{} is not a folder.", path.display()),
            Self::Unreadable { path, message } => {
                write!(f, "{} could not be read: {message}", path.display())
            }
            Self::Cancelled => write!(f, "The scan was cancelled."),
        }
    }
}

impl std::error::Error for ScanError {}

/// Walks `root` and returns its size tree. Unreadable subfolders are kept with an error and the
/// scan continues; an unreadable root, a missing root or a cancel is an error.
pub fn scan(root: &Path, state: &ScanState) -> Result<Tree, ScanError> {
    let metadata = fs::metadata(root).map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => ScanError::NotFound(root.to_path_buf()),
        _ => ScanError::Unreadable {
            path: root.to_path_buf(),
            message: err.to_string(),
        },
    })?;
    if !metadata.is_dir() {
        return Err(ScanError::NotADirectory(root.to_path_buf()));
    }

    let mut slots = scan_slots(root, root.display().to_string(), state);
    if state.cancel.is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    if slots[0].node.error.is_some() && slots[0].kids.is_empty() {
        return Err(ScanError::Unreadable {
            path: root.to_path_buf(),
            message: slots[0].node.error.take().unwrap_or_default(),
        });
    }
    let order = preorder(&slots);
    count_hard_links_once(&mut slots, &order);
    total_up(&mut slots, &order);
    Ok(flatten(slots))
}

/// Stack size of the scan's worker threads. The walk keeps no folder on the call stack (it is
/// a queue of tasks), so this is a margin, not a requirement.
const SCAN_STACK_BYTES: usize = 64 << 20;

/// A node of the tree while it is being built, in a flat list: `kids` are indexes into it. Nothing
/// is nested, so a folder thousands of levels deep neither recurses nor overflows the stack
/// when it is built, summed or dropped.
struct Slot {
    node: Node,
    kids: Vec<u32>,
    /// Volume-unique id of a file, 0 when unknown; used to find hard links.
    file_id: u64,
}

impl Slot {
    fn leaf(entry: Entry) -> Self {
        let mut node = Node::new(entry.name, entry.kind);
        node.size = entry.size;
        node.disk = entry.disk;
        node.files = u64::from(entry.kind == Kind::File);
        node.cloud_files = u64::from(entry.cloud);
        node.cloud_bytes = if entry.cloud { entry.size } else { 0 };
        node.modified_ms = entry.modified_ms;
        node.accessed_ms = entry.accessed_ms;
        node.lossy_name = entry.lossy;
        Self {
            node,
            kids: Vec::new(),
            file_id: entry.file_id,
        }
    }

    fn dir(name: String) -> Self {
        Self {
            node: Node::new(name, Kind::Dir),
            kids: Vec::new(),
            file_id: 0,
        }
    }
}

/// What the parallel walk shares: ids are handed out as folders are read, and each folder
/// leaves its slots in `done`.
struct Walk<'a> {
    state: &'a ScanState,
    next: AtomicU32,
    done: Mutex<Vec<Vec<(u32, Slot)>>>,
}

/// Walks the folder in parallel on a pool with a large stack, and returns the slots with the
/// root at index 0. Folders are tasks, not recursive calls, so depth never reaches the stack.
fn scan_slots(root: &Path, name: String, state: &ScanState) -> Vec<Slot> {
    let walk = Walk {
        state,
        next: AtomicU32::new(1),
        done: Mutex::new(Vec::new()),
    };
    let run = || {
        rayon::scope(|scope| scan_dir(scope, &walk, root.to_path_buf(), name, 0));
    };
    match rayon::ThreadPoolBuilder::new()
        .stack_size(SCAN_STACK_BYTES)
        .thread_name(|index| format!("scan-{index}"))
        .build()
    {
        Ok(pool) => pool.install(run),
        // No threads to be had: the global pool still walks it.
        Err(_) => run(),
    }
    let batches = walk
        .done
        .into_inner()
        .unwrap_or_else(|err| err.into_inner());
    let total = walk.next.into_inner() as usize;
    let mut slots: Vec<Option<Slot>> = Vec::new();
    slots.resize_with(total, || None);
    for (id, slot) in batches.into_iter().flatten() {
        slots[id as usize] = Some(slot);
    }
    slots
        .into_iter()
        .map(|slot| slot.expect("every reserved id was filled"))
        .collect()
}

/// Reads one folder, queues a task for each subfolder, and files its own slots under `done`.
fn scan_dir<'a>(
    scope: &rayon::Scope<'a>,
    walk: &'a Walk<'a>,
    path: PathBuf,
    name: String,
    id: u32,
) {
    let state = walk.state;
    let mut dir = Slot::dir(name);
    let finish = |dir: Slot, mut batch: Vec<(u32, Slot)>| {
        batch.push((id, dir));
        walk.done
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push(batch);
    };
    if state.cancel.is_cancelled() {
        return finish(dir, Vec::new());
    }
    state.dirs.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut current) = state.current.try_lock() {
        *current = path.display().to_string();
    }

    let listing = match list_dir(&path) {
        Ok(listing) => listing,
        Err(err) => {
            dir.node.error = Some(err.to_string());
            dir.node.error_code = err.raw_os_error();
            dir.node.errors = 1;
            return finish(dir, Vec::new());
        }
    };
    if let Some(err) = &listing.error {
        dir.node.error = Some(err.to_string());
        dir.node.error_code = err.raw_os_error();
    }

    let mut leaves = Vec::new();
    let mut subdirs = Vec::new();
    let mut files = 0;
    let mut bytes = 0;
    for entry in listing.entries {
        if state.ignore_junk && is_junk(&entry.name, entry.kind == Kind::Dir) {
            continue;
        }
        if entry.kind == Kind::Dir && entry.lossy {
            // No path can be built from the name, so the folder is listed but not entered.
            let mut folder = Slot::leaf(entry);
            folder.node.error = Some(
                "The folder name has characters that are not valid Unicode, so it was not opened."
                    .to_owned(),
            );
            leaves.push(folder);
        } else if entry.kind == Kind::Dir {
            subdirs.push((path.join(&entry.name), entry.name));
        } else {
            if entry.kind == Kind::File {
                files += 1;
                bytes += entry.size;
            }
            leaves.push(Slot::leaf(entry));
        }
    }
    state.files.fetch_add(files, Ordering::Relaxed);
    state.bytes.fetch_add(bytes, Ordering::Relaxed);

    // Children keep their listing order: files and unopened folders first, then subfolders.
    let count = leaves.len() + subdirs.len();
    let base = walk.next.fetch_add(count as u32, Ordering::Relaxed);
    dir.kids = (base..base + count as u32).collect();
    let mut batch = Vec::with_capacity(leaves.len() + 1);
    for (index, leaf) in leaves.into_iter().enumerate() {
        batch.push((base + index as u32, leaf));
    }
    let first_subdir = base + batch.len() as u32;
    finish(dir, batch);
    for (index, (path, name)) in subdirs.into_iter().enumerate() {
        let child = first_subdir + index as u32;
        scope.spawn(move |scope| scan_dir(scope, walk, path, name, child));
    }
}

/// Slot indexes in walk order: a folder, then everything in its first child, and so on. Parents
/// always come before their children.
fn preorder(slots: &[Slot]) -> Vec<u32> {
    let mut order = Vec::with_capacity(slots.len());
    let mut stack = vec![0u32];
    while let Some(at) = stack.pop() {
        order.push(at);
        stack.extend(slots[at as usize].kids.iter().rev());
    }
    order
}

/// A file with several names (hard links) takes disk space once. The first name in walk order
/// keeps its size on disk; the others show 0, so no folder total counts the data twice. Files are
/// matched on file id and size together, because not every file system hands out reliable ids.
fn count_hard_links_once(slots: &mut [Slot], order: &[u32]) {
    let key = |slot: &Slot| {
        (slot.node.kind == Kind::File && slot.file_id != 0)
            .then_some((slot.file_id, slot.node.size))
    };
    let mut ids: Vec<(u64, u64)> = order
        .iter()
        .filter_map(|&at| key(&slots[at as usize]))
        .collect();
    ids.sort_unstable();
    let repeated: HashSet<(u64, u64)> = ids
        .windows(2)
        .filter(|pair| pair[0] == pair[1])
        .map(|pair| pair[0])
        .collect();
    if repeated.is_empty() {
        return;
    }
    let mut seen = HashSet::new();
    for &at in order {
        let slot = &mut slots[at as usize];
        let Some(key) = key(slot) else { continue };
        if repeated.contains(&key) {
            slot.node.hard_linked = true;
            if !seen.insert(key) {
                slot.node.disk = 0;
            }
        }
    }
}

/// Adds every child into its folder and sorts each folder's children, largest first. Children
/// come after their parent in `order`, so going backwards finishes every child first.
fn total_up(slots: &mut [Slot], order: &[u32]) {
    for &at in order.iter().rev() {
        let mut kids = std::mem::take(&mut slots[at as usize].kids);
        let mut node = std::mem::replace(
            &mut slots[at as usize].node,
            Node::new(String::new(), Kind::Dir),
        );
        node.errors = u64::from(node.error.is_some());
        for &kid in &kids {
            let child = &slots[kid as usize].node;
            node.size += child.size;
            node.disk += child.disk;
            node.files += child.files;
            node.dirs += child.dirs + u64::from(child.kind == Kind::Dir);
            node.cloud_files += child.cloud_files;
            node.cloud_bytes += child.cloud_bytes;
            node.errors += child.errors;
        }
        kids.sort_by(|&a, &b| {
            let (a, b) = (&slots[a as usize].node, &slots[b as usize].node);
            b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))
        });
        slots[at as usize].node = node;
        slots[at as usize].kids = kids;
    }
}

/// Breadth-first, so every node's children end up next to each other.
fn flatten(slots: Vec<Slot>) -> Tree {
    let mut slots: Vec<Option<Slot>> = slots.into_iter().map(Some).collect();
    let mut take = |at: u32| slots[at as usize].take().expect("each slot is placed once");
    let root = take(0);
    let mut nodes = vec![root.node];
    let mut queue = VecDeque::from([(0usize, root.kids)]);
    while let Some((id, kids)) = queue.pop_front() {
        nodes[id].first_child = nodes.len() as NodeId;
        nodes[id].child_count = kids.len() as u32;
        for kid in kids {
            let mut child = take(kid);
            child.node.parent = id as NodeId;
            nodes.push(child.node);
            queue.push_back((nodes.len() - 1, child.kids));
        }
    }
    Tree { nodes }
}

/// Files Windows, Office and macOS leave behind in every folder, and the per-drive system
/// folders. Nobody needs them copied, so with "Ignore system and temp files" they are skipped.
pub fn is_junk(name: &str, is_dir: bool) -> bool {
    let name = name.to_lowercase();
    if is_dir {
        return matches!(name.as_str(), "$recycle.bin" | "system volume information");
    }
    matches!(
        name.as_str(),
        "thumbs.db" | "ehthumbs.db" | "desktop.ini" | ".ds_store"
    ) || name.starts_with("~$")
        || name.ends_with(".tmp")
}

/// Trees too deep to build on disk.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// `depth` folders inside each other, with a 7-byte file at the bottom, built without a walk.
    pub(crate) fn deep_chain(depth: u32) -> Tree {
        let mut nodes = Vec::new();
        for level in 0..=depth {
            let mut node = Node::new(format!("d{level}"), Kind::Dir);
            node.size = 7;
            node.disk = 7;
            node.files = 1;
            node.dirs = u64::from(depth - level);
            node.parent = level.saturating_sub(1);
            node.first_child = level + 1;
            node.child_count = 1;
            nodes.push(node);
        }
        let mut file = Node::new("bottom.bin".into(), Kind::File);
        (file.size, file.disk, file.files) = (7, 7, 1);
        file.parent = depth;
        nodes.push(file);
        nodes[depth as usize].first_child = depth + 1;
        Tree { nodes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, bytes: usize) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn scan_ok(root: &Path) -> Tree {
        scan(root, &ScanState::default()).unwrap()
    }

    #[test]
    fn folder_totals_are_recursive() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("a.bin"), 10);
        write(&temp.path().join("sub/b.bin"), 20);
        write(&temp.path().join("sub/deeper/c.bin"), 30);
        fs::create_dir(temp.path().join("empty")).unwrap();

        let tree = scan_ok(temp.path());
        let root = tree.root();
        let file = tree.child_by_name(0, "a.bin").unwrap();
        assert!(tree.node(file).modified_ms.is_some());
        assert!(tree.node(file).accessed_ms.is_some());
        assert!(tree.node(0).accessed_ms.is_none(), "folders carry no times");
        assert_eq!(root.size, 60);
        assert_eq!(root.files, 3);
        assert_eq!(root.dirs, 3);

        let sub = tree.child_by_name(0, "sub").unwrap();
        assert_eq!(tree.node(sub).size, 50);
        assert_eq!(tree.node(sub).files, 2);
        assert_eq!(tree.node(sub).dirs, 1);

        let empty = tree.child_by_name(0, "empty").unwrap();
        assert_eq!(tree.node(empty).size, 0);
        assert!(!tree.node(empty).has_children());
    }

    #[test]
    fn file_times_come_from_the_listing() {
        use std::time::{Duration, UNIX_EPOCH};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("old.bin");
        write(&path, 3);
        let accessed_ms = 1_577_836_800_000; // 2020-01-01
        let modified_ms = 1_420_070_400_000; // 2015-01-01
        let times = fs::FileTimes::new()
            .set_accessed(UNIX_EPOCH + Duration::from_millis(accessed_ms))
            .set_modified(UNIX_EPOCH + Duration::from_millis(modified_ms));
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(times)
            .unwrap();

        let tree = scan_ok(temp.path());
        let file = tree.node(tree.child_by_name(0, "old.bin").unwrap());
        assert_eq!(file.modified_ms, Some(modified_ms));
        assert_eq!(file.accessed_ms, Some(accessed_ms));
    }

    #[test]
    fn children_are_contiguous_and_largest_first() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("small.bin"), 1);
        write(&temp.path().join("big.bin"), 100);
        write(&temp.path().join("mid/x.bin"), 50);

        let tree = scan_ok(temp.path());
        let names: Vec<&str> = tree
            .root()
            .children()
            .map(|id| tree.node(id).name.as_str())
            .collect();
        assert_eq!(names, ["big.bin", "mid", "small.bin"]);
        assert_eq!(tree.len(), 5);
    }

    #[test]
    fn removing_an_item_updates_every_folder_above() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("a.bin"), 10);
        write(&temp.path().join("sub/deeper/c.bin"), 30);
        write(&temp.path().join("sub/b.bin"), 20);

        let mut tree = scan_ok(temp.path());
        let sub = tree.child_by_name(0, "sub").unwrap();
        let deeper = tree.child_by_name(sub, "deeper").unwrap();
        let c = tree.child_by_name(deeper, "c.bin").unwrap();
        assert_eq!(
            tree.path(c),
            temp.path().join("sub").join("deeper").join("c.bin")
        );
        assert_eq!(tree.parent(c), Some(deeper));
        assert_eq!(tree.parent(0), None);

        assert!(tree.remove(deeper));
        assert_eq!(
            (tree.root().size, tree.root().files, tree.root().dirs),
            (30, 2, 1)
        );
        assert_eq!((tree.node(sub).size, tree.node(sub).dirs), (20, 0));
        assert_eq!(tree.live_children(sub).count(), 1);
        // Anything inside a removed folder is already gone; the root never goes.
        assert!(!tree.is_live(c));
        assert!(!tree.remove(c));
        assert!(!tree.remove(deeper));
        assert!(!tree.remove(0));
        assert_eq!(tree.root().size, 30);
    }

    #[test]
    fn progress_counts_everything() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("a.bin"), 5);
        write(&temp.path().join("sub/b.bin"), 7);
        let state = ScanState::default();
        scan(temp.path(), &state).unwrap();
        let progress = state.progress();
        assert_eq!(progress.files, 2);
        assert_eq!(progress.bytes, 12);
        assert_eq!(progress.dirs, 2);
    }

    #[test]
    fn junk_is_skipped_only_when_asked() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("report.docx"), 10);
        write(&temp.path().join("~$report.docx"), 1);
        write(&temp.path().join("Thumbs.db"), 2);
        write(&temp.path().join("sub/desktop.ini"), 3);
        write(&temp.path().join("sub/setup.TMP"), 4);
        write(&temp.path().join("$RECYCLE.BIN/x.bin"), 5);
        // Names must match exactly.
        write(&temp.path().join("thumbs.db.bak"), 6);

        assert_eq!(scan_ok(temp.path()).root().files, 7);
        let state = ScanState::default().ignoring_junk(true);
        let tree = scan(temp.path(), &state).unwrap();
        assert_eq!((tree.root().files, tree.root().size), (2, 16));
        assert!(tree.child_by_name(0, "$RECYCLE.BIN").is_none());
        assert!(is_junk("System Volume Information", true));
        assert!(!is_junk("System Volume Information", false));
    }

    #[test]
    fn missing_root_and_file_root_are_errors() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file.txt");
        write(&file, 1);
        assert!(matches!(
            scan(&temp.path().join("nope"), &ScanState::default()),
            Err(ScanError::NotFound(_))
        ));
        assert!(matches!(
            scan(&file, &ScanState::default()),
            Err(ScanError::NotADirectory(_))
        ));
    }

    #[test]
    fn a_name_that_is_not_valid_unicode_is_flagged_and_its_folder_is_not_entered() {
        #[cfg(windows)]
        let odd = {
            use std::os::windows::ffi::OsStringExt;
            std::ffi::OsString::from_wide(&[0xD800, u16::from(b'a')])
        };
        #[cfg(unix)]
        let odd = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![0xFF, b'a'])
        };
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("fine.bin"), 5);
        let folder = temp.path().join(&odd);
        if fs::create_dir(&folder).is_err()
            || fs::write(temp.path().join(&odd).join("x"), b"x").is_err()
        {
            return; // this file system cannot hold such names
        }
        let file_name = {
            let mut name = odd.clone();
            name.push(".bin");
            name
        };
        fs::write(temp.path().join(file_name), b"abc").unwrap();

        let tree = scan_ok(temp.path());

        let flagged: Vec<_> = tree
            .live_children(0)
            .map(|id| tree.node(id))
            .filter(|node| node.lossy_name)
            .collect();
        assert_eq!(flagged.len(), 2);
        let dir = flagged.iter().find(|node| node.kind == Kind::Dir).unwrap();
        assert!(dir.error.is_some() && !dir.has_children(), "not entered");
        assert_eq!(tree.root().errors, 1);
        assert!(
            !tree
                .node(tree.child_by_name(0, "fine.bin").unwrap())
                .lossy_name
        );
    }

    #[test]
    fn cancelled_scan_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("a.bin"), 5);
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(
            scan(temp.path(), &ScanState::new(cancel)),
            Err(ScanError::Cancelled)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_listed_but_not_followed() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("real/data.bin"), 40);
        std::os::unix::fs::symlink(temp.path().join("real"), temp.path().join("link")).unwrap();
        // A loop back to the root must not hang the scan.
        std::os::unix::fs::symlink(temp.path(), temp.path().join("real/loop")).unwrap();

        let tree = scan_ok(temp.path());
        assert_eq!(tree.root().size, 40);
        assert_eq!(tree.root().files, 1);
        let link = tree.child_by_name(0, "link").unwrap();
        assert_eq!(tree.node(link).kind, Kind::Link);
        assert_eq!(tree.node(link).size, 0);
    }

    /// `mklink /J` needs no privilege. Removed on drop so a failed assertion never leaves the
    /// temp folder cleanup to walk through the junction.
    #[cfg(windows)]
    struct Junction(std::path::PathBuf);

    #[cfg(windows)]
    impl Junction {
        fn new(link: &Path, target: &Path) -> Self {
            // mklink rejects forward slashes, and `join("a/b")` leaves them in the path.
            let backslashed = |path: &Path| path.to_string_lossy().replace('/', "\\");
            let output = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(backslashed(link))
                .arg(backslashed(target))
                .output()
                .expect("cmd should run");
            assert!(
                output.status.success(),
                "mklink /J failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            Self(link.to_path_buf())
        }
    }

    #[cfg(windows)]
    impl Drop for Junction {
        fn drop(&mut self) {
            let _ = fs::remove_dir(&self.0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn junctions_are_listed_but_not_followed() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(&temp.path().join("real/data.bin"), 40);
        write(&outside.path().join("big.bin"), 5_000);
        let _inside = Junction::new(&temp.path().join("link"), &temp.path().join("real"));
        let _away = Junction::new(&temp.path().join("away"), outside.path());
        // A loop back to the root must not hang the scan.
        let _loop = Junction::new(&temp.path().join("real/loop"), temp.path());

        let tree = scan_ok(temp.path());
        assert_eq!(tree.root().size, 40);
        assert_eq!(tree.root().files, 1);
        for name in ["link", "away"] {
            let id = tree.child_by_name(0, name).unwrap();
            assert_eq!(tree.node(id).kind, Kind::Link, "{name}");
            assert_eq!(tree.node(id).size, 0, "{name}");
        }
        let real = tree.child_by_name(0, "real").unwrap();
        let looped = tree.child_by_name(real, "loop").unwrap();
        assert_eq!(tree.node(looped).kind, Kind::Link);
    }

    #[test]
    fn size_on_disk_is_whole_clusters_and_never_below_the_data() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("big.bin"), 100_000);
        write(&temp.path().join("sub/other.bin"), 70_000);

        let tree = scan_ok(temp.path());
        let big = tree.node(tree.child_by_name(0, "big.bin").unwrap());
        assert_eq!(big.size, 100_000);
        assert!(
            big.disk >= big.size,
            "disk {} < size {}",
            big.disk,
            big.size
        );
        assert!(big.disk < big.size + 64 * 1024, "disk {}", big.disk);
        assert_eq!(tree.root().size, 170_000);
        assert!(tree.root().disk >= 170_000);
        let sub = tree.node(tree.child_by_name(0, "sub").unwrap());
        assert_eq!(tree.root().disk, big.disk + sub.disk);
    }

    #[test]
    fn hard_links_take_disk_space_once() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("a/original.bin"), 200_000);
        fs::hard_link(
            temp.path().join("a/original.bin"),
            temp.path().join("b-link.bin"),
        )
        .unwrap();

        let tree = scan_ok(temp.path());
        let one = tree
            .node(
                tree.child_by_name(tree.child_by_name(0, "a").unwrap(), "original.bin")
                    .unwrap(),
            )
            .disk;
        let two = tree.node(tree.child_by_name(0, "b-link.bin").unwrap()).disk;
        // Logical sizes add up (two names), the disk is spent once.
        assert_eq!(tree.root().size, 400_000);
        assert!(one >= 200_000 || two >= 200_000);
        assert_eq!(tree.root().disk, one + two);
        assert!(
            one == 0 || two == 0,
            "one name must be free: {one} and {two}"
        );
        // Both names are marked, so the UI can explain the 0.
        let first = tree
            .child_by_name(tree.child_by_name(0, "a").unwrap(), "original.bin")
            .unwrap();
        assert!(tree.node(first).hard_linked);
        assert!(
            tree.node(tree.child_by_name(0, "b-link.bin").unwrap())
                .hard_linked
        );
    }

    #[test]
    fn plain_files_are_not_marked_as_hard_links() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("one.bin"), 10_000);
        write(&temp.path().join("two.bin"), 10_000);

        let tree = scan_ok(temp.path());
        assert!(
            !tree
                .node(tree.child_by_name(0, "one.bin").unwrap())
                .hard_linked
        );
        assert!(
            !tree
                .node(tree.child_by_name(0, "two.bin").unwrap())
                .hard_linked
        );
    }

    #[test]
    fn removing_an_item_also_lowers_the_disk_total() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("keep.bin"), 50_000);
        write(&temp.path().join("drop/big.bin"), 90_000);

        let mut tree = scan_ok(temp.path());
        let before = tree.root().disk;
        let drop = tree.child_by_name(0, "drop").unwrap();
        let dropped = tree.node(drop).disk;
        assert!(dropped >= 90_000);
        assert!(tree.remove(drop));
        assert_eq!(tree.root().disk, before - dropped);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_subfolder_is_kept_with_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("ok.bin"), 3);
        let locked = temp.path().join("locked");
        write(&locked.join("secret.bin"), 9);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // Root ignores permissions, so the folder stays readable there; skip in that case.
        let readable = fs::read_dir(&locked).is_ok();

        let tree = scan_ok(temp.path());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if readable {
            return;
        }
        let id = tree.child_by_name(0, "locked").unwrap();
        assert!(tree.node(id).error.is_some());
        assert_eq!(tree.root().errors, 1);
        assert_eq!(tree.root().size, 3);
    }

    #[test]
    fn a_folder_over_a_thousand_levels_deep_is_scanned_and_summed() {
        let temp = tempfile::tempdir().unwrap();
        let mut deepest = temp.path().to_path_buf();
        // Stays under the 4096-character path limit Linux enforces.
        for _ in 0..1_200 {
            deepest.push("d");
        }
        if fs::create_dir_all(&deepest).is_err() {
            return; // this file system or path limit cannot hold it
        }
        write(&deepest.join("bottom.bin"), 11);
        write(&temp.path().join("top.bin"), 5);

        let tree = scan_ok(temp.path());

        assert_eq!(tree.root().size, 16);
        assert_eq!(tree.root().files, 2);
        assert_eq!(tree.root().dirs, 1_200);
        let mut at = 0;
        for _ in 0..1_200 {
            at = tree.child_by_name(at, "d").unwrap();
        }
        assert_eq!(tree.node(at).size, 11);
        assert_eq!(tree.path(at), deepest);
    }

    #[test]
    fn an_in_memory_tree_far_deeper_than_any_path_is_held_and_dropped() {
        let tree = test_support::deep_chain(300_000);
        assert_eq!(tree.len(), 300_002);
        assert_eq!(tree.root().dirs, 300_000);
        drop(tree);
    }
}
