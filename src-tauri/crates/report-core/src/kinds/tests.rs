use super::*;

/// A report saved by an older build lacks newer fields; every body struct must still load.
#[test]
fn every_report_body_loads_from_an_empty_object() {
    assert_eq!(
        serde_json::from_str::<CompareReport>("{}").unwrap(),
        CompareReport::default()
    );
    assert_eq!(
        serde_json::from_str::<UsageReport>("{}").unwrap(),
        UsageReport::default()
    );
    assert_eq!(
        serde_json::from_str::<CleanReport>("{}").unwrap(),
        CleanReport::default()
    );
    for body in [
        r#"{"kind":"compare","data":{}}"#,
        r#"{"kind":"diskUsage","data":{}}"#,
        r#"{"kind":"cleanup","data":{}}"#,
        r#"{"kind":"readiness","data":{}}"#,
    ] {
        serde_json::from_str::<Body>(body).unwrap();
    }
    // The report envelope keeps its identity fields required; only `job` is optional.
    let saved = r#"{"id":"x","createdAtMs":1,"machine":"PC1","user":"sam","body":{"kind":"compare","data":{}}}"#;
    assert_eq!(
        serde_json::from_str::<Saved>(saved).unwrap().job,
        JobInfo::default()
    );
}

fn saved(body: Body) -> Saved {
    Saved {
        id: "x".into(),
        created_at_ms: 0,
        machine: "PC1".into(),
        user: "sam".into(),
        job: JobInfo {
            client: "Acme".into(),
            ticket: "T-42".into(),
            technician: String::new(),
        },
        body,
    }
}

#[test]
fn compare_rows_flag_missing_files() {
    let report = CompareReport {
        source: r"C:\Data".into(),
        destination: r"E:\Data".into(),
        missing: 3,
        missing_bytes: 2048,
        missing_paths: vec![
            MissingFile {
                path: "a.txt".into(),
                size: 1024,
            },
            MissingFile {
                path: r"sub\b.txt".into(),
                size: 0,
            },
        ],
        ..CompareReport::default()
    };
    let saved = saved(Body::Compare(report));
    let row = entry(&saved);
    assert!(row.problem);
    assert_eq!(row.headline, "3 files missing (2.00 KB)");
    let doc = document(&saved);
    assert_eq!(doc.tables[0].rows.len(), 2);
    assert_eq!(doc.tables[0].rows[0][1], "1.00 KB");
    assert_eq!(doc.tables[0].row_bytes, vec![1024, 0]);
    assert!(doc.csv().contains("a.txt,1.00 KB,1024"));
    let Section::Recovery(rec) = &doc.sections[0] else {
        panic!("a recovery block")
    };
    assert!(rec.script.contains(r"robocopy 'C:\Data' 'E:\Data' 'a.txt'"));
    assert!(rec
        .script
        .contains(r"robocopy 'C:\Data\sub' 'E:\Data\sub' 'b.txt'"));
    assert!(doc.data.as_ref().unwrap()["rec"]["dirs"].is_array());
    assert_eq!(doc.tables[0].footnote, "The first 2 of 3 are listed.");
    assert_eq!(doc.tiles[2].tone, Tone::Bad);
    assert!(doc.meta.contains(&("Ticket".into(), "T-42".into())));
    assert!(doc.meta.contains(&("Windows user".into(), "sam".into())));
}

#[test]
fn compare_html_has_the_recovery_block_and_stays_inside_the_csp() {
    let report = CompareReport {
        source: r"C:\Data".into(),
        destination: r"E:\Data".into(),
        missing: 2,
        missing_paths: vec![
            MissingFile {
                path: r"app\node_modules\x.js".into(),
                size: 5,
            },
            MissingFile {
                path: r"it's\</script>.txt".into(),
                size: 1,
            },
        ],
        ..CompareReport::default()
    };
    let html = document(&saved(Body::Compare(report))).html("Acme", "now");
    for needle in [
        "Copy the missing files again",
        "Show the full command",
        "Download .ps1",
        "data-bit=\"0\"",
        "node_modules: 1 file",
        "Copy pass #2 still didn't work",
        "id=\"rec-pass2\"",
    ] {
        assert!(html.contains(needle), "missing {needle}");
    }
    assert!(!html.contains("</script>.txt"), "island must escape <");
    assert_eq!(
        html.matches("<script").count(),
        2,
        "island and page script only"
    );
}

#[test]
fn reports_saved_before_sizes_existed_still_load() {
    let old = r#"{"source":"C:\\A","destination":"D:\\B","missing":2,"missingPaths":["x.txt","d\\y.txt"]}"#;
    let report: CompareReport = serde_json::from_str(old).unwrap();
    assert_eq!(report.missing_paths.len(), 2);
    assert_eq!(report.missing_paths[1].path, r"d\y.txt");
    assert_eq!(report.missing_paths[1].size, 0);
    let new = r#"{"missingPaths":[{"path":"x.txt","size":7},{"path":"y.txt"}]}"#;
    let report: CompareReport = serde_json::from_str(new).unwrap();
    assert_eq!(report.missing_paths[0].size, 7);
    assert_eq!(report.missing_paths[1].size, 0);
    let again: CompareReport =
        serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
    assert_eq!(again, report);
}

