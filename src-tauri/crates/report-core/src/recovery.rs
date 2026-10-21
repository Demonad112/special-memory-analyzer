//! The "Copy the missing files again" block of a Compare report: a PowerShell script with one
//! robocopy line per source folder, built from the missing list.
//!
//! Every path is quoted here, in Rust. The report page only filters and joins the pieces (to
//! apply the Exclude presets), so it never has to quote anything itself.

use crate::doc::{Recovery, Section};
use crate::kinds::MissingFile;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Folders the text box in the report shows; the downloaded script always has every folder.
pub(crate) const MAX_SCRIPT_FOLDERS: usize = 5_000;
/// A robocopy line is cut here (in UTF-16 units, as the page script counts) and continued on a
/// new line, so no line gets near the 32,767 character limit of a command line.
const LINE_LIMIT: usize = 6_000;
const SUFFIX: &str = " /COPY:DATSO /DCOPY:DAT /R:2 /W:1 /XJ /NP";
const CHECK: &str = "if ($LASTEXITCODE -ge 8) { $failed++ }";
/// The folder the second command copies into, as a PowerShell expression.
pub(crate) const DOWNLOADS_BASE: &str =
    "Join-Path $env:USERPROFILE 'Downloads\\Missed File Transfers'";

/// Folder names that are usually not worth copying again. Index = bit in a file's mask.
const PRESETS: [(&str, &[&str]); 5] = [
    ("node_modules", &["node_modules"]),
    (".git", &[".git"]),
    ("__pycache__", &["__pycache__"]),
    ("bin and obj", &["bin", "obj"]),
    (".vs", &[".vs"]),
];

/// What the second script does besides copying; tests change it.
#[derive(Clone, Debug)]
pub(crate) struct Options {
    /// PowerShell expression for the folder the second script copies into.
    pub pass2_base: String,
    /// End the second script by opening the folder in Explorer.
    pub open_explorer: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pass2_base: DOWNLOADS_BASE.into(),
            open_explorer: true,
        }
    }
}

struct Folder {
    /// Relative to the source, backslash separated; empty for the source folder itself.
    dir: String,
    /// File name and preset mask.
    files: Vec<(String, u8)>,
}

pub(crate) struct Plan {
    source: String,
    destination: String,
    folders: Vec<Folder>,
    /// Empty folders to create, with their preset mask.
    empty: Vec<(String, u8)>,
    /// Paths a script cannot carry safely, ready to show.
    pub(crate) manual: Vec<String>,
    /// Files and bytes behind each preset.
    counts: [(u64, u64); PRESETS.len()],
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    options: Options,
}

/// `text` as a PowerShell single-quoted string. Nothing inside expands, so `$`, backticks, `&`
/// and `()` are inert; only the quote itself needs doubling. PowerShell also reads the curly
/// quotes as quotes, so those are doubled too.
pub(crate) fn ps_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for ch in text.chars() {
        out.push(ch);
        if matches!(ch, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// `root` without its trailing separators: a quoted path must never end in a backslash, which
/// Windows PowerShell hands to robocopy as an escaped quote.
fn trim_root(root: &str) -> String {
    root.trim().trim_end_matches('\\').to_string()
}

/// `root` and `dir` joined. A drive root (`C:`) has no name to join to, so it becomes `C:\.`.
fn join(root: &str, dir: &str) -> String {
    match (dir.is_empty(), root.ends_with(':')) {
        (true, true) => format!("{root}\\."),
        (true, false) => root.to_string(),
        (false, _) => format!("{root}\\{dir}"),
    }
}

/// A path the script cannot carry: control characters or line separators in a name, or a part
/// that is empty, `.` or `..` (which could leave the folder).
fn unsafe_part(part: &str) -> bool {
    part.is_empty()
        || part == "."
        || part == ".."
        || part
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
}

/// A path for the "copy by hand" list: control characters shown as `<0x0A>`.
fn display(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for ch in path.chars() {
        if ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}') {
            out.push_str(&format!("<0x{:02X}>", u32::from(ch)));
        } else {
            out.push(ch);
        }
    }
    out
}

