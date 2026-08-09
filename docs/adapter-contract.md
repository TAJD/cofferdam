# Adapter contract

## Purpose

`cofferdam_core::adapter::Adapter` is a small, descriptive trait that lets a
language pipeline declare its own identity: which `Language` it produces
facts for, which files it claims, and which schema namespace its
corpus/graph facts live under. It exists to formalize — as a type, not just
prose in a module doc — the shape cofferdam's polylingual architecture
already has (TypeScript via `oxc`, Rust and HTML via `tree-sitter`), ahead
of a larger refactor that would route dispatch through it.

This is a deliberately narrow slice. It adds the trait and three thin
implementations; it does not change how the engine parses or dispatches
files today.

## The trait

```rust
pub trait Adapter {
    fn language(&self) -> Language;
    fn file_globs(&self) -> &[&str];
    fn schema_namespace(&self) -> &str;
}
```

- **`language(&self) -> Language`** — the `Language` variant
  (`cofferdam_core::source::Language`) this adapter produces facts for.
- **`file_globs(&self) -> &[&str]`** — the glob patterns / extensions this
  adapter claims, mirroring what `Language::from_path` already encodes for
  that language.
- **`schema_namespace(&self) -> &str`** — the schema-extension namespace
  this adapter's corpus/graph facts live under (e.g. `"ts"`, `"rust"`).

## The one-way contract (documented, not enforced)

The intended relationship between adapters and checks is one-way: adapters
produce facts from source; they do not see rule configuration, and they do
not construct `Issue`s directly (that stays the `Check` trait's job).

**This iteration does not enforce that contract.** There is no runtime
check that stops an `Adapter` impl from reading rule config or building an
`Issue`, and no test that exercises a deliberate violation. The trait is a
type-level description of intent, not a sandbox.

## What implements it today

- **`TypeScriptAdapter`** (`crates/cofferdam-core/src/adapter.rs`) —
  describes the existing oxc-based TS/JS pipeline. `language()` returns
  `Language::TypeScript`; `file_globs()` returns
  `["*.ts", "*.tsx", "*.js", "*.jsx", "*.cjs", "*.mjs", "*.mts", "*.cts"]`,
  matching `Language::from_path`'s extension list; `schema_namespace()`
  returns `"ts"`.
- **`RustAdapter`** (`crates/cofferdam-rust/src/lib.rs`) — describes the
  existing tree-sitter-based Rust pipeline. `language()` returns
  `Language::Rust`; `file_globs()` returns `["*.rs"]`; `schema_namespace()`
  returns `"rust"`.
- **`HtmlAdapter`** (`crates/cofferdam-html/src/lib.rs`) — describes the
  tree-sitter-based HTML pipeline behind [`cofferdam verify --dist`](/verify-dist).
  `language()` returns `Language::Html`; `file_globs()` returns
  `["*.html", "*.htm"]`; `schema_namespace()` returns `"html"`.

Both impls are additive metadata: they describe existing behaviour without
changing it. Neither `cofferdam-engine`'s discovery loop nor its parse/
dispatch loop routes through these impls yet — file-extension dispatch
still happens via `Language::from_path` exactly as before.

## Not yet built

The following are explicitly deferred, not implied by this change:

- **Runtime forbidden-surface enforcement.** No mechanism stops an adapter
  from reading rule config or emitting `Issue`s directly, and there is no
  deliberate-violation fixture test.
- **`[adapters]` registration in `cofferdam.toml`.** Adapters are not
  configurable or discoverable via config today.
- **Adapter identity scheme for non-byte-offset findings.** Tracked
  separately; not addressed here.
- **An adapter for a language with a different fact model** — SQL
  migrations, say. HTML made three, but all three are file-and-span
  languages; nothing yet proves the shape holds for a domain that isn't.
- **Engine rewiring.** `cofferdam-engine`'s discovery and parse/dispatch
  loops are untouched; they do not consult `Adapter` impls at runtime.
