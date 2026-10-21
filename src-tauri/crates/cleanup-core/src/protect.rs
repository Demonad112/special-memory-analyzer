use std::env;
use std::path::Path;

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
        let var = |name: &str| env::var(name).ok().filter(|value| !value.is_empty());
        let join = |base: Option<String>, tail: &str| base.map(|base| format!("{base}\\{tail}"));
        let drive = var("SystemDrive").or_else(|| Some("C:".into()));
        let profile = var("USERPROFILE");

        let portable = scan_core::portable_data_dir().map(|dir| dir.to_string_lossy().into_owned());
        let inside = [
            portable,
            var("SystemRoot"),
            var("ProgramFiles"),
            var("ProgramFiles(x86)"),
            var("ProgramW6432"),
            join(drive.clone(), "Recovery"),
            join(drive.clone(), "Boot"),
            // Record reports are client proof.
            join(var("LOCALAPPDATA"), "Deep Server Analyzer"),
        ];
        let mut exact = vec![
            var("ProgramData"),
            var("PUBLIC"),
            var("APPDATA"),
            var("LOCALAPPDATA"),
            var("OneDrive"),
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
        protected
    }

    /// Ok when `path` may be deleted: it is inside the scanned folder and nothing Windows needs.
    pub fn check(&self, path: &Path, scan_root: &Path) -> Result<(), String> {
        let path = normalize(&path.to_string_lossy());
        let root = normalize(&scan_root.to_string_lossy());
        if path == root || !within(&path, &root) {
            return Err("Only items inside the scanned folder can be deleted.".into());
        }
        let name = path.rsplit('\\').next().unwrap_or_default();
        let system_folder = path
            .split('\\')
            .any(|part| SYSTEM_NAMES[..2].contains(&part));
        if system_folder || SYSTEM_NAMES.contains(&name) {
            return Err(KEPT.into());
        }
        let protected = self.inside.iter().any(|base| within(&path, base))
            || self.exact.contains(&path)
            || self
                .children_of
                .iter()
                .any(|base| parent(&path) == Some(base.as_str()));
        if protected {
            return Err(KEPT.into());
        }
        Ok(())
    }
}

/// Lowercase, backslashes, no trailing backslash, no `\\?\` prefix.
fn normalize(path: &str) -> String {
    let path = path.trim().replace('/', "\\").to_lowercase();
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
        assert!(check(r"C:\Windows.old", r"C:\").is_ok());
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
}
