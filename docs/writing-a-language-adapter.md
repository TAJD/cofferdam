# Writing a language adapter

> Status: **draft sketch.** Walk-through, not a tutorial — the only
> implemented adapter is `cofferdam-ts`. Created for cd-9kn alongside
> the cd-8wj / cd-jub split to validate that the platform/language
> abstraction is real.

## The thesis

After cd-8wj (crate split) and cd-jub (Language trait + generic
Engine), every crate in the workspace except `cofferdam-ts` is
language-agnostic. Adding a second adapter — Python, Go, Ruby —
means writing one new crate and one binary registration. Nothing in
`cofferdam-core`, `cofferdam-engine`, `cofferdam-formatters`,
`cofferdam-cli`, or the baseline / severity / suppression machinery
moves.

If you find yourself editing those crates while writing an adapter,
you've found a layering violation. Stop and file a bead.

## The contract

The `Language` trait (`cofferdam-core/src/language.rs`):

```rust
pub trait Language: 'static + Send + Sync {
    type Parsed<'a>: Copy + 'a where Self: 'a;

    fn name() -> &'static str;
    fn default_extensions() -> &'static [&'static str];

    fn with_parsed<R>(
        file: &SourceFile,
        f: impl FnOnce(ParseOutcome<Self::Parsed<'_>>) -> R,
    ) -> R;
}
```

Three things to provide. The shape was chosen deliberately:

- `Parsed<'a>: Copy` — adapter returns a *borrowed* view. Two-or-three
  reference fields max. Not a self-owning AST. The TS adapter's
  `ParsedView<'a>` is `(&'a Program<'a>, &'a [Diagnostic])`, both
  references, trivially Copy. A Python adapter's `Parsed<'a>` would be
  similarly thin.
- `with_parsed` is callback-shaped. The adapter creates whatever
  arena / interner / allocator the parser needs on its stack frame,
  parses, calls `f` with the borrowed view, and the storage drops on
  return. This avoids the self-referential-struct ceremony you'd need
  for "return Parsed by value" — every parser worth using has the
  same arena-borrowed-AST shape.
- `name` and `default_extensions` are config-surface hints. Used by
  CLI flag defaults and engine logs; nothing structural depends on
  them.

## Workshop: a `cofferdam-py` skeleton

A hypothetical Python adapter, from scratch.

### Cargo.toml

```toml
[package]
name = "cofferdam-py"
version = "0.1.0"
edition = "2021"
description = "Python language adapter for the cofferdam analyzer."

[dependencies]
cofferdam-core = { path = "../cofferdam-core" }

# Parser choice — see "Picking a parser" below. rustpython-parser is
# the most likely first pick: pure Rust, mature, no Python runtime
# dependency, matches the licensing of the rest of the workspace.
rustpython-parser = "0.4"
rustpython-ast = "0.4"
```

`cofferdam-py` declares its OWN parser dep — exactly like
`cofferdam-ts` declares oxc. The workspace's `oxc_*` deps don't move;
they stay scoped to `cofferdam-ts`.

### src/lib.rs (sketch)

```rust
pub mod ast;            // PyAstView, PyAstVisitor, PyNodeKind, ...
pub mod parser;         // parse_into, ParsedPyView
pub mod line_classify;  // build_lines (string + comment + docstring walk)
pub mod prelude;

use cofferdam_core::{Language, ParseOutcome, SourceFile};
use rustpython_parser::Parse;

pub struct Python;

pub struct ParsedPyView<'a> {
    pub module: &'a rustpython_ast::Mod,
    pub diagnostics: &'a [String],
}

impl Clone for ParsedPyView<'_> { fn clone(&self) -> Self { *self } }
impl Copy for ParsedPyView<'_> {}

impl Language for Python {
    type Parsed<'a> = ParsedPyView<'a>;

