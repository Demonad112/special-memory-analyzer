# Installing Deep Server Analyzer

Windows 10/11 and Windows Server 2016+ (Desktop Experience), 64-bit.

## Files

| Variant | Use it for                                                    |
| ------- | ------------------------------------------------------------- |
| Online  | PCs with internet access (downloads WebView2 if it's missing) |

Build the installer with `corepack pnpm tauri build`. It lands in
`src-tauri/target/release/bundle/nsis/`.

Silent install: `"Deep Server Analyzer_<v>_x64-setup.exe" /S`. Installs per machine into
`C:\Program Files\Deep Server Analyzer`. Silent uninstall:
`"C:\Program Files\Deep Server Analyzer\uninstall.exe" /S`.

Release builds are signed with an ephemeral self-signed certificate, which SmartScreen does not trust,
so it will still warn. The USB kit for servers is in [usb-install.md](usb-install.md).

## Where data lives

Records, reports and the deletion log are kept in `%LOCALAPPDATA%\Deep Server Analyzer`
(`Records`, `Reports`, `Logs`). Disk Cleanup never deletes anything in that folder.

Portable mode: a file named `portable.txt` next to the exe moves them to `Data\` beside the exe.
Delete `portable.txt` to go back to `%LOCALAPPDATA%`. Settings kept by the web view (theme, recent
folders, company name) stay on the PC, not in `Data\`.

`DEEP_SERVER_ANALYZER_RECORDS_DIR`, `DEEP_SERVER_ANALYZER_REPORTS_DIR` and
`DEEP_SERVER_ANALYZER_LOGS_DIR` override the folders.
