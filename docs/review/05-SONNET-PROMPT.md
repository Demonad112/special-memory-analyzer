# Sonnet session prompt

Copy everything inside the fence into a new Claude Code session opened in `C:\Dev\projects\special-memory-analyzer`. Change `BATCH` to the batch you want, for example `B03`.

```
You are implementing fixes from a completed audit of Deep Server Analyzer (Tauri 2 + Rust + Vue 3, Windows).
The audit is in docs/review/. It is your only source of truth for what to change. Do not re-audit.

BATCH = B01

## Read first, in this order
1. docs/review/00-HANDOFF-README.md (orientation, owner answers)
2. docs/review/04-IMPLEMENTATION-PLAN.md: read the "Ground rules" and the section for BATCH only
3. docs/review/02-FINDINGS-REGISTER.md: read only the rows for the IDs listed in BATCH
4. docs/review/07-OPEN-QUESTIONS.md: use the owner's answer if one is filled in, otherwise the recommended default
5. docs/review/06-TEST-AND-VERIFY.md: commands and the minimum tests
6. CLAUDE.md (project rules; they override anything here if they conflict)

## Workflow
1. Run `git status` and `git fetch origin`. Branch from up-to-date origin/main as `fix/<batch-lowercase>-<slug>`.
   If the working tree has changes you did not make, stop and ask.
2. Run the baseline: `corepack pnpm quality`, `corepack pnpm test`, `corepack pnpm rust:test`.
   Run rust:test under pwsh, not Git Bash. If the baseline is red, stop and report. Do not fix unrelated failures.
3. For each finding ID in BATCH, in the order the plan gives:
   a. Re-read the cited lines. If the code no longer matches the register's evidence, stop on that finding,
      mark it "STALE: <what changed>" in the register's Recommended fix cell, and continue with the next one.
   b. Where the plan marks it TF, write the regression test first and confirm it fails.
   c. Apply the fix exactly as the register describes. Keep to the files the batch lists.
   d. Run the scoped tests (cargo test -p <crate> --manifest-path src-tauri/Cargo.toml, or
      corepack pnpm exec vitest run <file>), then commit:
      `git commit -F <msgfile>` with the subject `DSA-0NN: <imperative summary>`, a body explaining the change
      in 1-3 lines, and the trailer the session's attribution reminder gives.
   e. In docs/review/02-FINDINGS-REGISTER.md, append " **Fixed in `<short-sha>` (BATCH).**" to that row's
      Recommended fix cell. Commit the register update together with the fix, or as a follow-up commit in the same PR.
4. After the last finding, run the full gate (quality, test, rust:test) plus the batch's Acceptance steps.
   Manual checks you cannot do (running the GUI, a VM) go in the PR body as an unchecked checklist for the owner.
5. Push the branch and open ONE PR for the batch: `gh pr create --fill`, then edit the body to list each DSA ID
   with a one-line summary, the verification output summary (counts, not raw logs), and the manual checklist.
   Do not merge.
6. Final message: the IDs done, skipped or stale, test counts before and after, the PR link, and anything surprising.

## Stop and report (do not improvise) when
- A fix needs a file not listed for the batch, a new dependency, a version bump, or a change to a saved
  on-disk format beyond adding a #[serde(default)] field.
- A test that passed in the baseline now fails and the cause is not your change.
- The register's evidence does not match the code (step 3a), or two findings' fixes conflict.
- A step needs administrator rights, a real install, pushing a tag, or touching the machine certificate stores
  (installer-kit Test-Kit.ps1 -Full is CI-only).
- Anything would delete user data outside a temp folder created by a test.

## Must not
- Fix anything outside BATCH, even a one-line issue you notice. Add a new row at the end of the register
  (next free DSA number, Status INFERRED) and mention it in the PR instead.
- Swap, add or upgrade dependencies (npm or cargo) unless the batch explicitly says the owner approved it in 07.
- Reformat unrelated files, rename public commands, or change Tauri command signatures beyond what the
  register specifies (for those, also update the src/lib/*.ts caller in the same commit).
- Weaken, skip or delete a test to get green. Do not use #[ignore] or .skip on anything that was running before.
- Force-push, push to main, push tags, or merge PRs.
- Copy code from GPL projects (WinDirStat etc.). The project is Apache-2.0.
- Use raw colours in Vue. Use the --app-* tokens in src/styles.css.
```

## Notes for the owner

- Batches B02, B03, B05, B06 and B08 can run as separate sessions at the same time; see the independence table in `04`. Merge them in plan order.
- Fill in `07-OPEN-QUESTIONS.md` before B01 (Q-deps), B04 (Q1, Q3) and B11 (Q4, Q7, Q8). Otherwise the session uses the defaults.
- A batch that turns out too large for one session: tell Sonnet to stop after the current finding and open the PR with what is done. The next session picks up the remaining IDs.
