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

// ---- the pass over two compared trees -----------------------------------------------------

use crate::acl_audit::Counters;
use scan_core::security::Account;
use scan_core::{compare, scan, CancelToken, ScanState};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Mutex;

/// Two folders compared by size, and a fake reader for their permissions. A path with no entry
/// in `overrides` reads as `base`.
struct World {
    left: tempfile::TempDir,
    right: tempfile::TempDir,
    base: Descriptor,
    /// What a file reads as: the entries it inherited, none of its own.
    file_base: Descriptor,
    /// `(is source, path relative to the root with "/")` to what that path reads as.
    overrides: HashMap<(bool, String), Descriptor>,
    failing: HashSet<(bool, String)>,
    /// Paths that became a link or an online-only file after the scan: the read refuses them.
    swapped: HashSet<(bool, String)>,
    reads: Mutex<Vec<(bool, String)>>,
}

impl World {
    /// Both sides hold `names` as folders, each with `a.txt` and `b.txt` in it.
    fn new(names: &[&str]) -> Self {
        let world = Self {
            left: tempfile::tempdir().unwrap(),
            right: tempfile::tempdir().unwrap(),
            base: descriptor(ADMINS, vec![allow(EVERYONE, READ_AND_EXECUTE)]),
            file_base: descriptor(
                ADMINS,
                vec![ace(EVERYONE, AceKind::Allow, READ_AND_EXECUTE, true)],
            ),
            overrides: HashMap::new(),
            failing: HashSet::new(),
            swapped: HashSet::new(),
            reads: Mutex::new(Vec::new()),
        };
        for root in [world.left.path(), world.right.path()] {
            for name in names {
                fs::create_dir(root.join(name)).unwrap();
                fs::write(root.join(name).join("a.txt"), b"x").unwrap();
                fs::write(root.join(name).join("b.txt"), b"x").unwrap();
            }
        }
        world
    }

    fn set(&mut self, source: bool, path: &str, descriptor: Descriptor) {
        self.overrides.insert((source, path.to_owned()), descriptor);
    }

    fn key(&self, path: &Path) -> (bool, String) {
        // On Windows the pass opens the exact `\\?\` form of a path; the roots here are plain.
        let plain = scan_core::strip_verbatim(&path.to_string_lossy());
        let path = Path::new(&plain);
        let (source, relative) = match path.strip_prefix(self.left.path()) {
            Ok(relative) => (true, relative),
            Err(_) => (false, path.strip_prefix(self.right.path()).unwrap()),
        };
        let relative: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        (source, relative.join("/"))
    }

    fn read(&self, path: &Path, _: What) -> io::Result<Descriptor> {
        let key = self.key(path);
        self.reads.lock().unwrap().push(key.clone());
        if self.swapped.contains(&key) {
            return Err(io::Error::other(scan_core::security::ChangedSinceScan));
        }
        if self.failing.contains(&key) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Access is denied.",
            ));
        }
        let default = if key.1.ends_with(".txt") {
            &self.file_base
        } else {
            &self.base
        };
        Ok(self.overrides.get(&key).unwrap_or(default).clone())
    }

    fn run_with(
        &self,
        names: &SidNames,
        options: &CompareOptions,
        cancel: &CancelToken,
        limit: usize,
    ) -> Result<PermissionsResult, ScanError> {
        let state = ScanState::default();
        let diff = compare(
            &scan(self.left.path(), &state).unwrap(),
            &scan(self.right.path(), &state).unwrap(),
        );
        compare_permissions_with_limit(
            &diff,
            self.left.path(),
            self.right.path(),
            options,
            &|path, what| self.read(path, what),
            names,
            cancel,
            &Counters::default(),
            limit,
        )
    }

    fn run(&self) -> PermissionsResult {
        self.run_with(
            &plain_names(),
            &CompareOptions::default(),
            &CancelToken::new(),
            DETAIL_LIMIT,
        )
        .unwrap()
    }

    fn reads_of(&self, path: &str) -> usize {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, read)| read == path)
            .count()
    }

    fn id_of(&self, name: &str) -> u32 {
        let state = ScanState::default();
        let diff = compare(
            &scan(self.left.path(), &state).unwrap(),
            &scan(self.right.path(), &state).unwrap(),
        );
        (0..diff.len() as u32)
            .find(|&id| diff.node(id).unwrap().name == name)
            .unwrap()
    }
}

