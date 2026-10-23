# Audit handoff: Deep Server Analyzer

The audit covers `Demonad112/special-memory-analyzer` at `302f751` (v1.1.1) and was done on 2026-10-22. It was analysis only: no source file was changed, and everything is in `docs/review/`.

## Executive summary

1. **Health: 6.5/10.** The core is well built: clean crate split, strict TypeScript, clippy-clean, 109 TS and about 195 Rust tests all passing. There are no secrets in the code or its history, and the app has no network attack surface.
2. **Findings:** 70 in total: **0 Critical, 9 High, 27 Medium, 29 Low, 5 Enhancement.**
3. **Risk 1 (DSA-004).** The USB kit's integrity anchor (`thumbprint.txt`, `SHA256SUMS.txt`) ships inside the same zip it verifies. The kit then adds the certificate to `LocalMachine\Root` without checking it is code-signing-only. A tampered kit means an attacker root CA on the server.
4. **Risk 2 (DSA-001).** The never-delete list compares strings, so `C:\PROGRA~1`, `\\localhost\c$` or a `subst` drive gets past it when the app runs elevated.
5. **Risk 3 (DSA-002, DSA-003).** The single-item delete and the Quick cleanups bypass `deletions.jsonl`. Quick cleanups also delete permanently from folders read out of environment variables, with no protection check.
6. **Risk 4 (DSA-006, DSA-008, DSA-009).** A Record report can be wrong: a retried hash mismatch is reported as "identical", a crashed run never lists the files it did not reach, and one torn JSONL line makes the run unreadable.
7. **Risk 5 (DSA-005).** The kit's SYSTEM scheduled task runs scripts from a `ProgramData` folder whose ACL is not hardened, which is a possible local privilege escalation.
8. **Root causes:** duplicated safety code (several delete paths, three link checks), paths compared as strings at trust boundaries, crash consistency designed only for the happy path, and a self-referential trust anchor.
9. **Effort:** 50 S, 18 M and 2 L items, about 4–5 engineer-weeks at face value. That is planned as **11 Sonnet batches**, with B01–B04 and B06 (about a week) clearing every High.
10. **Do first:** B01 (CI and audit baseline), then B02 (kit trust) and B03/B04 (delete safety) in parallel, then B06 and B07 (Record integrity).

## Documents

| File                        | What it is                                                                                                                    | Read it when              |
| --------------------------- | ----------------------------------------------------------------------------------------------------------------------------- | ------------------------- |
| `00-HANDOFF-README.md`      | This index                                                                                                                    | First                     |
| `01-REVIEW-REPORT.md`       | Narrative: scope, method, all 13 areas (including "nothing found"), strengths, root causes, revisions log                     | You want the why          |
| `02-FINDINGS-REGISTER.md`   | **Source of truth.** One row per finding, `DSA-001` to `DSA-070`, with location, evidence, exact fix, effort and dependencies | Every fix                 |
| `03-ARCHITECTURE.md`        | Module map, data flow, artefacts, trust boundaries, release and install sequence (Mermaid), target structure                  | Before B03, B04, B09, B10 |
| `04-IMPLEMENTATION-PLAN.md` | 11 ordered batches with files, steps, acceptance and rollback, plus a parallelism table                                       | Each session              |
| `05-SONNET-PROMPT.md`       | Copy-paste prompt for the implementation session                                                                              | Starting a batch          |
| `06-TEST-AND-VERIFY.md`     | Commands, observed baseline, manual smoke checklist, minimum new tests                                                        | Each session              |
| `07-OPEN-QUESTIONS.md`      | 10 owner decisions, each with a default Sonnet uses if it is unanswered                                                       | **Now (owner)**           |
| `08-DEPENDENCY-REPORT.md`   | npm audit detail, Cargo notes, proposed `deny.toml`                                                                           | B01                       |
| `09-SECURITY-NOTES.md`      | Threat model for release → USB kit → server, and the elevated-app attack paths                                                | B02, B04, B05             |
| `CLAUDE.proposed.md`        | Proposed replacement for `CLAUDE.md`                                                                                          | B11 (owner decides)       |