#[test]
fn usage_lists_removed_items_first() {
    let report = UsageReport {
        path: r"C:\".into(),
        size: 1000,
        removed: vec![Removed {
            path: r"C:\old.iso".into(),
            size: 400,
            permanent: false,
        }],
        ..UsageReport::default()
    };
    let saved = saved(Body::DiskUsage(report));
    assert_eq!(
        entry(&saved).headline,
        "1000 bytes used · 400 bytes removed"
    );
    let doc = document(&saved);
    assert_eq!(doc.tables[0].heading, "Removed");
    assert_eq!(doc.tables[0].rows[0][2], "Recycle Bin");
}

#[test]
fn usage_report_with_an_explorer_gets_the_page_and_old_reports_do_not() {
    use crate::explorer::{ExplorerNode, KIND_FILE};
    let mut report = UsageReport {
        path: r"C:\".into(),
        size: 10,
        ..UsageReport::default()
    };
    // A report saved before the explorer existed: no explorer, no page, same tables.
    let old: UsageReport = serde_json::from_str(r#"{"path":"C:\\","size":10}"#).unwrap();
    assert!(old.explorer.is_none());
    let doc = document(&saved(Body::DiskUsage(old)));
    assert!(!doc.explorer && doc.data.is_none());

    // An empty scan has nothing to explore.
    report.explorer = Some(Explorer::default());
    assert!(!document(&saved(Body::DiskUsage(report.clone()))).explorer);

    report.explorer = Some(Explorer {
        nodes: vec![
            ExplorerNode {
                name: r"C:\".into(),
                disk: 10,
                ..ExplorerNode::default()
            },
            ExplorerNode {
                name: "a.mp4".into(),
                disk: 10,
                kind: KIND_FILE,
                ..ExplorerNode::default()
            },
        ],
        ..Explorer::default()
    });
    let doc = document(&saved(Body::DiskUsage(report)));
    assert!(doc.explorer);
    let data = doc.data.expect("data island");
    assert_eq!(data["x"]["root"], r"C:\");
    assert_eq!(data["x"]["nodes"][1]["n"], "a.mp4");
    // The flat tables are still there for readers without scripts.
    assert_eq!(doc.tables[0].heading, "Largest files");
}

/// The explorer page for the script tests in `src/lib/reportExplorer.test.ts`: names that
/// would break markup, folders three levels deep, a folded row and four file types.
/// `UPDATE_FIXTURES=1 cargo test -p report-core explorer_fixture` rewrites it.
#[test]
fn explorer_fixture_matches_what_the_page_renders() {
    use crate::explorer::{ExplorerNode, TypeFiles, KIND_DIR, KIND_FILE, KIND_MORE};
    let node = |name: &str, disk: u64, files: u64, kind: u8, parent: u32| ExplorerNode {
        name: name.into(),
        disk,
        files,
        kind,
        parent,
    };
    let hostile = "</script><b>x</b>.txt";
    let explorer = Explorer {
        nodes: vec![
            node(r"C:\", 1000, 10, KIND_DIR, 0),
            node("Users", 600, 6, KIND_DIR, 0),
            node("Windows", 300, 3, KIND_DIR, 0),
            node("pagefile.sys", 90, 1, KIND_FILE, 0),
            node("(2 smaller items)", 10, 2, KIND_MORE, 0),
            node("Alice", 500, 5, KIND_DIR, 1),
            node(hostile, 100, 1, KIND_FILE, 1),
            node("Movies", 400, 1, KIND_DIR, 5),
            node("notes.txt", 100, 1, KIND_FILE, 5),
            node("clip.mp4", 400, 1, KIND_FILE, 7),
            node("system.dll", 300, 1, KIND_FILE, 2),
        ],
        types: vec![
            TypeItem {
                extension: "mp4".into(),
                size: 400,
                files: 1,
            },
            TypeItem {
                extension: "dll".into(),
                size: 300,
                files: 1,
            },
            TypeItem {
                extension: "txt".into(),
                size: 200,
                files: 2,
            },
            TypeItem {
                extension: "sys".into(),
                size: 90,
                files: 1,
            },
        ],
        type_files: vec![
            TypeFiles {
                extension: "mp4".into(),
                files: vec![(r"Users\Alice\Movies\clip.mp4".into(), 400)],
            },
            TypeFiles {
                extension: "dll".into(),
                files: vec![(r"Windows\system.dll".into(), 300)],
            },
            TypeFiles {
                extension: "txt".into(),
                files: vec![
                    (format!(r"Users\{hostile}"), 100),
                    (r"Users\Alice\notes.txt".into(), 100),
                ],
            },
            TypeFiles {
                extension: "sys".into(),
                files: vec![("pagefile.sys".into(), 90)],
            },
        ],
        types_more: 0,
        scanned_items: 4000,
    };
    let report = UsageReport {
        path: r"C:\".into(),
        size: 1000,
        files: 10,
        dirs: 4,
        explorer: Some(explorer),
        ..UsageReport::default()
    };
    let html = document(&saved(Body::DiskUsage(report))).html("Fixture", "fixed date");
    // The hostile name did not open a script or element: ours, the explorer's and the island.
    assert_eq!(html.matches("<script").count(), 3);
    assert!(!html.contains("<b>x"));

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/explorer.html");
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &html).unwrap();
    }
    let committed = std::fs::read_to_string(&path)
        .expect("tests/fixtures/explorer.html (run with UPDATE_FIXTURES=1)")
        .replace("\r\n", "\n");
    // Only the explorer's own parts: the shared script may change without touching these.
    let part = |page: &str, from: &str, to: &str| -> String {
        let start = page.find(from).expect(from);
        let end = page[start..].find(to).expect(to) + start;
        page[start..end].to_string()
    };
    for (from, to) in [
        ("<script type=\"application/json\" id=\"d\">", "</script>"),
        ("<script>\n(function(){\nvar host=", "</script>"),
    ] {
        assert_eq!(
            part(&committed, from, to),
            part(&html, from, to),
            "fixture is stale: run UPDATE_FIXTURES=1 cargo test -p report-core explorer_fixture"
        );
    }
}

#[test]
fn cleanup_adds_up_what_was_freed() {
    let report = CleanReport {
        items: vec![
            CleanedItem {
                title: "Temporary files".into(),
                freed: 1024,
                removed_files: 3,
                ..CleanedItem::default()
            },
            CleanedItem {
                title: "Browser caches".into(),
                freed: 1024,
                ..CleanedItem::default()
            },
        ],
    };
    let saved = saved(Body::Cleanup(report));
    let row = entry(&saved);
    assert_eq!(row.headline, "Freed 2.00 KB");
    assert_eq!(row.subject, "Temporary files, Browser caches");
    assert_eq!(document(&saved).tables[0].rows.len(), 2);
    assert_eq!(document(&saved).verdict, Some(Verdict::Good));
    let share = document(&saved).share.expect("a bar");
    assert_eq!(share.detail, "2 of 2");
    assert_eq!(share.tone, Tone::Good);
}

#[test]
fn cleanup_bar_turns_amber_when_a_location_failed() {
    let report = CleanReport {
        items: vec![
            CleanedItem::default(),
            CleanedItem {
                error: Some("Access denied".into()),
                ..CleanedItem::default()
            },
        ],
    };
    let share = document(&saved(Body::Cleanup(report))).share.unwrap();
    assert_eq!(share.detail, "1 of 2");
    assert_eq!(share.percent, 50.0);
    assert_eq!(share.tone, Tone::Warn);
}

#[test]
fn disk_usage_bar_shows_the_share_of_the_drive_and_is_absent_without_one() {
    let mut report = UsageReport {
        path: r"C:\Users".into(),
        size: 250,
        drive_total: Some(1000),
        ..UsageReport::default()
    };
    let share = document(&saved(Body::DiskUsage(report.clone())))
        .share
        .expect("a bar");
    assert_eq!(share.percent, 25.0);
    report.drive_total = None;
    assert!(document(&saved(Body::DiskUsage(report))).share.is_none());
}

#[test]
fn every_report_kind_exports_raw_bytes_in_the_csv() {
    let compare = CompareReport {
        missing: 1,
        missing_paths: vec![MissingFile {
            path: "a".into(),
            size: 7,
        }],
        ..CompareReport::default()
    };
    let usage = UsageReport {
        largest_files: vec![FileItem {
            path: "f".into(),
            size: 9,
        }],
        ..UsageReport::default()
    };
    let clean = CleanReport {
        items: vec![CleanedItem {
            title: "Temp".into(),
            freed: 11,
            ..CleanedItem::default()
        }],
    };
    for (body, bytes) in [
        (Body::Compare(compare), "7"),
        (Body::DiskUsage(usage), "9"),
        (Body::Cleanup(clean), "11"),
    ] {
        let csv = document(&saved(body)).csv();
        let header = csv.lines().next().unwrap();
        assert!(header.contains("Bytes"), "no Bytes column in {header}");
        assert!(
            csv.lines()
                .any(|line| line.ends_with(&format!(",{bytes}"))
                    || line.contains(&format!(",{bytes},"))),
            "raw {bytes} missing in {csv}"
        );
    }
}

#[test]
fn check_label_follows_verify_level_and_hashes() {
    let mut item = ItemResult::done("a.txt", 1, ItemStatus::Copied);
    assert_eq!(
        check_label(&item, VerifyLevel::SizeAndTime),
        "Size + date match"
    );
    // Hash level but nothing hashed (e.g. a file that was already there): no claim.
    assert_eq!(check_label(&item, VerifyLevel::Hash), "Size + date match");
    item.source_hash = Some("abc".into());
    item.dest_hash = Some("abc".into());
    assert_eq!(
        check_label(&item, VerifyLevel::Hash),
        "Content verified (BLAKE3)"
    );
    // A size-and-time run never claims content was verified.
    assert_eq!(
        check_label(&item, VerifyLevel::SizeAndTime),
        "Size + date match"
    );
    item.dest_hash = Some("def".into());
    assert_eq!(check_label(&item, VerifyLevel::Hash), "Size + date match");
    let same = ItemResult::done("b.txt", 1, ItemStatus::SkippedIdentical);
    assert_eq!(check_label(&same, VerifyLevel::Hash), "Already there");
    assert_eq!(status_label(ItemStatus::SkippedIdentical), "Already there");
}

#[test]
fn record_verdict_matrix() {
    use transfer_core::{ConflictPolicy, RunTotals, TransferMode, TransferSettings, VerifyLevel};
    let run = |state: RunState, not_copied: u64, error: Option<&str>| RunSummary {
        id: "r".into(),
        settings: TransferSettings {
            source: "a".into(),
            destination: "b".into(),
            mode: TransferMode::Copy,
            verify: VerifyLevel::SizeAndTime,
            conflict: ConflictPolicy::Skip,
            ignore_junk: false,
            download_cloud: false,
            preserve: Default::default(),
        },
        state,
        created_at_ms: 0,
        started_at_ms: None,
        finished_at_ms: None,
        machine: String::new(),
        user: String::new(),
        totals: RunTotals {
            not_copied,
            ..RunTotals::default()
        },
        error: error.map(str::to_string),
        previous_run_id: None,
    };
    assert_eq!(
        record_verdict(&run(RunState::Completed, 0, None)),
        Verdict::Good
    );
    assert_eq!(
        record_verdict(&run(RunState::Watching, 0, None)),
        Verdict::Good
    );
    assert_eq!(
        record_verdict(&run(RunState::Completed, 2, None)),
        Verdict::Bad
    );
    // The bug this fixes: a failed or cancelled run with nothing listed used to read "All clear".
    assert_eq!(
        record_verdict(&run(RunState::Failed, 0, None)),
        Verdict::Bad
    );
    assert_eq!(
        record_verdict(&run(RunState::Cancelled, 0, None)),
        Verdict::Bad
    );
    assert_eq!(
        record_verdict(&run(RunState::Completed, 0, Some("disk full"))),
        Verdict::Bad
    );
}

fn readiness_report() -> ReadinessReport {
    use migrate_core::{Problem, ProblemKind, RuleCount};
    let problem = |path: &str, kind, rule: &str, blocker| Problem {
        path: path.into(),
        dir: false,
        kind,
        rule: rule.into(),
        hint: format!("fix {path}"),
        blocker,
        ..Problem::default()
    };
    let count = |kind, rule: &str, count, blocker| RuleCount {
        kind,
        rule: rule.into(),
        count,
        blocker,
    };
    ReadinessReport {
        source: r"F:\Finance".into(),
        destination_root: r"D:\Shares\Finance".into(),
        files: 10,
        dirs: 2,
        cloud_files: 3,
        name_problems: 1,
        long_paths: 3,
        blockers: 1,
        links: 1,
        problems_total: 5,
        counts: vec![
            count(ProblemKind::Name, "reserved", 1, true),
            count(ProblemKind::LongPath, "pathTooLong", 3, false),
            count(ProblemKind::Link, "link", 1, false),
        ],
        problems: vec![
            problem(r"a\CON.txt", ProblemKind::Name, "reserved", true),
            problem(r"deep\x.txt", ProblemKind::LongPath, "pathTooLong", false),
            problem("=cmd|x", ProblemKind::Link, "link", false),
        ],
        ..ReadinessReport::default()
    }
}

#[test]
fn readiness_row_and_document_group_problems_by_type() {
    let saved = saved(Body::Readiness(readiness_report()));
    let row = entry(&saved);
    assert_eq!(row.kind, ReportKind::Readiness);
    assert_eq!(row.title, "Migration readiness");
    assert_eq!(row.subject, r"F:\Finance → D:\Shares\Finance");
    assert_eq!(
        row.headline,
        "1 name problem · 3 long paths · 1 link not followed"
    );
    assert!(row.problem);

    let doc = document(&saved);
    assert_eq!(doc.verdict, Some(Verdict::Bad));
    let headings: Vec<&str> = doc.tables.iter().map(|t| t.heading.as_str()).collect();
    assert_eq!(
        headings,
        [
            "Problems by rule",
            "Names the target cannot hold",
            "Paths too long",
            "Links and junctions (not followed)"
        ]
    );
    assert_eq!(doc.tables[1].rows[0][1], "Reserved device name");
    assert_eq!(doc.tables[2].footnote, "The first 1 of 3 are listed.");
    assert!(doc
        .meta
        .contains(&("Target".into(), "File server (NTFS)".into())));
    assert!(doc.notes.iter().any(|note| note.contains("3 cloud files")));

    let csv = doc.csv();
    assert!(csv.contains("Type,Rule,Path (relative to the source),Item,Blocks the copy,Fix\r\n"));
    assert!(csv.contains(r"Name,Reserved device name,a\CON.txt,File,Yes,fix a\CON.txt"));
    // A path that looks like a formula is defused.
    assert!(!csv.contains(",=cmd"), "{csv}");
    let html = doc.html("Acme", "now");
    assert!(html.contains("Paths too long"));
}

#[test]
fn a_readiness_report_without_problems_is_good() {
    let saved = saved(Body::Readiness(ReadinessReport {
        source: r"F:\x".into(),
        files: 4,
        ..ReadinessReport::default()
    }));
    let row = entry(&saved);
    assert_eq!(row.headline, "No problems found");
    assert!(!row.problem);
    assert_eq!(row.subject, r"F:\x");
    let doc = document(&saved);
    assert_eq!(doc.verdict, Some(Verdict::Good));
    assert_eq!(doc.tables.len(), 1);
    assert!(doc.notes[0].contains("measured at the source"));
}

#[test]
fn readiness_in_use_section_names_the_holder_and_the_moment() {
    use migrate_core::{InUseStatus, InUseSummary, Problem, ProblemKind, RuleCount};
    let report = ReadinessReport {
        source: r"F:\Mail".into(),
        counts: vec![RuleCount {
            kind: ProblemKind::InUse,
            rule: "inUse".into(),
            count: 1,
            blocker: false,
        }],
        problems: vec![Problem {
            path: "a.pst".into(),
            kind: ProblemKind::InUse,
            rule: "inUse".into(),
            hint: "Held by OUTLOOK.EXE (PID 4120): close it".into(),
            ..Problem::default()
        }],
        in_use: InUseSummary {
            status: InUseStatus::Checked,
            checked: 9,
            in_use: 1,
            skipped: 2,
            ..InUseSummary::default()
        },
        ..ReadinessReport::default()
    };
    let saved = saved(Body::Readiness(report));
    let row = entry(&saved);
    assert_eq!(row.headline, "1 file in use");
    assert!(row.problem);
    let doc = document(&saved);
    assert_eq!(doc.verdict, Some(Verdict::Bad));
    let table = doc
        .tables
        .iter()
        .find(|table| table.heading == "Files in use")
        .unwrap();
    assert_eq!(table.rows[0][1], "Open in another program");
    assert!(table.rows[0][2].contains("OUTLOOK.EXE (PID 4120)"));
    let in_use = doc
        .tiles
        .iter()
        .find(|tile| tile.label == "In use")
        .unwrap();
    assert_eq!(in_use.value, "1 file");
    assert!(doc.notes.iter().any(|note| note.contains("moment in time")
        && note.contains("9 files")
        && note.contains("2 items (links and online-only files)")));
    assert!(doc
        .csv()
        .contains("In use,Open in another program,a.pst,File,No,"));
}

#[test]
fn readiness_says_when_files_in_use_were_not_checked() {
    use migrate_core::{InUseStatus, InUseSummary};
    for (status, says) in [
        (InUseStatus::Off, "turned off"),
        (InUseStatus::NotSupported, "not checked on this system"),
    ] {
        let doc = document(&saved(Body::Readiness(ReadinessReport {
            in_use: InUseSummary {
                status,
                ..InUseSummary::default()
            },
            ..ReadinessReport::default()
        })));
        assert!(doc.notes.iter().any(|note| note.contains(says)), "{says}");
        let tile = doc
            .tiles
            .iter()
            .find(|tile| tile.label == "In use")
            .unwrap();
        assert_eq!(tile.value, "Not checked");
    }
}

#[test]
fn usage_reports_without_duplicate_or_stale_fields_load_and_export_as_before() {
    let old: UsageReport =
        serde_json::from_str(r#"{"path":"C:\\","size":10,"files":1,"removed":[]}"#).unwrap();
    assert_eq!(
        (
            old.duplicates.clone(),
            old.stale.clone(),
            old.owners.clone()
        ),
        (None, None, None)
    );
    let doc = document(&saved(Body::DiskUsage(old)));
    let headings: Vec<&str> = doc.tables.iter().map(|t| t.heading.as_str()).collect();
    assert_eq!(headings, ["Largest files", "Biggest folders", "File types"]);
    assert!(!doc.html("", "").contains("Duplicates"));
    assert!(!doc.csv().contains("Stale data"));
    assert!(!doc.html("", "").contains("Space by owner"));
}

#[test]
fn usage_report_shows_the_owner_table() {
    let report = UsageReport {
        path: r"D:\Shares".into(),
        size: 10_000,
        owners: Some(Box::new(OwnerTotals {
            total_disk: 8_000,
            total_files: 12,
            owners: vec![
                OwnerItem {
                    name: r"CORP\alice".into(),
                    files: 9,
                    disk: 6_000,
                    top_folders: vec![r"D:\Shares\a".into(), r"D:\Shares\b".into()],
                },
                OwnerItem {
                    name: "Unknown (no access)".into(),
                    files: 3,
                    disk: 2_000,
                    top_folders: Vec::new(),
                },
            ],
        })),
        ..UsageReport::default()
    };
    let again: UsageReport =
        serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
    assert_eq!(again, report);

    let doc = document(&saved(Body::DiskUsage(report)));
    let table = doc
        .tables
        .iter()
        .find(|t| t.heading.starts_with("Space by owner"))
        .unwrap();
    assert_eq!(
        table.heading,
        format!("Space by owner: {} in 12 files", format_bytes(8_000))
    );
    assert_eq!(
        table.rows[0],
        [
            r"CORP\alice".to_string(),
            format_bytes(6_000),
            "9".to_string(),
            "75.0%".to_string(),
            r"D:\Shares\a; D:\Shares\b".to_string(),
        ]
    );
    assert_eq!(table.rows[1][3], "25.0%");
    assert!(doc.html("", "").contains("Unknown (no access)"));
}

#[test]
fn usage_report_shows_duplicate_and_stale_sections() {
    let report = UsageReport {
        path: r"D:\Shares".into(),
        size: 10_000,
        duplicates: Some(Box::new(DuplicateTotals {
            min_size: 1 << 20,
            groups: 1,
            files: 3,
            reclaimable: 4_000,
            top: vec![DuplicateItem {
                name: "setup, v2.iso".into(),
                size: 2_000,
                copies: 3,
                reclaimable: 4_000,
                paths: vec![
                    r"D:\Shares\a\setup, v2.iso".into(),
                    r"D:\Shares\b\setup, v2.iso".into(),
                ],
            }],
        })),
        stale: Some(Box::new(StaleTotals {
            years: 3,
            not_opened: false,
            now_ms: 1_792_800_000_000,
            stale_files: 2,
            stale_bytes: 2_500,
            unknown_files: 1,
            unknown_bytes: 7,
            folders: vec![StaleFolder {
                name: "Finance".into(),
                stale_files: 2,
                stale_bytes: 2_500,
                folder_bytes: 5_000,
                newest_ms: Some(1_600_000_000_000),
            }],
        })),
        ..UsageReport::default()
    };
    // The new fields survive a save and load.
    let again: UsageReport =
        serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
    assert_eq!(again, report);

    let doc = document(&saved(Body::DiskUsage(report)));
    let dupes = doc
        .tables
        .iter()
        .find(|t| t.heading.starts_with("Duplicates"))
        .unwrap();
    assert_eq!(
        dupes.heading,
        format!(
            "Duplicates: 3 files in 1 group, {} could be freed",
            format_bytes(4_000)
        )
    );
    assert_eq!(dupes.rows[0][2], "3");
    assert!(
        dupes
            .footnote
            .contains(&format!("{} or more", format_bytes(1 << 20))),
        "{}",
        dupes.footnote
    );

    let stale = doc
        .tables
        .iter()
        .find(|t| t.heading.starts_with("Stale data"))
        .unwrap();
    assert_eq!(
        stale.heading,
        format!(
            "Stale data: {} in 2 files (25.0% of the scan) not modified in 3 years",
            format_bytes(2_500)
        )
    );
    assert_eq!(
        stale.rows[0],
        [
            "Finance".to_string(),
            format_bytes(2_500),
            "2".into(),
            "50.0%".into(),
            "2020-09-13".into()
        ]
    );
    assert!(
        stale.footnote.contains("Measured on 2026-10-24."),
        "{}",
        stale.footnote
    );
    assert!(
        stale.footnote.contains("1 file (7 bytes)"),
        "{}",
        stale.footnote
    );

    let html = doc.html("", "");
    assert!(
        html.contains("Stale data: "),
        "the HTML export has a stale section"
    );
    assert!(html.contains("setup, v2.iso"));
}

fn permissions_report() -> ReadinessReport {
    use migrate_core::{
        AuditStatus, AuditedItem, EntryRow, Finding, FindingCount, FindingKind, PermissionsReport,
    };
    let entry = |account: &str, mask, inherited| EntryRow {
        account: account.into(),
        sid: "S-1-1-0".into(),
        access: "Allow".into(),
        rights: "Modify".into(),
        mask,
        inherited,
        applies_to: "This folder, subfolders and files".into(),
    };
    ReadinessReport {
        source: r"F:\Finance".into(),
        permission_issues: 1,
        permissions: PermissionsReport {
            status: AuditStatus::Checked,
            folders_read: 2,
            issues: 1,
            counts: vec![FindingCount {
                kind: FindingKind::WideWrite,
                count: 1,
            }],
            findings: vec![Finding {
                path: "Payroll".into(),
                dir: true,
                kind: FindingKind::WideWrite,
                detail: "Everyone: Modify".into(),
                sid: "S-1-1-0".into(),
            }],
            items: vec![
                AuditedItem {
                    path: String::new(),
                    dir: true,
                    owner: "BUILTIN\\Administrators".into(),
                    owner_sid: "S-1-5-32-544".into(),
                    protected: true,
                    sddl: "O:BAD:P(A;;FA;;;WD)".into(),
                    entries: vec![entry("Everyone", 0x1301BF, false)],
                },
                AuditedItem {
                    path: "Kunden, \"VIP\" Müller".into(),
                    dir: true,
                    owner: "=cmd|x".into(),
                    entries: vec![
                        entry("CORP\\Zoë, Jr.", 0x1F01FF, true),
                        entry("S-1-5-21-1-2-3-1001 (account not found)", 0x120089, false),
                    ],
                    ..AuditedItem::default()
                },
                AuditedItem {
                    path: "empty".into(),
                    dir: true,
                    ..AuditedItem::default()
                },
            ],
            ..PermissionsReport::default()
        },
        ..ReadinessReport::default()
    }
}

/// Splits CSV text (quotes, commas and line breaks inside quotes) back into cells.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut cell, mut quoted) = (Vec::new(), Vec::new(), String::new(), false);
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => row.push(std::mem::take(&mut cell)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            _ => cell.push(c),
        }
    }
    rows
}