/// Every SID is its own account.
fn plain_names() -> SidNames {
    SidNames::with_resolver(|sid, _| Account::Found {
        name: format!("user-{sid}"),
        domain: "D".to_owned(),
    })
}

#[test]
fn a_folder_with_other_entries_is_found_and_counted_up_the_tree() {
    let mut world = World::new(&["a", "b"]);
    world.set(
        false,
        "a",
        descriptor(
            ADMINS,
            vec![allow(EVERYONE, READ_AND_EXECUTE), allow(BOB, MODIFY)],
        ),
    );
    let result = world.run();
    let report = result.report();
    assert_eq!(report.status, AuditStatus::Checked);
    assert_eq!(report.folders_checked, 3, "the root, a and b");
    assert_eq!(report.files_checked, 0, "no file has an entry of its own");
    assert_eq!(
        report.counts,
        vec![StatusCount {
            status: PermStatus::EntriesDiffer,
            count: 1
        }]
    );
    assert_eq!(report.items.len(), 1);
    let item = &report.items[0];
    assert_eq!((item.path.as_str(), item.dir), ("a", true));
    assert_eq!(item.status, PermStatus::EntriesDiffer);
    assert_eq!(item.extra.len(), 1);
    assert_eq!(item.extra[0].sid, BOB);

    let a = world.id_of("a");
    assert_eq!(result.status(a), Some(PermStatus::EntriesDiffer));
    assert_eq!(result.status(world.id_of("b")), Some(PermStatus::Same));
    assert_eq!(result.status(0), Some(PermStatus::Same));
    assert_eq!(
        result.cell(0).unwrap().below,
        1,
        "one item below the root differs"
    );
    assert_eq!(result.cell(a).unwrap().below, 0);
    assert!(result
        .cell(a)
        .unwrap()
        .summary
        .contains("1 only at the destination"));
    assert_eq!(result.cell(world.id_of("b")).unwrap().below, 0);
    assert_eq!(result.cell(999), None);
    assert_eq!(report.differences(), 1);
    assert_eq!(report.mismatches(), 1);
}

#[test]
fn a_file_pair_is_compared_only_when_either_side_has_an_entry_of_its_own() {
    let mut world = World::new(&["a"]);
    let inherited = descriptor(
        ADMINS,
        vec![ace(EVERYONE, AceKind::Allow, READ_AND_EXECUTE, true)],
    );
    let own = descriptor(ADMINS, vec![allow(BOB, MODIFY)]);
    // a.txt: the source gave it an entry of its own that the destination lacks.
    world.set(true, "a/a.txt", own);
    world.set(true, "a/b.txt", inherited.clone());
    world.set(false, "a/b.txt", inherited);
    let result = world.run();
    let report = result.report();
    assert_eq!(report.files_checked, 1, "only a.txt has entries of its own");
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].path, r"a\a.txt");
    assert!(!report.items[0].dir);
    assert_eq!(report.items[0].missing.len(), 1);
    let file = {
        let id = world.id_of("a.txt");
        result.status(id)
    };
    assert_eq!(file, Some(PermStatus::EntriesDiffer));
    assert_eq!(result.status(world.id_of("b.txt")), None, "not reported");
}

#[test]
fn an_unreadable_side_is_not_readable_and_names_the_side() {
    let mut world = World::new(&["a", "b"]);
    world.failing.insert((false, "b".to_owned()));
    let result = world.run();
    let report = result.report();
    assert_eq!(report.count_of(PermStatus::NotReadable), 1);
    let item = &report.items[0];
    assert_eq!(item.path, "b");
    assert_eq!(item.status, PermStatus::NotReadable);
    assert!(item.error.starts_with("Destination: "), "{}", item.error);
    assert!(item.error.contains("Access is denied"));
    assert!(!item.error.contains("Source:"));
}

