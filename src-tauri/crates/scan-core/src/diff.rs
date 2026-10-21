use crate::scan::{Kind, Node, NodeId, Tree};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

/// How a path compares between the source (left) and the destination (right).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffStatus {
    Same,
    /// A file with another size, or a folder with any difference below it.
    Different,
    /// Only in the source: missing at the destination.
    OnlyLeft,
    /// Only in the destination.
    OnlyRight,
    /// A file on one side and a folder (or link) on the other.
    KindMismatch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffSide {
    pub size: u64,
    pub files: u64,
    pub dirs: u64,
}

impl DiffSide {
    fn of(node: &Node) -> Self {
        Self {
            size: node.size,
            files: node.files,
            dirs: node.dirs,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DiffNode {
    pub name: String,
    pub kind: Kind,
    pub left: Option<DiffSide>,
    pub right: Option<DiffSide>,
    pub status: DiffStatus,
    /// Source files at or below this path that the destination does not have.
    pub missing: u64,
    /// Bytes of those missing files.
    pub missing_bytes: u64,
    /// Destination files at or below this path that the source does not have.
    pub extra: u64,
    /// Files at or below this path that exist on both sides with different sizes.
    pub different: u64,
    /// Either side holds cloud-only placeholders here.
    pub cloud: bool,
    /// Either side could not be read completely here.
    pub error: Option<String>,
    /// The destination's spelling of the name when it differs from the source's (case only).
    right_name: Option<String>,
    parent: NodeId,
    first_child: NodeId,
    child_count: u32,
}

/// One row as the UI receives it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffRow {
    pub id: NodeId,
    pub name: String,
    pub kind: Kind,
    pub left: Option<DiffSide>,
    pub right: Option<DiffSide>,
    pub status: DiffStatus,
    pub missing: u64,
    pub missing_bytes: u64,
    pub extra: u64,
    pub different: u64,
    pub cloud: bool,
    pub error: Option<String>,
    pub has_children: bool,
}

/// Which of the two compared folders a path belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffSummary {
    pub left: DiffSide,
    pub right: DiffSide,
    pub missing: u64,
    pub missing_bytes: u64,
    pub extra: u64,
    pub different: u64,
    pub left_errors: u64,
    pub right_errors: u64,
    pub left_cloud: u64,
    pub right_cloud: u64,
}

/// A source file (or empty folder) the destination lacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingEntry {
    /// Relative to the source; an empty folder ends with a separator.
    pub path: String,
    pub size: u64,
}

/// The comparison of two trees, as a flat arena (root is 0). Children are sorted largest first.
#[derive(Clone, Debug)]
pub struct DiffTree {
    nodes: Vec<DiffNode>,
    summary: DiffSummary,
}

