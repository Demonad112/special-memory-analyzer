# Deep Server Analyzer

Windows desktop app with three tools: Compare two folders by size, Record a copy with a report of every missed file, and Disk Cleanup (in-app disk analyzer). Tauri 2 + Rust workspace, Vue 3 + TypeScript + Vite, Tailwind CSS v4.

## Commands

```bash
corepack pnpm install
corepack pnpm quality         # prettier check, vue-tsc, rustfmt check, clippy -D warnings
corepack pnpm test            # vitest; scope with: corepack pnpm exec vitest run <file>
corepack pnpm rust:test       # cargo test --workspace --manifest-path src-tauri/Cargo.toml

# Tauri
corepack pnpm tauri dev       # dev server on port 1430
corepack pnpm tauri build     # NSIS installer in src-tauri/target/release/bundle/nsis/

# GitHub
gh pr create --fill
gh pr view --web
gh run list --limit 5
gh run view <id> --log-failed
```

Run quality and tests before opening a PR. Scope tests to the touched files when the full suite is slow.

## Layout

- `src/`: Vue app. `pages/` one per screen, `components/`, `lib/` plain TS helpers with a `*.test.ts` beside each, `router.ts`, `styles.css`.
- `src-tauri/src/`: Tauri commands, one file per area (`compare.rs`, `record.rs`, `cleanup.rs`, `reports.rs`).
- `src-tauri/crates/`: `scan-core` (folder walker and size compare), `transfer-core` (Record), `cleanup-core` (Disk Cleanup, Recycle Bin, deletion log), `report-core` (saved reports, HTML and CSV export).
- `docs/`: notes that outlive a session (`install.md`).

## Conventions

- Match the style of the surrounding code; don't reformat unrelated files.
- Add or update tests with behaviour changes.
- Never commit secrets, `.env` files or credentials.
- Bundle related work into one branch and PR. Open a PR only when asked or when the work is ready to merge; avoid a PR per small step.
- Report test failures as they are; don't skip or weaken a test to get green.

## Environment notes

- Windows desktop: the shell may be Git Bash or PowerShell. Heredocs in Bash halve backslashes, so write files containing backslashes with the file tools.
- Cloud (web/mobile) sessions start with an empty container: install dependencies first. The app crate builds on Linux once WebKitGTK is installed, so `corepack pnpm quality` runs in full there:
  `apt-get update && apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev patchelf && corepack pnpm install --frozen-lockfile`. Code behind `#[cfg(windows)]` is still only checked on Windows.
- Run `rust:test` on Windows under pwsh, not Git Bash: see `docs/windows-testing.md`.

## Gotchas

- Every colour is a `--app-*` token in `src/styles.css`. No raw hex or Tailwind palette colours in components.
- Folder walks never follow reparse points (symlinks, junctions). OneDrive online-only files are flagged and never read.
- Long jobs are `async` + `spawn_blocking`, stream progress about every 100 ms, and keep big results in managed state. The UI loads one folder level at a time.
- Every new saved-report field needs `#[serde(default)]`.
- Disk Cleanup deletes go to the Recycle Bin by default, and every deletion is appended to `Logs\deletions.jsonl`.
- Apache-2.0 only: never copy source from GPL tools such as WinDirStat.
- Data lives in `%LOCALAPPDATA%\Deep Server Analyzer`. Overrides: `DEEP_SERVER_ANALYZER_RECORDS_DIR`, `_REPORTS_DIR`, `_LOGS_DIR`.
- Refresh the report explorer fixture with `UPDATE_FIXTURES=1 cargo test -p report-core explorer_fixture`.
