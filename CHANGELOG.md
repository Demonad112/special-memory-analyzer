# Changelog

## 1.3.0

Migration readiness, proof after the copy, delta copies, storage reports, trends, remote shares and branded reports.

### Migration readiness

- New **Migration readiness** screen (Compare page and Home). It scans a source folder and lists names that will fail at the target, with a fix hint: paths that are too long, trailing spaces or dots, reserved names such as `CON`, invalid characters, and names OneDrive or SharePoint reject.
- Pick the target: **File server (NTFS)** or **OneDrive / SharePoint**. Give a planned destination folder and path lengths are measured as they will be there.
- **Permissions** section: folders that break inheritance, accounts that no longer exist, folders anyone can write to, files with their own entries, and folders whose permissions could not be read. **Check every file** also reads file permissions.
- **Export permissions (CSV)** writes one row per access entry, for Excel.
- **In use** section: files that are open with a lock, and the program holding each one on a local drive.
- Readiness reports are saved, and export to HTML and CSV from the Reports page.

### Proof

- Compare has a **Compare permissions** option. It checks owner, inheritance and entries for each pair and shows what was added or is missing.
- **Same name, different account** (shown in amber) marks the same account name with a different ID, as when a local account is recreated on the new server.
- Compare reports gain a permissions section and a permissions CSV. With the option off, a compare saves and exports exactly as before.

### Cutover

- **Copy changes since this run** on a finished Record run starts a new run with the same source, destination and settings. It copies new and changed files and retries files the last run missed.
- A file edited at the destination after the last run is left alone and listed as "Changed at the destination since the last run". Nothing is ever deleted at the destination; files only found there are counted.
- The report shows the chain of runs and totals for changed, new, retried and unchanged files.
- Copy changes works on a run that an app crash left behind, and leaves files that were edited since.

### Storage

- **Duplicates** in Disk Cleanup finds files with identical content (size, then start and end, then a full hash). Nothing is selected for you, and the app refuses to delete every copy in a group. Deletes use the Recycle Bin by default and are logged.
- **Stale data** shows files not changed in 1 to 10 years, grouped by top-level folder. An option also uses the last-opened date.
- **By owner** shows who uses the space: files, space and top folders per account.

### Trends

- Every Disk Cleanup scan saves a small snapshot. **What grew** compares with an earlier scan of the same folder, with new and removed folders.
- A **Forecast** line estimates when the drive will be full, once there are at least 3 scans over 14 days.
- **Settings > Keep scan history** turns this off, and **Delete history** clears it.

### Remote shares

- Compare, Record, Readiness and Disk Cleanup accept `\server\share` and admin shares such as `\server\C$`.
- **Settings > Servers** saves server profiles (name, server, optional user, notes) and **List shares** shows a server's shares.
- Access uses your Windows sign-in. If that is refused, a sign-in dialog connects for this session only. The app never saves a password.
- **Deletes on network shares and mapped network drives are refused**, and the delete buttons are hidden, because the Recycle Bin does not cover them. Scanning and reports still work.

### Reports

- **Settings > Company logo** (PNG or JPEG up to 256 KB). HTML reports show the logo, company name and a "Prepared for" block with client, ticket and technician.
- CSV exports can start with the same job details (**Include job details in CSV**, on by default).
- **New runbook** on the Reports page combines a readiness report, Record runs and compare reports into one document, in order. A report deleted later shows as "Report no longer available". Runbooks keep their parts when old reports are pruned.

### Fixes

- DSA-085: Find duplicates no longer reads a file that became a link or an online-only OneDrive file after the scan.
- DSA-088: a Record job's client, ticket and technician are saved safely, so a crash cannot leave them empty.
- DSA-089: "Copy changes since this run" works after an app crash, and shows the real error if the run cannot be recovered.
- DSA-090: the permissions audit uses bounded memory on trees where most folders have a finding.
- DSA-091: the permissions audit and compare no longer read a folder or file that became a link or online-only after the scan.

### Website

- Added a GitHub Pages site (`site/`) with the latest download, features, How-To and FAQ. It deploys from `main` through `.github/workflows/pages.yml`.

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
