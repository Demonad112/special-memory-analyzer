//! Quick cleanups: a fixed list of places that only hold throw-away files (temp folders, browser
//! caches, the Windows Update download cache, the Recycle Bin, old setup files in Downloads).
//! Nothing outside these places is ever touched, and the places themselves are kept.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuickId {
    UserTemp,
    WindowsTemp,
    BrowserCache,
    UpdateCache,
    RecycleBin,
    OldInstallers,
    InternetCache,
    CrashReports,
    ShaderCaches,
    Playwright,
}

pub const ALL: [QuickId; 10] = [
    QuickId::UserTemp,
    QuickId::BrowserCache,
    QuickId::RecycleBin,
    QuickId::OldInstallers,
    QuickId::WindowsTemp,
    QuickId::UpdateCache,
    QuickId::InternetCache,
    QuickId::CrashReports,
    QuickId::ShaderCaches,
    QuickId::Playwright,
];

/// A quick cleanup card: what it is and how much it would free now.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickWin {
    pub id: QuickId,
    pub title: &'static str,
    pub description: &'static str,
    pub needs_admin: bool,
    pub size: u64,
    pub files: u64,
}

/// What one cleanup freed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cleaned {
    pub freed: u64,
    pub removed: u64,
    /// Files left: in use, too new, or not allowed.
    pub skipped: u64,
}

/// Temp files this new may belong to a setup that is still running.
const RECENT: Duration = Duration::from_secs(24 * 60 * 60);
/// Setup files in Downloads older than this count as old.
const OLD_INSTALLER: Duration = Duration::from_secs(90 * 24 * 60 * 60);
const INSTALLER_EXTENSIONS: [&str; 5] = ["exe", "msi", "msix", "msixbundle", "appx"];

/// The folders the quick cleanups work in, from the environment (tests use their own).
#[derive(Clone, Debug, Default)]
pub struct Places {
    pub temp: Option<PathBuf>,
    pub system_root: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub profile: Option<PathBuf>,
}

impl Places {
    pub fn for_this_pc() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            temp: var("TEMP"),
            system_root: var("SystemRoot"),
            local_app_data: var("LOCALAPPDATA"),
            profile: var("USERPROFILE"),
        }
    }

    /// The folders one cleanup empties. Only folders that exist are returned.
    pub fn targets(&self, id: QuickId) -> Vec<PathBuf> {
        let join = |base: &Option<PathBuf>, tail: &[&str]| {
            base.as_ref()
                .map(|base| tail.iter().fold(base.clone(), |path, part| path.join(part)))
        };
        let list: Vec<PathBuf> = match id {
            QuickId::UserTemp => self.temp.clone().into_iter().collect(),
            QuickId::WindowsTemp => join(&self.system_root, &["Temp"]).into_iter().collect(),
            QuickId::UpdateCache => join(&self.system_root, &["SoftwareDistribution", "Download"])
                .into_iter()
                .collect(),
            QuickId::OldInstallers => join(&self.profile, &["Downloads"]).into_iter().collect(),
            QuickId::RecycleBin => Vec::new(),
            QuickId::InternetCache => {
                join(&self.local_app_data, &["Microsoft", "Windows", "INetCache"])
                    .into_iter()
                    .collect()
            }
            QuickId::CrashReports => [&["Microsoft", "Windows", "WER"][..], &["CrashDumps"]]
                .into_iter()
                .filter_map(|tail| join(&self.local_app_data, tail))
                .collect(),
            QuickId::ShaderCaches => [
                &["D3DSCache"][..],
                &["NVIDIA", "DXCache"],
                &["NVIDIA", "GLCache"],
            ]
            .into_iter()
            .filter_map(|tail| join(&self.local_app_data, tail))
            .collect(),
            QuickId::Playwright => join(&self.local_app_data, &["ms-playwright"])
                .into_iter()
                .collect(),
            QuickId::BrowserCache => {
                let mut caches = Vec::new();
                for browser in [
                    &["Microsoft", "Edge", "User Data"][..],
                    &["Google", "Chrome", "User Data"],
                    &["BraveSoftware", "Brave-Browser", "User Data"],
                ] {
                    let Some(data) = join(&self.local_app_data, browser) else {
                        continue;
                    };
                    for profile in subfolders(&data) {
                        for cache in [&["Cache"][..], &["Code Cache"], &["GPUCache"]] {
                            caches.push(cache.iter().fold(profile.clone(), |p, part| p.join(part)));
                        }
                    }
                }
                if let Some(profiles) =
                    join(&self.local_app_data, &["Mozilla", "Firefox", "Profiles"])
                {
                    caches.extend(subfolders(&profiles).into_iter().map(|p| p.join("cache2")));
                }
                caches
            }
        };
        list.into_iter().filter(|path| path.is_dir()).collect()
    }

    /// True when `path` is one of this cleanup's own folders or inside one. Checked again right
    /// before anything is deleted.
    pub fn allows(&self, id: QuickId, path: &Path) -> bool {
        inside(&self.targets(id), path)
    }
}

