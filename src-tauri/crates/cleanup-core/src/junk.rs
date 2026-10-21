//! The junk finder: walks an already scanned tree (no rescan) and lists things that are usually
//! safe to remove, grouped by kind. It only suggests; nothing is selected or deleted here, and
//! anything [`Protected`] refuses is left out.

use crate::Protected;
use scan_core::{Kind, NodeId, Tree};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Logs and backup files older than this are listed.
pub const OLD_LOG_DAYS: u64 = 30;
/// Setup files in Downloads and dependency folders untouched for this long are listed.
pub const STALE_DAYS: u64 = 90;
/// Items shown per group; the group total still counts every match.
pub const MAX_ITEMS: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JunkKind {
    TempFiles,
    CrashDumps,
    OldLogs,
    Thumbnails,
    Installers,
    DevCaches,
    EmptyFolders,
}

impl JunkKind {
    const ALL: [JunkKind; 7] = [
        JunkKind::TempFiles,
        JunkKind::CrashDumps,
        JunkKind::OldLogs,
        JunkKind::Thumbnails,
        JunkKind::Installers,
        JunkKind::DevCaches,
        JunkKind::EmptyFolders,
    ];

    pub fn title(self) -> &'static str {
        match self {
            JunkKind::TempFiles => "Temporary files",
            JunkKind::CrashDumps => "Crash dumps",
            JunkKind::OldLogs => "Old logs and backups",
            JunkKind::Thumbnails => "Thumbnail caches",
            JunkKind::Installers => "Old setup files",
            JunkKind::DevCaches => "Developer caches",
            JunkKind::EmptyFolders => "Empty folders",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            JunkKind::TempFiles => "*.tmp and Office ~$ lock files.",
            JunkKind::CrashDumps => "*.dmp and *.mdmp files left by crashes.",
            JunkKind::OldLogs => "*.log, *.bak and *.old files not changed for 30 days.",
            JunkKind::Thumbnails => "Thumbs.db and .DS_Store files.",
            JunkKind::Installers => ".exe and .msi files in a Downloads folder, 90 days old.",
            JunkKind::DevCaches => {
                "node_modules of projects untouched for 90 days, and __pycache__ folders, one row per project or environment."
            }
            JunkKind::EmptyFolders => "Folders with nothing in them.",
        }
    }
}

/// One suggested item.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkItem {
    /// The node a tick on this row stands for. For a collapsed row, the first of `ids`.
    pub id: NodeId,
    /// Every node deleting this row removes. A single item is `[id]`; a collapsed
    /// `__pycache__` row lists each of its folders, so its `files` and `size` are their sum.
    pub ids: Vec<NodeId>,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub files: u64,
    pub is_folder: bool,
    pub modified_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkGroup {
    pub kind: JunkKind,
    pub title: &'static str,
    pub description: &'static str,
    /// Space on disk of every match, including those past [`MAX_ITEMS`].
    pub size: u64,
    pub count: u64,
    /// Largest first, at most [`MAX_ITEMS`].
    pub items: Vec<JunkItem>,
}

