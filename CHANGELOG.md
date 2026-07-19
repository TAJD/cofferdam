# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


<!-- Is there a script that generates this file? --> 

## [Unreleased]

### Fixed
- `cofferdam-ignore` suppression comments work again for plugin-check findings when the project is analyzed with a relative discovery path (e.g. `cofferdam check .`, the common case). CD-99 (0.3.8) made the file path sent to the plugin host absolute so a plugin's echoed `report.file` could be matched back to the analyzed file set, but the resulting `Issue.file` was then stamped with that absolutized path instead of the original discovery path — an absolute path never equals the relative path the suppression map is keyed by, so every plugin-check suppression comment silently stopped applying. Built-in checks were unaffected (CD-140).

## [0.3.10] - 2026-07-19

### Changed
- `Engine::analyze_incremental` re-analyzes single-file edits faster (CD-40, levers 1-3 of the CD-28/CD-32 tail): a `SourceFile` cache in `AnalysisState` avoids rebuilding one for every known file on each call's consistency sweep, one-time engine-config setup no longer re-runs every call, and the canonical import/export graph is now maintained incrementally instead of rebuilt from scratch each call. Measured ~13-17% faster on a real-world corpus (4.05x -> 4.75x vs. a cold full run); CD-40 stays open for the remaining, higher-effort levers needed to reach the ticket's >=10x target.

## [0.3.9] - 2026-07-19

### Added
- `VariableDeclaration` AST node kind in the check-sdk plugin surface — plugins can now `findAll("VariableDeclaration")` to see `const`/`let`/`var`-bound identifiers and their init expressions, not just `function` declarations. New example plugin `examples-plugins/no-banned-const` demonstrates flagging a `const`-bound banned identifier (CD-78).
- `.md`/`.mdx` files can now reach plugin checks: opt in with `[engine] extra_extensions = ["md", "mdx"]` in `cofferdam.toml` to widen discovery beyond the default extension list. Markdown gets no whole-file parse (`file.ast` is `null`, mirroring Astro), but `file.text` and line-scan `LineView`s are populated for Pattern-A regex/line-based plugin checks. No built-in check targets Markdown (CD-68).

### Fixed
- A `cofferdam-ignore` comment placed at the *related* (non-primary) occurrence of a paired `Refactor.DuplicateBlock` finding now suppresses it, matching a comment at the primary occurrence — previously only the primary side had any effect (CD-77).

### Fixed
- `packages/check-sdk/tests/ast-view.test.mjs` was still driving `plugin-host.mjs` with the old one-shot manifest protocol and had silently bit-rotted after the CD-33 streaming NDJSON migration; re-pointed at the header/file/end wire format used by the rest of the test suite (CD-106).

### Docs
- The plugin SDK guide's `package.json` example now includes the required `"type": "module"` field; the plugin host also recognizes the classic Node "missing type: module" error shapes and appends a hint pointing at the fix (CD-72).

### CI
- `plugin-sdk-e2e.yml` now runs `node --test tests/*.test.mjs` for `@cofferdam/check-sdk` after building the package, so a broken unit test (like CD-106) fails CI instead of going unnoticed — previously only `tsc` typechecking ran (CD-106).

## [0.3.8] - 2026-07-11

