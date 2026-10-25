use super::*;
use scan_core::security::{AppliesTo, MODIFY, READ_AND_EXECUTE};
use scan_core::{scan, ScanState};
use std::fs;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

const EVERYONE: &str = "S-1-1-0";
const ADMINS: &str = "S-1-5-32-544";
const GONE: &str = "S-1-5-21-1-2-3-1001";
const DOMAIN_USERS: &str = "S-1-5-21-1-2-3-513";

fn ace(sid: &str, mask: u32, inherited: bool) -> Ace {
    Ace {
        sid: sid.to_owned(),
        kind: AceKind::Allow,
        mask,
        inherited,
        flags: if inherited { 0x10 } else { 0 },
        applies_to: AppliesTo::from_flags(0x3),
        ..Ace::default()
    }
}

fn descriptor(aces: Vec<Ace>) -> Descriptor {
    Descriptor {
        owner: Some(ADMINS.to_owned()),
        aces,
        sddl: Some("O:BAD:".to_owned()),
        ..Descriptor::default()
    }
}

fn names() -> SidNames {
    SidNames::with_resolver(|sid, _| {
        if sid == GONE {
            Account::NotFound
        } else {
            Account::Found {
                name: format!("name-{sid}"),
                domain: "D".to_owned(),
            }
        }
    })
}

/// root/{open,locked,orphan}/, root/open/a.txt, root/open/b.txt
fn tree() -> (tempfile::TempDir, Tree) {
    let temp = tempfile::tempdir().unwrap();
    for dir in ["open", "locked", "orphan"] {
        fs::create_dir(temp.path().join(dir)).unwrap();
    }
    fs::write(temp.path().join("open").join("a.txt"), b"x").unwrap();
    fs::write(temp.path().join("open").join("b.txt"), b"x").unwrap();
    let tree = scan(temp.path(), &ScanState::new(CancelToken::new())).unwrap();
    (temp, tree)
}

type Table = HashMap<&'static str, io::Result<Descriptor>>;

fn run(tree: &Tree, every_file: bool, table: &Table, names: &SidNames) -> Audit {
    let read = |path: &Path, _: What| -> io::Result<Descriptor> {
        let name = path.file_name().unwrap().to_str().unwrap();
        match table.get(name) {
            Some(Ok(descriptor)) => Ok(descriptor.clone()),
            Some(Err(err)) => Err(io::Error::new(err.kind(), err.to_string())),
            None => Ok(descriptor(vec![ace(ADMINS, security::FULL_CONTROL, true)])),
        }
    };
    audit(
        tree,
        &AuditOptions {
            every_file,
            server: None,
        },
        &read,
        names,
        &CancelToken::new(),
        &Counters::default(),
    )
    .unwrap()
}

fn kinds(audit: &Audit, path: &str) -> Vec<FindingKind> {
    audit
        .findings
        .iter()
        .filter(|finding| finding.path == path)
        .map(|finding| finding.kind)
        .collect()
}

#[test]
fn a_path_on_a_share_names_its_server() {
    assert_eq!(server_of(r"\\nas\finance\x").as_deref(), Some("nas"));
    assert_eq!(server_of(r"\\?\UNC\nas\finance").as_deref(), Some("nas"));
    assert_eq!(server_of(r"\\?\C:\data"), None);
    assert_eq!(server_of(r"C:\data"), None);
    assert_eq!(server_of(r"\\"), None);
}

#[test]
fn each_finding_type_is_found_on_its_folder() {
    let (_temp, tree) = tree();
    let mut table = Table::new();
    table.insert(
        "locked",
        Ok(Descriptor {
            protected: true,
            ..descriptor(vec![ace(ADMINS, security::FULL_CONTROL, false)])
        }),
    );
    table.insert(
        "orphan",
        Ok(descriptor(vec![
            ace(GONE, MODIFY, false),
            ace(GONE, READ_AND_EXECUTE, false),
        ])),
    );
    table.insert("open", Ok(descriptor(vec![ace(EVERYONE, MODIFY, false)])));
    let found = run(&tree, false, &table, &names());
    assert_eq!(kinds(&found, "locked"), [FindingKind::BrokenInheritance]);
    // One finding per SID, however many entries name it.
    assert_eq!(kinds(&found, "orphan"), [FindingKind::UnknownAccount]);
    assert_eq!(kinds(&found, "open"), [FindingKind::WideWrite]);
    let wide = found.findings.iter().find(|f| f.path == "open").unwrap();
    assert_eq!(wide.sid, EVERYONE);
    assert!(
        wide.detail.starts_with("Everyone: Modify"),
        "{}",
        wide.detail
    );
    assert_eq!(found.folders_read, 4);
    assert_eq!(found.files_read, 0);
}

