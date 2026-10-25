//! The permissions audit: reads the owner and the access list of every folder in the readiness
//! tree (and of every file when asked) and reports what a migration has to look at: folders that
//! stop inheriting, accounts that no longer exist, write access for everyone, files with their
//! own entries, and items whose permissions could not be read.
//!
//! It only reads. Descriptors come from [`scan_core::security::read`], which asks for
//! `READ_CONTROL` alone and never follows a reparse point, so no file data is read and an
//! online-only file stays in the cloud (online-only files are skipped anyway). The audit walks the
//! scan tree: links are never entered and never read.
//!
//! An entry a folder inherited from its parent is not reported again on the folder: the parent
//! shows it once. Only the source folder itself reports everything it has, since its parent is not
//! part of the check. Account names are looked up once per SID, asked of the server for a
//! `\\server\share` source (D-07).

use crate::readiness::exact_path;
use scan_core::security::{
    self, is_wide_write_grant, Account, Ace, AceKind, Descriptor, Rights, SidNames, What,
};
use scan_core::{CancelToken, Kind, NodeId, ScanError, Tree};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

/// Most findings of one type a saved report keeps; the counts always cover every finding.
pub const FINDING_LIMIT: usize = 20_000;

/// Most access entries a saved report keeps for the permissions CSV. A report past it says so.
pub const ENTRY_LIMIT: usize = 100_000;

/// Reader threads. Each read is one open with `READ_CONTROL`; more threads mostly wait on the
/// disk or the server.
const THREADS: usize = 8;

/// Items per batch of work a thread takes at once.
const BATCH: usize = 64;

/// What the audit does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditOptions {
    /// Read every file's permissions too (D-05). Off: folders only.
    pub every_file: bool,
    /// The server to ask for account names, for a `\\server\share` source.
    pub server: Option<String>,
}

/// The kinds of finding, in the order the report shows them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FindingKind {
    /// A folder whose access list is protected: it does not inherit from its parent.
    #[default]
    BrokenInheritance,
    /// An owner or entry whose SID no longer resolves.
    UnknownAccount,
    /// Everyone, Authenticated Users, Users, Domain Users or Anonymous can write (D-06).
    WideWrite,
    /// A file with entries of its own.
    ExplicitFile,
    /// The permissions could not be read.
    Unreadable,
    /// Written by a newer version; shown as is.
    #[serde(other)]
    Other,
}

impl FindingKind {
    /// Every kind the audit produces, in the order the report shows them.
    pub const ALL: [FindingKind; 5] = [
        Self::BrokenInheritance,
        Self::UnknownAccount,
        Self::WideWrite,
        Self::ExplicitFile,
        Self::Unreadable,
    ];

    /// The heading of the group.
    pub fn heading(self) -> &'static str {
        match self {
            Self::BrokenInheritance => "Inheritance broken",
            Self::UnknownAccount => "Unknown accounts",
            Self::WideWrite => "Wide write access",
            Self::ExplicitFile => "Files with their own entries",
            Self::Unreadable => "Permissions not readable",
            Self::Other => "Other permission findings",
        }
    }

    /// What it means for the migration, in one line.
    pub fn explanation(self) -> &'static str {
        match self {
            Self::BrokenInheritance => {
                "These folders do not take permissions from their parent. Copy them with their permissions and check the new parent grants what is expected."
            }
            Self::UnknownAccount => {
                "The account was deleted (or lives in a domain this PC cannot see). The entry does nothing and will not map to anyone at the target."
            }
            Self::WideWrite => {
                "Anyone in these groups can change files here. Normal on some shares, a finding on most."
            }
            Self::ExplicitFile => {
                "Permissions set on single files are easy to lose or forget in a migration."
            }
            Self::Unreadable => {
                "Run as administrator to read more, or fix the folder's permissions first."
            }
            Self::Other => "",
        }
    }

    /// Findings that call for action before the copy; the rest describe how the share is built.
    pub fn is_issue(self) -> bool {
        matches!(
            self,
            Self::UnknownAccount | Self::WideWrite | Self::Unreadable | Self::Other
        )
    }
}

/// One thing to look at: an item and why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Finding {
    /// Relative to the source, `\`-separated; empty for the source folder itself.
    pub path: String,
    pub dir: bool,
    pub kind: FindingKind,
    pub detail: String,
    /// The account's SID, when the finding is about one.
    pub sid: String,
}

