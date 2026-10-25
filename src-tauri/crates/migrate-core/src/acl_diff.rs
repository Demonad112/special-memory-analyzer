//! Permissions compare (F-004): the difference between the security descriptors of the same
//! item on the source and on the destination.
//!
//! Decision D-09: entries are compared by SID, as a set keyed by SID, entry type, access mask and
//! flags. Entries a folder inherited are compared only at the compare root, because below it they
//! follow from the root. When the SIDs differ but resolve to the same account name (a local
//! account recreated on the new server), that is its own, milder status instead of a mismatch.
//!
//! [`diff_descriptors`] is pure: it works on decoded [`Descriptor`]s and a closure that says
//! whether two SIDs name the same account, so it is tested on every platform with plain structs.
//!
//! [`compare_permissions`] runs it over the folder pairs of a finished size compare (and over the
//! file pairs, reported only when either side has an entry of its own). It only reads: each
//! descriptor comes from [`scan_core::security::read`], which asks for `READ_CONTROL` alone and
//! never follows a reparse point, so no file data is read. Links are never read and online-only
//! files are skipped. Account names are asked of each side's own server (D-07), so each side of
//! a compare can be on a different one.
//!
//! Memory stays bounded while the pass runs, not only when it is saved: results are merged in
//! tree order, the details of at most [`DETAIL_LIMIT`] entries' worth of items are kept, and the
//! counts always cover every item. The items kept are the first ones in tree order, so the same
//! trees keep the same items on every run, whatever the threads do.

use crate::acl_audit::{entry_row, AuditStatus, Counters, EntryRow};
use crate::readiness::exact_path;
use scan_core::security::{Account, Ace, Descriptor, SidNames, What};
use scan_core::{CancelToken, DiffRow, DiffStatus, DiffTree, Kind, NodeId, ScanError, Side};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

/// How one item compares between the source and the destination.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermStatus {
    #[default]
    Same,
    OwnerDiffers,
    /// Entries are missing at the destination, or it has others.
    EntriesDiffer,
    /// Only one side stops inheriting from its parent.
    InheritanceDiffers,
    /// The accounts match by name but not by SID. Shown in amber: usually a recreated account.
    SameNameDifferentAccount,
    /// One side could not be read.
    NotReadable,
    /// Written by a newer version.
    #[serde(other)]
    Other,
}

impl PermStatus {
    /// Every status that is not `Same`, in the order reports list them (worst first).
    pub const DIFFERENCES: [Self; 5] = [
        Self::EntriesDiffer,
        Self::OwnerDiffers,
        Self::InheritanceDiffers,
        Self::NotReadable,
        Self::SameNameDifferentAccount,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Same => "Same",
            Self::OwnerDiffers => "Owner differs",
            Self::EntriesDiffer => "Entries differ",
            Self::InheritanceDiffers => "Inheritance differs",
            Self::SameNameDifferentAccount => "Same name, different account",
            Self::NotReadable => "Not readable",
            Self::Other => "Other",
        }
    }

    /// True for every status but `Same`.
    pub fn is_difference(self) -> bool {
        self != Self::Same
    }
}

/// The owner on each side when they are not the same account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerChange {
    /// `None` when the descriptor has no owner.
    pub source: Option<String>,
    pub destination: Option<String>,
    /// The two SIDs differ but name the same account.
    pub same_name: bool,
}

/// Everything that differs between two descriptors. Empty fields mean "no difference".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AclDiff {
    pub owner: Option<OwnerChange>,
    /// `(source, destination)` inheritance-protected flags, when they differ.
    pub protected: Option<(bool, bool)>,
    /// `(source, destination)` null-DACL flags, when they differ (a null DACL grants everyone
    /// full control).
    pub null_dacl: Option<(bool, bool)>,
    /// Entries on the source the destination lacks.
    pub missing: Vec<Ace>,
    /// Entries only the destination has.
    pub extra: Vec<Ace>,
    /// `(source, destination)` entries that are the same but for a SID that names the same
    /// account.
    pub renamed: Vec<(Ace, Ace)>,
}

