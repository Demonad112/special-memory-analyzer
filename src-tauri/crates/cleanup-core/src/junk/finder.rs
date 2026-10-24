//! Collects the rows of one junk scan into groups.

use super::*;

pub(super) struct Finder<'a> {
    pub(super) tree: &'a Tree,
    pub(super) protected: &'a Protected,
    pub(super) root: PathBuf,
    pub(super) groups: Vec<JunkGroup>,
}

impl Finder<'_> {
    pub(super) fn allowed(&self, id: NodeId) -> bool {
        self.protected
            .check(&self.tree.path(id), &self.root)
            .is_ok()
    }

    /// The group for `kind` in `tier`, made when the first row arrives.
    pub(super) fn group(&mut self, kind: JunkKind, tier: Tier) -> &mut JunkGroup {
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
    pub(super) fn push(
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
    pub(super) fn add_one(&mut self, kind: JunkKind, verdict: Verdict, id: NodeId) {
        if self.allowed(id) {
            self.push(kind, verdict, id, &[id], true);
        }
    }

    pub(super) fn add(&mut self, id: NodeId, kind: JunkKind) {
        self.add_one(kind, kind_verdict(kind), id);
    }

    pub(super) fn node_modules(&mut self, id: NodeId, restore: &str) {
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
    pub(super) fn pycache(&mut self, owner: NodeId, mut dirs: Vec<NodeId>) {
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
    pub(super) fn dot_cache(&mut self, folder: NodeId) {
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
    pub(super) fn review(&mut self, id: NodeId, entry: &Entry) {
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
    pub(super) fn chrome(&mut self, folder: NodeId) {
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
    pub(super) fn report_only(&mut self, id: NodeId, min: u64) {
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
