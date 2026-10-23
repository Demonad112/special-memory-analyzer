# Test and verify

## 1. Commands

Run from the repo root on Windows. Use **pwsh** for anything that runs Rust tests (see `docs/windows-testing.md`).

| Purpose                        | Command                                                                                                       |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------- |
| Install                        | `corepack pnpm install --frozen-lockfile`                                                                     |
| Run the app (dev, port 1430)   | `corepack pnpm tauri dev`                                                                                     |
| Build the installer            | `corepack pnpm tauri build` → `src-tauri/target/release/bundle/nsis/*-setup.exe` (unsigned locally)           |
| Format, types, rustfmt, clippy | `corepack pnpm quality`                                                                                       |
| Only the TS type check         | `corepack pnpm typecheck`                                                                                     |
| TS tests                       | `corepack pnpm test`, or one file: `corepack pnpm exec vitest run src/lib/cleanup.test.ts`                    |
| Rust tests                     | `corepack pnpm rust:test`, or one crate: `cargo test -p transfer-core --manifest-path src-tauri/Cargo.toml`   |
| Rust perf test (ignored)       | `cargo test -p scan-core --manifest-path src-tauri/Cargo.toml --release -- --ignored`                         |
| Refresh the explorer fixture   | `UPDATE_FIXTURES=1 cargo test -p report-core explorer_fixture` (pwsh: `$env:UPDATE_FIXTURES=1; cargo test …`) |
| npm audit                      | `corepack pnpm audit --prod`                                                                                  |
| Rust audit (after B01)         | `cargo deny --manifest-path src-tauri/Cargo.toml check advisories licenses bans`                              |
| Kit scripts lint               | `Invoke-ScriptAnalyzer -Path installer-kit -Recurse -Severity Error` (needs PSScriptAnalyzer 1.22.0)          |
| Kit unit tests                 | `powershell -NoProfile -ExecutionPolicy Bypass -File installer-kit\tests\Test-Kit.ps1`                        |
| Kit end to end                 | add `-Full`. **It changes machine certificate stores and the scheduled task. CI or throwaway VM only.**       |

## 2. Baseline observed (2026-10-22, commit 302f751, Windows 11, local toolchain)

| Check                        | Result                                                                                                                                                                     |
| ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pnpm quality`               | **Exit 1.** Only cause: the untracked root `HANDOFF.md` fails `prettier --check`. Its parts run separately: `typecheck` 0, `rust:fmt:check` 0, `rust:clippy -D warnings` 0 |
| `pnpm test`                  | 109 passed, 0 failed, 18.5 s (jsdom setup is 90% of the time, DSA-069)                                                                                                     |
| `pnpm rust:test`             | All pass. cleanup-core 62; report-core 51 + 1 ignored; scan-core 23 (+ ignored perf); transfer-core 59                                                                     |
| `pnpm audit --prod`          | 6 advisories (1 high, 4 moderate, 1 low), all `vite` 8.3.0–8.3.2, patched in 8.3.3 (DSA-018)                                                                               |
| `cargo audit` / `cargo deny` | Not installed, not run                                                                                                                                                     |
| Git history scan             | No private keys, `ghp_`/`github_pat_`/`AKIA` tokens or key files (`*.pfx`, `*.p12`, `*.pem`, `*.key`, `*.env`) ever added. 0 TODO/FIXME/HACK in source                     |
| CI (GitHub)                  | Not queried in this review (no network calls the app would make, but `gh run list` was also not run). Check with `gh run list --limit 5`                                   |

## 3. Manual smoke test (run after each batch that touches the app; about 15 min)

Use a scratch area such as `C:\dsa-smoke` with test data you create. **Never point Disk Cleanup at real data while testing.**

**Setup (pwsh):**

```
New-Item -ItemType Directory C:\dsa-smoke\src\a\b, C:\dsa-smoke\dst -Force
1..50 | % { Set-Content C:\dsa-smoke\src\a\f$_.txt ("x" * (Get-Random -Max 100000)) }
cmd /c mklink /J C:\dsa-smoke\src\link C:\dsa-smoke\dst
```

- [ ] **Launch.** The app opens, the title bar works (min, max, close, drag), and the theme toggle works in light and dark.
- [ ] **Compare** `src` vs `dst`. All files are listed as missing, and `link` is **not** in the missing list (after DSA-023). Expanding folders works. Cancel mid-run works. Leave the page and return mid-run, and the state is kept (after DSA-027).
- [ ] **Record** `src` → `dst` with verify = Hash. It finishes, the report shows all copied, and `link` is not followed.
  - Corrupt one destination byte without changing its length (`[IO.File]::WriteAllBytes`, then reset its mtime), choose "Retry", and confirm it is **re-copied**, not "skipped identical" (DSA-006).
- [ ] **Record crash.** Start a large record, end the process in Task Manager, relaunch and open the run. After DSA-008 it is Cancelled, lists the unreached files, and Retry completes.
- [ ] **Disk Cleanup.** Scan `C:\dsa-smoke`. Then:
  - Delete one file with the single action. It goes to the Recycle Bin, and `deletions.jsonl` gains a line (DSA-002).
  - Try to delete `link`. It is refused.
  - Bulk-select 3 files and choose Delete permanently (admin). The dialog says it **cannot be undone** (DSA-035).
  - Restore last, and the files return.
- [ ] **Never-delete list.** Elevated, scan `C:\PROGRA~1\Common Files`. Any delete attempt is refused (after DSA-001). **Do not run this before B04 lands.**
- [ ] **Quick cleanups.** Run the "User temp" dry-run count only, unless on a VM. After B04, the log has one line per target.
- [ ] **Reports.** The list loads, Open shows the HTML in a browser, and Export to HTML and CSV works through the dialog. A CSV row for a file named `=x.txt` starts with `'` in Excel.
- [ ] **Devtools console** (`tauri dev`, F12): no errors and, after B05, no CSP violations.

