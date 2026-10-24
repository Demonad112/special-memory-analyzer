# Implementation plan

Eleven batches, each sized for one Sonnet session. Finding details (location, evidence, exact fix) are in `02-FINDINGS-REGISTER.md`. This file says **what order**, **which files**, and **how to prove it worked**.

## Ground rules for every batch

- Branch per batch from up-to-date `main`: `fix/b<NN>-<slug>` (for example `fix/b03-delete-safety`). One PR per batch.
- One commit per finding: `DSA-0NN: <imperative summary>`. A finding that needs no code (already fixed, not reproducible) gets a register note, not a commit.
- Write the regression test first and see it fail where that is practical (marked **TF** below), then fix.
- Verification commands, run under **pwsh** on Windows (see `docs/windows-testing.md`):

  ```
  corepack pnpm quality
  corepack pnpm test
  corepack pnpm rust:test
  ```

  Scope Rust tests while iterating, for example `cargo test -p cleanup-core --manifest-path src-tauri/Cargo.toml`.

- Saved-format rule: any new field on a saved struct gets `#[serde(default)]`. Never remove or rename an existing field (Q5).
- Rollback for every batch: revert the PR's merge commit (`git revert -m 1 <merge>`). The batch-specific notes below say when that is not enough.

## Batch map and independence

| #   | Batch                                                   | Findings                                                                                          | Can run in parallel with                                      |
| --- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| B01 | Safety net and CI                                       | DSA-018, DSA-020, DSA-059, DSA-063, DSA-069                                                       | B02                                                           |
| B02 | USB kit trust hardening                                 | DSA-004, DSA-005, DSA-019, DSA-067                                                                | anything (touches only `installer-kit/`, `release.yml`, docs) |
| B03 | Delete safety I: one delete path                        | DSA-002, DSA-013, DSA-014, DSA-016, DSA-017, DSA-028                                              | B02, B06                                                      |
| B04 | Delete safety II: canonical paths, quick clean, restore | DSA-001, DSA-003, DSA-012                                                                         | B02, B06 (after B03)                                          |
| B05 | App surface hardening                                   | DSA-010, DSA-011, DSA-024, DSA-029, DSA-037, DSA-038, DSA-039, DSA-047                            | B02, B06                                                      |
| B06 | Record integrity I                                      | DSA-006, DSA-009, DSA-021, DSA-023, DSA-040, DSA-046, DSA-049, DSA-050                            | B02 to B05                                                    |
| B07 | Record integrity II                                     | DSA-007, DSA-008, DSA-015, DSA-022, DSA-034                                                       | B02 (after B06)                                               |
| B08 | Frontend correctness and UX                             | DSA-025, DSA-026, DSA-027, DSA-035, DSA-036, DSA-043, DSA-044, DSA-051, DSA-055, DSA-057, DSA-061 | any Rust-only batch                                           |
| B09 | Performance                                             | DSA-030, DSA-031, DSA-032, DSA-033, DSA-052                                                       | B08 (after B03, B05, B06)                                     |
| B10 | Refactor and quality                                    | DSA-041, DSA-042, DSA-045, DSA-048, DSA-053, DSA-054, DSA-056, DSA-060, DSA-070                   | none: run alone, last code batch                              |
| B12 | Follow-ups found during the fixes                       | DSA-071, DSA-072, DSA-073, DSA-074, DSA-075, DSA-077                                              | none: run alone, after B10                                    |
| B11 | Docs, deps, polish                                      | DSA-058, DSA-062, DSA-064, DSA-065, DSA-066, DSA-068                                              | none: run last, after B12                                     |

Every ID from DSA-001 to DSA-070 appears exactly once above. B12 (added 2026-10-23) holds the open findings logged while fixing: DSA-071 to DSA-075 and DSA-077. DSA-076 is a duplicate of DSA-071. B12 runs before B11 so that B11 stays the last batch and ends with the version bump for the next release.

"Parallel" means the batches touch disjoint files and can be separate sessions or branches. Merge order still follows the table.

---

