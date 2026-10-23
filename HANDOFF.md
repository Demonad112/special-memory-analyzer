# HANDOFF: Deep Server Analyzer, audit done, fixes not started

Date: 2026-10-23. Repo: `C:\Dev\projects\special-memory-analyzer` (remote `Demonad112/special-memory-analyzer`).

## Status at a glance

| Item        | State                                                                                                                                                                                                                           |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `main`      | `4451855` "docs: cover USB kit, release scripts and audit in CLAUDE.md". It includes PR #17 (kit unblock guide) and PR #18 (audit, `docs/review/`). CI run 39902464682 on `4451855` was still in progress when this was written |
| Open PRs    | Only the PR that adds this file. No fix batches have started                                                                                                                                                                    |
| Releases    | `v1.1.0`, `v1.1.1` (Build installer: success), `v1.1.1-test.1` (success)                                                                                                                                                        |
| Uncommitted | None                                                                                                                                                                                                                            |

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
3. Start the fix work: a new session on up-to-date `main` (after the audit docs are merged), pasting `docs/review/05-SONNET-PROMPT.md` with `BATCH = B01`.
4. After B01: B02 (kit) and B03 (delete safety I) can run in parallel. Then B04, then B05/B06. See the table in `04-IMPLEMENTATION-PLAN.md`.

## Rules to keep

- Read `CLAUDE.md` first. The owner's preference there overrides process rules; the force-push deny in `.claude/settings.json` stays.
- One commit per finding, `DSA-0NN: <summary>`. Mark the register row "Fixed in `<sha>`". New issues get the next free DSA number (DSA-071 onward), not drive-by fixes.
- Don't push, tag, release or merge unless the owner asks in that message. "I pushed it, continue" is not permission to push.
- No dependency bumps or new deps without approval; then run the full build including `vue-tsc`.
- Run `rust:test` under pwsh. Never run `installer-kit\tests\Test-Kit.ps1 -Full` on the dev machine: it changes LocalMachine certificate stores.
- Apache-2.0 only. Colours are `--app-*` tokens. Commit messages use `git commit -F <file>` and end with the attribution line the session gives.