#[test]
fn read_only_access_for_everyone_is_not_flagged() {
    let (_temp, tree) = tree();
    let mut table = Table::new();
    table.insert(
        "open",
        Ok(descriptor(vec![
            ace(EVERYONE, READ_AND_EXECUTE, false),
            ace(DOMAIN_USERS, READ_AND_EXECUTE, false),
        ])),
    );
    assert!(run(&tree, false, &table, &names()).findings.is_empty());
}

#[test]
fn every_risky_principal_with_write_is_flagged() {
    let (_temp, tree) = tree();
    let mut table = Table::new();
    table.insert(
        "open",
        Ok(descriptor(vec![
            ace(DOMAIN_USERS, security::FILE_WRITE_DATA, false),
            ace("S-1-5-11", security::GENERIC_ALL, false),
            ace("S-1-5-32-545", security::DELETE, false),
            ace("S-1-5-7", security::WRITE_DAC, false),
        ])),
    );
    let found = run(&tree, false, &table, &names());
    assert_eq!(kinds(&found, "open"), [FindingKind::WideWrite; 4]);
}

#[test]
fn a_deny_entry_for_everyone_is_not_a_grant() {
    let (_temp, tree) = tree();
    let mut table = Table::new();
    let mut deny = ace(EVERYONE, MODIFY, false);
    deny.kind = AceKind::Deny;
    table.insert("open", Ok(descriptor(vec![deny])));
    assert!(run(&tree, false, &table, &names()).findings.is_empty());
}

#[test]
fn a_missing_access_list_counts_as_wide_write() {
    let (_temp, tree) = tree();
    let mut table = Table::new();
    table.insert(
        "open",
        Ok(Descriptor {
            null_dacl: true,
            ..descriptor(Vec::new())
        }),
    );
    let found = run(&tree, false, &table, &names());
    assert_eq!(kinds(&found, "open"), [FindingKind::WideWrite]);
}

#[test]
fn an_owner_that_no_longer_exists_is_an_unknown_account() {
    let (_temp, tree) = tree();
    let mut table = Table::new();
    table.insert(
        "orphan",
        Ok(Descriptor {
            owner: Some(GONE.to_owned()),
            ..descriptor(Vec::new())
        }),
    );
    let found = run(&tree, false, &table, &names());
    let finding = found.findings.iter().find(|f| f.path == "orphan").unwrap();
    assert_eq!(finding.kind, FindingKind::UnknownAccount);
    assert_eq!(finding.sid, GONE);
    assert!(finding.detail.starts_with("Owner "));
}

#[test]
fn inherited_entries_are_reported_once_on_the_folder_that_set_them() {
    let (_temp, tree) = tree();
    let mut table = Table::new();
    table.insert("open", Ok(descriptor(vec![ace(GONE, MODIFY, true)])));
    // The source folder has no parent in the check, so it reports what it inherited.
    // The audit reads the `\\?\` form on Windows, so compare the folder's name.
    let root = tree.path(0).file_name().unwrap().to_owned();
    let read = |path: &Path, _: What| {
        if path.file_name() == Some(root.as_os_str()) {
            return Ok(descriptor(vec![ace(EVERYONE, MODIFY, true)]));
        }
        match table.get(path.file_name().unwrap().to_str().unwrap()) {
            Some(found) => Ok(found.as_ref().unwrap().clone()),
            None => Err(io::Error::other("none")),
        }
    };
    let found = audit(
        &tree,
        &AuditOptions::default(),
        &read,
        &names(),
        &CancelToken::new(),
        &Counters::default(),
    )
    .unwrap();
    assert_eq!(kinds(&found, ""), [FindingKind::WideWrite]);
    assert!(kinds(&found, "open").is_empty());
    // Its error is a finding, not a failure of the audit.
    assert_eq!(kinds(&found, "locked"), [FindingKind::Unreadable]);
}

#[test]
fn the_source_folder_alone_may_be_protected() {
    let (_temp, tree) = tree();
    let read = |_: &Path, _: What| {
        Ok(Descriptor {
            protected: true,
            ..descriptor(Vec::new())
        })
    };
    let found = audit(
        &tree,
        &AuditOptions::default(),
        &read,
        &names(),
        &CancelToken::new(),
        &Counters::default(),
    )
    .unwrap();
    assert!(kinds(&found, "").is_empty());
    assert_eq!(kinds(&found, "open"), [FindingKind::BrokenInheritance]);
}