### Added
- Per-check badges (`graph` / `file` / `type-aware` / `advisable`) in the check catalog (`docs/checks/index.md`, `public/checks.json`), computed by `cofferdam gen-docs` from `CheckMeta` plus a hand-maintained graph-check list (CD-54).
- `[budgets]` enforcement plus `cofferdam baseline prune | ratchet` and `cofferdam check --trend` (CD-64). A `[budgets]` table in `cofferdam.toml` caps finding counts per check id or bare category name as a hard gate — independent of `--fail-on` severity and baseline status (the count includes baselined findings) — and fails the build (exit 1) when exceeded. `baseline ratchet` lowers each budget to the current count (never raises it); `baseline prune` drops baseline entries whose finding no longer exists; `check --trend` appends per-category counts to a trend file over time. New doc [`docs/budgets.md`](docs/budgets.md).
- `cofferdam advise --analyze` reports current-vs-limit headroom per file for the budgeted checks (cognitive/cyclomatic complexity, parameter count, function length, line width), reusing each check's real visitor so `current` can't drift from what `check` would flag. `advise` output is now a versioned envelope (`schema_version` + `files`) shared by the CLI and MCP server; schema at [`docs/public/schemas/advise-v1.json`](docs/public/schemas/advise-v1.json).
- `cofferdam advise` now projects invariant, boundary-frozen, and public-API context (`Design.InvariantViolation`, `Design.BoundaryFrozen`, public-API orphan exemption) alongside the existing layer and complexity advice.
- `cofferdam agents --hooks` emits a ready-to-paste PreToolUse hook recipe that runs `advise` before an agent edits a file. New doc [`docs/hooks.md`](docs/hooks.md).
- `cofferdam-mcp` expanded from one tool to five — `cofferdam.check`, `cofferdam.advise`, `cofferdam.advise_diff`, `cofferdam.explain`, `cofferdam.invariants` — over the same projection functions the CLI uses, so MCP and CLI output stay byte-for-byte identical. Updated doc [`docs/mcp.md`](docs/mcp.md).
- `cofferdam doctor` now detects Biome/ESLint coexistence and prints migration advice (warn, not fail).
- SEO-grade checking (CD-79): JSX AST checks for image alt text and duplicate/missing titles, a tree-sitter-html adapter so `.html`/`.htm` files are analysed alongside TypeScript (now in `DEFAULT_EXTENSIONS`), and `cofferdam verify --dist` to run output-mode-eligible checks against a build's rendered HTML directory. Plugin authors opt a check into `verify --dist` eligibility via `outputMode: true` in `defineCheck()`.

### Fixed
- `.astro` components imported only from a page's frontmatter fence are no longer flagged as `Design.OrphanExport` / `Refactor.DeadExport` false positives (CD-45). The engine now recognises `Language::Astro`, widens default discovery to `.astro`, and extracts the frontmatter fence's imports into the project graph — counting them as used, since template-body usage is invisible to a frontmatter-only parse. Frontmatter exports (e.g. `export const prerender`) are discarded as non-graph exports.
- `[budgets]` keys that match no known check id or category now emit a warning instead of silently resolving to a count of 0 — a typo (e.g. `Refactor.CognitiveComplexty`) previously defeated the gate it was meant to be, and `baseline ratchet` would lock the dead key in at 0.
- Docs site (repo-only, no effect on the published package): pinned transitive `vite`/`esbuild` devDependencies to `6.4.3`/`^0.25.0` via a `pnpm.overrides` entry in `docs/package.json`, clearing 4 open Dependabot alerts (1 high, 3 moderate) — all dev-server-only issues in versions vitepress 1.6.4 had pulled in transitively. `vite@8` was tried first but broke the VitePress build (it dropped esbuild as its transform in favor of Rolldown); `6.4.3` is the newest release still API-compatible with vitepress 1.6.4 and is itself past every patched floor.
- CD-79 SEO-grade checking follow-ups, from real-repo validation against three downstream projects (CD-88 through CD-100):
  - Plugin-check findings' `file` path is now consistently absolute rather than sometimes relative to the check target — matches the documented wire contract; JSON/SARIF consumers keying on `file` should note the change (CD-99).
  - `verify --dist` now scans a gitignored `--dist` root in full while still skipping genuinely nested ignored subdirectories inside it, instead of sweeping every nested ignore (CD-95).
  - `--only <id>` accepts a plugin check's bare metadata id, not just its runtime category-prefixed id, and hard-errors on a genuine typo instead of silently matching nothing (CD-96).
  - Per-glob `disabled = true` overrides now also suppress findings from cross-file checks emitted in `finalize()` (`Design.OrphanExport`, `Refactor.DeadExport`, `Design.ImportCycle`, etc.), not just per-file `run()` output (CD-97).
  - The plugin host now awaits an `async finalize()`, so a rejection surfaces as `Warning.PluginCrashed` and a `ctx.report()` call after an `await` inside it is no longer lost (CD-91).
  - `LineView` gains span-aware `stringLiteralRanges` alongside the existing whole-line `isStringLiteral` flag, so a check can tell where on a line a string/template literal actually sits (CD-100).
  - `defineCheck()` no longer silently drops plugin-supplied fields outside its hardcoded copy list (CD-94).
  - The materialised plugin-host/type-host Node scripts now live in a version- and user-scoped temp directory instead of one shared, unversioned path, fixing a cross-version race on a machine running two cofferdam versions concurrently (CD-89).
  - `resolveTsMorph`'s ancestor-directory walk no longer stalls on a broken `ts-morph` install partway up the tree — it now correctly continues climbing to find a working install further up (CD-92).
