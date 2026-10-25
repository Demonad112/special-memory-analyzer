//! The Migration readiness report as a document: totals, counts per rule, then one table per
//! problem type. The CSV has every listed problem in one table.

use super::*;
use crate::header::ReadinessHeader;
use migrate_core::{
    rule_label, AuditStatus, Finding, FindingKind, InUseStatus, PermissionsReport, Problem,
    ProblemKind,
};
use scan_core::TargetProfile;

pub(super) fn subject(source: &str, destination_root: &str) -> String {
    if destination_root.trim().is_empty() {
        source.to_string()
    } else {
        format!("{source} → {}", destination_root.trim())
    }
}

pub(super) fn headline(report: &ReadinessHeader) -> String {
    let mut parts = Vec::new();
    if report.name_problems > 0 {
        parts.push(plural(report.name_problems, "name problem"));
    }
    if report.long_paths > 0 {
        parts.push(plural(report.long_paths, "long path"));
    }
    if report.in_use.in_use > 0 {
        parts.push(format!("{} in use", plural(report.in_use.in_use, "file")));
    }
    if report.unreadable > 0 {
        parts.push(plural(report.unreadable, "unreadable folder"));
    }
    if report.in_use.unreadable > 0 {
        parts.push(format!(
            "{} could not be opened",
            plural(report.in_use.unreadable, "file")
        ));
    }
    if report.permission_issues > 0 {
        parts.push(plural(report.permission_issues, "permission issue"));
    }
    if parts.is_empty() {
        parts.push("No problems found".to_string());
    }
    if report.links > 0 {
        parts.push(format!("{} not followed", plural(report.links, "link")));
    }
    parts.join(" · ")
}

fn target_label(target: TargetProfile) -> &'static str {
    match target {
        TargetProfile::FileServer => "File server (NTFS)",
        TargetProfile::SharePoint => "OneDrive / SharePoint",
    }
}

fn kind_label(kind: ProblemKind) -> &'static str {
    match kind {
        ProblemKind::Name => "Name",
        ProblemKind::LongPath => "Long path",
        ProblemKind::InUse => "In use",
        ProblemKind::Unreadable => "Unreadable",
        ProblemKind::Link => "Link",
        ProblemKind::Other => "Other",
    }
}

/// Every group the report can show, the ones this version writes first.
fn groups() -> impl Iterator<Item = ProblemKind> {
    ProblemKind::ALL.into_iter().chain([ProblemKind::Other])
}

fn problem_tone(problem: &Problem) -> Tone {
    if problem.blocker {
        Tone::Bad
    } else if problem.kind == ProblemKind::Link {
        Tone::Plain
    } else {
        Tone::Warn
    }
}

fn count_tile(label: &str, count: u64, one: &str, detail: &str, bad: Tone) -> Tile {
    if count > 0 {
        tile(label, plural(count, one), detail.into(), bad)
    } else {
        tile(label, "None".into(), "Nothing to fix".into(), Tone::Good)
    }
}

