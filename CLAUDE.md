# Deep Server Analyzer

Windows desktop app with three tools: Compare two folders by size, Record a copy with a report of every missed file, and Disk Cleanup (in-app disk analyzer). Tauri 2 + Rust workspace, Vue 3 + TypeScript + Vite, Tailwind CSS v4. Distributed through GitHub Releases (`v*` tags) and a PowerShell USB kit (`installer-kit/`). There is no auto-update and no network code in the app.

## Commands

```bash
corepack pnpm install
corepack pnpm check           # quality + test + rust:test (the full local gate)
corepack pnpm quality         # prettier check, vue-tsc, rustfmt check, clippy -D warnings
corepack pnpm test            # vitest; scope with: corepack pnpm exec vitest run <file>
corepack pnpm rust:test       # cargo test --workspace; scope with: cargo test -p <crate> --manifest-path src-tauri/Cargo.toml
corepack pnpm audit --prod    # npm advisories
cargo deny --manifest-path src-tauri/Cargo.toml check advisories licenses bans

# Tauri
corepack pnpm tauri dev       # dev server on port 1430
corepack pnpm tauri build     # NSIS installer in src-tauri/target/release/bundle/nsis/

# USB kit (Windows PowerShell 5.1)
powershell -NoProfile -ExecutionPolicy Bypass -File installer-kit\tests\Test-Kit.ps1   # unit checks
# -Full changes machine certificate stores: CI or a throwaway VM only

# Release (owner only)
scripts\Set-ReleaseTag.ps1 -Version vX.Y.Z   # refuses unless CI is green on origin/main; the release job then waits for approval (docs/install.md → Releasing)
scripts\Get-Release.ps1 -Version vX.Y.Z      # download and verify the assets

# GitHub
gh pr create --fill
gh run list --limit 5
gh run view <id> --log-failed
```

Run `check` before opening a PR. Scope tests to the touched files while iterating.

## Layout

- `src/`: the Vue app.
  - `pages/` has one file per screen. `components/` holds shared components.
  - `lib/` holds plain TS helpers, each with a `*.test.ts` beside it, plus the shared job state (`cleanupJob`, `recordJob`, `compareJob`).
  - `router.ts`, `styles.css`.
- `src-tauri/src/`: Tauri commands, one file per area (`compare.rs`, `record.rs`, `cleanup.rs`, `reports.rs`), plus `report_writer.rs`, which batches deleted items into the Disk Cleanup scan's saved report (one write about 2 s after the first of a burst; a new scan and app exit flush first).
- `src-tauri/crates/`:
  - `scan-core`: listing, scan, diff and paths. This is the lowest crate.
  - `transfer-core`: Record (prepare, run, copy, watch, store).
  - `cleanup-core`: Disk Cleanup (`protect`, `bulk`, `quick`, `junk/` for suggested cleanups, `restore`, `log`, `pathsafe` for link and reparse checks).
  - `report-core`: saved reports, plus HTML and CSV export. `kinds/` has one file per report kind (`compare`, `record`, `usage`); `assets/` holds the explorer and page CSS and JS embedded byte for byte in the HTML reports (their CSP hashes depend on the exact text, so Prettier skips the folder); `applog` writes `Logs\app.log`.
  - Dependencies point down: report-core uses scan-core and transfer-core; cleanup-core uses scan-core only.
- `installer-kit/`: USB kit scripts (`DsaTrust.psm1` is the shared module).
- `docs/`: notes that outlive a session. `docs/review/` holds the 2026-10 audit and its findings register.

## Conventions

- Match the style of the surrounding code; don't reformat unrelated files.
- Add or update tests with behaviour changes.
- Never commit secrets, `.env` files or credentials.
- Bundle related work into one branch and PR. Open a PR only when asked or when the work is ready to merge.
- **Audit fixes:** one commit per finding, with the subject `DSA-0NN: <imperative summary>`. Mark the row in `docs/review/02-FINDINGS-REGISTER.md` with "Fixed in `<sha>`". New issues found along the way get the next free DSA number in the register, not a drive-by fix.
- Owner preference: do not add or follow process rules that restrict how work is done (one concern per branch, mandatory branch-and-PR for every change, "don't push to `main`", "don't force-push") unless the stakes are incredibly critical. The `.claude/settings.json` force-push deny stays because it protects released history.
- Report test failures as they are; don't skip or weaken a test to get green.
- No new dependencies or version bumps without the owner's OK. Then run the full `check` plus `tauri build`.