impl DiffTree {
    pub fn summary(&self) -> &DiffSummary {
        &self.summary
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn node(&self, id: NodeId) -> Option<&DiffNode> {
        self.nodes.get(id as usize)
    }

    pub fn row(&self, id: NodeId) -> Option<DiffRow> {
        let node = self.node(id)?;
        Some(DiffRow {
            id,
            name: node.name.clone(),
            kind: node.kind,
            left: node.left,
            right: node.right,
            status: node.status,
            missing: node.missing,
            missing_bytes: node.missing_bytes,
            extra: node.extra,
            different: node.different,
            cloud: node.cloud,
            error: node.error.clone(),
            has_children: node.child_count > 0,
        })
    }

    /// The path of `id` relative to the compared folder on `side`, or `None` when that side
    /// does not have it. The root is the empty path.
    pub fn relative_path(&self, id: NodeId, side: Side) -> Option<PathBuf> {
        let node = self.node(id)?;
        let present = match side {
            Side::Left => node.left.is_some(),
            Side::Right => node.right.is_some(),
        };
        if !present {
            return None;
        }
        let mut names = Vec::new();
        let mut current = id;
        while current != 0 {
            let node = &self.nodes[current as usize];
            let name = match side {
                Side::Right => node.right_name.as_deref().unwrap_or(&node.name),
                Side::Left => &node.name,
            };
            names.push(name);
            current = node.parent;
        }
        Some(names.iter().rev().collect())
    }

    /// Every source file the destination lacks, as paths relative to the source, in table order.
    /// Empty missing folders are listed too, ending with a separator.
    pub fn missing_paths(&self) -> Vec<String> {
        self.missing_entries()
            .into_iter()
            .map(|entry| entry.path)
            .collect()
    }

    /// [`DiffTree::missing_paths`] with the size of each source file (0 for an empty folder).
    pub fn missing_entries(&self) -> Vec<MissingEntry> {
        let mut out = Vec::new();
        self.collect_missing(0, false, &mut out);
        out
    }

    fn collect_missing(&self, id: NodeId, inherited: bool, out: &mut Vec<MissingEntry>) {
        let node = &self.nodes[id as usize];
        let missing =
            inherited || matches!(node.status, DiffStatus::OnlyLeft | DiffStatus::KindMismatch);
        if missing && id != 0 {
            let empty_folder = node.kind == Kind::Dir && node.child_count == 0;
            if node.kind != Kind::Dir || empty_folder {
                if let Some(path) = self.relative_path(id, Side::Left) {
                    let mut text = path.display().to_string();
                    if empty_folder {
                        text.push(std::path::MAIN_SEPARATOR);
                    }
                    out.push(MissingEntry {
                        path: text,
                        size: node.left.map_or(0, |side| side.size),
                    });
                }
                return;
            }
        }
        if !missing && node.status == DiffStatus::Same {
            return;
        }
        for child in node.first_child..node.first_child + node.child_count {
            if self.nodes[child as usize].left.is_some() {
                self.collect_missing(child, missing && id != 0, out);
            }
        }
    }

    /// The rows directly below `id`, or `None` when `id` does not exist.
    pub fn children(&self, id: NodeId) -> Option<Vec<DiffRow>> {
        let node = self.node(id)?;
        Some(
            (node.first_child..node.first_child + node.child_count)
                .filter_map(|child| self.row(child))
                .collect(),
        )
    }
}

/// Compares `left` (source) with `right` (destination) by relative path and file size. Names are
/// matched exactly first, then case-insensitively, as Windows treats them.
pub fn compare(left: &Tree, right: &Tree) -> DiffTree {
    let root = diff(String::new(), Some((left, 0)), Some((right, 0)));
    let summary = DiffSummary {
        left: DiffSide::of(left.root()),
        right: DiffSide::of(right.root()),
        missing: root.node.missing,
        missing_bytes: root.node.missing_bytes,
        extra: root.node.extra,
        different: root.node.different,
        left_errors: left.root().errors,
        right_errors: right.root().errors,
        left_cloud: left.root().cloud_files,
        right_cloud: right.root().cloud_files,
    };
    DiffTree {
        nodes: flatten(root),
        summary,
    }
}

struct Owned {
    node: DiffNode,
    children: Vec<Owned>,
}

type Pick<'a> = Option<(&'a Tree, NodeId)>;

fn diff(name: String, left: Pick<'_>, right: Pick<'_>) -> Owned {
    let left_node = left.map(|(tree, id)| tree.node(id));
    let right_node = right.map(|(tree, id)| tree.node(id));
    let kind = left_node.or(right_node).map_or(Kind::Dir, |node| node.kind);

    let children: Vec<Owned> = pair_children(left, right)
        .into_iter()
        .map(|(child_left, child_right)| {
            let name = child_left
                .or(child_right)
                .map(|(tree, id)| tree.node(id).name.clone())
                .unwrap_or_default();
            diff(name, child_left, child_right)
        })
        .collect();

    let mut node = DiffNode {
        name,
        kind,
        left: left_node.map(DiffSide::of),
        right: right_node.map(DiffSide::of),
        status: DiffStatus::Same,
        missing: 0,
        missing_bytes: 0,
        extra: 0,
        different: 0,
        cloud: left_node.is_some_and(|node| node.cloud_files > 0)
            || right_node.is_some_and(|node| node.cloud_files > 0),
        error: left_node
            .and_then(|node| node.error.clone())
            .or_else(|| right_node.and_then(|node| node.error.clone())),
        right_name: match (left_node, right_node) {
            (Some(l), Some(r)) if l.name != r.name => Some(r.name.clone()),
            _ => None,
        },
        parent: 0,
        first_child: 0,
        child_count: 0,
    };

    match (left_node, right_node) {
        (Some(l), None) => {
            node.status = DiffStatus::OnlyLeft;
            node.missing = l.files;
            node.missing_bytes = l.size;
        }
        (None, Some(r)) => {
            node.status = DiffStatus::OnlyRight;
            node.extra = r.files;
        }
        (Some(l), Some(r)) if l.kind != r.kind => {
            node.status = DiffStatus::KindMismatch;
            node.missing = l.files;
            node.missing_bytes = l.size;
            node.extra = r.files;
        }
        (Some(l), Some(r)) if l.kind == Kind::File => {
            if l.size != r.size {
                node.status = DiffStatus::Different;
                node.different = 1;
            }
        }
        _ => {
            for child in &children {
                node.missing += child.node.missing;
                node.missing_bytes += child.node.missing_bytes;
                node.extra += child.node.extra;
                node.different += child.node.different;
            }
            if children
                .iter()
                .any(|child| child.node.status != DiffStatus::Same)
            {
                node.status = DiffStatus::Different;
            }
        }
    }

    let mut children = children;
    children.sort_by(|a, b| {
        largest(&b.node)
            .cmp(&largest(&a.node))
            .then_with(|| a.node.name.cmp(&b.node.name))
    });
    Owned { node, children }
}

