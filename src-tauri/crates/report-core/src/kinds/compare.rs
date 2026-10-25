//! The Compare report as a document.

use super::*;
use migrate_core::{AuditStatus, EntryRow, PermDetail, PermStatus, PermissionsCompare};

pub(super) fn document(report: &CompareReport, meta: Vec<(String, String)>) -> Document {
    let listed = report.missing_paths.len() as u64;
    let mut notes = Vec::new();
    if report.unreadable > 0 {
        notes.push(format!(
            "{} could not be read completely; sizes there may be low.",
            plural(report.unreadable, "folder")
        ));
    }
    let permissions = &report.permissions;
    if !permissions.is_off() {
        notes.push(permissions_note(permissions));
    }
    let plan = Plan::new(
        &report.source,
        &report.destination,
        &report.missing_paths,
        Options::default(),
    );
    let intro = format!(
                "Open PowerShell on a PC that can reach both folders, paste the command and press Enter. It copies {} again{}.",
                plan.as_ref().map_or_else(
                    || "the missing files".to_string(),
                    |plan| plural(plan.files, "missing file")
                ),
                if listed < report.missing {
                    format!(" (only the {} listed here)", format_count(listed))
                } else {
                    String::new()
                }
            );
    Document {
        title: "Compare".into(),
        subtitle: format!("{} → {}", report.source, report.destination),
        meta,
        verdict: Some(if report.missing > 0 || permissions.mismatches() > 0 {
            Verdict::Bad
        } else {
            Verdict::Good
        }),
        tiles: {
            let mut tiles = vec![
                tile(
                    "Source",
                    format_bytes(report.source_size),
                    plural(report.source_files, "file"),
                    Tone::Plain,
                ),
                tile(
                    "Destination",
                    format_bytes(report.destination_size),
                    plural(report.destination_files, "file"),
                    Tone::Plain,
                ),
                if report.missing > 0 {
                    tile(
                        "Missing at destination",
                        plural(report.missing, "file"),
                        format!("{} not copied", format_bytes(report.missing_bytes)),
                        Tone::Bad,
                    )
                } else {
                    tile(
                        "Missing at destination",
                        "None".into(),
                        "Every file is there".into(),
                        Tone::Good,
                    )
                },
                tile(
                    "Other differences",
                    plural(report.different, "size change"),
                    format!("{} only at destination", plural(report.extra, "file")),
                    Tone::Plain,
                ),
            ];
            if !permissions.is_off() {
                tiles.push(permissions_tile(permissions));
            }
            tiles
        },
        notes,
        share: (report.source_files > 0).then(|| {
            let there = report.source_files.saturating_sub(report.missing);
            Share {
                label: "Source files found at the destination".into(),
                detail: format!(
                    "{} of {}",
                    format_count(there),
                    format_count(report.source_files)
                ),
                percent: there as f64 * 100.0 / report.source_files as f64,
                tone: if report.missing == 0 {
                    Tone::Good
                } else {
                    Tone::Plain
                },
            }
        }),
        tables: std::iter::once(
            Table {
                heading: "Missing at destination".into(),
                columns: vec!["Path (relative to the source)".into(), "Size".into()],
                numeric: vec![false, true],
                rows: report
                    .missing_paths
                    .iter()
                    .map(|file| vec![file.path.clone(), format_bytes(file.size)])
                    .collect(),
                size_col: Some(1),
                row_bytes: report.missing_paths.iter().map(|file| file.size).collect(),
                footnote: if listed < report.missing {
                    format!(
                        "The first {} of {} are listed.",
                        format_count(listed),
                        format_count(report.missing)
                    )
                } else {
                    format!("Compared in {}.", seconds(report.elapsed_ms))
                },
                ..Table::default()
            }
            .toned(Tone::Bad),
        )
        .chain(permission_table(permissions))
        .collect(),
        csv_layouts: permission_layout(permissions)
            .map(|table| vec![(PERMISSIONS_LAYOUT.to_string(), table)])
            .unwrap_or_default(),
        sections: plan
            .iter()
            .map(|plan| plan.section(intro.clone()))
            .collect(),
        data: plan.as_ref().map(|plan| json!({ "rec": plan.data() })),
        ..Document::default()
    }
}

/// The name of the permissions CSV, as `report_export` receives it.
pub const PERMISSIONS_LAYOUT: &str = "permissions";

fn permissions_note(permissions: &PermissionsCompare) -> String {
    if permissions.status != AuditStatus::Checked {
        return "Permissions were not compared on this system.".into();
    }
    let mut note = format!(
        "Permissions: read for {} and {} that have entries of their own, read only; nothing was changed. Entries are compared by SID; entries a folder inherited count only on the source folder itself.",
        plural(permissions.folders_checked, "folder pair"),
        plural(permissions.files_checked, "file pair"),
    );
    if permissions.elevated {
        note.push_str(" Compared as administrator.");
    }
    for (side, server) in [
        ("source", &permissions.source_server),
        ("destination", &permissions.destination_server),
    ] {
        if !server.is_empty() {
            note.push_str(&format!(" Names on the {side} were looked up on {server}."));
        }
    }
    if permissions.skipped > 0 {
        note.push_str(&format!(
            " {} (links, online-only files and unusual names) were not read.",
            plural(permissions.skipped, "pair")
        ));
    }
    if permissions.items_truncated {
        note.push_str(&format!(
            " Only the first differing items are listed; the counts cover all {}.",
            format_count(permissions.differences())
        ));
    }
    note
}