/// How many findings of one kind there are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FindingCount {
    pub kind: FindingKind,
    pub count: u64,
}

/// One access entry, with the account named.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EntryRow {
    /// `DOMAIN\name`, or the SID when it has no name.
    pub account: String,
    pub sid: String,
    /// `Allow` or `Deny`.
    pub access: String,
    /// Explorer's words for the rights ("Modify", "Special").
    pub rights: String,
    pub mask: u32,
    pub inherited: bool,
    pub applies_to: String,
}

/// The permissions of one item, for the CSV.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuditedItem {
    pub path: String,
    pub dir: bool,
    pub owner: String,
    pub owner_sid: String,
    pub protected: bool,
    /// Owner and access list as SDDL text (folders only).
    pub sddl: String,
    pub entries: Vec<EntryRow>,
}

/// Whether the audit ran.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuditStatus {
    /// Turned off for this check (and what reports from before the audit existed load as).
    #[default]
    Off,
    Checked,
    /// Not a Windows system: nothing was read.
    NotSupported,
    /// Written by a newer version.
    #[serde(other)]
    Other,
}

/// What the audit did, and the findings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionsReport {
    pub status: AuditStatus,
    /// Every file was read, not just folders.
    pub every_file: bool,
    /// The check ran as administrator: more of the tree could be read.
    pub elevated: bool,
    /// Account names were asked of this server (empty: this PC).
    pub server: String,
    /// Said when a mapped drive's server could not be found, so names were asked of this PC.
    pub server_note: String,
    pub folders_read: u64,
    pub files_read: u64,
    /// Items not read: links and online-only files, and names that are not valid Unicode.
    pub skipped: u64,
    /// Findings that call for action (see [`FindingKind::is_issue`]).
    pub issues: u64,
    pub counts: Vec<FindingCount>,
    /// Grouped by kind, then by path; at most [`FINDING_LIMIT`] per kind.
    pub findings: Vec<Finding>,
    /// More findings of some kind were found than `findings` keeps; the counts cover them all.
    pub findings_truncated: bool,
    /// Folders (and files with entries of their own) with their entries: the permissions CSV.
    pub items: Vec<AuditedItem>,
    /// More items had entries than [`ENTRY_LIMIT`] keeps.
    pub items_truncated: bool,
}

impl PermissionsReport {
    pub fn count_of(&self, kind: FindingKind) -> u64 {
        self.counts
            .iter()
            .filter(|count| count.kind == kind)
            .map(|count| count.count)
            .sum()
    }

    pub fn listed(&self, kind: FindingKind) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(move |finding| finding.kind == kind)
    }

    /// The same report without the findings and items (the totals the UI shows up front).
    pub fn summary(&self) -> Self {
        Self {
            findings: Vec::new(),
            items: Vec::new(),
            ..self.clone()
        }
    }
}

/// Live counters of the pass.
#[derive(Debug, Default)]
pub struct Counters {
    pub total: AtomicU64,
    pub done: AtomicU64,
}

/// The server named by a path on a share: `\\srv\share\x` and `\\?\UNC\srv\share\x` give `srv`.
/// `None` for a local path.
pub fn server_of(path: &str) -> Option<String> {
    let rest = if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        rest
    } else if path.starts_with(r"\\?\") || path.starts_with(r"\\.\") {
        return None;
    } else {
        path.strip_prefix(r"\\")?
    };
    let server = rest.split('\\').next().unwrap_or_default();
    (!server.is_empty()).then(|| server.to_owned())
}

type Read<'a> = dyn Fn(&Path, What) -> io::Result<Descriptor> + Sync + 'a;

/// What one item produced.
#[derive(Default)]
struct ItemResult {
    findings: Vec<(FindingKind, String, String)>,
    item: Option<AuditedItem>,
    /// The item became a link or an online-only file after the scan and was not read (DSA-091):
    /// counted as skipped, not as unreadable.
    changed: bool,
}

struct Target {
    id: NodeId,
    dir: bool,
}

/// What the audit found, before it is capped for a saved report. The findings are bounded while
/// they are collected: at most `FINDING_LIMIT` per kind, the first ones in tree order. `counts`
/// covers every finding.
pub struct Audit {
    findings: Vec<Finding>,
    /// Every finding of each kind, kept or not.
    counts: HashMap<FindingKind, u64>,
    /// More findings of some kind were found than were kept.
    findings_truncated: bool,
    items: Vec<AuditedItem>,
    /// More items had entries than the budget keeps.
    items_truncated: bool,
    folders_read: u64,
    files_read: u64,
    skipped: u64,
}