## B01: Safety net and CI

**Goal:** green local gate, dependency and secret scanning in CI, reproducible toolchains, faster tests. No app behaviour change.

**Files:** `package.json`, `pnpm-lock.yaml`, `.github/workflows/ci.yml`, new `deny.toml`, new `rust-toolchain.toml`, new `.nvmrc`, `.github/workflows/release.yml` (toolchain lines only), `vite.config.ts`, `.prettierignore`.

**Steps**

1. **Prerequisite.** Add `HANDOFF.md` to `.prettierignore` so `pnpm quality` is green locally. This is not a finding; note it in the PR. Run `corepack pnpm quality` and confirm the baseline is green.
2. **DSA-059.** Add `"check": "corepack pnpm quality && corepack pnpm test && corepack pnpm rust:test"` to `package.json` scripts.
3. **DSA-018.** _Ask the owner first: this is a dependency bump._ Set `vite` to the latest `8.3.x` that is 8.3.3 or later, run `corepack pnpm install`, `pnpm why vite` (expect a single version), `corepack pnpm audit --prod` (expect 0), then `corepack pnpm build` and `corepack pnpm tauri build`. If the owner declines, skip this step and leave the B01 audit job non-blocking.
4. **DSA-063.** Run `rustc --version`. Add `rust-toolchain.toml` with `[toolchain] channel = "<that x.y.z>"` and `components = ["rustfmt","clippy"]`. Add `.nvmrc` with the current Node LTS major.minor.patch (`node --version`). In both workflows, replace `node-version: lts/*` with `node-version-file: .nvmrc`, and `toolchain: stable` with the same pinned version.
5. **DSA-020.** Add an `audit` job to `ci.yml` on `ubuntu-latest` with `continue-on-error: true` and these steps:
   - checkout
   - pnpm and node setup (same SHAs as the other jobs)
   - `corepack pnpm install --frozen-lockfile`
   - `corepack pnpm audit --prod --audit-level high`
   - install cargo-deny with `taiki-e/install-action` pinned by SHA
   - `cargo deny --manifest-path src-tauri/Cargo.toml check advisories licenses bans`
   - `gitleaks/gitleaks-action` pinned by SHA, with `fetch-depth: 0`

   Create `deny.toml`:
   - `[licenses] allow = ["Apache-2.0","MIT","BSD-2-Clause","BSD-3-Clause","ISC","Unicode-3.0","Zlib","MPL-2.0","CC0-1.0","Apache-2.0 WITH LLVM-exception","BSL-1.0"]`
   - `[advisories] yanked = "warn"`

   Run cargo-deny locally if available. If it is not, note that in the PR.

6. **DSA-069.** In `vite.config.ts`, set `test.environment = 'node'` and add `// @vitest-environment jsdom` to the top of each test that touches `document` or `window`. Find them with `grep -l "document\|window" src/**/*.test.ts`. Check that all 109 tests still pass and note the new duration.

**Acceptance**

- `corepack pnpm check` passes locally.
- CI shows the new `audit` job. If it fails on advisories, list them in the PR; that is expected while it is non-blocking.
- Release workflow syntax is valid (`gh workflow view "Build installer"`). Do not push a tag.

**Rollback:** revert. `rust-toolchain.toml` affects every developer's build, so remove it if the pinned version will not install.

---

## B02: USB kit trust hardening

**Goal:** a tampered kit can no longer install a root CA, the data folder is admin-only, the full thumbprint is checked, and releases carry provenance.

**Files:** `installer-kit/DsaTrust.psm1`, `installer-kit/Install-DSA.ps1`, `installer-kit/DSA-Installer.ps1`, `installer-kit/tests/Test-Kit.ps1`, `.github/workflows/release.yml`, `docs/usb-install.md`, `docs/first-time-setup.md`.

**Steps**