#[test]
fn a_pair_that_became_a_link_after_the_scan_is_skipped_not_unreadable() {
    // DSA-091: the read refuses an item that is a link or online-only now.
    let mut world = World::new(&["a", "b"]);
    world.swapped.insert((true, "a".to_owned()));
    world.swapped.insert((false, "b".to_owned()));
    let result = world.run();
    let report = result.report();
    assert_eq!(report.skipped, 2, "one pair per swapped side");
    assert_eq!(report.count_of(PermStatus::NotReadable), 0);
    assert!(report.items.is_empty());
    // The root is still checked; the swapped pairs have no status.
    assert_eq!(report.folders_checked, 1);
    assert_eq!(result.status(world.id_of("a")), None);
    assert_eq!(result.status(world.id_of("b")), None);
    assert_eq!(result.status(0), Some(PermStatus::Same));
}

#[test]
fn items_on_one_side_only_are_never_read() {
    let mut world = World::new(&["a"]);
    fs::create_dir(world.left.path().join("only_left")).unwrap();
    fs::create_dir(world.right.path().join("only_right")).unwrap();
    world.set(true, "a", world.base.clone());
    let result = world.run();
    assert_eq!(result.report().folders_checked, 2, "the root and a");
    assert_eq!(world.reads_of("only_left"), 0);
    assert_eq!(world.reads_of("only_right"), 0);
    assert_eq!(world.reads_of("a"), 2, "once on each side");
}

#[test]
fn inherited_entries_are_compared_at_the_root_only() {
    let mut world = World::new(&["a"]);
    let with_inherited = descriptor(
        ADMINS,
        vec![
            allow(EVERYONE, READ_AND_EXECUTE),
            ace(BOB, AceKind::Allow, MODIFY, true),
        ],
    );
    let without = descriptor(ADMINS, vec![allow(EVERYONE, READ_AND_EXECUTE)]);
    for path in ["", "a"] {
        world.set(true, path, with_inherited.clone());
        world.set(false, path, without.clone());
    }
    let result = world.run();
    assert_eq!(result.status(0), Some(PermStatus::EntriesDiffer));
    assert_eq!(result.status(world.id_of("a")), Some(PermStatus::Same));
    assert_eq!(result.report().items[0].path, "", "the root");
    assert_eq!(result.report().items[0].missing[0].sid, BOB);
}

#[test]
fn each_side_asks_its_own_server_for_names() {
    let mut world = World::new(&["a"]);
    world.set(
        true,
        "a",
        descriptor(OLD_ALICE, vec![allow(OLD_ALICE, MODIFY)]),
    );
    world.set(
        false,
        "a",
        descriptor(NEW_ALICE, vec![allow(NEW_ALICE, MODIFY)]),
    );
    type Asked = std::sync::Arc<Mutex<Vec<(String, Option<String>)>>>;
    let asked: Asked = Asked::default();
    let log = asked.clone();
    // Alice exists as the same name on both servers, but each SID only resolves on its own.
    let names = SidNames::with_resolver(move |sid, server| {
        log.lock()
            .unwrap()
            .push((sid.to_owned(), server.map(str::to_owned)));
        match (sid, server) {
            (OLD_ALICE, Some("old")) | (NEW_ALICE, Some("new")) => Account::Found {
                name: "alice".to_owned(),
                domain: server.unwrap().to_uppercase(),
            },
            _ => Account::NotFound,
        }
    });
    let options = CompareOptions {
        source_server: Some("old".to_owned()),
        destination_server: Some("new".to_owned()),
    };
    let result = world
        .run_with(&names, &options, &CancelToken::new(), DETAIL_LIMIT)
        .unwrap();
    assert_eq!(
        result.status(world.id_of("a")),
        Some(PermStatus::SameNameDifferentAccount)
    );
    let item = &result.report().items[0];
    assert_eq!(item.same_name.len(), 1);
    assert_eq!(item.same_name[0].source.account, r"OLD\alice");
    assert_eq!(item.same_name[0].destination.account, r"NEW\alice");
    assert_eq!(item.source_owner, r"OLD\alice");
    assert_eq!(item.destination_owner, r"NEW\alice");
    assert_eq!(
        result
            .report()
            .count_of(PermStatus::SameNameDifferentAccount),
        1
    );
    assert_eq!(result.report().mismatches(), 0, "amber is not a mismatch");
    let asked = asked.lock().unwrap();
    assert!(asked.contains(&(OLD_ALICE.to_owned(), Some("old".to_owned()))));
    assert!(asked.contains(&(NEW_ALICE.to_owned(), Some("new".to_owned()))));
    assert!(
        !asked.contains(&(OLD_ALICE.to_owned(), Some("new".to_owned()))),
        "a source SID is never asked of the destination's server"
    );
}