/// The folders (and, with `every_file`, files) to read, in tree order, and how many items the
/// audit leaves alone. Links are never read; online-only files are never opened.
fn targets(tree: &Tree, every_file: bool) -> (Vec<Target>, u64) {
    let mut found = Vec::new();
    let mut skipped = 0u64;
    for index in 0..tree.len() {
        let id = index as NodeId;
        let node = tree.node(id);
        if !tree.is_live(id) {
            continue;
        }
        let dir = node.kind == Kind::Dir;
        if node.kind == Kind::Link {
            skipped += 1;
        } else if !dir && !every_file {
            // Folders only.
        } else if node.lossy_name || (!dir && node.cloud_files > 0) {
            skipped += 1;
        } else {
            found.push(Target { id, dir });
        }
    }
    (found, skipped)
}

/// One audited item on its way to the merge: its path, whether it is a folder, and what it gave.
type Piece = (String, bool, ItemResult);

/// Batches a worker may run ahead of the merge. Bounds what waits to be merged.
const WINDOW: usize = 4 * THREADS;

/// Results merged in tree order, as batches finish. Items stop being kept once the entry budget
/// is spent; the counts and findings go on.
struct Merge {
    /// The next batch to merge.
    next: usize,
    pending: BTreeMap<usize, Vec<Piece>>,
    audit: Audit,
    entry_limit: usize,
    entries_kept: usize,
    /// Findings kept per kind, up to `finding_limit`.
    finding_limit: usize,
    findings_kept: HashMap<FindingKind, usize>,
}

impl Merge {
    fn add(&mut self, (path, dir, result): Piece) {
        let audit = &mut self.audit;
        if result.changed {
            audit.skipped += 1;
            return;
        }
        if dir {
            audit.folders_read += 1;
        } else {
            audit.files_read += 1;
        }
        for (kind, detail, sid) in result.findings {
            *audit.counts.entry(kind).or_default() += 1;
            // The batches arrive in tree order, so the kept findings are the first ones, the
            // same on every run, and memory stays bounded however many are found.
            let kept = self.findings_kept.entry(kind).or_default();
            if *kept >= self.finding_limit {
                audit.findings_truncated = true;
                continue;
            }
            *kept += 1;
            audit.findings.push(Finding {
                path: path.clone(),
                dir,
                kind,
                detail,
                sid,
            });
        }
        if let Some(mut item) = result.item {
            let cost = item.entries.len().max(1);
            // The first item that does not fit ends the kept items for good, so the same tree
            // keeps the same items on every run.
            if !audit.items_truncated && self.entries_kept + cost <= self.entry_limit {
                self.entries_kept += cost;
                item.path = path;
                audit.items.push(item);
            } else {
                audit.items_truncated = true;
            }
        }
    }

    /// Takes a finished batch and merges every batch that is now next in line.
    fn submit(&mut self, batch: usize, pieces: Vec<Piece>) {
        self.pending.insert(batch, pieces);
        while let Some(ready) = self.pending.remove(&self.next) {
            for piece in ready {
                self.add(piece);
            }
            self.next += 1;
        }
    }
}

/// Reads the permissions of the tree and classifies them. `read` gives the descriptor of a path
/// (the system call in the app); `names` resolves SIDs. `Err(Cancelled)` when cancelled.
pub fn audit(
    tree: &Tree,
    options: &AuditOptions,
    read: &Read,
    names: &SidNames,
    cancel: &CancelToken,
    counters: &Counters,
) -> Result<Audit, ScanError> {
    audit_with_limits(
        tree,
        options,
        read,
        names,
        cancel,
        counters,
        ENTRY_LIMIT,
        FINDING_LIMIT,
    )
}

/// [`audit`] keeping the entries of at most `entry_limit` access entries' worth of items.
pub fn audit_with_limit(
    tree: &Tree,
    options: &AuditOptions,
    read: &Read,
    names: &SidNames,
    cancel: &CancelToken,
    counters: &Counters,
    entry_limit: usize,
) -> Result<Audit, ScanError> {
    audit_with_limits(
        tree,
        options,
        read,
        names,
        cancel,
        counters,
        entry_limit,
        FINDING_LIMIT,
    )
}

