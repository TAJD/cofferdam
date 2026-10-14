//! Source-file representation passed to checks.
//!
//! Phase 0 holds raw text plus a path. Once oxc lands (phase 1+), this struct
//! grows an AST handle and a token map. Checks that only need lines operate
//! on `text` directly without paying the parse cost — the engine decides
//! per-check whether to parse.

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
    /// Astro SFCs — not parsed as a whole file. The engine only pulls
    /// import edges out of the frontmatter fence (cd-45); no built-in
    /// check declares this language today.
    Astro,
    /// HTML — handled by tree-sitter-html via `cofferdam-html` (CD-83).
    Html,
    /// Markdown / MDX — not parsed as a whole file (CD-68). Text-corpus
    /// content (blog posts, docs) reaches plugin checks as raw
    /// `file.text` + `LineView`s with no AST (`ast: null` on the wire);
    /// no built-in check declares this language today (CD-357 removed the
    /// last one, `Consistency.SpellingDialect`) — plugin checks can still
    /// opt in. Discovery only walks `.md`/`.mdx` files when a project opts
    /// in via `cofferdam.toml` `[engine] extra_extensions`.
    Markdown,
}

impl Language {
    /// Best-effort language guess from a file's path extension.
    /// Anything we don't recognise falls back to `TypeScript` so existing
    /// callers (and existing fixtures) keep their current behaviour.
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => match ext.to_ascii_lowercase().as_str() {
                "rs" => Language::Rust,
                "astro" => Language::Astro,
                "html" | "htm" => Language::Html,
                "md" | "mdx" => Language::Markdown,
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
    /// grapheme- or column-aware widths for line-length checks to handle tabs
    /// and wide chars correctly.
    pub fn lines(&self) -> impl Iterator<Item = (u32, &str)> {
        self.text
            .split('\n')
            .enumerate()
            .map(|(i, l)| (i as u32 + 1, l.trim_end_matches('\r')))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_path_maps_markdown_extensions() {
        assert_eq!(Language::from_path(Path::new("a.md")), Language::Markdown);
        assert_eq!(Language::from_path(Path::new("a.mdx")), Language::Markdown);
        assert_eq!(Language::from_path(Path::new("a.MD")), Language::Markdown);
    }

    #[test]
    fn from_path_other_extensions_unaffected() {
        assert_eq!(Language::from_path(Path::new("a.ts")), Language::TypeScript);
        assert_eq!(Language::from_path(Path::new("a.rs")), Language::Rust);
        assert_eq!(Language::from_path(Path::new("a.astro")), Language::Astro);
        assert_eq!(Language::from_path(Path::new("a.html")), Language::Html);
    }
}
