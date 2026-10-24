//! Name, path and age rules that decide what a file or folder is.

use super::*;

/// Folders under AppData and ProgramData that are identified, so never reported as unidentified.
pub(super) const KNOWN_APP_FOLDERS: [&str; 24] = [
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

pub(super) fn modified_of(tree: &Tree, id: NodeId) -> Option<u64> {
    if tree.node(id).kind == Kind::Dir {
        newest_modified(tree, id)
    } else {
        tree.node(id).modified_ms
    }
}

pub(super) fn age_days(modified_ms: Option<u64>, now_ms: u64) -> Option<u64> {
    modified_ms.map(|modified| now_ms.saturating_sub(modified) / DAY_MS)
}

pub(super) fn classify_file(name: &str, age: Option<u64>, in_downloads: bool) -> Option<JunkKind> {
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

pub(super) enum DirClass {
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

pub(super) fn classify_dir(tree: &Tree, parent: NodeId, id: NodeId, now_ms: u64) -> DirClass {
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
pub(super) fn segments(path: &Path) -> Vec<String> {
    path.to_string_lossy()
        .to_lowercase()
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Whether the last segments of `parts` are `pattern`; `*` stands for any one segment.
pub(super) fn ends_with(parts: &[String], pattern: &[&str]) -> bool {
    parts.len() >= pattern.len()
        && parts[parts.len() - pattern.len()..]
            .iter()
            .zip(pattern)
            .all(|(part, want)| *want == "*" || part.eq_ignore_ascii_case(want))
}

/// Where each catalog base sits in a path: the segments before the entry's own.
pub(super) fn base_prefixes(base: Base) -> &'static [&'static [&'static str]] {
    match base {
        Base::LocalAppData => &[&["appdata", "local"]],
        Base::AppData => &[&["appdata", "roaming"]],
        Base::UserProfile => &[&["users", "*"], &["home", "*"]],
        Base::SystemDrive => &[&[]],
    }
}

/// Whether folder `id` (named `lower_name`, lowercase) is the folder `entry` describes. The tree
/// has no notion of a drive's special folders, so the match is on the path's last segments.
pub(super) fn matches_entry(tree: &Tree, id: NodeId, lower_name: &str, entry: &Entry) -> bool {
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
pub(super) fn in_user_profile(tree: &Tree, id: NodeId) -> bool {
    let parts = segments(&tree.path(id));
    parts.len() >= 2 && matches!(parts[parts.len() - 2].as_str(), "users" | "home")
}

/// Lock files that show a `package.json` is a real project.
pub(super) const LOCKFILES: [&str; 6] = [
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
pub(super) fn stale_project(
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
pub(super) fn reinstall(lock: &str) -> &'static str {
    match lock {
        "pnpm-lock.yaml" => "Run `pnpm install` in the project folder.",
        "yarn.lock" => "Run `yarn install --frozen-lockfile` in the project folder.",
        "bun.lockb" | "bun.lock" => "Run `bun install` in the project folder.",
        _ => "Run `npm ci` in the project folder.",
    }
}

/// The live child of `id` called `name`, ignoring case.
pub(super) fn child_named(tree: &Tree, id: NodeId, name: &str) -> Option<NodeId> {
    tree.live_children(id)
        .find(|&child| tree.node(child).name.eq_ignore_ascii_case(name))
}

/// Folders an app or a language runtime owns. What is inside belongs to it, not to a project,
/// so it is never listed. Segments are matched with both slashes, so this holds on any host.
pub(super) fn runtime_owned(path: &Path) -> bool {
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
pub(super) const PYTHON_ROOTS: [&str; 7] = [
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
pub(super) struct Owners {
    rooted: HashMap<NodeId, bool>,
}

impl Owners {
    pub(super) fn of(&mut self, tree: &Tree, pycache: NodeId) -> NodeId {
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

    pub(super) fn is_root(&mut self, tree: &Tree, dir: NodeId) -> bool {
        *self.rooted.entry(dir).or_insert_with(|| {
            PYTHON_ROOTS
                .iter()
                .any(|name| child_named(tree, dir, name).is_some())
        })
    }
}

/// The latest modified time of any file at or below `id`.
pub(super) fn newest_modified(tree: &Tree, id: NodeId) -> Option<u64> {
    let mut newest = None;
    let mut stack = vec![id];
    while let Some(at) = stack.pop() {
        let node = tree.node(at);
        newest = newest.max(node.modified_ms);
        stack.extend(tree.live_children(at));
    }
    newest
}
