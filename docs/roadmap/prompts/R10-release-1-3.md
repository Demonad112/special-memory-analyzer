# R10: Release 1.3.0

Features: version 1.3.0 and the CHANGELOG. Size: Small.

**Before you paste:** R01 to R09 MERGED.

**Parallel:** None: run last.

Paste everything inside the block into a new LOCAL Claude Code session in `C:\Dev\projects\special-memory-analyzer` (its own worktree).

```
You are finishing Deep Server Analyzer 1.3 in C:\Dev\projects\special-memory-analyzer (GitHub:
Demonad112/special-memory-analyzer; Tauri 2 + Rust workspace, Vue 3 + TypeScript). This is a LOCAL session on
Windows in its own worktree, cut from up-to-date `main`. Use pwsh for commands (docs/windows-testing.md).
BATCH = R10 (Release 1.3.0). Features in this batch: version 1.3.0 and the CHANGELOG.

SETUP: corepack pnpm install --frozen-lockfile

READ FIRST: CLAUDE.md, docs/roadmap/1.3.md (rules, the register rows for this batch, Decisions D-01 to D-24),
and docs/roadmap/1.3-plan.md (ground rules and section R10). Then read the code those sections name. Where the
docs and the code disagree, the code wins: adapt, and say what you changed in the PR.

TASK: branch `chore/r10-release-1-3` from up-to-date main. Do the steps in section R10 of the plan, in order. One commit for the version bump ("chore: bump version to 1.3.0") and one for the changelog and docs ("docs: 1.3.0 changelog").
Use `git commit -F <file>`; the message ends with the Co-Authored-By line for the model you run as (for example
"Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>"). After each feature is complete, append
" **Done in `<short-sha>` (R10).**" to its Status cell in docs/roadmap/1.3.md and run
`corepack pnpm exec prettier --write docs/roadmap/1.3.md`. Problems you find outside this batch go in the PR under
"Found along the way"; do not fix them here.

RULES (CLAUDE.md safety rules; never relax them):
  - Tests with every behaviour change, in the same commit. Split pure logic from Windows calls so it is tested here.
  - Saved formats change additively only: every new saved field gets #[serde(default)]; never rename or remove a
    saved field; a new report kind is a new Body variant.
  - Every delete goes through cleanup_core::bulk::run and a Logs\deletions.jsonl line. Never call recycle or
    delete_permanently from a command.
  - Folder walks never follow reparse points (scan tree Kind::Link, cleanup_core::pathsafe).
  - OneDrive online-only files are never opened, read or hashed, in any mode.
  - Long jobs are async commands + spawn_blocking, stream progress about every 100 ms, keep big results in managed
    state; the UI loads one folder level at a time; every await that writes shared state checks a sequence or
    owner token. Scans take the scan guard so they never overlap an elevated record.
  - Webview commands take ids; a path that must cross (a folder to scan, an export target) is validated and
    canonicalized once in Rust (scan_core::paths::canonical_root). Errors go through report_core::applog::error.
  - Every colour is an --app-* token in src/styles.css. No raw hex or Tailwind palette colours.
  - No new crates. Only the crate features that 1.3.md D-13 and D-24 allow; name each in the PR. If a Cargo.toml
    changed, run `cargo deny --manifest-path src-tauri/Cargo.toml check advisories licenses bans`.
  - Apache-2.0 only: never copy source from GPL tools such as WinDirStat. Microsoft docs are fine as a reference.

CHECKS: run `corepack pnpm check` before marking the PR ready; scope tests while iterating
(`corepack pnpm exec vitest run <file>`, `cargo test -p <crate> --manifest-path src-tauri/Cargo.toml`).
Local cargo test binaries may be blocked by Defender ASR on this machine: if `rust:test` is blocked, say so in the PR and rely on the Windows CI job; `quality` and `test` must pass locally. Report failures as they are; never skip or weaken a test.

DELIVERY: push early; open a DRAFT PR titled "R10: Release 1.3.0" with the steps done, a "Not done yet" list, a
"Windows-only, verified by CI only" section, "Found along the way", and the crate features added (or "none"),
ending "🤖 Generated with [Claude Code](https://claude.com/claude-code)". Watch CI (`gh run list`,
`gh run view <id> --log-failed`) and fix failures. When every step is done and CI is green, `gh pr ready`.
DO NOT MERGE, tag, release or force-push. Never run installer-kit/tests/Test-Kit.ps1 -Full.
MERGE HOLD: the owner merges 1.3 PRs only after v1.2.0 is tagged; that does not stop you opening the PR.

BUDGET: if usage runs low, finish or revert the current step (never leave a half step committed), push, update
the PR's "Not done yet" list and stop.

BATCH NOTES (R10): Every row in the 1.3.md register must say "Done in"; if one does not, stop and report it. Bump package.json, src-tauri/tauri.conf.json, src-tauri/Cargo.toml and the matching Cargo.lock entry to 1.3.0 (tags must match all three). Add "## 1.3.0" at the top of CHANGELOG.md (on main since PR #39; create it with a "# Changelog" heading only if it is missing), grouped like 1.2.0, one plain user-facing line per behaviour. Put the plan's "1.3 checks" list in the PR for the owner. Do not tag: the owner runs scripts\Set-ReleaseTag.ps1 -Version v1.3.0 after the checks.
PREREQUISITES: R01 to R09 MERGED. If a prerequisite batch is not merged to main, stop and say so instead of starting.
RESUME: first run `git fetch origin; gh pr list --state open --search "R10 in:title"`. If a branch or PR for this
batch already exists, check it out, read the PR body's "Not done yet" list and finish those steps instead of
starting over. Steps already marked "Done in" in docs/roadmap/1.3.md are done.
```
