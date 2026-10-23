# Session runbook: one batch per session, one at a time

This is how to work through the audit fixes on a limited weekly usage budget. Run **one session at a time**, in the order below. Each session does one batch (or picks up an unfinished one), opens one PR, and stops. You review and merge between sessions.

## Current position

_Updated 2026-10-23._

- **B01** and **B02** are merged (PRs #20, #21, #22). DSA-018 is skipped until Q-deps is approved.
- **Next session: B03.** Run it locally with the prompts below, or in the cloud with "Prompt: run a batch in a cloud session". Local `cargo` builds are blocked on the dev machine (Access denied, os error 5), so Rust checks run in the cloud or in CI.

## Order

| #   | Session    | Batch                       | Findings                                                  | Needs before starting            | Rough size                             |
| --- | ---------- | --------------------------- | --------------------------------------------------------- | -------------------------------- | -------------------------------------- |
| 1   | Finish B01 | Safety net and CI           | DSA-020, 059, 063, 069 (018 skipped)                      | –                                | Small                                  |
| 2   | B02        | USB kit trust hardening     | DSA-004, 005, 019, 067                                    | B01 merged                       | Medium                                 |
| 3   | B03        | Delete safety I             | DSA-002, 013, 014, 016, 017, 028                          | B01 merged                       | Medium                                 |
| 4   | B04        | Delete safety II            | DSA-001, 003, 012                                         | B03 merged; **Q1, Q3 answered**  | Medium                                 |
| 5   | B05        | App surface hardening       | DSA-010, 011, 024, 029, 037, 038, 039, 047                | B01 merged                       | Medium                                 |
| 6   | B06        | Record integrity I          | DSA-006, 009, 021, 023, 040, 046, 049, 050                | B05 merged (DSA-046 lands there) | Medium                                 |
| 7   | B07        | Record integrity II         | DSA-007, 008, 015, 022, 034                               | B06 merged                       | Medium–large                           |
| 8   | B08        | Frontend correctness and UX | DSA-025, 026, 027, 035, 036, 043, 044, 051, 055, 057, 061 | B01 merged                       | Medium                                 |
| 9   | B09        | Performance                 | DSA-030, 031, 032, 033, 052                               | B03, B05, B06 merged             | Medium                                 |
| 10  | B10        | Refactor and quality        | DSA-041, 042, 045, 048, 053, 054, 056, 060, 070           | All above merged                 | Large: split over 2 sessions if needed |
| 11  | B11        | Docs, deps, polish          | DSA-058, 062, 064, 065, 066, 068                          | **Q4, Q7, Q8 answered**          | Small                                  |

All 9 High findings are fixed after session 7. If the budget runs short, stop there: the rest is Medium and below.

## Rhythm for each session

**Before (you, about 5 min)**

1. Merge the previous batch's PR, if CI is green and you're happy with it.
2. Answer any open questions the next batch needs (`docs/review/07-OPEN-QUESTIONS.md`).
3. Open a **new** Claude Code session in `C:\Dev\projects\special-memory-analyzer`, with model **Sonnet**, and paste the prompt below with `BATCH` set.

**During (the session)**

1. Branch from up-to-date `main`: `fix/<batch>-<slug>`.
2. Baseline: `corepack pnpm check` (once B01 has landed; before that, `quality`, `test` and `rust:test`).
3. For each finding, in plan order: test first where marked TF, fix, scoped tests, then one commit `DSA-0NN: …`. Mark the register row "Fixed in `<sha>`".
4. Full gate, push, and one PR (not merged). Report back.

**If the usage limit is near (the session, or you telling it):**

- Finish the current finding, or `git stash` it. Never leave half a fix committed.
- Push the branch and open the PR as a **draft**, listing the IDs still to do.
- Write the remaining IDs in the PR body under "Not done yet".

The next session resumes with the resume prompt.

**After (you)**

- Glance at the PR's CI and the manual checklist in its body, then merge. Archive the session only after its branch is pushed (or merged): archiving can remove its worktree.

## Prompt: start a batch

```
You are implementing one batch of fixes from a completed audit of Deep Server Analyzer
(Tauri 2 + Rust + Vue 3, Windows). Repo: C:\Dev\projects\special-memory-analyzer.

BATCH = B02

Read, in order: CLAUDE.md; docs/review/10-SESSION-RUNBOOK.md; docs/review/04-IMPLEMENTATION-PLAN.md
(the "Ground rules" and the BATCH section only); the register rows in docs/review/02-FINDINGS-REGISTER.md
for BATCH's IDs; docs/review/07-OPEN-QUESTIONS.md (use my answers, else the defaults);
docs/review/06-TEST-AND-VERIFY.md.

Then follow the workflow in docs/review/05-SONNET-PROMPT.md exactly: branch from up-to-date origin/main,
baseline gate, one commit per DSA ID (test first where marked TF), mark register rows "Fixed in <sha>",
full gate, push, one PR into main, do not merge.

Budget: I'm on a usage limit. Work through the IDs in order. If you're running low, finish or stash the
current ID, push, open the PR as a draft with a "Not done yet" list, and stop.

Stop and ask me if: the baseline is red, a fix needs a file outside the batch or a new dependency, the
register's evidence doesn't match the code, or anything needs admin rights, a tag or a release.
Never run installer-kit\tests\Test-Kit.ps1 -Full locally. Run rust:test under pwsh.
```

## Prompt: resume an unfinished batch

```
Resume batch BATCH = B01 of the Deep Server Analyzer audit fixes (C:\Dev\projects\special-memory-analyzer).

Read CLAUDE.md, HANDOFF.md, and docs/review/10-SESSION-RUNBOOK.md. Then find the batch's branch and PR:
`git fetch origin; gh pr list --state open --search "<batch> in:title"`, or the branch named in HANDOFF.md.
Check it out, run `git log origin/main..HEAD` and the PR body to see which DSA IDs are done.
If HANDOFF.md names a git stash for an unfinished ID, `git stash apply` it first and finish that ID.
Add any missing "Fixed in <sha>" register marks for IDs already committed. Then
and continue with the remaining IDs from docs/review/04-IMPLEMENTATION-PLAN.md, following
docs/review/05-SONNET-PROMPT.md. Re-run the full gate, push, and mark the PR ready for review
(gh pr ready) when every ID is done. Do not merge.

Same budget rule: if you're running low, finish or stash the current ID, push, update the PR's
"Not done yet" list and stop.
```

## Prompt: run a batch in a cloud session

Use this when the session runs in the cloud on GitHub instead of locally. Cloud sessions start from a fresh clone of `main`, so the previous batch must be merged first. Set `BATCH` (B01 to B11) and paste it. Code behind `#[cfg(windows)]` is not compiled there; the Windows CI job is the check for it.

```
You are continuing the Deep Server Analyzer audit fixes in the GitHub repo Demonad112/special-memory-analyzer
(Tauri 2 + Rust workspace, Vue 3 + TypeScript). This is a CLOUD session: start from a fresh clone of `main`.
BATCH = <B0N>. DSA-018 (vite bump) stays skipped until the owner approves it.

SETUP (empty container):
  apt-get update && apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev     libjavascriptcoregtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev patchelf
  corepack pnpm install --frozen-lockfile
Rust 1.98.1 and Node 24.18.0 are pinned (rust-toolchain.toml, .nvmrc).

READ FIRST: CLAUDE.md, docs/review/10-SESSION-RUNBOOK.md (order table: confirm the previous batch is merged),
docs/review/04-IMPLEMENTATION-PLAN.md (section BATCH), docs/review/05-SONNET-PROMPT.md, and each ID's row in
docs/review/02-FINDINGS-REGISTER.md. HANDOFF.md may be stale; the code and the register win.

TASK: branch `fix/<batch>-<slug>` from up-to-date main. Fix the batch's IDs in the plan's order, test first where
marked TF. One commit per finding, subject "DSA-0NN: <imperative summary>", message ends with
"Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"; use `git commit -F <file>`. After each fix append
" **Fixed in `<short-sha>` (BATCH).**" to the row's Recommended fix cell and run prettier on the register.
New issues get the next free DSA number (DSA-071+) in the register; do not fix them here.

OWNER DECISIONS: elevated use is normal (Q3). Quick cleanups stay permanent but logged and protection-checked
(B04). Deletes go to the Recycle Bin by default and are logged to Logs\deletions.jsonl. Apache-2.0 only: never
copy GPL source. Other questions use the defaults in docs/review/07-OPEN-QUESTIONS.md.

CHECKS: run `corepack pnpm check`. Windows-only code is not compiled here: write its tests anyway and say in the
PR that they are verified by the Windows CI job only. Report failures as they are; never skip or weaken a test.

DELIVERY: push early; open a DRAFT PR titled "BATCH: <batch name> (<IDs>)" with the IDs done, a "Not done yet"
list, a "Windows-only, verified by CI only" section, ending "🤖 Generated with [Claude Code](https://claude.com/claude-code)".
Watch CI (`gh run list`, `gh run view <id> --log-failed`) and fix failures. When every ID is done and CI is green,
`gh pr ready`. DO NOT MERGE, tag, release or force-push. Never run installer-kit/tests/Test-Kit.ps1 -Full.

BUDGET: if usage runs low, finish or revert the current ID (never leave a half fix committed), push, update the
PR's "Not done yet" list and stop.
```

## Things only you do

- Merge PRs, answer questions in `07`, approve dependency bumps (Q-deps for vite).
- Release tags (`scripts\Set-ReleaseTag.ps1`). Suggested release points: after B04 (all delete-safety Highs) and after B07 (all Highs).
- Real-machine checks from each PR's manual checklist (`06-TEST-AND-VERIFY.md` §3), on scratch data only.