#[test]
fn the_permissions_csv_has_one_row_per_entry_and_survives_a_round_trip() {
    let saved = saved(Body::Readiness(permissions_report()));
    let doc = document(&saved);
    let csv = doc.csv_layout_with(Some("permissions"), false).unwrap();
    let rows = parse_csv(&csv);
    assert_eq!(rows[0].len(), 13);
    assert_eq!(rows[0][0], "Path");
    // 1 + 2 entries, and the item without entries still shows.
    assert_eq!(rows.len(), 1 + 3 + 1);
    assert!(rows.iter().all(|row| row.len() == 13));
    assert_eq!(rows[1][0], "(the source folder)");
    assert_eq!(rows[1][4], "Yes");
    assert_eq!(rows[1][9], "0x001301BF");
    assert_eq!(rows[1][12], "O:BAD:P(A;;FA;;;WD)");
    assert_eq!(rows[2][0], "Kunden, \"VIP\" Müller");
    assert_eq!(rows[2][5], "CORP\\Zoë, Jr.");
    assert_eq!(rows[2][10], "Yes");
    // A formula in a name is defused.
    assert_eq!(rows[2][2], "'=cmd|x");
    assert_eq!(rows[4][0], "empty");
    assert_eq!(rows[4][5], "");
}

#[test]
fn the_default_csv_is_unchanged_and_an_unknown_layout_is_refused() {
    let doc = document(&saved(Body::Readiness(permissions_report())));
    assert_eq!(
        doc.csv_layout_with(None, false).unwrap(),
        doc.csv_with(false)
    );
    assert!(doc.csv().contains("Type,Rule,Path"));
    assert!(doc.csv_layout_with(Some("nope"), false).is_err());
    // A report without permissions has no permissions CSV.
    let plain = document(&saved(Body::Readiness(readiness_report())));
    assert!(plain.csv_layout_with(Some("permissions"), false).is_err());
}

