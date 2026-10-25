use super::*;
use scan_core::security::{AceKind, AppliesTo, MODIFY, READ_AND_EXECUTE};

const ADMINS: &str = "S-1-5-32-544";
const EVERYONE: &str = "S-1-1-0";
/// A local account on the old server and its recreation on the new one.
const OLD_ALICE: &str = "S-1-5-21-1-1-1-1001";
const NEW_ALICE: &str = "S-1-5-21-2-2-2-1001";
const THIRD_ALICE: &str = "S-1-5-21-3-3-3-1001";
const BOB: &str = "S-1-5-21-1-1-1-1002";

fn ace(sid: &str, kind: AceKind, mask: u32, inherited: bool) -> Ace {
    let flags = 0x3 | if inherited { 0x10 } else { 0 };
    Ace {
        sid: sid.to_owned(),
        kind,
        ace_type: u8::from(kind == AceKind::Deny),
        mask,
        flags,
        inherited,
        applies_to: AppliesTo::from_flags(flags),
    }
}

fn allow(sid: &str, mask: u32) -> Ace {
    ace(sid, AceKind::Allow, mask, false)
}

fn descriptor(owner: &str, aces: Vec<Ace>) -> Descriptor {
    Descriptor {
        owner: Some(owner.to_owned()),
        aces,
        ..Descriptor::default()
    }
}

/// Alice's two SIDs name the same account; nothing else does.
fn same_account(a: &str, b: &str) -> bool {
    [a, b]
        .iter()
        .all(|sid| [OLD_ALICE, NEW_ALICE, THIRD_ALICE].contains(sid))
}

fn diff(source: &Descriptor, destination: &Descriptor, root: bool) -> AclDiff {
    diff_descriptors(source, destination, root, &same_account)
}

#[test]
fn equal_descriptors_are_the_same() {
    let side = descriptor(ADMINS, vec![allow(EVERYONE, READ_AND_EXECUTE)]);
    let result = diff(&side, &side.clone(), false);
    assert_eq!(result, AclDiff::default());
    assert_eq!(result.status(), PermStatus::Same);
}

#[test]
fn a_different_owner_is_owner_differs() {
    let source = descriptor(ADMINS, vec![]);
    let destination = descriptor(BOB, vec![]);
    let result = diff(&source, &destination, false);
    assert_eq!(result.status(), PermStatus::OwnerDiffers);
    let owner = result.owner.unwrap();
    assert_eq!(owner.source.as_deref(), Some(ADMINS));
    assert_eq!(owner.destination.as_deref(), Some(BOB));
    assert!(!owner.same_name);
}

#[test]
fn a_missing_owner_on_one_side_is_owner_differs() {
    let source = descriptor(ADMINS, vec![]);
    let destination = Descriptor::default();
    assert_eq!(
        diff(&source, &destination, false).status(),
        PermStatus::OwnerDiffers
    );
}

#[test]
fn owner_sids_compare_without_regard_to_case() {
    let source = descriptor("s-1-5-32-544", vec![]);
    let destination = descriptor("S-1-5-32-544", vec![]);
    assert_eq!(
        diff(&source, &destination, false).status(),
        PermStatus::Same
    );
}

#[test]
fn an_entry_missing_on_the_destination_is_listed() {
    let kept = allow(EVERYONE, READ_AND_EXECUTE);
    let lost = allow(BOB, MODIFY);
    let source = descriptor(ADMINS, vec![kept.clone(), lost.clone()]);
    let destination = descriptor(ADMINS, vec![kept]);
    let result = diff(&source, &destination, false);
    assert_eq!(result.status(), PermStatus::EntriesDiffer);
    assert_eq!(result.missing, vec![lost]);
    assert!(result.extra.is_empty());
}

#[test]
fn an_extra_deny_entry_on_the_destination_is_listed() {
    let shared = allow(EVERYONE, READ_AND_EXECUTE);
    let deny = ace(BOB, AceKind::Deny, MODIFY, false);
    let source = descriptor(ADMINS, vec![shared.clone()]);
    let destination = descriptor(ADMINS, vec![shared, deny.clone()]);
    let result = diff(&source, &destination, false);
    assert_eq!(result.status(), PermStatus::EntriesDiffer);
    assert_eq!(result.extra, vec![deny]);
    assert!(result.missing.is_empty());
}