impl AclDiff {
    /// The one status the item shows. Entries come first because access is what a client asks
    /// about; the rest of the differences stay in the detail.
    pub fn status(&self) -> PermStatus {
        let owner_differs = self.owner.as_ref().is_some_and(|owner| !owner.same_name);
        if !self.missing.is_empty() || !self.extra.is_empty() || self.null_dacl.is_some() {
            PermStatus::EntriesDiffer
        } else if owner_differs {
            PermStatus::OwnerDiffers
        } else if self.protected.is_some() {
            PermStatus::InheritanceDiffers
        } else if self.owner.is_some() || !self.renamed.is_empty() {
            PermStatus::SameNameDifferentAccount
        } else {
            PermStatus::Same
        }
    }
}

/// The identity of an entry for the set comparison: SID, entry type, mask and flags.
type Key = (String, u8, u32, u8);

fn key(ace: &Ace) -> Key {
    (
        ace.sid.to_ascii_uppercase(),
        ace.ace_type,
        ace.mask,
        ace.flags,
    )
}

/// The entries that count: all of them at the compare root, only the explicit ones below it.
/// Duplicates collapse (it is a set) and the order is by key, so the result never depends on
/// the order the system listed them in.
fn entry_set(descriptor: &Descriptor, root: bool) -> BTreeMap<Key, &Ace> {
    descriptor
        .aces
        .iter()
        .filter(|ace| root || !ace.inherited)
        .map(|ace| (key(ace), ace))
        .collect()
}

/// True when the descriptor has an entry of its own (not inherited).
pub fn has_explicit_entries(descriptor: &Descriptor) -> bool {
    descriptor.aces.iter().any(|ace| !ace.inherited)
}

/// Compares the source's descriptor with the destination's. `root` is the compare root, the only
/// item whose inherited entries count. `same_account(source_sid, destination_sid)` says whether
/// two different SIDs name the same account.
pub fn diff_descriptors(
    source: &Descriptor,
    destination: &Descriptor,
    root: bool,
    same_account: &dyn Fn(&str, &str) -> bool,
) -> AclDiff {
    let mut diff = AclDiff::default();

    let same_sid = |a: &Option<String>, b: &Option<String>| match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (None, None) => true,
        _ => false,
    };
    if !same_sid(&source.owner, &destination.owner) {
        let same_name = match (&source.owner, &destination.owner) {
            (Some(a), Some(b)) => same_account(a, b),
            _ => false,
        };
        diff.owner = Some(OwnerChange {
            source: source.owner.clone(),
            destination: destination.owner.clone(),
            same_name,
        });
    }
    if source.protected != destination.protected {
        diff.protected = Some((source.protected, destination.protected));
    }
    if source.null_dacl != destination.null_dacl {
        diff.null_dacl = Some((source.null_dacl, destination.null_dacl));
    }

    let left = entry_set(source, root);
    let right = entry_set(destination, root);
    let only_left: Vec<&Ace> = left
        .iter()
        .filter(|(key, _)| !right.contains_key(*key))
        .map(|(_, ace)| *ace)
        .collect();
    let mut only_right: Vec<Option<&Ace>> = right
        .iter()
        .filter(|(key, _)| !left.contains_key(*key))
        .map(|(_, ace)| Some(*ace))
        .collect();

    // A source-only entry pairs with a destination-only one that differs by nothing but a SID
    // naming the same account. Each destination entry pairs once, in key order.
    for ace in only_left {
        let partner = only_right.iter().position(|candidate| {
            candidate.is_some_and(|other| {
                other.ace_type == ace.ace_type
                    && other.mask == ace.mask
                    && other.flags == ace.flags
                    && same_account(&ace.sid, &other.sid)
            })
        });
        match partner.and_then(|at| only_right[at].take()) {
            Some(other) => diff.renamed.push((ace.clone(), other.clone())),
            None => diff.missing.push(ace.clone()),
        }
    }
    diff.extra = only_right.into_iter().flatten().cloned().collect();
    diff
}

/// Most access entries' worth of differing items a result and its saved report keep. A result
/// past it says so; the counts always cover every item.
pub const DETAIL_LIMIT: usize = 100_000;

/// Reader threads; each read is one open with `READ_CONTROL`.
const THREADS: usize = 8;