/// Groups with at least one match. `now_ms` is the current time in ms since 1970.
pub fn find(tree: &Tree, protected: &Protected, now_ms: u64) -> Vec<JunkGroup> {
    let root = tree.path(0);
    let mut groups: Vec<JunkGroup> = JunkKind::ALL
        .iter()
        .map(|&kind| JunkGroup {
            kind,
            title: kind.title(),
            description: kind.description(),
            size: 0,
            count: 0,
            items: Vec::new(),
        })
        .collect();

    // Folders on the way down, with whether any of them is called Downloads.
    let mut stack: Vec<(NodeId, bool)> = vec![(0, false)];
    // `__pycache__` folders by the project or environment they belong to: one row each.
    let mut pycache: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
    let mut owners = Owners::default();
    while let Some((id, in_downloads)) = stack.pop() {
        for child in tree.live_children(id) {
            let node = tree.node(child);
            match node.kind {
                Kind::Link => {}
                Kind::File => {
                    let age = age_days(node.modified_ms, now_ms);
                    if let Some(kind) = classify_file(&node.name, age, in_downloads) {
                        add(&mut groups, tree, protected, &root, child, kind);
                    }
                }
                Kind::Dir => match classify_dir(tree, id, child, now_ms) {
                    DirClass::Junk(kind) => add(&mut groups, tree, protected, &root, child, kind),
                    DirClass::PyCache => {
                        let owner = owners.of(tree, child);
                        pycache.entry(owner).or_default().push(child);
                    }
                    // Dependencies are judged as a whole; nothing inside is looked at.
                    DirClass::Dependencies => {}
                    DirClass::Walk => {
                        let downloads = in_downloads || node.name.eq_ignore_ascii_case("downloads");
                        stack.push((child, downloads));
                    }
                },
            }
        }
    }
    for (owner, dirs) in pycache {
        add_pycache(&mut groups, tree, protected, &root, owner, dirs);
    }

    groups.retain(|group| group.count > 0);
    for group in &mut groups {
        if group.kind == JunkKind::EmptyFolders {
            group.items.sort_by(|a, b| a.path.cmp(&b.path));
        } else {
            group
                .items
                .sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
        }
        group.items.truncate(MAX_ITEMS);
    }
    groups.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.title.cmp(b.title)));
    groups
}

fn add(
    groups: &mut [JunkGroup],
    tree: &Tree,
    protected: &Protected,
    root: &std::path::Path,
    id: NodeId,
    kind: JunkKind,
) {
    let path = tree.path(id);
    if protected.check(&path, root).is_err() {
        return;
    }
    let node = tree.node(id);
    let Some(group) = groups.iter_mut().find(|group| group.kind == kind) else {
        return;
    };
    group.size += node.disk;
    group.count += 1;
    group.items.push(JunkItem {
        id,
        ids: vec![id],
        name: node.name.clone(),
        path: path.display().to_string(),
        size: node.disk,
        files: node.files,
        is_folder: node.kind == Kind::Dir,
        modified_ms: if node.kind == Kind::Dir {
            newest_modified(tree, id)
        } else {
            node.modified_ms
        },
    });
}

/// One row for all the `__pycache__` folders of a project or environment. A venv holds hundreds,
/// and one row each would bury everything else. The row's `path` is the project when it covers
/// several folders; the group's `count` still counts every folder.
fn add_pycache(
    groups: &mut [JunkGroup],
    tree: &Tree,
    protected: &Protected,
    root: &Path,
    owner: NodeId,
    mut dirs: Vec<NodeId>,
) {
    dirs.retain(|&dir| protected.check(&tree.path(dir), root).is_ok());
    dirs.sort_unstable();
    let Some(&first) = dirs.first() else { return };
    let Some(group) = groups
        .iter_mut()
        .find(|group| group.kind == JunkKind::DevCaches)
    else {
        return;
    };
    let size: u64 = dirs.iter().map(|&dir| tree.node(dir).disk).sum();
    group.size += size;
    group.count += dirs.len() as u64;
    let shown = if dirs.len() == 1 { first } else { owner };
    group.items.push(JunkItem {
        id: first,
        name: tree.node(first).name.clone(),
        path: tree.path(shown).display().to_string(),
        size,
        files: dirs.iter().map(|&dir| tree.node(dir).files).sum(),
        is_folder: true,
        modified_ms: dirs
            .iter()
            .filter_map(|&dir| newest_modified(tree, dir))
            .max(),
        ids: dirs,
    });
}

fn age_days(modified_ms: Option<u64>, now_ms: u64) -> Option<u64> {
    modified_ms.map(|modified| now_ms.saturating_sub(modified) / DAY_MS)
}