/// [`audit`] keeping the entries of at most `entry_limit` access entries' worth of items and at
/// most `finding_limit` findings of each kind. What is kept is the first ones in tree order,
/// however the threads happen to run, and memory stays bounded whatever the size of the tree.
/// The counts cover everything found.
#[allow(clippy::too_many_arguments)]
pub fn audit_with_limits(
    tree: &Tree,
    options: &AuditOptions,
    read: &Read,
    names: &SidNames,
    cancel: &CancelToken,
    counters: &Counters,
    entry_limit: usize,
    finding_limit: usize,
) -> Result<Audit, ScanError> {
    let (targets, skipped) = targets(tree, options.every_file);
    counters.done.store(0, Ordering::Relaxed);
    counters
        .total
        .store(targets.len() as u64, Ordering::Relaxed);
    let next = AtomicUsize::new(0);
    let merge = Mutex::new(Merge {
        next: 0,
        pending: BTreeMap::new(),
        audit: Audit {
            findings: Vec::new(),
            counts: HashMap::new(),
            findings_truncated: false,
            items: Vec::new(),
            items_truncated: false,
            folders_read: 0,
            files_read: 0,
            skipped,
        },
        entry_limit,
        entries_kept: 0,
        finding_limit,
        findings_kept: HashMap::new(),
    });
    let turn = Condvar::new();
    let lock = || merge.lock().unwrap_or_else(PoisonError::into_inner);
    let threads = THREADS.min(targets.len().div_ceil(BATCH)).max(1);
    thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let batch = next.fetch_add(1, Ordering::Relaxed);
                let from = batch.saturating_mul(BATCH);
                if from >= targets.len() || cancel.is_cancelled() {
                    break;
                }
                // Not too far ahead of the merge: the batch the merge waits for is always taken.
                let mut merging = lock();
                while batch >= merging.next + WINDOW && !cancel.is_cancelled() {
                    merging = turn
                        .wait_timeout(merging, Duration::from_millis(50))
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                drop(merging);
                let mut pieces = Vec::new();
                for target in &targets[from..(from + BATCH).min(targets.len())] {
                    if cancel.is_cancelled() {
                        return;
                    }
                    let path = exact_path(&tree.path(target.id));
                    let result = classify(
                        read(&path, What::OwnerAndDacl),
                        target.dir,
                        target.id == 0,
                        options.server.as_deref(),
                        names,
                    );
                    counters.done.fetch_add(1, Ordering::Relaxed);
                    pieces.push((relative(tree, target.id), target.dir, result));
                }
                lock().submit(batch, pieces);
                turn.notify_all();
            });
        }
    });
    if cancel.is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    Ok(merge
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .audit)
}

fn relative(tree: &Tree, id: NodeId) -> String {
    let mut names = Vec::new();
    let mut current = id;
    while let Some(parent) = tree.parent(current) {
        names.push(tree.node(current).name.as_str());
        current = parent;
    }
    names.reverse();
    names.join("\\")
}