#[test]
fn permission_issues_show_in_the_headline_tile_and_tables() {
    let saved = saved(Body::Readiness(permissions_report()));
    let row = entry(&saved);
    assert_eq!(row.headline, "1 permission issue");
    assert!(row.problem);
    let doc = document(&saved);
    let tile = doc.tiles.iter().find(|t| t.label == "Permissions").unwrap();
    assert_eq!(tile.value, "1 issue");
    let table = doc
        .tables
        .iter()
        .find(|t| t.heading == "Wide write access")
        .unwrap();
    assert_eq!(table.rows[0], ["Payroll\\", "Everyone: Modify"]);
    assert!(doc.notes.iter().any(|n| n.contains("read only")));
}

#[test]
fn a_report_without_the_permissions_pass_says_so() {
    let doc = document(&saved(Body::Readiness(readiness_report())));
    assert!(doc
        .notes
        .iter()
        .any(|n| n == "Permissions were not checked (turned off for this check)."));
    let tile = doc.tiles.iter().find(|t| t.label == "Permissions").unwrap();
    assert_eq!(tile.value, "Not checked");
}

/// With "Compare permissions" off, a compare saves and renders exactly what 1.2.0 did: the same
/// JSON, the same CSV, and the same page markup and data island. The files in `tests/fixtures`
/// were written by 1.2.0's code; `UPDATE_FIXTURES=1` rewrites them, so only do that on purpose.
#[test]
// Every field 1.2.0 had is spelled out; the `..default()` is for the ones added since.
#[allow(clippy::needless_update)]
fn compare_without_permissions_is_byte_identical_to_1_2_0() {
    let report = CompareReport {
        source: r"C:\Data".into(),
        destination: r"E:\Data".into(),
        source_size: 5_000,
        source_files: 4,
        destination_size: 3_900,
        destination_files: 3,
        missing: 2,
        missing_bytes: 1_100,
        different: 1,
        extra: 1,
        unreadable: 1,
        elapsed_ms: 1_500,
        missing_paths: vec![
            MissingFile {
                path: "a.txt".into(),
                size: 1_024,
            },
            MissingFile {
                path: r"it's\</script>.txt".into(),
                size: 76,
            },
        ],
        ..CompareReport::default()
    };
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let json = serde_json::to_string_pretty(&Body::Compare(report.clone())).unwrap();
    let saved = saved(Body::Compare(report));
    let doc = document(&saved);
    let csv = doc.csv();
    let html = doc.html("Fixture", "fixed date");
    // Markup up to the first script, then the data island: the shared page script and styles
    // are other batches' to change.
    let markup = &html[html.find("<body").unwrap()..html.find("<script").unwrap()];
    let island_from = html.find("<script type=\"application/json\"").unwrap();
    let island = &html[island_from..island_from + html[island_from..].find("</script>").unwrap()];
    let page = format!("{markup}\n{island}");

    for (name, text) in [
        ("compare-1.2.0.json", &json),
        ("compare-1.2.0.csv", &csv),
        ("compare-1.2.0.html", &page),
    ] {
        let path = fixtures.join(name);
        if std::env::var_os("UPDATE_FIXTURES").is_some() {
            std::fs::write(&path, text).unwrap();
        }
        let committed = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("tests/fixtures/{name} (run with UPDATE_FIXTURES=1)"))
            .replace("\r\n", "\n");
        assert_eq!(
            committed,
            text.replace("\r\n", "\n"),
            "{name} changed: a compare with permissions off must stay byte-identical to 1.2.0"
        );
    }
    assert!(!json.contains("ermission"), "{json}");
}

