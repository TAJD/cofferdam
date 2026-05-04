# Continuation prompt — finishing the SDK epic the right way

Paste this into a fresh Claude Code session (or `claude /resume` then dump it
in) when you want to pick the SDK epic back up. It's self-contained — assume
the new session has no memory of what came before.

---

## Context

You're continuing work on the cofferdam plugin SDK epic (`cd-81a`).
The previous session shipped substantial scaffolding on the
`cd-7e4-plugin-sdk-e2e` worktree at
`C:\Users\tajdi\cofferdam\cd-7e4-plugin-sdk-e2e`. **11 commits are
already pushed**, including:

- `@cofferdam/check-sdk` TypeScript package (types, defineCheck factory,
  plugin host, loader)
- Biome-style suppression syntax (`// cofferdam-ignore: …`)
- `Issue.fix` payload + CLI fix-engine prefers it
- `LineView::span_for` + `is_jsx_text` flag
- File-scope filtering (`FileScope` + globset matcher)
- `cofferdam-napi` crate (excluded from workspace; needs `libnode.dll`)
- `examples-plugins/brand-casing/` plugin compiles against the SDK
- `scripts/regen-plugin-fixtures.mjs` + `check-plugin-fixtures.mjs`

11 of the SDK-tree beads are closed. **6 newly-filed beads** capture the
platform/language split work the design doc
(`design/platform-extensibility.md`) demands before cd-81a.2 freezes.

## What's now in the queue

Run `bd ready` to see unblocked work. The dependency chain is:

```
cd-8wj (crate split) ──► cd-jub (Language trait) ──► cd-81a.2 (AST surface)
            │                                       ▲
            ├──► cd-7ws (CI guardrails)             │
            ├──► cd-2ej (CLAUDE.md update)          │
            └──► cd-9kn (lang-adapter doc)          │
                                                    │
            cd-717 (AST design note) ───────────────┘
```