1. **DSA-004.** Add `Test-DsaCertificate` (exact checks a–e in the register) to `DsaTrust.psm1` and export it. In `Install-DSA.ps1`, load the certificate once (`$cert = New-Object …X509Certificate2 $cer`) and call `Test-DsaCertificate -Cert $cert` right after the hash check (after line 104), before any trust change. Add the `[string]$ExpectedThumbprint` parameter. When it is set, compare with `-ine` against the full thumbprint and throw on mismatch. In the interactive path with no expected thumbprint, print the full thumbprint and the `.cer` SHA-256, then `Read-Host 'Type the LAST 8 characters of the thumbprint shown on the release page'`. Require a case-insensitive match of the last 8, or throw.
2. **DSA-019.** In `DSA-Installer.ps1`, show the full thumbprint (not `Substring(0,12)`). Add a text box "Paste the thumbprint from the release page". Disable the trust/install button until it matches (strip spaces, case-insensitive). `-NoPrompt` without `-ExpectedThumbprint` must throw in `Install-DSA.ps1`.
3. **DSA-005.** Add `Initialize-DsaDataDir` (see the register) and call it in `Install-DSA.ps1` before line 129. Test-Kit sets `DSA_TRUST_DIR` to a scratch folder, so the ACL code must work on an arbitrary path.
4. **Tests** (`Test-Kit.ps1`):
   - Generate three throwaway certificates with `New-SelfSignedCertificate`: a CA (`-KeyUsage CertSign -TextExtension @("2.5.29.19={text}CA=true")`), a server-auth certificate (`-Type SSLServerAuthentication`), and a code-signing certificate like CI's. Assert that `Test-DsaCertificate` rejects the first two and accepts the third.
   - Assert that after `Initialize-DsaDataDir` the folder's ACL is protected and `BUILTIN\Users` has no Write, Modify or FullControl right.
   - Clean up the certificates in `finally`.
5. **DSA-067.** In `release.yml`:
   - Add `id-token: write` and `attestations: write` to the `release` job permissions.
   - Before `gh release create`, run `actions/attest-build-provenance` (pinned SHA) with `subject-path: installer/*.exe` and `kit/*.zip`.
   - Generate a top-level `SHA256SUMS-release.txt` over `installer/*.exe kit/*.zip` and upload it as an extra asset.
6. Docs: update `docs/usb-install.md` and `docs/first-time-setup.md` for the full-thumbprint check, `-ExpectedThumbprint`, and `gh attestation verify`.

**Acceptance**

- `powershell -NoProfile -ExecutionPolicy Bypass -File installer-kit\tests\Test-Kit.ps1` passes. Run `-Full` only in CI or on a throwaway VM: it touches the machine stores.
- `Invoke-ScriptAnalyzer -Path installer-kit -Recurse -Severity Error` reports nothing.
- The CI `kit` job is green.
- Do **not** push a tag to test the release. The owner does that (`scripts/Set-ReleaseTag.ps1`).

**Rollback:** revert. Machines that already installed trust are unaffected, since the removal task is unchanged.

---

## B03: Delete safety I (one delete path)

**Goal:** every Disk Cleanup delete goes through `bulk::delete_one`, is logged per item, refuses reparse ancestors, and never touches the app's data folders.

**Files:** `src-tauri/src/cleanup.rs`, `src-tauri/crates/cleanup-core/src/bulk.rs`, `src-tauri/crates/cleanup-core/src/protect.rs`, `src-tauri/crates/cleanup-core/src/quick.rs`, `src-tauri/crates/scan-core/src/list.rs`, new `src-tauri/crates/cleanup-core/src/pathsafe.rs` (optional, see DSA-054), `src-tauri/crates/cleanup-core/src/lib.rs`.

**Steps (in this order)**