/// Pairs per batch of work a thread takes at once.
const BATCH: usize = 64;

/// Batches a worker may run ahead of the merge. Bounds what waits to be merged.
const WINDOW: usize = 4 * THREADS;

/// An entry that is the same at both ends but for a SID naming the same account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SameNameEntry {
    pub source: EntryRow,
    pub destination: EntryRow,
}

/// What differs on one item, for the report and the details of a row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PermDetail {
    /// Relative to the source, `\`-separated; empty for the compare root.
    pub path: String,
    pub dir: bool,
    pub status: PermStatus,
    /// Both owners (account names, or SIDs) when they differ, else empty.
    pub source_owner: String,
    pub destination_owner: String,
    pub source_owner_sid: String,
    pub destination_owner_sid: String,
    pub source_protected: bool,
    pub destination_protected: bool,
    /// A sentence for what the entries cannot show (a missing access list).
    pub note: String,
    /// Why the item could not be read, when it could not.
    pub error: String,
    /// On the source, absent at the destination.
    pub missing: Vec<EntryRow>,
    /// Only at the destination.
    pub extra: Vec<EntryRow>,
    pub same_name: Vec<SameNameEntry>,
}

impl PermDetail {
    /// The access entries this item costs in the budget.
    fn cost(&self) -> usize {
        (self.missing.len() + self.extra.len() + 2 * self.same_name.len()).max(1)
    }

    /// One line for a table row's tooltip.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.error.is_empty() {
            parts.push(self.error.clone());
        }
        if !self.source_owner.is_empty() || !self.destination_owner.is_empty() {
            parts.push(format!(
                "Owner: {} → {}",
                or_none(&self.source_owner),
                or_none(&self.destination_owner)
            ));
        }
        if self.source_protected != self.destination_protected {
            let inheritance = |protected: bool| if protected { "off" } else { "on" };
            parts.push(format!(
                "Inheritance: {} at the source, {} at the destination",
                inheritance(self.source_protected),
                inheritance(self.destination_protected),
            ));
        }
        if !self.note.is_empty() {
            parts.push(self.note.clone());
        }
        if !self.missing.is_empty() {
            parts.push(format!("{} missing at the destination", self.missing.len()));
        }
        if !self.extra.is_empty() {
            parts.push(format!("{} only at the destination", self.extra.len()));
        }
        if !self.same_name.is_empty() {
            parts.push(format!(
                "{} under a different account with the same name",
                self.same_name.len()
            ));
        }
        parts.join("; ")
    }
}

fn or_none(text: &str) -> &str {
    if text.is_empty() {
        "(none)"
    } else {
        text
    }
}

/// How many items ended in one status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StatusCount {
    pub status: PermStatus,
    pub count: u64,
}

/// The permissions part of a saved compare: what was done, the totals and the differing items.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionsCompare {
    /// `Off` for a compare that did not look at permissions (and what reports from before this
    /// existed load as).
    pub status: AuditStatus,
    /// The compare ran as administrator: more of the trees could be read.
    pub elevated: bool,
    /// Account names were asked of these servers (empty: this PC).
    pub source_server: String,
    pub destination_server: String,
    /// Said when a mapped drive's server could not be found, so names were asked of this PC.
    pub server_note: String,
    /// Folder pairs and file pairs (with an entry of their own on either side) compared.
    pub folders_checked: u64,
    pub files_checked: u64,
    /// Pairs not read: links, online-only files, names that are not valid Unicode, and a file
    /// paired with a folder.
    pub skipped: u64,
    /// Items per difference, worst first; items that are the same are not listed.
    pub counts: Vec<StatusCount>,
    /// The differing items in tree order, at most [`DETAIL_LIMIT`] entries' worth.
    pub items: Vec<PermDetail>,
    /// More items differ than `items` keeps.
    pub items_truncated: bool,
}

impl PermissionsCompare {
    /// True when the compare did not look at permissions: the saved report then carries nothing
    /// about them, exactly as before the option existed.
    pub fn is_off(&self) -> bool {
        self.status == AuditStatus::Off
    }

