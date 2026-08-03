# Claude / Agent Instructions

## What cofferdam is

TypeScript code-quality analyzer. Rust workspace at the repo root + planned `@cofferdam/*` npm packages. Five-category model (Consistency, Design, Readability, Refactor, Warning), priority-sorted within each. Inspired by Elixir's [Credo](https://github.com/rrrene/credo); see README for design context.

## Start of task

Run `cargo run -p cofferdam-cli -- context` (or `cofferdam context` if installed) before you start editing. It resolves your working-tree diff and prints a token-budgeted digest — delta-scoped findings, blast radius, sibling-file precedent, `.cofferdam/knowledge/*.md` notes, and inline `// @cofferdam-context:` annotations. Advisory only, always exits 0. We ship it as the default agent entrypoint, so dogfood it here.

## Build / toolchain

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

`rust-toolchain.toml` pins `channel = "stable"` (host-portable for CI). MSRV is **1.93**, declared in `Cargo.toml` `rust-version` and enforced by a dedicated CI job (cd-4kfk) — don't use newer-than-MSRV language features without bumping it deliberately.

**Windows: use the default MSVC toolchain. Do NOT set `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`.** This machine has the MSVC C++ workload installed; a GNU override mixes GNU-built artifacts with MSVC-built ones (including the git hooks' cargo runs) and fails with LNK1103 link errors, blocking pushes. CI runners pick up their native host triple and need no override. Never edit `rust-toolchain.toml` to pin a Windows-specific channel — that breaks Linux/macOS CI.

Lints are centralised in the root `Cargo.toml` `[workspace.lints]` table; every crate inherits via `[lints] workspace = true` in its own `Cargo.toml`. A new crate MUST include that stanza or clippy behavior diverges from the workspace.

## Git hooks (opt-in)

Tracked hooks live in `.githooks/`. Mirror the cheapest `ci.yml` checks so drift is caught locally instead of on a red CI run. Install once per checkout:

```bash
git config core.hooksPath .githooks
```

`pre-commit` runs `cargo fmt --check` + `cofferdam gen-docs --check` (sub-second on warm cache). There is no `pre-push` hook — `cargo clippy -D warnings` and `cargo test --workspace` are enforced by CI instead, since a local pre-push run duplicated verification already done seconds earlier without adding independent value CI (the actual merge gate) doesn't already provide. See `.githooks/README.md` for details. Don't bypass `pre-commit` with `--no-verify` — fix the underlying issue.

## Cutting a release

The release is **tag-triggered**: pushing a `v*` tag runs `.github/workflows/release.yml`, which builds the 7-platform binary matrix and publishes everything automatically — GitHub Releases (binaries + sha256), npm `@cofferdam/cofferdam` and `@cofferdam/check-sdk` (both via OIDC Trusted Publisher, no token), then install-smoke-tests the published packages on Linux/macOS/Windows. There is no manual publish step; the tag is the whole trigger.

Versions bump by patch digit even for features pre-1.0 (0.3.x → 0.3.(x+1)); reserve minor bumps for breaking changes. All 24 version locations must agree — `scripts/version.mjs` owns them.

**`main` is protected by a GitHub ruleset** (no bypass, including for admins): direct pushes are rejected, so the version-bump commit must go through a PR like any other change, and merging requires the `test`, `MSRV (1.93)`, `windows release build (cd-rvn guard)`, and `cargo-deny` checks to pass. Tags are not covered by the ruleset, so the `v*` tag push in step 3 still goes directly to the remote.

```bash
# 1. from a clean, green main, on a branch:
git checkout -b release/0.3.8
node scripts/release.mjs 0.3.8          # bump+regen all 24 version locations, roll CHANGELOG [Unreleased] -> [0.3.8]
git diff                                # review
git commit -am "chore(release): prepare v0.3.8" && git push -u origin release/0.3.8
gh pr create --title "chore(release): prepare v0.3.8" --fill
# 2. WAIT for the PR's required checks to go green, then merge (this gate caught the MSRV break + a plugin flake — don't skip it)
gh pr merge --squash --delete-branch
# 3. tag the merged commit on main
git checkout main && git pull
git tag -a v0.3.8 -m "cofferdam 0.3.8" && git push origin v0.3.8
```

`release.mjs` refuses a dirty tree and an empty `[Unreleased]` (pass `--allow-empty-changelog` to override). `release.yml`'s `verify` job re-checks version consistency **and** the CHANGELOG roll *before* any build, so a slip can't leave you half-published. The tag push is the irreversible step (npm versions are immutable) — that's why it's deliberately separate from prep and gated on green CI. Never let AI agents cut the tag without explicit go-ahead.

## Project structure

```
crates/
  cofferdam-core/         # Check trait, Issue, Span, parser, AST surface
  cofferdam-engine/       # discovery, orchestration, parse loop, sort
  cofferdam-checks/       # built-in checks (one file per check, grouped in category subdirs)
  cofferdam-formatters/   # text, json, compact, SARIF output
  cofferdam-cli/          # `cofferdam` binary
  cofferdam-graph/        # canonical cross-file graph schema (cd-9hp.9)
  cofferdam-rust/         # Rust source adapter — cofferdam's polylingual proof (cd-91zc)
  cofferdam-lsp/          # workspace-aware LSP server over stdio (cd-9hp.4 cp5)
  cofferdam-napi/         # napi-rs FFI surface (phase 4, stub today)
packages/                 # @cofferdam/cofferdam + @cofferdam/check-sdk (published on npm, versioned in lockstep)
examples/                 # fixture .ts files exercised by checks
examples-plugins/         # plugin SDK e2e fixtures
examples-type-host/       # ts-morph type-host CI fixtures
```

## Writing a check (the recipe)

Almost every new check is one file in `cofferdam-checks/src/<category>/<check_name>.rs`, re-exported from `<category>/mod.rs`, plus one fixture in `examples/`, and one line in `cofferdam-checks/src/lib.rs::all_builtins()`. The `readability`, `warning`, and `consistency` categories still use single files (`readability.rs`, `warning.rs`, `consistency.rs`); `design/` and `refactor/` are directory modules. Pattern by category:

| Pattern | Reference | What it does |
|---|---|---|
| Text-line scan | `readability.rs::MaxLineLength` | Iterate `file.lines()`, no AST |
| AST visitor (single node) | `warning.rs::TripleEquals` | `oxc_ast_visit::Visit`, match nodes |
| AST visitor (function-shape) | `design/max_parameters.rs::MaxParameters` | Walk `Function` + `ArrowFunctionExpression` |
| Per-function score stack | `refactor/cyclomatic_complexity.rs::CyclomaticComplexity` | Push on function entry, walk + tally, pop, emit if over limit |
| Cross-file (corpus API) | `design/duplicate_export_name.rs::DuplicateExportName` | Per-file `run` writes into `ctx.corpus`; `finalize` reads back and emits one `Issue` per match with `related: Vec<RelatedSpan>` |
| Configurable check | `warning.rs::NoConsoleLog` | Define a `&[OptionSpec]` constant, reference it from `CheckMeta.options`, read values via `ctx.options.get_string_list("...")` (or matching getter). User config lands in `[checks."Category.Name"]` in `cofferdam.invariants.toml`. |

### Required scaffolding for any AST check

```rust
use cofferdam_core::span_from_bytes;
use cofferdam_core::{Category, Check, CheckContext, CheckMeta, Issue, Priority, Severity, SourceFile};
use oxc_ast::ast::{...};
use oxc_ast_visit::Visit;

const META: CheckMeta = CheckMeta {
    id: "Category.Name",          // Stable, dotted. Don't rename.
    category: Category::Warning,  // Pick one of the five.
    base_priority: 15,            // -20..=20. Warning=15, Refactor=10, Design=5, Readability=-5, etc.
    default_severity: Severity::High,  // engine stamps this onto every emitted Issue
    explanation: "...",
    body: include_str!("../../docs/Category.Name.md"),  // long-form catalog entry; file must exist (compile-time include); ../../ because checks live in src/<category>/<check>.rs
    requires_types: false,        // true → routes through the ts-morph type host (cd-9hp.2); see design/type-host-wire.md
    consistency: false,           // true → engine runs in two-pass mode
    options: &[],                 // or a `&[OptionSpec]` const for a configurable check
    autofix: false,               // true → implement Check::autofix
    pure_run: true,               // run() is pure over (content, options) → enables findings cache. MUST be false if run() reads ctx.corpus OR ctx.types
};

impl Check for X {
    fn meta(&self) -> &'static CheckMeta { &META }
    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else { return Vec::new(); };  // ALWAYS first
        let mut visitor = Collector { file, issues: Vec::new() };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}
```

### Type-aware checks (cd-9hp.2)

A check with `requires_types: true` is routed through a Node ts-morph "type host" instead of the pure-Rust path. Query types via `ctx.types` — a `TypeOracle` that is `None` when no host is available (the engine then skips the check, so guard rather than assume). The trait + `TypeFacts` live in `cofferdam-core::types`; engine routing in `cofferdam-engine`; the Node worker + `WorkerTypeOracle` in `cofferdam-cli/src/type_host.rs`. Wire protocol: `design/type-host-wire.md`; user-facing concept + opt-out: `docs/type-aware-checks.md`. `Warning.UnusedNullCheck` (`cofferdam-checks/src/warning.rs`) is the first built-in that sets the flag. A `requires_types` check MUST keep `pure_run: false` (its findings depend on whole-project types the per-file cache can't key on). The CLI installs the worker only when a `requires_types` check is registered AND `[engine] type_aware` isn't `false` in `cofferdam.toml` (the opt-out for Node-less CI). When the host can't start (no Node / ts-morph / tsconfig), type-aware checks are skipped with one warning; `--fail-on-type-unavailable` (cd-260l) turns that into exit code 2 for CI that must not get silent false negatives.

### Cofferdam-specific gotchas (every agent needs these)

- **`oxc_syntax::scope::ScopeFlags`**, NOT `oxc_ast_visit::walk::ScopeFlags`. The `visit_function` impl signature requires it — wrong import is the most common build failure.
- **`cofferdam_core::span_from_bytes(text, start, end)`** is the only correct way to convert oxc byte offsets into our `Span` (with line/column). Do not roll your own — there's a UTF-8 nuance on the column count.
- **Always `let Some(parsed) = ctx.parsed else { return Vec::new(); };`** at the top of `run()` for AST checks. `ctx.parsed` is `None` when parsing failed (we already emitted `Warning.ParseError` for that file).
- **Visitors must call `walk::walk_<node>(self, ...)`** at the end of overridden visit methods to descend into children. Forgetting this means nested matches are missed.
- **A check that only overrides `visit_assignment_expression` misses `for...of`/`for...in` loop heads** (`for (x of xs)`, `for ([a, b] of pairs)`) — those reassign their target every iteration but are a distinct `ForStatementLeft` grammar production, not an `AssignmentExpression`. If a check's reassignment/binding logic matters for assignment targets, also override `visit_for_statement_left` (see `Refactor.PreferConstOverLet`, CD-154). More generally: oxc's `inherit_variants!` macro makes sibling enums (`SimpleAssignmentTarget`, `AssignmentTargetPattern`, `AssignmentTargetMaybeDefault`, `ForStatementLeft`, ...) share `AssignmentTarget`'s variants and gives each an `as_assignment_target()` accessor (`Option<&AssignmentTarget>`) — use that to funnel them all through one recursive collector instead of duplicating match arms per sibling enum.
- **Register the new check** in `cofferdam-checks/src/lib.rs::all_builtins()`. The compiler doesn't catch a forgotten registration.
- **Fixtures live in `examples/`** with one file per check (`triple_equals.ts`, `max_params.ts`, ...). Mix flagged + non-flagged cases.
- **Verification before claiming done**:
  ```bash
  cargo build --workspace
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo fmt --all -- --check
  cargo run -p cofferdam-cli -- check examples/<your-fixture>.ts
  ```

## Issue tracking — Projektor

Cofferdam issues live in the **Cofferdam** project (key `CD`) on Projektor (MCP-native issue tracker), not beads. Use the `mcp__projektor__*` tools directly (`get_prioritized_issues` for "what's next", `search_issues` / `list_issues` to look things up, `create_issue` / `update_issue` to file and close work).

Migrated 2026-07-03 from the prior beads (`bd`) tracker — old `cd-*` beads IDs are preserved as a `beads:cd-*` label on each migrated Projektor issue for traceability. **The `.beads/issues.jsonl` archive is gone from the tree** and only ~9 beads carried labels through the migration, so a `cd-*` ID referenced in code/docs with no matching Projektor issue is likely *lost planned work*, not closed work — check Projektor before assuming it shipped (the "scalable architecture" ticket cd-6ad was lost this way and re-filed as CD-28).

## Output formats (when adding a new one)

`cofferdam-formatters/src/<name>.rs` + register in `lib.rs`. CLI flag in `cofferdam-cli/src/main.rs`'s `OutputFormat` enum + `Cmd::Check` block. JSON schema is the contract — additive changes only.

## Rules for AI agents (you, and any subagents you spawn)

- **Never `git config`** to change the user's identity or hooks. The author info is already configured.
- **Never `--no-verify`** on commits or pushes. Hooks exist for a reason; if they fail, fix the underlying issue.
- **Never amend a previously-pushed commit.** Add a new commit instead.
- **You may close Projektor issues** once you've finished the work and the full verification block passes (build, test, clippy, fmt, fixture run). Use `mcp__projektor__update_issue` (status: `done`) and include a short reason for non-trivial work. Do NOT close without verification, and do NOT close work that's still uncommitted — close after the commit so the issue lifecycle and git history align.
- **Don't commit** when running as a subagent — leave staging + commit to the controller.
- **Validate against real repos.** Fixtures in `examples/` are necessary but not sufficient. See the "Real-repo validation" section below for the known-good repos.
- **Do not add a check whose `meta().id` collides with an existing one.** Grep `crates/cofferdam-checks` for the proposed ID first.
- **`println!`/`dbg!` in checks is forbidden.** Findings go through `Issue` only — anything else corrupts robot-mode JSON.

### Cross-file checks (corpus API)

Project-graph checks (DRY, duplicate exports, future boundary / orphan-export rules) collect during per-file `run` and emit during `finalize`. Pattern:

```rust
static MY_SLOT: CorpusKey<Vec<Fingerprint>> = CorpusKey::new("Category.MyCheck.fingerprints");

impl Check for MyCheck {
    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        ctx.corpus.with_slot(&MY_SLOT, |slot| slot.push(/* per-file data */));
        Vec::new()  // emit nothing per-file
    }
    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        ctx.corpus.with_slot(&MY_SLOT, |slot| /* group + emit */ vec![])
    }
}
```

Two checks share storage by referencing the same `CorpusKey<T>` constant (same name + same `T`); distinct keys (or a different `T`) get distinct slots. Findings spanning multiple locations use `Issue.related: Vec<RelatedSpan>` — formatters omit it when empty. The corpus already uses an outer `RwLock<HashMap>` with per-slot `Mutex`es (`cofferdam-core/src/corpus.rs`) — the locking groundwork for per-file parallelism landed, but the engine loop is still single-threaded, so nothing exercises the concurrency yet (tracked as CD-30 under the CD-28 scalability epic).

**Engine finalize ordering (cd-wqc + cd-9hp.5).** Finalize runs in two phases. Phase A runs every check that is NOT in `cofferdam_core::FINALIZE_OBSERVER_CHECK_IDS` and appends issues; the engine then rebuilds the `ALL_PRE_FILTER_FINDINGS` snapshot from the union of run + pass2 + Phase A issues; Phase B runs the observer set. Today only `Consistency.UnusedSuppression` is an observer — dispatch is by check ID, not by a generic `CheckMeta` flag (cd-9hp.5 removed the flag because it had one user in six months). If you write a new check that emits from `finalize()` AND needs to see other checks' findings, add its ID to `FINALIZE_OBSERVER_CHECK_IDS` in `crates/cofferdam-core/src/check.rs`.

## Parallel agent dispatch (when running multiple agents)

Cofferdam's structure is parallel-friendly because most checks are one self-contained file. Safe to fan out:

- New checks under `cofferdam-checks/src/<category>/<check_name>.rs` + re-export in `<category>/mod.rs` (each agent picks an unused name)
- New formatters under `cofferdam-formatters/src/<format>.rs`
- New examples / fixtures
- Documentation pages under `docs/`

NOT safe to parallelize — sequential single-agent only:

- Anything touching `Check`, `CheckContext`, `CheckMeta` (cofferdam-core/src/check.rs) — the central trait
- `DiscoveryOptions` / discovery loop
- The engine's parse loop in `cofferdam-engine/src/lib.rs`
- The `OutputFormat` enum + `Cmd::Check` glue in `cofferdam-cli/src/main.rs` (multiple agents will conflict on the same lines)

When dispatching for parallel check work:
1. One git worktree per agent (`git worktree add ../cofferdam-check-<name> -b check-<name>`).
2. Inline the recipe above in the prompt — don't say "see CLAUDE.md", subagents don't auto-load it. Or do, but include the gotchas section verbatim.
3. Tell the agent the exact file path + the existing check it should model on.
4. End with: "Do NOT commit. Do NOT close the Projektor issue. Run the verification block; paste the last 20 lines."
5. Controller pulls each branch, verifies independently, merges (resolves only `lib.rs` registration conflicts), closes the Projektor issue.

**Windows caveat:** the Agent tool's `isolation: "worktree"` has been observed to silently fall back to the main working tree on this host — agent edits show up directly in `git status` of the controller, with no per-agent branch to pull. When that happens, agents touching the same lines will overwrite each other with no merge step. Mitigations: only fan out when each agent's edits target disjoint methods/files (different `Check` structs, different `visit_X` methods on the same struct); have each agent run the full verification block before reporting back so the resulting tree is at least internally consistent; controller spot-checks `git status` after dispatch instead of trusting the worktree-list output.

## Real-repo validation

Test fixtures in `examples/` are necessary but not sufficient. Spot-check changes against a sizable real-world TypeScript repo — any local checkout that has cofferdam enabled is a good candidate — to catch whether a change introduces unexpected false positives or parse errors. No baseline number is enshrined here; the point is qualitative ("did the top-10 findings shift in ways I can defend?"), not a numeric regression gate.

**Run validation with `--no-cache` (or clear `.cofferdam/cache/` first).** The findings cache is keyed on `(content, config, engine_version)`, so a rebuild under the *same* version replays a prior build's findings from the disk cache — stale entries silently mask the very change you're validating. The cache self-heals on release (the version bump invalidates the cache dir); it only bites developers re-running the same version against an already-cached repo.