fn largest(node: &DiffNode) -> u64 {
    let size = |side: Option<DiffSide>| side.map_or(0, |side| side.size);
    size(node.left).max(size(node.right))
}

/// Children of a side, empty unless that side is a folder.
fn folder_children(side: Pick<'_>) -> Vec<NodeId> {
    match side {
        Some((tree, id)) if tree.node(id).kind == Kind::Dir => tree.node(id).children().collect(),
        _ => Vec::new(),
    }
}

/// Pairs the children of both sides by name: exact match first, then case-insensitive, then the
/// leftovers on each side alone.
fn pair_children<'a>(left: Pick<'a>, right: Pick<'a>) -> Vec<(Pick<'a>, Pick<'a>)> {
    let left_ids = folder_children(left);
    let right_ids = folder_children(right);
    let (Some((left_tree, _)), Some((right_tree, _))) = (left, right) else {
        return left_ids
            .into_iter()
            .map(|id| (left.map(|(tree, _)| (tree, id)), None))
            .chain(
                right_ids
                    .into_iter()
                    .map(|id| (None, right.map(|(tree, _)| (tree, id)))),
            )
            .collect();
    };

    let mut exact: HashMap<&str, usize> = HashMap::new();
    let mut folded: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, &id) in right_ids.iter().enumerate() {
        let name = right_tree.node(id).name.as_str();
        exact.insert(name, index);
        folded.entry(name.to_lowercase()).or_default().push(index);
    }

    let mut used = vec![false; right_ids.len()];
    let mut pairs = Vec::with_capacity(left_ids.len().max(right_ids.len()));
    let mut unmatched = Vec::new();
    for &id in &left_ids {
        let name = left_tree.node(id).name.as_str();
        match exact.get(name) {
            Some(&index) if !used[index] => {
                used[index] = true;
                pairs.push((Some((left_tree, id)), Some((right_tree, right_ids[index]))));
            }
            _ => unmatched.push(id),
        }
    }
    for id in unmatched {
        let name = left_tree.node(id).name.to_lowercase();
        let index = folded
            .get(&name)
            .and_then(|candidates| candidates.iter().copied().find(|&index| !used[index]));
        let right_side = index.map(|index| {
            used[index] = true;
            (right_tree, right_ids[index])
        });
        pairs.push((Some((left_tree, id)), right_side));
    }
    for (index, &id) in right_ids.iter().enumerate() {
        if !used[index] {
            pairs.push((None, Some((right_tree, id))));
        }
    }
    pairs
}