#[test]
fn files_are_read_only_when_every_file_is_on() {
    let (_temp, tree) = tree();
    let mut table = Table::new();
    table.insert("a.txt", Ok(descriptor(vec![ace(ADMINS, MODIFY, false)])));
    let reads = AtomicUsize::new(0);
    let read = |path: &Path, what: What| {
        assert_eq!(what, What::OwnerAndDacl);
        reads.fetch_add(1, Ordering::SeqCst);
        match table.get(path.file_name().unwrap().to_str().unwrap()) {
            Some(Ok(d)) => Ok(d.clone()),
            _ => Ok(descriptor(vec![ace(ADMINS, MODIFY, true)])),
        }
    };
    let run = |every_file| {
        reads.store(0, Ordering::SeqCst);
        let found = audit(
            &tree,
            &AuditOptions {
                every_file,
                server: None,
            },
            &read,
            &names(),
            &CancelToken::new(),
            &Counters::default(),
        )
        .unwrap();
        (found, reads.load(Ordering::SeqCst))
    };
    let (folders, count) = run(false);
    assert_eq!((count, folders.files_read), (4, 0));
    assert!(folders.findings.is_empty());

    let (all, count) = run(true);
    assert_eq!((count, all.files_read), (6, 2));
    assert_eq!(kinds(&all, r"open\a.txt"), [FindingKind::ExplicitFile]);
    assert!(kinds(&all, r"open\b.txt").is_empty());
    // Only the file with entries of its own gets CSV rows.
    let file_rows: Vec<&str> = all
        .items
        .iter()
        .filter(|item| !item.dir)
        .map(|item| item.path.as_str())
        .collect();
    assert_eq!(file_rows, [r"open\a.txt"]);
}

#[cfg(unix)]
#[test]
fn links_are_never_read() {
    let (temp, _) = tree();
    std::os::unix::fs::symlink(temp.path().join("open"), temp.path().join("link")).unwrap();
    let tree = scan(temp.path(), &ScanState::new(CancelToken::new())).unwrap();
    let seen = Mutex::new(Vec::new());
    let read = |path: &Path, _: What| {
        seen.lock().unwrap().push(path.to_path_buf());
        Ok(descriptor(Vec::new()))
    };
    let found = audit(
        &tree,
        &AuditOptions {
            every_file: true,
            server: None,
        },
        &read,
        &names(),
        &CancelToken::new(),
        &Counters::default(),
    )
    .unwrap();
    assert!(seen.lock().unwrap().iter().all(|p| !p.ends_with("link")));
    assert_eq!(found.skipped, 1);
    // A link to a folder adds no rows below it.
    assert!(found
        .items
        .iter()
        .all(|item| !item.path.starts_with("link")));
}

#[test]
fn accounts_are_asked_of_the_server_once_per_sid() {
    let (_temp, tree) = tree();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&asked);
    let names = SidNames::with_resolver(move |sid, server| {
        log.lock()
            .unwrap()
            .push((sid.to_owned(), server.map(str::to_owned)));
        Account::Found {
            name: "x".into(),
            domain: String::new(),
        }
    });
    let read = |_: &Path, _: What| Ok(descriptor(vec![ace(EVERYONE, MODIFY, false)]));
    audit(
        &tree,
        &AuditOptions {
            every_file: false,
            server: Some("NAS".into()),
        },
        &read,
        &names,
        &CancelToken::new(),
        &Counters::default(),
    )
    .unwrap();
    let mut asked = asked.lock().unwrap().clone();
    asked.sort();
    let mut expected = vec![
        (ADMINS.to_owned(), Some("NAS".to_owned())),
        (EVERYONE.to_owned(), Some("NAS".to_owned())),
    ];
    expected.sort();
    assert_eq!(asked, expected);
}

#[test]
fn a_cancelled_audit_gives_nothing() {
    let (_temp, tree) = tree();
    let cancel = CancelToken::new();
    cancel.cancel();
    let read = |_: &Path, _: What| Ok(descriptor(Vec::new()));
    let outcome = audit(
        &tree,
        &AuditOptions::default(),
        &read,
        &names(),
        &cancel,
        &Counters::default(),
    );
    assert!(matches!(outcome, Err(ScanError::Cancelled)));
}