    pub fn count_of(&self, status: PermStatus) -> u64 {
        self.counts
            .iter()
            .filter(|count| count.status == status)
            .map(|count| count.count)
            .sum()
    }

    /// The same report without the item details (the totals the UI shows up front).
    pub fn summary(&self) -> Self {
        Self {
            items: Vec::new(),
            ..self.clone()
        }
    }

    /// Items that differ in any way, the amber ones included.
    pub fn differences(&self) -> u64 {
        self.counts.iter().map(|count| count.count).sum()
    }

    /// Items that differ for real: everything but the same-name-different-account ones.
    pub fn mismatches(&self) -> u64 {
        self.differences() - self.count_of(PermStatus::SameNameDifferentAccount)
    }
}

/// What a table row shows about its permissions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermCell {
    /// `None` for a folder row that was not itself compared but has differences below it.
    pub status: Option<PermStatus>,
    /// Items below this row that differ in any way.
    pub below: u32,
    /// Empty when the item's details were not kept (see [`PermissionsCompare::items_truncated`]).
    pub summary: String,
}

/// The pass's result, kept to answer the table.
#[derive(Debug)]
pub struct PermissionsResult {
    /// Per diff node; `None` for a node that was not compared.
    statuses: Vec<Option<PermStatus>>,
    /// Per diff node: how many nodes below it differ in any way.
    below: Vec<u32>,
    /// The node of each entry in `report.items`, ascending.
    item_ids: Vec<NodeId>,
    report: PermissionsCompare,
}

impl PermissionsResult {
    /// The result of a compare that did not read permissions (not Windows).
    pub fn not_supported() -> Self {
        Self {
            statuses: Vec::new(),
            below: Vec::new(),
            item_ids: Vec::new(),
            report: PermissionsCompare {
                status: AuditStatus::NotSupported,
                ..PermissionsCompare::default()
            },
        }
    }

    pub fn report(&self) -> &PermissionsCompare {
        &self.report
    }

    /// The result with the elevated flag and servers the caller knows.
    pub fn with_context(
        mut self,
        elevated: bool,
        source_server: Option<&str>,
        destination_server: Option<&str>,
    ) -> Self {
        self.report.elevated = elevated;
        self.report.source_server = source_server.unwrap_or_default().to_owned();
        self.report.destination_server = destination_server.unwrap_or_default().to_owned();
        self
    }

    /// The same result with a note about where the account names came from.
    pub fn with_note(mut self, note: &str) -> Self {
        self.report.server_note = note.to_owned();
        self
    }

    pub fn status(&self, id: NodeId) -> Option<PermStatus> {
        self.statuses.get(id as usize).copied().flatten()
    }

    pub fn detail(&self, id: NodeId) -> Option<&PermDetail> {
        let at = self.item_ids.binary_search(&id).ok()?;
        self.report.items.get(at)
    }

    /// What the row of `id` shows, or `None` when it has nothing to show.
    pub fn cell(&self, id: NodeId) -> Option<PermCell> {
        let status = self.status(id);
        let below = self.below.get(id as usize).copied().unwrap_or(0);
        if status.is_none() && below == 0 {
            return None;
        }
        Some(PermCell {
            status,
            below,
            summary: self.detail(id).map(PermDetail::summary).unwrap_or_default(),
        })
    }
}

/// A row as the UI receives it: the size compare's row, plus the permissions cell when the
/// compare looked at permissions. Without one the row serializes exactly as [`DiffRow`] does.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareRow {
    #[serde(flatten)]
    pub row: DiffRow,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<PermCell>,
}

/// The rows below `id` with their permissions cells, or `None` when `id` does not exist.
pub fn compare_rows(
    diff: &DiffTree,
    id: NodeId,
    permissions: Option<&PermissionsResult>,
) -> Option<Vec<CompareRow>> {
    Some(
        diff.children(id)?
            .into_iter()
            .map(|row| CompareRow {
                permissions: permissions.and_then(|result| result.cell(row.id)),
                row,
            })
            .collect(),
    )
}

/// What the pass does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompareOptions {
    /// The server to ask for the source side's account names (`\\server\share` source).
    pub source_server: Option<String>,
    /// The server to ask for the destination side's account names.
    pub destination_server: Option<String>,
}