fn flatten(root: Owned) -> Vec<DiffNode> {
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
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{scan, ScanState};
    use std::fs;
    use std::path::Path;

    fn write(path: &Path, bytes: usize) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn compare_dirs(left: &Path, right: &Path) -> DiffTree {
        let state = ScanState::default();
        compare(&scan(left, &state).unwrap(), &scan(right, &state).unwrap())
    }

    fn row<'a>(rows: &'a [DiffRow], name: &str) -> &'a DiffRow {
        rows.iter().find(|row| row.name == name).unwrap()
    }

    #[test]
    fn identical_folders_are_same() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        for root in [left.path(), right.path()] {
            write(&root.join("a.bin"), 10);
            write(&root.join("sub/b.bin"), 20);
        }
        let diff = compare_dirs(left.path(), right.path());
        assert_eq!(diff.node(0).unwrap().status, DiffStatus::Same);
        let summary = diff.summary();
        assert_eq!(
            (summary.missing, summary.extra, summary.different),
            (0, 0, 0)
        );
        assert_eq!(summary.left.size, 30);
        assert_eq!(summary.right.size, 30);
    }

    #[test]
    fn reports_missing_extra_and_different() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        write(&left.path().join("same.bin"), 5);
        write(&right.path().join("same.bin"), 5);
        write(&left.path().join("changed.bin"), 5);
        write(&right.path().join("changed.bin"), 4);
        write(&left.path().join("gone/one.bin"), 1);
        write(&left.path().join("gone/two.bin"), 2);
        write(&left.path().join("part/kept.bin"), 3);
        write(&right.path().join("part/kept.bin"), 3);
        write(&left.path().join("part/lost.bin"), 3);
        write(&right.path().join("new.bin"), 7);

        let diff = compare_dirs(left.path(), right.path());
        let summary = diff.summary();
        assert_eq!(summary.missing, 3);
        assert_eq!(summary.missing_bytes, 6);
        assert_eq!(summary.extra, 1);
        assert_eq!(summary.different, 1);
        assert_eq!(diff.node(0).unwrap().status, DiffStatus::Different);

        let rows = diff.children(0).unwrap();
        assert_eq!(row(&rows, "same.bin").status, DiffStatus::Same);
        assert_eq!(row(&rows, "changed.bin").status, DiffStatus::Different);
        assert_eq!(row(&rows, "new.bin").status, DiffStatus::OnlyRight);
        assert_eq!(row(&rows, "new.bin").left, None);

        let gone = row(&rows, "gone");
        assert_eq!(gone.status, DiffStatus::OnlyLeft);
        assert_eq!(gone.missing, 2);
        assert!(gone.has_children);
        let gone_rows = diff.children(gone.id).unwrap();
        assert!(gone_rows
            .iter()
            .all(|row| row.status == DiffStatus::OnlyLeft));

        let part = row(&rows, "part");
        assert_eq!(part.status, DiffStatus::Different);
        assert_eq!(part.missing, 1);
        assert_eq!(part.left.unwrap().size, 6);
        assert_eq!(part.right.unwrap().size, 3);
    }

    #[test]
    fn empty_missing_folder_marks_parent_different() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        fs::create_dir(left.path().join("empty")).unwrap();
        let diff = compare_dirs(left.path(), right.path());
        assert_eq!(diff.node(0).unwrap().status, DiffStatus::Different);
        assert_eq!(
            row(&diff.children(0).unwrap(), "empty").status,
            DiffStatus::OnlyLeft
        );
    }

    #[test]
    fn file_versus_folder_is_a_kind_mismatch() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        write(&left.path().join("thing/a.bin"), 1);
        write(&left.path().join("thing/b.bin"), 1);
        write(&right.path().join("thing"), 1);
        let diff = compare_dirs(left.path(), right.path());
        let thing = row(&diff.children(0).unwrap(), "thing").clone();
        assert_eq!(thing.status, DiffStatus::KindMismatch);
        assert_eq!(thing.missing, 2);
        assert_eq!(thing.extra, 1);
        assert_eq!(diff.children(thing.id).unwrap().len(), 2);
    }

    #[test]
    fn names_match_case_insensitively() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        write(&left.path().join("Photos/IMG.JPG"), 8);
        write(&right.path().join("photos/img.jpg"), 8);
        let diff = compare_dirs(left.path(), right.path());
        assert_eq!(diff.node(0).unwrap().status, DiffStatus::Same);
        assert_eq!(diff.children(0).unwrap().len(), 1);
    }

    #[test]
    fn builds_paths_and_the_missing_list() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        write(&left.path().join("Docs/a.txt"), 1);
        write(&right.path().join("docs/a.txt"), 1);
        write(&left.path().join("Docs/lost.txt"), 3);
        write(&left.path().join("gone/deep/b.txt"), 2);
        fs::create_dir_all(left.path().join("gone/empty")).unwrap();
        write(&left.path().join("same.txt"), 1);
        write(&right.path().join("same.txt"), 1);
        let diff = compare_dirs(left.path(), right.path());

        let docs = row(&diff.children(0).unwrap(), "Docs").clone();
        let a = row(&diff.children(docs.id).unwrap(), "a.txt").clone();
        assert_eq!(
            diff.relative_path(a.id, Side::Left).unwrap(),
            Path::new("Docs").join("a.txt")
        );
        assert_eq!(
            diff.relative_path(a.id, Side::Right).unwrap(),
            Path::new("docs").join("a.txt")
        );
        let lost = row(&diff.children(docs.id).unwrap(), "lost.txt").clone();
        assert!(diff.relative_path(lost.id, Side::Right).is_none());
        assert_eq!(diff.relative_path(0, Side::Left).unwrap(), Path::new(""));

        let sep = std::path::MAIN_SEPARATOR;
        let mut missing = diff.missing_paths();
        missing.sort();
        assert_eq!(
            missing,
            [
                format!("Docs{sep}lost.txt"),
                format!("gone{sep}deep{sep}b.txt"),
                format!("gone{sep}empty{sep}"),
            ]
        );

        let mut sized = diff.missing_entries();
        sized.sort_by(|a, b| a.path.cmp(&b.path));
        let sizes: Vec<u64> = sized.iter().map(|entry| entry.size).collect();
        assert_eq!(
            sizes,
            [3, 2, 0],
            "files carry their size, an empty folder 0"
        );
    }

    #[test]
    fn unknown_id_has_no_children() {
        let left = tempfile::tempdir().unwrap();
        let diff = compare_dirs(left.path(), left.path());
        assert!(diff.children(999).is_none());
        assert_eq!(diff.children(0).unwrap().len(), 0);
    }
}
