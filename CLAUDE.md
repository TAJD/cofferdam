# Claude / Agent Instructions

## What cofferdam is

TypeScript code-quality analyzer. Rust workspace at the repo root + planned `@cofferdam/*` npm packages. Five-category model (Consistency, Design, Readability, Refactor, Warning), priority-sorted within each. Inspired by Elixir's [Credo](https://github.com/rrrene/credo); see README for design context.

## Build / toolchain

```bash
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

`rust-toolchain.toml` pins `channel = "stable"` (host-portable for CI). On a Windows box without the MSVC C++ workload, local builds need:

```bash
export RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu   # bash
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu" # PowerShell
```

CI runners pick up their native host triple and need no override. Never edit `rust-toolchain.toml` to pin a Windows-specific channel — that breaks Linux/macOS CI.

## Git hooks (opt-in)

Tracked hooks live in `.githooks/`. Mirror the cheapest `ci.yml` checks so drift is caught locally instead of on a red CI run. Install once per checkout:

```bash
git config core.hooksPath .githooks
```

`pre-commit` runs `cargo fmt --check` + `cofferdam gen-docs --check` (sub-second on warm cache). `pre-push` runs `cargo clippy -D warnings` + `cargo test --workspace` (10s+ but only fires before publishing). See `.githooks/README.md` for details. Don't bypass with `--no-verify` — fix the underlying issue.

## Project structure

```
crates/
  cofferdam-core/         # Check trait, Issue, Span, parser, AST surface
  cofferdam-engine/       # discovery, orchestration, parse loop, sort
  cofferdam-checks/       # built-in checks (one file per check, grouped in category subdirs)
  cofferdam-formatters/   # text + json output, future compact/SARIF
  cofferdam-cli/          # `cofferdam` binary
  cofferdam-graph/        # canonical cross-file graph schema (cd-9hp.9)
  cofferdam-rust/         # Rust source adapter — cofferdam's polylingual proof (cd-91zc)
  cofferdam-lsp/          # workspace-aware LSP server over stdio (cd-9hp.4 cp5)
  cofferdam-napi/         # napi-rs FFI surface (phase 4, stub today)
