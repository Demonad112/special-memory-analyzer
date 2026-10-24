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

## Releasing

Only the owner releases. Pushing a `v*` tag starts `release.yml`: the `installer` job builds and
signs the installer, then the `release` job publishes it. The `release` job runs in the GitHub
environment `release`, so it waits for an approval before anything is published.

GitHub settings this relies on (set 2026-10-23):

- Environment `release`: required reviewer `Demonad112` (self-review allowed), deployments limited
  to `v*` tags.
- Tag ruleset `release-tags` on `refs/tags/v*`: restricts creation, update and deletion and blocks
  force pushes. Repository admins can bypass it.
- The `Default` branch ruleset targets `main`: pull requests need the "Quality and tests" and
  "USB kit scripts" checks.

Steps:

1. Run `scripts\Set-ReleaseTag.ps1 -Version vX.Y.Z`. It refuses unless CI is green on `origin/main`; the tag must match the version in `package.json`,
   `tauri.conf.json` and `Cargo.toml`, or `release.yml` fails.
2. When the `installer` job finishes, open the run: Actions → the run → Review deployments →
   tick `release` → Approve and deploy.
3. Run `scripts\Get-Release.ps1 -Version vX.Y.Z` to wait for the run and check `SHA256SUMS`.

A tag whose release was rejected or never approved publishes nothing. `-Version` can be left out
for a final release: both scripts then use the version in `package.json`.

### Test builds

To try the pipeline before a final release, tag a test build with the same version plus `-test.N`:

1. `scripts\Set-ReleaseTag.ps1 -Version v1.2.0-test.1`. The `X.Y.Z` part must equal the version in
   `package.json`.
2. Approve under Actions → the run → Review deployments, as above.
3. `scripts\Get-Release.ps1 -Version v1.2.0-test.1`.

`release.yml` publishes any tag containing `-` as a pre-release (`--prerelease --latest=false`), so
a test build never becomes the "Latest" release. A plain `vX.Y.Z` tag is a normal latest release.

Run the manual checks in [release-checklist.md](release-checklist.md) on the test build before the
final tag.

Known leftover: `v1.1.1-test.1` was created before this and is marked as the latest release. It has
not been edited. To fix it by hand: `gh release edit v1.1.1-test.1 --prerelease`.