fn classify_file(name: &str, age: Option<u64>, in_downloads: bool) -> Option<JunkKind> {
    let lower = name.to_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, ext)| ext);
    // Without a known date a file is never "old".
    let older_than = |days: u64| age.is_some_and(|age| age >= days);
    if ext == "tmp" || name.starts_with("~$") {
        Some(JunkKind::TempFiles)
    } else if ext == "dmp" || ext == "mdmp" {
        Some(JunkKind::CrashDumps)
    } else if matches!(ext, "log" | "bak" | "old") && older_than(OLD_LOG_DAYS) {
        Some(JunkKind::OldLogs)
    } else if lower == "thumbs.db" || lower == ".ds_store" {
        Some(JunkKind::Thumbnails)
    } else if in_downloads && matches!(ext, "exe" | "msi") && older_than(STALE_DAYS) {
        Some(JunkKind::Installers)
    } else {
        None
    }
}

enum DirClass {
    Junk(JunkKind),
    PyCache,
    /// A `node_modules` that is not listed. Its contents are not junk either.
    Dependencies,
    Walk,
}

fn classify_dir(tree: &Tree, parent: NodeId, id: NodeId, now_ms: u64) -> DirClass {
    let node = tree.node(id);
    let lower = node.name.to_lowercase();
    match lower.as_str() {
        "__pycache__" if !runtime_owned(&tree.path(id)) => DirClass::PyCache,
        "node_modules" => {
            if !runtime_owned(&tree.path(id)) && project_is_stale(tree, parent, id, now_ms) {
                DirClass::Junk(JunkKind::DevCaches)
            } else {
                DirClass::Dependencies
            }
        }
        // `.cache` is not matched by name: one folder holds pip's cache next to multi-GB models
        // and tool state.
        _ => {
            // A folder that could not be read completely may not really be empty.
            let empty =
                tree.live_children(id).next().is_none() && node.errors == 0 && node.error.is_none();
            if empty {
                DirClass::Junk(JunkKind::EmptyFolders)
            } else {
                DirClass::Walk
            }
        }
    }
}

/// Lock files that show a `package.json` is a real project.
const LOCKFILES: [&str; 6] = [
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "bun.lock",
];

/// `node_modules` is listed only beside a `package.json` and a lock file (so it can be rebuilt
/// with `npm ci` or `pnpm install`), when the project and the folder itself have both sat for
/// [`STALE_DAYS`]. The project counts as touched when its manifest, lock file, `src/` or git
/// state changed. An unknown date counts as recent: better to keep than to suggest.
fn project_is_stale(tree: &Tree, project: NodeId, modules: NodeId, now_ms: u64) -> bool {
    let stale = |modified: Option<u64>| {
        modified.is_some_and(|modified| now_ms.saturating_sub(modified) / DAY_MS >= STALE_DAYS)
    };
    let Some(manifest) = child_named(tree, project, "package.json") else {
        return false;
    };
    let Some(lock) = LOCKFILES
        .iter()
        .find_map(|name| child_named(tree, project, name))
    else {
        return false;
    };
    let mut signals = vec![tree.node(manifest).modified_ms, tree.node(lock).modified_ms];
    if let Some(src) = child_named(tree, project, "src") {
        signals.push(newest_modified(tree, src));
    }
    if let Some(git) = child_named(tree, project, ".git") {
        let index = child_named(tree, git, "index");
        let head = child_named(tree, git, "logs").and_then(|logs| child_named(tree, logs, "HEAD"));
        signals.extend(
            [index, head]
                .into_iter()
                .flatten()
                .map(|id| tree.node(id).modified_ms),
        );
    }
    signals.into_iter().all(stale) && stale(newest_modified(tree, modules))
}

/// The live child of `id` called `name`, ignoring case.
fn child_named(tree: &Tree, id: NodeId, name: &str) -> Option<NodeId> {
    tree.live_children(id)
        .find(|&child| tree.node(child).name.eq_ignore_ascii_case(name))
}

/// Folders an app or a language runtime owns. What is inside belongs to it, not to a project,
/// so it is never listed. Segments are matched with both slashes, so this holds on any host.
fn runtime_owned(path: &Path) -> bool {
    let text = path.to_string_lossy().to_lowercase();
    let parts: Vec<&str> = text.split(['/', '\\']).collect();
    parts.contains(&"lightning-services")
        || parts.windows(2).any(|pair| {
            matches!(
                (pair[0], pair[1]),
                ("roaming", "local")
                    | ("local", "packages")
                    | ("local", "node")
                    | ("local", "python")
                    | ("programs", "python")
            )
        })
}

