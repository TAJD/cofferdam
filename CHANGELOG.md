# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


<!-- Is there a script that generates this file? --> 

## [Unreleased]

### Added
- Plugin corpus access + cross-file `finalize` hook (cd-9hp.6). Plugins can now share state across files via `ctx.corpus.read/write/append(key, value)` and emit cross-file findings from an optional `finalize(ctx, opts)` callback declared on `defineCheck(...)`. Slots are plugin-private — the host namespaces every key by the calling `check.id`, so two plugins picking the same naive name cannot see each other's data. `FinalizeContext.report` requires an explicit `file` since finalize has no implicit "current file"; `related` carries the secondary locations. The plugin host (`plugin-host.mjs`) propagates `related` from plugin reports through to engine `Issue.related` for cross-file rendering. New example fixture: [`examples-plugins/duplicate-class/`](examples-plugins/duplicate-class) — a cross-file class-name duplicate detector demonstrating the end-to-end pattern. Documented in [`docs/plugin-sdk-guide.md`](docs/plugin-sdk-guide.md#cross-file-plugin-checks-corpus--finalize-cd-9hp6).
- `schema_version` field for `cofferdam.invariants.toml` (cd-9hp.12). Spec files now declare their schema version as `schema_version = "1.0"` (or integer `1`) at the top of the file. Versions newer than this build understands are rejected with an upgrade message; versions older than the supported window are rejected with a migration message; missing fields are loaded as `1.0` with a one-time hint. The full versioning policy — MAJOR.MINOR semantics, deprecation window, bump rules — is documented in [docs/schema-versioning.md](docs/schema-versioning.md). Existing spec fixtures updated to declare `1.0` explicitly. The canonical-graph and predicate-DSL schemas will inherit this same policy (cd-T1, cd-9hp.1).

### Schema changes
- `cofferdam.invariants.toml`: added optional `schema_version` field at the top level. Missing field is backward-compatible; current build is at `1.0`. No semantic change to any existing field. See [docs/schema-versioning.md](docs/schema-versioning.md).

## [0.3.2] - 2026-05-07

### Fixed
- Plugin findings now flow through the full `cofferdam check` pipeline (cd-1c7 / gh #31). Previously visible only via `cofferdam check --no-baseline --format=json`, plugin findings were silently dropped from default text output, never recorded in `.cofferdam/baseline.json`, and ignored by `--fail-on` — making it impossible to enforce custom architectural rules in CI. Plugin findings now render under the per-category headings (or a new `Other` heading when their check id prefix is not one of the five built-in categories), participate in baseline writes / diffs, and trigger the gate at their declared severity. Inline `// cofferdam-ignore` directives also apply to plugin check ids.

## [0.3.1] - 2026-05-06

### Added
- Glob support in `[public_api].exports` (cd-no7 / partial gh #27). Entries containing `*`, `?`, `[`, or `{` compile into a `globset::GlobSet`; exact paths keep their existing semantics. A single `"components/ui/**/*.tsx"` now exempts every shadcn primitive without listing each file. The deeper "follow re-export edges in `Design.OrphanExport` reachability" fix is tracked separately as cd-klp.
- `Warning.NoConsoleLog` gains a `methods` option (default `["log"]`). Set `[checks."Warning.NoConsoleLog"] methods = ["log", "warn", "error"]` to opt back into the broad scope.

### Changed
- `Warning.NoConsoleLog` default scope narrowed to `console.log` only (cd-xin / gh #30). Previously fired on every `console.X` call regardless of the rule id; the broad scope was a high-false-positive trap on real codebases (legitimate `console.error` in catch blocks, recoverable `console.warn`). Rule id unchanged — existing baselines and suppressions keep working. Use the new `methods` option to restore prior behavior.

### Fixed
- `Warning.TripleEquals` no longer fires on idiomatic `x == null` / `x != null` (cd-w9i / gh #28). All four operand shapes (`x == null`, `x != null`, `null == x`, `null != x`) are exempt — matches ESLint's `eqeqeq: "smart"`. Bare `==` / `!=` against any non-null operand still flagged.
- `Consistency.UnusedSuppression` no longer flags active directives that suppress findings emitted by finalize-stage checks (cd-wqc / gh #29). Previously, directives targeting `Warning.UnusedImport`, `Design.OrphanExport`, `Design.DeadExport`, or any other check that emits from `finalize()` were misreported as stale because the pre-filter findings snapshot was taken before finalize ran. The engine now runs finalize in two phases: non-observers first, snapshot rebuild, then observers (`Consistency.UnusedSuppression`). Internal: `CheckMeta` gains an `observes_findings: bool` field; only `Consistency.UnusedSuppression` sets it.

## [0.3.0] - 2026-05-06

### Added
- `cofferdam advise --diff <git-ref>` — pre-flight validation for proposed changes (cd-ugh). Runs the full check pipeline against `git show <ref>:<path>` source AND the working tree, then reports `would_fire` (rules introduced by the change) and `would_clear` (rules cleared by the change). Findings are keyed by `(file, check_id, rule_signature)` reusing the baseline subsystem's SHA-256-of-trimmed-span scheme, so reformats and line shifts don't show up as spurious entries. `--fail-on=<level>` gates only on `would_fire`. Output is JSON regardless of `--format`.
- `cofferdam advise --diff` includes plugin findings in `would_fire` / `would_clear` (cd-s7f). The plugin host runs on both the materialised pre-diff source and the working-tree post-diff source; plugin issues merge into the same set-diff as engine issues.

### Changed
- `cofferdam_engine::Engine::analyze_with_text` now delegates to a new `analyze_with_sources(sources)` entry that takes pre-loaded `(PathBuf, String)` pairs without touching the filesystem. Public API is additive — existing callers are unaffected.
- `cofferdam-cli`'s plugin host gains a sibling `run_plugins_with_sources` that mirrors `run_plugins` but accepts pre-loaded sources. Internal API; the disk-read entry remains the default.

### Fixed
- `scripts/smoke-install.{sh,ps1}` now look for the binary under `node_modules/@cofferdam/cofferdam/bin/` (the scoped path post-0.2.3 rename). Both scripts asserted the unscoped path, failing CI on every push.
- `docs/plugin-sdk-guide.md` — reference to `@cofferdam/check-sdk` README now points at the npmjs.com listing instead of the in-repo path. VitePress can't resolve files outside the docs tree, so the dead-link checker was failing the build even though the file exists.

## [0.2.3] - 2026-05-06

### Changed
- npm package renamed from `cofferdam` to `@cofferdam/cofferdam`. Existing `cofferdam` package is deprecated with a redirect; install with `npm install -D @cofferdam/cofferdam` going forward.

### Fixed
- Layer resolution now picks the most-specific layer when multiple `[layers]` globs match (cd-31r / gh #5). Honors `!negation` patterns within a single layer's glob list. Configs that previously relied on alphabetical layer-name ordering for overlapping globs may see different layer assignments — use a `!` exclude or rely on prefix specificity instead.

## [0.2.2] - 2026-05-05

### Added
- `@cofferdam/check-sdk` is now published on npm. Released in lockstep with the `cofferdam` binary — `@cofferdam/check-sdk@X.Y.Z` is built and tested against `cofferdam@X.Y.Z`. Pre-1.0, pin both packages to the same exact version.
- `packages/check-sdk/README.md` ships as the npmjs.com listing — install snippet, three-pattern overview, full API surface index, versioning policy.

### Changed
- Release pipeline gains a `publish-check-sdk` job alongside `publish-npm`. Both fire on the same `v*` tag, both authenticate via OIDC Trusted Publisher.

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
