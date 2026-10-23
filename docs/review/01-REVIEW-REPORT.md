# Review report: Deep Server Analyzer v1.1.1

**Verdict: sound core, sharp edges in the delete and trust paths. Health 6.5/10.** Code quality, tests and CI hygiene are above average for a one-person tool. The real risk is in two places, both reachable today: (1) the USB kit can be made to install an arbitrary root certificate, and (2) the Disk Cleanup safety net (never-delete list, logging, reparse checks) has gaps around odd path spellings, the single-item delete and quick cleanups. Record has three integrity bugs that can make a migration report wrong. There are no secrets, no Critical findings, and no remote attack surface: the app makes no network calls.

## 1. Scope and method

- **Target.** `Demonad112/special-memory-analyzer` at `302f751` (branch `docs/unblock-step-smart-app-control`, one commit ahead of `main`'s docs). Version 1.1.1 (`package.json`, `tauri.conf.json`, `Cargo.toml` agree).
- **Read in full by the lead reviewer:**
  - `.github/workflows/{ci,release}.yml`
  - `scripts/{Get-Release,Set-ReleaseTag}.ps1`
  - `installer-kit/{DsaTrust.psm1, Install-DSA.ps1, Remove-Trust.ps1}`, plus grep of the rest of the kit
  - `tauri.conf.json`, `capabilities/default.json`, the manifests, `.gitignore`, `.claude/settings.json`
- **Swept by three read-only sub-reviews**, then re-read by the lead at the cited lines: transfer-core + scan-core + `record.rs`/`compare.rs`; cleanup-core + report-core + `cleanup.rs`/`reports.rs`/`lib.rs`; and all of `src/`. Sub-review claims were treated as leads. Every finding in the register was checked against the source; the status column says how far.
- **Executed (safe only):**
  - `corepack pnpm quality` (and its parts separately)
  - `corepack pnpm test`
  - `corepack pnpm rust:test`
  - `corepack pnpm audit --prod`
  - `git log` secret and key scans; `git grep` for TODO/FIXME/HACK
- **Not executed:** the app, the kit's install path, and the release workflow. `cargo audit` and `cargo deny` are not installed and nothing was installed to get them.

## 2. What the app is

A Windows 10/11 desktop tool for IT and MSP work on servers. It has three jobs:

| Tool             | Input                        | Output                                                                                                                                                                                       |
| ---------------- | ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Compare**      | Two folders                  | Size diff tree; missing list; saved report                                                                                                                                                   |
| **Record**       | Source, destination, options | Copies itself, or watches another tool copy. Verifies size, time or hash. Produces a per-file report of everything not copied, with the reason; retry; recovery folder plus `NOT-COPIED.csv` |
| **Disk Cleanup** | A folder or drive            | Treemap, biggest items, junk groups, quick cleanups. Deletes go to the Recycle Bin, with permanent delete for admins. Deletion log; restore last run                                         |

Reports export to HTML (offline explorer page) or CSV. Data lives in `%LOCALAPPDATA%\Deep Server Analyzer`, or in a portable folder. It is installed per machine with NSIS, from GitHub Releases or the USB kit. Runtime: Tauri 2, WebView2, Rust stable (MSVC), Node LTS for builds.

Largest files: `junk.rs` 1,380 lines, `kinds.rs` 1,310, `CleanupPage.vue` about 1,000, `doc.rs` 863, `ComparePage.vue` about 850, `copy.rs` 777.

## 3. Architecture as found

See `03-ARCHITECTURE.md`. In short: a clean four-crate split with a thin command layer, and progress through Tauri `Channel`s. Long jobs mostly run under `spawn_blocking` (exceptions: DSA-031). State is managed per tool, and the UI loads one level at a time. The weak spots are architectural duplication in safety code (three link tests, two delete paths) and two god files.

## 4. Results by area

### 4.1 Security (High 3, Medium 8, Low 4)

- **High:** DSA-001 (string-only never-delete list), DSA-002 (single delete is unlogged and has no reparse check), DSA-003 (quick cleanups permanent, unlogged, no protect check).
- **Medium:**
  - DSA-010: `csp: null`
  - DSA-011: export to any path
  - DSA-012: restore trusts a user-writable log when elevated
  - DSA-013: junction swap above the item
  - DSA-014: env-overridden data folders not protected
  - DSA-015: process-wide backup privilege
  - DSA-016, DSA-017: reparse handling gaps
- **Low:** DSA-037 to DSA-040.
- **Reviewed, nothing found:**
  - `eval` or `Function`
  - `v-html` or `innerHTML` in app code
  - shell injection: `Command::new` appears only in tests (`icacls`, `cmd /c mklink`, `powershell` for recovery-script tests)
  - SQL (none)
  - unsafe deserialization beyond serde_json of local files
  - TLS (the app has no network code)
  - weak crypto: hashing for verification uses the in-crate hasher; no security claim depends on it
  - report and run id path traversal: both guarded (`report-core/src/lib.rs:181-186`, `RunStore::open`)
  - recovery PowerShell quoting (`report-core/src/recovery.rs`): sound
  - HTML export escaping and CSV formula defusing (`report-core/src/doc.rs:499-513`): sound
  - share password: zeroed after use

### 4.2 Update and supply chain (High 2, Medium 3, Low 0, Enhancement 2)

- **High:** DSA-004 (kit trust anchor travels with the payload; no certificate constraints before the Root store), DSA-005 (SYSTEM task scripts in an un-hardened ProgramData folder).
- **Medium:** DSA-018 (vite advisories, dev only), DSA-019 (12-character thumbprint display), DSA-020 (no audit or secret scan in CI).
- **Enhancement:** DSA-066 (real signing), DSA-067 (provenance).
- **How the app is fetched and updated.** Fetch is through GitHub Releases on `v*` tags only; nothing tracks `main` and there is no auto-update. Integrity is Authenticode plus SHA256SUMS, both anchored in the downloaded zip, with an out-of-band thumbprint in the release notes. Rollback means installing an older release. A partial download fails the hash or Authenticode check, so nothing runs. The kit makes no TLS downloads; the WebView2 bootstrapper downloads from Microsoft.
- **Good:** SHA-pinned actions, least privilege, ephemeral key, signature checked in CI, and `Set-ReleaseTag.ps1` refuses to tag unless CI is green on `origin/main`.

### 4.3 Correctness (High 2, Medium 7, Low 5)

- **High:** DSA-006 (a retry after a hash mismatch is recorded as identical), DSA-007 (Watch mode reads online-only files).
- **Medium:**
  - DSA-021: rename before verify
  - DSA-022: lossy non-Unicode names
  - DSA-023: links counted as missing
  - DSA-024: missing serde defaults (project rule)
  - DSA-025, DSA-026, DSA-027: frontend async races and lost compare state
- **Low:** DSA-041 to DSA-045.
- **Reviewed, nothing found:**
  - time handling: UTC formatting in `transfer_core::format_utc`, with a time tolerance for mtime compare
  - long paths: `\\?\` prefixing exists, though in two places (DSA-054)
  - reserved names (`prepare.rs` `RESERVED_NAMES`)
  - folder-id bounds in compare
  - share path validation

### 4.4 Error handling and resilience (High 2, Medium 2, Low 6)

- **High:** DSA-008 (a crashed run stays `Running`), DSA-009 (a torn JSONL line kills the run).
- **Medium:** DSA-028 (log written after the whole bulk run), DSA-029 (non-atomic report rewrite).
- **Low:** DSA-046 to DSA-051.
- **Nothing found:**
  - `unwrap`, `expect` and `panic!` in non-test Rust: none, per both sweeps
  - empty `catch` blocks in TS: all commented, with fallbacks
  - mutex lock order is consistent (tree, then report)

### 4.5 Performance and memory (Medium 5, Low 1, Enhancement 1)

- DSA-030: Reports list parses MBs per report.
- DSA-031: heavy work on async or main threads.
- DSA-032: full report rewrite per delete, under a lock.
- DSA-033: results maps held repeatedly.
- DSA-034: hashing blocks Watch cancel.
- DSA-052: O(n·m) retry selection.
- DSA-069: test environment startup.

Large-input posture is otherwise good:

- rayon scan
- streaming JSONL
- progress throttled to about 100 ms
- one level loaded at a time in the UI
- `perf.rs` ignored test for scans

No full-file reads of user files except hashing, which is chunked in `copy.rs`.

### 4.6 Architecture and design (Low 3)

DSA-053 (god files), DSA-054 (duplicated safety code), DSA-055 (duplicated UI helpers). Global state is limited to Tauri managed state and the `OnceLock`s in `secure.rs` and `bulk.rs`. Testability is good in the crates; the command layer (`src-tauri/src/*.rs`) has no tests.

### 4.7 Code quality (Low 3, Enhancement 1)

- DSA-056: dead code.
- DSA-057: colour-token rule breaches.
- DSA-058: dead lint-staged config and no ESLint.
- DSA-070: explorer JS cycle.

Clippy with `-D warnings`, rustfmt and vue-tsc are all clean. TS is `strict` with `noUncheckedIndexedAccess`. There are zero TODO, FIXME or HACK markers. Naming and comments are consistent and plain.

### 4.8 Testing (Low 1, Enhancement 1)

**Baseline:**

- vitest: 109 pass across 12 files.
- Rust: cleanup-core 62, report-core 51 (1 ignored), scan-core 23 (perf ignored), transfer-core 59. All pass.

**Gaps:**

- no component tests (DSA-068)
- no tests for the command layer
- `quality` excludes tests (DSA-059)
- untested risk paths: every High finding except DSA-004 and DSA-005, plus short-name and UNC roots, junction ancestors, restore targets, torn JSONL, crash recovery, retry after a mismatch, and cloud files in Watch

The minimum test set is in `06-TEST-AND-VERIFY.md` §4.

### 4.9 Logging and observability (Low 1)

DSA-060: app errors go to `eprintln!` and are lost in a GUI build. The deletion log and per-run audit logs are good and hold no secrets. They do contain user names, machine names and paths, which is appropriate for their purpose. The kit's `trust.log` is good.

### 4.10 UX (Medium 2, Low 2)

DSA-035 (the permanent-delete wording), DSA-036 (Escape cancels the scan under a dialog), DSA-061 (a11y), DSA-062 (fixed locale). Progress, cancel and error toasts exist on every long job, and error strings are plain and specific. That is a strength.

### 4.11 Build, CI and packaging (Low 2)

- DSA-063: floating toolchains.
- DSA-064: no release environment gate.

Version stamping is enforced: the tag must equal all three manifests. The release is reproducible apart from the toolchains. CI covers format, types, clippy, Rust and TS tests, and kit PSScriptAnalyzer plus an end-to-end kit test on PowerShell 5.1.

### 4.12 Docs and repo hygiene (Low 1)

DSA-065: the README says builds are unsigned, the homepage URL points at another org, and the untracked `HANDOFF.md` breaks local prettier. `LICENSE` and `NOTICE` are present (Apache-2.0). `.gitignore` covers `node_modules`, `target`, `dist`, `.env` and `src-tauri/gen`. `docs/` (install, USB, first-time setup, Windows testing) is accurate against the kit code, except for the thumbprint process, which B02 changes.

### 4.13 Could be done better

| Today                                             | Better                                                    | Tradeoff                                                                                               | Finding          |
| ------------------------------------------------- | --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | ---------------- |
| Self-signed certificate plus temporary root trust | Azure Trusted Signing or an OV certificate                | About USD 10/month and identity validation. It removes the root-store step and the SmartScreen warning | DSA-066          |
| Manual thumbprint compare                         | `gh attestation verify` plus a pinned expected thumbprint | Needs the `gh` CLI on the admin box, or a web check                                                    | DSA-067, DSA-004 |
| String-prefix protection                          | Canonical paths plus a final-path check                   | Win32 code to write                                                                                    | DSA-001          |
| Rewriting MB-sized JSON per delete                | Debounced write, or explorer data in a sibling file       | Small risk of lost edits on a crash, so flush on exit                                                  | DSA-032          |
| `eprintln!`                                       | A size-capped `app.log`                                   | No new crate needed                                                                                    | DSA-060          |
| jsdom for every test                              | `environment: 'node'` by default                          | Per-file pragma                                                                                        | DSA-069          |

## 5. Strengths worth preserving

1. **Rules about safety are written down and mostly enforced in code**, with tests: Recycle Bin by default, never following reparse points, cloud files never read, serde defaults.
2. **The release pipeline:** SHA-pinned actions, ephemeral key, version-tag consistency gate, signature verification, and a CI-gated tagging script.
3. **The kit is careful:** time-boxed trust, removal on failure, a startup trigger, idempotency, and an end-to-end CI test on PowerShell 5.1.
4. **Report export is properly escaped,** with a CSP and formula-safe CSV.
5. **No panics on user data,** consistent error strings, and progress, cancel and status for every long job.
6. **Exact npm pins, a committed lockfile and `Cargo.lock`.**

## 6. Systemic themes (root causes)

1. **Safety logic has more than one implementation.** Every High in Disk Cleanup comes from a path that skipped the "good" implementation: the single delete versus bulk, quick versus bulk, link tests in three places. _Fix the pipeline once_ (B03 and B04, DSA-054) rather than patching each caller.
2. **Paths are treated as strings at trust boundaries.** Protection, export and reveal all trust the shape of a string. Canonicalize once, at entry.
3. **Crash consistency was designed for the happy path.** JSONL append is good, but the readers are strict, the state machine has no recovery, and some writes are non-atomic. Treat "the process died at any line" as a test case.
4. **The trust anchor sits inside the thing being verified.** The kit's checks are thorough but self-referential. An external anchor (pinned thumbprint, provenance, or real PKI) is the missing piece.
5. **The frontend uses async without request identity.** Every `await` that writes shared state needs a sequence or owner check.

## 7. Revisions log

| Finding          | Revision                                                                                                                                                                                                                                                                                                                                |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| DSA-040          | **Revised.** A sub-review placed the CSV formula-injection gap at `report-core/src/recovery.rs:156-162`. That file belongs to the recovery _script_, and its quoting is sound. The real gap is in **transfer-core** `recovery.rs:150-162` (`NOT-COPIED.csv`), where fields are quoted but formulas are not defused. Location corrected. |
| (dropped)        | **Revised: removed.** A sub-review flagged the bulk-delete confirm token as "reusable for the whole session". On reading `bulk.rs:77-87`, the token is a keyed hash of the exact target set and changes per app run. That is the intended design, so it is not a defect.                                                                |
| DSA-022, DSA-045 | **Revised.** The sub-review cited `walk.rs` under scan-core. It is in **transfer-core** (`crates/transfer-core/src/walk.rs`). Paths corrected.                                                                                                                                                                                          |
| DSA-018          | **Revised severity.** `pnpm audit` labels one advisory "high". Rated **Medium** because every affected path is the Vite dev server or build tooling, and the shipped app serves static `dist/` through the Tauri asset protocol.                                                                                                        |
| DSA-003          | **Considered downgrading** because permanent deletion of temp folders may be intentional. **Kept High**, because the stated rule "every deletion is logged" is broken regardless of intent, and the target roots come unchecked from environment variables. The intent question goes to Q1.                                             |
| Register IDs     | Rows were re-sequenced once, before publication, so the register sorts by severity, then area. No ID was published before that, so the IDs are stable from this version on.                                                                                                                                                             |
