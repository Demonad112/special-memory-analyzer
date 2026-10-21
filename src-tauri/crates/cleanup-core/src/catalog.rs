//! The path catalog: known folders sorted into what may be cleared now (`SAFE`), what needs a look
//! first (`REVIEW`), what is only reported (`REPORT_ONLY`), what Windows handles (`HANDOFF`) and
//! what is only explained (`GUIDE`). Constants only; nothing here deletes anything.
//!
//! Paths are folder names under a [`Base`], so they match on any PC. `*` matches one folder.

use crate::quick::QuickId;
use serde::Serialize;

/// The folder a catalog path starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Base {
    LocalAppData,
    AppData,
    UserProfile,
    SystemDrive,
}

/// Which list a finding belongs to. Ordered as the UI shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// May be ticked and cleared without a second look.
    Safe,
    /// Could go, but a person looks first.
    Review,
    /// Reported with its size only: no delete ids leave the backend.
    Never,
}

/// What removing the folder costs, as the UI says it. Ordered from least to most to lose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// The app or Windows rebuilds it on its own.
    Regenerates,
    /// It is downloaded again when needed, possibly slowly or with a network.
    Redownloads,
    /// It holds data nothing else can restore.
    UserData,
}

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub base: Base,
    /// Folder names from `base` down, e.g. `["Microsoft", "Windows", "INetCache"]`.
    pub path: &'static [&'static str],
    pub label: &'static str,
    /// What happens after removal, in plain words.
    pub restore: &'static str,
    pub confidence: Confidence,
    /// The quick cleanup that clears this folder, when one does.
    pub quick: Option<QuickId>,
}

/// A folder the app explains but never deletes: no delete button, only the steps to take.
#[derive(Clone, Copy, Debug)]
pub struct Guide {
    pub file: &'static str,
    pub label: &'static str,
    pub how: &'static str,
}

/// Safe to clear now: caches the app or Windows rebuilds on its own.
pub const SAFE: &[Entry] = &[
    Entry {
        base: Base::LocalAppData,
        path: &["Microsoft", "Windows", "INetCache"],
        label: "Internet cache",
        restore: "Rebuilt as pages load.",
        confidence: Confidence::Regenerates,
        quick: Some(QuickId::InternetCache),
    },
    Entry {
        base: Base::LocalAppData,
        path: &["Microsoft", "Windows", "WER"],
        label: "Windows error reports",
        restore: "Windows writes new reports when an app crashes.",
        confidence: Confidence::Regenerates,
        quick: Some(QuickId::CrashReports),
    },
    Entry {
        base: Base::LocalAppData,
        path: &["CrashDumps"],
        label: "Crash dumps",
        restore: "Written again when an app crashes.",
        confidence: Confidence::Regenerates,
        quick: Some(QuickId::CrashReports),
    },
    Entry {
        base: Base::LocalAppData,
        path: &["D3DSCache"],
        label: "DirectX shader cache",
        restore: "Rebuilt the next time a game or app draws. The first launch may stutter.",
        confidence: Confidence::Regenerates,
        quick: Some(QuickId::ShaderCaches),
    },
    Entry {
        base: Base::LocalAppData,
        path: &["NVIDIA", "DXCache"],
        label: "NVIDIA shader cache (DirectX)",
        restore: "Rebuilt by the driver. The first launch of each game may stutter.",
        confidence: Confidence::Regenerates,
        quick: Some(QuickId::ShaderCaches),
    },
    Entry {
        base: Base::LocalAppData,
        path: &["NVIDIA", "GLCache"],
        label: "NVIDIA shader cache (OpenGL)",
        restore: "Rebuilt by the driver. The first launch of each game may stutter.",
        confidence: Confidence::Regenerates,
        quick: Some(QuickId::ShaderCaches),
    },
    Entry {
        base: Base::LocalAppData,
        path: &["ms-playwright"],
        label: "Playwright browsers",
        restore: "Run `npx playwright install` to download them again.",
        confidence: Confidence::Redownloads,
        quick: Some(QuickId::Playwright),
    },
    // Chrome profiles: `*` is any profile folder. The Browser caches card covers these.
    Entry {
        base: Base::LocalAppData,
        path: &["Google", "Chrome", "User Data", "*", "Cache"],
        label: "Chrome cache",
        restore: "Chrome rebuilds it. Sign-ins and history are kept.",
        confidence: Confidence::Regenerates,
        quick: Some(QuickId::BrowserCache),
    },
    Entry {
        base: Base::LocalAppData,
        path: &["Google", "Chrome", "User Data", "*", "Code Cache"],
        label: "Chrome code cache",
        restore: "Chrome rebuilds it. Sign-ins and history are kept.",
        confidence: Confidence::Regenerates,
        quick: Some(QuickId::BrowserCache),
    },
    Entry {
        base: Base::LocalAppData,
        path: &["Google", "Chrome", "User Data", "*", "GPUCache"],
        label: "Chrome GPU cache",
        restore: "Chrome rebuilds it. Sign-ins and history are kept.",
        confidence: Confidence::Regenerates,
        quick: Some(QuickId::BrowserCache),
    },
];

