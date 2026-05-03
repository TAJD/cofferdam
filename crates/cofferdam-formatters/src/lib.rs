//! `cofferdam-formatters` — pluggable report renderers.
//!
//! Phase 0 ships the human text formatter. JSON, SARIF, and GitHub
//! annotation formatters land in phase 6 once the CI ergonomics package
//! becomes the focus.

pub mod compact;
pub mod json;
pub mod text;

pub use compact::{CompactFormatter, COMPACT_HEADER};
pub use json::{JsonFormatter, JsonRenderOpts};
pub use text::{TextFormatter, TextRenderOpts};