/// Files that mark a folder as a Python project or environment.
const PYTHON_ROOTS: [&str; 7] = [
    "pyvenv.cfg",
    "conda-meta",
    "pyproject.toml",
    "setup.py",
    "setup.cfg",
    "requirements.txt",
    "Pipfile",
];

/// Where a `__pycache__` belongs: the nearest folder marked as a venv or project. Without a
/// mark, the nearest `lib` / `site-packages`; without that, the folder holding it.
#[derive(Default)]
struct Owners {
    rooted: HashMap<NodeId, bool>,
}

impl Owners {
    fn of(&mut self, tree: &Tree, pycache: NodeId) -> NodeId {
        let holder = tree.parent(pycache).unwrap_or(0);
        let mut fallback = None;
        let mut at = Some(holder);
        while let Some(dir) = at {
            if self.is_root(tree, dir) {
                return dir;
            }
            let name = tree.node(dir).name.to_lowercase();
            if fallback.is_none() && matches!(name.as_str(), "lib" | "site-packages") {
                fallback = Some(dir);
            }
            at = tree.parent(dir);
        }
        fallback.unwrap_or(holder)
    }

    fn is_root(&mut self, tree: &Tree, dir: NodeId) -> bool {
        *self.rooted.entry(dir).or_insert_with(|| {
            PYTHON_ROOTS
                .iter()
                .any(|name| child_named(tree, dir, name).is_some())
        })
    }
}

/// The latest modified time of any file at or below `id`.
fn newest_modified(tree: &Tree, id: NodeId) -> Option<u64> {
    let mut newest = None;
    let mut stack = vec![id];
    while let Some(at) = stack.pop() {
        let node = tree.node(at);
        newest = newest.max(node.modified_ms);
        stack.extend(tree.live_children(at));
    }
    newest
}

