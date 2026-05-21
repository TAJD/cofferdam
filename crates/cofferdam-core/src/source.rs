//! Source-file representation passed to checks.
//!
//! Phase 0 holds raw text plus a path. Once oxc lands (phase 1+), this struct
//! grows an AST handle and a token map. Checks that only need lines (e.g.
//! `Readability.MaxLineLength`) operate on `text` directly without paying the
//! parse cost — the engine decides per-check whether to parse.

use std::path::{Path, PathBuf};

/// Source language a `SourceFile` carries — used by the engine to route
/// per-file dispatch to the right parser and check set (cd-91zc checkpoint 4).
///
/// Detection is path-extension based at construction time. New languages
/// extend this enum; the engine's per-file loop and `Check::language()`
/// default both gate on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// TypeScript / TSX / JS / JSX / MJS / CJS — handled by oxc.
    TypeScript,
    /// Rust — handled by tree-sitter-rust via `cofferdam-rust` (cd-91zc).
    Rust,
}

impl Language {
    /// Best-effort language guess from a file's path extension.
    /// Anything we don't recognise falls back to `TypeScript` so existing
    /// callers (and existing fixtures) keep their current behaviour.
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => match ext.to_ascii_lowercase().as_str() {
                "rs" => Language::Rust,
                "ts" | "tsx" | "js" | "jsx" | "cjs" | "mjs" | "mts" | "cts" => Language::TypeScript,
                _ => Language::TypeScript,
            },
            None => Language::TypeScript,
        }
    }
}

/// One source file under analysis. Owns its text so checks can slice
/// arbitrary byte ranges without lifetime gymnastics. The `language`
/// field is derived from the file extension at construction time and
/// drives per-language dispatch in the engine.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: PathBuf,
    pub text: String,
    /// Source language, derived from `path` at construction time. The
    /// engine reads this to skip oxc parsing for non-TS files and to
    /// route per-file dispatch to the matching check set.
    pub language: Language,
}

impl SourceFile {
    /// Construct a `SourceFile`. Derives `language` from `path`'s
    /// extension via `Language::from_path`.
    pub fn new(path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        let path = path.into();
        let language = Language::from_path(&path);
        Self {
            path,
            text: text.into(),
            language,
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