1. **DSA-017 (TF).** In `list.rs:367`, switch link detection to the name-surrogate bit. Pull the classification into `fn is_link_tag(tag: u32) -> bool`. Test it with `TAG_MOUNT_POINT`, `TAG_SYMLINK`, `0xA000_001D` → true and `0x9000_601A` (cloud) → false.
2. **DSA-016 (TF).** In `quick.rs::sweep`, skip reparse-point folders and files using `bulk::is_reparse_point` (make it `pub(crate)`). Test with a junction inside a temp target: it is skipped and its target is untouched.
3. **DSA-013 (TF).** Add `has_reparse_ancestor(path, scan_root)` and call it in `bulk::delete_one` before deleting. Test with a junction as the parent (see the register).
4. **DSA-002 (TF where possible).**
   - Extract `fn log_entries(results: &[ItemResult], permanent: bool, elevated: bool, run: &str) -> Vec<LogEntry>` from `cleanup_delete_many`.
   - Reimplement `cleanup_delete` to build the single target the same way `cleanup_delete_many` does and call `bulk::run` with one target. Map the single result back to the current return contract (the error strings for "still there" and "already gone").
   - Log via the helper.
   - The frontend contract (`Overview` return) must stay identical. Check `src/lib/cleanup.ts:221-229`.
5. **DSA-028.** Give `bulk::run` an `on_result` callback and append the log line per item. Keep the summary log-error message for the UI.
6. **DSA-014.** Pass the data roots into `Protected` (see the register) from `src-tauri/src/cleanup.rs` everywhere `Protected::for_this_pc()` is built. A small `fn protected() -> Protected` helper in `cleanup.rs` avoids repeating this. Test that a path under an overridden `DEEP_SERVER_ANALYZER_LOGS_DIR` is refused.

**Acceptance**

- `cargo test -p cleanup-core -p scan-core` passes, including the new tests.
- Manual check:
  - Scan a temp folder, delete one file with the single-delete action, and confirm `%LOCALAPPDATA%\Deep Server Analyzer\Logs\deletions.jsonl` has a new line with `"method":"recycle"`.
  - Junction test: `mklink /J` a folder inside the scan root, then try to delete the junction in the UI. It must be refused, and the target must be intact.

**Rollback:** revert. There is no data-format change.

---

## B04: Delete safety II (canonical paths, quick clean, restore)

**Prerequisite:** B03 is merged. **Q1 and Q3 answered, or defaults taken.**

**Files:** `src-tauri/crates/cleanup-core/src/protect.rs`, `src-tauri/crates/cleanup-core/src/quick.rs`, `src-tauri/crates/cleanup-core/src/restore.rs`, `src-tauri/crates/cleanup-core/src/log.rs`, `src-tauri/crates/cleanup-core/src/bulk.rs`, `src-tauri/src/cleanup.rs`, `src-tauri/crates/scan-core/src/paths.rs`.

**Steps**

1. **DSA-001 (TF).**
   - Add `pub fn canonical_root(path: &Path) -> Result<PathBuf, String>` in `scan-core/src/paths.rs`. It calls `fs::canonicalize`, then turns `\\?\C:\…` into `C:\…` and `\\?\UNC\h\s\…` into `\\h\s\…`. It refuses `\\?\GLOBALROOT`, `\\?\Volume{`, and UNC hosts equal to `localhost`, `127.0.0.1`, `::1`, `.` or the computer name (`COMPUTERNAME` env).
   - Call it at the top of `cleanup_scan` and use the result as the tree root.
   - In `Protected::for_this_pc`, canonicalize each base where it exists, keeping the raw form as well.
   - Tests, all `#[cfg(windows)]`:
     - `C:\PROGRA~1` canonicalizes to `C:\Program Files`, and `check` on a child is refused.
     - `\\localhost\c$\Windows` is refused by `canonical_root`.
     - With `subst Z: C:\Windows` set up, a check on `Z:\System32\x` is refused; remove the subst in a drop guard.
2. **DSA-003.**
   - Add `Protected::is_protected_root(&self, path) -> bool`, true for an exact or `children_of` match, a drive root, or `SystemRoot`/profile itself.
   - Filter `Places::targets` with it. The quick target `SystemRoot\Temp` is inside `SystemRoot`, so it must stay allowed: the rule refuses the _root_ equal to a protected base, not paths inside one.
   - Add per-target summary logging (one `LogEntry` per target, `method` `"permanent"`, `detail` `"N files, quick cleanup <id>"`) via `log::append`.
   - Empty Recycle Bin also logs one entry.
   - Test: with `Places { temp: Some(<profile root>) }`, `targets(UserTemp)` returns nothing.
