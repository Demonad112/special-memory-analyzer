//! Deleting several items at once: a dry-run [`preview`] that says what would happen to each
//! one without touching the disk, and [`run`], which deletes one item at a time and reports each
//! result on its own so one locked file does not stop the rest.

use crate::{delete_permanently, recycle, Protected};
use scan_core::{Kind, NodeId, Tree};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreviewStatus {
    /// Will be deleted.
    Go,
    /// On the never-delete list; see the reason.
    Protected,
    /// A symlink or junction: left alone, so a delete can never reach what it points to.
    Reparse,
    /// Already deleted, or not part of this scan.
    Gone,
    /// A folder above it is also selected, so it goes with that folder.
    Inside,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewItem {
    pub id: NodeId,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub files: u64,
    pub is_folder: bool,
    pub status: PreviewStatus,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub items: Vec<PreviewItem>,
    /// Items that will be deleted.
    pub go_count: u64,
    /// Space on disk those items take.
    pub go_size: u64,
}

/// What a delete of `ids` would do. Repeated ids are listed once, in the order given.
pub fn preview(tree: &Tree, protected: &Protected, ids: &[NodeId]) -> Preview {
    let root = tree.path(0);
    let mut seen = HashSet::new();
    let mut items: Vec<PreviewItem> = Vec::new();
    for &id in ids.iter().filter(|&&id| seen.insert(id)) {
        if id == 0 || !tree.is_live(id) {
            items.push(PreviewItem {
                id,
                name: String::new(),
                path: String::new(),
                size: 0,
                files: 0,
                is_folder: false,
                status: PreviewStatus::Gone,
                reason: Some("Already gone. Scan again to refresh.".into()),
            });
            continue;
        }
        let node = tree.node(id);
        let path = tree.path(id);
        let (status, reason) = if node.kind == Kind::Link {
            (
                PreviewStatus::Reparse,
                Some(
                    "A shortcut or junction. Skipped, so what it points to is never touched."
                        .into(),
                ),
            )
        } else {
            match protected.check(&path, &root) {
                Ok(()) => (PreviewStatus::Go, None),
                Err(reason) => (PreviewStatus::Protected, Some(reason)),
            }
        };
        items.push(PreviewItem {
            id,
            name: node.name.clone(),
            path: path.display().to_string(),
            size: node.disk,
            files: node.files,
            is_folder: node.kind == Kind::Dir,
            status,
            reason,
        });
    }

    // An item inside another selected, deletable folder goes with it.
    let going: HashSet<NodeId> = items
        .iter()
        .filter(|item| item.status == PreviewStatus::Go)
        .map(|item| item.id)
        .collect();
    for item in &mut items {
        if item.status != PreviewStatus::Go {
            continue;
        }
        let mut above = tree.parent(item.id);
        while let Some(parent) = above {
            if going.contains(&parent) {
                item.status = PreviewStatus::Inside;
                item.reason = Some("Goes with the folder that contains it.".into());
                break;
            }
            above = tree.parent(parent);
        }
    }

    let go = items.iter().filter(|item| item.status == PreviewStatus::Go);
    Preview {
        go_count: go.clone().count() as u64,
        go_size: go.map(|item| item.size).sum(),
        items,
    }
}

/// One item to delete: its id in the scan, where it is, and what it was worth in the scan.
#[derive(Clone, Debug)]
pub struct Target {
    pub id: NodeId,
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Done,
    /// Open in another program.
    InUse,
    AccessDenied,
    NotFound,
    /// Refused before touching anything: protected, or a link.
    Skipped,
    Failed,
}

impl Outcome {
    pub fn code(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::InUse => "inUse",
            Outcome::AccessDenied => "accessDenied",
            Outcome::NotFound => "notFound",
            Outcome::Skipped => "skipped",
            Outcome::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemResult {
    pub id: NodeId,
    pub path: String,
    pub size: u64,
    pub outcome: Outcome,
    pub message: Option<String>,
}

/// Deletes each target in turn. The protected list is checked again here, and a path that is
/// now a link (or no longer there) is not touched, whatever the preview said.
pub fn run(
    targets: &[Target],
    permanent: bool,
    protected: &Protected,
    scan_root: &Path,
) -> Vec<ItemResult> {
    targets
        .iter()
        .map(|target| {
            let (outcome, message) = delete_one(target, permanent, protected, scan_root);
            ItemResult {
                id: target.id,
                path: target.path.display().to_string(),
                size: target.size,
                outcome,
                message,
            }
        })
        .collect()
}

fn delete_one(
    target: &Target,
    permanent: bool,
    protected: &Protected,
    scan_root: &Path,
) -> (Outcome, Option<String>) {
    let path = target.path.as_path();
    if let Err(reason) = protected.check(path, scan_root) {
        return (Outcome::Skipped, Some(reason));
    }
    if fs::symlink_metadata(path).is_err() {
        return (Outcome::NotFound, Some("It was already gone.".into()));
    }
    if is_reparse_point(path) {
        return (
            Outcome::Skipped,
            Some("It is a shortcut or junction now, so it was left alone.".into()),
        );
    }
    let attempt = if permanent {
        delete_permanently(path)
    } else {
        recycle(path)
    };
    // A delete that errors can still have taken the whole item, and one that reports success
    // can leave part of a folder behind.
    if fs::symlink_metadata(path).is_err() {
        return (Outcome::Done, None);
    }
    let detail = attempt.err();
    match probe(path) {
        Some((outcome, message)) => (outcome, Some(message)),
        None => (
            Outcome::Failed,
            Some(detail.unwrap_or_else(|| format!("{} is still there.", path.display()))),
        ),
    }
}

/// True for a symlink or junction (a Windows reparse point).
pub fn is_reparse_point(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        fs::symlink_metadata(path)
            .is_ok_and(|meta| meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    }
    #[cfg(not(windows))]
    {
        fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
    }
}

/// Files looked at when working out why a folder would not delete.
const PROBE_LIMIT: usize = 5_000;

/// Why `path` (or a file in it) cannot be deleted: opened for writing, a sharing violation
/// means another program has it, and a permission error means access denied.
fn probe(path: &Path) -> Option<(Outcome, String)> {
    let mut budget = PROBE_LIMIT;
    let mut stack = vec![path.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(meta) = fs::symlink_metadata(&at) else {
            continue;
        };
        if meta.is_dir() && !is_reparse_point(&at) {
            if let Ok(entries) = fs::read_dir(&at) {
                stack.extend(entries.flatten().map(|entry| entry.path()));
            }
        } else if meta.is_file() {
            budget = budget.saturating_sub(1);
            if let Err(err) = fs::OpenOptions::new().read(true).write(true).open(&at) {
                const SHARING_VIOLATION: i32 = 32;
                const LOCK_VIOLATION: i32 = 33;
                return match err.raw_os_error() {
                    Some(SHARING_VIOLATION | LOCK_VIOLATION) => Some((
                        Outcome::InUse,
                        format!("{} is open in another program.", at.display()),
                    )),
                    _ if err.kind() == std::io::ErrorKind::PermissionDenied => Some((
                        Outcome::AccessDenied,
                        format!("Access denied: {}.", at.display()),
                    )),
                    _ => None,
                };
            }
            if budget == 0 {
                break;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use scan_core::{scan, ScanState};

    fn write(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; 5000]).unwrap();
    }

    fn id(tree: &Tree, names: &[&str]) -> NodeId {
        names.iter().fold(0, |at, name| {
            tree.child_by_name(at, name)
                .unwrap_or_else(|| panic!("{name} missing"))
        })
    }

    #[test]
    fn preview_labels_each_item_and_never_deletes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("a/one.bin"));
        write(&root.join("a/two.bin"));
        write(&root.join("b.bin"));
        write(&root.join("keep/x.bin"));
        let tree = scan(root, &ScanState::default()).unwrap();
        let keep = root.join("keep").display().to_string();
        let protected = Protected::new(&[keep.as_str()], &[], &[]);

        let ids = [
            id(&tree, &["a"]),
            id(&tree, &["a", "one.bin"]),
            id(&tree, &["b.bin"]),
            id(&tree, &["keep"]),
            id(&tree, &["b.bin"]),
            0,
            9_999,
        ];
        let plan = preview(&tree, &protected, &ids);
        let status: Vec<PreviewStatus> = plan.items.iter().map(|item| item.status).collect();
        assert_eq!(
            status,
            [
                PreviewStatus::Go,
                PreviewStatus::Inside,
                PreviewStatus::Go,
                PreviewStatus::Protected,
                PreviewStatus::Gone,
                PreviewStatus::Gone,
            ]
        );
        assert_eq!(plan.go_count, 2);
        let a = tree.node(id(&tree, &["a"])).disk;
        let b = tree.node(id(&tree, &["b.bin"])).disk;
        assert_eq!(plan.go_size, a + b);
        // The dry run touched nothing.
        assert!(root.join("a/one.bin").exists() && root.join("b.bin").exists());
    }

    #[test]
    fn run_deletes_each_item_and_reports_the_missing_and_protected_ones() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("a/one.bin"));
        write(&root.join("b.bin"));
        write(&root.join("keep/x.bin"));
        let keep = root.join("keep").display().to_string();
        let protected = Protected::new(&[keep.as_str()], &[], &[]);
        let target = |path: PathBuf| Target {
            id: 1,
            path,
            size: 1,
        };

        let results = run(
            &[
                target(root.join("a")),
                target(root.join("b.bin")),
                target(root.join("keep")),
                target(root.join("nope.bin")),
            ],
            true,
            &protected,
            root,
        );
        let outcomes: Vec<Outcome> = results.iter().map(|result| result.outcome).collect();
        assert_eq!(
            outcomes,
            [
                Outcome::Done,
                Outcome::Done,
                Outcome::Skipped,
                Outcome::NotFound
            ]
        );
        assert!(!root.join("a").exists() && !root.join("b.bin").exists());
        assert!(root.join("keep/x.bin").exists());
    }

    #[test]
    fn items_outside_the_scanned_folder_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        write(&other.path().join("x.bin"));
        let results = run(
            &[Target {
                id: 1,
                path: other.path().join("x.bin"),
                size: 1,
            }],
            true,
            &Protected::default(),
            temp.path(),
        );
        assert_eq!(results[0].outcome, Outcome::Skipped);
        assert!(other.path().join("x.bin").exists());
    }

    #[cfg(windows)]
    fn junction(link: &Path, target: &Path) {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(status.status.success(), "mklink /J failed");
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_in_the_selection_is_refused_and_its_target_survives() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        write(&outside.join("precious.bin"));
        fs::create_dir_all(&root).unwrap();
        junction(&root.join("link"), &outside);

        let tree = scan(&root, &ScanState::default()).unwrap();
        let link = id(&tree, &["link"]);
        let plan = preview(&tree, &Protected::default(), &[link]);
        assert_eq!(plan.items[0].status, PreviewStatus::Reparse);
        assert_eq!(plan.go_count, 0);

        // Even with a stale preview, run() leaves the junction and its target alone.
        let results = run(
            &[Target {
                id: link,
                path: root.join("link"),
                size: 0,
            }],
            true,
            &Protected::default(),
            &root,
        );
        assert_eq!(results[0].outcome, Outcome::Skipped);
        assert!(outside.join("precious.bin").exists());
        assert!(root.join("link").exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_locked_file_is_reported_as_in_use_and_stays() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("held.bin"));
        write(&root.join("free.bin"));
        // Share mode 0: nobody else may open, write or delete it.
        let _held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(root.join("held.bin"))
            .unwrap();

        let results = run(
            &[
                Target {
                    id: 1,
                    path: root.join("held.bin"),
                    size: 1,
                },
                Target {
                    id: 2,
                    path: root.join("free.bin"),
                    size: 1,
                },
            ],
            true,
            &Protected::default(),
            root,
        );
        assert_eq!(results[0].outcome, Outcome::InUse);
        assert!(root.join("held.bin").exists());
        // One locked file does not stop the next.
        assert_eq!(results[1].outcome, Outcome::Done);
    }
}