- File-level `.gitignore` / `.cofferdamignore` patterns targeting a cofferdam-managed extension (`.ts`, `.tsx`, `.rs`, `.astro`, `.html`, etc.) were silently ignored — the extension whitelist used for discovery was implemented as an `ignore`-crate `Override`, which takes precedence over ignore-file exclusion by design, so any file matching an allowed extension bypassed `.gitignore`/`.cofferdamignore` entirely. Directory-level rules (`dist/`, `node_modules/`) were unaffected. Discovery now applies ignore rules first and filters by extension afterward (CD-103).

### Changed
- Docs site repositioned around the two loops (advisory + debt) and cofferdam's place in the toolchain (CD-47): new homepage hero, "where cofferdam sits" / "the two loops" sections with Mermaid diagrams, an `advise` demo replacing the `Warning.TripleEquals` demo, a Biome/ESLint coexistence section, and a "why the name" footer. `docs/reference/advise.md` now leads with the JSON envelope and schema link and adds a "what agents should branch on" field guide. `docs/agents.md` and `docs/hooks.md` gained the two-loops diagram and hooks-wiring diagram; `docs/invariants.md` gained a 10-line starter spec and an annotated invariants-anatomy diagram; `docs/budgets.md` and `docs/output-formats.md` gained signature-stability, would_fire/would_clear set-difference, ratchet, and priority×severity diagrams. README mirrors the new hero and stack diagram with a single `advise` demo.

## [0.3.7] - 2026-07-04

### Fixed
- Plugin-check findings no longer carry a `docs_url` (JSON) or `helpUri` (SARIF) that points at a non-existent hosted catalog page. Only built-in checks with a registered `CheckMeta` receive the URL; plugin checks omit the field entirely (cd-fbl7).
- CI (repo only, no effect on installed binaries): the MSRV job installed a nonexistent Rust toolchain after a Dependabot bump rewrote `dtolnay/rust-toolchain@1.93` to `@1.100` — that action's `@<ref>` selects the *toolchain*, not the action version — so it was pinned back to the declared MSRV and Dependabot told to leave it alone. Separately, the `plugin_findings` integration tests are now serialized so several Node plugin hosts no longer start at once on a constrained runner and intermittently return zero findings (the tests, not the product, were flaky).

### Added
- `cofferdam check --time-checks` (CD-34) prints a per-check wall-clock timing breakdown to stderr after analysis, so you can see which checks dominate a slow run without reaching for a profiler. Off by default; no effect on findings or machine-readable output.
- `cofferdam agents` subcommand (cd-l4se). Prints a version-pinned markdown prompt that tells an AI coding agent how to use cofferdam in the current project: when to reach for `cofferdam advise`, how to pre-flight changes with `advise --diff`, how to consume machine-readable findings via `check --robot`, what `cofferdam.invariants.toml` governs, and where to report misbehaviour. Pipe the output into `AGENTS.md` / `CLAUDE.md` to keep agent context files in sync with the installed version. New doc [`docs/agents.md`](docs/agents.md).
- Docs site pages `install.md` (binary overrides, air-gapped installs, build-from-source — moved out of MAINTAINERS) and `languages.md` (TypeScript surface, Rust adapter, future-adapter shape — moved out of README), both in the Getting started sidebar (cd-9rxx).