fn compared_with_permissions() -> CompareReport {
    use migrate_core::{
        AuditStatus, EntryRow, PermDetail, PermStatus, PermissionsCompare, SameNameEntry,
        StatusCount,
    };
    let entry = |account: &str, sid: &str| EntryRow {
        account: account.into(),
        sid: sid.into(),
        access: "Allow".into(),
        rights: "Modify".into(),
        mask: 0x1301BF,
        inherited: false,
        applies_to: "This folder, subfolders and files".into(),
    };
    CompareReport {
        source: r"C:\Data".into(),
        destination: r"E:\Data".into(),
        permission_mismatches: 3,
        permissions: PermissionsCompare {
            status: AuditStatus::Checked,
            elevated: true,
            destination_server: "nas2".into(),
            folders_checked: 10,
            files_checked: 2,
            skipped: 1,
            counts: vec![
                StatusCount {
                    status: PermStatus::EntriesDiffer,
                    count: 2,
                },
                StatusCount {
                    status: PermStatus::OwnerDiffers,
                    count: 1,
                },
                StatusCount {
                    status: PermStatus::SameNameDifferentAccount,
                    count: 1,
                },
            ],
            items: vec![
                PermDetail {
                    path: String::new(),
                    dir: true,
                    status: PermStatus::EntriesDiffer,
                    missing: vec![entry(r"SRV\alice", "S-1-5-21-1-2-3-1001")],
                    ..PermDetail::default()
                },
                PermDetail {
                    path: "Finance".into(),
                    dir: true,
                    status: PermStatus::OwnerDiffers,
                    source_owner: r"SRV\alice".into(),
                    destination_owner: "S-1-5-18".into(),
                    ..PermDetail::default()
                },
                PermDetail {
                    path: "=cmd.xlsx".into(),
                    dir: false,
                    status: PermStatus::SameNameDifferentAccount,
                    same_name: vec![SameNameEntry {
                        source: entry(r"OLD\bob", "S-1-5-21-1-2-3-1002"),
                        destination: entry(r"NEW\bob", "S-1-5-21-4-5-6-1002"),
                    }],
                    ..PermDetail::default()
                },
            ],
            items_truncated: true,
            ..PermissionsCompare::default()
        },
        ..CompareReport::default()
    }
}

