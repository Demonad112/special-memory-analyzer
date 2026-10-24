//! The Disk usage and Quick cleanup reports as documents.

use super::*;

pub(super) fn disk_usage(report: &UsageReport, meta: Vec<(String, String)>) -> Document {
    let removed: u64 = report.removed.iter().map(|item| item.size).sum();
    let mut notes = Vec::new();
    if report.unreadable > 0 {
        notes.push(format!(
            "{} could not be read, so the totals may be low.",
            plural(report.unreadable, "folder")
        ));
    }
    if report.cloud_files > 0 {
        notes.push(format!(
            "{} ({}) are online-only and take no space on this PC; they are not counted.",
            plural(report.cloud_files, "cloud file"),
            format_bytes(report.cloud_bytes)
        ));
    }
    let size_row = |name: &str, size: u64, total: u64| -> Vec<String> {
        let share = if total > 0 {
            format!("{:.1}%", size as f64 * 100.0 / total as f64)
        } else {
            String::new()
        };
        vec![name.to_string(), format_bytes(size), share]
    };
    let mut tables = vec![
        Table {
            heading: "Largest files".into(),
            columns: vec!["File".into(), "Size".into(), "Share".into()],
            numeric: vec![false, true, true],
            rows: report
                .largest_files
                .iter()
                .map(|file| size_row(&file.path, file.size, report.size))
                .collect(),
            size_col: Some(1),
            row_bytes: report.largest_files.iter().map(|file| file.size).collect(),
            ..Table::default()
        },
        Table {
            heading: "Biggest folders".into(),
            columns: vec!["Folder".into(), "Size".into(), "Share".into()],
            numeric: vec![false, true, true],
            rows: report
                .top_folders
                .iter()
                .map(|folder| size_row(&folder.name, folder.size, report.size))
                .collect(),
            size_col: Some(1),
            row_bytes: report
                .top_folders
                .iter()
                .map(|folder| folder.size)
                .collect(),
            ..Table::default()
        },
        Table {
            heading: "File types".into(),
            columns: vec!["Type".into(), "Size".into(), "Share".into()],
            numeric: vec![false, true, true],
            rows: report
                .types
                .iter()
                .map(|kind| {
                    let name = if kind.extension.is_empty() {
                        format!("No extension ({})", plural(kind.files, "file"))
                    } else {
                        format!(".{} ({})", kind.extension, plural(kind.files, "file"))
                    };
                    size_row(&name, kind.size, report.size)
                })
                .collect(),
            size_col: Some(1),
            row_bytes: report.types.iter().map(|kind| kind.size).collect(),
            ..Table::default()
        },
    ];
    if !report.removed.is_empty() {
        tables.insert(
            0,
            Table {
                heading: "Removed".into(),
                columns: vec!["Item".into(), "Size".into(), "How".into()],
                numeric: vec![false, true, false],
                rows: report
                    .removed
                    .iter()
                    .map(|item| {
                        vec![
                            item.path.clone(),
                            format_bytes(item.size),
                            if item.permanent {
                                "Deleted".into()
                            } else {
                                "Recycle Bin".into()
                            },
                        ]
                    })
                    .collect(),
                size_col: Some(1),
                row_bytes: report.removed.iter().map(|item| item.size).collect(),
                ..Table::default()
            },
        );
    }
    if let Some(dupes) = &report.duplicates {
        tables.push(duplicates_table(dupes));
    }
    if let Some(stale) = &report.stale {
        tables.push(stale_table(stale, report.size));
    }
    if let Some(owners) = &report.owners {
        tables.push(owners_table(owners));
    }
    let explorer_on = report
        .explorer
        .as_ref()
        .is_some_and(|explorer| explorer.nodes.len() > 1);
    let explorer_data =
        report.explorer.as_ref().filter(|_| explorer_on).map(
            |explorer| serde_json::json!({ "x": explorer::page_data(explorer, &report.path) }),
        );
    let mut tiles = vec![
        tile(
            "Used",
            format_bytes(report.size),
            format!(
                "{} · {}",
                plural(report.files, "file"),
                plural(report.dirs, "folder")
            ),
            Tone::Plain,
        ),
        tile(
            "Removed",
            format_bytes(removed),
            plural(report.removed.len() as u64, "item"),
            if removed > 0 { Tone::Good } else { Tone::Plain },
        ),
    ];
    if let (Some(total), Some(free)) = (report.drive_total, report.drive_free) {
        tiles.insert(
            1,
            tile(
                "Free space at scan",
                format_bytes(free),
                format!("of {}", format_bytes(total)),
                Tone::Plain,
            ),
        );
    }
    Document {
        title: "Disk usage".into(),
        subtitle: report.path.clone(),
        meta,
        verdict: (removed > 0).then_some(Verdict::Good),
        tiles,
        notes,
        share: report
            .drive_total
            .filter(|total| *total > 0)
            .map(|total| Share {
                label: "Space this folder takes on the drive".into(),
                detail: format!("{} of {}", format_bytes(report.size), format_bytes(total)),
                percent: (report.size as f64 * 100.0 / total as f64).min(100.0),
                tone: Tone::Plain,
            }),
        tables,
        explorer: explorer_on,
        data: explorer_data,
        ..Document::default()
    }
}