#[cfg(test)]
mod tests {
    use super::*;
    use scan_core::{scan, ScanState};
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    fn write(path: &Path, age_days: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; 5000]).unwrap();
        let when = SystemTime::now() - Duration::from_secs(age_days * 24 * 60 * 60);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    fn names(groups: &[JunkGroup], kind: JunkKind) -> Vec<String> {
        let mut names: Vec<String> = groups
            .iter()
            .filter(|group| group.kind == kind)
            .flat_map(|group| group.items.iter().map(|item| item.name.clone()))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn finds_each_kind_and_leaves_recent_files_alone() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("a.tmp"), 0);
        write(&root.join("~$report.docx"), 0);
        write(&root.join("crash.dmp"), 1);
        write(&root.join("old.log"), 60);
        write(&root.join("new.log"), 2);
        write(&root.join("keep.bak"), 5);
        write(&root.join("old.bak"), 45);
        write(&root.join("sub/Thumbs.db"), 0);
        write(&root.join("Downloads/setup.exe"), 200);
        write(&root.join("Downloads/fresh.exe"), 3);
        write(&root.join("Other/setup.exe"), 200);
        // node_modules is listed only beside a manifest and a lock file, both as old as it is.
        write(&root.join("app/package.json"), 400);
        write(&root.join("app/package-lock.json"), 400);
        write(&root.join("app/node_modules/pkg/index.js"), 400);
        write(&root.join("web/package.json"), 400);
        write(&root.join("web/package-lock.json"), 400);
        write(&root.join("web/node_modules/pkg/index.js"), 2);
        // A lone node_modules has nothing to rebuild it from.
        write(&root.join("loose/node_modules/pkg/index.js"), 400);
        write(&root.join("py/__pycache__/m.pyc"), 0);
        // A whole .cache folder is never listed.
        write(&root.join(".cache/pip/x"), 400);
        fs::create_dir_all(root.join("empty/inner")).unwrap();

        let tree = scan(root, &ScanState::default()).unwrap();
        let groups = find(&tree, &Protected::default(), now());

        assert_eq!(
            names(&groups, JunkKind::TempFiles),
            ["a.tmp", "~$report.docx"]
        );
        assert_eq!(names(&groups, JunkKind::CrashDumps), ["crash.dmp"]);
        assert_eq!(names(&groups, JunkKind::OldLogs), ["old.bak", "old.log"]);
        assert_eq!(names(&groups, JunkKind::Thumbnails), ["Thumbs.db"]);
        assert_eq!(names(&groups, JunkKind::Installers), ["setup.exe"]);
        assert_eq!(
            names(&groups, JunkKind::DevCaches),
            ["__pycache__", "node_modules"]
        );
        let dev = groups
            .iter()
            .find(|group| group.kind == JunkKind::DevCaches)
            .unwrap();
        assert_eq!(dev.count, 2);
        for item in &dev.items {
            assert_eq!(item.ids, [item.id], "{}: one folder, one id", item.path);
        }
        // Only the innermost empty folder; "empty" has a child, so it isn't empty yet.
        assert_eq!(names(&groups, JunkKind::EmptyFolders), ["inner"]);
        let installer = &groups
            .iter()
            .find(|group| group.kind == JunkKind::Installers)
            .unwrap()
            .items[0];
        assert!(installer.path.contains("Downloads"));
    }

    #[test]
    fn protected_paths_are_never_suggested() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("keep/x.tmp"), 0);
        write(&root.join("other/y.tmp"), 0);
        let tree = scan(root, &ScanState::default()).unwrap();
        let keep = root.join("keep").display().to_string();
        let protected = Protected::new(&[keep.as_str()], &[], &[]);

        let groups = find(&tree, &protected, now());
        assert_eq!(names(&groups, JunkKind::TempFiles), ["y.tmp"]);
    }

    #[test]
    fn totals_count_every_match_and_removed_items_vanish() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for n in 0..3 {
            write(&root.join(format!("{n}.tmp")), 0);
        }
        let mut tree = scan(root, &ScanState::default()).unwrap();
        let first = find(&tree, &Protected::default(), now());
        assert_eq!(first[0].count, 3);
        assert!(first[0].size >= 15_000);

        let id = first[0].items[0].id;
        assert!(tree.remove(id));
        let second = find(&tree, &Protected::default(), now());
        assert_eq!(second[0].count, 2);
    }

    #[test]
    fn unknown_dates_are_never_old() {
        assert_eq!(classify_file("x.log", None, false), None);
        assert_eq!(
            classify_file("x.log", Some(31), false),
            Some(JunkKind::OldLogs)
        );
        assert_eq!(classify_file("setup.msi", Some(100), false), None);
    }

    fn dev_cache_paths(groups: &[JunkGroup]) -> Vec<String> {
        groups
            .iter()
            .filter(|group| group.kind == JunkKind::DevCaches)
            .flat_map(|group| group.items.iter().map(|item| item.path.clone()))
            .collect()
    }

    #[test]
    fn caches_inside_runtimes_and_package_stores_are_not_flagged() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join(".cache/pip/http/x"), 400);
        write(&root.join(".cargo/registry/index/x/.cache/y"), 400);
        write(
            &root.join(
                "AppData/Roaming/Local/lightning-services/php-8.2.27+1/node_modules/pkg/index.js",
            ),
            400,
        );
        write(
            &root.join("AppData/Local/Packages/Claude_x/LocalCache/proj/node_modules/pkg/index.js"),
            400,
        );
        write(
            &root.join("AppData/Local/node/corepack/v1/proj/node_modules/pkg/index.js"),
            400,
        );
        write(
            &root.join("AppData/Local/Python/venv/lib/__pycache__/m.pyc"),
            400,
        );
        let tree = scan(root, &ScanState::default()).unwrap();
        let groups = find(&tree, &Protected::default(), now());
        let paths = dev_cache_paths(&groups);

        assert!(
            !paths.iter().any(|path| path.ends_with(".cache")),
            "whole .cache folders are never flagged: {paths:?}"
        );
        for owner in ["lightning-services", "Packages", "corepack", "Python"] {
            assert!(
                !paths.iter().any(|path| path.contains(owner)),
                "{owner} is a runtime owner, not a project: {paths:?}"
            );
        }
    }

    #[test]
    fn node_modules_is_flagged_only_when_its_project_is_stale() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // A project untouched for 400 days.
        write(&root.join("stale-app/package.json"), 400);
        write(&root.join("stale-app/package-lock.json"), 400);
        write(&root.join("stale-app/node_modules/pkg/index.js"), 400);
        // node_modules is old, but the project's own files were touched 2 days ago.
        write(&root.join("live-app/package.json"), 2);
        write(&root.join("live-app/package-lock.json"), 400);
        write(&root.join("live-app/src/main.js"), 2);
        write(&root.join("live-app/node_modules/pkg/index.js"), 400);
        let tree = scan(root, &ScanState::default()).unwrap();
        let groups = find(&tree, &Protected::default(), now());
        let paths = dev_cache_paths(&groups);

        assert!(
            paths.iter().any(|path| path.contains("stale-app")),
            "{paths:?}"
        );
        assert!(
            !paths.iter().any(|path| path.contains("live-app")),
            "{paths:?}"
        );
    }

    #[test]
    fn pycache_folders_in_one_venv_are_one_row() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for n in 0..360 {
            write(&root.join(format!("venv/lib/pkg{n}/__pycache__/m.pyc")), 0);
        }
        let tree = scan(root, &ScanState::default()).unwrap();
        let groups = find(&tree, &Protected::default(), now());
        let group = groups
            .iter()
            .find(|group| group.kind == JunkKind::DevCaches)
            .unwrap();

        assert_eq!(group.count, 360);
        assert_eq!(group.items.len(), 1, "one row for the venv");
        let summed: u64 = group.items.iter().map(|item| item.size).sum();
        assert_eq!(summed, group.size);
    }

    #[test]
    fn a_venv_inside_a_project_gets_its_own_row_and_the_row_lists_every_folder() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("proj/pyproject.toml"), 0);
        write(&root.join("proj/pkg/__pycache__/a.pyc"), 0);
        write(&root.join("proj/.venv/pyvenv.cfg"), 0);
        write(
            &root.join("proj/.venv/lib/site-packages/x/__pycache__/b.pyc"),
            0,
        );
        write(
            &root.join("proj/.venv/lib/site-packages/y/__pycache__/c.pyc"),
            0,
        );
        let tree = scan(root, &ScanState::default()).unwrap();
        let groups = find(&tree, &Protected::default(), now());
        let group = &groups[0];

        assert_eq!((group.count, group.items.len()), (3, 2));
        let mut rows: Vec<(usize, bool)> = group
            .items
            .iter()
            .map(|item| (item.ids.len(), item.path.ends_with(".venv")))
            .collect();
        rows.sort();
        assert_eq!(rows, [(1, false), (2, true)]);
        for item in &group.items {
            assert_eq!(item.ids[0], item.id);
            for &id in &item.ids {
                assert_eq!(tree.node(id).name, "__pycache__");
            }
        }
    }

    #[test]
    fn protected_pycache_folders_are_left_out_of_a_collapsed_row() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("venv/pyvenv.cfg"), 0);
        write(&root.join("venv/lib/keep/__pycache__/a.pyc"), 0);
        write(&root.join("venv/lib/go/__pycache__/b.pyc"), 0);
        let tree = scan(root, &ScanState::default()).unwrap();
        let keep = root.join("venv/lib/keep").display().to_string();
        let protected = Protected::new(&[keep.as_str()], &[], &[]);
        let groups = find(&tree, &protected, now());

        assert_eq!(groups[0].count, 1);
        assert_eq!(groups[0].items[0].ids.len(), 1);
        assert!(groups[0].items[0].path.ends_with("__pycache__"));
        assert!(groups[0].items[0].path.contains("go"));
    }
}
