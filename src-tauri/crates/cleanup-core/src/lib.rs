//! Disk Cleanup: what the UI shows of a scanned [`Tree`] (one folder level, the largest files,
//! space by file type), which paths may never be deleted, and the drive list and delete calls.

pub mod bulk;
pub mod junk;
pub mod log;
mod os;
mod protect;
pub mod quick;

pub use os::{delete_permanently, drives, is_elevated, recycle, Drive};
pub use protect::Protected;

use scan_core::{Kind, NodeId, Tree};
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// One file or folder in a listing.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: NodeId,
    pub name: String,
    pub kind: Kind,
    /// Size on disk (whole clusters, compression and sparse aware, hard links once):
    /// online-only (cloud) files count as 0.
    pub size: u64,
    /// Logical size (what Explorer shows as "Size"); cloud-only bytes included.
    pub logical: u64,
    /// Another name for the same data is in this scan; only one name carries the disk space.
    pub hard_linked: bool,
    pub files: u64,
    pub dirs: u64,
    pub cloud_files: u64,
    /// Online-only bytes below this row, left out of `size`.
    pub cloud_bytes: u64,
    pub errors: u64,
    pub error: Option<String>,
    pub modified_ms: Option<u64>,
    pub has_children: bool,
}

impl Row {
    pub fn new(tree: &Tree, id: NodeId) -> Self {
        let node = tree.node(id);
        Self {
            id,
            name: node.name.clone(),
            kind: node.kind,
            size: node.disk,
            logical: node.size,
            hard_linked: node.hard_linked,
            files: node.files,
            dirs: node.dirs,
            cloud_files: node.cloud_files,
            cloud_bytes: node.cloud_bytes,
            errors: node.errors,
            error: node.error.clone(),
            modified_ms: node.modified_ms,
            has_children: tree.live_children(id).next().is_some(),
        }
    }
}

/// What is directly inside folder `id`, largest first. Empty for a file or a removed item.
pub fn rows(tree: &Tree, id: NodeId) -> Vec<Row> {
    if !tree.is_live(id) {
        return Vec::new();
    }
    let mut rows: Vec<Row> = tree
        .live_children(id)
        .map(|child| Row::new(tree, child))
        .collect();
    // Removals can leave siblings out of order.
    rows.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    rows
}

/// A file in the "Largest files" list.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRow {
    pub id: NodeId,
    pub name: String,
    /// The folder it is in.
    pub folder: String,
    pub size: u64,
    pub logical: u64,
    pub hard_linked: bool,
    pub modified_ms: Option<u64>,
}

/// Space used by one file extension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeRow {
    /// Lowercase, without the dot; empty for files with no extension.
    pub extension: String,
    pub size: u64,
    pub logical: u64,
    pub files: u64,
}

/// Whole-scan lists, rebuilt after each delete.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub root: Row,
    pub largest_files: Vec<FileRow>,
    pub file_types: Vec<TypeRow>,
}

pub const LARGEST_FILES: usize = 100;

pub fn overview(tree: &Tree) -> Overview {
    let mut largest = BinaryHeap::new();
    // Per extension: size on disk, logical size, file count.
    let mut types: HashMap<String, (u64, u64, u64)> = HashMap::new();
    let mut stack = vec![0];
    while let Some(id) = stack.pop() {
        for child in tree.live_children(id) {
            let node = tree.node(child);
            match node.kind {
                Kind::Dir => stack.push(child),
                // Links and online-only files take no space here.
                Kind::Link => {}
                Kind::File if node.cloud_files > 0 => {}
                Kind::File => {
                    largest.push(Reverse((node.disk, child)));
                    if largest.len() > LARGEST_FILES {
                        largest.pop();
                    }
                    let entry = types.entry(extension(&node.name)).or_default();
                    entry.0 += node.disk;
                    entry.1 += node.size;
                    entry.2 += 1;
                }
            }
        }
    }

    let mut largest = largest.into_vec();
    // Reverse sorts the biggest first.
    largest.sort();
    let largest_files = largest
        .into_iter()
        .map(|Reverse((size, id))| FileRow {
            id,
            name: tree.node(id).name.clone(),
            folder: tree
                .parent(id)
                .map(|parent| tree.path(parent).display().to_string())
                .unwrap_or_default(),
            size,
            logical: tree.node(id).size,
            hard_linked: tree.node(id).hard_linked,
            modified_ms: tree.node(id).modified_ms,
        })
        .collect();

    let mut file_types: Vec<TypeRow> = types
        .into_iter()
        .map(|(extension, (size, logical, files))| TypeRow {
            extension,
            size,
            logical,
            files,
        })
        .collect();
    file_types.sort_by(|a, b| {
        b.size
            .cmp(&a.size)
            .then_with(|| a.extension.cmp(&b.extension))
    });

    Overview {
        root: Row::new(tree, 0),
        largest_files,
        file_types,
    }
}

/// "Movie.MP4" -> "mp4"; "README" and ".gitignore" -> "".
fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => name[dot + 1..].to_lowercase(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scan_core::{scan, ScanState};
    use std::fs;
    use std::path::Path;

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn extensions_are_lowercase_and_dotfiles_have_none() {
        assert_eq!(extension("Movie.MP4"), "mp4");
        assert_eq!(extension("archive.tar.gz"), "gz");
        assert_eq!(extension("README"), "");
        assert_eq!(extension(".gitignore"), "");
        assert_eq!(extension("trailing."), "");
    }

    #[test]
    fn overview_lists_largest_files_and_types() {
        let temp = tempfile::tempdir().unwrap();
        // Sizes are space on disk, so use files well above one cluster.
        write(&temp.path().join("a.mp4"), 500_000);
        write(&temp.path().join("sub/b.MP4"), 300_000);
        write(&temp.path().join("sub/c.txt"), 100_000);
        write(&temp.path().join("sub/deeper/notes"), 40_000);

        let mut tree = scan(temp.path(), &ScanState::default()).unwrap();
        let view = overview(&tree);
        let names: Vec<&str> = view
            .largest_files
            .iter()
            .map(|file| file.name.as_str())
            .collect();
        assert_eq!(names, ["a.mp4", "b.MP4", "c.txt", "notes"]);
        assert_eq!(
            view.largest_files[1].folder,
            temp.path().join("sub").display().to_string()
        );
        let mp4 = &view.file_types[0];
        assert_eq!((mp4.extension.as_str(), mp4.files), ("mp4", 2));
        assert!(mp4.size >= 800_000);
        assert_eq!(view.file_types.len(), 3);
        let total: u64 = view.file_types.iter().map(|kind| kind.size).sum();
        assert_eq!(view.root.size, total);
        assert!(view.root.size >= 940_000);
        let a_size = view.largest_files[0].size;

        // After a delete the lists and the folder listing leave it out.
        let sub = tree.child_by_name(0, "sub").unwrap();
        assert!(tree.remove(sub));
        let view = overview(&tree);
        assert_eq!(view.largest_files.len(), 1);
        assert_eq!(view.root.size, a_size);
        assert!(rows(&tree, 0).iter().all(|row| row.name != "sub"));
        assert!(rows(&tree, sub).is_empty());
    }
}