fn mask_of(dirs: &[&str]) -> u8 {
    let mut mask = 0;
    for (bit, (_, names)) in PRESETS.iter().enumerate() {
        if dirs
            .iter()
            .any(|dir| names.iter().any(|name| dir.eq_ignore_ascii_case(name)))
        {
            mask |= 1 << bit;
        }
    }
    mask
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

impl Plan {
    /// `None` when a folder is unknown or nothing is missing.
    pub(crate) fn new(
        source: &str,
        destination: &str,
        missing: &[MissingFile],
        options: Options,
    ) -> Option<Self> {
        let (source, destination) = (trim_root(source), trim_root(destination));
        if source.is_empty() || destination.is_empty() || missing.is_empty() {
            return None;
        }
        let mut by_dir: BTreeMap<String, Vec<(String, u8)>> = BTreeMap::new();
        let mut plan = Self {
            source,
            destination,
            folders: Vec::new(),
            empty: Vec::new(),
            manual: Vec::new(),
            counts: [(0, 0); PRESETS.len()],
            files: 0,
            bytes: 0,
            options,
        };
        for entry in missing {
            let is_folder = entry.path.ends_with('\\');
            let parts: Vec<&str> = entry.path.trim_end_matches('\\').split('\\').collect();
            if parts.iter().any(|part| unsafe_part(part)) {
                plan.manual.push(display(&entry.path));
                continue;
            }
            if is_folder {
                let mask = mask_of(&parts);
                plan.empty.push((parts.join("\\"), mask));
                continue;
            }
            let (name, dirs) = parts.split_last()?;
            // robocopy reads a file name starting with `-` as an option and has no way to stop that.
            if name.starts_with('-') {
                plan.manual.push(display(&entry.path));
                continue;
            }
            let mask = mask_of(dirs);
            for (bit, count) in plan.counts.iter_mut().enumerate() {
                if mask & (1 << bit) != 0 {
                    count.0 += 1;
                    count.1 += entry.size;
                }
            }
            plan.files += 1;
            plan.bytes += entry.size;
            by_dir
                .entry(dirs.join("\\"))
                .or_default()
                .push(((*name).to_string(), mask));
        }
        plan.folders = by_dir
            .into_iter()
            .map(|(dir, files)| Folder { dir, files })
            .collect();
        (plan.files > 0 || !plan.empty.is_empty() || !plan.manual.is_empty()).then_some(plan)
    }

    /// `(bit, label)` of the presets that match something, with their file counts.
    fn presets_shown(&self) -> Vec<(usize, String)> {
        PRESETS
            .iter()
            .enumerate()
            .filter(|(bit, _)| self.counts[*bit].0 > 0)
            .map(|(bit, (label, _))| {
                let (files, _) = self.counts[bit];
                let noun = if files == 1 { "file" } else { "files" };
                (bit, format!("{label}: {files} {noun}"))
            })
            .collect()
    }

    fn pre(&self, pass2: bool) -> String {
        if !pass2 {
            return "# Deep Server Analyzer: copy the missing files again. Paste this into PowerShell.\n$failed = 0"
                .into();
        }
        format!(
            "# Deep Server Analyzer: copy the missing files into a folder on this PC. Paste this into PowerShell.\n\
             $base = {}\n\
             $stamp = Get-Date -Format 'yyyy-MM-dd HHmm'\n\
             $out = Join-Path $base $stamp\n\
             try {{ [System.IO.Directory]::CreateDirectory($out) | Out-Null }}\n\
             catch {{\n\
             \x20 $out = Join-Path (Join-Path $env:TEMP 'Missed File Transfers') $stamp\n\
             \x20 [System.IO.Directory]::CreateDirectory($out) | Out-Null\n\
             }}\n\
             $failed = 0",
            self.options.pass2_base
        )
    }

    fn post(&self, pass2: bool) -> String {
        let mut out = String::from(
            "if ($failed -gt 0) { Write-Host \"$failed step(s) reported errors. Read the messages above.\" -ForegroundColor Red } else { Write-Host 'Done. No errors.' -ForegroundColor Green }",
        );
        if pass2 {
            out.push_str("\nWrite-Host \"Copied into: $out\"");
            if self.options.open_explorer {
                out.push_str("\nif (Test-Path -LiteralPath $out) { explorer.exe $out }");
            }
        }
        out
    }

    /// Where `dir` goes: its quoted path at the destination, or inside `$out` in the second script.
    fn target(&self, dir: &str, pass2: bool) -> String {
        match (pass2, dir.is_empty()) {
            (false, _) => ps_quote(&join(&self.destination, dir)),
            (true, true) => "$out".into(),
            (true, false) => format!("(Join-Path $out {})", ps_quote(dir)),
        }
    }

    fn head(&self, dir: &str, pass2: bool) -> String {
        format!(
            "robocopy {} {}",
            ps_quote(&join(&self.source, dir)),
            self.target(dir, pass2)
        )
    }

    fn make_dir(&self, dir: &str, pass2: bool) -> String {
        let target = self.target(dir, pass2);
        format!("try {{ [System.IO.Directory]::CreateDirectory({target}) | Out-Null }} catch {{ $failed++ }}")
    }

    /// The script without the files `mask` leaves out, and whether `max_folders` cut it short.
    pub(crate) fn script(
        &self,
        mask: u8,
        pass2: bool,
        max_folders: Option<usize>,
    ) -> (String, bool) {
        let mut lines = vec![self.pre(pass2)];
        let (mut shown, mut cut) = (0, false);
        for folder in &self.folders {
            let names: Vec<String> = folder
                .files
                .iter()
                .filter(|(_, bits)| bits & mask == 0)
                .map(|(name, _)| ps_quote(name))
                .collect();
            if names.is_empty() {
                continue;
            }
            if max_folders.is_some_and(|max| shown >= max) {
                cut = true;
                continue;
            }
            shown += 1;
            let head = self.head(&folder.dir, pass2);
            let mut line = head.clone();
            for name in names {
                if utf16_len(&line) + utf16_len(&name) > LINE_LIMIT && line != head {
                    lines.push(format!("{line}{SUFFIX}"));
                    lines.push(CHECK.into());
                    line = head.clone();
                }
                line.push(' ');
                line.push_str(&name);
            }
            lines.push(format!("{line}{SUFFIX}"));
            lines.push(CHECK.into());
        }
        for (dir, bits) in &self.empty {
            if bits & mask == 0 {
                lines.push(self.make_dir(dir, pass2));
            }
        }
        lines.push(self.post(pass2));
        let mut text = lines.join("\n");
        text.push('\n');
        (text, cut)
    }

    /// What the page script needs to rebuild the text when presets are ticked.
    pub(crate) fn data(&self) -> Value {
        let dirs: Vec<Value> = self
            .folders
            .iter()
            .map(|folder| {
                let files: Vec<Value> = folder
                    .files
                    .iter()
                    .map(|(name, bits)| json!([ps_quote(name), bits]))
                    .collect();
                json!({
                    "h": self.head(&folder.dir, false),
                    "h2": self.head(&folder.dir, true),
                    "f": files,
                })
            })
            .collect();
        let make: Vec<Value> = self
            .empty
            .iter()
            .map(|(dir, bits)| json!([self.make_dir(dir, false), self.make_dir(dir, true), bits]))
            .collect();
        json!({
            "pre": self.pre(false),
            "pre2": self.pre(true),
            "post": self.post(false),
            "post2": self.post(true),
            "sfx": SUFFIX,
            "chk": CHECK,
            "limit": MAX_SCRIPT_FOLDERS,
            "max": LINE_LIMIT,
            "dirs": dirs,
            "mk": make,
        })
    }

    pub(crate) fn section(&self, intro: String) -> Section {
        let (script, truncated) = self.script(0, false, Some(MAX_SCRIPT_FOLDERS));
        let (script_pass2, _) = self.script(0, true, Some(MAX_SCRIPT_FOLDERS));
        Section::Recovery(Recovery {
            heading: "Copy the missing files again".into(),
            intro,
            script,
            script_pass2,
            truncated,
            presets: self.presets_shown(),
            manual: self.manual.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64) -> MissingFile {
        MissingFile {
            path: path.into(),
            size,
        }
    }

    fn plan(source: &str, destination: &str, files: &[MissingFile]) -> Plan {
        Plan::new(source, destination, files, Options::default()).expect("a plan")
    }

    #[test]
    fn quoting_matrix() {
        let cases = [
            ("plain.txt", "'plain.txt'"),
            ("it's.txt", "'it''s.txt'"),
            ("two''quotes", "'two''''quotes'"),
            ("$(calc).txt", "'$(calc).txt'"),
            ("a`b`.txt", "'a`b`.txt'"),
            ("a&b;c|d.txt", "'a&b;c|d.txt'"),
            ("100%.txt", "'100%.txt'"),
            ("bang!^.txt", "'bang!^.txt'"),
            ("file[1].txt", "'file[1].txt'"),
            ("emoji 📁.txt", "'emoji 📁.txt'"),
            (
                "curly\u{2018}a\u{2019}.txt",
                "'curly\u{2018}\u{2018}a\u{2019}\u{2019}.txt'",
            ),
            (
                "low\u{201a}high\u{201b}.txt",
                "'low\u{201a}\u{201a}high\u{201b}\u{201b}.txt'",
            ),
            ("", "''"),
        ];
        for (input, expected) in cases {
            assert_eq!(ps_quote(input), expected, "quoting {input:?}");
        }
    }

    #[test]
    fn roots_never_end_with_a_backslash() {
        assert_eq!(trim_root(r"C:\Data\"), r"C:\Data");
        assert_eq!(trim_root(r"C:\"), "C:");
        assert_eq!(trim_root(r"\\nas\share\\"), r"\\nas\share");
        assert_eq!(join("C:", ""), r"C:\.");
        assert_eq!(join("C:", "a"), r"C:\a");
        assert_eq!(join(r"C:\Data", ""), r"C:\Data");
        assert_eq!(join(r"\\nas\share", r"a\b"), r"\\nas\share\a\b");
    }

    #[test]
    fn one_robocopy_line_per_source_folder() {
        let plan = plan(
            r"C:\Data",
            r"E:\Backup",
            &[
                file(r"Docs\b.txt", 10),
                file("root.txt", 5),
                file(r"Docs\a b.txt", 20),
                file(r"Docs\it's.txt", 1),
            ],
        );
        let (text, cut) = plan.script(0, false, None);
        assert!(!cut);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], "$failed = 0");
        assert_eq!(
            lines[2],
            r"robocopy 'C:\Data' 'E:\Backup' 'root.txt' /COPY:DATSO /DCOPY:DAT /R:2 /W:1 /XJ /NP"
        );
        assert_eq!(lines[3], CHECK);
        assert_eq!(
            lines[4],
            r"robocopy 'C:\Data\Docs' 'E:\Backup\Docs' 'b.txt' 'a b.txt' 'it''s.txt' /COPY:DATSO /DCOPY:DAT /R:2 /W:1 /XJ /NP"
        );
        assert!(lines.last().unwrap().contains("$failed -gt 0"));
        assert_eq!((plan.files, plan.bytes), (4, 36));
    }

    #[test]
    fn drive_roots_use_a_dot_and_unc_roots_lose_the_slash() {
        let plan = plan(r"C:\", r"\\nas\backup\", &[file("a.txt", 1)]);
        let (text, _) = plan.script(0, false, None);
        assert!(
            text.contains(r"robocopy 'C:\.' '\\nas\backup' 'a.txt'"),
            "{text}"
        );
        assert!(!text.contains("\\'"), "a quoted path ends in a backslash");
    }

    #[test]
    fn names_a_script_cannot_carry_are_listed_for_hand_copying() {
        let plan = plan(
            r"C:\Data",
            r"E:\Backup",
            &[
                file("ok.txt", 1),
                file("line\nbreak.txt", 1),
                file("tab\tbed.txt", 1),
                file("sep\u{2028}line.txt", 1),
                file("nul\u{0}.txt", 1),
                file(r"a\..\escape.txt", 1),
                file(r"a\\double.txt", 1),
                file("ctrl\u{7}bell\\inner.txt", 1),
            ],
        );
        assert_eq!(plan.files, 1);
        assert_eq!(plan.manual.len(), 7, "{:?}", plan.manual);
        assert!(plan.manual.contains(&"line<0x0A>break.txt".to_string()));
        assert!(plan.manual.contains(&"tab<0x09>bed.txt".to_string()));
        let (text, _) = plan.script(0, false, None);
        assert!(text.contains("'ok.txt'"));
        assert!(!text.contains("break") && !text.contains("escape") && !text.contains("bell"));
        for line in text.lines() {
            assert!(!line.contains('\u{2028}'));
        }
    }

    #[test]
    fn a_name_starting_with_a_dash_is_listed_for_hand_copying() {
        let plan = plan(
            r"C:\Data",
            r"E:\Backup",
            &[
                file("-rf.txt", 1),
                file(r"-dir\ok.txt", 1),
                file("a-b.txt", 1),
            ],
        );
        assert_eq!(plan.manual, vec!["-rf.txt".to_string()]);
        assert_eq!(plan.files, 2);
        let (text, _) = plan.script(0, false, None);
        assert!(text.contains(r"robocopy 'C:\Data\-dir'"));
        assert!(!text.contains("'-rf.txt'"));
    }

    #[test]
    fn presets_count_files_and_leave_them_out() {
        let plan = plan(
            r"C:\Data",
            r"E:\Backup",
            &[
                file(r"app\node_modules\x\index.js", 100),
                file(r"app\Node_Modules\y.js", 50),
                file(r"app\src\main.rs", 10),
                file(r"app\.git\HEAD", 1),
                file(r"app\bin\app.exe", 1),
                file(r"app\obj\app.o", 1),
                file("node_modules.txt", 1),
                file(r"app\__pycache__\a.pyc", 1),
                file(r"app\.vs\state", 1),
                file(r"empty\node_modules\", 0),
            ],
        );
        let shown = plan.presets_shown();
        assert_eq!(shown[0], (0, "node_modules: 2 files".to_string()));
        assert_eq!(shown[1], (1, ".git: 1 file".to_string()));
        assert_eq!(shown[3], (3, "bin and obj: 2 files".to_string()));
        assert_eq!(plan.counts[0], (2, 150));

        let (all, _) = plan.script(0, false, None);
        assert!(
            all.contains("'index.js'")
                && all.contains("CreateDirectory('E:\\Backup\\empty\\node_modules')")
        );
        let (without, _) = plan.script(0b11111, false, None);
        for gone in [
            "index.js",
            "HEAD",
            "app.exe",
            "a.pyc",
            "state",
            "CreateDirectory",
        ] {
            assert!(!without.contains(gone), "{gone} should be left out");
        }
        assert!(without.contains("'main.rs'") && without.contains("'node_modules.txt'"));
    }

    #[test]
    fn long_folders_are_split_into_several_lines() {
        let files: Vec<MissingFile> = (0..400)
            .map(|index| file(&format!("a-rather-long-file-name-number-{index:04}.dat"), 1))
            .collect();
        let plan = plan(r"C:\S", r"D:\T", &files);
        let (text, _) = plan.script(0, false, None);
        let robocopy: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("robocopy"))
            .collect();
        assert!(robocopy.len() > 1);
        for line in &robocopy {
            assert!(
                line.len() < LINE_LIMIT + 200,
                "line of {} chars",
                line.len()
            );
            assert!(line.ends_with(SUFFIX));
        }
        let names = text.matches("a-rather-long").count();
        assert_eq!(names, 400, "every name appears exactly once");
        assert_eq!(text.matches(CHECK).count(), robocopy.len());
    }

    #[test]
    fn the_text_box_stops_at_5000_folders_but_the_data_keeps_all() {
        let files: Vec<MissingFile> = (0..MAX_SCRIPT_FOLDERS + 7)
            .map(|index| file(&format!("d{index:05}\\f.txt"), 1))
            .collect();
        let plan = plan(r"C:\S", r"D:\T", &files);
        let (text, cut) = plan.script(0, false, Some(MAX_SCRIPT_FOLDERS));
        assert!(cut);
        assert_eq!(text.matches("robocopy").count(), MAX_SCRIPT_FOLDERS);
        let (all, cut) = plan.script(0, false, None);
        assert!(!cut);
        assert_eq!(all.matches("robocopy").count(), MAX_SCRIPT_FOLDERS + 7);
        assert_eq!(
            plan.data()["dirs"].as_array().unwrap().len(),
            MAX_SCRIPT_FOLDERS + 7
        );
    }

    #[test]
    fn the_second_script_copies_into_a_folder_on_this_pc() {
        let plan = plan(
            r"C:\Data",
            r"E:\Backup",
            &[
                file("root.txt", 1),
                file(r"Docs\a.txt", 1),
                file(r"Empty\", 0),
            ],
        );
        let (text, _) = plan.script(0, true, None);
        assert!(
            text.contains("$base = Join-Path $env:USERPROFILE 'Downloads\\Missed File Transfers'")
        );
        assert!(text.contains("Get-Date -Format 'yyyy-MM-dd HHmm'"));
        assert!(text.contains("$env:TEMP"));
        assert!(text.contains(r"robocopy 'C:\Data' $out 'root.txt'"));
        assert!(text.contains(r"robocopy 'C:\Data\Docs' (Join-Path $out 'Docs') 'a.txt'"));
        assert!(text.contains("CreateDirectory((Join-Path $out 'Empty'))"));
        assert!(text.trim_end().ends_with("explorer.exe $out }"));
        assert!(!plan.script(0, false, None).0.contains("explorer"));
    }

    #[test]
    fn nothing_missing_or_an_unknown_folder_gives_no_plan() {
        assert!(Plan::new("C:\\A", "D:\\B", &[], Options::default()).is_none());
        assert!(Plan::new("", "D:\\B", &[file("a", 1)], Options::default()).is_none());
        assert!(Plan::new("C:\\A", "  ", &[file("a", 1)], Options::default()).is_none());
    }

    /// Runs the generated script under Windows PowerShell against a tree of awkward names and
    /// checks every file arrives with its content.
    #[cfg(windows)]
    mod on_windows {
        use super::*;
        use std::fs;
        use std::path::Path;
        use std::process::Command;

        const AWKWARD: &[&str] = &[
            "plain.txt",
            "it's.txt",
            "two'' quotes.txt",
            "$(calc).txt",
            "back`tick`.txt",
            "a&b;c.txt",
            "100%.txt",
            "bang!^.txt",
            "file[1].txt",
            "(paren).txt",
            "{brace}.txt",
            "#hash.txt",
            "@at.txt",
            "=equals.txt",
            "mid-dash.txt",
            "emoji \u{1F4C1}.txt",
            "curly\u{2018}quote\u{2019}.txt",
            "low\u{201a}high\u{201b}.txt",
            "caf\u{e9} \u{4e2d}\u{6587}.txt",
            "UPPER and lower.TXT",
        ];

        fn write(root: &Path, relative: &str, content: &str) {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn run_script(dir: &Path, text: &str) -> String {
            let path = dir.join("fix.ps1");
            // Windows PowerShell reads a script without a BOM as ANSI.
            fs::write(&path, format!("\u{feff}{}", text.replace('\n', "\r\n"))).unwrap();
            let output = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(&path)
                .output()
                .expect("powershell");
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            assert!(
                output.status.success(),
                "powershell failed: {stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            stdout
        }

        fn hostile_tree(source: &Path) -> Vec<MissingFile> {
            let mut missing = Vec::new();
            let mut add = |relative: String| {
                write(source, &relative, &format!("content of {relative}"));
                missing.push(file(&relative, 1));
            };
            for name in AWKWARD {
                add((*name).to_string());
                add(format!("sub dir\\{name}"));
                add(format!("it's [odd] $dir\\deep er\\{name}"));
            }
            add(format!("{}.txt", "x".repeat(200)));
            add(format!(
                "{}\\{}\\leaf.txt",
                "d".repeat(100),
                "e".repeat(100)
            ));
            add("node_modules\\pkg\\index.js".into());
            fs::create_dir_all(source.join("empty [folder]\\it's here")).unwrap();
            missing.push(file("empty [folder]\\it's here\\", 0));
            missing
        }

        fn assert_arrived(source: &Path, destination: &Path, missing: &[MissingFile]) {
            for entry in missing {
                let from = source.join(&entry.path);
                let to = destination.join(&entry.path);
                if entry.path.ends_with('\\') {
                    assert!(to.is_dir(), "empty folder missing: {}", entry.path);
                } else {
                    assert_eq!(
                        fs::read_to_string(&to).unwrap_or_else(|_| format!("MISSING {to:?}")),
                        fs::read_to_string(&from).unwrap(),
                        "{}",
                        entry.path
                    );
                }
            }
        }

        #[test]
        fn the_script_copies_every_file_of_a_hostile_tree() {
            let base = tempfile::tempdir().unwrap();
            let source = base.path().join("src [tree]");
            let destination = base.path().join("dst it's");
            fs::create_dir_all(&source).unwrap();
            fs::create_dir_all(&destination).unwrap();
            let missing = hostile_tree(&source);
            let plan = plan(
                &format!("{}\\", source.display()),
                &destination.display().to_string(),
                &missing,
            );
            let (text, _) = plan.script(0, false, None);
            let stdout = run_script(base.path(), &text);
            assert!(stdout.contains("Done. No errors."), "{stdout}");
            assert_arrived(&source, &destination, &missing);
        }

        #[test]
        fn presets_leave_files_out_and_the_second_script_copies_into_a_new_folder() {
            let base = tempfile::tempdir().unwrap();
            let source = base.path().join("src");
            fs::create_dir_all(&source).unwrap();
            let missing = hostile_tree(&source);
            let target = base.path().join("Missed Files");
            let options = Options {
                pass2_base: ps_quote(&target.display().to_string()),
                open_explorer: false,
            };
            let plan = Plan::new(
                &source.display().to_string(),
                &base.path().join("unused").display().to_string(),
                &missing,
                options,
            )
            .unwrap();
            let (text, _) = plan.script(0b00001, true, None);
            let stdout = run_script(base.path(), &text);
            assert!(stdout.contains("Done. No errors."), "{stdout}");
            let stamp = fs::read_dir(&target)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let kept: Vec<MissingFile> = missing
                .iter()
                .filter(|entry| !entry.path.starts_with("node_modules"))
                .cloned()
                .collect();
            assert_arrived(&source, &stamp, &kept);
            assert!(!stamp.join("node_modules").exists());
        }
    }
}