packages/                 # @cofferdam/* npm packages (phase 4+)
examples/                 # fixture .ts files exercised by checks
```

## Writing a check (the recipe)

Almost every new check is one file in `cofferdam-checks/src/<category>/<check_name>.rs`, re-exported from `<category>/mod.rs`, plus one fixture in `examples/`, and one line in `cofferdam-checks/src/lib.rs::all_builtins()`. The `readability` and `warning` categories still use single files (`readability.rs`, `warning.rs`); `design/`, `refactor/`, and `consistency/` are directory modules. Pattern by category:

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

A check with `requires_types: true` is routed through a Node ts-morph "type host" instead of the pure-Rust path. Query types via `ctx.types` — a `TypeOracle` that is `None` when no host is available (the engine then skips the check, so guard rather than assume). The trait + `TypeFacts` live in `cofferdam-core::types`; engine routing in `cofferdam-engine`; the Node worker + `WorkerTypeOracle` in `cofferdam-cli/src/type_host.rs`. Wire protocol: `design/type-host-wire.md`; user-facing concept + opt-out: `docs/type-aware-checks.md`. `Warning.UnusedNullCheck` (`cofferdam-checks/src/warning.rs`) is the first built-in that sets the flag. A `requires_types` check MUST keep `pure_run: false` (its findings depend on whole-project types the per-file cache can't key on). The CLI installs the worker only when a `requires_types` check is registered AND `[engine] type_aware` isn't `false` in `cofferdam.toml` (the opt-out for Node-less CI).

### Cofferdam-specific gotchas (every agent needs these)

- **`oxc_syntax::scope::ScopeFlags`**, NOT `oxc_ast_visit::walk::ScopeFlags`. The `visit_function` impl signature requires it — wrong import is the most common build failure.
- **`cofferdam_core::span_from_bytes(text, start, end)`** is the only correct way to convert oxc byte offsets into our `Span` (with line/column). Do not roll your own — there's a UTF-8 nuance on the column count.
- **Always `let Some(parsed) = ctx.parsed else { return Vec::new(); };`** at the top of `run()` for AST checks. `ctx.parsed` is `None` when parsing failed (we already emitted `Warning.ParseError` for that file).
- **Visitors must call `walk::walk_<node>(self, ...)`** at the end of overridden visit methods to descend into children. Forgetting this means nested matches are missed.
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

## Issue tracking — beads (bd)

`.beads/` at repo root. Prefix `cd-`. Use `bd` (the Go reference implementation, v1.0.3+). The `.beads/issues.jsonl` file is the canonical export and is committed; the working DB under `.beads/embeddeddolt/` is gitignored.

```bash
bd ready                         # next unblocked work
bd show <id>                     # full issue body + deps
bd update <id> --status in_progress
bd create "Title" --type feature --priority 2 --labels phase-1 -d "..."
bd dep add <issue> <depends-on>
bd close <id>                    # mark complete
bd export                        # write .beads/issues.jsonl
```

bd auto-syncs the JSONL on most write operations (the `--sandbox` flag disables this). If a manual flush is needed before a git operation, run `bd export`. One-time setup on a fresh checkout: `git config beads.role maintainer` (suppresses the role-not-configured warning).

### When `bd` "stops working"

`bd doctor` is **not yet supported in embedded mode** (current bd 1.0.3 limitation). Use these for diagnostics instead:

```bash
bd ping                          # confirm DB connectivity (round-trips in ~20ms)
bd info                          # show issue count + database path
```

Common failure modes, cheapest fixes first:

1. **`Error: import failed: database not initialized: issue_prefix config is missing`** — fresh DB without bootstrapping. Run any `bd info` or `bd list` command and bd will auto-import from `.beads/issues.jsonl` if present, initializing `issue_prefix` from the JSONL header. The explicit `bd import` requires `bd init --prefix <prefix>` first; auto-import does not.

2. **Stale `.beads/dolt-server.*` files** (`.lock`, `.pid`, `.port`, `.log`) — leftovers from a previous server-mode invocation. Embedded mode doesn't use them. Safe to delete when no bd process is running.

3. **`.beads/embeddeddolt/` corruption** — rare, usually from a killed mid-write process. Recovery rebuilds from the JSONL:

   ```bash
   mv .beads/embeddeddolt .beads/embeddeddolt.bad
   bd info                                         # creates fresh DB, auto-imports from .beads/issues.jsonl
   bd info | grep "Issue Count"                    # must match JSONL line count: wc -l .beads/issues.jsonl
   # ...if counts match:
   rm -rf .beads/embeddeddolt.bad
   ```

   Keep the `.bad` directory until you've confirmed the rebuild — JSONL is the recovery-of-last-resort source, but if the JSONL itself is stale you'll need the prior DB.

4. **JSONL drift from the DB** — happens if a script edited `.beads/issues.jsonl` directly without going through bd. Fix: `bd export --force` to overwrite JSONL from DB, or `bd import` to overwrite DB from JSONL. Check direction first with `git diff .beads/issues.jsonl`.

## Output formats (when adding a new one)

`cofferdam-formatters/src/<name>.rs` + register in `lib.rs`. CLI flag in `cofferdam-cli/src/main.rs`'s `OutputFormat` enum + `Cmd::Check` block. JSON schema is the contract — additive changes only.

## Rules for AI agents (you, and any subagents you spawn)

- **Never `git config`** to change the user's identity or hooks. The author info is already configured.
- **Never `--no-verify`** on commits or pushes. Hooks exist for a reason; if they fail, fix the underlying issue.
- **Never amend a previously-pushed commit.** Add a new commit instead.
- **You may close beads** once you've finished the work and the full verification block passes (build, test, clippy, fmt, fixture run). Use `bd close <id>` and include a short reason for non-trivial work. Do NOT close without verification, and do NOT close work that's still uncommitted — close after the commit so the issue lifecycle and git history align.
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

Two checks share storage by referencing the same `CorpusKey<T>` constant (same name + same `T`); distinct keys (or a different `T`) get distinct slots. Findings spanning multiple locations use `Issue.related: Vec<RelatedSpan>` — formatters omit it when empty. The single-`Mutex<HashMap>` corpus serialises slot access; cd-6ad swaps in per-slot locks once per-file parallelism lands.

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
1. One git worktree per agent (`bd worktree create check-<name>`).
2. Inline the recipe above in the prompt — don't say "see CLAUDE.md", subagents don't auto-load it. Or do, but include the gotchas section verbatim.
3. Tell the agent the exact file path + the existing check it should model on.
4. End with: "Do NOT commit. Do NOT close beads. Run the verification block; paste the last 20 lines."
5. Controller pulls each branch, verifies independently, merges (resolves only `lib.rs` registration conflicts), closes the bead.

**Windows caveat:** the Agent tool's `isolation: "worktree"` has been observed to silently fall back to the main working tree on this host — agent edits show up directly in `git status` of the controller, with no per-agent branch to pull. When that happens, agents touching the same lines will overwrite each other with no merge step. Mitigations: only fan out when each agent's edits target disjoint methods/files (different `Check` structs, different `visit_X` methods on the same struct); have each agent run the full verification block before reporting back so the resulting tree is at least internally consistent; controller spot-checks `git status` after dispatch instead of trusting the worktree-list output.

## Real-repo validation

Test fixtures in `examples/` are necessary but not sufficient. Run against `C:/Users/tajdi/bestefforttools` (325 TS files) and `C:/Users/tajdi/gistreact` (31) — both known to parse cleanly — to spot-check whether a change introduces unexpected false positives or parse errors. No baseline number is enshrined here; the point is qualitative ("did the top-10 findings shift in ways I can defend?"), not a numeric regression gate.

**Run validation with `--no-cache` (or clear `.cofferdam/cache/` first).** The findings cache is keyed on `(content, config, engine_version)`, so a rebuild under the *same* version replays a prior build's findings from the disk cache — stale entries silently mask the very change you're validating. The cache self-heals on release (the version bump invalidates the cache dir); it only bites developers re-running the same version against an already-cached repo.
