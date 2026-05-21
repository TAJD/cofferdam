//! `cofferdam-core` — shared types and traits for the cofferdam analyzer.
//!
//! This crate is intentionally dependency-light. Engine, checks, formatters,
//! CLI, LSP, and the napi FFI surface all depend on it; it depends on none of
//! them. Adding a heavy dep here ripples through the whole workspace.

pub mod ast;
pub mod check;
pub mod corpus;
pub mod dsl;
pub mod edit;
pub mod graph;
pub mod invariants;
pub mod issue;
pub mod layers;
pub mod lines;
pub mod options;
pub mod parser;
pub mod source;
pub mod span_util;

pub use ast::{AstView, AstVisitor, NodeKind, NodeRef, Walk};
pub use check::{
    is_finalize_observer, Category, Check, CheckContext, CheckMeta, FinalizeContext,
    FINALIZE_OBSERVER_CHECK_IDS,
};
pub use corpus::{CorpusError, CorpusIndex, CorpusKey};
pub use edit::TextEdit;
pub use graph::{
    ExportKind, ExportRecord, ImportKind, ImportRecord, ImportedName, InvariantsRuntime,
    LayersConfig, ALL_PRE_FILTER_FINDINGS, EXPORTS, IMPORTS, INVARIANTS, LAYERS,
    REGISTERED_CHECK_IDS,
};
pub use invariants::{
    BoundarySpec, InvariantSpec, InvariantsSpec, PublicApiSpec, ScriptedInvariantSpec,
};
pub use issue::{Issue, ParseSeverityError, Priority, RelatedSpan, Severity, Span};
pub use lines::{LineView, Lines};
pub use options::{
    validate_options, CheckOptions, OptionDefault, OptionKind, OptionSpec, OptionValue,
    OptionsError, RawOptionValue, EMPTY_OPTIONS,
};
pub use parser::{parse_into, source_type_for, ParsedView};
pub use source::{Language, SourceFile};
pub use span_util::span_from_bytes;

// Re-export the oxc bits checks will commonly reach for, so plugin and
// built-in check authors don't all add direct oxc deps.
pub use oxc_allocator::Allocator;
pub use oxc_ast;
pub use oxc_span;
