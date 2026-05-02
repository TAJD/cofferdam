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

`rust-toolchain.toml` pins `channel = "stable"` (host-portable for CI). On a Windows box without the MSVC C++ workload (notably Tom's primary dev box), local builds need:

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

## Issue tracking — beads (br)

`.beads/` at repo root. Prefix `cd-`. Use `br` (the Rust port), NOT `bd` — embedded Dolt needs CGO that's not available on the Windows-no-MSVC box.

```bash
br ready                         # next unblocked work
br show <id>                     # full issue body + deps
br update <id> --status in_progress
br create "Title" --type feature --priority 2 --labels phase-1 -d "..."
br dep add <issue> <depends-on>
br sync --flush-only             # writes JSONL after status changes
br update <id> --status closed
```

After issue updates run `br sync --flush-only` so `.beads/issues.jsonl` reflects the change before the next git operation. The DB (`.beads/beads.db*`) is gitignored; the JSONL is the canonical exported form.

## Output formats (when adding a new one)

`cofferdam-formatters/src/<name>.rs` + register in `lib.rs`. CLI flag in `cofferdam-cli/src/main.rs`'s `OutputFormat` enum + `Cmd::Check` block. JSON schema is the contract — additive changes only.

## Rules for AI agents (you, and any subagents you spawn)

- **Never `git config`** to change the user's identity or hooks. The author info is already configured.
- **Never `--no-verify`** on commits or pushes. Hooks exist for a reason; if they fail, fix the underlying issue.
- **Never amend a previously-pushed commit.** Add a new commit instead.
- **Don't auto-close beads** even after a clean build. The user verifies and closes; agents only mark `in_progress`.
- **Don't commit** when running as a subagent — leave staging + commit to the controller.
- **Validate against real repos.** Test fixtures in `examples/` are necessary but not sufficient. Run against `C:/Users/tajdi/bestefforttools` (325 TS files), `C:/Users/tajdi/gistreact` (31), `C:/Users/tajdi/rovikore-landing-page` (4) — all known to parse cleanly with the current oxc setup.
- **Do not add a check whose `meta().id` collides with an existing one.** Grep `crates/cofferdam-checks` for the proposed ID first.
- **`println!`/`dbg!` in checks is forbidden.** Findings go through `Issue` only — anything else corrupts robot-mode JSON.

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
1. One git worktree per agent (`br worktree create check-<name>`).
2. Inline the recipe above in the prompt — don't say "see CLAUDE.md", subagents don't auto-load it. Or do, but include the gotchas section verbatim.
3. Tell the agent the exact file path + the existing check it should model on.
4. End with: "Do NOT commit. Do NOT close beads. Run the verification block; paste the last 20 lines."
5. Controller pulls each branch, verifies independently, merges (resolves only `lib.rs` registration conflicts), closes the bead.

## Validated reference points

Real-repo benchmarks captured during development; useful for sanity-checking that a change hasn't regressed:

| Repo | Files | Findings (today) | Time |
|---|---|---|---|
| `C:/Users/tajdi/bestefforttools` | 325 | 348 | 0.18s |
| `C:/Users/tajdi/gistreact` | 31 | 267 | < 1s |
| `C:/Users/tajdi/rovikore-landing-page` | 4 | 366 | < 1s |
| Combined (all three) | 360 | ~981 | < 2s |

Zero parse errors across all three under oxc 0.128 / cofferdam main.
