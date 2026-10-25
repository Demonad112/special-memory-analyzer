# Deep Server Analyzer

A Windows 10/11 desktop app for checking, copying and cleaning up folders and disks.

- **Migration readiness**: scan a source folder before a move. It lists names that will fail at the target (long paths, bad names, OneDrive/SharePoint rules), audits NTFS permissions (broken inheritance, unknown accounts, wide write access, CSV export) and finds files that are open and locked.
- **Compare**: pick a left and a right folder and compare them by size. See what is only on one side, what differs, and what matches. Optionally compare owners and permissions too.
- **Record**: copy files from one folder to another with live progress, verify the result, and get a report that lists every missed or failed file in red, with the reason. **Copy changes since this run** does a short final pass at cutover with only what changed.
- **Disk Cleanup**: an in-app disk space analyzer with a treemap, the biggest folders and files, file types and a cleanup list. It also finds duplicates and stale data, shows space by owner, and tracks growth between scans with a forecast. Deletes go to the Recycle Bin by default, and every deletion is logged.
- **Servers and shares**: every screen accepts `\\server\share` and admin shares such as `\\server\C$`. Settings keeps server profiles (never a password) and lists a server's shares. Disk Cleanup on a network share or mapped drive is scan and report only; deletes are refused.
- **Reports and Settings**: reports are saved as JSON in the app data folder and can be exported to standalone HTML or CSV, with your company logo and a "Prepared for" block. A migration runbook combines readiness, Record and compare reports into one document. Settings cover theme, report retention, branding, servers and scan history.

Folder walks never follow symlinks or junctions, and OneDrive online-only files are flagged and never read.

## Install

Website with the latest download, How-To and FAQ: <https://demonad112.github.io/special-memory-analyzer/>.

Download the installer from the releases page, or build it yourself (below). See [docs/install.md](docs/install.md) for silent install, portable mode and data folders. Release builds are signed with an ephemeral self-signed certificate that Windows does not trust, so SmartScreen still warns. For servers, the USB kit ([docs/usb-install.md](docs/usb-install.md)) trusts the certificate for the length of the install only.

## Build

Needs Node.js 22+, Rust (stable, MSVC toolchain) and the WebView2 runtime.

```bash
corepack pnpm install
corepack pnpm tauri dev      # run the app
corepack pnpm quality        # prettier, vue-tsc, rustfmt, clippy
corepack pnpm test           # vitest
corepack pnpm rust:test      # cargo tests
corepack pnpm tauri build    # installer in src-tauri/target/release/bundle/nsis/
```

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
