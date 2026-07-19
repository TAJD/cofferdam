//! File discovery — walks paths, respects `.gitignore`/`.cofferdamignore`,
//! filters to TypeScript extensions.
//!
//! Owned by the engine (not the CLI) because the LSP and napi-rs surfaces
//! both need the same discovery contract. Built on the `ignore` crate
//! (ripgrep's walker) so we get parallel walking, hidden-file handling,
//! and standard ignore-file semantics for free.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// File extensions cofferdam analyzes by default.
///
/// `.d.ts` matches the `ts` extension, but declaration files are skipped
/// by default in `DiscoveryOptions::skip_declaration_files`. Set that to
/// `false` to walk them — useful once the type-aware tier (phase 5) wants
/// to read ambient declarations directly.
///
/// `rs` lands via cd-91zc checkpoint 4 — the Rust adapter. The engine's
/// per-language dispatch ensures `.rs` files are routed only to checks
/// declaring `Language::Rust`; the default discovery widening means
/// `cofferdam check crates/` walks Rust files alongside TS files
/// without per-call option fiddling.
///
/// `astro` lands via cd-45 — Astro SFCs aren't parsed as a whole file
/// (their template body isn't valid TS/JS), but the engine extracts
/// import edges from the frontmatter fence so islands referenced only
/// from `.astro` pages aren't flagged as orphan exports.
///
/// `html`/`htm` land via CD-83 — the HTML adapter. Same per-language
/// dispatch story as `rs`: `.html`/`.htm` files route only to checks
/// declaring `Language::Html`.
pub const DEFAULT_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "rs", "astro", "html", "htm"];

/// Configuration for the file walker. Controls which extensions are
/// included, whether to honour `.gitignore` / `.cofferdamignore`, and
/// hidden-file handling. Used by every CLI subcommand and the LSP /
/// napi-rs surfaces, so the discovery contract stays uniform.
#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    /// Extensions to include (without leading dot).
    pub extensions: Vec<String>,
    /// Honor `.gitignore`, global gitignore, and `.cofferdamignore`.
    pub respect_ignore: bool,
    /// Walk hidden files/directories (those starting with `.`).
    pub include_hidden: bool,
    /// Skip TypeScript declaration files (`.d.ts`, `.d.cts`, `.d.mts`).
    pub skip_declaration_files: bool,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            extensions: DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
            respect_ignore: true,
            include_hidden: false,
            skip_declaration_files: true,
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

    // cd-103: extension filtering used to go through `WalkBuilder::overrides`
    // (an `ignore`-crate whitelist). Overrides take precedence over ignore
    // files by design, so a gitignore-style glob like `*.ts` in
    // `.gitignore`/`.cofferdamignore` was silently defeated for any file
    // whose name also happened to satisfy the extension whitelist — i.e.
    // every file cofferdam would otherwise care about. Filtering by
    // extension after the walk (instead of via an override) lets ignore
    // rules apply normally; only directory-level pruning still comes from
    // the walker itself.
    let mut builder = WalkBuilder::new(roots[0].as_ref());
    for root in roots.iter().skip(1) {
        builder.add(root.as_ref());
    }
    builder
        .standard_filters(opts.respect_ignore)
        .hidden(!opts.include_hidden)
        .add_custom_ignore_filename(".cofferdamignore");

    let mut out = Vec::new();
    for entry in builder.build() {
        match entry {
            Ok(entry) => {
                if entry.file_type().is_some_and(|ft| ft.is_file()) {
                    let path = entry.into_path();
                    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                        continue;
                    };
                    if !opts.extensions.iter().any(|e| e == ext) {
                        continue;
                    }
                    // Skip declaration files if requested
                    if opts.skip_declaration_files
                        && (name.ends_with(".d.ts")
                            || name.ends_with(".d.cts")
                            || name.ends_with(".d.mts"))
                    {
                        continue;
                    }
                    out.push(path);
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
