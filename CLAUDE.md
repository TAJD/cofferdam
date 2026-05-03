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

## Project structure

```
crates/
  cofferdam-core/         # Check trait, Issue, Span, parser, AST surface
  cofferdam-engine/       # discovery, orchestration, parse loop, sort
  cofferdam-checks/       # built-in checks (one file per category)
  cofferdam-formatters/   # text + json output, future compact/SARIF
  cofferdam-cli/          # `cofferdam` binary
  cofferdam-lsp/          # LSP server (phase 6, stub today)
  cofferdam-napi/         # napi-rs FFI surface (phase 4, stub today)
packages/                 # @cofferdam/* npm packages (phase 4+)
examples/                 # fixture .ts files exercised by checks
```

## Writing a check (the recipe)

Almost every new check is one file in `cofferdam-checks/src/<category>.rs`, one fixture in `examples/`, and one line in `cofferdam-checks/src/lib.rs::all_builtins()`. Pattern by category:

| Pattern | Reference | What it does |
|---|---|---|
| Text-line scan | `readability.rs::MaxLineLength` | Iterate `file.lines()`, no AST |
| AST visitor (single node) | `warning.rs::TripleEquals` | `oxc_ast_visit::Visit`, match nodes |
| AST visitor (function-shape) | `design.rs::MaxParameters` | Walk `Function` + `ArrowFunctionExpression` |
| Per-function score stack | `refactor.rs::CyclomaticComplexity` | Push on function entry, walk + tally, pop, emit if over limit |
| Cross-file (corpus API) | `design.rs::DuplicateExportName` | Per-file `run` writes into `ctx.corpus`; `finalize` reads back and emits one `Issue` per match with `related: Vec<RelatedSpan>` |

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
    explanation: "...",
    requires_types: false,        // true → routes to ts-morph in phase 5
    consistency: false,           // true → engine runs in two-pass mode
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
- **Validate against real repos.** Test fixtures in `examples/` are necessary but not sufficient. Run against `C:/Users/tajdi/bestefforttools` (325 TS files), `C:/Users/tajdi/gistreact` (31), `C:/Users/tajdi/rovikore-landing-page` (4) — all known to parse cleanly with the current oxc setup.
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

## Parallel agent dispatch (when running multiple agents)

Cofferdam's structure is parallel-friendly because most checks are one self-contained file. Safe to fan out:

- New checks under `cofferdam-checks/src/<category>.rs` (each agent picks an unused name)
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

## Validated reference points

Real-repo benchmarks captured during development; useful for sanity-checking that a change hasn't regressed. Numbers update as new checks land — the point is "did this PR cause an unexpected swing?", not "is this number forever correct".

| Repo | Files | Findings | Release time |
|---|---|---|---|
| `C:/Users/tajdi/bestefforttools` | 325 | 398 | 269 ms |
| `C:/Users/tajdi/gistreact` | 31 | 110 | 205 ms |

`C:/Users/tajdi/rovikore-landing-page` was on the list earlier but no longer contains TS files at that path — dropped.

Per-check breakdown on bestefforttools (captured 2026-05-02 after the full Refactor/Design check chain landed: cd-0ps, cd-qf3, cd-4cr, cd-vlq, cd-qnu, cd-jdq, cd-s2k, cd-39c, cd-u30, cd-2pu, cd-mti):

| Check | Hits |
|---|---|
| `Readability.MaxLineLength` (limit 120) | dominant baseline |
| `Readability.MaxFunctionLength` (limit 50) | dominant baseline |
| `Warning.TripleEquals` | a handful, all real `==` / `!=` |
| `Design.MaxParameters` (limit 5) | low |
| `Design.DuplicateExportName` | 8 |
| `Refactor.CyclomaticComplexity` (limit 10) | 20 |
| `Refactor.CognitiveComplexity` (limit 15) | 9 (subset of cyclomatic) |
| `Refactor.DuplicateBlock` (≥6 stmts, ≥80 chars, AST-canonical) | 13 |

Spot-checked: zero false positives in the top-10 of each new cross-file/complexity check — duplicate exports are real barrel collisions, duplicate blocks are real test-setup copy-paste, complexity hits are real deeply-nested reducers / handlers. Limits tuned by gut-feel from the spot-check; revisit if a refactor cluster makes the noise:signal ratio drop.

Zero parse errors across both repos under oxc 0.128 / cofferdam main.