fn inside(targets: &[PathBuf], path: &Path) -> bool {
    targets
        .iter()
        .any(|target| path != target && path.starts_with(target))
}

fn subfolders(path: &Path) -> Vec<PathBuf> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

impl QuickId {
    pub fn title(self) -> &'static str {
        match self {
            QuickId::UserTemp => "Temporary files",
            QuickId::WindowsTemp => "Windows temporary files",
            QuickId::BrowserCache => "Browser caches",
            QuickId::UpdateCache => "Windows Update downloads",
            QuickId::RecycleBin => "Recycle Bin",
            QuickId::OldInstallers => "Old setup files in Downloads",
            QuickId::InternetCache => "Internet cache",
            QuickId::CrashReports => "Crash reports and dumps",
            QuickId::ShaderCaches => "GPU shader caches",
            QuickId::Playwright => "Playwright browsers",
        }
    }

    fn description(self) -> &'static str {
        match self {
            QuickId::UserTemp => "Left behind by apps and setups. Files from the last day stay.",
            QuickId::WindowsTemp => "The system temp folder. Files from the last day stay.",
            QuickId::BrowserCache => {
                "Edge, Chrome, Brave and Firefox page caches. Sign-ins and history stay."
            }
            QuickId::UpdateCache => {
                "Updates Windows already downloaded. It fetches them again if needed."
            }
            QuickId::RecycleBin => {
                "Everything in the Recycle Bin on every drive, for good. Check what's inside \
                 before emptying."
            }
            QuickId::OldInstallers => {
                ".exe and .msi files over 90 days old. They go to the Recycle Bin."
            }
            QuickId::InternetCache => "Windows' internet cache. Windows rebuilds it as pages load.",
            QuickId::CrashReports => {
                "Windows error reports and crash dumps. New ones appear when an app crashes."
            }
            QuickId::ShaderCaches => {
                "Compiled GPU shaders. Rebuilt on the next launch, so the first load may stutter."
            }
            QuickId::Playwright => {
                "Browsers downloaded by Playwright. Run `npx playwright install` to get them back."
            }
        }
    }

    pub fn needs_admin(self) -> bool {
        matches!(self, QuickId::WindowsTemp | QuickId::UpdateCache)
    }

    /// Files older than this are kept out of the cleanup (`None`: no age limit).
    fn min_age(self) -> Option<Duration> {
        match self {
            QuickId::UserTemp | QuickId::WindowsTemp => Some(RECENT),
            QuickId::OldInstallers => Some(OLD_INSTALLER),
            _ => None,
        }
    }
}

/// Every quick cleanup with its current size.
pub fn list(places: &Places) -> Vec<QuickWin> {
    ALL.iter()
        .map(|&id| {
            let found = measure(places, id);
            QuickWin {
                id,
                title: id.title(),
                description: id.description(),
                needs_admin: id.needs_admin(),
                size: found.freed,
                files: found.removed,
            }
        })
        .collect()
}

/// What [`clean`] would free now.
pub fn measure(places: &Places, id: QuickId) -> Cleaned {
    if id == QuickId::RecycleBin {
        return crate::os::recycle_bin_size();
    }
    let mut total = Cleaned::default();
    let targets = places.targets(id);
    for target in &targets {
        sweep(&targets, id, target, target, false, &mut total);
    }
    total
}

