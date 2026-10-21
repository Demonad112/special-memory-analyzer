//! The junk finder: walks an already scanned tree (no rescan) and lists things that are usually
//! safe to remove, grouped by kind. It only suggests; nothing is selected or deleted here, and
//! anything [`Protected`] refuses is left out.

use crate::Protected;
use scan_core::{Kind, NodeId, Tree};
use serde::Serialize;

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
                "node_modules not touched for 90 days, plus __pycache__ and .cache folders."
            }
            JunkKind::EmptyFolders => "Folders with nothing in them.",
        }
    }
}

/// One suggested item.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkItem {
    pub id: NodeId,
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
                Kind::Dir => {
                    if let Some(kind) = classify_dir(tree, child, now_ms) {
                        add(&mut groups, tree, protected, &root, child, kind);
                    } else {
                        let downloads = in_downloads || node.name.eq_ignore_ascii_case("downloads");
                        stack.push((child, downloads));
                    }
                }
            }
        }
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

fn classify_dir(tree: &Tree, id: NodeId, now_ms: u64) -> Option<JunkKind> {
    let node = tree.node(id);
    let lower = node.name.to_lowercase();
    match lower.as_str() {
        "__pycache__" | ".cache" => Some(JunkKind::DevCaches),
        "node_modules" => {
            // An unknown date counts as recent: better to keep than to suggest.
            let stale = newest_modified(tree, id)
                .is_some_and(|newest| now_ms.saturating_sub(newest) / DAY_MS >= STALE_DAYS);
            stale.then_some(JunkKind::DevCaches)
        }
        _ => {
            // A folder that could not be read completely may not really be empty.
            let empty =
                tree.live_children(id).next().is_none() && node.errors == 0 && node.error.is_none();
            empty.then_some(JunkKind::EmptyFolders)
        }
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
        write(&root.join("app/node_modules/pkg/index.js"), 400);
        write(&root.join("web/node_modules/pkg/index.js"), 2);
        write(&root.join("py/__pycache__/m.pyc"), 0);
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
}
