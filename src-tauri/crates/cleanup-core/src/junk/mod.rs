//! The junk finder: walks an already scanned tree (no rescan) and lists things that are usually
//! safe to remove, grouped by kind and by [`Tier`]. It only suggests; nothing is selected or
//! deleted here, and anything [`Protected`] refuses is left out. Rows in the `Never` tier are
//! reported by size and carry no ids, so nothing the UI sends back can delete them.

use crate::catalog::{self, Base, Confidence, Entry, Tier};
use crate::Protected;
use scan_core::{Kind, NodeId, Tree};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Logs and backup files older than this are listed.
pub const OLD_LOG_DAYS: u64 = 30;
/// Setup files in Downloads and dependency folders untouched for this long are listed.
pub const STALE_DAYS: u64 = 90;
/// Items shown per group; the group total still counts every match.
pub const MAX_ITEMS: usize = 500;
/// Folders in AppData and ProgramData that nothing recognises are reported from this size up.
pub const UNIDENTIFIED_BYTES: u64 = 500 * 1024 * 1024;

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
    AppCaches,
    BrowserData,
    Unidentified,
}

impl JunkKind {
    pub fn title(self) -> &'static str {
        match self {
            JunkKind::TempFiles => "Temporary files",
            JunkKind::CrashDumps => "Crash dumps",
            JunkKind::OldLogs => "Old logs and backups",
            JunkKind::Thumbnails => "Thumbnail caches",
            JunkKind::Installers => "Old setup files",
            JunkKind::DevCaches => "Developer caches",
            JunkKind::EmptyFolders => "Empty folders",
            JunkKind::AppCaches => "Application caches",
            JunkKind::BrowserData => "Browser data",
            JunkKind::Unidentified => "Unidentified folders",
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
                "node_modules of projects untouched for 90 days, __pycache__ folders (one row per project or environment), and the download caches under .cache in a user profile."
            }
            JunkKind::EmptyFolders => "Folders with nothing in them.",
            JunkKind::AppCaches => {
                "Download caches of tools and apps, such as Cargo's registry and Spotify's."
            }
            JunkKind::BrowserData => {
                "Chrome's cache folders, and the size of the whole profile folder they sit in."
            }
            JunkKind::Unidentified => {
                "Large folders in AppData and ProgramData that are not recognised. Reported by size only; nothing here can be selected."
            }
        }
    }

    /// One line on why the group is suggested, for the tier it sits in.
    pub fn why(self, tier: Tier) -> &'static str {
        match (self, tier) {
            (JunkKind::TempFiles, _) => "Left behind by apps; nothing reads them again.",
            (JunkKind::CrashDumps, _) => "Memory dumps of past crashes, only useful to debug them.",
            (JunkKind::OldLogs, _) => {
                "Logs are rebuilt, but a .bak or .old file can be the only copy of something."
            }
            (JunkKind::Thumbnails, _) => "Rebuilt when a folder is opened.",
            (JunkKind::Installers, _) => "Already installed; the vendor has another copy.",
            (JunkKind::DevCaches, Tier::Safe) => "Tools rebuild or download these on demand.",
            (JunkKind::DevCaches, _) => {
                "Dependencies and large downloads. Getting them back takes time and a network."
            }
            (JunkKind::EmptyFolders, _) => "Nothing inside, though an app may expect the folder.",
            (JunkKind::AppCaches, _) => "Caches that download again, possibly slowly.",
            (JunkKind::BrowserData, Tier::Never) => {
                "Profile data: sign-ins, history, extensions. Reported only, never offered."
            }
            (JunkKind::BrowserData, _) => "Chrome rebuilds its caches; sign-ins are kept.",
            (JunkKind::Unidentified, _) => "Not recognised. Reported by size only.",
        }
    }
}

