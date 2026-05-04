//! `cofferdam-ts` — TypeScript language adapter for the cofferdam
//! analyzer.
//!
//! Wraps the oxc parser and the AST surface that built-in checks
//! (and, via `cofferdam-napi`, plugin checks) operate against. Pairs
//! with `cofferdam-core` (the platform crate) to give the TS-flavored
//! `Check` trait + `CheckContext` that built-ins implement.
//!
//! See `design/platform-extensibility.md` for why the split exists:
//! every type with an oxc dependency lives here so a future Python or
//! Go adapter can slot in alongside without touching `cofferdam-core`,
//! `cofferdam-engine`, or `cofferdam-formatters`.

pub mod ast;
pub mod check;
pub mod line_classify;
pub mod parser;
pub mod prelude;

pub use ast::{AstView, AstVisitor, NodeKind, NodeRef, Walk};
pub use check::{Check, CheckContext};
pub use line_classify::build_lines;
pub use parser::{diagnostic_messages, parse_fatal, parse_into, source_type_for, ParsedView};

// Re-export the oxc bits that plugin / built-in check authors will
// reach for. Going through `cofferdam_ts::oxc_*` (rather than depending
// on `oxc_*` directly) is what keeps the "no oxc outside the adapter"
// CI guardrail meaningful — only this crate is allowed to import oxc.
pub use oxc_allocator;
pub use oxc_allocator::Allocator;
pub use oxc_ast;
pub use oxc_ast_visit;
pub use oxc_diagnostics;
pub use oxc_parser;
pub use oxc_semantic;
pub use oxc_span;
pub use oxc_syntax;