/// Runs one quick cleanup.
pub fn clean(places: &Places, id: QuickId) -> Result<Cleaned, String> {
    if id == QuickId::RecycleBin {
        let before = crate::os::recycle_bin_size();
        crate::os::empty_recycle_bin()?;
        return Ok(Cleaned {
            skipped: 0,
            ..before
        });
    }
    let mut total = Cleaned::default();
    let targets = places.targets(id);
    for target in &targets {
        sweep(&targets, id, target, target, true, &mut total);
    }
    Ok(total)
}

/// Walks `dir` (inside `target`), counting or removing what may go. Links are skipped, never
/// followed. Emptied subfolders go too; `target` itself stays.
fn sweep(
    targets: &[PathBuf],
    id: QuickId,
    target: &Path,
    dir: &Path,
    remove: bool,
    total: &mut Cleaned,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        // Any reparse point (symlink, junction, cloud placeholder, other name surrogate) is left
        // alone: it is never walked into, emptied or removed.
        if crate::bulk::is_reparse_point(&path) {
            total.skipped += 1;
            continue;
        }
        if metadata.is_dir() {
            // Old installers: only the files directly in Downloads.
            if id == QuickId::OldInstallers {
                continue;
            }
            sweep(targets, id, target, &path, remove, total);
            if remove && path != target {
                // Fails while anything is left inside, which is fine.
                let _ = fs::remove_dir(&path);
            }
            continue;
        }
        if id == QuickId::OldInstallers && !is_installer(&path) {
            continue;
        }
        let old_enough = id.min_age().is_none_or(|age| {
            metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|elapsed| elapsed >= age)
        });
        if !old_enough {
            total.skipped += 1;
            continue;
        }
        let size = metadata.len();
        if !remove {
            total.freed += size;
            total.removed += 1;
            continue;
        }
        if !inside(targets, &path) {
            total.skipped += 1;
            continue;
        }
        let removed = if id == QuickId::OldInstallers {
            crate::os::recycle(&path).is_ok()
        } else {
            fs::remove_file(&path).is_ok()
        };
        if removed {
            total.freed += size;
            total.removed += 1;
        } else {
            total.skipped += 1;
        }
    }
}