type Read<'a> = dyn Fn(&Path, What) -> io::Result<Descriptor> + Sync + 'a;

struct Target {
    id: NodeId,
    dir: bool,
}

/// The pairs to read, in tree order, and how many matched pairs the pass leaves alone. Links are
/// never read; online-only files are never opened.
fn targets(diff: &DiffTree) -> (Vec<Target>, u64) {
    let mut found = Vec::new();
    let mut skipped = 0u64;
    for index in 0..diff.len() {
        let id = index as NodeId;
        let Some(node) = diff.node(id) else {
            continue;
        };
        if node.left.is_none() || node.right.is_none() {
            continue;
        }
        let dir = node.kind == Kind::Dir;
        if node.status == DiffStatus::KindMismatch
            || node.kind == Kind::Link
            || node.lossy_name
            || (!dir && node.cloud)
        {
            skipped += 1;
        } else {
            found.push(Target { id, dir });
        }
    }
    (found, skipped)
}

/// What one pair produced.
struct Outcome {
    status: PermStatus,
    /// Kept only for a difference.
    detail: Option<PermDetail>,
}

struct Piece {
    id: NodeId,
    dir: bool,
    /// `None` for a file pair with no entry of its own on either side.
    outcome: Option<Outcome>,
    /// A side became a link or an online-only file after the scan (DSA-091): not read, counted
    /// as skipped.
    changed: bool,
}

/// True when either read refused because the item changed since the scan.
fn changed_since_scan(
    source: &io::Result<Descriptor>,
    destination: &io::Result<Descriptor>,
) -> bool {
    [source, destination].into_iter().any(|read| {
        read.as_ref()
            .is_err_and(scan_core::security::is_changed_since_scan)
    })
}

struct Context<'a> {
    names: &'a SidNames,
    source_server: Option<&'a str>,
    destination_server: Option<&'a str>,
}

impl Context<'_> {
    fn source(&self, sid: &str) -> Account {
        self.names.lookup(sid, self.source_server)
    }

    fn destination(&self, sid: &str) -> Account {
        self.names.lookup(sid, self.destination_server)
    }

    /// Two SIDs name the same account when both resolve and the names match. The domain is left
    /// out on purpose: a local account recreated on another server only differs in it.
    fn same_account(&self, source: &str, destination: &str) -> bool {
        match (self.source(source), self.destination(destination)) {
            (Account::Found { name: a, .. }, Account::Found { name: b, .. }) => {
                a.to_lowercase() == b.to_lowercase()
            }
            _ => false,
        }
    }
}

/// Compares one pair. `root` is the compare root.
fn classify(
    source: io::Result<Descriptor>,
    destination: io::Result<Descriptor>,
    dir: bool,
    root: bool,
    context: &Context,
) -> Option<Outcome> {
    let (source, destination) = match (source, destination) {
        (Ok(source), Ok(destination)) => (source, destination),
        (source, destination) => {
            let mut parts = Vec::new();
            if let Err(err) = source {
                parts.push(format!("Source: {err}"));
            }
            if let Err(err) = destination {
                parts.push(format!("Destination: {err}"));
            }
            return Some(Outcome {
                status: PermStatus::NotReadable,
                detail: Some(PermDetail {
                    dir,
                    status: PermStatus::NotReadable,
                    error: parts.join("; "),
                    ..PermDetail::default()
                }),
            });
        }
    };
    if !dir && !has_explicit_entries(&source) && !has_explicit_entries(&destination) {
        return None;
    }
    let diff = diff_descriptors(&source, &destination, root, &|a, b| {
        context.same_account(a, b)
    });
    let status = diff.status();
    if status == PermStatus::Same {
        return Some(Outcome {
            status,
            detail: None,
        });
    }
    let owner = |sid: &Option<String>, side: &dyn Fn(&str) -> Account| {
        sid.as_deref()
            .map(|sid| (side(sid).display(sid), sid.to_owned()))
            .unwrap_or_default()
    };
    let (source_owner, destination_owner) =
        diff.owner.as_ref().map_or_else(Default::default, |change| {
            (
                owner(&change.source, &|sid| context.source(sid)),
                owner(&change.destination, &|sid| context.destination(sid)),
            )
        });
    let note = match diff.null_dacl {
        Some((true, _)) => "The source has no access list at all (everyone has full control)",
        Some((false, _)) => "The destination has no access list at all (everyone has full control)",
        None => "",
    };
    let detail = PermDetail {
        path: String::new(),
        dir,
        status,
        source_owner: source_owner.0,
        destination_owner: destination_owner.0,
        source_owner_sid: source_owner.1,
        destination_owner_sid: destination_owner.1,
        source_protected: source.protected,
        destination_protected: destination.protected,
        note: note.to_owned(),
        error: String::new(),
        missing: diff
            .missing
            .iter()
            .map(|ace| entry_row(ace, &context.source(&ace.sid)))
            .collect(),
        extra: diff
            .extra
            .iter()
            .map(|ace| entry_row(ace, &context.destination(&ace.sid)))
            .collect(),
        same_name: diff
            .renamed
            .iter()
            .map(|(source, destination)| SameNameEntry {
                source: entry_row(source, &context.source(&source.sid)),
                destination: entry_row(destination, &context.destination(&destination.sid)),
            })
            .collect(),
    };
    Some(Outcome {
        status,
        detail: Some(detail),
    })
}

