# B04: Delete safety II (canonical paths, quick clean, restore)

Findings: DSA-001, 003, 012. Size: Medium.

**Before you paste:** B03 MERGED. Q1 and Q3 decided (accepted: Quick cleanups stay permanent but logged and protection-checked; elevated use is normal).

**Parallel:** Do not run alongside B03 or B05 (shared cleanup-core files).

Paste everything inside the block into a new cloud session on `Demonad112/special-memory-analyzer`.

```
You are continuing the Deep Server Analyzer audit fixes in the GitHub repo Demonad112/special-memory-analyzer
(Tauri 2 + Rust workspace, Vue 3 + TypeScript). This is a CLOUD session: start from a fresh clone of `main`.
BATCH = B04 (Delete safety II (canonical paths, quick clean, restore)). Findings in this batch: DSA-001, 003, 012. DSA-018 (vite bump) stays skipped until the owner approves it.

SETUP (empty container):
  apt-get update && apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev     libjavascriptcoregtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev patchelf
  corepack pnpm install --frozen-lockfile
Rust 1.98.1 and Node 24.18.0 are pinned (rust-toolchain.toml, .nvmrc).

READ FIRST: CLAUDE.md, docs/review/10-SESSION-RUNBOOK.md (order table),
docs/review/04-IMPLEMENTATION-PLAN.md (section B04), docs/review/05-SONNET-PROMPT.md, and each ID's row in
docs/review/02-FINDINGS-REGISTER.md. HANDOFF.md may be stale; the code and the register win.

TASK: branch `fix/b04-delete-safety-2` from up-to-date main. Fix the batch's IDs in the plan's order, test first where
marked TF. One commit per finding, subject "DSA-0NN: <imperative summary>", message ends with
"Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"; use `git commit -F <file>`. After each fix append
" **Fixed in `<short-sha>` (B04).**" to the row's Recommended fix cell and run prettier on the register.
New issues get the next free DSA number (DSA-071+) in the register; do not fix them here.

OWNER DECISIONS: elevated use is normal (Q3). Quick cleanups stay permanent but logged and protection-checked
(B04). Deletes go to the Recycle Bin by default and are logged to Logs\deletions.jsonl. Apache-2.0 only: never
copy GPL source. Other questions use the defaults in docs/review/07-OPEN-QUESTIONS.md.

CHECKS: run `corepack pnpm check`. Windows-only code is not compiled here: write its tests anyway and say in the
PR that they are verified by the Windows CI job only. Report failures as they are; never skip or weaken a test.

DELIVERY: push early; open a DRAFT PR titled "B04: Delete safety II (canonical paths, quick clean, restore) (DSA-001, 003, 012)" with the IDs done, a "Not done yet"
list, a "Windows-only, verified by CI only" section, ending "🤖 Generated with [Claude Code](https://claude.com/claude-code)".
Watch CI (`gh run list`, `gh run view <id> --log-failed`) and fix failures. When every ID is done and CI is green,
`gh pr ready`. DO NOT MERGE, tag, release or force-push. Never run installer-kit/tests/Test-Kit.ps1 -Full.

BUDGET: if usage runs low, finish or revert the current ID (never leave a half fix committed), push, update the
PR's "Not done yet" list and stop.

BATCH NOTES (B04): DSA-001 canonicalises paths before the never-delete check (short names, \localhost\c$, subst). Heavily Windows-only: say in the PR what only the Windows CI job verifies.
PREREQUISITES: B03 MERGED. Q1 and Q3 decided (accepted: Quick cleanups stay permanent but logged and protection-checked; elevated use is normal). If a prerequisite batch is not merged to main, stop and say so instead of starting.
RESUME: first run `git fetch origin; gh pr list --state open --search "<this batch> in:title"`. If a branch or PR for this batch already exists, check it out, read the PR body's "Not done yet" list and finish those IDs instead of starting over.
```