#[test]
fn the_same_account_with_a_different_mask_is_a_missing_and_an_extra_entry() {
    let source = descriptor(ADMINS, vec![allow(BOB, MODIFY)]);
    let destination = descriptor(ADMINS, vec![allow(BOB, READ_AND_EXECUTE)]);
    let result = diff(&source, &destination, false);
    assert_eq!(result.status(), PermStatus::EntriesDiffer);
    assert_eq!(result.missing, vec![allow(BOB, MODIFY)]);
    assert_eq!(result.extra, vec![allow(BOB, READ_AND_EXECUTE)]);
}

#[test]
fn allow_and_deny_with_the_same_mask_are_different_entries() {
    let source = descriptor(ADMINS, vec![allow(BOB, MODIFY)]);
    let destination = descriptor(ADMINS, vec![ace(BOB, AceKind::Deny, MODIFY, false)]);
    assert_eq!(
        diff(&source, &destination, false).status(),
        PermStatus::EntriesDiffer
    );
}

#[test]
fn different_inheritance_flags_are_different_entries() {
    let mut narrow = allow(BOB, MODIFY);
    narrow.flags = 0x0;
    narrow.applies_to = AppliesTo::from_flags(0);
    let source = descriptor(ADMINS, vec![allow(BOB, MODIFY)]);
    let destination = descriptor(ADMINS, vec![narrow]);
    assert_eq!(
        diff(&source, &destination, false).status(),
        PermStatus::EntriesDiffer
    );
}

#[test]
fn protected_on_one_side_only_is_inheritance_differs() {
    let source = descriptor(ADMINS, vec![allow(EVERYONE, READ_AND_EXECUTE)]);
    let mut destination = source.clone();
    destination.protected = true;
    let result = diff(&source, &destination, false);
    assert_eq!(result.status(), PermStatus::InheritanceDiffers);
    assert_eq!(result.protected, Some((false, true)));
}

#[test]
fn the_same_name_with_a_different_sid_is_amber_not_a_mismatch() {
    let source = descriptor(OLD_ALICE, vec![allow(OLD_ALICE, MODIFY)]);
    let destination = descriptor(NEW_ALICE, vec![allow(NEW_ALICE, MODIFY)]);
    let result = diff(&source, &destination, false);
    assert_eq!(result.status(), PermStatus::SameNameDifferentAccount);
    assert!(result.missing.is_empty() && result.extra.is_empty());
    assert_eq!(
        result.renamed,
        vec![(allow(OLD_ALICE, MODIFY), allow(NEW_ALICE, MODIFY))]
    );
    assert!(result.owner.unwrap().same_name);
}

#[test]
fn a_same_name_entry_with_another_mask_is_still_a_mismatch() {
    let source = descriptor(ADMINS, vec![allow(OLD_ALICE, MODIFY)]);
    let destination = descriptor(ADMINS, vec![allow(NEW_ALICE, READ_AND_EXECUTE)]);
    let result = diff(&source, &destination, false);
    assert_eq!(result.status(), PermStatus::EntriesDiffer);
    assert!(result.renamed.is_empty());
}

#[test]
fn a_real_difference_outranks_a_same_name_one() {
    let source = descriptor(
        OLD_ALICE,
        vec![allow(OLD_ALICE, MODIFY), allow(BOB, MODIFY)],
    );
    let destination = descriptor(NEW_ALICE, vec![allow(NEW_ALICE, MODIFY)]);
    let result = diff(&source, &destination, false);
    assert_eq!(result.status(), PermStatus::EntriesDiffer);
    // The detail still shows the amber pair next to the real loss.
    assert_eq!(result.renamed.len(), 1);
    assert_eq!(result.missing, vec![allow(BOB, MODIFY)]);
}

#[test]
fn a_wrong_owner_outranks_a_same_name_entry() {
    let source = descriptor(ADMINS, vec![allow(OLD_ALICE, MODIFY)]);
    let destination = descriptor(BOB, vec![allow(NEW_ALICE, MODIFY)]);
    assert_eq!(
        diff(&source, &destination, false).status(),
        PermStatus::OwnerDiffers
    );
}

