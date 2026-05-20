# cd-91zc — Decision: phase 0 against flat-corpus shape (signed off 2026-05-20)

User direction: dogfood cofferdam on its own Rust codebase. Two strategic payoffs — polylingual demonstration of the architecture, and concrete pressure on cd-9hp.9 (canonical graph) + cd-9hp.10 (adapter contract). Both briefs were designed in vacuum; the Rust adapter is the load-bearing user that informs their final shape.

`cd-9hp.10` now `depends on cd-91zc` — the adapter contract crystallises from this work.

## Checkpoint 1: crate skeleton + tree-sitter linkage (shipped 2026-05-20)

- `crates/cofferdam-rust/` exists, added to workspace `members`.
- `Cargo.toml` pins `tree-sitter = "0.23"` and `tree-sitter-rust = "0.23"`.
- `src/lib.rs` documents the planned module layout and ships a smoke test confirming tree-sitter-rust parses trivial source. Nothing more.
- `cargo build -p cofferdam-rust` and `cargo test -p cofferdam-rust` both pass.

## Kickoff prompt for fresh session — checkpoint 2 (parser → Span layer)

Copy-paste verbatim into a fresh Claude Code session:

---

Implement the tree-sitter-rust → cofferdam-core conversion layer for cd-91zc phase 0. The crate skeleton (already committed) pins tree-sitter and ships a smoke test. This is checkpoint 2 of 6.

Scope:

1. Create `crates/cofferdam-rust/src/parser.rs`. Public surface:

   ```rust
   pub fn parse_rust(text: &str) -> Result<RustParseTree, RustParseError>;

   pub struct RustParseTree {
       tree: tree_sitter::Tree,
       text: String,
   }

   impl RustParseTree {
       pub fn root_node(&self) -> tree_sitter::Node<'_>;
       pub fn span_of(&self, node: tree_sitter::Node<'_>) -> cofferdam_core::Span;
       pub fn text_of(&self, node: tree_sitter::Node<'_>) -> &str;
   }
   ```

   `span_of` converts a tree-sitter node's byte-range into a cofferdam `Span` (byte offsets + 1-based line/column). Reuse `cofferdam_core::span_from_bytes` if it exists; otherwise add a thin wrapper.

2. Round-trip tests covering:
   - Trivial source: `fn main() {}` — the `function_item` node's span matches the source bytes exactly.
   - UTF-8 sanity: a source with non-ASCII identifiers — line/column counts match what `span_from_bytes` produces.
   - Multi-line source: nested `mod`s with whitespace; per-node spans round-trip.
   - Error recovery: malformed input yields a `RustParseError` carrying the tree-sitter error-node locations so downstream check authors can skip them.

3. NO checks in this checkpoint. Just the parser surface.

Do NOT in this checkpoint:
- Implement any rule. Checks land in checkpoint 3 (`Rust.NoUnwrapInLib`).
- Wire into `cofferdam-engine`. Engine integration is checkpoint 4.
- Add the `Language` enum to `SourceFile`. That's checkpoint 4 too.

Implementation notes:
- `tree_sitter::Node::start_byte()` / `end_byte()` give the byte range; `start_position()` / `end_position()` give zero-based row/col. Convert to cofferdam's 1-based line/column at the boundary.
- Tree-sitter is C-backed via `cc`; first build downloads ~500KB of generated bindings. Subsequent builds are cached.
- `insta` is in `dev-dependencies` for golden-file tests — use `insta::assert_yaml_snapshot!(tree.root_node().to_sexp())` for the round-trip golden form.

Verification block: `cargo build -p cofferdam-rust`, `cargo test -p cofferdam-rust`, `cargo clippy -p cofferdam-rust --all-targets -- -D warnings`, `cargo fmt --check`.

Commit message: `feat(cofferdam-rust): tree-sitter to Span layer (cd-91zc checkpoint 2)`.

---

## Remaining checkpoints

3. **`Rust.NoUnwrapInLib`** — first concrete check. Walks the parse tree for `unwrap()` / `expect()` call expressions; flags those outside `#[cfg(test)]` / `#[test]` / `mod tests`. Fixture suite under `crates/cofferdam-rust/tests/fixtures/`.
4. **Engine wiring** — `Language` enum on `SourceFile`; per-file dispatch routes to Rust checks when `Language::Rust`. `cofferdam check crates/` works end-to-end.
5. **Two more checks** — `Rust.NoUnimplementedInNonTest`, `Rust.MissingPubDoc`. Validates the path generalises.
6. **CI dogfood** — `crates/`'s baseline shipped; CI gate parallel to cd-9tq's TS dogfood.

Phase 1 (post-cd-9hp.9): migrate to the canonical graph.
Phase 2 (post-cd-9hp.10): formalise the adapter contract.