3. **DSA-012.**
   - Add `#[serde(default)] pub bin_name: Option<String>` to `LogEntry` and set it in `bulk::delete_one` after a successful recycle, if the bin item can be identified. If it cannot be identified cheaply, skip this half and record that in the register.
   - In `restore.rs`, before `create_dir_all`, call `Protected::for_this_pc().check(target, <drive root of target>)` and refuse protected targets.
   - Limit bin enumeration to the current user's SID folder (`os.rs:158-165`).
   - Test: a restore whose target is under `SystemRoot` returns `Failed` with a reason.

**Acceptance**

- `cargo test -p cleanup-core -p scan-core` passes.
- Manual check: run Quick cleanup "User temp" and see one log line per target in `deletions.jsonl`.

**Rollback:** revert. `LogEntry.bin_name` is additive and old builds ignore it.

---

## B05: App surface hardening

**Files:** `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/src/reports.rs`, `src-tauri/src/record.rs`, `src/lib/reports.ts`, `src/lib/record.ts`, `src-tauri/crates/report-core/src/lib.rs`, `src-tauri/crates/cleanup-core/src/log.rs`, `src-tauri/crates/transfer-core/src/prepare.rs`, `src-tauri/crates/transfer-core/src/store.rs`, `src-tauri/src/cleanup.rs` (the warning field), `src/lib/cleanup.ts` (show the warning).

**Steps**

1. **DSA-024 (TF).** Add serde defaults (see the register) and tests for minimal JSON.
2. **DSA-029 + DSA-046.** Add `pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()>` (tmp file, `sync_all`, rename) in `transfer-core` (report-core already depends on it). Use it in `report_core::write` and `store::write_json`. Test: write, then check that no `.tmp` is left.
3. **DSA-047.** Add `#[serde(default, skip_serializing_if = "Option::is_none")] warning: Option<String>` to `Overview` (cleanup-core) and set it when the report save fails in `cleanup_delete`, `cleanup_delete_many` and the scan. In `src/lib/cleanup.ts`, show `overview.warning` in the existing toast.
4. **DSA-011.** Validate the extension, absolute path and protected roots in `report_export`. Unit-test the validation function, extracted as a pure fn in `reports.rs` with a `#[cfg(test)]` module.
5. **DSA-037.** Change `record_reveal(run_id, relative_path, side)` to build the path from the run store, and update `src/lib/record.ts`.
6. **DSA-038.** `grep -rn "plugin-opener\|openPath" src` must return nothing. Remove `"opener:allow-open-path"` from `capabilities/default.json`. Run the app and check that the reveal buttons still work (they are Rust-side).
7. **DSA-039.** Reject `.`, `..` and prefix parts in `destination_path` callers. Change it to return `Option<PathBuf>`, or add a `safe_relative(&str) -> bool` check at manifest load. Prefer the check, to keep the signature.
8. **DSA-010.** Set the CSP (see the register). Run `corepack pnpm tauri dev`, visit Compare, Record, Record report, Cleanup (scan, treemap, delete dialog), Reports (open and export) and Settings, and confirm the devtools console has no CSP violations. If Tauri dev needs `connect-src` for the Vite HMR websocket, use `devCsp` for dev only.

**Acceptance**

- All tests pass.
- Manual: exporting HTML and CSV still works through the dialog.
- Calling `invoke('report_export', {…, path: 'C:\\Windows\\x.html'})` from devtools returns an error.

**Rollback:** revert. Serde defaults are additive.

---

## B06: Record integrity I

**Files:** `src-tauri/crates/transfer-core/src/{run.rs,copy.rs,store.rs,reason.rs,recovery.rs}`, `src-tauri/crates/scan-core/src/diff.rs`, `src-tauri/crates/report-core/src/doc.rs` (only to move `csv_cell`).

**Steps**