#[test]
fn the_report_counts_everything_and_caps_the_lists() {
    let findings: Vec<Finding> = (0..FINDING_LIMIT + 5)
        .map(|n| Finding {
            path: format!("d{n:06}"),
            dir: true,
            kind: FindingKind::WideWrite,
            ..Finding::default()
        })
        .collect();
    let item = |n: usize| AuditedItem {
        path: format!("d{n}"),
        dir: true,
        entries: vec![EntryRow::default(); 40_000],
        ..AuditedItem::default()
    };
    let audit = Audit {
        findings,
        items: (0..3).map(item).collect(),
        folders_read: 3,
        files_read: 0,
        skipped: 0,
    };
    let report = audit.into_report(&AuditOptions::default(), true, true);
    assert_eq!(
        report.count_of(FindingKind::WideWrite),
        FINDING_LIMIT as u64 + 5
    );
    assert_eq!(report.listed(FindingKind::WideWrite).count(), FINDING_LIMIT);
    assert_eq!(report.issues, FINDING_LIMIT as u64 + 5);
    assert_eq!(report.items.len(), 2);
    assert!(report.items_truncated);
    assert!(report.elevated);
    assert!(report.summary().findings.is_empty() && report.summary().items.is_empty());
}

#[test]
fn inheritance_and_file_entries_are_not_issues_but_the_rest_are() {
    assert!(!FindingKind::BrokenInheritance.is_issue());
    assert!(!FindingKind::ExplicitFile.is_issue());
    for kind in [
        FindingKind::UnknownAccount,
        FindingKind::WideWrite,
        FindingKind::Unreadable,
    ] {
        assert!(kind.is_issue());
    }
}

#[test]
fn an_old_report_without_permissions_loads_as_not_checked() {
    let report: PermissionsReport = serde_json::from_str("{}").unwrap();
    assert_eq!(report.status, AuditStatus::Off);
    let newer: Finding = serde_json::from_str(r#"{"kind":"somethingNew","path":"a"}"#).unwrap();
    assert_eq!(newer.kind, FindingKind::Other);
}

#[cfg(not(windows))]
#[test]
fn nothing_is_read_off_windows() {
    let temp = tempfile::tempdir().unwrap();
    let result = crate::readiness::run(
        temp.path(),
        &crate::Options::default(),
        &crate::ReadinessState::new(CancelToken::new()),
    )
    .unwrap();
    let report = result.report();
    assert_eq!(report.permissions.status, AuditStatus::NotSupported);
    assert!(!report.has_problems());
}

/// Real descriptors from a temp tree (verified by the Windows CI job only).
#[cfg(windows)]
#[test]
fn real_folders_are_read_with_their_owner_and_entries() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("sub")).unwrap();
    fs::write(temp.path().join("sub").join("f.txt"), b"x").unwrap();
    let result = crate::readiness::run(
        temp.path(),
        &crate::Options {
            every_file: true,
            ..crate::Options::default()
        },
        &crate::ReadinessState::new(CancelToken::new()),
    )
    .unwrap();
    let permissions = &result.report().permissions;
    assert_eq!(permissions.status, AuditStatus::Checked);
    assert_eq!(permissions.folders_read, 2);
    assert_eq!(permissions.files_read, 1);
    let sub = permissions.items.iter().find(|i| i.path == "sub").unwrap();
    assert!(!sub.owner_sid.is_empty() && !sub.owner.is_empty());
    assert!(!sub.entries.is_empty());
    assert!(sub.sddl.starts_with("O:"));
}

/// `icacls` on a temp tree: a protected folder, Everyone with Modify, and a file with an entry of its own (verified by the Windows CI job only).
#[cfg(windows)]
#[test]
fn icacls_made_findings_are_found_in_real_descriptors() {
    use std::process::Command;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for dir in ["locked", "wide", "plain"] {
        fs::create_dir(root.join(dir)).unwrap();
    }
    fs::write(root.join("plain").join("own.txt"), b"x").unwrap();
    let icacls = |path: &Path, args: &[&str]| {
        let status = Command::new("icacls")
            .arg(path)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "icacls {args:?}");
    };
    icacls(&root.join("locked"), &["/inheritance:d"]);
    icacls(
        &root.join("orphan"),
        &["/grant", "*S-1-5-21-1-2-3-1001:(OI)(CI)R"],
    );
    icacls(&root.join("wide"), &["/grant", "*S-1-1-0:(OI)(CI)M"]);
    icacls(
        &root.join("plain").join("own.txt"),
        &["/grant", "*S-1-1-0:R"],
    );

    let result = crate::readiness::run(
        root,
        &crate::Options {
            every_file: true,
            ..crate::Options::default()
        },
        &crate::ReadinessState::new(CancelToken::new()),
    )
    .unwrap();
    let report = &result.report().permissions;
    let has = |kind, path: &str| report.listed(kind).any(|f| f.path == path);
    assert!(has(FindingKind::BrokenInheritance, "locked"));
    assert!(has(FindingKind::WideWrite, "wide"));
    assert!(has(FindingKind::ExplicitFile, r"plain\own.txt"));
    assert!(!has(FindingKind::BrokenInheritance, "plain"));
}
