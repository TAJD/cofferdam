//! `cofferdam-formatters` — pluggable report renderers.
//!
//! Phase 0 ships the human text formatter. JSON, SARIF, and GitHub
//! annotation formatters land in phase 6 once the CI ergonomics package
//! becomes the focus.

pub mod json;
pub mod text;

pub use json::JsonFormatter;
pub use text::TextFormatter;