#[test]
fn a_compare_with_permissions_lists_the_differences_and_exports_a_csv() {
    let report = compared_with_permissions();
    let saved = saved(Body::Compare(report));
    let row = entry(&saved);
    assert!(
        row.problem,
        "a permission mismatch is a problem even with nothing missing"
    );
    assert_eq!(row.headline, "Nothing missing · 3 permissions differ");
    let doc = document(&saved);
    assert_eq!(doc.verdict, Some(Verdict::Bad));
    // The size table stays first, so the default CSV is the one 1.2.0 wrote.
    assert_eq!(doc.tables[0].heading, "Missing at destination");
    let table = &doc.tables[1];
    assert_eq!(table.heading, "Permission differences");
    assert_eq!(table.rows.len(), 3);
    assert_eq!(table.rows[0][0], "(the source folder)");
    assert_eq!(table.rows[1][0], r"Finance\");
    assert_eq!(table.rows[1][1], "Owner differs");
    assert_eq!(table.rows[1][2], r"Owner: SRV\alice → S-1-5-18");
    assert_eq!(table.rows[2][1], "Same name, different account");
    assert_eq!(
        table.row_tone.as_ref().unwrap(),
        &vec![Tone::Bad, Tone::Bad, Tone::Warn],
        "amber for the same-name row only"
    );
    assert!(table.footnote.starts_with("The first 3 of 4 are listed."));
    let permissions_tile = doc.tiles.last().unwrap();
    assert_eq!(permissions_tile.label, "Permissions");
    assert_eq!(permissions_tile.tone, Tone::Bad);
    let note = doc.notes.last().unwrap();
    assert!(note.contains("10 folder pairs"), "{note}");
    assert!(note.contains("Compared as administrator."), "{note}");
    assert!(note.contains("looked up on nas2"), "{note}");
    assert!(note.contains("counts cover all 4"), "{note}");

    let csv = doc.csv_layout_with(Some("permissions"), false).unwrap();
    let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').split("\r\n").collect();
    assert!(lines[0].starts_with("Path,Item,Status,Source owner"));
    // Entry rows, plus a row for the owner difference that has no entry to show.
    assert!(csv.contains("Missing at the destination"));
    assert!(csv.contains("S-1-5-21-1-2-3-1001"));
    assert!(csv.contains("Source (same name, different account)"));
    assert!(csv.contains("Destination (same name, different account)"));
    assert!(csv.contains("S-1-5-18"));
    assert!(csv.contains("0x001301BF"));
    // A hostile name is defused like every other CSV cell.
    assert!(csv.contains("'=cmd.xlsx"), "{csv}");
    assert!(doc.csv_layout_with(Some("nonsense"), false).is_err());
}

#[test]
fn the_permissions_tile_is_amber_when_only_same_name_accounts_differ() {
    use migrate_core::{AuditStatus, PermStatus, PermissionsCompare, StatusCount};
    let report = CompareReport {
        permissions: PermissionsCompare {
            status: AuditStatus::Checked,
            counts: vec![StatusCount {
                status: PermStatus::SameNameDifferentAccount,
                count: 5,
            }],
            ..PermissionsCompare::default()
        },
        ..CompareReport::default()
    };
    let saved = saved(Body::Compare(report));
    let doc = document(&saved);
    assert_eq!(doc.verdict, Some(Verdict::Good), "amber is not a failure");
    assert_eq!(doc.tiles.last().unwrap().tone, Tone::Warn);
    assert!(!entry(&saved).problem);
}

#[test]
fn a_compare_that_could_not_read_permissions_says_so() {
    use migrate_core::{AuditStatus, PermissionsCompare};
    let report = CompareReport {
        permissions: PermissionsCompare {
            status: AuditStatus::NotSupported,
            ..PermissionsCompare::default()
        },
        ..CompareReport::default()
    };
    let doc = document(&saved(Body::Compare(report)));
    assert_eq!(doc.tiles.last().unwrap().value, "Not compared");
    assert!(doc
        .notes
        .last()
        .unwrap()
        .contains("not compared on this system"));
    assert!(doc.csv_layout_with(Some("permissions"), false).is_err() || doc.csv_layouts.is_empty());
}

#[test]
fn saved_permissions_read_back_and_older_reports_load_as_off() {
    let report = compared_with_permissions();
    let text = serde_json::to_string(&Body::Compare(report.clone())).unwrap();
    assert!(text.contains("\"permissionMismatches\":3"));
    // The count is written ahead of the long lists, so the Reports page can stop before them.
    assert!(text.find("permissionMismatches").unwrap() < text.find("missingPaths").unwrap());
    let Body::Compare(again) = serde_json::from_str::<Body>(&text).unwrap() else {
        panic!("a compare")
    };
    assert_eq!(again, report);
    // 1.2.0's report has neither field.
    let old = r#"{"source":"C:\\A","destination":"D:\\B","missing":1}"#;
    let loaded: CompareReport = serde_json::from_str(old).unwrap();
    assert!(loaded.permissions.is_off());
    assert_eq!(loaded.permission_mismatches, 0);
    // A status written by a newer version loads without failing the report.
    let newer =
        r#"{"permissions":{"status":"somethingNewer","counts":[{"status":"alsoNew","count":2}]}}"#;
    let loaded: CompareReport = serde_json::from_str(newer).unwrap();
    assert_eq!(loaded.permissions.status, migrate_core::AuditStatus::Other);
}
