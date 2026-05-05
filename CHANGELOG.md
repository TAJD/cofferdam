# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


<!-- Is there a script that generates this file? --> 

## [Unreleased]

## [0.2.1] - 2026-05-05

### Added
- Plugin SDK (`@cofferdam/check-sdk`) and Node-side plugin host: declare `plugins = [...]` in `cofferdam.toml`, write checks in TypeScript via `defineCheck`, and the cofferdam binary spawns a Node host that runs them alongside the built-in checks (cd-81a).
- AST surface for plugins: `AstView.findAll(kind)` + `AstView.walk(visitor)` over a v0 frozen 9-kind union (Program, CallExpression, ImportDeclaration, Function, ArrowFunctionExpression, Class, ObjectExpression, MemberExpression, IdentifierReference). Span data on every node round-trips back to source bytes (cd-81a.2, cd-svf).
- AST wire format (option D — flat array + firstChild/nextSibling indices) crossing the Rust→Node host boundary, with byte-accurate spans (cd-svf).
- Three e2e plugin fixtures demonstrating Pattern A (line walk — `brand-casing`), Pattern B (AST findAll — `no-http-client`), and Pattern C (stateful walk — `tenant-isolation`); all three exercised in CI by `plugin-sdk-e2e.yml` (cd-7e4, cd-b5h, cd-11j).
- Plugin host wall-clock timeout (default 60 s, override with `COFFERDAM_PLUGIN_HOST_TIMEOUT_SECS`) so a stuck plugin can't hang cofferdam indefinitely (cd-81a.7).
- Plugin host rejects plugins whose vendored `@cofferdam/check-sdk` major is incompatible with the host, with a named load error (cd-b1q).
- Biome-style suppression syntax (`// cofferdam-ignore: <CheckId>: <reason>`), file-wide and ranged variants, alongside the existing ESLint-style aliases (cd-81a.4).
- `Consistency.BroadSuppression` info-level nudge that flags any `// cofferdam-ignore` directive lacking a check id, so suppression intent stays auditable (cd-81a.4).
- `cofferdam.invariants.toml` — canonical project-wide architectural spec, replacing per-check `[layers]` blocks. Adds `Design.BoundaryFrozen` and `Design.InvariantViolation` checks (cd-9ph).
- `LineView` gains `is_jsx_text` classification flag, `line_start` byte offset, and a `span_for(start, end)` helper for plugin authors (cd-0ne, cd-cgd).

### Fixed
- Binary `--version` and Cargo workspace version now sync with the published git tag at release time (was reporting `0.1.4` inside the `v0.2.0` release because only npm was bumped). The release workflow now runs the same regex-replace on `Cargo.toml` that `publish-npm` already ran on `package.json`.
- `scripts/check-spans.mjs` now accepts an optional `[check-id]` filter so the brand-casing fixture's plugin spans can be verified independently of the project-graph checks (`OrphanExport`, `DeadExport`, …) that now also fire on the same source.
- `scripts/smoke-install.ps1` exits 0 explicitly so transient Windows file-locks during `node_modules` cleanup don't bubble into the smoke-test exit code.

## [0.2.0] - 2026-05-03

### Added
- `cofferdam init` command to scaffold a `cofferdam.toml` configuration file in a project (cd-fnm).
- Five-level severity axis (`hint`, `info`, `warning`, `error`, `critical`) and `--fail-on=<level>` flag to gate CI on a configurable severity threshold (cd-t1a).
- `--since <git-ref>` flag for PR-mode checking — only files changed since the given ref are analyzed (cd-3pn).
- `cofferdam.toml` configuration file support with per-check option wiring (cd-4ms).
- Inline suppression directives (`// cofferdam:disable <Check.Id>`) to silence findings on a line (cd-5t7).
- Baseline workflow: `cofferdam baseline save` / `baseline check` to adopt cofferdam on an existing codebase without breaking CI on pre-existing findings (cd-d31).
- `Refactor.DuplicateBlock` check: flags copy-pasted statement blocks (≥6 statements, AST-canonical) across the whole project via the corpus API (cd-qnu, cd-jdq, cd-s2k, cd-mti).
- `Refactor.CyclomaticComplexity` check: flags functions with cyclomatic complexity above the configured limit (default 10) (cd-4cr).
- `Refactor.CognitiveComplexity` check: flags functions with cognitive complexity above the configured limit (default 15), using Sonar's B2/B3/B4 nesting model (cd-vlq, cd-39c, cd-u30, cd-2pu).
- `Design.DuplicateExportName` check: flags identical export names colliding across multiple barrel files in the same project (cross-file corpus check).
- Per-check options schema in `cofferdam-core` to support configurable check thresholds.
- Plugin-facing AST surface and `LineView` API for future external check authors (cd-81a).
- Default exclusion of `.d.ts`, `.d.cts`, and `.d.mts` declaration files from analysis (cd-ofu).
- Smoke test harness: end-to-end `npm install` verification on Linux, macOS, and Windows after each npm publish (cd-5zn, cd-61j).
- `--format=compact` pipe-delimited output for shell-script consumers (cd-ab2).
- Built-in check catalog at `docs/checks.md` (cd-m3g).
- Drop-in CI integration recipes at `docs/ci.md` (cd-nde).
- `Refactor.UnusedVariable` check: flags declared variables that are never read (cd-ydw).
- `Refactor.PreferOptionalChain` check: suggests replacing `&&`-guarded property access chains with optional chaining (`?.`) (cd-moj).
- `Refactor.PreferNullishCoalescing` check: suggests replacing `|| fallback` patterns on nullable values with the nullish coalescing operator (`??`) (cd-moj).

### Fixed
- Path separators normalized to forward slashes in all output formatters, fixing Windows-specific display issues (cd-ose).

### Changed
- GitHub Actions runners updated to current major versions; Node 24 forced for all JS actions ahead of the GitHub enforcement date.
- macOS CI switched from `macos-14` (deprecating) to `macos-15` for both Apple Silicon and Intel cross-compilation targets.

## [0.1.4] - 2026-05-02

### Fixed
- npm publish step now pins `npm@11` to avoid a self-upgrade bug (`MODULE_NOT_FOUND: promise-retry`) that corrupted the global npm install mid-release.

## [0.1.3] - 2026-05-02

### Changed
- npm publishing switched to OIDC Trusted Publisher authentication; no more long-lived npm access tokens required in repository secrets.
- GitHub Actions updated to current major versions (`actions/checkout@v6`, `actions/setup-node@v6`, etc.).

## [0.1.2] - 2026-05-02

### Changed
- macOS release builds moved to `macos-15` (Apple Silicon) runner using `cargo-zigbuild` for cross-compilation, replacing the scarce and slow `macos-13` (Intel) runner.
- `repository.url` in `package.json` corrected to use `git+https://` scheme.

## [0.1.1] - 2026-05-02

### Added
- Initial release of `cofferdam` on npm.
- Core analysis engine with five check categories: `Consistency`, `Design`, `Readability`, `Refactor`, `Warning`.
- Built-in checks: `Readability.MaxLineLength`, `Readability.MaxFunctionLength`, `Design.MaxParameters`, `Warning.TripleEquals`.
- JSON and text output formatters.
- Multi-platform binaries published to GitHub Releases (Linux x64/arm64 gnu+musl, macOS x64/arm64, Windows x64).
- Postinstall binary download for the npm package.
