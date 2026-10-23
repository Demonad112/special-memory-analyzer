# Personal playbook: running the audit fixes one batch at a time

B01 and B02 are done and merged. Everything else runs from a **cloud session** on `Demonad112/special-memory-analyzer`, one batch at a time, using the prompt files in `docs/review/prompts/`.

## The loop (every batch)

1. Check the "Before you paste" line at the top of the prompt file. Every batch it names must already be **merged to `main`**.
2. Open a new cloud session on the repo, paste the whole code block from the prompt file, and let it run.
3. The session opens a **draft PR**, watches CI, fixes failures, then marks it ready. It never merges.
4. Review the PR: CI green, the "Not done yet" list empty, the "Windows-only, verified by CI only" section read.
5. **You merge it.** Then run the next prompt in the table.
6. If the session stopped on the usage limit, the PR has a "Not done yet" list. Run the **same prompt file again**: it resumes on the existing branch and finishes the remaining IDs.

If two PRs both edit `02-FINDINGS-REGISTER.md`, the second to merge conflicts. Fix: merge `main` in, take `main`'s version of that file, and re-add that batch's "Fixed in" marks.

## Order

| Run | Prompt file                         | Findings                                                  | Start only when               | Notes                                                                               |
| --- | ----------------------------------- | --------------------------------------------------------- | ----------------------------- | ----------------------------------------------------------------------------------- |
| 1   | `prompts/B03-delete-safety.md`      | DSA-002, 013, 014, 016, 017, 028                          | B01 merged (done)             | Running now. Windows-only paths verified by CI only.                                |
| 1b  | `prompts/B08-frontend-ux.md`        | DSA-025, 026, 027, 035, 036, 043, 044, 051, 055, 057, 061 | B01 merged (done)             | Optional, **can run at the same time as B03** (src/ only).                          |
| 2   | `prompts/B04-delete-safety-2.md`    | DSA-001, 003, 012                                         | **B03 merged**                | Q1, Q3 decided.                                                                     |
| 3   | `prompts/B05-app-surface.md`        | DSA-010, 011, 024, 029, 037, 038, 039, 047                | **B04 merged**                | Edits Tauri permissions: read the PR's list.                                        |
| 4   | `prompts/B06-record-integrity.md`   | DSA-006, 009, 021, 023, 040, 046, 049, 050                | **B05 merged**                | Additive report fields only.                                                        |
| 5   | `prompts/B07-record-integrity-2.md` | DSA-007, 008, 015, 022, 034                               | **B06 merged**                | **All 9 High findings are fixed after this.** Fine place to stop if usage is short. |
| 6   | `prompts/B08-frontend-ux.md`        | (see 1b)                                                  | B01 merged                    | Skip if you ran it alongside B03.                                                   |
| 7   | `prompts/B09-performance.md`        | DSA-030, 031, 032, 033, 052                               | **B03, B05, B06 merged**      | Wants before/after numbers in the PR.                                               |
| 8   | `prompts/B10-refactor.md`           | DSA-041, 042, 045, 048, 053, 054, 056, 060, 070           | **B01 to B09 all merged**     | Run **twice** (it says which half), merge between. Run alone.                       |
| 9   | `prompts/B11-docs-deps.md`          | DSA-058, 062, 064, 065, 066, 068                          | Q4, Q7, Q8 decided (they are) | Small. Dependency changes: check the full build. Can run any time after B01.        |

## Decisions already made (these are in each prompt)

- Quick cleanups stay permanent deletes, but logged and protection-checked (B04).
- Elevated (Administrator) use is normal, so DSA-001, 012 and 015 stay High or Medium.
- Canonical repo is `Demonad112/special-memory-analyzer`.
- No ESLint; add `@vue/test-utils`. Report dates stay `en-US`; the UI follows the user locale.
- Report and run formats change additively only.
- DSA-018 (vite bump) stays skipped until you approve Q-deps. Tell B11's session when you do.

## Things only you do

- Merge each PR. Cloud sessions branch from `main`, so an unmerged batch is invisible to the next one.
- Release tags (`scripts\Set-ReleaseTag.ps1 -Version vX.Y.Z`). Nothing here pushes tags.
- Never run `installer-kit\tests\Test-Kit.ps1 -Full` on your own machine.
- Local `cargo` builds fail on the dev machine ("Access denied", os error 5). That is why this runs in the cloud and CI.

## If something goes wrong

- A cloud session reports a prerequisite is not merged: merge it, then paste the prompt again.
- CI fails on a Windows-only test that Linux could not run: leave the PR as is. The session fixes it from the CI log, or paste the prompt again.
- A batch grew too big for the usage limit: let it stop, merge what it finished (its PR lists what is left), and re-run the same prompt for the rest.
- Unsure of the current state: read `HANDOFF.md` and `gh pr list --state open`.
