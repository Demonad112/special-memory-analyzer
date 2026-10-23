use std::env;
use std::path::{Path, PathBuf};

/// Paths Disk Cleanup never deletes, whatever the user picks.
///
/// Paths are compared as Windows paths: case-insensitive, `/` and `\` alike.
#[derive(Clone, Debug, Default)]
pub struct Protected {
    /// These folders and everything in them (Windows, Program Files, ...).
    inside: Vec<String>,
    /// These folders themselves; what is in them may go (Users, the profile, Documents, ...).
    exact: Vec<String>,
    /// Every folder directly in these (each profile in Users).
    children_of: Vec<String>,
}

/// Names Windows keeps at a drive's root or in every drive.
const SYSTEM_NAMES: [&str; 5] = [
    "$recycle.bin",
    "system volume information",
    "pagefile.sys",
    "hiberfil.sys",
    "swapfile.sys",
];

const KEPT: &str =
    "Windows or an installed app needs this, so Deep Server Analyzer won't delete it.";

const WINDOWS_OLD: &str = "Windows keeps this so you can roll back. Remove it from Windows' own \
    Storage settings (Temporary files) or Disk Cleanup, not here.";

/// Folders in the profile that hold tool settings and toolchains. Always kept as folders, even
/// when they are not on disk to list.
const HOME_DOT_FOLDERS: [&str; 7] = [
    ".cache", ".cargo", ".rustup", ".claude", ".npm", ".config", ".ssh",
];

impl Protected {
    pub fn new(inside: &[&str], exact: &[&str], children_of: &[&str]) -> Self {
        let normal = |list: &[&str]| list.iter().map(|path| normalize(path)).collect();
        Self {
            inside: normal(inside),
            exact: normal(exact),
            children_of: normal(children_of),
        }
    }

    /// The protected folders on this PC, from the environment.
    pub fn for_this_pc() -> Self {
        Self::from_env(&|name| env::var(name).ok().filter(|value| !value.is_empty()))
    }

    /// [`Self::for_this_pc`] with the environment passed in, so tests don't touch the real one.
    fn from_env(var: &dyn Fn(&str) -> Option<String>) -> Self {
        let join = |base: Option<String>, tail: &str| base.map(|base| format!("{base}\\{tail}"));
        let drive = var("SystemDrive").or_else(|| Some("C:".into()));
        let profile = var("USERPROFILE");
        let local = var("LOCALAPPDATA");

        let portable = scan_core::portable_data_dir().map(|dir| dir.to_string_lossy().into_owned());
        let mut inside = vec![
            portable,
            var("SystemRoot"),
            var("ProgramFiles"),
            var("ProgramFiles(x86)"),
            var("ProgramW6432"),
            join(drive.clone(), "Recovery"),
            join(drive.clone(), "Boot"),
            // Record reports are client proof.
            join(local.clone(), "Deep Server Analyzer"),
            // Runtimes and app data other programs depend on. Their caches are not listed as
            // junk either, so nothing here is ever a cleanup target.
            join(var("APPDATA"), "Local"),
            join(local.clone(), "Packages"),
            join(local.clone(), "node"),
            join(local.clone(), "Python"),
            join(local.clone(), "Microsoft\\OneDrive"),
            join(local.clone(), "Google\\DriveFS"),
        ];
        // The sync folders hold the user's own files, and deleting there deletes them in the cloud.
        for name in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
            inside.push(var(name));
        }
        let mut exact = vec![
            var("ProgramData"),
            var("PUBLIC"),
            var("APPDATA"),
            local,
            profile.clone(),
        ];
        for folder in [
            "AppData",
            "Desktop",
            "Documents",
            "Downloads",
            "Music",
            "Pictures",
            "Videos",
        ] {
            exact.push(join(profile.clone(), folder));
        }
        // Tool settings and toolchains in the profile: the folder stays, what is inside may go.
        for folder in HOME_DOT_FOLDERS {
            exact.push(join(profile.clone(), folder));
        }
        for folder in dot_folders_in(profile.as_deref()) {
            exact.push(join(profile.clone(), &folder));
        }
        let children_of = [join(drive, "Users")];