    fn name() -> &'static str { "python" }
    fn default_extensions() -> &'static [&'static str] { &["py", "pyi"] }

    fn with_parsed<R>(
        file: &SourceFile,
        f: impl FnOnce(ParseOutcome<Self::Parsed<'_>>) -> R,
    ) -> R {
        // rustpython-parser owns its allocations — no arena like oxc's.
        // We build the Mod, hold it on the stack, hand `f` a view that
        // borrows from it, and drop on return.
        match Mod::parse(&file.text, &file.path.display().to_string()) {
            Ok(module) => {
                let diagnostics: Vec<String> = vec![];
                f(ParseOutcome {
                    parsed: Some(ParsedPyView { module: &module, diagnostics: &diagnostics }),
                    diagnostics,
                })
            }
            Err(err) => f(ParseOutcome {
                parsed: None,
                diagnostics: vec![format!("{err}")],
            }),
        }
    }
}

// Type aliases mirroring cofferdam-ts:
pub type CheckContext<'p, 'r> = cofferdam_core::CheckContext<'p, 'r, Python>;
pub type DynCheck = dyn cofferdam_core::Check<Python>;
pub trait PyCheck: cofferdam_core::Check<Python> {}
impl<T: cofferdam_core::Check<Python> + ?Sized> PyCheck for T {}
```

The skeleton is intentionally close to `cofferdam-ts/src/lib.rs`. The
similarity is the proof that the abstraction holds: the *language
adapter pattern* is itself a pattern, and the second adapter mostly
copies the first.

### A Python check, end-to-end

Suppose we want `Warning.NoPrintInTests` — flag `print(...)` calls in
`tests/` directories. Live in a hypothetical `cofferdam-py-checks`
crate (parallel to `cofferdam-checks`):

```rust
use cofferdam_core::{
    Category, CheckMeta, FileScope, Issue, Priority, Severity, SourceFile,
};
use cofferdam_py::{Check, CheckContext, Python};
use cofferdam_py::rustpython_ast::{Expr, Stmt};

const META: CheckMeta = CheckMeta {
    id: "Warning.NoPrintInTests",
    category: Category::Warning,
    base_priority: 5,
    default_severity: Severity::Medium,
    explanation: "Use the test runner's logging instead of print().",
    body: include_str!("../docs/Warning.NoPrintInTests.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    files: Some(&FileScope {
        extensions: &["py"],
        path_pattern: Some("**/tests/**"),
        exclude_patterns: &[],
    }),
};

pub struct NoPrintInTests;

impl Check<Python> for NoPrintInTests {
    fn meta(&self) -> &'static CheckMeta { &META }
    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_, '_>) -> Vec<Issue> {
        let Some(view) = ctx.parsed else { return Vec::new(); };
        // ... walk view.module looking for Expr::Call with name "print"
        // ... build Issue with span, message, etc.
        unimplemented!()
    }
}
```

Note what this DOESN'T touch:
- No engine changes. `Engine<Python>` constructs from a `Vec<Box<dyn Check<Python>>>` exactly like `Engine<TypeScript>`.
- No formatter changes. Issues serialise the same way regardless of language.
- No CLI changes beyond a binary that wires `Engine::<Python>::new(all_py_builtins())`. In practice that's a sibling binary `cofferdam-py-cli`, with the same flag set as `cofferdam-cli`.
- No baseline/severity/suppression changes. `// cofferdam-ignore: ...` works inside Python comments because the suppression code reads comment text, not AST.

## Picking a parser

For Python, the candidates as of 2026-05:

| Parser | License | Pros | Cons |
|---|---|---|---|
| `rustpython-parser` | MIT | Pure Rust, mature, owns the AST shape, used by RustPython itself | AST is Mod-rooted (not arena-allocated like oxc) |
| `tree-sitter-python` | MIT | Incremental reparsing, query language for plugin authors | C dependency, AST less typed than rustpython's |
| `ruff_python_parser` | MIT | Designed for linter use, error recovery | Pulls in ruff workspace, larger surface |

