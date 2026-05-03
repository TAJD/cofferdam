# Post-v1 roadmap — improvements after the cd-g4y arc closes

> **Context.** Drafted 2026-05-03 during the session that filed the cd-g4y "Adoption-unlock" epic (12 beads getting cofferdam from "tool that runs" to "npm install + include in build step works"). This document captures the *next* ring of work — improvements that aren't gating for v1 adoption but compound the value of the tool once the install-and-build-step story is real.
>
> **Not yet filed as beads.** Convert the highest-value items into a sibling epic (proposed: `cd-? "Post-v1 next ring"`) once cd-g4y is materially underway. Filing them now would muddy the cd-g4y closure criteria.
>
> **Code anchors throughout.** Every bucket lists the exact files where the work lands so future sessions don't have to re-derive the surface area.

## Recommended attack order

If picking the next 3 buckets after cd-g4y closes, do them in this order:

1. **Bucket 3 — check pack expansion.** Without more checks, cofferdam is a five-rule linter and people see that immediately. NoConsoleLog + NoDebugger + NoEval + QuoteStyle would double the value with maybe 2 days of work.
2. **Bucket 1 — adoption ergonomics inside the tool.** `--explain`, `--watch`, one autofix. Makes cofferdam *feel* good vs merely work.
3. **Bucket 4 — community hygiene.** Costs nothing, unblocks any external contribution.

Buckets 2, 5, 6, 7 are next-quarter, not next-sprint. They're documented here so the option-space stays visible.

---

## Bucket 1 — Adoption ergonomics inside the tool

The "ESLint has it, why doesn't this" gaps that surface the moment a real user tries cofferdam.

### `cofferdam check --explain <check_id>`
Print the check's full doc + good/bad example. Today users hit a finding and have no in-tool path to "why is this flagged?".

- New CLI subcommand or flag in `crates/cofferdam-cli/src/main.rs` (the `Cli` / `Cmd` enums around line 26-62).
- `CheckMeta::explanation` already exists in `crates/cofferdam-core/src/check.rs` — just needs surfacing.
- After cd-b3k lands the generated catalog at `docs/checks/<category>.md`, `--explain` can `cat` the relevant section for nicer output. Until then, fall back to `CheckMeta::explanation` raw.

### `cofferdam check --watch`
Re-run on file save. The single biggest inner-loop DX improvement before LSP lands in phase 6.

- ~50 LOC using the `notify` crate (add to `crates/cofferdam-cli/Cargo.toml`).
- Wrap the existing `Engine::analyze` call in `crates/cofferdam-cli/src/main.rs::Cmd::Check` handler with a debounced loop.
- Honor `.gitignore` / `.cofferdamignore` for the watch set (re-use `crates/cofferdam-engine/src/discover.rs`).

### Standard CLI ergonomics: `--max-issues=N`, `--quiet`, `NO_COLOR`
Table-stakes. All in `crates/cofferdam-cli/src/main.rs`:

- `--max-issues=N`: short-circuit `Engine::analyze` once N findings collected. Bounds output for huge repos and CI memory.
- `--quiet`: suppress info lines, only emit findings. Pairs naturally with the future `--max-issues`.
- `NO_COLOR` env: industry-standard. Check whether the text formatter at `crates/cofferdam-formatters/src/text.rs` already honors it (the build error trace earlier suggested `owo-colors` is the color crate — it auto-detects `NO_COLOR` if used through the `OwoColorize::if_supports_color` API).

### One real autofix — `Warning.TripleEquals` (`==` → `===`)
Phase-4 in the README roadmap, but a single autofix as proof-of-concept dramatically changes how the tool feels. Pick TripleEquals because it's the simplest mechanical replacement.

- New `Cmd::Fix` subcommand in `crates/cofferdam-cli/src/main.rs`.
- Add an `autofix` capability to the `Check` trait in `crates/cofferdam-core/src/check.rs`. Could be `fn autofix(&self, issue: &Issue, source: &SourceFile) -> Option<TextEdit>`.
- Implementation in `crates/cofferdam-checks/src/warning.rs::TripleEquals` — it already has the byte spans of the `==` / `!=` operator, just emit `===` / `!==` over them.
- Apply edits in order, write the file atomically (temp + rename).
- Out of scope: interactive mode (`--interactive`), conflict resolution (multiple checks editing the same span).

---

## Bucket 2 — Distribution channels beyond npm

npm catches TS users but misses sizable adjacent audiences. Each channel is a different cohort that won't install cofferdam through npm.