/// Under `.cache` in the profile: pure caches, re-downloaded quickly.
pub const DOT_CACHE_PURE: &[&str] = &["pip", "npm", "pnpm", "puppeteer"];

/// Under `.cache` in the profile: slow to download again (models), so never offered as safe.
pub const DOT_CACHE_SLOW: &[&str] = &["huggingface", "torch", "whisper"];

/// Could go, but a person should look first.
pub const REVIEW: &[Entry] = &[
    Entry {
        base: Base::UserProfile,
        path: &[".cargo", "registry", "cache"],
        label: "Cargo download cache",
        restore: "Cargo downloads crates again on the next build. Needs the network.",
        confidence: Confidence::Redownloads,
        quick: None,
    },
    Entry {
        base: Base::UserProfile,
        path: &[".cargo", "registry", "src"],
        label: "Cargo unpacked sources",
        restore: "Unpacked again from the cache. Without the cache, Cargo downloads them.",
        confidence: Confidence::Redownloads,
        quick: None,
    },
    Entry {
        base: Base::LocalAppData,
        path: &["Spotify", "Data"],
        label: "Spotify cache",
        restore: "Songs stream and cache again. Check Spotify's storage settings before \
                  clearing, in case offline music lives here.",
        confidence: Confidence::Redownloads,
        quick: None,
    },
];

/// Chrome's whole profile folder: reported with its size, never offered. Its cache folders are
/// the `*` entries in [`SAFE`], so the cache can be shown apart from this total.
pub const CHROME_USER_DATA: Entry = Entry {
    base: Base::LocalAppData,
    path: &["Google", "Chrome", "User Data"],
    label: "Chrome profile data",
    restore: "Holds sign-ins, history, extensions and settings. Clearing the cache folders \
              leaves these alone.",
    confidence: Confidence::UserData,
    quick: None,
};

/// Found and reported with their size; nothing here gets a delete action.
pub const REPORT_ONLY: &[Entry] = &[
    Entry {
        base: Base::LocalAppData,
        path: &["local-updater"],
        label: "Local updater folder",
        restore: "Not identified. Check what is inside before removing anything.",
        confidence: Confidence::UserData,
        quick: None,
    },
    Entry {
        base: Base::AppData,
        path: &["Local"],
        label: "App runtimes under Roaming",
        restore:
            "Holds runtimes such as lightning-services. Removing it breaks the apps that use them.",
        confidence: Confidence::UserData,
        quick: None,
    },
];

/// Windows owns these. The app points at the Windows tool; it never deletes them itself.
pub const HANDOFF: &[Entry] = &[Entry {
    base: Base::SystemDrive,
    path: &["Windows.old"],
    label: "Previous Windows install",
    restore: "You can't roll back to the old Windows afterwards.",
    confidence: Confidence::UserData,
    quick: None,
}];

/// System files the app explains but never removes.
pub const GUIDE: &[Guide] = &[
    Guide {
        file: "hiberfil.sys",
        label: "Hibernation file",
        how: "Turn hibernation off in an admin terminal with `powercfg /hibernate off`. \
              That also turns off Fast Startup and frees the file.",
    },
    Guide {
        file: "pagefile.sys",
        label: "Page file",
        how: "Windows manages it. To change its size, open System Properties, then \
              Advanced, Performance Settings, Advanced, Virtual memory. Don't delete it \
              while Windows is running.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quick::Places;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn resolve(base: &Path, path: &[&str]) -> PathBuf {
        path.iter()
            .fold(base.to_path_buf(), |path, part| path.join(part))
    }

    #[test]
    fn every_safe_folder_resolves_to_its_quick_cleanup_and_nothing_else() {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("Local");
        let places = Places {
            local_app_data: Some(local.clone()),
            ..Places::default()
        };
        for entry in SAFE {
            // Chrome's `*` is covered by the Browser caches card, which has its own tests.
            let Some(id) = entry.quick else { continue };
            if entry.path.contains(&"*") {
                continue;
            }
            assert_eq!(entry.base, Base::LocalAppData, "{}", entry.label);
            let folder = resolve(&local, entry.path);
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("x.bin"), b"x").unwrap();
            assert!(places.allows(id, &folder.join("x.bin")), "{}", entry.label);

            // A sibling whose name only starts with the same letters is not inside.
            let last = entry.path.last().unwrap();
            let sibling = folder.with_file_name(format!("{last}Old"));
            fs::create_dir_all(&sibling).unwrap();
            assert!(
                !places.allows(id, &sibling.join("x.bin")),
                "{}",
                entry.label
            );
        }
    }

    #[test]
    fn dot_cache_classes_do_not_overlap() {
        for name in DOT_CACHE_PURE {
            assert!(!DOT_CACHE_SLOW.contains(name), "{name} is in both classes");
        }
    }

    #[test]
    fn every_entry_says_what_it_is_and_what_happens_after() {
        let entries = SAFE.iter().chain(REVIEW).chain(REPORT_ONLY).chain(HANDOFF);
        for entry in entries {
            assert!(!entry.label.is_empty());
            assert!(!entry.restore.is_empty(), "{}", entry.label);
            assert!(!entry.path.is_empty(), "{}", entry.label);
        }
        for guide in GUIDE {
            assert!(!guide.how.is_empty(), "{}", guide.file);
        }
    }
}
