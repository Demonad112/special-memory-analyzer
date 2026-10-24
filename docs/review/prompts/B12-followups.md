# B12: Follow-ups found during the fixes

Findings: DSA-071, 072, 073, 074, 075, 077. Size: Medium to large.

**Before you paste:** B10 MERGED (both sessions).

**Parallel:** Run alone. B11 waits for it.

Paste everything inside the block into a new cloud session on `Demonad112/special-memory-analyzer`.

```
You are continuing the Deep Server Analyzer audit fixes in the GitHub repo Demonad112/special-memory-analyzer
(Tauri 2 + Rust workspace, Vue 3 + TypeScript). This is a CLOUD session: start from a fresh clone of `main`.
BATCH = B12 (Follow-ups found during the fixes). Findings in this batch: DSA-071, 072, 073, 074, 075, 077.
DSA-076 is a duplicate of DSA-071; do not touch its row. DSA-018 (vite bump) belongs to B11; do not do it here.

SETUP (empty container):
  apt-get update && apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev     libjavascriptcoregtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev patchelf
  corepack pnpm install --frozen-lockfile
Rust 1.98.1 and Node 24.18.0 are pinned (rust-toolchain.toml, .nvmrc).

READ FIRST: CLAUDE.md, docs/review/10-SESSION-RUNBOOK.md (order table),
docs/review/04-IMPLEMENTATION-PLAN.md (section B12), docs/review/05-SONNET-PROMPT.md, and each ID's row in
docs/review/02-FINDINGS-REGISTER.md. HANDOFF.md may be stale; the code and the register win.
B10 moved code: report scripts and styles are in report-core/src/assets/, junk.rs is cleanup-core/src/junk/,
kinds.rs is report-core/src/kinds/, path helpers are in cleanup_core::pathsafe. Find code at its new location.

TASK: branch `fix/b12-followups` from up-to-date main. Fix the batch's IDs in the plan's order (071, 072, 073, 075,
074, 077), test first where marked TF. One commit per finding, subject "DSA-0NN: <imperative summary>", message ends with
"Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"; use `git commit -F <file>`. After each fix append
" **Fixed in `<short-sha>` (B12).**" to the row's Recommended fix cell and run prettier on the register.
New issues get the next free DSA number (read the register for the highest number; DSA-078+ if DSA-077 is the last)
in the register; do not fix them here.

DECISIONS FOR THIS BATCH (made 2026-10-23, do not re-ask):
  - DSA-071: fill file_id from ino (+dev) on Unix so the test runs on Linux. Never weaken its assertions.
  - DSA-072: "Restore last cleanup" covers bulk runs only. Single deletes are logged with `run: None`, and a failed log
    append shows through `Overview.warning`.
  - DSA-073: persist the recovery folder in a `recovery.json` beside the run (`#[serde(default)]` fields).
  - DSA-075: the small option. Refuse to delete a lossy-named node, with a clear message, and keep Compare from
    treating two lossy names as one file. No OsString tree refactor.
  - DSA-074: thread-token privileges on every copying thread. If that can't be done cleanly, refuse scans while a
    record runs instead. Say which in the PR.
  - DSA-077: dedicated rayon pool with a 64 MB stack plus iterative total_up/diff/drop. Do it last. If budget is
    short, leave it in "Not done yet" and leave its register row as it is.

OWNER DECISIONS: elevated use is normal (Q3). Quick cleanups stay permanent but logged and protection-checked
(B04). Deletes go to the Recycle Bin by default and are logged to Logs\deletions.jsonl. Apache-2.0 only: never
copy GPL source. Report and run formats change additively only (Q5). Other questions use the defaults in
docs/review/07-OPEN-QUESTIONS.md.

CHECKS: run `corepack pnpm check` and `cargo test --workspace --manifest-path src-tauri/Cargo.toml`. After DSA-071 it
must be fully green on Linux. Run the explorer fixture test WITHOUT UPDATE_FIXTURES; it must not change.
Windows-only code (DSA-074) is not compiled here: write its tests anyway, cross-check with
`cargo clippy --target x86_64-pc-windows-gnu -D warnings` where the target is installed, and say in the PR that it is
verified by the Windows CI job only. Report failures as they are; never skip or weaken a test.

DELIVERY: push early; open a DRAFT PR titled "B12: Follow-ups (DSA-071, 072, 073, 074, 075, 077)" with the IDs done,
a "Not done yet" list, a "Windows-only, verified by CI only" section, a manual checklist for the owner, ending
"🤖 Generated with [Claude Code](https://claude.com/claude-code)".
Watch CI (`gh run list`, `gh run view <id> --log-failed`) and fix failures. When every ID is done and CI is green,
`gh pr ready`. DO NOT MERGE, tag, release or force-push. Never run installer-kit/tests/Test-Kit.ps1 -Full.

BUDGET: if usage runs low, finish or revert the current ID (never leave a half fix committed), push, update the
PR's "Not done yet" list and stop.

PREREQUISITES: B10 MERGED (both sessions: PR 33 and the "B10 (2/2)" PR). If a prerequisite batch is not merged to
main, stop and say so instead of starting.
RESUME: first run `git fetch origin; gh pr list --state open --search "B12 in:title"`. If a branch or PR for this batch
already exists, check it out, read the PR body's "Not done yet" list and finish those IDs instead of starting over.
```
