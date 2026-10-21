use crate::list::{list_dir, Entry};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// Files: another name for the same data exists in this scan (hard link). Only the first name
    /// keeps its [`Node::disk`].
    pub hard_linked: bool,
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
            hard_linked: false,
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

    let mut tree = scan_dir(root, root.display().to_string(), state);
    if state.cancel.is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    if tree.node.error.is_some() && tree.children.is_empty() {
        return Err(ScanError::Unreadable {
            path: root.to_path_buf(),
            message: tree.node.error.unwrap_or_default(),
        });
    }
    count_hard_links_once(&mut tree);
    total_up(&mut tree);
    Ok(flatten(tree))
}

/// A node with owned children, built during the walk and then flattened into a [`Tree`].
struct Owned {
    node: Node,
    children: Vec<Owned>,
    /// Volume-unique id of a file, 0 when unknown; used to find hard links.
    file_id: u64,
}

impl Owned {
    fn leaf(entry: Entry) -> Self {
        let mut node = Node::new(entry.name, entry.kind);
        node.size = entry.size;
        node.disk = entry.disk;
        node.files = u64::from(entry.kind == Kind::File);
        node.cloud_files = u64::from(entry.cloud);
        node.cloud_bytes = if entry.cloud { entry.size } else { 0 };
        node.modified_ms = entry.modified_ms;
        Self {
            node,
            children: Vec::new(),
            file_id: entry.file_id,
        }
    }
}

/// A file with several names (hard links) takes disk space once. The first name in walk order
/// keeps its size on disk; the others show 0, so no folder total counts the data twice. Files are
/// matched on file id and size together, because not every file system hands out reliable ids.
fn count_hard_links_once(root: &mut Owned) {
    fn collect(dir: &Owned, ids: &mut Vec<(u64, u64)>) {
        for child in &dir.children {
            if child.node.kind == Kind::File && child.file_id != 0 {
                ids.push((child.file_id, child.node.size));
            }
            collect(child, ids);
        }
    }
    fn zero_repeats(
        dir: &mut Owned,
        repeated: &HashSet<(u64, u64)>,
        seen: &mut HashSet<(u64, u64)>,
    ) {
        for child in &mut dir.children {
            if child.node.kind == Kind::File && child.file_id != 0 {
                let key = (child.file_id, child.node.size);
                if repeated.contains(&key) {
                    child.node.hard_linked = true;
                    if !seen.insert(key) {
                        child.node.disk = 0;
                    }
                }
            }
            zero_repeats(child, repeated, seen);
        }
    }

    let mut ids = Vec::new();
    collect(root, &mut ids);
    ids.sort_unstable();
    let repeated: HashSet<(u64, u64)> = ids
        .windows(2)
        .filter(|pair| pair[0] == pair[1])
        .map(|pair| pair[0])
        .collect();
    if !repeated.is_empty() {
        zero_repeats(root, &repeated, &mut HashSet::new());
    }
}

/// Adds every child into its folder and sorts each folder's children, largest first.
fn total_up(dir: &mut Owned) {
    let own_error = u64::from(dir.node.error.is_some());
    for child in &mut dir.children {
        total_up(child);
    }
    let node = &mut dir.node;
    node.errors = own_error;
    for child in &dir.children {
        node.size += child.node.size;
        node.disk += child.node.disk;
        node.files += child.node.files;
        node.dirs += child.node.dirs + u64::from(child.node.kind == Kind::Dir);
        node.cloud_files += child.node.cloud_files;
        node.cloud_bytes += child.node.cloud_bytes;
        node.errors += child.node.errors;
    }
    dir.children.sort_by(|a, b| {
        b.node
            .size
            .cmp(&a.node.size)
            .then_with(|| a.node.name.cmp(&b.node.name))
    });
}

fn scan_dir(path: &Path, name: String, state: &ScanState) -> Owned {
    let mut dir = Owned {
        node: Node::new(name, Kind::Dir),
        children: Vec::new(),
        file_id: 0,
    };
    if state.cancel.is_cancelled() {
        return dir;
    }
    state.dirs.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut current) = state.current.try_lock() {
        *current = path.display().to_string();
    }

    let listing = match list_dir(path) {
        Ok(listing) => listing,
        Err(err) => {
            dir.node.error = Some(err.to_string());
            dir.node.error_code = err.raw_os_error();
            dir.node.errors = 1;
            return dir;
        }
    };
    if let Some(err) = &listing.error {
        dir.node.error = Some(err.to_string());
        dir.node.error_code = err.raw_os_error();
    }

    let mut subdirs = Vec::new();
    let mut files = 0;
    let mut bytes = 0;
    for entry in listing.entries {
        if state.ignore_junk && is_junk(&entry.name, entry.kind == Kind::Dir) {
            continue;
        }
        if entry.kind == Kind::Dir {
            subdirs.push((path.join(&entry.name), entry.name));
        } else {
            if entry.kind == Kind::File {
                files += 1;
                bytes += entry.size;
            }
            dir.children.push(Owned::leaf(entry));
        }
    }
    state.files.fetch_add(files, Ordering::Relaxed);
    state.bytes.fetch_add(bytes, Ordering::Relaxed);

    let subdirs: Vec<Owned> = subdirs
        .into_par_iter()
        .map(|(path, name)| scan_dir(&path, name, state))
        .collect();
    dir.children.extend(subdirs);
    dir
}

/// Breadth-first, so every node's children end up next to each other.
fn flatten(root: Owned) -> Tree {
    let mut nodes = vec![root.node];
    let mut queue = VecDeque::from([(0usize, root.children)]);
    while let Some((id, children)) = queue.pop_front() {
        nodes[id].first_child = nodes.len() as NodeId;
        nodes[id].child_count = children.len() as u32;
        for mut child in children {
            child.node.parent = id as NodeId;
            nodes.push(child.node);
            queue.push_back((nodes.len() - 1, child.children));
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
}