        let present = |list: &[Option<String>]| -> Vec<String> {
            list.iter().flatten().map(|path| normalize(path)).collect()
        };
        let mut protected = Self {
            inside: present(&inside),
            exact: present(&exact),
            children_of: present(&children_of),
        };
        // The Users folder itself too.
        protected.exact.extend(protected.children_of.clone());
        // Short-name and `subst` spellings of the bases resolve to the same long path.
        for list in [
            &mut protected.inside,
            &mut protected.exact,
            &mut protected.children_of,
        ] {
            add_canonical(list);
        }
        protected
    }

    /// These folders and everything in them are kept too. For the app's own data folders, which
    /// the user can move with environment variables (records, reports and the deletion log).
    pub fn with_extra_inside(mut self, folders: impl IntoIterator<Item = PathBuf>) -> Self {
        for folder in folders {
            let folder = normalize(&folder.to_string_lossy());
            if !folder.is_empty() && !self.inside.contains(&folder) {
                self.inside.push(folder);
            }
        }
        self
    }

    /// Whether a normalized path is a protected folder, inside one, or a profile under Users.
    fn is_kept(&self, path: &str) -> bool {
        self.inside.iter().any(|base| within(path, base))
            || self.exact.iter().any(|base| base == path)
            || self
                .children_of
                .iter()
                .any(|base| parent(path) == Some(base.as_str()))
    }

    /// True for a folder that must never be a cleanup target itself: a protected folder, a
    /// profile, a drive root or the Windows folder. What is inside a protected base (such as
    /// `%SystemRoot%\Temp`) is not a root.
    pub fn is_protected_root(&self, path: &Path) -> bool {
        let normal = normalize(&path.to_string_lossy());
        let resolved = scan_core::canonical_root(path)
            .ok()
            .map(|path| normalize(&path.to_string_lossy()));
        [Some(normal), resolved].into_iter().flatten().any(|path| {
            let drive_root = path.len() == 2 && path.ends_with(':');
            drive_root
                || path.is_empty()
                || self.exact.contains(&path)
                || self.inside.contains(&path)
                || self
                    .children_of
                    .iter()
                    .any(|base| parent(&path) == Some(base.as_str()))
        })
    }

    /// Ok when `path` may be deleted: it is inside the scanned folder and nothing Windows needs.
    pub fn check(&self, path: &Path, scan_root: &Path) -> Result<(), String> {
        let scan_path = path;
        let path = normalize(&path.to_string_lossy());
        let root = normalize(&scan_root.to_string_lossy());
        if path.starts_with("globalroot") || path.starts_with("volume{") {
            return Err(KEPT.into());
        }
        if path == root || !within(&path, &root) {
            return Err("Only items inside the scanned folder can be deleted.".into());
        }
        if path.split('\\').any(|part| part == "windows.old") {
            return Err(WINDOWS_OLD.into());
        }
        let name = path.rsplit('\\').next().unwrap_or_default();
        let system_folder = path
            .split('\\')
            .any(|part| SYSTEM_NAMES[..2].contains(&part));
        if system_folder || SYSTEM_NAMES.contains(&name) {
            return Err(KEPT.into());
        }
        // Also by the name the parent resolves to, so 8.3 short names and `subst` drives match
        // the same protected folders as the long form.
        let resolved = resolved_parent(scan_path);
        if self.is_kept(&path) || resolved.as_deref().is_some_and(|path| self.is_kept(path)) {
            return Err(KEPT.into());
        }
        Ok(())
    }
}

/// Adds the canonical spelling of every folder that exists, keeping the raw one too.
fn add_canonical(list: &mut Vec<String>) {
    let extra: Vec<String> = list
        .iter()
        .filter_map(|base| scan_core::canonical_root(Path::new(base)).ok())
        .map(|path| normalize(&path.to_string_lossy()))
        .filter(|path| !list.contains(path))
        .collect();
    for path in extra {
        if !list.contains(&path) {
            list.push(path);
        }
    }
}