/// One suggested item.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkItem {
    /// Keys the row: the first node it stands for. For a collapsed row, the first of `ids`.
    pub id: NodeId,
    /// Every node deleting this row removes. A single item is `[id]`; a collapsed
    /// `__pycache__` row lists each of its folders, so its `files` and `size` are their sum.
    /// Empty for a `Never` row: it is reported, not offered.
    pub ids: Vec<NodeId>,
    pub name: String,
    pub path: String,
    /// Space on disk: online-only (cloud) files count as 0 (see `cloud_bytes`).
    pub size: u64,
    pub files: u64,
    pub is_folder: bool,
    pub modified_ms: Option<u64>,
    /// Online-only bytes below this row, left out of `size`; when above 0 the UI says so.
    pub cloud_bytes: u64,
    pub tier: Tier,
    pub confidence: Confidence,
    /// Why the row is suggested (or, for a catalog folder, what it was recognised as).
    pub why: String,
    /// What to do to get it back, when there is something to say.
    pub restore: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JunkGroup {
    pub kind: JunkKind,
    /// A group holds one tier only: a kind with items in two tiers is two groups.
    pub tier: Tier,
    /// The least forgiving confidence of any match in the group.
    pub confidence: Confidence,
    pub title: &'static str,
    pub description: &'static str,
    pub why: &'static str,
    /// Space on disk of every match, including those past [`MAX_ITEMS`].
    pub size: u64,
    pub count: u64,
    /// Online-only bytes below every match, left out of `size`.
    pub cloud_bytes: u64,
    /// Largest first, at most [`MAX_ITEMS`].
    pub items: Vec<JunkItem>,
}

/// What a row says about itself.
struct Verdict {
    tier: Tier,
    confidence: Confidence,
    why: String,
    restore: Option<String>,
}

impl Verdict {
    fn new(
        tier: Tier,
        confidence: Confidence,
        why: impl Into<String>,
        restore: Option<&str>,
    ) -> Self {
        Self {
            tier,
            confidence,
            why: why.into(),
            restore: restore.map(str::to_owned),
        }
    }
}

/// The verdict for the kinds that are found by name or extension alone.
fn kind_verdict(kind: JunkKind) -> Verdict {
    let (tier, confidence) = match kind {
        JunkKind::TempFiles | JunkKind::CrashDumps | JunkKind::Thumbnails => {
            (Tier::Safe, Confidence::Regenerates)
        }
        JunkKind::OldLogs => (Tier::Review, Confidence::UserData),
        JunkKind::Installers => (Tier::Review, Confidence::Redownloads),
        JunkKind::EmptyFolders => (Tier::Review, Confidence::Regenerates),
        // These have verdicts of their own; if one lands here, a person looks first.
        JunkKind::DevCaches
        | JunkKind::AppCaches
        | JunkKind::BrowserData
        | JunkKind::Unidentified => (Tier::Review, Confidence::UserData),
    };
    Verdict::new(tier, confidence, kind.why(tier), None)
}

/// Groups with at least one match, Safe first. `now_ms` is the current time in ms since 1970.
pub fn find(tree: &Tree, protected: &Protected, now_ms: u64) -> Vec<JunkGroup> {
    find_with(tree, protected, now_ms, UNIDENTIFIED_BYTES)
}

fn find_with(
    tree: &Tree,
    protected: &Protected,
    now_ms: u64,
    min_unidentified: u64,
) -> Vec<JunkGroup> {
    let mut finder = Finder {
        tree,
        protected,
        root: tree.path(0),
        groups: Vec::new(),
    };

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
                        finder.add(child, kind);
                    }
                }
                Kind::Dir => {
                    // Reported whatever else is done with the folder; it is still walked.
                    finder.report_only(child, min_unidentified);
                    match classify_dir(tree, id, child, now_ms) {
                        DirClass::Junk(kind) => finder.add(child, kind),
                        DirClass::PyCache => {
                            let owner = owners.of(tree, child);
                            pycache.entry(owner).or_default().push(child);
                        }
                        DirClass::NodeModules(restore) => finder.node_modules(child, restore),
                        // Dependencies are judged as a whole; nothing inside is looked at.
                        DirClass::Dependencies => {}
                        DirClass::DotCache => finder.dot_cache(child),
                        DirClass::Chrome => finder.chrome(child),
                        DirClass::Review(entry) => finder.review(child, entry),
                        DirClass::Walk => {
                            let downloads =
                                in_downloads || node.name.eq_ignore_ascii_case("downloads");
                            stack.push((child, downloads));
                        }
                    }
                }
            }
        }
    }
    for (owner, dirs) in pycache {
        finder.pycache(owner, dirs);
    }

    let mut groups = finder.groups;
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
    groups.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then_with(|| b.size.cmp(&a.size))
            .then_with(|| a.title.cmp(b.title))
    });
    groups
}

mod classify;
mod finder;
#[cfg(test)]
mod tests;

use classify::*;
use finder::*;