pub(super) fn document(report: &ReadinessReport, mut meta: Vec<(String, String)>) -> Document {
    meta.push(("Target".into(), target_label(report.target).into()));
    if !report.destination_root.trim().is_empty() {
        meta.push((
            "Destination root".into(),
            report.destination_root.trim().into(),
        ));
    }
    let limit = report.target.path_limit();
    let mut notes = vec![if report.destination_root.trim().is_empty() {
        format!("Path lengths are measured at the source (limit {limit} characters). Give a destination root to measure them where the files are going.")
    } else {
        format!(
            "Path lengths are measured as they will be under {} (limit {limit} characters).",
            report.destination_root.trim()
        )
    }];
    if report.unreadable > 0 {
        notes.push(format!(
            "{} could not be read completely; nothing below them was checked.",
            plural(report.unreadable, "folder")
        ));
    }
    if report.cloud_files > 0 {
        notes.push(format!(
            "{} are online-only: they were checked by name only and never opened.",
            plural(report.cloud_files, "cloud file")
        ));
    }
    notes.push(in_use_note(report));
    notes.push(permissions_note(&report.permissions));

    let rules = Table {
        heading: "Problems by rule".into(),
        columns: vec![
            "Type".into(),
            "Rule".into(),
            "Items".into(),
            "Blocks the copy".into(),
        ],
        numeric: vec![false, false, true, false],
        rows: report
            .counts
            .iter()
            .map(|count| {
                vec![
                    kind_label(count.kind).into(),
                    rule_label(&count.rule).into(),
                    format_count(count.count),
                    if count.blocker { "Yes" } else { "No" }.into(),
                ]
            })
            .collect(),
        row_tone: Some(
            report
                .counts
                .iter()
                .map(|count| {
                    if count.blocker {
                        Tone::Bad
                    } else {
                        Tone::Plain
                    }
                })
                .collect(),
        ),
        footnote: format!("Checked in {}.", seconds(report.elapsed_ms)),
        ..Table::default()
    };
    let mut tables = vec![rules];
    for kind in groups() {
        let total = report.count_of(kind);
        if total == 0 {
            continue;
        }
        let listed: Vec<&Problem> = report.listed(kind).collect();
        tables.push(Table {
            heading: kind.heading().into(),
            columns: vec![
                "Path (relative to the source)".into(),
                "Rule".into(),
                "Fix".into(),
            ],
            numeric: vec![false, false, false],
            rows: listed
                .iter()
                .map(|problem| {
                    vec![
                        display_path(problem),
                        rule_label(&problem.rule).into(),
                        problem.hint.clone(),
                    ]
                })
                .collect(),
            row_tone: Some(listed.iter().map(|problem| problem_tone(problem)).collect()),
            footnote: if (listed.len() as u64) < total {
                format!(
                    "The first {} of {} are listed.",
                    format_count(listed.len() as u64),
                    format_count(total)
                )
            } else {
                String::new()
            },
            ..Table::default()
        });
    }

    tables.extend(permission_tables(&report.permissions));

    let csv_table = Table {
        heading: "Problems".into(),
        columns: vec![
            "Type".into(),
            "Rule".into(),
            "Path (relative to the source)".into(),
            "Item".into(),
            "Blocks the copy".into(),
            "Fix".into(),
        ],
        numeric: vec![false; 6],
        rows: report
            .problems
            .iter()
            .map(|problem| {
                vec![
                    kind_label(problem.kind).into(),
                    rule_label(&problem.rule).into(),
                    problem.path.clone(),
                    if problem.dir { "Folder" } else { "File" }.into(),
                    if problem.blocker { "Yes" } else { "No" }.into(),
                    problem.hint.clone(),
                ]
            })
            .collect(),
        ..Table::default()
    };

    Document {
        title: "Migration readiness".into(),
        subtitle: subject(&report.source, &report.destination_root),
        meta,
        verdict: Some(if report.has_problems() {
            Verdict::Bad
        } else {
            Verdict::Good
        }),
        tiles: vec![
            tile(
                "Checked",
                plural(report.files, "file"),
                plural(report.dirs, "folder"),
                Tone::Plain,
            ),
            count_tile(
                "Names",
                report.name_problems,
                "item",
                "Rename before the copy",
                if report.blockers > 0 {
                    Tone::Bad
                } else {
                    Tone::Warn
                },
            ),
            count_tile(
                "Long paths",
                report.long_paths,
                "path",
                &format!("Over {limit} characters"),
                Tone::Warn,
            ),
            in_use_tile(report),
            permissions_tile(report),
            tile(
                "Links",
                if report.links > 0 {
                    plural(report.links, "link")
                } else {
                    "None".into()
                },
                "Listed, not followed".into(),
                Tone::Plain,
            ),
        ],
        notes,
        tables,
        csv_table: Some(csv_table),
        csv_layouts: permission_layout(&report.permissions)
            .map(|table| vec![(PERMISSIONS_LAYOUT.to_string(), table)])
            .unwrap_or_default(),
        ..Document::default()
    }
}

/// The source folder itself has an empty relative path.
fn display_path(problem: &Problem) -> String {
    if problem.path.is_empty() {
        "(the source folder)".into()
    } else if problem.dir {
        format!("{}\\", problem.path)
    } else {
        problem.path.clone()
    }
}

/// What the in-use pass did, in one sentence.
fn in_use_note(report: &ReadinessReport) -> String {
    let pass = &report.in_use;
    match pass.status {
        InUseStatus::Checked => format!(
            "Files in use: {} opened for read during this check and closed at once, so this is a moment in time. Close the programs listed before the copy, or record the copy in Watch mode. Opening a file can update its last access time on NTFS.{}",
            plural(pass.checked, "file"),
            if pass.skipped > 0 {
                format!(
                    " {} (links and online-only files) were not opened.",
                    plural(pass.skipped, "item")
                )
            } else {
                String::new()
            }
        ),
        InUseStatus::NotSupported => "Files in use were not checked on this system.".into(),
        InUseStatus::Off | InUseStatus::Other => {
            "Files in use were not checked (turned off for this check).".into()
        }
    }
}

fn in_use_tile(report: &ReadinessReport) -> Tile {
    let pass = &report.in_use;
    match pass.status {
        InUseStatus::Checked => count_tile(
            "In use",
            pass.in_use,
            "file",
            "Close them or use Watch mode",
            Tone::Warn,
        ),
        _ => tile("In use", "Not checked".into(), String::new(), Tone::Plain),
    }
}

/// The name of the permissions CSV, as `report_export` receives it.
pub const PERMISSIONS_LAYOUT: &str = "permissions";

fn finding_tone(kind: FindingKind) -> Tone {
    if kind.is_issue() {
        Tone::Warn
    } else {
        Tone::Plain
    }
}

