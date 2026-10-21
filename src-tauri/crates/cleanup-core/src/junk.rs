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

struct Finder<'a> {
    tree: &'a Tree,
    protected: &'a Protected,
    root: PathBuf,
    groups: Vec<JunkGroup>,
}

impl Finder<'_> {
    fn allowed(&self, id: NodeId) -> bool {
        self.protected
            .check(&self.tree.path(id), &self.root)
            .is_ok()
    }

    /// The group for `kind` in `tier`, made when the first row arrives.
    fn group(&mut self, kind: JunkKind, tier: Tier) -> &mut JunkGroup {
        let at = match self
            .groups
            .iter()
            .position(|group| group.kind == kind && group.tier == tier)
        {
            Some(at) => at,
            None => {
                self.groups.push(JunkGroup {
                    kind,
                    tier,
                    confidence: Confidence::Regenerates,
                    title: kind.title(),
                    description: kind.description(),
                    why: kind.why(tier),
                    size: 0,
                    count: 0,
                    cloud_bytes: 0,
                    items: Vec::new(),
                });
                self.groups.len() - 1
            }
        };
        &mut self.groups[at]
    }

    /// One row standing for `members`, shown at the path of `shown`. A `deletable` row lists the
    /// members as its ids; a reported row lists none. The group's totals count every member.
    fn push(
        &mut self,
        kind: JunkKind,
        verdict: Verdict,
        shown: NodeId,
        members: &[NodeId],
        deletable: bool,
    ) {
        let Some(&first) = members.first() else {
            return;
        };
        let tree = self.tree;
        let nodes = || members.iter().map(|&member| tree.node(member));
        let size: u64 = nodes().map(|node| node.disk).sum();
        let cloud_bytes: u64 = nodes().map(|node| node.cloud_bytes).sum();
        let Verdict {
            tier,
            confidence,
            why,
            restore,
        } = verdict;
        let item = JunkItem {
            id: first,
            ids: if deletable {
                members.to_vec()
            } else {
                Vec::new()
            },
            name: tree.node(first).name.clone(),
            path: tree.path(shown).display().to_string(),
            size,
            files: nodes().map(|node| node.files).sum(),
            is_folder: tree.node(first).kind == Kind::Dir,
            modified_ms: members
                .iter()
                .filter_map(|&member| modified_of(tree, member))
                .max(),
            cloud_bytes,
            tier,
            confidence,
            why,
            restore,
        };
        let group = self.group(kind, tier);
        group.confidence = group.confidence.max(confidence);
        group.size += size;
        group.cloud_bytes += cloud_bytes;
        group.count += members.len() as u64;
        group.items.push(item);
    }

    /// A single deletable row, unless the path is protected.
    fn add_one(&mut self, kind: JunkKind, verdict: Verdict, id: NodeId) {
        if self.allowed(id) {
            self.push(kind, verdict, id, &[id], true);
        }
    }

    fn add(&mut self, id: NodeId, kind: JunkKind) {
        self.add_one(kind, kind_verdict(kind), id);
    }

    fn node_modules(&mut self, id: NodeId, restore: &str) {
        let verdict = Verdict::new(
            Tier::Review,
            Confidence::Redownloads,
            "Dependencies of a project untouched for 90 days.",
            Some(restore),
        );
        self.add_one(JunkKind::DevCaches, verdict, id);
    }

    /// One row for all the `__pycache__` folders of a project or environment. A venv holds
    /// hundreds, and one row each would bury everything else. The row's `path` is the project when
    /// it covers several folders; the group's `count` still counts every folder.
    fn pycache(&mut self, owner: NodeId, mut dirs: Vec<NodeId>) {
        dirs.retain(|&dir| self.allowed(dir));
        dirs.sort_unstable();
        let Some(&first) = dirs.first() else { return };
        let shown = if dirs.len() == 1 { first } else { owner };
        let verdict = Verdict::new(
            Tier::Safe,
            Confidence::Regenerates,
            "Python writes these when it runs.",
            Some("Python writes them again the next time the code runs."),
        );
        self.push(JunkKind::DevCaches, verdict, shown, &dirs, true);
    }

    /// The download caches under `.cache` in a user profile, one row per tool: the pure caches are
    /// Safe, the slow-to-fetch ones (models) are Review, anything else is left unlisted.
    fn dot_cache(&mut self, folder: NodeId) {
        let tree = self.tree;
        for sub in tree.live_children(folder) {
            let node = tree.node(sub);
            if node.kind != Kind::Dir {
                continue;
            }
            let lower = node.name.to_lowercase();
            let verdict = if catalog::DOT_CACHE_PURE.contains(&lower.as_str()) {
                Verdict::new(
                    Tier::Safe,
                    Confidence::Redownloads,
                    format!("The {} download cache.", node.name),
                    Some("Downloaded again the next time the tool needs it."),
                )
            } else if catalog::DOT_CACHE_SLOW.contains(&lower.as_str()) {
                Verdict::new(
                    Tier::Review,
                    Confidence::Redownloads,
                    format!("{} models and data; large and slow to fetch.", node.name),
                    Some("Downloaded again on first use. That can be many GB."),
                )
            } else {
                continue;
            };
            self.add_one(JunkKind::DevCaches, verdict, sub);
        }
    }

    /// A catalog folder that needs a look first: one row, not walked.
    fn review(&mut self, id: NodeId, entry: &Entry) {
        let verdict = Verdict::new(
            Tier::Review,
            entry.confidence,
            format!("Recognised as {}.", entry.label),
            Some(entry.restore),
        );
        self.add_one(JunkKind::AppCaches, verdict, id);
    }

    /// Chrome: the cache folders of every profile as one Safe row, and the whole `User Data`
    /// folder as a reported total beside it.
    fn chrome(&mut self, folder: NodeId) {
        let tree = self.tree;
        let cache_names: Vec<&str> = catalog::SAFE
            .iter()
            .filter(|entry| entry.path.contains(&"*"))
            .filter_map(|entry| entry.path.last().copied())
            .collect();
        let caches: Vec<NodeId> = tree
            .live_children(folder)
            .filter(|&profile| tree.node(profile).kind == Kind::Dir)
            .flat_map(|profile| tree.live_children(profile))
            .filter(|&cache| {
                let node = tree.node(cache);
                node.kind == Kind::Dir
                    && cache_names
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(&node.name))
            })
            .filter(|&cache| self.allowed(cache))
            .collect();
        let restore = catalog::SAFE
            .iter()
            .find(|entry| entry.path.contains(&"*"))
            .map(|entry| entry.restore);
        let verdict = Verdict::new(
            Tier::Safe,
            Confidence::Regenerates,
            "Cache folders of every Chrome profile.",
            restore,
        );
        self.push(JunkKind::BrowserData, verdict, folder, &caches, true);

        let entry = &catalog::CHROME_USER_DATA;
        let verdict = Verdict::new(
            Tier::Never,
            entry.confidence,
            "Everything Chrome keeps, the cache included. Reported only.",
            Some(entry.restore),
        );
        self.push(JunkKind::BrowserData, verdict, folder, &[folder], false);
    }

    /// Reports big folders nothing identifies, and the catalog's report-only folders, by size.
    /// They get no ids. Only folders of `min` bytes or more are listed.
    fn report_only(&mut self, id: NodeId, min: u64) {
        let tree = self.tree;
        let node = tree.node(id);
        if node.disk < min {
            return;
        }
        let lower = node.name.to_lowercase();
        if let Some(entry) = catalog::REPORT_ONLY
            .iter()
            .find(|entry| matches_entry(tree, id, &lower, entry))
        {
            let verdict = Verdict::new(
                Tier::Never,
                entry.confidence,
                format!("Recognised as {}. Reported only.", entry.label),
                Some(entry.restore),
            );
            self.push(JunkKind::Unidentified, verdict, id, &[id], false);
            return;
        }
        if KNOWN_APP_FOLDERS.contains(&lower.as_str()) {
            return;
        }
        let Some(parent) = tree.parent(id) else {
            return;
        };
        let parts = segments(&tree.path(parent));
        let in_app_data = ends_with(&parts, &["programdata"])
            || ends_with(&parts, &["appdata", "local"])
            || ends_with(&parts, &["appdata", "roaming"]);
        if in_app_data {
            let verdict = Verdict::new(
                Tier::Never,
                Confidence::UserData,
                "Not recognised. Check what is inside before removing anything.",
                None,
            );
            self.push(JunkKind::Unidentified, verdict, id, &[id], false);
        }
    }
}