**Clean up:** `Remove-Item C:\dsa-smoke -Recurse -Force` (remove the junction first with `cmd /c rmdir C:\dsa-smoke\src\link`).

## 4. Minimum new tests before refactoring (B10)

These pin the behaviour B10's file splits must not change. Most of them land as part of their fix batch, so by B10 they exist.

| Test                                                                                         | Crate or file                                              | Covers           |
| -------------------------------------------------------------------------------------------- | ---------------------------------------------------------- | ---------------- |
| Retry after a hash mismatch re-copies                                                        | `transfer-core/src/run.rs`                                 | DSA-006          |
| A failed verify under Overwrite keeps the old destination                                    | `transfer-core/src/copy.rs`                                | DSA-021          |
| Torn last line of `results.jsonl` is tolerated; a middle bad line errors                     | `transfer-core/src/store.rs`                               | DSA-009          |
| Interrupted `Running` run recovers with the gaps listed                                      | `transfer-core/src/run.rs`                                 | DSA-008          |
| Cloud entry in Watch is never hashed                                                         | `transfer-core/src/watch.rs`                               | DSA-007          |
| Links excluded from the missing list                                                         | `scan-core/src/diff.rs`                                    | DSA-023          |
| Name-surrogate tags are links; cloud tags are not                                            | `scan-core/src/list.rs`                                    | DSA-017          |
| Short-name, loopback-UNC and subst roots refused or resolved                                 | `cleanup-core/src/protect.rs` + `scan-core/src/paths.rs`   | DSA-001          |
| Single delete is logged and refuses a junction                                               | `cleanup-core/src/bulk.rs` (+ a command-level helper test) | DSA-002          |
| Junction ancestor refused                                                                    | `cleanup-core/src/bulk.rs`                                 | DSA-013          |
| Quick target equal to a protected root is dropped; reparse folders are skipped               | `cleanup-core/src/quick.rs`                                | DSA-003, DSA-016 |
| Restore into `SystemRoot` refused                                                            | `cleanup-core/src/restore.rs`                              | DSA-012          |
| Minimal `Saved` and `LogEntry` JSON parse                                                    | `report-core/src/lib.rs`, `cleanup-core/src/log.rs`        | DSA-024          |
| Export path validation                                                                       | `src-tauri/src/reports.rs` (`#[cfg(test)]`)                | DSA-011          |
| Out-of-order `cleanup_children` responses                                                    | `src/lib/cleanup.test.ts`                                  | DSA-025          |
| Compare job state survives remount and guards stale children                                 | `src/lib/compare.test.ts`                                  | DSA-026, DSA-027 |
| `errorMessage` handles string and `{message}`                                                | `src/lib/errors.test.ts`                                   | DSA-044          |
| `Test-DsaCertificate` rejects CA and server-auth certificates; the data dir ACL is protected | `installer-kit/tests/Test-Kit.ps1`                         | DSA-004, DSA-005 |
| The explorer fixture stays byte-identical across the B10 split                               | existing `explorer_fixture`                                | DSA-053          |
