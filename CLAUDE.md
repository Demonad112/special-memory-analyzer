# <Project name>

<One sentence: what this project is and its stack. Optional.>

## Commands

Keep the lines for this project's stack and delete the rest.

```bash
# JavaScript / TypeScript (pnpm via corepack; swap for npm or yarn)
corepack pnpm install
corepack pnpm dev
corepack pnpm test            # vitest; scope with: corepack pnpm exec vitest run <file>
corepack pnpm lint
corepack pnpm exec vue-tsc --noEmit     # or: corepack pnpm exec tsc --noEmit
corepack pnpm exec prettier --check .

# Rust
cargo build
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings

# Tauri
corepack pnpm tauri dev
corepack pnpm tauri build

# Python
pip install -r requirements.txt
pytest

# GitHub
gh pr create --fill
gh pr view --web
gh run list --limit 5
gh run view <id> --log-failed
```

Run lint, typecheck and tests before opening a PR. Scope tests to the touched files when the full suite is slow.

## Layout

- `src/`: application code
- `tests/`: tests, mirroring `src/`
- `docs/`: notes that outlive a session

## Conventions

- Match the style of the surrounding code; don't reformat unrelated files.
- Keep changes focused. One concern per branch and PR.
- Add or update tests with behaviour changes.
- Never commit secrets, `.env` files or credentials.
- Don't force-push, and don't push straight to `main`; use a branch and a PR.
- Report test failures as they are; don't skip or weaken a test to get green.

## Environment notes

- Windows desktop: the shell may be Git Bash or PowerShell. Heredocs in Bash halve backslashes, so write files containing backslashes with the file tools.
- Cloud (web/mobile) sessions start with an empty container: install dependencies first.

## Gotchas

- <Only things that can't be learned from reading the code: required env vars, quirks, traps. Add a line when something trips Claude twice.>
