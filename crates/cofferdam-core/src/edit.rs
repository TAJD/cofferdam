//! Autofix edit types.
//!
//! A `TextEdit` describes a single in-place text replacement: a byte range
//! (expressed as a `Span`) and the replacement string. The autofix engine
//! applies edits in **reverse byte-offset order** so earlier edits don't
//! shift the byte positions of later edits.

use serde::{Deserialize, Serialize};

use crate::issue::Span;

/// A single text edit: replace the bytes `span.start_byte..span.end_byte`
/// in the source file with `replacement`.
///
/// Edits must be non-overlapping and are applied in reverse offset order by
/// the fix engine so earlier replacements don't invalidate later spans.
///
/// `Serialize`/`Deserialize` are derived so plugins crossing the napi
/// boundary can carry a fix payload back from `ctx.report` — see
/// `Issue.fix` (cd-81a.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    /// The byte range to replace. Matches `Span` so callers can pass an
    /// `Issue.span` directly when the issue's span already targets exactly the
    /// text to replace.
    pub span: Span,
    /// The replacement text. May be longer or shorter than the removed range.
    pub replacement: String,
}