### Homebrew tap + formula
`brew install cofferdam` is the macOS-native expectation.

- Separate repo `TAJD/homebrew-cofferdam` with a single `Formula/cofferdam.rb`.
- Formula points at the macOS arm64 + x64 release archives already produced by `.github/workflows/release.yml` (lines 60-70 in that file build both via cargo-zigbuild on the macos-15 runner).
- New step in `release.yml` post-publish job: bump the formula's `url` + `sha256` and PR it to the tap repo. ~30 lines.
- ~2 hours one-time work.

### Publish to crates.io
README says `cargo install --git ... cofferdam-cli` works, but it's not on crates.io.

- All workspace crates need crates.io-compatible deps (no `git = ...` deps). Audit `Cargo.toml` files under `crates/`.
- Publish order: `cofferdam-core` → `cofferdam-engine` → `cofferdam-checks` → `cofferdam-formatters` → `cofferdam-cli`.
- New step in `.github/workflows/release.yml` after the GitHub Release publishes. Use `cargo publish --token ${{ secrets.CRATES_IO_TOKEN }}` for each crate in dep order.
- Will also need a `CRATES_IO_TOKEN` repo secret (GitHub OIDC for crates.io is in beta but not GA as of Q1 2026).

### Scoop manifest + winget package
Windows-native installs.

- Scoop: separate `TAJD/scoop-cofferdam` bucket repo with `bucket/cofferdam.json`. Auto-update via Scoop's autoupdate manifest format pointing at the GitHub Releases.
- winget: PR a manifest to `microsoft/winget-pkgs` under `manifests/t/TAJD/cofferdam/<version>/`. Manual or via `wingetcreate update`.

### Docker image
For CI environments that prefer image pinning over binary downloads.

- New `Dockerfile` at repo root, multi-stage (builder + alpine runtime). Final image ~50 MB.
- New step in `release.yml` pushes to `ghcr.io/tajd/cofferdam:<version>` and `:latest`.
- README usage: `docker run -v $PWD:/src ghcr.io/tajd/cofferdam check /src`.

---

## Bucket 3 — Check pack expansion

5 built-in checks is a starter set, not an adoption set. Quick wins, all using existing recipes per `CLAUDE.md`'s "Writing a check" section. Each is one file in `crates/cofferdam-checks/src/<category>.rs`, one fixture in `examples/`, one line in `crates/cofferdam-checks/src/lib.rs::all_builtins()`.

### `Warning.NoConsoleLog`
Every TS shop wants this. Probably 90% of teams have an ESLint rule for it.

