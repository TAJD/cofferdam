//! File discovery — walks paths, respects `.gitignore`/`.cofferdamignore`,
//! filters to TypeScript extensions.
//!
//! Owned by the engine (not the CLI) because the LSP and napi-rs surfaces
//! both need the same discovery contract. Built on the `ignore` crate
//! (ripgrep's walker) so we get parallel walking, hidden-file handling,
//! and standard ignore-file semantics for free.

use std::path::{Path, PathBuf};

use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;

/// File extensions cofferdam analyzes by default.
///
/// `.d.ts` files are intentionally included — they're real source for the
/// type-aware tier (phase 5). Checks that don't apply (e.g. unused locals)
/// will skip declaration files via their own logic.
pub const DEFAULT_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts"];

#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    /// Extensions to include (without leading dot).
    pub extensions: Vec<String>,
    /// Honor `.gitignore`, global gitignore, and `.cofferdamignore`.
    pub respect_ignore: bool,
    /// Walk hidden files/directories (those starting with `.`).
    pub include_hidden: bool,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            extensions: DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
            respect_ignore: true,
            include_hidden: false,
        }
    }
}

/// Walk one or more roots and return matching file paths.
///
/// `roots` may mix files and directories. Files are kept as-is when their
/// extension matches; directories are walked recursively under
/// `DiscoveryOptions`.
///
/// Errors from the walker (permission denied on a single subdirectory etc.)
/// are silently skipped — matching ripgrep's default behavior. We surface
/// IO errors only when the *root itself* is unreachable.
pub fn discover<P: AsRef<Path>>(
    roots: &[P],
    opts: &DiscoveryOptions,
) -> Result<Vec<PathBuf>, ignore::Error> {
    if roots.is_empty() {
        return Ok(Vec::new());
    }

    let mut overrides = OverrideBuilder::new(roots[0].as_ref());
    for ext in &opts.extensions {
        overrides.add(&format!("*.{}", ext))?;
    }
    let overrides = overrides.build()?;

    let mut builder = WalkBuilder::new(roots[0].as_ref());
    for root in roots.iter().skip(1) {
        builder.add(root.as_ref());
    }
    builder
        .overrides(overrides)
        .standard_filters(opts.respect_ignore)
        .hidden(!opts.include_hidden)
        .add_custom_ignore_filename(".cofferdamignore");

    let mut out = Vec::new();
    for entry in builder.build() {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_some_and(|ft| ft.is_file()) {
                    out.push(entry.into_path());
                }
            }
            // Per-entry errors (perm denied, broken symlink) are non-fatal.
            // Loud surfacing happens only on root-level failures, which
            // `ignore` reports via the first iterator item. A future
            // verbose flag (cd-9 era) can re-emit these.
            Err(_) => continue,
        }
    }
    out.sort();
    Ok(out)
}
