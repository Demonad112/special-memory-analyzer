# Dependency report

## Summary

| Ecosystem               | Pinning                                                 | Audit result (2026-10-22)                                                                   | Action                                                                            |
| ----------------------- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| npm (runtime 5, dev 12) | Exact versions in `package.json`, plus `pnpm-lock.yaml` | 6 advisories, all in `vite` 8.3.2: 1 high, 4 moderate, 1 low                                | Bump vite (DSA-018), needs approval (Q-deps)                                      |
| Cargo (app + 4 crates)  | Caret ranges, plus committed `Cargo.lock`               | **Not audited.** `cargo-audit` and `cargo-deny` are not installed and nothing was installed | Add cargo-deny to CI (DSA-020) and run it once locally                            |
| GitHub Actions          | All pinned by commit SHA with a version comment         | n/a                                                                                         | Keep. Add the new actions (attest, cargo-deny installer, gitleaks) by SHA as well |
| Toolchains              | `node-version: lts/*`, Rust `stable` (floating)         | n/a                                                                                         | Pin them (DSA-063)                                                                |
| PowerShell              | PSScriptAnalyzer `1.22.0` pinned in CI                  | n/a                                                                                         | –                                                                                 |

## npm audit detail (`corepack pnpm audit --prod`)

All six advisories reach `vite` through `vue-router` (`.>vue-router>vite`, `.>vue-router>unplugin>vite`). Vulnerable range `>=8.3.0 <=8.3.2`; patched in `>=8.3.3`.

| Severity | Advisory (title as reported)                                                                               | Exposure in this project      |
| -------- | ---------------------------------------------------------------------------------------------------------- | ----------------------------- |
| high     | launch-editor: `fs.existsSync()` existence check can …                                                     | Dev server only (`tauri dev`) |
| moderate | launch-editor: `escapeCmdArgs()` omits `%` escaping                                                        | Dev server only               |
| moderate | launch-editor: editor CLI flag misinterpretation                                                           | Dev server only               |
| moderate | Vite: files outside `server.fs.allow` could be served when project paths match system paths on non-Windows | Dev server only; non-Windows  |
| moderate | Vite: first 4 bytes of files outside `server.fs` …                                                         | Dev server only               |
| low      | Vite: cross-origin script execution through Vite custom middleware (GHSA-9jrq-w75r-8gcw)                   | Dev server only               |

The dev server binds `localhost:1430`. The shipped app does not include Vite. The risk is to the developer machine while `tauri dev` runs and a malicious web page is open in another browser. Severity is Medium (Revised; see `01` §7).

**Upgrade plan:** set `"vite": "8.3.x"` (≥ 8.3.3, the latest patch in 8.3) in `devDependencies`. `vue-router` 5.3.1 declares vite as a peer or optional dependency, so check with `corepack pnpm why vite` that a single version resolves. Then run `install`, `audit` (expect 0), `quality`, `test` and `tauri build`.

## Cargo notes

- Direct app dependencies: `tauri 2.11.3`, `tauri-plugin-dialog "2"`, `tauri-plugin-opener "2"`, `serde 1.0.228`, `windows-version 0.1` (Windows only), `tauri-build 2.6.3`.
- Crates: `rayon 1.12`, `windows-sys 0.61`, `notify 8`, `chrono 0.4` (no default features, `clock` only), `tempfile 3.27` (dev).
- `tauri-plugin-dialog = "2"` and `tauri-plugin-opener = "2"` accept any 2.x on `cargo update`. That is fine with the lockfile committed, but `cargo update` in a PR can jump minor versions, so review lock diffs.
- `chrono` is used only for the clock. `transfer_core::format_utc` already formats UTC by hand, so `chrono` may be removable (Enhancement, not registered; check usage with `grep -rn chrono src-tauri/crates`).
- No GPL crate was seen in the direct list. The transitive license check is part of `cargo deny check licenses` (B01).

## Proposed `deny.toml` (B01)

```toml
[graph]
targets = [{ triple = "x86_64-pc-windows-msvc" }]

[advisories]
yanked = "warn"

[licenses]
allow = [
  "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "MIT", "BSD-2-Clause", "BSD-3-Clause",
  "ISC", "Unicode-3.0", "Zlib", "MPL-2.0", "CC0-1.0", "BSL-1.0",
]
confidence-threshold = 0.9

[bans]
multiple-versions = "warn"
wildcards = "deny"
```

Path dependencies (`cleanup-core = { path = … }`) count as wildcards to cargo-deny unless `allow-wildcard-paths = true` is set under `[bans]`. Add that line if the first run complains.

## Unused dependencies

- npm: none found. `@lucide/vue`, `@tauri-apps/plugin-dialog`, `jsdom` (vitest) and `@types/node` (vite config) are all used.
- `lint-staged` is configured but not installed (DSA-058).
- Cargo: `cargo machete` or `cargo udeps` was not run. Optional in B01.
