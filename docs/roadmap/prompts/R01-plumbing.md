# R01: Shared plumbing

Features: none user-facing; shared modules for F-001 to F-008. Size: Medium.

**Before you paste:** v1.2.0 tagged (merge hold).

**Parallel:** Only with the F-011 half of R09. R02, R04 and R06 need it merged.

Paste everything inside the block into a new cloud session on `Demonad112/special-memory-analyzer`.

```
You are implementing Deep Server Analyzer 1.3 features in the GitHub repo Demonad112/special-memory-analyzer
(Tauri 2 + Rust workspace, Vue 3 + TypeScript). This is a CLOUD session: start from a fresh clone of `main`.
BATCH = R01 (Shared plumbing). Features in this batch: none user-facing; shared modules for F-001 to F-008.

SETUP (empty container):
  apt-get update && apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev patchelf
  corepack pnpm install --frozen-lockfile
Rust 1.98.1 and Node 24.18.0 are pinned (rust-toolchain.toml, .nvmrc).

READ FIRST: CLAUDE.md, docs/roadmap/1.3.md (rules, the register rows for this batch, Decisions D-01 to D-24),
and docs/roadmap/1.3-plan.md (ground rules and section R01). Then read the code those sections name. Where the
docs and the code disagree, the code wins: adapt, and say what you changed in the PR.

TASK: branch `feat/r01-plumbing` from up-to-date main. Do the steps in section R01 of the plan, in order. One commit per plan step, subject "R01: <imperative summary>".
Use `git commit -F <file>`; the message ends with the Co-Authored-By line for the model you run as (for example
"Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"). After each feature is complete, append
" **Done in `<short-sha>` (R01).**" to its Status cell in docs/roadmap/1.3.md and run
`corepack pnpm exec prettier --write docs/roadmap/1.3.md`. Problems you find outside this batch go in the PR under
"Found along the way"; do not fix them here.

RULES (CLAUDE.md safety rules; never relax them):
  - Tests with every behaviour change, in the same commit. Split pure logic from Windows calls so it is tested here.
  - Saved formats change additively only: every new saved field gets #[serde(default)]; never rename or remove a
    saved field; a new report kind is a new Body variant.
  - Every delete goes through cleanup_core::bulk::run and a Logs\deletions.jsonl line. Never call recycle or
    delete_permanently from a command.
  - Folder walks never follow reparse points (scan tree Kind::Link, cleanup_core::pathsafe).
  - OneDrive online-only files are never opened, read or hashed, in any mode.
  - Long jobs are async commands + spawn_blocking, stream progress about every 100 ms, keep big results in managed
    state; the UI loads one folder level at a time; every await that writes shared state checks a sequence or
    owner token. Scans take the scan guard so they never overlap an elevated record.
  - Webview commands take ids; a path that must cross (a folder to scan, an export target) is validated and
    canonicalized once in Rust (scan_core::paths::canonical_root). Errors go through report_core::applog::error.
  - Every colour is an --app-* token in src/styles.css. No raw hex or Tailwind palette colours.
  - No new crates. Only the crate features that 1.3.md D-13 and D-24 allow; name each in the PR. If a Cargo.toml
    changed, run `cargo deny --manifest-path src-tauri/Cargo.toml check advisories licenses bans`.
  - Apache-2.0 only: never copy source from GPL tools such as WinDirStat. Microsoft docs are fine as a reference.

CHECKS: run `corepack pnpm check` before marking the PR ready; scope tests while iterating
(`corepack pnpm exec vitest run <file>`, `cargo test -p <crate> --manifest-path src-tauri/Cargo.toml`).
Windows-only code (#[cfg(windows)]) is not compiled here: write its tests anyway and list them in the PR as verified by the Windows CI job ("Quality and tests" on windows-latest) only. Report failures as they are; never skip or weaken a test.

DELIVERY: push early; open a DRAFT PR titled "R01: Shared plumbing for 1.3" with the steps done, a "Not done yet" list, a
"Windows-only, verified by CI only" section, "Found along the way", and the crate features added (or "none"),
ending "🤖 Generated with [Claude Code](https://claude.com/claude-code)". Watch CI (`gh run list`,
`gh run view <id> --log-failed`) and fix failures. When every step is done and CI is green, `gh pr ready`.
DO NOT MERGE, tag, release or force-push. Never run installer-kit/tests/Test-Kit.ps1 -Full.
MERGE HOLD: the owner merges 1.3 PRs only after v1.2.0 is tagged; that does not stop you opening the PR.

BUDGET: if usage runs low, finish or revert the current step (never leave a half step committed), push, update
the PR's "Not done yet" list and stop.

BATCH NOTES (R01): No user-visible change. Moving is_valid_windows_name, LONG_PATH_LIMIT and hash_file must keep `pub use` re-exports so transfer-core callers and every existing test stay unchanged. The security and in-use modules open the path itself, never a reparse target. Keep scan-core the lowest crate: it must not depend on any other workspace crate. Report the Node size change from accessed_ms and the perf test result in the PR.
PREREQUISITES: None in the 1.3 plan; main must be at 1.2.0 or later. If a prerequisite batch is not merged to main, stop and say so instead of starting.
RESUME: first run `git fetch origin; gh pr list --state open --search "R01 in:title"`. If a branch or PR for this
batch already exists, check it out, read the PR body's "Not done yet" list and finish those steps instead of
starting over. Steps already marked "Done in" in docs/roadmap/1.3.md are done.
```