fn is_installer(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| INSTALLER_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn age(path: &Path, days: u64) {
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60))
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn links_in_a_target_are_skipped_and_what_they_point_to_survives() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Temp");
        let outside = temp.path().join("outside");
        write(&outside.join("precious.bin"), 10);
        write(&dir.join("old.log"), 10);
        age(&dir.join("old.log"), 3);
        std::os::unix::fs::symlink(&outside, dir.join("folder-link")).unwrap();
        std::os::unix::fs::symlink(outside.join("precious.bin"), dir.join("file-link")).unwrap();
        let places = Places {
            temp: Some(dir.clone()),
            ..Places::default()
        };

        let cleaned = clean(&places, QuickId::UserTemp).unwrap();
        assert_eq!((cleaned.removed, cleaned.skipped), (1, 2));
        assert!(fs::symlink_metadata(dir.join("folder-link")).is_ok());
        assert!(fs::symlink_metadata(dir.join("file-link")).is_ok());
        assert!(outside.join("precious.bin").exists());
        assert!(!dir.join("old.log").exists());
    }

    #[cfg(windows)]
    #[test]
    fn a_junction_in_a_target_is_skipped_and_its_target_is_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Temp");
        let outside = temp.path().join("outside");
        write(&outside.join("precious.bin"), 10);
        age(&outside.join("precious.bin"), 3);
        fs::create_dir_all(&dir).unwrap();
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(dir.join("junction"))
            .arg(&outside)
            .output()
            .unwrap();
        assert!(status.status.success(), "mklink /J failed");
        let places = Places {
            temp: Some(dir.clone()),
            ..Places::default()
        };

        let cleaned = clean(&places, QuickId::UserTemp).unwrap();
        assert_eq!((cleaned.removed, cleaned.skipped), (0, 1));
        assert!(dir.join("junction").exists());
        assert!(outside.join("precious.bin").exists());
    }

    #[test]
    fn temp_cleanup_keeps_the_folder_and_new_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Temp");
        write(&dir.join("old.log"), 10);
        write(&dir.join("setup123/old.bin"), 20);
        write(&dir.join("fresh.tmp"), 5);
        age(&dir.join("old.log"), 3);
        age(&dir.join("setup123/old.bin"), 3);
        let places = Places {
            temp: Some(dir.clone()),
            ..Places::default()
        };

        assert_eq!(
            measure(&places, QuickId::UserTemp),
            Cleaned {
                freed: 30,
                removed: 2,
                skipped: 1
            }
        );
        let cleaned = clean(&places, QuickId::UserTemp).unwrap();
        assert_eq!((cleaned.freed, cleaned.removed), (30, 2));
        assert!(dir.is_dir());
        assert!(dir.join("fresh.tmp").exists());
        assert!(!dir.join("setup123").exists());
    }

    #[test]
    fn browser_caches_are_found_per_profile_and_nothing_else_goes() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("Local");
        let edge = local.join("Microsoft/Edge/User Data");
        write(&edge.join("Default/Cache/Cache_Data/f_001"), 100);
        write(&edge.join("Profile 1/Code Cache/js/x"), 50);
        write(&edge.join("Default/Bookmarks"), 7);
        write(
            &local.join("Mozilla/Firefox/Profiles/abc.default/cache2/entries/y"),
            30,
        );
        write(
            &local.join("Mozilla/Firefox/Profiles/abc.default/places.sqlite"),
            9,
        );
        let places = Places {
            local_app_data: Some(local.clone()),
            ..Places::default()
        };

        assert_eq!(measure(&places, QuickId::BrowserCache).freed, 180);
        clean(&places, QuickId::BrowserCache).unwrap();
        assert!(edge.join("Default/Bookmarks").exists());
        assert!(local
            .join("Mozilla/Firefox/Profiles/abc.default/places.sqlite")
            .exists());
        assert!(edge.join("Default/Cache").is_dir());
        assert_eq!(measure(&places, QuickId::BrowserCache).freed, 0);
    }

    #[test]
    fn only_old_installers_directly_in_downloads_count() {
        let temp = tempfile::tempdir().unwrap();
        let downloads = temp.path().join("Downloads");
        write(&downloads.join("old-setup.EXE"), 40);
        write(&downloads.join("new-setup.msi"), 30);
        write(&downloads.join("report.pdf"), 20);
        write(&downloads.join("tools/older.exe"), 10);
        age(&downloads.join("old-setup.EXE"), 120);
        age(&downloads.join("report.pdf"), 120);
        age(&downloads.join("tools/older.exe"), 120);
        let places = Places {
            profile: Some(temp.path().to_path_buf()),
            ..Places::default()
        };
        let found = measure(&places, QuickId::OldInstallers);
        assert_eq!((found.freed, found.removed, found.skipped), (40, 1, 1));
    }

    #[test]
    fn allow_list_covers_only_inside_the_known_folders() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Temp");
        fs::create_dir_all(&dir).unwrap();
        let places = Places {
            temp: Some(dir.clone()),
            ..Places::default()
        };
        assert!(places.allows(QuickId::UserTemp, &dir.join("a.log")));
        assert!(!places.allows(QuickId::UserTemp, &dir));
        assert!(!places.allows(QuickId::UserTemp, temp.path()));
        assert!(!places.allows(QuickId::UserTemp, &temp.path().join("Temp2/a.log")));
        // Another cleanup's folder is not this one's.
        assert!(!places.allows(QuickId::BrowserCache, &dir.join("a.log")));
        // A missing folder yields nothing to clean.
        assert!(Places::default().targets(QuickId::WindowsTemp).is_empty());
    }

    #[test]
    fn this_pc_places_point_at_real_folders() {
        let places = Places::for_this_pc();
        for id in ALL {
            for target in places.targets(id) {
                assert!(target.is_dir(), "{}", target.display());
            }
        }
    }
}