/// The findings and the CSV rows of one item. `root` is the source folder, which reports the
/// entries it inherited too: its parent is not part of the check.
fn classify(
    read: io::Result<Descriptor>,
    dir: bool,
    root: bool,
    server: Option<&str>,
    names: &SidNames,
) -> ItemResult {
    let descriptor = match read {
        Ok(descriptor) => descriptor,
        Err(err) if scan_core::security::is_changed_since_scan(&err) => {
            return ItemResult {
                changed: true,
                ..ItemResult::default()
            }
        }
        Err(err) => {
            return ItemResult {
                findings: vec![(FindingKind::Unreadable, err.to_string(), String::new())],
                item: None,
                changed: false,
            }
        }
    };
    let mut findings = Vec::new();
    let named = |sid: &str| names.lookup(sid, server);
    if let Some(owner) = &descriptor.owner {
        if named(owner) == Account::NotFound {
            findings.push((
                FindingKind::UnknownAccount,
                format!("Owner {owner} no longer exists"),
                owner.clone(),
            ));
        }
    }
    if dir && descriptor.protected && !root {
        findings.push((
            FindingKind::BrokenInheritance,
            "Does not inherit permissions from its parent".to_owned(),
            String::new(),
        ));
    }
    if descriptor.null_dacl {
        findings.push((
            FindingKind::WideWrite,
            "No access list at all: everyone has full control".to_owned(),
            "S-1-1-0".to_owned(),
        ));
    }
    let mut unknown_seen = HashSet::new();
    let mut explicit = 0usize;
    for ace in &descriptor.aces {
        if !ace.inherited {
            explicit += 1;
        }
        if ace.inherited && !root {
            continue;
        }
        if matches!(ace.kind, AceKind::Allow | AceKind::Deny)
            && named(&ace.sid) == Account::NotFound
            && unknown_seen.insert(ace.sid.to_ascii_uppercase())
        {
            findings.push((
                FindingKind::UnknownAccount,
                format!("{} no longer exists", ace.sid),
                ace.sid.clone(),
            ));
        }
        if let Some(principal) = is_wide_write_grant(ace) {
            findings.push((
                FindingKind::WideWrite,
                format!(
                    "{}: {} ({})",
                    principal.words(),
                    Rights::from_mask(ace.mask).words(),
                    ace.applies_to.words()
                ),
                ace.sid.clone(),
            ));
        }
    }
    if !dir && explicit > 0 {
        findings.push((
            FindingKind::ExplicitFile,
            if explicit == 1 {
                "1 entry of its own".to_owned()
            } else {
                format!("{explicit} entries of its own")
            },
            String::new(),
        ));
    }
    // Folders always make a CSV row; a file only when it has entries of its own.
    let item = (dir || explicit > 0).then(|| AuditedItem {
        path: String::new(),
        dir,
        owner: descriptor
            .owner
            .as_deref()
            .map(|owner| named(owner).display(owner))
            .unwrap_or_default(),
        owner_sid: descriptor.owner.clone().unwrap_or_default(),
        protected: descriptor.protected,
        sddl: descriptor.sddl.clone().unwrap_or_default(),
        entries: descriptor
            .aces
            .iter()
            .filter(|ace| matches!(ace.kind, AceKind::Allow | AceKind::Deny))
            .map(|ace| entry_row(ace, &named(&ace.sid)))
            .collect(),
    });
    ItemResult {
        findings,
        item,
        changed: false,
    }
}

pub(crate) fn entry_row(ace: &Ace, account: &Account) -> EntryRow {
    EntryRow {
        account: account.display(&ace.sid),
        sid: ace.sid.clone(),
        access: if ace.kind == AceKind::Deny {
            "Deny"
        } else {
            "Allow"
        }
        .to_owned(),
        rights: Rights::from_mask(ace.mask).words().to_owned(),
        mask: ace.mask,
        inherited: ace.inherited,
        applies_to: ace.applies_to.words().to_owned(),
    }
}

impl Audit {
    /// The saved form: findings capped per kind, items capped by their entries.
    pub fn into_report(
        self,
        options: &AuditOptions,
        elevated: bool,
        supported: bool,
    ) -> PermissionsReport {
        let counts = self.counts;
        let mut findings = Vec::new();
        for kind in FindingKind::ALL {
            let mut listed: Vec<Finding> = self
                .findings
                .iter()
                .filter(|finding| finding.kind == kind)
                .cloned()
                .collect();
            listed.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.detail.cmp(&b.detail)));
            listed.truncate(FINDING_LIMIT);
            findings.extend(listed);
        }
        let counts: Vec<FindingCount> = FindingKind::ALL
            .into_iter()
            .filter_map(|kind| counts.get(&kind).map(|&count| FindingCount { kind, count }))
            .collect();
        let issues = counts
            .iter()
            .filter(|count| count.kind.is_issue())
            .map(|count| count.count)
            .sum();
        PermissionsReport {
            status: if supported {
                AuditStatus::Checked
            } else {
                AuditStatus::NotSupported
            },
            every_file: options.every_file,
            elevated,
            server: options.server.clone().unwrap_or_default(),
            server_note: String::new(),
            folders_read: self.folders_read,
            files_read: self.files_read,
            skipped: self.skipped,
            issues,
            counts,
            findings,
            findings_truncated: self.findings_truncated,
            items: self.items,
            items_truncated: self.items_truncated,
        }
    }
}

/// The system calls behind the audit, for the app.
pub fn system_read(path: &Path, what: What) -> io::Result<Descriptor> {
    security::read_unless_changed(path, what)
}

#[cfg(test)]
mod tests;
