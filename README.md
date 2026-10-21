# Project template

Starter for new projects that use Claude Code, on the desktop app and in cloud (web/mobile) sessions.

## Use it

1. Click **Use this template** on GitHub and create the new repo.
2. Fill in `CLAUDE.md` (commands, layout, gotchas). Keep it short.
3. Install the Claude GitHub app on the new repo so cloud sessions can push and open PRs.

Cloud sessions start with an empty container. If your project needs dependencies installed, ask Claude to install them at the start of the session (or to add a setup hook if you want it automatic).

## What is here

| Path | Purpose |
|---|---|
| `CLAUDE.md` | Project notes Claude reads every session |
| `.claude/settings.json` | Allow-list for read-only git/gh and common test/lint/build commands; force-push deny |
| `.github/pull_request_template.md` | Default PR body |
| `.gitignore` | Ignores local Claude settings and worktrees |

Personal preferences belong in `~/.claude/` or `CLAUDE.local.md`, not in the repo.