Recommendation for a sketch: **rustpython-parser**. Smallest dep tree,
matches the cofferdam workspace's pure-Rust posture, AST shape is
straightforward to walk. Tree-sitter would be the choice if Python
incremental reparsing matters (LSP phase), but the LSP integration is
a separate concern from a basic adapter.

For Go: `gopls` infrastructure is the obvious source but pulling it in
is heavy; a tree-sitter-go bridge or a small Go-AST parser via `go/ast`
called over CGO is more proportionate.

## What about a TypeScript-side SDK?

`@cofferdam/check-sdk` (TS) is the plugin-author npm package that
mirrors `cofferdam-ts`'s AST shape. A Python adapter would need its
own equivalent — `@cofferdam/check-sdk-py`? — for plugins authored in
TypeScript that target the Python adapter. **Probably out of scope
for the first language adapter**: writing Python checks in TypeScript
through the napi loader is an unusual ask; most teams would write the
checks in Rust (as built-ins) or Python directly through whatever
plugin loader the Python adapter exposes. File a follow-up bead if
TS-authored Python checks become a real ask.

## Open architectural questions the doc surfaces

Walking through the sketch flushes these out — they're listed in
`design/platform-extensibility.md` as "decisions still open" but the
adapter exercise makes them concrete:

1. **Encoding / BOM / line endings.** Python source has UTF-8-with-BOM
   stragglers and CRLF on Windows-authored files. The `LineView` line
   table in `cofferdam-core` already handles CRLF stripping; BOM is
   not yet special-cased. A Python adapter would either need to strip
   it before `with_parsed` or `cofferdam-core` would need a
   normalisation pre-pass. Recommendation: pull the normalisation into
   core (every text language needs it, every adapter would otherwise
   duplicate it).

2. **Diagnostic span shape.** Adapter-returned `ParseOutcome.diagnostics`
   is `Vec<String>` today. For better error UX, a future adapter may
   want to surface `(message, span)` pairs. Bumping
   `ParseOutcome.diagnostics` to `Vec<(String, Option<Span>)>` is a
   compatible widening — file when the second adapter actually wants
   it.

3. **Type-aware checks.** `CheckMeta::requires_types` is a TS-shaped
   concept (routes to ts-morph in phase 5). For Python, `requires_types`
   would mean mypy/pyright integration. The flag stays on `CheckMeta`
   in core (it's a hint, not a contract), but each adapter decides what
   the flag implies. Probably a per-adapter doc note rather than a
   structural change.

4. **Plugin SDK shape.** `@cofferdam/check-sdk` (cd-81a.8) is
   TS-shaped. If Python plugins matter, a sibling `cofferdam-py-sdk`
   on PyPI (or whatever) — but again, that's a follow-up epic.

## What stays exactly the same

For confidence, the list of platform pieces a language adapter does
NOT touch:

- `cofferdam-core` — every type here is language-agnostic by
  construction, enforced by `scripts/platform-purity.mjs`.
- `cofferdam-engine` — `Engine<L: Language>` works for any `L`.
- `cofferdam-formatters` — text/json output of `Issue` is identical
  across languages.
- Baseline (`cofferdam-engine::baseline`), severity post-pass, suppression
  parsing, file scoping, the corpus-index two-pass model — all
  language-agnostic.
- `cofferdam.toml` config schema — per-check options, severity
  overrides, are keyed by `check_id` (a string), not language.
- `--fail-on`, `--since`, `--max-issues`, `--quiet`, `NO_COLOR`,
  `.cofferdamignore` — pure CLI ergonomics, language-blind.

If a Python adapter exercise demands changes here, it means the
abstraction needs to widen. That's a useful signal.

## Status

This doc is a sketch, not a recipe. No `cofferdam-py` crate exists.
File a bead under cd-81a (or successor epic) if you want to implement
one, and use this doc as the design.
