# Running the Rust tests on Windows

Run `corepack pnpm rust:test` under **pwsh** (PowerShell), not Git Bash.

## Evidence

- Under Git Bash, two `transfer-core` tests fail 2/2 on the same runner:
  - `unreadable_folders_are_listed_as_not_copied_before_the_copy`
  - `an_unreadable_folder_is_copied_by_retry_once_readable`

  Both rely on an `icacls` deny rule to make a folder unreadable.

- Under pwsh, the same tests pass on the same runner.

## Cause

The root cause under Git Bash is unknown. App code is not implicated. Do not change app code to make these tests pass under Git Bash; run them under pwsh.

## CI

`.github/workflows/ci.yml` has no `shell:` override on the `rust:test` step, so it runs under the default Windows shell (pwsh). The only `shell: bash` in the workflows is the tag-check step in `.github/workflows/release.yml`, which does not run the Rust tests.