/// `\`-separated path of `id` on the source, empty for the root.
fn relative(diff: &DiffTree, id: NodeId) -> String {
    diff.relative_path(id, Side::Left)
        .map(|path| {
            path.components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("\\")
        })
        .unwrap_or_default()
}

/// The full path of `id` on one side. The root is the folder itself, with no trailing separator.
fn side_path(diff: &DiffTree, root: &Path, id: NodeId, side: Side) -> Option<PathBuf> {
    let relative = diff.relative_path(id, side)?;
    Some(if relative.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    })
}

/// Results merged in tree order as batches finish.
struct Merge {
    /// The next batch to merge.
    next: usize,
    pending: BTreeMap<usize, Vec<Piece>>,
    statuses: Vec<Option<PermStatus>>,
    counts: HashMap<PermStatus, u64>,
    items: Vec<PermDetail>,
    item_ids: Vec<NodeId>,
    items_truncated: bool,
    entries_kept: usize,
    entry_limit: usize,
    folders_checked: u64,
    files_checked: u64,
    /// Pairs found to be links or online-only files when they were about to be read.
    changed: u64,
}

impl Merge {
    fn add(&mut self, diff: &DiffTree, piece: Piece) {
        if piece.changed {
            self.changed += 1;
            return;
        }
        let Some(outcome) = piece.outcome else {
            return;
        };
        if piece.dir {
            self.folders_checked += 1;
        } else {
            self.files_checked += 1;
        }
        self.statuses[piece.id as usize] = Some(outcome.status);
        if !outcome.status.is_difference() {
            return;
        }
        *self.counts.entry(outcome.status).or_default() += 1;
        let Some(mut detail) = outcome.detail else {
            return;
        };
        // The first item that does not fit ends the kept items for good, so the same trees keep
        // the same items on every run.
        if !self.items_truncated && self.entries_kept + detail.cost() <= self.entry_limit {
            self.entries_kept += detail.cost();
            detail.path = relative(diff, piece.id);
            self.items.push(detail);
            self.item_ids.push(piece.id);
        } else {
            self.items_truncated = true;
        }
    }

    /// Takes a finished batch and merges every batch that is now next in line.
    fn submit(&mut self, diff: &DiffTree, batch: usize, pieces: Vec<Piece>) {
        self.pending.insert(batch, pieces);
        while let Some(ready) = self.pending.remove(&self.next) {
            for piece in ready {
                self.add(diff, piece);
            }
            self.next += 1;
        }
    }
}

/// Reads the permissions of every matched pair of `diff` and compares them. `left` and `right`
/// are the folders that were compared; `read` gives the descriptor of a path (the system call in
/// the app); `names` resolves SIDs. `Err(Cancelled)` when cancelled.
#[allow(clippy::too_many_arguments)]
pub fn compare_permissions(
    diff: &DiffTree,
    left: &Path,
    right: &Path,
    options: &CompareOptions,
    read: &Read,
    names: &SidNames,
    cancel: &CancelToken,
    counters: &Counters,
) -> Result<PermissionsResult, ScanError> {
    compare_permissions_with_limit(
        diff,
        left,
        right,
        options,
        read,
        names,
        cancel,
        counters,
        DETAIL_LIMIT,
    )
}

