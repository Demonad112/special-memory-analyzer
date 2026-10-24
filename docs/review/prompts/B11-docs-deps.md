# B11: Docs, deps, polish

Findings: DSA-018, 058, 062, 064, 065, 066, 068, plus the CLAUDE.md merge and the 1.2.0 version bump. Size: Small to medium.

**Before you paste:** B10 (both sessions) and B12 MERGED. All owner questions are answered in `07-OPEN-QUESTIONS.md` (2026-10-23), including Q-deps.

**Parallel:** None. Run last.

Paste everything inside the block into a new **local** Claude Code session in `C:\Dev\projects\special-memory-analyzer` (worktree cut from `main`).

```
You are continuing the Deep Server Analyzer audit fixes in the GitHub repo Demonad112/special-memory-analyzer
(Tauri 2 + Rust workspace, Vue 3 + TypeScript). This is a LOCAL session in a worktree cut from `main` on Windows.
BATCH = B11 (Docs, deps, polish), the last batch. Findings in this batch: DSA-018, 058, 062, 064, 065, 066, 068.

SETUP: corepack pnpm install --frozen-lockfile
Rust 1.98.1 and Node 24.18.0 are pinned (rust-toolchain.toml, .nvmrc). Defender ASR blocks freshly built Rust
test binaries on this machine, so Rust tests are verified by CI. cargo fmt/clippy still run locally.

READ FIRST: CLAUDE.md, docs/review/10-SESSION-RUNBOOK.md (order table),
docs/review/04-IMPLEMENTATION-PLAN.md (section B11), docs/review/05-SONNET-PROMPT.md,
docs/review/07-OPEN-QUESTIONS.md (all answered), and each ID's row in docs/review/02-FINDINGS-REGISTER.md.
HANDOFF.md may be stale; the code and the register win.

TASK: branch `fix/b11-docs-deps` from up-to-date main. Do the plan's B11 steps in order. One commit per finding,
subject "DSA-0NN: <imperative summary>", message ends with "Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>";
use `git commit -F <file>`. After each fix append " **Fixed in `<short-sha>` (B11).**" to the row's Recommended fix
cell and run prettier on the register. New issues get the next free DSA number (read the register for the highest
number) in the register; do not fix them here.

DECISIONS FOR THIS BATCH (2026-10-23, do not re-ask):
  - DSA-018 / Q-deps: APPROVED. Bump vite to the latest 8.3.x (>= 8.3.3). `pnpm why vite` must show one version,
    `corepack pnpm audit --prod` must report 0, and `corepack pnpm check` and `corepack pnpm build` must pass. Try
    `corepack pnpm tauri build` too, and report whether it built or ASR blocked it.
  - DSA-064: the owner CREATED the `release` environment (required reviewer Demonad112, `v*` tags only) and the
    `release-tags` tag ruleset on 2026-10-23. Add `environment: release` to the `release` job in release.yml (not the
    `installer` job), document the settings and the approval step in docs/install.md → "Releasing", and mark it fixed.
  - DSA-065: homepage is https://github.com/Demonad112/special-memory-analyzer. HANDOFF.md stays untracked: add it
    to .gitignore and .prettierignore.
  - DSA-062: report dates stay en-US, UI dates use the user locale; show the 500-row cap.
  - DSA-058: no ESLint; delete the lint-staged block and the stray eslint-disable comment.
  - DSA-068: @vue/test-utils approved.
  - DSA-066: comparison doc only, no paid service.
  - CLAUDE.md: merge docs/review/CLAUDE.proposed.md (approved). Update it for the B09/B10/B12 layout and delete the
    proposed file.
  - Last commit: bump the version 1.1.1 -> 1.2.0 in package.json, src-tauri/tauri.conf.json, src-tauri/Cargo.toml and
    Cargo.lock. Subject "chore: bump version to 1.2.0". Do NOT tag or release.

OWNER DECISIONS: elevated use is normal (Q3). Quick cleanups stay permanent but logged and protection-checked
(B04). Deletes go to the Recycle Bin by default and are logged to Logs\deletions.jsonl. Apache-2.0 only: never
copy GPL source. Report formats change additively only (Q5).

CHECKS: run `corepack pnpm check` and `corepack pnpm build`. Report failures as they are; never skip or weaken a test.

DELIVERY: push early; open a DRAFT PR titled "B11: Docs, deps, polish (DSA-018, 058, 062, 064, 065, 066, 068)" with the
IDs done, a "Not done yet" list, a "Windows-only, verified by CI only" section,
the `pnpm audit --prod` result, ending "🤖 Generated with [Claude Code](https://claude.com/claude-code)".
Watch CI (`gh run list`, `gh run view <id> --log-failed`) and fix failures. When every ID is done and CI is green,
`gh pr ready`. DO NOT MERGE, tag, release or force-push. Never run installer-kit/tests/Test-Kit.ps1 -Full.

BUDGET: if usage runs low, finish or revert the current ID (never leave a half fix committed), push, update the
PR's "Not done yet" list and stop.

PREREQUISITES: B10 (both sessions) and B12 MERGED. If one is not merged to main, stop and say so instead of starting.
RESUME: first run `git fetch origin; gh pr list --state open --search "B11 in:title"`. If a branch or PR for this batch
already exists, check it out, read the PR body's "Not done yet" list and finish those IDs instead of starting over.
```
