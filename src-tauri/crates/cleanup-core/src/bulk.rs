//! Deleting several items at once: a dry-run [`preview`] that says what would happen to each
//! one without touching the disk, and [`run`], which deletes one item at a time and reports each
//! result on its own so one locked file does not stop the rest.

use crate::{delete_permanently, recycle, Protected};
use scan_core::{Kind, NodeId, Tree};
use serde::Serialize;
use std::collections::hash_map::RandomState;
use std::collections::HashSet;
use std::fs;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A run over more items than this needs the confirmation token from [`preview`].
pub const BRAKE_ITEMS: u64 = 200;
/// A run over more space than this (1 GiB) needs the confirmation token too.
pub const BRAKE_BYTES: u64 = 1 << 30;
/// Items named in the second confirmation.
const BRAKE_BIGGEST: usize = 3;

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
    /// Set when the run is big enough that [`run`] refuses without the token inside.
    pub brake: Option<Brake>,
}

/// The second confirmation for a big run.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Brake {
    /// Handed back to [`run`] to show the second confirmation was answered.
    pub token: String,
    /// The largest items that will go, biggest first.
    pub biggest: Vec<PreviewItem>,
}

/// True when `count` items or `size` bytes are enough to need the second confirmation.
pub fn needs_brake(count: u64, size: u64) -> bool {
    count > BRAKE_ITEMS || size > BRAKE_BYTES
}