### Changed
- `llms.txt` (served at `https://tajd.github.io/cofferdam/llms.txt`, generated by `cofferdam gen-docs`) rewritten as a full LLM-agent entrypoint (cd-54sh): corrected the install command to `@cofferdam/cofferdam` (it still advertised the package name deprecated in 0.2.3), stamped with the current version, full subcommand list including the agent-facing `advise` / `advise --diff` / `watch` / `lsp`, links to every concept doc page, and a 4-step agent workflow section. `scripts/version.mjs check` now audits the llms.txt version line as location #7, so a stale llms.txt fails the release gate like every other version location.
- README slimmed (cd-9rxx): CI section reduced to the `npx` one-liner + ci-recipes link; Languages section reduced to a summary + link. MAINTAINERS corrected (build floor is Rust 1.93, not 1.78; phased-build statuses updated) and its duplicated project-structure block replaced with a link to CLAUDE.md's canonical map.
- The Node type host and plugin host now run as a persistent worker pool fed a streamed manifest rather than a fresh process per batch (CD-31, CD-33), cutting per-run Node startup cost on type-aware and plugin-heavy projects.

### Performance
The CD-28 "scalable engine" epic — parallel analysis on cold runs, incremental replay on edits.
- Per-file analysis now runs in parallel across CPU cores (CD-30), and pass 2 reuses the parses from pass 1 instead of re-parsing every file (CD-29). Cold whole-project runs scale with core count where they were previously single-threaded.
- `cofferdam watch` re-analyzes incrementally (CD-32): a single-file edit re-runs pass 1 for only the changed file and replays cross-file state (corpus, project graph, suppressions) for the rest instead of rebuilding it from scratch. Measured ~6.9× faster per edit than a cold full run on a 325-file project, and byte-identical to a from-scratch analysis (an integration gate holds this on every edit/add/remove). Backed by new per-file corpus provenance and a `cofferdam-graph` per-file removal API (CD-36).
- Regression guards (repo CI only, no effect on installed binaries): a criterion bench suite times the cold full run and the single-edit incremental path, and a peak-RSS memory gate trips CI on a >20% resident-memory regression (CD-35).

## [0.3.6] - 2026-06-09

