//! `cofferdam-core` — shared types and traits for the cofferdam analyzer.
//!
//! This crate is intentionally dependency-light. Engine, checks, formatters,
//! CLI, LSP, and the napi FFI surface all depend on it; it depends on none of
//! them. Adding a heavy dep here ripples through the whole workspace.

pub mod check;
pub mod issue;
pub mod parser;
pub mod source;
pub mod span_util;

pub use check::{Category, Check, CheckContext, CheckMeta};
pub use issue::{Issue, Priority, Severity, Span};
pub use parser::{parse_into, source_type_for, ParsedView};
pub use source::SourceFile;
pub use span_util::span_from_bytes;

// Re-export the oxc bits checks will commonly reach for, so plugin and
// built-in check authors don't all add direct oxc deps.
pub use oxc_allocator::Allocator;
pub use oxc_ast as ast;
pub use oxc_span;