/// A token for exactly this set of targets. It changes with the secret of each app run, so one
/// from an earlier run, or for a different selection, does not match.
fn token_for(targets: impl IntoIterator<Item = (NodeId, String)>) -> String {
    static SECRET: OnceLock<RandomState> = OnceLock::new();
    let mut targets: Vec<(NodeId, String)> = targets.into_iter().collect();
    targets.sort();
    format!(
        "{:016x}",
        SECRET.get_or_init(RandomState::new).hash_one(&targets)
    )
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
    let go_count = go.clone().count() as u64;
    let go_size: u64 = go.clone().map(|item| item.size).sum();
    let brake = needs_brake(go_count, go_size).then(|| {
        let mut biggest: Vec<PreviewItem> = go.clone().cloned().collect();
        biggest.sort_by_key(|item| std::cmp::Reverse(item.size));
        biggest.truncate(BRAKE_BIGGEST);
        Brake {
            token: token_for(go.map(|item| (item.id, item.path.clone()))),
            biggest,
        }
    });
    Preview {
        go_count,
        go_size,
        brake,
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
///
/// A run over [`BRAKE_ITEMS`] items or [`BRAKE_BYTES`] bytes is refused, before anything is
/// touched, unless `token` is the one [`preview`] issued for these same targets.
pub fn run(
    targets: &[Target],
    permanent: bool,
    protected: &Protected,
    scan_root: &Path,
    token: Option<&str>,
) -> Result<Vec<ItemResult>, String> {
    let size = targets.iter().map(|target| target.size).sum();
    if needs_brake(targets.len() as u64, size) {
        let expected = token_for(
            targets
                .iter()
                .map(|target| (target.id, target.path.display().to_string())),
        );
        if token != Some(expected.as_str()) {
            return Err(format!(
                "That is {} items and over the limit for one run. Review the biggest items and confirm again.",
                targets.len()
            ));
        }
    }
    Ok(targets
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
        .collect())
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
    if has_reparse_ancestor(path, scan_root) {
        return (
            Outcome::Skipped,
            Some("A folder above it is a shortcut or junction now, so it was left alone.".into()),
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

/// True when a folder between `scan_root` (exclusive) and `path` (exclusive) is a link now. The
/// scan saw plain folders; a swapped parent would send the delete through to another place.
fn has_reparse_ancestor(path: &Path, scan_root: &Path) -> bool {
    // Compare depths, not prefixes, so a differently cased root still stops the walk.
    let root_depth = scan_root.components().count();
    path.ancestors()
        .skip(1)
        .take_while(|ancestor| ancestor.components().count() > root_depth)
        .any(is_reparse_point)
}

/// True for a symlink or junction (a Windows reparse point).
pub(crate) fn is_reparse_point(path: &Path) -> bool {
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
            None,
        )
        .unwrap();
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
            None,
        )
        .unwrap();
        assert_eq!(results[0].outcome, Outcome::Skipped);
        assert!(other.path().join("x.bin").exists());
    }

    fn many(root: &Path, count: usize) -> (Tree, Vec<NodeId>) {
        for n in 0..count {
            write(&root.join(format!("f{n}.bin")));
        }
        let tree = scan(root, &ScanState::default()).unwrap();
        let ids = (0..count)
            .map(|n| id(&tree, &[format!("f{n}.bin").as_str()]))
            .collect();
        (tree, ids)
    }

    fn targets(tree: &Tree, ids: &[NodeId]) -> Vec<Target> {
        ids.iter()
            .map(|&id| Target {
                id,
                path: tree.path(id),
                size: tree.node(id).disk,
            })
            .collect()
    }

    #[test]
    fn a_run_of_exactly_the_limit_needs_no_token() {
        let temp = tempfile::tempdir().unwrap();
        let (tree, ids) = many(temp.path(), BRAKE_ITEMS as usize);
        let plan = preview(&tree, &Protected::default(), &ids);
        assert!(plan.brake.is_none());
        let results = run(
            &targets(&tree, &ids),
            true,
            &Protected::default(),
            temp.path(),
            None,
        )
        .unwrap();
        assert!(results.iter().all(|result| result.outcome == Outcome::Done));
    }

    #[test]
    fn over_the_item_limit_run_refuses_without_the_token_and_proceeds_with_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (tree, ids) = many(root, BRAKE_ITEMS as usize + 2);
        let protected = Protected::default();
        let picked = &ids[..BRAKE_ITEMS as usize + 1];
        let brake = preview(&tree, &protected, picked)
            .brake
            .expect("201 items trip the brake");
        assert_eq!(brake.biggest.len(), 3);
        let all = targets(&tree, picked);

        // No token, a made-up one, and one issued for a different selection: nothing is touched.
        assert!(run(&all, true, &protected, root, None).is_err());
        assert!(run(&all, true, &protected, root, Some("nope")).is_err());
        let other = preview(&tree, &protected, &ids[1..]).brake.unwrap();
        assert!(run(&all, true, &protected, root, Some(&other.token)).is_err());
        assert!(root.join("f0.bin").exists() && root.join("f200.bin").exists());

        let results = run(&all, true, &protected, root, Some(&brake.token)).unwrap();
        assert_eq!(results.len(), 201);
        assert!(results.iter().all(|result| result.outcome == Outcome::Done));
        assert!(!root.join("f0.bin").exists() && root.join("f201.bin").exists());
    }

    #[test]
    fn a_selection_over_one_gigabyte_is_refused_without_the_token() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("a.bin"));
        write(&root.join("b.bin"));
        let big = |name: &str, size: u64| Target {
            id: 1,
            path: root.join(name),
            size,
        };
        let two = [
            big("a.bin", BRAKE_BYTES / 2 + 1),
            big("b.bin", BRAKE_BYTES / 2),
        ];
        assert!(run(&two, true, &Protected::default(), root, None).is_err());
        assert!(root.join("a.bin").exists() && root.join("b.bin").exists());

        // Exactly 1 GiB is not over it.
        let two = [big("a.bin", BRAKE_BYTES / 2), big("b.bin", BRAKE_BYTES / 2)];
        assert!(run(&two, true, &Protected::default(), root, None).is_ok());
    }

    #[test]
    fn the_brake_names_the_three_biggest_items_that_will_go() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (_, _) = many(root, BRAKE_ITEMS as usize + 1);
        for (n, len) in [(5, 300_000), (6, 200_000), (7, 100_000)] {
            fs::write(root.join(format!("f{n}.bin")), vec![b'x'; len]).unwrap();
        }
        let tree = scan(root, &ScanState::default()).unwrap();
        let ids: Vec<NodeId> = (0..=BRAKE_ITEMS as usize)
            .map(|n| id(&tree, &[format!("f{n}.bin").as_str()]))
            .collect();
        let brake = preview(&tree, &Protected::default(), &ids).brake.unwrap();
        let names: Vec<&str> = brake
            .biggest
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(names, ["f5.bin", "f6.bin", "f7.bin"]);
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

    /// A directory link: a junction on Windows, a symlink elsewhere.
    fn dir_link(link: &Path, target: &Path) {
        #[cfg(windows)]
        junction(link, target);
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[test]
    fn a_parent_swapped_for_a_link_after_the_scan_is_refused_and_the_other_folder_is_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let other = temp.path().join("other");
        write(&root.join("a/b/file.bin"));
        write(&other.join("b/file.bin"));
        let target = Target {
            id: 1,
            path: root.join("a/b/file.bin"),
            size: 5000,
        };

        // Scan time: `a` is a plain folder. Delete time: it has become a link to `other`.
        fs::remove_dir_all(root.join("a")).unwrap();
        dir_link(&root.join("a"), &other);

        let results = run(&[target], true, &Protected::default(), &root, None).unwrap();
        assert_eq!(results[0].outcome, Outcome::Skipped);
        assert!(other.join("b/file.bin").exists());
    }

    #[test]
    fn a_plain_parent_chain_is_not_a_reparse_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        write(&root.join("a/b/file.bin"));
        assert!(!has_reparse_ancestor(&root.join("a/b/file.bin"), &root));
        assert!(!has_reparse_ancestor(&root.join("a"), &root));
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
            None,
        )
        .unwrap();
        assert_eq!(results[0].outcome, Outcome::Skipped);
        assert!(outside.join("precious.bin").exists());
        assert!(root.join("link").exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_recycle_delete_takes_the_file_and_folder_out_and_leaves_the_rest() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("old.bin"));
        write(&root.join("folder/inner.bin"));
        write(&root.join("keep.bin"));

        let results = run(
            &[
                Target {
                    id: 1,
                    path: root.join("old.bin"),
                    size: 1,
                },
                Target {
                    id: 2,
                    path: root.join("folder"),
                    size: 1,
                },
            ],
            false,
            &Protected::default(),
            root,
            None,
        )
        .unwrap();
        assert_eq!(results[0].outcome, Outcome::Done);
        assert_eq!(results[1].outcome, Outcome::Done);
        assert!(!root.join("old.bin").exists() && !root.join("folder").exists());
        assert!(root.join("keep.bin").exists());
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
            None,
        )
        .unwrap();
        assert_eq!(results[0].outcome, Outcome::InUse);
        assert!(root.join("held.bin").exists());
        // One locked file does not stop the next.
        assert_eq!(results[1].outcome, Outcome::Done);
    }
}