### Added
- `--fail-on-type-unavailable` flag for `cofferdam check` (cd-260l). By default, when a type-aware check is registered but the ts-morph type host cannot start (no Node, no ts-morph, no tsconfig), cofferdam prints one warning and silently skips type-aware checks — CI that relies on type coverage gets false negatives without noticing. With the flag set, that condition exits with code 2 and a clear diagnostic instead. Default off; no effect when nothing declares `requires_types` or when `[engine] type_aware = false`. Documented in [`docs/type-aware-checks.md`](docs/type-aware-checks.md#enforcing-type-coverage-in-ci).
- `limit` option for `Refactor.CyclomaticComplexity` and `Refactor.CognitiveComplexity` (cd-yrvl / [gh #50](https://github.com/TAJD/cofferdam/issues/50)). Both checks claimed configurability but shipped `options: &[]`, so the config validator rejected `[checks."Refactor.CyclomaticComplexity"] limit = N`. The thresholds (defaults 10 and 15, unchanged) are now real options.
- Supply-chain and toolchain CI gates: `cargo-deny` runs on every push/PR plus a weekly cron against the policy in `deny.toml` (cd-othw), and an MSRV job compiles the workspace on the declared minimum Rust version (cd-4kfk). Repo CI only — no effect on installed binaries.

### Changed
- Declared Rust toolchain floor corrected to 1.93 and now enforced in CI (cd-4kfk). The previous `rust-version = "1.79"` was fiction — transitive dependencies (oxc_syntax, smol_str, edition-2024 crates) already required 1.93. Only affects building from source; npm/prebuilt-binary users are untouched.
- Plugin host reports that reference files outside the analyzed set are now rejected (cd-neav). Per-file `run()` reports were never affected (the host stamps their path), but `finalize()` reports carry a plugin-supplied `file` verbatim — a buggy plugin could inject findings for paths cofferdam never analyzed, which then flowed into baselines and formatter output. Out-of-scope reports (and out-of-scope `related` entries, which are dropped individually while the finding survives) now surface as one aggregated `Warning.PluginHostFailed` per plugin check id naming the dropped paths. Plugin authors: `finalize` must report against files that were part of the run — see the [finalize contract](docs/plugin-sdk-guide.md#finalizectx-opts).

### Fixed
- `--since` no longer narrows the *analysis* file set, only the *reported* one (cd-aksx / [gh #53](https://github.com/TAJD/cofferdam/issues/53)). Cross-file checks (`Design.OrphanExport`, `Refactor.DeadExport`, import cycles) previously built a partial project graph from just the changed files, so consumers living in unchanged files were invisible and exports were falsely flagged orphaned/dead. The engine now always analyzes the full discovered set; `--since` filters findings down to changed files after analysis.
- The embedded Node host scripts (plugin host, type host) are now materialized to version-stamped temp filenames (`cofferdam-plugin-host-<version>.mjs`) so concurrent or interleaved runs of different cofferdam versions can never execute each other's host script (cd-8bmz).
- The plugin-host and type-host shutdown paths always attempt to kill the child Node process when the wait loop errors or times out; previously an early-return on a `try_wait` error could leak an orphaned Node process (cd-pj1t).

### Performance
- Glob patterns in the invariants DSL evaluator are compiled once per analysis pass instead of once per file × pattern (cd-3kxc). Layer membership (`compute_layer`), import-specifier matching, and `matches` predicates previously rebuilt their `GlobSet` on every evaluation — O(files × patterns) compilations on real repos with `[layers]` configured.

## [0.3.5] - 2026-05-23

### Added
- Per-path check overrides via `[[overrides]]` in `cofferdam.toml` (cd-m5tu / [gh #46](https://github.com/TAJD/cofferdam/issues/46)). Each block pairs a `paths` glob array with `[overrides.checks."Category.Name"]` sub-tables that retune a single check — any option (e.g. `limit`), `severity`, or `disabled = true` — for matching files, while **every other check keeps running** on them. Globs are project-root-relative (matched with the same engine as `[public_api].exports`); blocks cascade in declaration order with the last matching block winning per key, like ESLint's `overrides`. The motivating case: relax `Readability.MaxFunctionLength` (or disable it) on `**/*.test.tsx` without excluding test files from analysis or hand-annotating each one. Option values are validated against each check's schema at load time, identical to a global `[checks."X"]` block. Files with no matching override take a zero-cost fast path; a file whose options an override changed bypasses the per-file findings cache (the cache key can't capture per-file option deltas). New doc [`docs/overrides.md`](docs/overrides.md).
- `[engine] type_aware` opt-out in `cofferdam.toml` (cd-9hp.2.4). Setting `type_aware = false` force-disables type-aware checks (those declaring `requires_types`, today `Warning.UnusedNullCheck`) even when one is registered and a `tsconfig.json` + `ts-morph` install are present — the type host is never spawned and the checks are skipped silently. This is the escape hatch for CI runners with no Node runtime, which would otherwise see a "type host unavailable" warning. The default (key omitted, or `true`) leaves type-aware checks enabled; cofferdam still auto-opts-out when no `requires_types` check is registered, so the worker cost is never paid unless something needs it. New concept doc `docs/type-aware-checks.md` covers the requirements (tsconfig + ts-morph + Node), both opt-out paths, and the cost model.
- CI smoke test for the ts-morph type host (cd-9hp.2.4): the `type-host-smoke` workflow installs `ts-morph` into the committed fixture project `examples-type-host/unused-null` and asserts the worker pool resolves real TypeScript types end-to-end — flagging every redundant null guard including a **cross-file** case (operand type from an imported interface), proving project-wide resolution — that `[engine] type_aware = false` yields zero findings, and that project-init cold-start stays under a generous ceiling. Kept separate from the always-on `ci.yml` (which stays Node-free and fast), mirroring `plugin-sdk-e2e.yml`.

### Fixed
- `Consistency.UnusedSuppression` no longer falsely flags a live `cofferdam-ignore-file` directive as stale when the suppressed finding comes from a `pure_run` check on a file with a byte-identical sibling (cd-mwr6 / [gh #47](https://github.com/TAJD/cofferdam/issues/47)). Root cause: the per-file findings cache is keyed on `(content, config, check)` with no path, so a cached finding could surface stamped with another identical file's path. That mis-stamped finding landed in the `ALL_PRE_FILTER_FINDINGS` snapshot under the wrong file, so `UnusedSuppression` — which reads that snapshot — saw "no findings here" for the real file and reported the directive as covering nothing. `FindingsCache::get_for_path` now re-stamps `Issue.file`, `Issue.location.uri`, and same-file related spans onto the consuming file before returning, so the snapshot is always correct. A regression test exercises the exact gh #47 scenario (two identical files with a file-scoped suppression, the second served from the cache) and asserts no spurious `UnusedSuppression`.
- `Readability.MaxLineLength` now measures terminal display columns (via `unicode-width`) instead of raw UTF-8 bytes (cd-c8aq). Box-drawing characters, em dashes, and accented prose were over-counted ~3× and falsely flagged; wide CJK now counts as 2 columns, zero-width marks as 0. Span byte offsets stay byte-based; only the limit comparison, the reported width, and the column are in display columns. The message now reads "N columns".

## [0.3.4] - 2026-05-21

### Added
- Type-aware check infrastructure — groundwork only, no user-facing check yet (cd-9hp.2 cp1 + cp2). A Node-side ts-morph "type host" exposes TypeScript's type system to checks that declare `requires_types`, over a stdin/stdout JSON-RPC channel (`design/type-host-wire.md`). The engine routes such checks through a `TypeOracle`; `cofferdam-core` stays Node-free. A hidden `cofferdam type-host --ping` subcommand measures worker cold-start. **No built-in check sets `requires_types` yet**, so this ships entirely dormant — `cofferdam check` behaviour, dependencies, and output are unchanged (no Node or ts-morph required). The first real type-aware check (`Warning.UnusedNullCheck`) lands in a later release (cd-9hp.2.3).
- `scripts/version.mjs` — deterministic version manager. `check [X.Y.Z]` asserts every in-repo version location agrees (and, with an argument, matches a tag); `set X.Y.Z [--regen]` rewrites them in lockstep with no `cargo-edit` dependency. The release workflow now runs `version.mjs check <tag>` as a hard gate before building, so a forgotten bump fails the release loudly instead of shipping silently (see Fixed).

### Fixed
- In-repo version realigned with the published release. v0.3.3 shipped to npm under the `v0.3.3` tag without bumping a single in-repo version file — `Cargo.toml`, the internal path-dep pins, both `package.json` files, and `docs/public/checks.json` all stayed at 0.3.2, so `cofferdam --version` reported 0.3.2 while `npm install` served 0.3.3. The cause: `release.yml` derives the published version from the tag and self-heals at build time, so nothing failed when the human-side bump was skipped. 0.3.4 moves every location forward past the published 0.3.3, and the new `verify` job in `release.yml` (backed by `scripts/version.mjs`) makes the tag-vs-repo mismatch a release-blocking error going forward. This also caught a previously-untracked location: `cofferdam-cli` pins `cofferdam-lsp` directly (not via `[workspace.dependencies]`), so it was missed by the documented "six places" and had drifted too.

## [0.3.3] - 2026-05-20

### Added
- Plugin corpus access + cross-file `finalize` hook (cd-9hp.6). Plugins can now share state across files via `ctx.corpus.read/write/append(key, value)` and emit cross-file findings from an optional `finalize(ctx, opts)` callback declared on `defineCheck(...)`. Slots are plugin-private — the host namespaces every key by the calling `check.id`, so two plugins picking the same naive name cannot see each other's data. `FinalizeContext.report` requires an explicit `file` since finalize has no implicit "current file"; `related` carries the secondary locations. The plugin host (`plugin-host.mjs`) propagates `related` from plugin reports through to engine `Issue.related` for cross-file rendering. New example fixture: [`examples-plugins/duplicate-class/`](examples-plugins/duplicate-class) — a cross-file class-name duplicate detector demonstrating the end-to-end pattern. Documented in [`docs/plugin-sdk-guide.md`](docs/plugin-sdk-guide.md#cross-file-plugin-checks-corpus--finalize-cd-9hp6).
- `schema_version` field for `cofferdam.invariants.toml` (cd-9hp.12). Spec files now declare their schema version as `schema_version = "1.0"` (or integer `1`) at the top of the file. Versions newer than this build understands are rejected with an upgrade message; versions older than the supported window are rejected with a migration message; missing fields are loaded as `1.0` with a one-time hint. The full versioning policy — MAJOR.MINOR semantics, deprecation window, bump rules — is documented in [docs/schema-versioning.md](docs/schema-versioning.md). Existing spec fixtures updated to declare `1.0` explicitly. The canonical-graph and predicate-DSL schemas will inherit this same policy (cd-T1, cd-9hp.1).
- Fallible + namespaced corpus API for plugin authors (cd-9hp.7). `CorpusIndex` gains `try_with_slot` (returns `CorpusError::TypeMismatch` instead of panicking on a type-id collision) and `try_with_namespaced_slot(check_id, key, ...)` (auto-prefixes the slot name with the calling check's id so two plugins picking the same naive name see independent slots). Built-in checks keep `with_slot` — logic-error panics still surface loudly at code review. `CorpusError` joins the `cofferdam_core` public surface for plugin hosts to render. Documented in [`docs/plugin-sdk-guide.md`](docs/plugin-sdk-guide.md#cross-file-plugin-checks-corpus--finalize-cd-9hp6) — same section as cd-9hp.6, which exposes this runtime to plugin authors via `ctx.corpus`.

### Fixed
- `[public_api].exports` no longer silently misses exact-path entries when the spec is discovered from a relative root (cd-gro / [gh #41](https://github.com/TAJD/cofferdam/issues/41)). `Engine.analyze_with_sources` absolutizes every source path via `std::path::absolute` (cd-q9f), so the `file_key` passed to `Design.OrphanExport`'s `PublicApi::is_match` is always absolute. `resolve_public_api` now applies the same absolutize to `project_root` before joining each entry — without this, the stored `exact` HashSet keys stayed relative (`./apps/web/src/app.tsx`) and missed the absolute file_key (`c:/users/…/apps/web/src/app.tsx`). The glob path's root-prefix stripping gets the same treatment. Three regression tests pin a relative project_root + absolute file_key through both exact and glob branches.
- Inline `// cofferdam-ignore <CheckId> — reason` (space-separator, em-dash / ASCII hyphen / colon reason) now binds the check id and suppresses the finding (cd-b77 / [gh #42](https://github.com/TAJD/cofferdam/issues/42)). Previously only the canonical `// cofferdam-ignore: <CheckId>: <reason>` (colon-separator) form was recognised — the natural-language space form fell through the parser unbound, AND the directive line was incorrectly flagged `Consistency.BroadSuppression`. Both halves now consult the same `looks_like_check_id` heuristic (`Category.Name`-shaped first token): the engine's suppression parser binds the id, `Consistency.BroadSuppression` stays silent on the directive, and `Consistency.UnusedSuppression` sees the same scoped view of the file. Prose comments like `// cofferdam-ignore please understand this` still flag broad because "please" isn't check-id-shaped. The `Consistency.BroadSuppression` explanation and inline message now show both accepted forms so the canonical syntax is visible at the diagnostic site.

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
