# Deep Server Analyzer

A Windows 10/11 desktop app for checking, copying and cleaning up folders and disks.

- **Compare**: pick a left and a right folder and compare them by size. See what is only on one side, what differs, and what matches.
- **Record**: copy files from one folder to another with live progress, verify the result, and get a report that lists every missed or failed file in red, with the reason.
- **Disk Cleanup**: an in-app disk space analyzer with a treemap, the biggest folders and files, file types and a cleanup list. Deletes go to the Recycle Bin by default, and every deletion is logged.
- **Reports and Settings**: reports are saved as JSON in the app data folder and can be exported to standalone HTML or CSV. Settings cover theme and report retention.

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
