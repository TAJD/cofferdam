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

pub mod parser;

// Re-export the parser surface. Checks come in checkpoints 3 and 5.
//
// pub use checks::all_rust_checks;
pub use parser::{parse_rust, RustParseError, RustParseTree};

/// Identifier the engine uses to route per-file dispatch (cd-91zc).
/// Lifts to a `Language` enum on `SourceFile` in checkpoint 4.
pub const LANGUAGE_TAG: &str = "rust";