fn permissions_note(permissions: &PermissionsReport) -> String {
    match permissions.status {
        AuditStatus::Checked => {
            let scope = if permissions.every_file {
                format!(
                    "{} and {}",
                    plural(permissions.folders_read, "folder"),
                    plural(permissions.files_read, "file")
                )
            } else {
                format!(
                    "{} (files were not read: turn on Check every file to find files with their own entries)",
                    plural(permissions.folders_read, "folder")
                )
            };
            format!(
                "Permissions: read for {scope}, read only; nothing was changed. {}{} Entries a folder inherited are listed once, on the folder that sets them.{}",
                if permissions.elevated {
                    "Ran as administrator. "
                } else {
                    "Ran without administrator rights, so folders this account cannot read are listed as unreadable. "
                },
                if permissions.server.is_empty() {
                    "Account names were looked up on this PC.".to_string()
                } else {
                    format!("Account names were looked up on {}.", permissions.server)
                },
                if permissions.skipped > 0 {
                    format!(
                        " {} (links, online-only files) were not read.",
                        plural(permissions.skipped, "item")
                    )
                } else {
                    String::new()
                }
            )
        }
        AuditStatus::NotSupported => "Permissions were not checked on this system.".into(),
        AuditStatus::Off | AuditStatus::Other => {
            "Permissions were not checked (turned off for this check).".into()
        }
    }
}

fn permissions_tile(report: &ReadinessReport) -> Tile {
    let permissions = &report.permissions;
    match permissions.status {
        AuditStatus::Checked => count_tile(
            "Permissions",
            permissions.issues,
            "issue",
            "Unknown accounts, wide write access",
            Tone::Warn,
        ),
        _ => tile(
            "Permissions",
            "Not checked".into(),
            String::new(),
            Tone::Plain,
        ),
    }
}

fn finding_path(finding: &Finding) -> String {
    if finding.path.is_empty() {
        "(the source folder)".into()
    } else if finding.dir {
        format!("{}\\", finding.path)
    } else {
        finding.path.clone()
    }
}

/// A summary table, then one table per finding type that has findings.
fn permission_tables(permissions: &PermissionsReport) -> Vec<Table> {
    if permissions.status != AuditStatus::Checked || permissions.counts.is_empty() {
        return Vec::new();
    }
    let mut tables = vec![Table {
        heading: "Permission findings".into(),
        columns: vec!["Finding".into(), "Items".into()],
        numeric: vec![false, true],
        rows: permissions
            .counts
            .iter()
            .map(|count| vec![count.kind.heading().into(), format_count(count.count)])
            .collect(),
        ..Table::default()
    }];
    for kind in FindingKind::ALL {
        let total = permissions.count_of(kind);
        if total == 0 {
            continue;
        }
        let listed: Vec<&Finding> = permissions.listed(kind).collect();
        tables.push(Table {
            heading: kind.heading().into(),
            columns: vec!["Path (relative to the source)".into(), "Detail".into()],
            numeric: vec![false, false],
            rows: listed
                .iter()
                .map(|finding| vec![finding_path(finding), finding.detail.clone()])
                .collect(),
            row_tone: Some(listed.iter().map(|_| finding_tone(kind)).collect()),
            footnote: if (listed.len() as u64) < total {
                format!(
                    "The first {} of {} are listed. {}",
                    format_count(listed.len() as u64),
                    format_count(total),
                    kind.explanation()
                )
            } else {
                kind.explanation().into()
            },
            ..Table::default()
        });
    }
    tables
}

/// One row per access entry; a folder row also carries the SDDL. An item with no entries still
/// gets one row so it shows. `None` when no permissions were read.
fn permission_layout(permissions: &PermissionsReport) -> Option<Table> {
    if permissions.status != AuditStatus::Checked {
        return None;
    }
    let yes_no = |value: bool| if value { "Yes" } else { "No" }.to_string();
    let mut rows = Vec::new();
    for item in &permissions.items {
        let path = if item.path.is_empty() {
            "(the source folder)".to_string()
        } else {
            item.path.clone()
        };
        let lead = |account: [String; 7]| {
            let mut row = vec![
                path.clone(),
                if item.dir { "Folder" } else { "File" }.to_string(),
                item.owner.clone(),
                item.owner_sid.clone(),
                yes_no(item.protected),
            ];
            row.extend(account);
            row.push(if item.dir {
                item.sddl.clone()
            } else {
                String::new()
            });
            row
        };
        if item.entries.is_empty() {
            rows.push(lead(Default::default()));
        }
        for entry in &item.entries {
            rows.push(lead([
                entry.account.clone(),
                entry.sid.clone(),
                entry.access.clone(),
                entry.rights.clone(),
                format!("0x{:08X}", entry.mask),
                yes_no(entry.inherited),
                entry.applies_to.clone(),
            ]));
        }
    }
    let columns = [
        "Path",
        "Item",
        "Owner",
        "Owner SID",
        "Inheritance protected",
        "Account",
        "SID",
        "Allow or deny",
        "Rights",
        "Mask (hex)",
        "Inherited",
        "Applies to",
        "SDDL",
    ];
    Some(Table {
        heading: "Permissions".into(),
        columns: columns.iter().map(ToString::to_string).collect(),
        numeric: vec![false; columns.len()],
        rows,
        ..Table::default()
    })
}