1. **DSA-006 (TF).** Turn off `skip_identical` on forced overwrite, and add the retry test from the register.
2. **DSA-021 (TF).** Verify the `.dspart` file before the rename. Add a test where a hash mismatch under Overwrite leaves the original destination intact. Simulate the mismatch through a `cfg(test)` hook in `write_part`, or by passing a wrong expected hash into `verify` directly.
3. **DSA-009 (TF).** Make a torn last line tolerated (see the register).
4. **DSA-023 (TF).** Exclude links from `collect_missing`.
5. **DSA-040.** Move `csv_cell` to transfer-core and use it in `list_csv` and `doc.rs`.
6. **DSA-049.** Map extra SMB error codes and extend the test.
7. **DSA-050.** Make the "(kept)" fallback check for existence.
8. _(DSA-046 was done in B05; skip it if it is already in.)_

**Acceptance**

- `cargo test -p transfer-core -p scan-core -p report-core` passes.
- `UPDATE_FIXTURES` is not needed unless `doc.rs` output changed. If the explorer fixture test fails, inspect the diff before regenerating.

**Rollback:** revert.

---

## B07: Record integrity II

**Prerequisite:** B06.

**Files:** `src-tauri/crates/transfer-core/src/{run.rs,watch.rs,prepare.rs,copy.rs,secure.rs}`, `src-tauri/src/record.rs`, `src-tauri/crates/scan-core/src/list.rs`.

**Steps**

1. **DSA-008 (TF).** Add `recover_interrupted` and call it when loading runs while no job is live.
2. **DSA-007 (TF).** Carry the cloud flag into the manifest (`#[serde(default)]`) and skip hashing and opening cloud sources in Watch.
3. **DSA-034.** Add a cancellable chunked `hash_file`.
4. **DSA-015.** Disable privileges at job end. Windows-only code: add a `#[cfg(windows)]` test that enables, then disables, and checks `SeBackupPrivilege` is not enabled using `GetTokenInformation(TokenPrivileges)`.
5. **DSA-022.** Detect lossy names, flag them as `InvalidName` or unreadable, and never build a path from a lossy name. Test with a Windows file name containing an unpaired surrogate, created via `OsString::from_wide(&[0xD800, 'a' as u16])`; skip the test if creation fails.

**Acceptance**

- Tests pass.
- Manual: start a record of a large folder, kill the app from Task Manager mid-copy, relaunch, and open the run. It shows "Cancelled", the uncopied files are listed, and Retry copies them.

**Rollback:** revert. The manifest `cloud` field is additive.

---

## B08: Frontend correctness and UX

**Files:** `src/lib/cleanup.ts`, `src/lib/compare.ts`, new `src/lib/errors.ts`, new `src/lib/useToast.ts`, new `src/lib/useElapsed.ts`, `src/lib/format.ts`, `src/pages/ComparePage.vue`, `src/pages/CleanupPage.vue`, `src/pages/RecordReportPage.vue`, `src/pages/ReportsPage.vue`, `src/components/BulkDeleteDialog.vue`, `src/components/TitleBar.vue`, `src/styles.css`, and the matching `*.test.ts` files.

**Steps**

1. DSA-025 (TF, vitest with a mocked `invoke`).
2. DSA-027, then DSA-026 (compare state into `compareJob` with a sequence guard).
3. DSA-043, DSA-044 (TF), DSA-051.
4. DSA-035 (permanent-delete confirm sub-state).
5. DSA-036 (Escape order).
6. DSA-057 (token colours plus `--color-*: initial`; run `corepack pnpm build` and fix any palette classes that now fail).
7. DSA-061 (a11y).
8. DSA-055 (shared helpers). Do this last, because it touches the same pages.

**Acceptance**

- `corepack pnpm quality` and `corepack pnpm test` pass.
- Manual in `tauri dev`:
  - Double-click two folders quickly in Cleanup; the breadcrumb and list agree.
  - Start a compare, go to Reports and back; the compare is still running or finished.
  - Press Escape with the delete dialog open during a scan; the scan continues.
  - In dark mode, the Compare page buttons are readable.

