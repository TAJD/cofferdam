//! `cofferdam-core` — language-agnostic types and traits for the
//! cofferdam analyzer.
//!
//! This crate is the **platform** layer (per `design/platform-extensibility.md`):
//! every other crate in the workspace depends on it; it depends on no
//! language-specific machinery. Adding a heavy or language-bound dep
//! here ripples through the whole workspace and erodes the split that
//! lets future adapters (Python, Go) slot in alongside `cofferdam-ts`.
//!
//! What lives here:
//! - The taxonomy types (`Category`, `Severity`, `Priority`).
//! - The data shape of a finding (`Issue`, `Span`, `RelatedSpan`,
//!   `TextEdit`).
//! - Check metadata (`CheckMeta`, `FileScope`).
//! - Configuration plumbing (`OptionSpec`, `OptionValue`, `CheckOptions`).
//! - The cross-file corpus (`CorpusIndex`, `CorpusKey`,
//!   `FinalizeContext`).
//! - `SourceFile`, the byte-offset → line/column helper
//!   (`span_from_bytes`), and the `LineView` data shape.
//!
//! What lives in language-adapter crates (e.g. `cofferdam-ts`):
//! - The `Check` trait and the `CheckContext` flavor that carries the
//!   parsed AST.
//! - The parser invocation and any AST surface.
//! - The classification pass that populates `LineView` flags.

pub mod check;
pub mod corpus;
pub mod edit;
pub mod issue;
pub mod language;
pub mod lines;
pub mod options;
pub mod source;
pub mod span_util;

pub use check::{Category, Check, CheckContext, CheckMeta, FileScope, FinalizeContext};
pub use corpus::{CorpusIndex, CorpusKey};
pub use edit::TextEdit;
pub use issue::{Issue, ParseSeverityError, Priority, RelatedSpan, Severity, Span};
pub use language::{Language, ParseOutcome};
pub use lines::{apply_byte_range, byte_to_line, compute_line_starts, LineFlags, LineView, Lines};
pub use options::{
    validate_options, CheckOptions, OptionDefault, OptionKind, OptionSpec, OptionValue,
    OptionsError, RawOptionValue, EMPTY_OPTIONS,
};
pub use source::SourceFile;
pub use span_util::span_from_bytes;
