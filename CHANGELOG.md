# Changelog

## 1.2.0

Fixes from the 2026-10 audit.

### Delete safety

- The never-delete list now resolves paths first, so short 8.3 names, `\\localhost\c$` and `subst` drives no longer get past it.
- Every delete, including single-item deletes and Quick cleanups, goes through one pipeline and is written to `Logs\deletions.jsonl`.
- Bulk deletes log as they go, so a crash mid-run no longer loses the record.
- Folder walks no longer follow links or mount points, including parents of a delete target.
- "Restore last cleanup" restores only the last bulk run and checks the paths it restores to.
- A file with an invalid name (unpaired surrogate) is refused with the Explorer message instead of being mangled.

### Record integrity

- A run that was killed or crashed now shows Cancelled and lists the files it never reached, and Retry copies them.
- A retried hash mismatch is no longer reported as identical.
- One damaged line in a run's results no longer makes the run unreadable.
- Copied files are verified before they replace the old file.
- Network drops on SMB shares are retried automatically.
- Elevated records and scans can no longer overlap; the second one is refused with a clear message.

### App surface

- The webview has a Content Security Policy, and exports and "open folder" only accept paths that Rust validates.
- Errors are written to `Logs\app.log` (rotating), so a failure leaves a trace.
- Saved files are written atomically and synced to disk.

### Frontend and UX

- Compare, Cleanup and Record keep their state when you switch pages.
- Fixed several stale-result races when expanding folders or starting a new scan.
- Escape closes a dialog instead of cancelling the scan behind it.
- The review step says whether items go to the Recycle Bin or are deleted permanently.
- Better keyboard navigation for tabs and context menus, and dates follow your Windows region.

### Performance

- Deleting items no longer rewrites the whole disk-usage report each time, so deletes feel instant. Pending changes are saved shortly after, and on exit.
- The Reports page opens quickly with many large reports.
- Saving reports and building the "Copy missing files" list no longer freeze the UI.
- Very deep folder trees (thousands of levels) no longer risk a crash.

### Refactor

- Link checks, delete handling and UI helpers each live in one shared place instead of several copies.
- Colours use the app's theme tokens everywhere.

### Release tooling

- The USB kit and installer checks are anchored and hardened, and the SYSTEM task folder has a protected ACL.
- Releases need an approval and a protected tag. Toolchains are pinned.
- `Set-ReleaseTag.ps1` and `Get-Release.ps1` default to the current version. Test tags such as `v1.2.0-test.1` are supported and published as pre-releases.
- CI fails on high-severity dependency advisories.

## 1.1.1

Earlier release; no changelog was kept.