#[test]
fn a_cancel_stops_the_pass() {
    let world = World::new(&["a"]);
    let cancel = CancelToken::new();
    cancel.cancel();
    let outcome = world.run_with(
        &plain_names(),
        &CompareOptions::default(),
        &cancel,
        DETAIL_LIMIT,
    );
    assert!(matches!(outcome, Err(ScanError::Cancelled)));
}

#[test]
fn a_small_limit_keeps_the_same_first_items_on_every_run_and_the_totals_complete() {
    // More pairs than one batch, so several threads take part.
    let names: Vec<String> = (0..300).map(|n| format!("d{n:03}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut world = World::new(&refs);
    for name in &names {
        world.set(
            false,
            name,
            descriptor(BOB, vec![allow(EVERYONE, READ_AND_EXECUTE)]),
        );
    }
    let mut kept = Vec::new();
    for _ in 0..3 {
        let result = world
            .run_with(
                &plain_names(),
                &CompareOptions::default(),
                &CancelToken::new(),
                5,
            )
            .unwrap();
        let report = result.report();
        assert_eq!(
            report.count_of(PermStatus::OwnerDiffers),
            300,
            "totals stay complete"
        );
        assert!(report.items_truncated);
        assert_eq!(report.items.len(), 5);
        // The kept details answer the table; the rest of the rows still have a status.
        assert!(result.detail(world.id_of("d000")).is_some());
        assert!(result.detail(world.id_of("d299")).is_none());
        assert_eq!(
            result.status(world.id_of("d299")),
            Some(PermStatus::OwnerDiffers)
        );
        assert_eq!(result.cell(world.id_of("d299")).unwrap().summary, "");
        assert_eq!(result.cell(0).unwrap().below, 300);
        kept.push(
            report
                .items
                .iter()
                .map(|item| item.path.clone())
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(kept[0], ["d000", "d001", "d002", "d003", "d004"]);
    assert_eq!(kept[0], kept[1]);
    assert_eq!(kept[1], kept[2]);
}

#[test]
fn the_limit_counts_entries_not_items() {
    let mut world = World::new(&["a", "b"]);
    let wide = descriptor(
        ADMINS,
        vec![
            allow(BOB, MODIFY),
            allow(OLD_ALICE, MODIFY),
            allow(NEW_ALICE, MODIFY),
        ],
    );
    world.set(true, "a", wide.clone());
    world.set(true, "b", wide);
    let result = world
        .run_with(
            &plain_names(),
            &CompareOptions::default(),
            &CancelToken::new(),
            4,
        )
        .unwrap();
    // `a` costs 3 entries; `b` would make 6, so it is dropped (and the cut is final).
    assert_eq!(result.report().items.len(), 1);
    assert!(result.report().items_truncated);
    assert_eq!(result.report().count_of(PermStatus::EntriesDiffer), 2);
}

#[cfg(unix)]
#[test]
fn links_are_never_read() {
    let world = World::new(&["a"]);
    for root in [world.left.path(), world.right.path()] {
        std::os::unix::fs::symlink(root.join("a"), root.join("link")).unwrap();
    }
    let result = world.run();
    assert_eq!(result.report().skipped, 1);
    assert_eq!(world.reads_of("link"), 0);
}

#[test]
fn a_file_paired_with_a_folder_is_skipped() {
    let world = World::new(&["a"]);
    fs::create_dir(world.left.path().join("x")).unwrap();
    fs::write(world.right.path().join("x"), b"y").unwrap();
    let result = world.run();
    assert_eq!(result.report().skipped, 1);
    assert_eq!(world.reads_of("x"), 0);
}

#[test]
fn a_compare_that_did_not_look_is_off_and_loads_from_nothing() {
    let off: PermissionsCompare = serde_json::from_str("{}").unwrap();
    assert!(off.is_off());
    assert_eq!(off, PermissionsCompare::default());
    let not_supported = PermissionsResult::not_supported();
    assert_eq!(not_supported.report().status, AuditStatus::NotSupported);
    assert!(!not_supported.report().is_off());
    assert_eq!(not_supported.cell(0), None);
    let context = not_supported.with_context(true, Some("old"), None);
    assert!(context.report().elevated);
    assert_eq!(context.report().source_server, "old");
    assert_eq!(context.report().destination_server, "");
}

#[test]
fn a_report_reads_back_the_way_it_was_saved() {
    let mut world = World::new(&["a"]);
    world.set(
        false,
        "a",
        descriptor(
            BOB,
            vec![allow(EVERYONE, READ_AND_EXECUTE), allow(BOB, MODIFY)],
        ),
    );
    let report = world.run().report().clone();
    let text = serde_json::to_string(&report).unwrap();
    assert_eq!(
        serde_json::from_str::<PermissionsCompare>(&text).unwrap(),
        report
    );
    assert!(text.contains("\"sameName\""));
}

#[test]
fn rows_without_permissions_serialize_exactly_like_size_rows() {
    let world = World::new(&["a", "b"]);
    let state = ScanState::default();
    let diff = compare(
        &scan(world.left.path(), &state).unwrap(),
        &scan(world.right.path(), &state).unwrap(),
    );
    let plain = diff.children(0).unwrap();
    let wrapped = compare_rows(&diff, 0, None).unwrap();
    assert_eq!(plain.len(), wrapped.len());
    for (plain, wrapped) in plain.iter().zip(&wrapped) {
        assert_eq!(
            serde_json::to_string(plain).unwrap(),
            serde_json::to_string(wrapped).unwrap(),
            "byte-identical with the option off"
        );
    }
    assert!(compare_rows(&diff, 9999, None).is_none());
}

#[test]
fn rows_with_permissions_add_one_cell_and_change_nothing_else() {
    let mut world = World::new(&["a"]);
    world.set(false, "a", descriptor(BOB, vec![]));
    let result = world.run();
    let state = ScanState::default();
    let diff = compare(
        &scan(world.left.path(), &state).unwrap(),
        &scan(world.right.path(), &state).unwrap(),
    );
    let wrapped = compare_rows(&diff, 0, Some(&result)).unwrap();
    let json = serde_json::to_value(&wrapped[0]).unwrap();
    let mut without = serde_json::to_value(&diff.children(0).unwrap()[0]).unwrap();
    let cell = json["permissions"].clone();
    assert_eq!(cell["status"], "entriesDiffer");
    assert_eq!(cell["below"], 0);
    without["permissions"] = cell;
    assert_eq!(json, without);
}

/// Windows only: two real trees, one folder's ACL changed at the destination after "copying",
/// read with the system calls. Verified by the Windows CI job.
#[cfg(windows)]
#[test]
fn windows_a_changed_acl_at_the_destination_is_found_with_the_real_reader() {
    let world = World::new(&["a", "b"]);
    let changed = world.right.path().join("a");
    let status = std::process::Command::new("icacls")
        .arg(&changed)
        .args(["/grant", "*S-1-1-0:(OI)(CI)R"])
        .output()
        .expect("icacls runs");
    assert!(status.status.success(), "{status:?}");

    let state = ScanState::default();
    let diff = compare(
        &scan(world.left.path(), &state).unwrap(),
        &scan(world.right.path(), &state).unwrap(),
    );
    let result = compare_permissions(
        &diff,
        world.left.path(),
        world.right.path(),
        &CompareOptions::default(),
        &system_read,
        &SidNames::default(),
        &CancelToken::new(),
        &Counters::default(),
    )
    .unwrap();

    let id_of = |name: &str| {
        (0..diff.len() as u32)
            .find(|&id| diff.node(id).unwrap().name == name)
            .unwrap()
    };
    assert_eq!(result.status(id_of("a")), Some(PermStatus::EntriesDiffer));
    assert_eq!(result.status(id_of("b")), Some(PermStatus::Same));
    let detail = result
        .detail(id_of("a"))
        .expect("the changed folder has details");
    assert!(
        detail.extra.iter().any(|entry| entry.sid == EVERYONE),
        "Everyone's read entry exists only at the destination: {detail:?}"
    );
    assert!(detail.missing.is_empty(), "{detail:?}");
    assert_ne!(
        result.status(0),
        Some(PermStatus::NotReadable),
        "the compare roots are readable"
    );
}