fn permissions_tile(permissions: &PermissionsCompare) -> Tile {
    if permissions.status != AuditStatus::Checked {
        return tile(
            "Permissions",
            "Not compared".into(),
            String::new(),
            Tone::Plain,
        );
    }
    let amber = permissions.count_of(PermStatus::SameNameDifferentAccount);
    let detail = if amber > 0 {
        format!(
            "{} with the same name, different account",
            format_count(amber)
        )
    } else {
        "Owner, inheritance and entries".to_string()
    };
    match permissions.mismatches() {
        0 if amber > 0 => tile("Permissions", "Match".into(), detail, Tone::Warn),
        0 => tile("Permissions", "Match".into(), detail, Tone::Good),
        count => tile(
            "Permissions",
            plural(count, "difference"),
            detail,
            Tone::Bad,
        ),
    }
}

fn item_path(detail: &PermDetail) -> String {
    if detail.path.is_empty() {
        "(the source folder)".into()
    } else if detail.dir {
        format!("{}\\", detail.path)
    } else {
        detail.path.clone()
    }
}

fn status_tone(status: PermStatus) -> Tone {
    if status == PermStatus::SameNameDifferentAccount {
        Tone::Warn
    } else {
        Tone::Bad
    }
}

/// One row per differing item, worst first in the order the compare found them.
fn permission_table(permissions: &PermissionsCompare) -> Option<Table> {
    if permissions.status != AuditStatus::Checked || permissions.differences() == 0 {
        return None;
    }
    let listed = permissions.items.len() as u64;
    let total = permissions.differences();
    Some(Table {
        heading: "Permission differences".into(),
        columns: vec![
            "Path (relative to the source)".into(),
            "Status".into(),
            "What differs".into(),
        ],
        numeric: vec![false; 3],
        rows: permissions
            .items
            .iter()
            .map(|item| vec![item_path(item), item.status.label().into(), item.summary()])
            .collect(),
        row_tone: Some(
            permissions
                .items
                .iter()
                .map(|item| status_tone(item.status))
                .collect(),
        ),
        footnote: if listed < total {
            format!(
                "The first {} of {} are listed. Same name, different account: the entries name the same account but the SIDs differ (a recreated account).",
                format_count(listed),
                format_count(total)
            )
        } else {
            "Same name, different account: the entries name the same account but the SIDs differ (a recreated account).".into()
        },
        ..Table::default()
    })
}

/// One row per entry that differs; an item with no entry to show (an owner or inheritance
/// difference, an unreadable item) gets one row so it shows. `None` when nothing was compared.
fn permission_layout(permissions: &PermissionsCompare) -> Option<Table> {
    if permissions.status != AuditStatus::Checked {
        return None;
    }
    let yes_no = |value: bool| if value { "Yes" } else { "No" }.to_string();
    let mut rows = Vec::new();
    for item in &permissions.items {
        let lead = |difference: &str, entry: [String; 7]| {
            let mut row = vec![
                item_path(item),
                if item.dir { "Folder" } else { "File" }.to_string(),
                item.status.label().to_string(),
                item.source_owner.clone(),
                item.destination_owner.clone(),
                yes_no(item.source_protected),
                yes_no(item.destination_protected),
                difference.to_string(),
            ];
            row.extend(entry);
            row.push(item.error.clone());
            row
        };
        let entry = |row: &EntryRow| {
            [
                row.account.clone(),
                row.sid.clone(),
                row.access.clone(),
                row.rights.clone(),
                format!("0x{:08X}", row.mask),
                yes_no(row.inherited),
                row.applies_to.clone(),
            ]
        };
        let before = rows.len();
        for row in &item.missing {
            rows.push(lead("Missing at the destination", entry(row)));
        }
        for row in &item.extra {
            rows.push(lead("Only at the destination", entry(row)));
        }
        for pair in &item.same_name {
            rows.push(lead(
                "Source (same name, different account)",
                entry(&pair.source),
            ));
            rows.push(lead(
                "Destination (same name, different account)",
                entry(&pair.destination),
            ));
        }
        if rows.len() == before {
            rows.push(lead("", Default::default()));
        }
    }
    let columns = [
        "Path",
        "Item",
        "Status",
        "Source owner",
        "Destination owner",
        "Source inheritance protected",
        "Destination inheritance protected",
        "Difference",
        "Account",
        "SID",
        "Allow or deny",
        "Rights",
        "Mask (hex)",
        "Inherited",
        "Applies to",
        "Error",
    ];
    Some(Table {
        heading: "Permission differences".into(),
        columns: columns.iter().map(ToString::to_string).collect(),
        numeric: vec![false; columns.len()],
        rows,
        ..Table::default()
    })
}
