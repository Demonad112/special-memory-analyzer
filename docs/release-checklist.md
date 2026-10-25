# Release checklist

Manual checks to run on a test build (`vX.Y.Z-test.N`, see [Releasing](install.md#releasing)) before the final tag. Run them on the installed test build, not a dev build.

## Record

- [ ] Start a record of a large folder, kill the app in Task Manager mid-copy, relaunch. The run shows Cancelled, lists the uncopied files, and Retry copies them.
- [ ] Run "Copy to recovery folder", restart the app, reopen the report. The recovery-folder button opens the folder.

## Disk Cleanup

- [ ] Delete 10 items one by one from a large scan. Each delete feels instant.
- [ ] Quit straight away, then reopen Reports. The disk-usage report lists all 10.
- [ ] Check the list, treemap, tick boxes, right-click menu, Backspace, the Delete and Shift+Delete confirm, and the Largest files and File types tabs.
- [ ] Delete a single item, then use "Restore last cleanup". It restores only the last bulk run.
- [ ] Deleting a badly named file (unpaired surrogate) is refused with the Explorer message.

## Compare

- [ ] On a big tree the UI stays responsive while the report saves and "Copy missing files" builds its list.
- [ ] Check the running panel and the result toolbar: filter, Report, Copy missing list, Copy missing files confirm, Re-check.
- [ ] A badly named file shows a warning icon and is not paired.

## Elevated

- [ ] As administrator, start a record, then a Compare or Cleanup scan. The scan is refused with a clear message.
- [ ] The reverse: start a scan, then a record. The record is refused.

## Reports

- [ ] The page opens quickly with many large reports.
- [ ] A saved report opens in the browser and its explorer works.

## Deep tree

- [ ] Scanning a folder a few thousand levels deep (`\\?\` paths) does not crash.

## Logs

- [ ] After a failure, `%LOCALAPPDATA%\Deep Server Analyzer\Logs\app.log` has a line for it.

## Install

- [ ] The test installer installs over 1.1.1 and keeps existing reports readable.
- [ ] `scripts\Get-Release.ps1 -Version vX.Y.Z-test.N` verifies `SHA256SUMS`.

## 1.3 checks

- [ ] Readiness on a real share with the permissions audit. Open the permissions CSV in Excel.
- [ ] Compare with permissions on two shares on different servers. "Same name, different account" shows in amber if available.
- [ ] Delta run after editing three files. Kill the app mid-delta, reopen, and run "Copy changes since this run" again.
- [ ] Delete a duplicate, then restore it from the Recycle Bin.
- [ ] History: "What grew" works after two scans of the same folder.
- [ ] `\localhost\C$\Users\Public` works in Compare, Record, Readiness and Disk Cleanup, and Disk Cleanup hides delete.
- [ ] A mapped drive (`net use Z: \localhost\C$`) hides delete and resolves account names.
- [ ] Settings > Servers > List shares on localhost.
- [ ] A branded HTML export, and a runbook export with a delta chain.