**Top of queue: cd-8wj.** Until that lands, cd-81a.2 / cd-7e4 / cd-b5h
/ cd-11j / cd-81a.7 (the napi loader's AST projection) all stay
parked. The design doc strongly recommends shipping the split BEFORE
freezing the AST — otherwise plugin authors get broken twice.

## What "win" looks like for cd-8wj

Read `design/platform-extensibility.md` first (in the main worktree at
`C:\Users\tajdi\cofferdam\design\platform-extensibility.md`).

Then deliver:

1. **New crate `crates/cofferdam-ts/`** that owns:
   - oxc parser invocation (move `cofferdam-core/src/parser.rs`)
   - `ParsedView` (move from core)
   - `LiteralCollector` and the JSX-text + string-literal AST walks
     (move from `cofferdam-core/src/lines.rs`)
   - `AstView`, `AstVisitor`, `Walk`, `NodeKind`, `NodeRef` (move from
     `cofferdam-core/src/ast.rs`)
   - The CLAUDE.md "Required scaffolding for any AST check" prelude

2. **`crates/cofferdam-core/Cargo.toml`** has NO `oxc*` dependency —
   verify with `cargo metadata --format-version 1 | grep oxc` (in core's
   tree only) returning nothing relevant.

3. **`cofferdam-core/src/lib.rs`** has no `pub use oxc_*` re-exports.
   The `Allocator`/`oxc_ast`/`oxc_span` re-exports (currently lines
   ~33-37) move out.

4. **`LineView` struct stays in core**, but its classification pass —
   the AST walk that populates `is_string_literal`, `is_jsx_text` —
   moves to `cofferdam-ts`. Per the design's decision #3 leaning.

5. **`cofferdam-checks` depends on `cofferdam-ts`**, not on oxc
   directly. Built-ins import via `cofferdam_ts::ast::*` /
   `cofferdam_ts::visit::*` style.

6. **All workspace verification clean:**
   ```
   cargo build --workspace
   cargo test --workspace      # currently 285+ tests
   cargo clippy --workspace --all-targets -- -D warnings
   cargo fmt --all -- --check
   ```

## Then immediately follow with cd-jub (Language trait)

Per the design doc's `Language` trait:

```rust
pub trait Language: 'static {
    type Ast: Send + Sync;
    fn name() -> &'static str;
    fn default_extensions() -> &'static [&'static str];
    fn parse(source: &str) -> Result<Self::Ast, ParseError>;
}
```

Make `Engine<L: Language>` and `Check<L: Language>` and
`CheckContext<'_, L>` generic. `cofferdam_ts::TypeScript` is the only
impl today. The CLI binary wires it in.

Add a **platform-purity doc test** in `cofferdam-core` that builds a
`Check`, runs it via the engine, emits an `Issue` — without pulling in
`cofferdam-ts` as a dependency. If the test ever needs a TS dep, the
abstraction has eroded.

## After both land

- File the **CI guardrails** (cd-7ws): the four mechanical checks the
  design lists — `cargo metadata` assertion, `rg 'use oxc'` outside the
  adapter, SDK dist guard, platform-purity doc test.
- Update **CLAUDE.md** (cd-2ej): the AST visitor recipes need
  `cofferdam_ts::ast::*` instead of `oxc_ast::ast::*`. Subagents read
  CLAUDE.md to write checks; if it's stale they regress to oxc imports.
- Land the **AST design note** (cd-717) before any new `cd-81a.2` work
  hits the SDK. Lock the v0 node taxonomy with a fixture-justification
  per type.

## Tooling rules to obey

These come from the existing `CLAUDE.md` at the repo root and `~/.claude/CLAUDE.md`:

- `bd` (beads) for tasks. Don't use TodoWrite. `bd ready` finds work.
  Claim with `bd update <id> --claim --status in_progress` before
  starting. Close with `bd close <id> --reason="..."` AFTER the
  verification block passes AND the work is committed.
- Worktree branch is `cd-7e4-plugin-sdk-e2e`. Continue committing there
  (the work is logically one feature stream). Push on every commit.
- On Windows host: `export RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu`
  before cargo commands (the rust-toolchain.toml pins MSVC which needs
  the C++ workload).
- `cofferdam-napi` is **excluded** from the workspace (needs
  `libnode.dll`). Don't try to put it back into the `members` list —
  workspace builds will break. The crate compiles standalone via
  `cargo build --manifest-path crates/cofferdam-napi/Cargo.toml` only
  with a Node dev environment.
- Never `git config`, never `--no-verify`, never amend pushed commits.

## Quick orient commands

```bash
cd C:\Users\tajdi\cofferdam\cd-7e4-plugin-sdk-e2e
git log --oneline -12                      # what shipped previously
bd list --status=in_progress | head -20    # who's holding what
bd show cd-8wj                             # the bead you're starting on
bd show cd-jub                             # the next one
cat design/platform-extensibility.md       # the architectural North Star
                                           # (lives in the main worktree;
                                           # cd ../ if missing here)
```

## Don't

- Don't add a second language adapter (Python/Go) — the point is
  preserving the option, not exercising it. Out of scope per the design.
- Don't reopen the already-closed cd-81a.* beads. Their external
  acceptance gates are met. The structural work is captured in cd-8wj.
- Don't try to land cd-81a.2 (AST surface) before cd-8wj + cd-jub +
  cd-717 land — you'll bake oxc shape into the SDK forever.
- Don't touch `cofferdam-napi` until libnode.dll is set up via
  `@napi-rs/cli` or `NAPI_NODE_DEV_DIR`. The crate's source is correct;
  the link step needs the Node dev environment.

## When you're done with the split

Run `bd close cd-8wj cd-jub` with a reason citing the verification
block that passed, then move to cd-7ws (CI guardrails) so the layering
can't erode silently. After that, cd-717 (AST design note) unblocks
cd-81a.2 (AST surface freeze), which unblocks cd-7e4 (BrandCasing
fixture runs end-to-end), and the rest of the SDK epic completes
naturally.