fn duplicates_table(dupes: &DuplicateTotals) -> Table {
    Table {
        heading: format!(
            "Duplicates: {} in {}, {} could be freed",
            plural(dupes.files, "file"),
            plural(dupes.groups, "group"),
            format_bytes(dupes.reclaimable)
        ),
        columns: vec![
            "File".into(),
            "Could be freed".into(),
            "Copies".into(),
            "Size of each".into(),
            "Where".into(),
        ],
        numeric: vec![false, true, true, true, false],
        rows: dupes
            .top
            .iter()
            .map(|item| {
                vec![
                    item.name.clone(),
                    format_bytes(item.reclaimable),
                    format_count(item.copies),
                    format_bytes(item.size),
                    item.paths.join("; "),
                ]
            })
            .collect(),
        size_col: Some(1),
        row_bytes: dupes.top.iter().map(|item| item.reclaimable).collect(),
        footnote: format!(
            "Files of {} or more with identical content (BLAKE3). Online-only and hard-linked files are not compared.{}",
            format_bytes(dupes.min_size),
            if dupes.groups > dupes.top.len() as u64 {
                format!(" The {} largest groups are listed.", dupes.top.len())
            } else {
                String::new()
            }
        ),
        ..Table::default()
    }
}

fn owners_table(owners: &OwnerTotals) -> Table {
    Table {
        heading: format!(
            "Space by owner: {} in {}",
            format_bytes(owners.total_disk),
            plural(owners.total_files, "file")
        ),
        columns: vec![
            "Owner".into(),
            "Space".into(),
            "Files".into(),
            "Share".into(),
            "Most of it in".into(),
        ],
        numeric: vec![false, true, true, true, false],
        rows: owners
            .owners
            .iter()
            .map(|owner| {
                let share = if owners.total_disk > 0 {
                    format!("{:.1}%", owner.disk as f64 * 100.0 / owners.total_disk as f64)
                } else {
                    String::new()
                };
                vec![
                    owner.name.clone(),
                    format_bytes(owner.disk),
                    format_count(owner.files),
                    share,
                    owner.top_folders.join("; "),
                ]
            })
            .collect(),
        size_col: Some(1),
        row_bytes: owners.owners.iter().map(|owner| owner.disk).collect(),
        footnote: "Owner of each file as Windows records it. Online-only files are listed with the owner but take no space here.".into(),
        ..Table::default()
    }
}