## Environment notes

- Windows desktop: the shell may be Git Bash or PowerShell. Heredocs in Bash halve backslashes, so write files containing backslashes with the file tools.
- Run `rust:test` on Windows under pwsh, not Git Bash: see `docs/windows-testing.md`.
- Toolchains are pinned in `rust-toolchain.toml` and `.nvmrc`; CI reads both.
- Cloud (web/mobile) sessions start with an empty container: install dependencies first.

  ```
  apt-get update && apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev patchelf && corepack pnpm install --frozen-lockfile
  ```

  Code behind `#[cfg(windows)]` is only checked on Windows.

## Safety rules (do / don't)

- **Every delete goes through one pipeline:** `cleanup_core::bulk::run`, then a `deletions.jsonl` line per item. Never call `recycle`/`delete_permanently` directly from a command. Quick cleanups log one summary line per target.
- **Paths at trust boundaries are canonicalized once** (`scan_core::paths::canonical_root`). Never compare raw path strings for protection.
- **Link and reparse checks** use the shared helpers (`cleanup_core::pathsafe`). A reparse point is anything with `FILE_ATTRIBUTE_REPARSE_POINT`; a link is a name-surrogate tag. Folder walks never follow reparse points, and that includes parents of a delete target.
- **Scans and elevated records never overlap.** An elevated record turns on backup privileges in the process token, which would change what a running Compare or Cleanup scan can read. `RecordState::begin` refuses while a scan runs, and scans hold a guard (`scan_guard`) so a record cannot switch on under them.
- App errors go through `report_core::applog::error` (not `eprintln!`), so they reach `Logs\app.log` (1 MB x 3). The GUI build discards stderr.
- A run's "copy to recovery folder" location is saved as `recovery.json` beside the run's other files (`transfer-core/src/recovery.rs`). Deleting that file only loses the remembered folder.
- OneDrive and cloud online-only files are flagged and never opened, hashed or read, in any mode, Watch included.
- Disk Cleanup deletes go to the Recycle Bin by default. Permanent and big runs need admin and the confirm token.
- Every new saved-report, run or log field needs `#[serde(default)]`. Never rename or remove a saved field.
- Saved files are written atomically (tmp file, `sync_all`, rename). JSONL readers tolerate a torn last line.
- Long jobs are `async` + `spawn_blocking`, stream progress about every 100 ms, and keep big results in managed state. No heavy work in sync commands (they run on the main thread). The UI loads one folder level at a time.
- Frontend: every `await` that writes shared state checks a sequence or owner token first.
- Webview commands take ids, not raw paths. Where a path is unavoidable (export), validate it in Rust.
- Every colour is a `--app-*` token in `src/styles.css`. No raw hex or Tailwind palette colours in components.
- USB kit: never add a certificate to a store without `Test-DsaCertificate`, and never write SYSTEM-run scripts to a folder without the protected ACL (`Initialize-DsaDataDir`).
- Apache-2.0 only: never copy source from GPL tools such as WinDirStat.

## Gotchas

- Data lives in `%LOCALAPPDATA%\Deep Server Analyzer`. Overrides: `DEEP_SERVER_ANALYZER_RECORDS_DIR`, `_REPORTS_DIR`, `_LOGS_DIR`. All of them are on the never-delete list.
- Refresh the report explorer fixture with `UPDATE_FIXTURES=1 cargo test -p report-core explorer_fixture`, and review the diff.
- Release tags must match the version in `package.json`, `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml`.
- The signing certificate is ephemeral per release. Its private key is deleted, so a release can never be re-signed.