/// Folders under AppData and ProgramData that are identified, so never reported as unidentified.
const KNOWN_APP_FOLDERS: [&str; 24] = [
    "microsoft",
    "packages",
    "programs",
    "temp",
    "google",
    "mozilla",
    "nvidia",
    "nvidia corporation",
    "package cache",
    "docker",
    "apple",
    "apple computer",
    "adobe",
    "intel",
    "amd",
    "node",
    "python",
    "pip",
    "npm",
    "spotify",
    "ms-playwright",
    "d3dscache",
    "crashdumps",
    "connecteddevicesplatform",
];

fn modified_of(tree: &Tree, id: NodeId) -> Option<u64> {
    if tree.node(id).kind == Kind::Dir {
        newest_modified(tree, id)
    } else {
        tree.node(id).modified_ms
    }
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
    /// A stale project's `node_modules`, with the line that says how to install it again.
    NodeModules(&'static str),
    /// A `node_modules` that is not listed. Its contents are not junk either.
    Dependencies,
    /// The `.cache` folder of a user profile.
    DotCache,
    /// Chrome's `User Data`.
    Chrome,
    /// A catalog folder that needs a look first.
    Review(&'static Entry),
    Walk,
}

fn classify_dir(tree: &Tree, parent: NodeId, id: NodeId, now_ms: u64) -> DirClass {
    let node = tree.node(id);
    let lower = node.name.to_lowercase();
    match lower.as_str() {
        "__pycache__" if !runtime_owned(&tree.path(id)) => DirClass::PyCache,
        "node_modules" => {
            if runtime_owned(&tree.path(id)) {
                return DirClass::Dependencies;
            }
            match stale_project(tree, parent, id, now_ms) {
                Some(lock) => DirClass::NodeModules(reinstall(lock)),
                None => DirClass::Dependencies,
            }
        }
        // `.cache` is judged only as a profile's own folder, one tool at a time: it holds pip's
        // cache next to multi-GB models and tool state.
        ".cache" if in_user_profile(tree, parent) => DirClass::DotCache,
        "user data" if matches_entry(tree, id, &lower, &catalog::CHROME_USER_DATA) => {
            DirClass::Chrome
        }
        _ => {
            if let Some(entry) = catalog::REVIEW
                .iter()
                .find(|entry| matches_entry(tree, id, &lower, entry))
            {
                return DirClass::Review(entry);
            }
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

/// Path segments, lowercase, split on both slashes so a Windows path reads the same on any host.
fn segments(path: &Path) -> Vec<String> {
    path.to_string_lossy()
        .to_lowercase()
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether the last segments of `parts` are `pattern`; `*` stands for any one segment.
fn ends_with(parts: &[String], pattern: &[&str]) -> bool {
    parts.len() >= pattern.len()
        && parts[parts.len() - pattern.len()..]
            .iter()
            .zip(pattern)
            .all(|(part, want)| *want == "*" || part.eq_ignore_ascii_case(want))
}

/// Where each catalog base sits in a path: the segments before the entry's own.
fn base_prefixes(base: Base) -> &'static [&'static [&'static str]] {
    match base {
        Base::LocalAppData => &[&["appdata", "local"]],
        Base::AppData => &[&["appdata", "roaming"]],
        Base::UserProfile => &[&["users", "*"], &["home", "*"]],
        Base::SystemDrive => &[&[]],
    }
}

/// Whether folder `id` (named `lower_name`, lowercase) is the folder `entry` describes. The tree
/// has no notion of a drive's special folders, so the match is on the path's last segments.
fn matches_entry(tree: &Tree, id: NodeId, lower_name: &str, entry: &Entry) -> bool {
    let Some(last) = entry.path.last() else {
        return false;
    };
    if !last.eq_ignore_ascii_case(lower_name) {
        return false;
    }
    let parts = segments(&tree.path(id));
    base_prefixes(entry.base).iter().any(|prefix| {
        let pattern: Vec<&str> = prefix.iter().chain(entry.path).copied().collect();
        ends_with(&parts, &pattern)
    })
}

/// Whether folder `id` is a user's home folder: `Users\<name>` or `home/<name>`.
fn in_user_profile(tree: &Tree, id: NodeId) -> bool {
    let parts = segments(&tree.path(id));
    parts.len() >= 2 && matches!(parts[parts.len() - 2].as_str(), "users" | "home")
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

/// The lock file of the project when its `node_modules` is listed: beside a `package.json` and a
/// lock file (so it can be rebuilt), with the project and the folder itself both untouched for
/// [`STALE_DAYS`]. The project counts as touched when its manifest, lock file, `src/` or git
/// state changed. An unknown date counts as recent: better to keep than to suggest.
fn stale_project(
    tree: &Tree,
    project: NodeId,
    modules: NodeId,
    now_ms: u64,
) -> Option<&'static str> {
    let stale = |modified: Option<u64>| {
        modified.is_some_and(|modified| now_ms.saturating_sub(modified) / DAY_MS >= STALE_DAYS)
    };
    let manifest = child_named(tree, project, "package.json")?;
    let (lock_name, lock) = LOCKFILES
        .iter()
        .find_map(|name| child_named(tree, project, name).map(|id| (*name, id)))?;
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
    (signals.into_iter().all(stale) && stale(newest_modified(tree, modules))).then_some(lock_name)
}

/// How to put `node_modules` back, from the lock file that rebuilds it.
fn reinstall(lock: &str) -> &'static str {
    match lock {
        "pnpm-lock.yaml" => "Run `pnpm install` in the project folder.",
        "yarn.lock" => "Run `yarn install --frozen-lockfile` in the project folder.",
        "bun.lockb" | "bun.lock" => "Run `bun install` in the project folder.",
        _ => "Run `npm ci` in the project folder.",
    }
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

    /// What every row must hold, in any tier: a group holds its own tier only, rows that can be
    /// deleted list their ids (the first is `id`), and a `Never` row lists none.
    fn assert_shape(groups: &[JunkGroup]) {
        for group in groups {
            assert!(!group.items.is_empty(), "{}", group.title);
            for item in &group.items {
                assert_eq!(item.tier, group.tier, "{}", item.path);
                assert!(item.confidence <= group.confidence, "{}", item.path);
                assert!(!item.why.is_empty(), "{}", item.path);
                if group.tier == Tier::Never {
                    assert!(item.ids.is_empty(), "{}: reported, not offered", item.path);
                } else {
                    assert_eq!(item.ids.first(), Some(&item.id), "{}", item.path);
                }
            }
        }
    }

    fn rows(groups: &[JunkGroup], kind: JunkKind, tier: Tier) -> Vec<&JunkItem> {
        groups
            .iter()
            .filter(|group| group.kind == kind && group.tier == tier)
            .flat_map(|group| &group.items)
            .collect()
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
        // A kind with rows in two tiers is two groups: pycache is Safe, node_modules is Review.
        let dev: Vec<&JunkGroup> = groups
            .iter()
            .filter(|group| group.kind == JunkKind::DevCaches)
            .collect();
        assert_eq!(dev.iter().map(|group| group.count).sum::<u64>(), 2);
        assert_eq!(
            dev.iter().map(|group| group.tier).collect::<Vec<_>>(),
            [Tier::Safe, Tier::Review]
        );
        for item in dev.iter().flat_map(|group| &group.items) {
            assert_eq!(item.ids, [item.id], "{}: one folder, one id", item.path);
        }
        assert_shape(&groups);
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

    #[test]
    fn each_kind_lands_in_its_tier() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("a.tmp"), 0);
        write(&root.join("crash.dmp"), 1);
        write(&root.join("old.bak"), 45);
        write(&root.join("sub/Thumbs.db"), 0);
        write(&root.join("Downloads/setup.exe"), 200);
        write(&root.join("py/__pycache__/m.pyc"), 0);
        fs::create_dir_all(root.join("empty")).unwrap();
        let tree = scan(root, &ScanState::default()).unwrap();
        let groups = find(&tree, &Protected::default(), now());

        let tier_of = |kind| {
            groups
                .iter()
                .find(|group| group.kind == kind)
                .map(|group| (group.tier, group.confidence))
        };
        let regenerates = Confidence::Regenerates;
        assert_eq!(
            tier_of(JunkKind::TempFiles),
            Some((Tier::Safe, regenerates))
        );
        assert_eq!(
            tier_of(JunkKind::CrashDumps),
            Some((Tier::Safe, regenerates))
        );
        assert_eq!(
            tier_of(JunkKind::Thumbnails),
            Some((Tier::Safe, regenerates))
        );
        assert_eq!(
            tier_of(JunkKind::DevCaches),
            Some((Tier::Safe, regenerates))
        );
        // A .bak can be the only copy; an installer is another download away.
        assert_eq!(
            tier_of(JunkKind::OldLogs),
            Some((Tier::Review, Confidence::UserData))
        );
        assert_eq!(
            tier_of(JunkKind::Installers),
            Some((Tier::Review, Confidence::Redownloads))
        );
        assert_eq!(
            tier_of(JunkKind::EmptyFolders),
            Some((Tier::Review, regenerates))
        );
        // Safe first, then Review.
        let tiers: Vec<Tier> = groups.iter().map(|group| group.tier).collect();
        assert!(tiers.windows(2).all(|pair| pair[0] <= pair[1]), "{tiers:?}");
        assert_shape(&groups);
    }

    #[test]
    fn node_modules_says_how_to_install_it_again() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for (project, lock) in [
            ("npm-app", "package-lock.json"),
            ("pnpm-app", "pnpm-lock.yaml"),
            ("yarn-app", "yarn.lock"),
            ("bun-app", "bun.lockb"),
        ] {
            write(&root.join(format!("{project}/package.json")), 400);
            write(&root.join(format!("{project}/{lock}")), 400);
            write(&root.join(format!("{project}/node_modules/p/i.js")), 400);
        }
        let tree = scan(root, &ScanState::default()).unwrap();
        let groups = find(&tree, &Protected::default(), now());

        let listed = rows(&groups, JunkKind::DevCaches, Tier::Review);
        assert_eq!(listed.len(), 4);
        for (project, command) in [
            ("npm-app", "npm ci"),
            ("pnpm-app", "pnpm install"),
            ("yarn-app", "yarn install"),
            ("bun-app", "bun install"),
        ] {
            let item = listed
                .iter()
                .find(|item| item.path.contains(project))
                .unwrap();
            assert!(
                item.restore.as_deref().unwrap().contains(command),
                "{project}"
            );
            assert_eq!(item.confidence, Confidence::Redownloads);
        }
        assert!(rows(&groups, JunkKind::DevCaches, Tier::Safe).is_empty());
        assert_shape(&groups);
    }

    #[test]
    fn dot_cache_is_split_by_tool_and_only_in_a_profile() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for tool in [
            "pip",
            "npm",
            "pnpm",
            "puppeteer",
            "huggingface",
            "torch",
            "whisper",
        ] {
            write(&root.join(format!("Users/sam/.cache/{tool}/x")), 400);
        }
        // Not a tool the catalog knows, and a loose file: left unlisted.
        write(&root.join("Users/sam/.cache/other-tool/x"), 400);
        write(&root.join("Users/sam/.cache/loose.bin"), 400);
        // A .cache that is not a profile's own is never listed.
        write(&root.join("project/.cache/pip/x"), 400);
        let tree = scan(root, &ScanState::default()).unwrap();
        let groups = find(&tree, &Protected::default(), now());

        let last = |item: &&JunkItem| item.path.rsplit(['/', '\\']).next().unwrap().to_owned();
        let mut safe: Vec<String> = rows(&groups, JunkKind::DevCaches, Tier::Safe)
            .iter()
            .map(last)
            .collect();
        let mut review: Vec<String> = rows(&groups, JunkKind::DevCaches, Tier::Review)
            .iter()
            .map(last)
            .collect();
        safe.sort();
        review.sort();
        assert_eq!(safe, ["npm", "pip", "pnpm", "puppeteer"]);
        assert_eq!(review, ["huggingface", "torch", "whisper"]);
        for item in rows(&groups, JunkKind::DevCaches, Tier::Review) {
            assert_eq!(item.confidence, Confidence::Redownloads);
            assert!(item.restore.is_some(), "{}", item.path);
        }
        assert_shape(&groups);
    }

    /// Where a catalog entry sits under a fake profile, so its path is matched like a real one.
    fn place(root: &Path, entry: &Entry) -> PathBuf {
        let base = match entry.base {
            Base::LocalAppData => root.join("Users/sam/AppData/Local"),
            Base::AppData => root.join("Users/sam/AppData/Roaming"),
            Base::UserProfile => root.join("Users/sam"),
            Base::SystemDrive => root.to_path_buf(),
        };
        entry.path.iter().fold(base, |path, part| path.join(part))
    }

    #[test]
    fn every_review_entry_is_a_review_row_with_its_restore_text() {
        for entry in catalog::REVIEW {
            let temp = tempfile::tempdir().unwrap();
            let folder = place(temp.path(), entry);
            write(&folder.join("x.bin"), 400);
            let tree = scan(temp.path(), &ScanState::default()).unwrap();
            let groups = find_with(&tree, &Protected::default(), now(), 1);

            let listed = rows(&groups, JunkKind::AppCaches, Tier::Review);
            assert_eq!(listed.len(), 1, "{}", entry.label);
            let item = listed[0];
            assert_eq!(item.path, folder.display().to_string(), "{}", entry.label);
            assert_eq!(item.confidence, entry.confidence, "{}", entry.label);
            assert_eq!(item.restore.as_deref(), Some(entry.restore));
            assert_eq!(item.ids.len(), 1, "{}", entry.label);
            assert_shape(&groups);
        }
    }

    #[test]
    fn every_report_only_entry_is_reported_without_ids() {
        for entry in catalog::REPORT_ONLY {
            let temp = tempfile::tempdir().unwrap();
            let folder = place(temp.path(), entry);
            write(&folder.join("x.bin"), 400);
            let tree = scan(temp.path(), &ScanState::default()).unwrap();
            let groups = find_with(&tree, &Protected::default(), now(), 1);

            let listed = rows(&groups, JunkKind::Unidentified, Tier::Never);
            let item = listed
                .iter()
                .find(|item| item.path == folder.display().to_string())
                .unwrap_or_else(|| panic!("{} is not reported", entry.label));
            assert!(item.ids.is_empty(), "{}", entry.label);
            assert_eq!(item.confidence, entry.confidence, "{}", entry.label);
            assert!(item.size >= 4000, "{}", entry.label);
            assert_shape(&groups);
        }
    }

    #[test]
    fn unidentified_folders_are_reported_from_the_size_limit_up() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(&root.join("ProgramData/Addy7/big.bin"), 0);
        write(&root.join("ProgramData/Tiny/small.bin"), 0);
        write(&root.join("Users/sam/AppData/Local/Mystery/m.bin"), 0);
        // Identified folders are not "unidentified", and a folder elsewhere is not in scope.
        write(&root.join("Users/sam/AppData/Local/Microsoft/m.bin"), 0);
        write(&root.join("Documents/Elsewhere/e.bin"), 0);
        let tree = scan(root, &ScanState::default()).unwrap();
        let big = tree
            .node(tree.child_by_name(0, "ProgramData").unwrap())
            .disk;

        let all = find_with(&tree, &Protected::default(), now(), 1);
        let mut found: Vec<String> = rows(&all, JunkKind::Unidentified, Tier::Never)
            .iter()
            .map(|item| item.name.clone())
            .collect();
        found.sort();
        assert_eq!(found, ["Addy7", "Mystery", "Tiny"]);
        assert_shape(&all);

        // Nothing here is near 500 MB, and a limit above the folder's size lists nothing.
        let default = find(&tree, &Protected::default(), now());
        assert!(rows(&default, JunkKind::Unidentified, Tier::Never).is_empty());
        let above = find_with(&tree, &Protected::default(), now(), big + 1);
        assert!(rows(&above, JunkKind::Unidentified, Tier::Never).is_empty());
    }

    #[test]
    fn chrome_cache_is_reported_apart_from_the_whole_profile_folder() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let data = root.join("Users/sam/AppData/Local/Google/Chrome/User Data");
        write(&data.join("Default/Cache/c1"), 0);
        write(&data.join("Default/Code Cache/c2"), 0);
        write(&data.join("Profile 1/GPUCache/c3"), 0);
        write(&data.join("Default/History"), 0);
        write(&data.join("Default/Extensions/e/x.js"), 0);
        let tree = scan(root, &ScanState::default()).unwrap();
        let groups = find(&tree, &Protected::default(), now());

        let cache = rows(&groups, JunkKind::BrowserData, Tier::Safe);
        let total = rows(&groups, JunkKind::BrowserData, Tier::Never);
        assert_eq!((cache.len(), total.len()), (1, 1));
        assert_eq!(
            cache[0].ids.len(),
            3,
            "one cache folder per profile and kind"
        );
        assert!(total[0].ids.is_empty());
        assert_eq!(total[0].path, data.display().to_string());
        assert!(
            total[0].size > cache[0].size,
            "the total includes the cache"
        );
        assert_eq!(
            total[0].size,
            tree.node(tree.child_by_name(0, "Users").unwrap()).disk
        );
        assert!(cache[0].restore.as_deref().unwrap().contains("Chrome"));
        assert_shape(&groups);
    }

    #[test]
    fn a_protected_chrome_cache_is_left_out_but_the_total_is_still_reported() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let data = root.join("Users/sam/AppData/Local/Google/Chrome/User Data");
        write(&data.join("Default/Cache/c1"), 0);
        let tree = scan(root, &ScanState::default()).unwrap();
        let keep = data.join("Default/Cache").display().to_string();
        let protected = Protected::new(&[keep.as_str()], &[], &[]);
        let groups = find(&tree, &protected, now());

        assert!(rows(&groups, JunkKind::BrowserData, Tier::Safe).is_empty());
        assert_eq!(rows(&groups, JunkKind::BrowserData, Tier::Never).len(), 1);
        assert_shape(&groups);
    }

    #[test]
    fn totals_still_count_every_match_past_the_item_limit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let extra = 7;
        for n in 0..MAX_ITEMS + extra {
            write(&root.join(format!("{n}.tmp")), 0);
        }
        let tree = scan(root, &ScanState::default()).unwrap();
        let groups = find(&tree, &Protected::default(), now());
        let group = &groups[0];

        assert_eq!(group.tier, Tier::Safe);
        assert_eq!(group.count, (MAX_ITEMS + extra) as u64);
        assert_eq!(group.items.len(), MAX_ITEMS);
        let listed: u64 = group.items.iter().map(|item| item.size).sum();
        assert!(group.size > listed, "the total includes the unlisted rows");
        assert_eq!(group.size, tree.node(0).disk);
    }

    #[test]
    fn rows_carry_what_the_ui_needs_and_serialize_in_camel_case() {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("a.tmp"), 0);
        let tree = scan(temp.path(), &ScanState::default()).unwrap();
        let groups = find(&tree, &Protected::default(), now());
        let json = serde_json::to_value(&groups[0]).unwrap();

        assert_eq!(json["tier"], "safe");
        assert_eq!(json["confidence"], "regenerates");
        assert_eq!(json["cloudBytes"], 0);
        let item = &json["items"][0];
        assert_eq!(item["tier"], "safe");
        assert_eq!(item["confidence"], "regenerates");
        assert_eq!(item["cloudBytes"], 0);
        assert!(item["why"].as_str().is_some_and(|why| !why.is_empty()));
        assert!(item["restore"].is_null());
        let review = serde_json::to_value(Confidence::UserData).unwrap();
        assert_eq!(review, "user_data");
    }
}