#[test]
fn one_destination_entry_pairs_with_only_one_source_entry() {
    // Both source entries name Alice, as does the lone destination entry; only one pairs.
    let source = descriptor(
        ADMINS,
        vec![allow(OLD_ALICE, MODIFY), allow(NEW_ALICE, MODIFY)],
    );
    let destination = descriptor(ADMINS, vec![allow(THIRD_ALICE, MODIFY)]);
    let result = diff(&source, &destination, false);
    assert_eq!(result.renamed.len(), 1);
    assert_eq!(result.missing.len(), 1);
    assert!(result.extra.is_empty());
}

#[test]
fn inherited_entries_below_the_root_are_ignored() {
    let explicit = allow(BOB, MODIFY);
    let source = descriptor(
        ADMINS,
        vec![
            explicit.clone(),
            ace(EVERYONE, AceKind::Allow, READ_AND_EXECUTE, true),
        ],
    );
    let destination = descriptor(
        ADMINS,
        vec![explicit, ace(BOB, AceKind::Allow, MODIFY, true)],
    );
    assert_eq!(
        diff(&source, &destination, false).status(),
        PermStatus::Same
    );
}

#[test]
fn inherited_entries_count_at_the_root() {
    let source = descriptor(
        ADMINS,
        vec![ace(EVERYONE, AceKind::Allow, READ_AND_EXECUTE, true)],
    );
    let destination = descriptor(ADMINS, vec![]);
    let result = diff(&source, &destination, true);
    assert_eq!(result.status(), PermStatus::EntriesDiffer);
    assert_eq!(result.missing.len(), 1);
    assert_eq!(
        diff(&source, &destination, false).status(),
        PermStatus::Same
    );
}

#[test]
fn the_order_and_repeats_of_entries_do_not_matter() {
    let a = allow(BOB, MODIFY);
    let b = allow(EVERYONE, READ_AND_EXECUTE);
    let source = descriptor(ADMINS, vec![a.clone(), b.clone()]);
    let destination = descriptor(ADMINS, vec![b.clone(), a.clone(), b]);
    assert_eq!(
        diff(&source, &destination, false).status(),
        PermStatus::Same
    );
}

#[test]
fn sids_compare_without_regard_to_case() {
    let source = descriptor(ADMINS, vec![allow("s-1-1-0", READ_AND_EXECUTE)]);
    let destination = descriptor(ADMINS, vec![allow("S-1-1-0", READ_AND_EXECUTE)]);
    assert_eq!(
        diff(&source, &destination, false).status(),
        PermStatus::Same
    );
}

#[test]
fn a_null_dacl_on_one_side_is_an_entries_difference() {
    let source = Descriptor {
        null_dacl: true,
        ..descriptor(ADMINS, vec![])
    };
    let destination = descriptor(ADMINS, vec![allow(EVERYONE, READ_AND_EXECUTE)]);
    let result = diff(&source, &destination, false);
    assert_eq!(result.null_dacl, Some((true, false)));
    assert_eq!(result.status(), PermStatus::EntriesDiffer);
}

#[test]
fn files_with_only_inherited_entries_have_no_explicit_entries() {
    let inherited = descriptor(
        ADMINS,
        vec![ace(EVERYONE, AceKind::Allow, READ_AND_EXECUTE, true)],
    );
    assert!(!has_explicit_entries(&inherited));
    let own = descriptor(ADMINS, vec![allow(BOB, MODIFY)]);
    assert!(has_explicit_entries(&own));
}

#[test]
fn statuses_read_back_and_unknown_ones_load_as_other() {
    for status in PermStatus::DIFFERENCES {
        let text = serde_json::to_string(&status).unwrap();
        assert_eq!(serde_json::from_str::<PermStatus>(&text).unwrap(), status);
        assert!(status.is_difference());
    }
    assert_eq!(
        serde_json::from_str::<PermStatus>("\"somethingNewer\"").unwrap(),
        PermStatus::Other
    );
    assert!(!PermStatus::Same.is_difference());
}