fn stale_table(stale: &StaleTotals, total: u64) -> Table {
    let rule = if stale.not_opened {
        format!(
            "not modified or opened in {}",
            plural(u64::from(stale.years), "year")
        )
    } else {
        format!("not modified in {}", plural(u64::from(stale.years), "year"))
    };
    let share = |part: u64, whole: u64| {
        if whole > 0 {
            format!("{:.1}%", part as f64 * 100.0 / whole as f64)
        } else {
            String::new()
        }
    };
    let mut footnote = format!(
        "Measured on {}.",
        transfer_core::format_utc(stale.now_ms)
            .split(' ')
            .next()
            .unwrap_or_default()
    );
    if stale.unknown_files > 0 {
        footnote.push_str(&format!(
            " {} ({}) with a missing, future or pre-1980 date are not counted.",
            plural(stale.unknown_files, "file"),
            format_bytes(stale.unknown_bytes)
        ));
    }
    if stale.not_opened {
        footnote.push_str(" Windows does not always keep last-access times up to date.");
    }
    Table {
        heading: format!(
            "Stale data: {} in {} ({} of the scan) {rule}",
            format_bytes(stale.stale_bytes),
            plural(stale.stale_files, "file"),
            share(stale.stale_bytes, total)
        ),
        columns: vec![
            "Folder".into(),
            "Stale".into(),
            "Files".into(),
            "Of the folder".into(),
            "Newest change".into(),
        ],
        numeric: vec![false, true, true, true, false],
        rows: stale
            .folders
            .iter()
            .map(|folder| {
                vec![
                    folder.name.clone(),
                    format_bytes(folder.stale_bytes),
                    format_count(folder.stale_files),
                    share(folder.stale_bytes, folder.folder_bytes),
                    folder
                        .newest_ms
                        .map(|ms| {
                            transfer_core::format_utc(ms)
                                .split(' ')
                                .next()
                                .unwrap_or_default()
                                .to_string()
                        })
                        .unwrap_or_default(),
                ]
            })
            .collect(),
        size_col: Some(1),
        row_bytes: stale
            .folders
            .iter()
            .map(|folder| folder.stale_bytes)
            .collect(),
        footnote,
        ..Table::default()
    }
}

pub(super) fn cleanup(report: &CleanReport, meta: Vec<(String, String)>) -> Document {
    Document {
        title: "Quick cleanup".into(),
        subtitle: format!("Freed {}", format_bytes(report.freed())),
        meta,
        verdict: Some(Verdict::Good),
        tiles: vec![tile(
            "Freed",
            format_bytes(report.freed()),
            plural(
                report.items.iter().map(|item| item.removed_files).sum(),
                "file",
            ) + " removed",
            Tone::Good,
        )],
        notes: vec!["Files in use or changed in the last day are left in place.".into()],
        share: (!report.items.is_empty()).then(|| {
            let total = report.items.len() as u64;
            let clean = report
                .items
                .iter()
                .filter(|item| item.error.is_none())
                .count() as u64;
            Share {
                label: "Locations cleaned without error".into(),
                detail: format!("{} of {}", format_count(clean), format_count(total)),
                percent: clean as f64 * 100.0 / total as f64,
                tone: if clean == total {
                    Tone::Good
                } else {
                    Tone::Warn
                },
            }
        }),
        tables: vec![Table {
            heading: "Cleaned".into(),
            columns: vec![
                "Item".into(),
                "Freed".into(),
                "Files removed".into(),
                "Files left".into(),
            ],
            numeric: vec![false, true, true, true],
            rows: report
                .items
                .iter()
                .map(|item| {
                    vec![
                        match &item.error {
                            Some(error) => format!("{} ({error})", item.title),
                            None => item.title.clone(),
                        },
                        format_bytes(item.freed),
                        format_count(item.removed_files),
                        format_count(item.skipped_files),
                    ]
                })
                .collect(),
            size_col: Some(1),
            row_bytes: report.items.iter().map(|item| item.freed).collect(),
            ..Table::default()
        }],
        ..Document::default()
    }
}
