//! HTML source-code adapter for cofferdam (CD-83).
//!
//! Mirrors `cofferdam-rust`'s shape: this crate targets HTML via
//! `tree-sitter` + `tree-sitter-html` instead of `oxc`. It shares the
//! same downstream abstractions — `Check`, `Issue`, `Span`, `CheckMeta`
//! — and differs only in the parser layer and the per-language checks.

pub mod checks;
pub mod parser;

pub use checks::all_html_checks;
pub use parser::{parse_html, HtmlParseError, HtmlParseTree};

use cofferdam_core::{Adapter, Language};

/// Identifier the engine uses to route per-file dispatch (CD-83).
pub const LANGUAGE_TAG: &str = "html";

/// Adapter impl describing this crate's HTML pipeline (tree-sitter-based).
/// See `cofferdam_core::adapter` for the (unenforced) one-way contract this
/// documents.
pub struct HtmlAdapter;

impl Adapter for HtmlAdapter {
    fn language(&self) -> Language {
        Language::Html
    }

    fn file_globs(&self) -> &[&str] {
        &["*.html", "*.htm"]
    }

    fn schema_namespace(&self) -> &str {
        "html"
    }
}

#[cfg(test)]
mod adapter_tests {
    use super::*;

    #[test]
    fn html_adapter_language() {
        assert_eq!(HtmlAdapter.language(), Language::Html);
    }

    #[test]
    fn html_adapter_file_globs() {
        assert_eq!(HtmlAdapter.file_globs(), &["*.html", "*.htm"]);
    }

    #[test]
    fn html_adapter_schema_namespace() {
        assert_eq!(HtmlAdapter.schema_namespace(), "html");
    }
}
