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

use scan_core::security::{Ace, Descriptor};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

#[cfg(test)]
mod tests;