/// `path` with its parent folder resolved (short names, `subst`, links); the last name is kept
/// as written, so a link itself is judged as the link. None when the parent does not resolve.
fn resolved_parent(path: &Path) -> Option<String> {
    let name = path.file_name()?;
    let parent = scan_core::canonical_root(path.parent()?).ok()?;
    Some(normalize(&parent.join(name).to_string_lossy()))
}

/// Names of the folders in the profile that start with a dot.
fn dot_folders_in(profile: Option<&str>) -> Vec<String> {
    let Some(entries) = profile.and_then(|profile| std::fs::read_dir(profile).ok()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with('.'))
        .collect()
}

/// Lowercase, backslashes, no trailing backslash, no `\\?\` prefix.
fn normalize(path: &str) -> String {
    let path = path.trim().replace('/', "\\").to_lowercase();
    if let Some(rest) = path.strip_prefix("\\\\?\\unc\\") {
        return format!("\\\\{}", rest.trim_end_matches('\\'));
    }
    let path = path.strip_prefix("\\\\?\\").unwrap_or(&path);
    path.trim_end_matches('\\').to_string()
}

fn within(path: &str, base: &str) -> bool {
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('\\'))
}

fn parent(path: &str) -> Option<&str> {
    path.rfind('\\').map(|slash| &path[..slash])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn protected() -> Protected {
        Protected::new(
            &[r"C:\Windows", r"C:\Program Files"],
            &[r"C:\Users", r"C:\Users\Sam\Documents"],
            &[r"C:\Users"],
        )
    }

    fn check(path: &str, root: &str) -> Result<(), String> {
        protected().check(Path::new(path), Path::new(root))
    }

    #[test]
    fn ordinary_items_inside_the_scan_can_go() {
        assert!(check(r"C:\Users\Sam\Downloads\setup.exe", r"C:\").is_ok());
        assert!(check(r"C:\Users\Sam\Documents\old", r"C:\Users").is_ok());
        assert!(check(r"D:\Backups\2019", r"D:\").is_ok());
        assert!(check(r"C:\Windows.old", r"C:\").is_err());
        assert!(check(r"c:/temp/a.log", r"C:\TEMP").is_ok());
    }

    #[test]
    fn only_items_inside_the_scanned_folder() {
        assert!(check(r"C:\", r"C:\").is_err());
        assert!(check(r"D:\Data", r"D:\Data\").is_err());
        assert!(check(r"D:\Data2\x", r"D:\Data").is_err());
        assert!(check(r"E:\x", r"D:\").is_err());
    }

    #[test]
    fn windows_folders_and_files_are_kept() {
        assert!(check(r"C:\Windows", r"C:\").is_err());
        assert!(check(r"C:\WINDOWS\Temp\x.log", r"C:\").is_err());
        assert!(check(r"C:\Program Files\App", r"C:\").is_err());
        assert!(check(r"C:\Users", r"C:\").is_err());
        assert!(check(r"C:\Users\Sam", r"C:\").is_err());
        assert!(check(r"C:\Users\Sam\Documents", r"C:\").is_err());
        assert!(check(r"C:\pagefile.sys", r"C:\").is_err());
        assert!(check(r"D:\System Volume Information\x", r"D:\").is_err());
        assert!(check(r"D:\$Recycle.Bin", r"D:\").is_err());
        assert!(check(r"\\?\C:\Windows\System32", r"C:\").is_err());
    }

    fn wide() -> Protected {
        Protected::new(
            &[
                r"C:\Windows",
                r"C:\Program Files",
                r"C:\Program Files (x86)",
                r"C:\Recovery",
                r"C:\Users\Sam\AppData\Local\Deep Server Analyzer",
            ],
            &[
                r"C:\ProgramData",
                r"C:\Users\Public",
                r"C:\Users\Sam",
                r"C:\Users\Sam\AppData",
                r"C:\Users\Sam\AppData\Local",
                r"C:\Users\Sam\Downloads",
            ],
            &[r"C:\Users"],
        )
    }

    fn wide_check(path: &str) -> Result<(), String> {
        wide().check(Path::new(path), Path::new(r"C:\"))
    }

    #[test]
    fn other_users_profile_roots_are_kept_but_their_files_are_not_blanket_protected() {
        assert!(wide_check(r"C:\Users\Alex").is_err());
        assert!(wide_check(r"C:\USERS\alex\").is_err());
        assert!(wide_check(r"C:\Users\Default").is_err());
        assert!(wide_check(r"C:\Users\Alex\Downloads\old.iso").is_ok());
        assert!(wide_check(r"C:\Users\Sam\Downloads").is_err());
        assert!(wide_check(r"C:\Users\Sam\Downloads\old.iso").is_ok());
    }

    #[test]
    fn app_and_system_folders_are_kept_with_everything_inside() {
        for path in [
            r"C:\Program Files (x86)\Vendor\app.exe",
            r"C:\Recovery\WindowsRE",
            r"C:\Users\Sam\AppData\Local\Deep Server Analyzer",
            r"C:\Users\Sam\AppData\Local\Deep Server Analyzer\Records\run1\report.json",
            r"C:\ProgramData",
            r"C:\Users\Public",
            r"C:\Users\Sam\AppData\Local",
            r"C:\Users\Sam\AppData",
        ] {
            assert!(wide_check(path).is_err(), "{path} should be kept");
        }
        // Siblings that only share a prefix are ordinary.
        assert!(wide_check(r"C:\Program Files Old\x").is_ok());
        assert!(wide_check(r"C:\ProgramData\Vendor\cache").is_ok());
        assert!(wide_check(r"C:\Users\Sam\AppData\Local\Temp\x.tmp").is_ok());
        assert!(wide_check(r"C:\Users\Sam\AppData\Local\Deep Server AnalyzerOld\x").is_ok());
    }

    #[test]
    fn extra_inside_folders_are_kept_with_everything_in_them() {
        let protected = Protected::default().with_extra_inside([
            PathBuf::from(r"D:\Work\my-logs"),
            PathBuf::from(r"D:\Work\my-reports\"),
        ]);
        let root = Path::new(r"D:\Work");
        for path in [
            r"D:\Work\my-logs",
            r"D:\Work\my-logs\deletions.jsonl",
            r"D:\Work\MY-REPORTS\a\b.json",
        ] {
            assert!(protected.check(Path::new(path), root).is_err(), "{path}");
        }
        for path in [r"D:\Work\my-logs-old\x", r"D:\Work\other\x"] {
            assert!(protected.check(Path::new(path), root).is_ok(), "{path}");
        }
    }

    #[test]
    fn system_names_are_kept_at_any_depth() {
        assert!(wide_check(r"C:\Data\$Recycle.Bin").is_err());
        assert!(wide_check(r"C:\Data\hiberfil.sys").is_err());
        assert!(wide_check(r"C:\Data\System Volume Information").is_err());
    }

    #[test]
    fn this_pc_has_its_windows_folder_protected() {
        let this_pc = Protected::for_this_pc();
        assert!(!this_pc.children_of.is_empty());
        assert!(this_pc.exact.iter().any(|path| path.ends_with("\\users")));
    }

    #[test]
    fn this_pc_keeps_app_roots_onedrive_and_home_dot_folders_but_not_their_caches() {
        // Driven through the environment, so the real list in `for_this_pc` is what gets tested.
        let set = [
            ("USERPROFILE", r"C:\Users\Sam"),
            ("APPDATA", r"C:\Users\Sam\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\Users\Sam\AppData\Local"),
            ("OneDrive", r"C:\Users\Sam\OneDrive"),
        ];
        let before: Vec<_> = set
            .iter()
            .map(|(name, _)| (*name, env::var_os(name)))
            .collect();
        for (name, value) in set {
            env::set_var(name, value);
        }
        let this_pc = Protected::for_this_pc();
        for (name, value) in before {
            match value {
                Some(value) => env::set_var(name, value),
                None => env::remove_var(name),
            }
        }

        let root = Path::new(r"C:\");
        for path in [
            r"C:\Users\Sam\AppData\Roaming\Local\lightning-services\x",
            r"C:\Users\Sam\AppData\Local\Packages\Claude_x\LocalCache\y",
            r"C:\Users\Sam\AppData\Local\node\corepack\z",
            r"C:\Users\Sam\AppData\Local\Microsoft\OneDrive\settings\a.dat",
            r"C:\Users\Sam\AppData\Local\Google\DriveFS\x",
            r"C:\Users\Sam\OneDrive\Docs\a.docx",
            r"C:\Users\Sam\.cache",
            r"C:\Users\Sam\.cargo",
            r"C:\Users\Sam\.rustup",
            r"C:\Users\Sam\.claude",
            r"C:\Users\Sam\.npm",
            r"C:\Users\Sam\.config",
            r"C:\Users\Sam\.ssh",
        ] {
            assert!(
                this_pc.check(Path::new(path), root).is_err(),
                "{path} should be kept"
            );
        }
        // Sub-folders of a home dot-folder are what cleanup is for.
        for path in [
            r"C:\Users\Sam\AppData\Local\Temp\x.tmp",
            r"C:\Users\Sam\.cache\pip",
            r"C:\Users\Sam\.npm\_cacache\x",
        ] {
            assert!(
                this_pc.check(Path::new(path), root).is_ok(),
                "{path} should be deletable"
            );
        }
    }

    fn sam(name: &str) -> Option<String> {
        let value = match name {
            "USERPROFILE" => r"C:\Users\Sam",
            "APPDATA" => r"C:\Users\Sam\AppData\Roaming",
            "LOCALAPPDATA" => r"C:\Users\Sam\AppData\Local",
            "OneDrive" => r"C:\Users\Sam\OneDrive",
            _ => return None,
        };
        Some(value.into())
    }

    #[test]
    fn nothing_the_catalog_offers_is_kept_and_its_protected_neighbours_still_are() {
        use crate::catalog::{Base, Entry, REVIEW, SAFE};
        let this_pc = Protected::from_env(&sam);
        let root = Path::new(r"C:\");
        let resolve = |entry: &Entry| {
            let base = match entry.base {
                Base::LocalAppData => r"C:\Users\Sam\AppData\Local",
                Base::AppData => r"C:\Users\Sam\AppData\Roaming",
                Base::UserProfile => r"C:\Users\Sam",
                Base::SystemDrive => r"C:",
            };
            let parts: Vec<&str> = entry
                .path
                .iter()
                .map(|part| if *part == "*" { "Default" } else { part })
                .collect();
            format!(r"{base}\{}", parts.join(r"\"))
        };
        for entry in SAFE.iter().chain(REVIEW) {
            let path = resolve(entry);
            assert!(
                this_pc.check(Path::new(&path), root).is_ok(),
                "{path} is in the catalog, so it must stay deletable"
            );
            // What is inside an offered folder is offered with it.
            assert!(
                this_pc
                    .check(Path::new(&format!(r"{path}\x\y.bin")), root)
                    .is_ok(),
                "{path}"
            );
        }
        for path in [
            r"C:\Users\Sam\AppData\Local\Microsoft\OneDrive\Cache",
            r"C:\Users\Sam\AppData\Local\Packages",
            r"C:\Users\Sam\AppData\Local\Python\pythoncore\Lib",
            r"C:\Users\Sam\AppData\Roaming\Local",
            r"C:\Users\Sam\OneDrive",
            r"C:\Users\Sam\.cargo",
        ] {
            assert!(this_pc.check(Path::new(path), root).is_err(), "{path}");
        }
        // Sharing a prefix with a protected folder is not being inside it.
        assert!(this_pc
            .check(Path::new(r"C:\Users\Sam\AppData\Local\Packages2\x"), root)
            .is_ok());
        assert!(this_pc
            .check(Path::new(r"C:\Users\Sam\OneDrive Old\x"), root)
            .is_ok());
    }

    #[test]
    fn windows_old_is_handed_off_wherever_it_is() {
        for path in [
            r"C:\Windows.old",
            r"C:\windows.OLD\Users\x",
            r"D:\Windows.old",
        ] {
            let reason = check(path, path.get(..3).unwrap_or(r"C:\")).unwrap_err();
            assert!(reason.contains("Storage settings"), "{path}: {reason}");
        }
        assert!(check(r"C:\Windows.older\x", r"C:\").is_ok());
    }

    #[test]
    fn verbatim_unc_and_device_spellings_do_not_slip_past() {
        let protected = Protected::new(&[r"\\server\share\Windows"], &[], &[]);
        let root = Path::new(r"\\server\share");
        assert!(protected
            .check(Path::new(r"\\?\UNC\server\share\Windows\x"), root)
            .is_err());
        assert!(protected
            .check(Path::new(r"\\?\UNC\server\share\Data\x"), root)
            .is_ok());
        let any = Protected::default();
        for path in [
            r"\\?\GLOBALROOT\Device\HarddiskVolume3\Windows\x",
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\Windows\x",
        ] {
            assert!(any.check(Path::new(path), Path::new(path)).is_err());
            assert!(
                any.check(Path::new(path), Path::new(r"\\?\GLOBALROOT"))
                    .is_err(),
                "{path}"
            );
        }
    }

    #[test]
    fn a_protected_root_is_a_base_itself_not_a_path_inside_one() {
        let this_pc = Protected::from_env(&|name| match name {
            "SystemRoot" => Some(r"C:\Windows".into()),
            "USERPROFILE" => Some(r"C:\Users\Sam".into()),
            "LOCALAPPDATA" => Some(r"C:\Users\Sam\AppData\Local".into()),
            _ => sam(name),
        });
        for path in [
            r"C:\",
            r"C:",
            r"C:\Windows",
            r"C:\WINDOWS\",
            r"C:\Users",
            r"C:\Users\Sam",
            r"C:\Users\Alex",
            r"C:\Users\Sam\Documents",
            r"C:\Users\Sam\AppData\Local",
        ] {
            assert!(this_pc.is_protected_root(Path::new(path)), "{path}");
        }
        for path in [
            r"C:\Windows\Temp",
            r"C:\Users\Sam\AppData\Local\Temp",
            r"D:\Data",
        ] {
            assert!(!this_pc.is_protected_root(Path::new(path)), "{path}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn an_8_3_short_name_of_a_protected_folder_is_refused() {
        let protected = Protected::from_env(&|name| match name {
            "ProgramFiles" => Some(r"C:\Program Files".into()),
            _ => None,
        });
        let short = Path::new(r"C:\PROGRA~1\Vendor\app.exe");
        assert!(protected.check(short, Path::new(r"C:\PROGRA~1")).is_err());
        assert!(protected.check(short, Path::new(r"C:\")).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_subst_drive_onto_the_windows_folder_is_refused() {
        use std::process::Command;
        struct Subst(&'static str);
        impl Drop for Subst {
            fn drop(&mut self) {
                let _ = Command::new("subst").args([self.0, "/D"]).status();
            }
        }
        let letter = "Z:";
        let windows = env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        // A stale mapping from an earlier crashed run would make the next call fail.
        let _ = Command::new("subst").args([letter, "/D"]).status();
        let made = Command::new("subst").args([letter, &windows]).status();
        if !made.is_ok_and(|status| status.success()) {
            eprintln!("subst is not available here; skipping");
            return;
        }
        let _guard = Subst(letter);
        let protected =
            Protected::from_env(&|name| (name == "SystemRoot").then(|| windows.clone()));
        let item = Path::new(r"Z:\System32\x");
        assert!(protected.check(item, Path::new(r"Z:\")).is_err());
    }
}
