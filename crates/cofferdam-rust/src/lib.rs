//! Rust source-code adapter for cofferdam (cd-91zc).
//!
//! This crate is the load-bearing demonstration of cofferdam's polylingual
//! architecture. Where `cofferdam-engine` + the existing built-ins target
//! TypeScript via `oxc`, this crate targets Rust via `tree-sitter` +
//! `tree-sitter-rust`. The two adapters share the same downstream
//! abstractions — `Check`, `Issue`, `Span`, `CorpusIndex`, `CheckMeta` —
//! and differ only in the parser layer and the per-language checks.
//!
//! ## Status
//!
//! **Phase 0 — pre-canonical-graph.** This crate operates against the
//! flat-corpus shape that the TS adapter uses today. When cd-9hp.9
//! (canonical graph) ships, the Rust adapter migrates to writing
//! `Extension { ns: "rust", kind: ... }` nodes/edges through that
//! substrate instead. Cross-language predicates (DSL queries that span
//! TS and Rust) light up at the same time.
//!
//! ## Layout (planned, ships per checkpoint)
//!
//! ```text
//! crates/cofferdam-rust/
//!   src/
//!     lib.rs                  — this file; crate root, public surface
//!     parser.rs               — tree-sitter-rust → cofferdam Span / AstView
//!     checks/
//!       mod.rs                — all_rust_checks() exporter
//!       no_unwrap_in_lib.rs   — Rust.NoUnwrapInLib
//!       no_unimplemented.rs   — Rust.NoUnimplementedInNonTest
//!       missing_pub_doc.rs    — Rust.MissingPubDoc
//!   tests/
//!     fixtures/               — spec_contract-style per-check fixtures
//! ```
//!
//! ## Engine integration (planned)
//!
//! `cofferdam-engine` gains a `Language` enum on `SourceFile`
//! (`Ts | Rust`) and per-file dispatch routes to the right check set:
//!
//! ```ignore
//! match file.language {
//!     Language::Ts => for check in self.ts_checks { ... }
//!     Language::Rust => for check in self.rust_checks { ... }
//! }
//! ```
//!
//! The check trait stays domain-agnostic — `Check::run(file, ctx)` just
//! sees a `SourceFile`. The parser is what differs.
//!
//! ## Checkpoint plan
//!
//! Each checkpoint ships as a self-contained PR. The bead `--design`
//! field carries the live status.
//!
//! 1. **Crate skeleton + tree-sitter integration**. This file. Cargo.toml
//!    with tree-sitter pins. Empty stubs. Verifies the parser links.
//! 2. **Parser → Span layer**. Convert tree-sitter nodes into cofferdam
//!    `Span` (byte offsets + line/column). Round-trip tests.
//! 3. **First check: `Rust.NoUnwrapInLib`**. Walk the tree-sitter syntax
//!    tree for `unwrap()` / `expect()` call expressions; flag the ones
//!    that live outside `#[cfg(test)]` / `#[test]` / `mod tests`.
//! 4. **Engine wiring**. `Language` enum on `SourceFile`; dispatch
//!    routing. `cofferdam check crates/` works end-to-end.
//! 5. **Two more checks: `NoUnimplementedInNonTest`, `MissingPubDoc`**.
//!    Validates the path generalises beyond one check.
//! 6. **CI dogfood**. `crates/`'s baseline shipped; CI gate parallel to
//!    `cd-9tq`'s TS dogfood.
//!
//! Phase 1 (post-cd-9hp.9): migrate to canonical graph.
//! Phase 2 (post-cd-9hp.10): formalise the adapter contract.

// Re-export the parser surface as it lands. Today: nothing — the
// `parser` module is checkpoint 2.
//
// pub use parser::{parse_rust_file, RustParseTree};
// pub use checks::all_rust_checks;

/// Identifier the engine uses to route per-file dispatch (cd-91zc).
/// Lifts to a `Language` enum on `SourceFile` in checkpoint 4.
pub const LANGUAGE_TAG: &str = "rust";

#[cfg(test)]
mod tests {
    /// Smoke test: confirm tree-sitter-rust links and parses a trivial
    /// source. This is the only test that should exist in the crate
    /// skeleton; everything else lands per-checkpoint with a real
    /// fixture suite.
    #[test]
    fn tree_sitter_rust_parses_trivial_source() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .expect("load tree-sitter-rust");
        let tree = parser
            .parse("fn main() {}", None)
            .expect("parse trivial source");
        let root = tree.root_node();
        assert_eq!(root.kind(), "source_file");
        assert!(
            root.child_count() > 0,
            "expected at least one top-level item"
        );
    }
}
