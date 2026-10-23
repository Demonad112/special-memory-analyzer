# HANDOFF: Deep Server Analyzer, audit fixes, B01 and B02 done, B03 next

Date: 2026-10-23. Repo: `C:\Dev\projects\special-memory-analyzer` (remote `Demonad112/special-memory-analyzer`).

## Status at a glance

| Item        | State                                                                                                                                                                                                                                                                               |
| ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| B01         | Done. [PR #20](https://github.com/Demonad112/special-memory-analyzer/pull/20) merged (DSA-059, 063, 020); [PR #21](https://github.com/Demonad112/special-memory-analyzer/pull/21) (DSA-069 and register marks) merged by the owner. DSA-018 skipped on purpose (vite bump, Q-deps). |
| B02         | Done. [PR #22](https://github.com/Demonad112/special-memory-analyzer/pull/22) merged by the owner (DSA-004, 005, 019, 067).                                                                                                                                                         |
| Next        | **B03: Delete safety I** (DSA-002, 013, 014, 016, 017, 028). Not started.                                                                                                                                                                                                           |
| Open PRs    | None expected. Run `gh pr list --state open` to confirm.                                                                                                                                                                                                                            |
| Uncommitted | This file only (HANDOFF.md was edited on the local `main` checkout and not committed). Nothing else is built-but-unpushed.                                                                                                                                                          |

## Things the next session must know

- **Local Rust checks are blocked on this machine.** `cargo` build scripts fail with `Access denied (os error 5)` (fresh unsigned binaries; Smart App Control is off, so Defender ASR is the likely cause). So `clippy` and `rust:test` cannot run here, and **B03 is Rust-heavy**. Options: ask the owner to allow the repo folder in Defender/ASR, or rely on CI (`gh run view <id> --log-failed`) and say so in the PR. Do not change security settings yourself.
- `corepack pnpm check` runs quality, vitest and `rust:test`; prettier, vue-tsc, rustfmt and vitest work locally.
- B02 changed the kit: `-NoPrompt` now needs `-ExpectedThumbprint`. Never run `installer-kit	ests\Test-Kit.ps1 -Full` locally.
- Use `git commit -F <file>` and end messages with the Co-Authored-By line. In PowerShell, quote stash refs (`'stash@{0}'`). Python heredocs in Git Bash mangle backslashes; use the file tools for Windows paths.
- Work in a fresh worktree or branch from up-to-date `main`: `fix/b03-delete-safety`. The old worktree `.claude/worktrees/b01-safety-net-27168c` held B01 and B02 and can be archived once PRs #21 and #22 are merged.

## Owner decisions (defaults the owner accepted for B03 onward)

| Q      | Decision                                                            |
| ------ | ------------------------------------------------------------------- |
| Q1     | Quick cleanups stay permanent deletes, but logged (DSA-003, in B04) |
| Q3     | Elevated use is normal, so DSA-001, 012, 015 stay High/Medium       |
| Q-deps | No vite bump until the full build is verified; then approve         |
| Q4     | Canonical repo is `Demonad112/special-memory-analyzer`              |
| Q8     | No ESLint; add `@vue/test-utils` (B11)                              |

The rest use the defaults in `docs/review/07-OPEN-QUESTIONS.md`.

## Continuation prompt for the next session

```
Start batch BATCH = B03 (Delete safety I) of the Deep Server Analyzer audit fixes
(C:\Dev\projects\special-memory-analyzer). PRs #21 and #22 are merged.

Read CLAUDE.md, HANDOFF.md and docs/review/10-SESSION-RUNBOOK.md. Then `git switch main; git pull`,
branch fix/b03-delete-safety, and follow docs/review/04-IMPLEMENTATION-PLAN.md (B03) and
docs/review/05-SONNET-PROMPT.md: DSA-017, 016, ... in the plan's order, test first where marked TF,
one commit per finding "DSA-0NN: ...", a "Fixed in <sha> (B03)" mark in 02-FINDINGS-REGISTER.md.
Local cargo builds fail with "Access denied (os error 5)": do not retry or change security settings;
run prettier, vue-tsc, rustfmt and vitest locally and let CI run clippy and rust:test.
Push, open one PR (draft if unfinished, with a "Not done yet" list) and do not merge.
Budget: I'm on a usage limit. If low, finish or stash the current ID, push, update the PR and stop.
Never run installer-kit	ests\Test-Kit.ps1 -Full locally.
```

## What was done this session

A read-only principal-engineer audit, plus a handoff for a cheaper model (Sonnet). The application source is unchanged. Everything is in `docs/review/`:

| File                        | Purpose                                                                        |
| --------------------------- | ------------------------------------------------------------------------------ |
| `00-HANDOFF-README.md`      | Index, executive summary, what was not reviewed                                |
| `01-REVIEW-REPORT.md`       | All 13 areas, strengths, root causes, revisions log                            |
| `02-FINDINGS-REGISTER.md`   | **Source of truth**: DSA-001 to DSA-070, with location, evidence and exact fix |
| `03-ARCHITECTURE.md`        | Module map, data flow, trust boundaries, release flow (Mermaid)                |
| `04-IMPLEMENTATION-PLAN.md` | 11 batches (B01 to B11), files, steps, acceptance, parallelism                 |
| `05-SONNET-PROMPT.md`       | Copy-paste prompt for each fix session                                         |
| `06-TEST-AND-VERIFY.md`     | Commands, baseline, manual smoke test, minimum tests                           |
| `07-OPEN-QUESTIONS.md`      | Owner decisions Q1 to Q10 + Q-deps, each with a default                        |
| `08-DEPENDENCY-REPORT.md`   | npm audit, Cargo notes, proposed `deny.toml`                                   |
| `09-SECURITY-NOTES.md`      | Threat model for release → USB kit → server                                    |
| `CLAUDE.proposed.md`        | Proposed `CLAUDE.md` (merge in B11, not before)                                |

**Findings:** 0 Critical, 9 High, 27 Medium, 29 Low, 5 Enhancement (70 in total). The Highs are:

- DSA-001: the never-delete list compares strings, so short names, `\\localhost\c$` and `subst` bypass it
- DSA-002: the single-item delete is unlogged and has no reparse check
- DSA-003: Quick cleanups delete permanently, unlogged, with no protection check
- DSA-004: the kit trusts a certificate in LocalMachine\Root with no EKU/CA check, and its anchor ships inside the same zip
- DSA-005: the SYSTEM task runs scripts from an un-hardened `ProgramData\DSA`
- DSA-006: a retried hash mismatch is recorded as identical
- DSA-007: Watch mode reads OneDrive online-only files
- DSA-008: a crashed run stays `Running` and loses unreached files
- DSA-009: one torn JSONL line makes a run unreadable

**Baseline at `302f751` (2026-10-22):**

- vitest: 109/109 pass. Rust: all pass (cleanup-core 62, report-core 51 + 1 ignored, scan-core 23, transfer-core 59). vue-tsc, rustfmt and clippy are clean.
- `pnpm quality` failed locally **only** because the then-untracked `HANDOFF.md` was not prettier-formatted. That file is now formatted and committed, so B01 step 1 (the `.prettierignore` entry) and Q10 are no longer needed.
- `pnpm audit --prod`: 6 advisories, all vite 8.3.2 (dev/build only); the fix is a bump to 8.3.3 or later, which needs approval.
- `cargo audit`/`cargo deny`: not installed, not run.

Not verified in this session: running the app, the kit install path, and the release workflow.

## Rules to keep

- Read `CLAUDE.md` first. The owner's preference there overrides process rules; the force-push deny in `.claude/settings.json` stays.
- One commit per finding, `DSA-0NN: <summary>`. Mark the register row "Fixed in `<sha>`". New issues get the next free DSA number (DSA-071 onward), not drive-by fixes.
- Don't push, tag, release or merge unless the owner asks in that message. "I pushed it, continue" is not permission to push.
- No dependency bumps or new deps without approval; then run the full build including `vue-tsc`.
- Run `rust:test` under pwsh. Never run `installer-kit\tests\Test-Kit.ps1 -Full` on the dev machine: it changes LocalMachine certificate stores.
- Apache-2.0 only. Colours are `--app-*` tokens. Commit messages use `git commit -F <file>` and end with the attribution line the session gives.