- AST visitor (single node) — same shape as `crates/cofferdam-checks/src/warning.rs::TripleEquals`.
- Match `CallExpression` where `callee` is `MemberExpression { object: Identifier("console"), property: Identifier(_) }`. Emit on any `console.*` call by default; config can narrow to specific methods (`["log", "debug"]`).
- Fixture: `examples/no_console_log.ts` with mixed flagged/non-flagged cases.
- Default severity: Low (it's a debug aid, not a bug). Default priority: -10 (sort late).

### `Warning.NoDebugger`
Companion to NoConsoleLog. Even simpler — match `DebuggerStatement`.

### `Warning.NoEval`
Universal security ban.

- Match `CallExpression` where `callee` is `Identifier("eval")` OR `MemberExpression { object: Identifier("Function"), property: Identifier("prototype") }`.
- Default severity: **High** (security category isn't first-class yet but eval is universally banned).
- Doubles as the canary for the eventual "promote Security to a first-class category" config story (README section).

### `Refactor.PreferOptionalChain` / `PreferNullishCoalescing`
Modern TS idioms. Type-aware tier (phase 5) is the right home for full coverage, but high-confidence cases (e.g. `a && a.b && a.b.c`) are detectable from AST shape alone — ship now, expand in phase 5.

- Reference: `crates/cofferdam-checks/src/refactor.rs` already has CyclomaticComplexity / CognitiveComplexity / DuplicateBlock — same module.
- Mark `requires_types: false` initially. Re-mark to `true` when the type-aware tier (phase 5) lands and the rule grows to cover ambiguous cases.

### `Consistency.QuoteStyle`
Already named in the README phased roadmap as the phase-2 canary, but no bead.

- Two-pass mode: first pass scans every string literal, second pass picks the dominant quote style as the project default, third walk emits findings for non-conforming strings.
- Engine support for two-pass mode is partially in place (the `consistency: bool` flag in `CheckMeta` mentioned in `CLAUDE.md`'s scaffolding example) but the engine's two-pass loop isn't wired yet — needs work in `crates/cofferdam-engine/src/lib.rs`.
- This is the bigger lift in this bucket; might justify its own bead.

---

## Bucket 4 — Community hygiene

None of this affects the binary; all of it matters the moment a second contributor arrives.

### Repo-root files
- **`CONTRIBUTING.md`** — referenced by cd-b3k acceptance but not tracked as a deliverable. Cover: how to add a check (link the recipe in `CLAUDE.md`), how to run the verification block (`cargo build/test/clippy/fmt + smoke against bestefforttools`), the parallel-agent dispatch rules.
- **`SECURITY.md`** — vuln-reporting policy. GitHub prompts users to add this on the repo home page. Template: report via `security@<email>` or a private GitHub Security Advisory.
- **`CODE_OF_CONDUCT.md`** — Contributor Covenant 2.1 boilerplate. Required for some org's contribution policies.

### `.github/` files
- **`.github/ISSUE_TEMPLATE/bug.yml`** — repro steps, expected vs actual, cofferdam version, OS/arch.
- **`.github/ISSUE_TEMPLATE/feature.yml`** — use case, why current behaviour is insufficient.
- **`.github/ISSUE_TEMPLATE/check-request.yml`** — proposed check ID, category, what it flags, example good/bad code.
- **`.github/pull_request_template.md`** — checklist: bead ID closed, fixture added, verification block ran, no `println!` in checks.
- **`.github/dependabot.yml`** — three ecosystems: `cargo` (root), `github-actions` (`.github/workflows/`), `npm` (`packages/cofferdam/`). Weekly cadence is enough.
- **`.github/FUNDING.yml`** — only if you want sponsorship buttons. Skip otherwise.

### Repo settings (one-time, not in-repo)
- Enable **GitHub Discussions** — better Q&A surface than issues for "how do I configure X".
- Add the docs site URL (cd-m77 once VitePress lands) to the repo "About" sidebar Website field.
- Branch protection on `main`: require CI green, require 1 approval, no direct pushes.

---

## Bucket 5 — Performance + observability

Becomes urgent the moment someone tries cofferdam on a 500k LOC monorepo.

### `cofferdam check --time-checks`
Per-check duration breakdown to stderr. Invaluable when a custom check makes the run slow.

- Instrument the per-check call in the engine's parse loop at `crates/cofferdam-engine/src/lib.rs` — wrap each `check.run(file, ctx)` in `Instant::now()` / `.elapsed()`, accumulate per-check totals.
- Emit summary at end of run, sorted descending. Goes to stderr, doesn't pollute findings JSON.
- Trivial — maybe 30 LOC. Should land before any incremental-cache work because it tells you whether incremental is even worth doing.

### Incremental check mode
Cache findings keyed by file-content hash. On re-check, only re-process changed files. Big DX win for large repos. Pairs naturally with `--watch`.

- New module under `crates/cofferdam-engine` — `cache.rs` or similar.
- Persist to `.cofferdam/cache/findings.json` (per the canonical layout decided in cd-fnm). Cache schema versioned.
- Hash key: SHA-256 of `(file_content_bytes, set_of_active_check_ids, check_versions)`. Invalidate aggressively when any input changes.
- `Engine::analyze` consults the cache before running each check, only re-runs on a miss.

### Benchmark harness (Criterion)
Regressions get caught instead of trickling out as user reports.

- New top-level `benches/` directory at workspace root.
- Criterion benches that run the engine against vendored TS files (or symlink to `examples/`).
- New CI job in `.github/workflows/ci.yml`: run benches in CI on PR, post regression summary as PR comment via `criterion-compare-action` or similar.

### Memory baseline
README claims "<2s on 100k LOC across 8 cores" but doesn't bound memory.

- Extend `scripts/smoke.ps1` to capture peak RSS via PowerShell `Get-Process` polling or `Measure-Command` + working-set tracking.
- Add the captured number to `scripts/smoke-baseline.json`.
- Lock as a regression gate in CI: fail if RSS grows >20% vs baseline.

---

## Bucket 6 — Editor inner-loop (before LSP)

LSP is phase 6 in the README and won't land soon. A thin VS Code extension that shells out to `cofferdam check --robot` on save and surfaces findings as VS Code diagnostics is **~200 lines of TypeScript** and closes ~80% of the inner-loop gap. Major adoption lever for individuals before the org-level CI story matters.

### What ships
- New top-level `editors/vscode/` directory (or separate `cofferdam-vscode` repo to keep CI matrix simple).
- Shell out to `cofferdam check --robot` on save (debounced).
- Parse the JSON output (already documented; stable schema per `CLAUDE.md`).
- Map findings to `vscode.Diagnostic[]` with severity from check severity.
- Status-bar item showing finding count.
- Configuration: `cofferdam.path` (binary location, default search PATH), `cofferdam.checkOnSave` (default true), `cofferdam.cofferdamArgs` (extra flags).

### What it doesn't do (deferred to real LSP)
- Hover docs for findings — needs LSP request/response.
- Quick-fix actions — needs `--fix` first (Bucket 1).
- Workspace symbols — needs LSP indexing.

### Distribution
- Publish to **VS Code Marketplace** under publisher `tajd`. Free.
- Optionally publish to **Open VSX** for VSCodium / Cursor users.

---

## Bucket 7 — Test rigor

Get this in before the codebase grows past the point where retrofitting hurts. The cofferdam codebase is still small enough that adding these is cheap.

### Coverage gate in CI
- Add `cargo-llvm-cov` to `.github/workflows/ci.yml` (`cargo install cargo-llvm-cov` step + `cargo llvm-cov --workspace --lcov --output-path lcov.info`).
- Upload to Codecov via `codecov/codecov-action@v4`.
- Surfaces dead code paths; prevents tests from rotting silently.

### Snapshot tests for formatters
- Add `insta` crate to `crates/cofferdam-formatters/Cargo.toml` (dev-dep).
- New tests under `crates/cofferdam-formatters/tests/` — render a known set of issues through both `TextFormatter::render` and `JsonFormatter::render`/`render_pretty`, snapshot the output.
- Locks down the JSON shape (which is the public contract per `CLAUDE.md`'s "Output formats" section) and the text-formatter output. Accidental drift fails CI.

### Property-based tests for parser + discovery
- Add `proptest` crate as a dev-dep on `cofferdam-engine` and `cofferdam-core`.
- Targets:
  - **Span/UTF-8 invariants** — `crates/cofferdam-core/src/span_from_bytes` (mentioned in `CLAUDE.md` as having a UTF-8 nuance on column count). Property: for any valid UTF-8 string and byte range within it, `span_from_bytes` produces a Span whose line+column round-trips back to the same byte offset via the inverse function.
  - **Discovery determinism** — `crates/cofferdam-engine/src/discover.rs::discover`. Property: given a directory tree and a `DiscoveryOptions`, the returned Vec<PathBuf> is identical across repeated calls (no race, no ordering flakiness).
  - **Engine idempotency** — running the engine twice over the same input produces the same `Vec<Issue>`.

---

## Cross-cutting things deliberately NOT in this roadmap

These came up in the brainstorm but don't make the cut. Filed here so the next session doesn't re-suggest them.

| Idea | Why not |
|---|---|
| Persistent `cofferdam daemon` mode (CLI calls IPC into long-running process) | LSP server (phase 6) is the better answer. Daemon mode would compete with LSP for the same "eliminate startup cost" use case without LSP's editor integration. |
| `cofferdam-action` GitHub Marketplace listing | Already a downstream artifact of cd-h1n; listing it on the Marketplace is a 5-min web form, not a bead. |
| Custom domain `cofferdam.dev` | Premature. `tajd.github.io/cofferdam` (cd-t6n) ships fine for v1. Claim the domain when there's traffic to point at it. |
| Telemetry / version-check / outdated-warning | Opt-in territory; deliberately defer. cofferdam doesn't need to phone home, and adding it later is much easier than removing it later. |
| JetBrains IntelliJ plugin | Bigger lift than VS Code (Kotlin + IntelliJ Platform). Defer until either someone in the JB ecosystem volunteers it or LSP is real (then most of the work is just LSP wiring). |
| Marketing landing page (separate from docs site) | Wait until v0.5+ when the story is fuller and there's more to market. |

---

## Filing protocol when ready

When converting this to beads (after cd-g4y is materially underway):

1. **One sibling epic** under cofferdam: `cd-? "Post-v1 next ring"` with a description that points back at this file.
2. **Each bucket = one child issue**, NOT one bead per item. Buckets cluster naturally; sub-items become acceptance-criteria bullets within the bucket bead.
3. **Exception**: Bucket 3's `Consistency.QuoteStyle` deserves its own bead because it requires engine work (two-pass mode) that the other checks don't.
4. **Exception**: Bucket 6's VS Code extension might warrant its own epic if it grows beyond a thin shim — defer the call until the shim is built.

Don't file buckets 5–7 until cd-g4y is closing. They're horizons, not commitments.
