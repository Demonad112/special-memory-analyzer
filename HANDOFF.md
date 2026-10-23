# HANDOFF: Deep Server Analyzer, audit done, B01 stopped part-way

Date: 2026-10-23. Repo: `C:\Dev\projects\special-memory-analyzer` (remote `Demonad112/special-memory-analyzer`).

## Status at a glance

| Item        | State                                                                                                                                                                        |
| ----------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `main`      | Includes PR #17 (kit unblock guide), #18 (audit, `docs/review/`), #19 (this file) and the CLAUDE.md update. CI on `main` after #19: success (run 39906332765)                |
| Open PRs    | [#20](https://github.com/Demonad112/special-memory-analyzer/pull/20) **draft**: B01, branch `fix/b01-safety-net`, 3 of 4 findings done. See "B01 state at the 11:50 pm stop" |
| Releases    | `v1.1.0`, `v1.1.1` (Build installer: success), `v1.1.1-test.1` (success)                                                                                                     |
| Uncommitted | None                                                                                                                                                                         |

## B01 state at the 11:50 pm stop (2026-10-22)

- **Branch:** `fix/b01-safety-net` on origin (head `58e4c1b`), [PR #20](https://github.com/Demonad112/special-memory-analyzer/pull/20) as a **draft**, not merged. The local worktree branch `Claude/b01-safety-net-27168c` (in `.claude/worktrees/b01-safety-net-27168c`) is the same commits.
- **Done and committed:**
  - DSA-059 (`36f021e`): `check` script
  - DSA-063 (`4b194b5`): Rust 1.98.1 and Node 24.18.0 pinned
  - DSA-020 (`296b201` and `58e4c1b`): CI audit job, `deny.toml`, every scanner runs
- **Not done: DSA-069** (vitest `environment: 'node'` + jsdom pragmas). The work in progress is **uncommitted**, saved as a git stash in this repo: `B01 DSA-069 WIP: vitest environment node + jsdom pragmas (unverified)`. It covers 7 files, +7/−1. One test was failing under it and the cause wasn't found.
  - Restore with `git stash list`, then `git stash apply stash@{N}` on the B01 branch.
  - The stash lives in the repo's shared refs, so it survives archiving the worktree. If it's ever lost, redo the change from the register row.
- **Also not done:** the "Fixed in" marks in `02-FINDINGS-REGISTER.md` for DSA-059, 063 and 020.
- **Skipped by design:** DSA-018 (vite bump), pending Q-deps approval.
- **Unverified locally:** rust:fmt:check, clippy and rust:test on the pinned toolchain. Smart App Control blocked them at first; it's now off, but the re-run was cut off. CI on PR #20 will show them.
- **Safe to archive:** the B01 session (its commits are pushed and its WIP is stashed) and this audit session.

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

## Next steps (in order)

1. Done 2026-10-23: PRs #17 and #18 (audit) merged; `main` is at `4451855` (CLAUDE.md now covers the USB kit, release scripts and `docs/review/`).
2. **Owner:** answer at least Q1, Q3 and Q-deps in `docs/review/07-OPEN-QUESTIONS.md`. Unanswered questions use the defaults written there.
3. **Work one session at a time** following `docs/review/10-SESSION-RUNBOOK.md`: the order table, the per-session rhythm, and the two prompts (start a batch / resume a batch). Sessions run one after another, not in parallel, to keep usage within the weekly budget.
4. First session: finish B01 (see "B01 state at the 11:50 pm stop" below), using the resume prompt.

## Rules to keep

- Read `CLAUDE.md` first. The owner's preference there overrides process rules; the force-push deny in `.claude/settings.json` stays.
- One commit per finding, `DSA-0NN: <summary>`. Mark the register row "Fixed in `<sha>`". New issues get the next free DSA number (DSA-071 onward), not drive-by fixes.
- Don't push, tag, release or merge unless the owner asks in that message. "I pushed it, continue" is not permission to push.
- No dependency bumps or new deps without approval; then run the full build including `vue-tsc`.
- Run `rust:test` under pwsh. Never run `installer-kit\tests\Test-Kit.ps1 -Full` on the dev machine: it changes LocalMachine certificate stores.
- Apache-2.0 only. Colours are `--app-*` tokens. Commit messages use `git commit -F <file>` and end with the attribution line the session gives.