## How to use this in the Sonnet session

1. The owner fills in `07-OPEN-QUESTIONS.md`, or at least Q-deps, Q1 and Q3.
2. Open a new Claude Code session in the repo, paste the fenced block from `05-SONNET-PROMPT.md`, and set `BATCH = B01`.
3. Sonnet works one batch per session: a branch, one commit per `DSA-0NN`, register rows marked "Fixed in", and one PR. It stops on any surprise.
4. Review and merge the PR, then start the next batch. Batches marked independent in `04` can run as parallel sessions.
5. New issues Sonnet finds become new register rows (DSA-071 onward), not drive-by fixes.

## What was NOT reviewed, and why

| Not reviewed                                                                                                       | Why                                                                                                                                                    | Suggested follow-up                            |
| ------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------- |
| Running the app, the kit install path, or the release workflow                                                     | The review rules allow only safe, read-only execution                                                                                                  | The manual smoke list in `06` after each batch |
| `cargo audit` / `cargo deny`                                                                                       | Not installed; nothing outside a throwaway environment may be installed                                                                                | Added to CI in B01                             |
| GitHub repo settings (branch and tag protection, collaborators, Actions permissions) and recent CI runs            | No GitHub API calls were made                                                                                                                          | Owner check (DSA-064, Q3)                      |
| Tauri's NSIS installer template and the WebView2 bootstrapper                                                      | Upstream or Microsoft code                                                                                                                             | –                                              |
| Behaviour on Windows Server and on ARM64                                                                           | No such environment                                                                                                                                    | Q6                                             |
| Line-by-line review of every frontend component                                                                    | Covered by one sweep plus spot checks. Smaller components (`TreemapView`, `DriveTiles`, `JunkPanel`, `QuickWins`, `ShareSignIn`) got lighter attention | –                                              |
| The Win32 internals of `cleanup-core::os` (recycle, bin parsing) and `transfer-core::secure` (ACL and stream copy) | Read for misuse patterns only; the unsafe FFI was not audited for soundness                                                                            | Optional targeted review if those files change |
| Branches other than the current one (`feat/*`, `ci/usb-kit-signing`)                                               | Out of scope: the audit is of the shipped line                                                                                                         | –                                              |

## Differences between `CLAUDE.proposed.md` and the existing `CLAUDE.md`

The existing `CLAUDE.md` is untouched. The proposal:

- **Adds:**
  - the `check` script, `audit`/`cargo deny`, the kit test commands and the release scripts
  - the crate dependency direction
  - the `DSA-0NN` commit convention, with register upkeep
  - a "no new dependencies or bumps without OK" rule
  - a **Safety rules** section that turns this audit's root causes into standing rules (one delete pipeline, canonical paths, reparse ancestors, cloud files never opened in any mode, atomic writes and tolerant JSONL, no heavy sync commands, request-sequence guards in the frontend, ids rather than paths over IPC, kit certificate and ACL checks)
  - "the env-override data folders are protected"
- **Keeps word for word:** the owner process preference, the force-push deny note, the cloud-session setup, the colour-token rule, `#[serde(default)]`, Recycle Bin by default, Apache-2.0 only, and the fixture refresh command.
- **Changes:** "Run quality and tests before opening a PR" becomes "Run `check`".
- **Note:** some rules in the proposal describe the code _after_ the batches land (`pathsafe`, `canonical_root`, `compareJob`, `rust-toolchain.toml`). Merge it in B11, not before.

## Repository state at the end of this review

The only files added are under `docs/review/`. The untracked root `HANDOFF.md` was already there before the review and was not touched. The branch is `docs/unblock-step-smart-app-control`. Nothing was committed or pushed.
