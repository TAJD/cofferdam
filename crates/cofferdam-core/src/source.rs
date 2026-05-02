//! Source-file representation passed to checks.
//!
//! Phase 0 holds raw text plus a path. Once oxc lands (phase 1+), this struct
//! grows an AST handle and a token map. Checks that only need lines (e.g.
//! `Readability.MaxLineLength`) operate on `text` directly without paying the
//! parse cost — the engine decides per-check whether to parse.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
}

impl SourceFile {
    pub fn new(path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            text: text.into(),
        }
    }

    /// Iterate lines with their 1-based line number.
    ///
    /// Uses byte length per line; once we have a real AST we'll switch to
    /// grapheme- or column-aware widths for `MaxLineLength` to handle tabs and
    /// wide chars correctly.
    pub fn lines(&self) -> impl Iterator<Item = (u32, &str)> {
        self.text
            .split('\n')
            .enumerate()
            .map(|(i, l)| (i as u32 + 1, l.trim_end_matches('\r')))
    }
}