/// [`compare_permissions`] keeping the details of at most `entry_limit` access entries' worth of
/// items. The items kept are the first ones in tree order, however the threads happen to run.
#[allow(clippy::too_many_arguments)]
pub fn compare_permissions_with_limit(
    diff: &DiffTree,
    left: &Path,
    right: &Path,
    options: &CompareOptions,
    read: &Read,
    names: &SidNames,
    cancel: &CancelToken,
    counters: &Counters,
    entry_limit: usize,
) -> Result<PermissionsResult, ScanError> {
    let (targets, skipped) = targets(diff);
    counters.done.store(0, Ordering::Relaxed);
    counters
        .total
        .store(targets.len() as u64, Ordering::Relaxed);
    let context = Context {
        names,
        source_server: options.source_server.as_deref(),
        destination_server: options.destination_server.as_deref(),
    };
    let next = AtomicUsize::new(0);
    let merge = Mutex::new(Merge {
        next: 0,
        pending: BTreeMap::new(),
        statuses: vec![None; diff.len()],
        counts: HashMap::new(),
        items: Vec::new(),
        item_ids: Vec::new(),
        items_truncated: false,
        entries_kept: 0,
        entry_limit,
        folders_checked: 0,
        files_checked: 0,
        changed: 0,
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
                    let mut changed = false;
                    let outcome = match (
                        side_path(diff, left, target.id, Side::Left),
                        side_path(diff, right, target.id, Side::Right),
                    ) {
                        (Some(source), Some(destination)) => {
                            let source = read(&exact_path(&source), What::OwnerAndDacl);
                            let destination = read(&exact_path(&destination), What::OwnerAndDacl);
                            // The read checks the item again right before it opens it.
                            changed = changed_since_scan(&source, &destination);
                            if changed {
                                None
                            } else {
                                classify(source, destination, target.dir, target.id == 0, &context)
                            }
                        }
                        _ => None,
                    };
                    counters.done.fetch_add(1, Ordering::Relaxed);
                    pieces.push(Piece {
                        id: target.id,
                        dir: target.dir,
                        outcome,
                        changed,
                    });
                }
                lock().submit(diff, batch, pieces);
                turn.notify_all();
            });
        }
    });
    if cancel.is_cancelled() {
        return Err(ScanError::Cancelled);
    }
    let merged = merge.into_inner().unwrap_or_else(PoisonError::into_inner);

    // A node's count covers everything below it: children always come after their parent, so one
    // pass from the last node back to the root sees every child before its parent.
    let mut below = vec![0u32; diff.len()];
    for index in (1..diff.len()).rev() {
        let differs = merged.statuses[index].is_some_and(PermStatus::is_difference);
        if let Some(parent) = diff.parent(index as NodeId) {
            below[parent as usize] = below[parent as usize]
                .saturating_add(below[index])
                .saturating_add(u32::from(differs));
        }
    }

    let counts = PermStatus::DIFFERENCES
        .into_iter()
        .filter_map(|status| {
            merged
                .counts
                .get(&status)
                .map(|&count| StatusCount { status, count })
        })
        .collect();
    Ok(PermissionsResult {
        statuses: merged.statuses,
        below,
        item_ids: merged.item_ids,
        report: PermissionsCompare {
            status: AuditStatus::Checked,
            elevated: false,
            source_server: String::new(),
            destination_server: String::new(),
            server_note: String::new(),
            folders_checked: merged.folders_checked,
            files_checked: merged.files_checked,
            skipped: skipped + merged.changed,
            counts,
            items: merged.items,
            items_truncated: merged.items_truncated,
        },
    })
}

/// The system read, for the app.
pub fn system_read(path: &Path, what: What) -> io::Result<Descriptor> {
    scan_core::security::read_unless_changed(path, what)
}

#[cfg(test)]
mod tests;