**Rollback:** revert.

---

## B09: Performance

**Prerequisite:** B03, B05, B06.

**Files:** `src-tauri/crates/report-core/src/lib.rs`, `src-tauri/crates/report-core/src/kinds.rs`, `src-tauri/src/cleanup.rs`, `src-tauri/src/compare.rs`, `src-tauri/src/lib.rs` (exit flush), `src-tauri/crates/transfer-core/src/{run.rs,store.rs,prepare.rs}`.

**Steps**

1. **DSA-052.** Use a `HashSet` in `Selection::matches`.
2. **DSA-031.** Move `cleanup_junk`, `compare_missing` and `save_report` to `spawn_blocking`.
3. **DSA-030.** Parse only a header struct for `list`. Benchmark it: add an `#[ignore]` test in `report-core` that writes 20 × 5 MB reports and times `list`. Record the before and after numbers in the PR.
4. **DSA-032.** Write outside the lock, debounce, and flush on exit.
5. **DSA-033.** Stream totals.

**Acceptance**

- Tests pass.
- The `list` benchmark is at least 5× faster.
- Manual: deleting 10 items one by one in Cleanup on a large scan feels instant, and after quitting the app the saved report lists all 10 removals.

**Rollback:** revert. Check that the debounce flush did not lose removals in the manual test above.

---

## B10: Refactor and quality

Run this alone, after B01–B09. Make pure moves first, then cleanups. **One commit per file split.**

**Steps**

1. DSA-054: create `cleanup_core::pathsafe` and move `normalize`, `is_reparse_point` and `has_reparse_ancestor` there. Replace the three link tests.
2. DSA-053: split `junk.rs`, `kinds.rs`, the `doc.rs`/`explorer.rs` assets (`include_str!`), `CleanupPage.vue` and `ComparePage.vue`. No logic edits.
   - The explorer fixture must stay byte-identical. Run the fixture test **without** `UPDATE_FIXTURES`.
3. DSA-056: remove the dead code.
4. DSA-041: re-check cancel under the lock in `compare_run`.
5. DSA-042: count only files in the retry progress.
6. DSA-045: either do nothing and record "deferred" in the register, or add a bigger stack to the scan thread.
7. DSA-048: keep N rotated deletion logs.
8. DSA-060: add an `app.log` appender and route `eprintln!` through it.
9. DSA-070: add a cycle guard to the explorer JS, then refresh the fixture with `UPDATE_FIXTURES=1 cargo test -p report-core explorer_fixture` and review the diff.

**Acceptance**

- All tests pass.
- `git diff --stat` for the split commits shows moves with matching line totals (±imports).

**Rollback:** revert per commit.

---

## B12: Follow-ups found during the fixes

**Prerequisite:** B10 (both sessions).

**Files:** `src-tauri/crates/scan-core/src/{list.rs,scan.rs,diff.rs}`, `src-tauri/crates/transfer-core/src/{secure.rs,walk.rs,copy.rs,run.rs,store.rs,recovery.rs}`, `src-tauri/crates/cleanup-core/src/{log.rs,pathsafe.rs,restore.rs}`, `src-tauri/src/{cleanup.rs,compare.rs,record.rs}`, and the Cleanup/Compare page components that show a warning.

**Steps**

