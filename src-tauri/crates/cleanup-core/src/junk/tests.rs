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

/// Joins `/`-separated segments one component at a time, so the result uses only the
/// platform separator, like the paths the finder builds.
fn join_all(base: &Path, segments: &str) -> PathBuf {
    segments
        .split('/')
        .fold(base.to_path_buf(), |path, part| path.join(part))
}

/// Where a catalog entry sits under a fake profile, so its path is matched like a real one.
fn place(root: &Path, entry: &Entry) -> PathBuf {
    let base = match entry.base {
        Base::LocalAppData => join_all(root, "Users/sam/AppData/Local"),
        Base::AppData => join_all(root, "Users/sam/AppData/Roaming"),
        Base::UserProfile => join_all(root, "Users/sam"),
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
        assert_eq!(Path::new(&item.path), folder, "{}", entry.label);
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
            .find(|item| Path::new(&item.path) == folder)
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
    let data = join_all(root, "Users/sam/AppData/Local/Google/Chrome/User Data");
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
    assert_eq!(Path::new(&total[0].path), data);
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
    let data = join_all(root, "Users/sam/AppData/Local/Google/Chrome/User Data");
    write(&data.join("Default/Cache/c1"), 0);
    let tree = scan(root, &ScanState::default()).unwrap();
    let keep = join_all(&data, "Default/Cache").display().to_string();
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