1. **DSA-071 (TF).** Fill `file_id` from `MetadataExt::ino()` (with `dev`) in the Unix `read_dir` fallback, so `hard_links_take_disk_space_once` runs and passes on Linux. Gate the test with `#[cfg(windows)]` only if `ino` changes non-test behaviour. Keep the assertions as they are. After this, `cargo test --workspace` is green on Linux.
2. **DSA-072.** Show a failed single-delete log append through `Overview.warning`. Owner decision (2026-10-23): "Restore last cleanup" covers bulk runs only, so single deletes are written with `run: None`. Test both.
3. **DSA-073 (TF).** Save the recovery folder next to the run (`recovery.json`, `#[serde(default)]`). `record_reveal` reads it after a restart and falls back to `recovery_folder(destination, None, run_id)` when that folder exists.
4. **DSA-075 (TF).** Small option: `cleanup_delete` refuses a node with `lossy_name` and says why ("name is not valid Unicode; delete it in Explorer"). Compare marks such rows and does not treat two lossy names as the same file. Do not move the tree to `OsString`.
5. **DSA-074.** Enable backup/restore privileges on the copy threads only (`ImpersonateSelf`, `OpenThreadToken`, `AdjustTokenPrivileges` on the thread token, `RevertToSelf` when the thread finishes). Do this on every thread that copies, including pool workers. If the copy can't be confined to known threads, fall back to the other option: refuse to start a Compare or Cleanup scan while a record runs, with a clear message. Say which option was used in the PR. Windows-only: verified by CI.
6. **DSA-077.** Run the scan on a dedicated rayon pool with `stack_size(64 << 20)`, and make `total_up`, `diff` and the `Owned` drop iterative. Last, because it is the largest step. If budget is short, leave it in "Not done yet" and leave the register row as it is. Add a test with a deep tree (a few thousand levels, built in a temp dir where the OS allows, or as an in-memory tree).

**Acceptance**

- `cargo test --workspace` passes on Linux (DSA-071 fixed) and in Windows CI.
- The explorer fixture is unchanged (run it without `UPDATE_FIXTURES`).
- Report and run formats change additively only (Q5).

**Rollback:** revert per commit.

---

## B11: Docs, deps, polish

**Prerequisite:** B10 and B12 merged. Run last.

**Steps**

1. **DSA-065.** Fix the README install text and the `tauri.conf.json` `homepage` (Q4: `Demonad112/special-memory-analyzer`). `HANDOFF.md` stays untracked (Q10). Make sure it is in `.prettierignore` and `.gitignore`.
2. **DSA-058.** Q8: remove the dead `lint-staged` block and the stray `eslint-disable` comment. No ESLint.
3. **DSA-062.** Q7: report dates stay `en-US`, and UI dates use the user locale (`undefined`). Make the 500-row cap visible ("Showing 500 of N; the full list is in the CSV"), if it is not already.
4. **DSA-064.** Owner action, not done by the session. Document the GitHub settings in `docs/install.md` → "Releasing": environment `release` with the owner as required reviewer, and a tag ruleset for `v*` limited to the owner. Do **not** add `environment: release` to `release.yml` in this batch, or the workflow will wait forever until the owner creates the environment. Leave DSA-064 open with the note "docs done; owner applies the settings, then adds `environment: release`".
5. **DSA-068.** Q8 approved `@vue/test-utils`. Add it and write `BulkDeleteDialog.test.ts` (the permanent confirm, Escape blocked while deleting, and the token passed through).
6. **DSA-066.** Write `docs/signing-options.md` comparing the current kit with Azure Trusted Signing. No code, no paid service (Q2).
7. **DSA-018.** Q-deps approved 2026-10-23: bump `vite` to the latest 8.3.x (≥ 8.3.3). Check `pnpm why vite` shows one version, `corepack pnpm audit --prod` reports 0, and `corepack pnpm check` and `corepack pnpm build` pass. CI must be green before the PR is marked ready.
8. Merge `docs/review/CLAUDE.proposed.md` into `CLAUDE.md` (approved). Drop anything no longer true and add the layout changes from B09, B10 and B12: `report-core/src/assets/`, `junk/`, `kinds/`, `cleanup_core::pathsafe`, `report_writer.rs`, `Logs\app.log`. Delete the proposed file afterwards.
9. **Release prep.** Last commit: bump the version from `1.1.1` to `1.2.0` in `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` (and `Cargo.lock`). Subject: `chore: bump version to 1.2.0`. Do not tag. The owner releases after the manual checklists, with `scripts\Set-ReleaseTag.ps1 -Version v1.2.0`.

**Acceptance:** `corepack pnpm check` and `corepack pnpm build` pass, and `pnpm audit --prod` reports 0. The docs render on GitHub. The version matches in all three files.
