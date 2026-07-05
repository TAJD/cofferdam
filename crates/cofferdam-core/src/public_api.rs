//! `[public_api]` allowlist resolution, shared between checks that need
//! to exempt the project's published surface from "nobody-imports-it"
//! findings (`Design.OrphanExport`, `Warning.UnusedImport`).
//!
//! Entries in `[public_api].exports` (from `cofferdam.invariants.toml`)
//! that contain no glob metacharacters (`* ? [ {`) are stored in
//! `exact` for O(1) lookup. Glob entries compile into a `GlobSet` and
//! match against the project-root-relative, forward-slash form of a
//! file key. `package.json:<key>` entries are accepted by the schema
//! but currently dropped here — resolution lands in a follow-up bead.
//!
//! `project_root` is absolutized via `std::path::absolute` before any
//! join (cd-gro / gh #41). `Engine.analyze_with_sources` promotes
//! every source path to absolute form (cd-q9f), so the stored exact
//! keys must match that — otherwise `is_match` on an absolute
//! `file_key` silently misses every exact entry when the spec was
//! discovered from a relative path. The root prefix used for
//! glob-path stripping gets the same treatment so an absolute
//! `file_key` can be reduced to the project-relative form that globs
//! are written against.

use std::collections::HashSet;
use std::path::Path;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

/// Resolved public-API allowlist. Build via [`resolve_public_api`] and
/// test file keys via [`PublicApi::is_match`].
pub struct PublicApi {
    exact: HashSet<String>,
    globs: GlobSet,
    /// Normalised absolute root prefix (`path_key(project_root)`) used to
    /// strip the leading segment before glob matching.
    root_key: String,
}

impl PublicApi {
    /// Test whether `file_key` (a forward-slash, normalised path — may be
    /// absolute or relative) is covered by this allowlist.
    pub fn is_match(&self, file_key: &str) -> bool {
        if self.exact.contains(file_key) {
            return true;
        }
        // Derive a project-relative path for glob matching.
        //
        // Strategy (in preference order):
        // 1. Strip the absolute root prefix `<root_key>/` if file_key is absolute.
        // 2. Strip a leading `./` from relative paths (engine sometimes emits
        //    `./src/foo.ts` when invoked against a relative target directory).
        // 3. Fall back to the raw file_key — glob authors using an absolute-path
        //    pattern will still match.
        let rel = {
            let with_slash = if self.root_key.ends_with('/') {
                self.root_key.as_str().to_string()
            } else {
                format!("{}/", self.root_key)
            };
            if let Some(stripped) = file_key.strip_prefix(with_slash.as_str()) {
                stripped
            } else if let Some(stripped) = file_key.strip_prefix(&self.root_key) {
                stripped.trim_start_matches('/')
            } else {
                // Relative path (possibly `./src/…`). Strip the leading `./`
                // so patterns like `src/legacy/**/*.ts` match without needing
                // a leading `./` in the pattern.
                file_key.trim_start_matches("./")
            }
        };
        self.globs.is_match(rel)
    }
}

impl Default for PublicApi {
    fn default() -> Self {
        PublicApi {
            exact: HashSet::new(),
            globs: GlobSet::empty(),
            root_key: String::new(),
        }
    }
}

/// Convert `[public_api].exports` entries into a `PublicApi` for fast
/// lookup. Exact paths are normalised as absolute file keys (same
/// canonicalisation as the rest of the checks crate). Glob patterns
/// are kept project-root-relative in forward-slash form so
/// `GlobSet::is_match` receives the same relative key the caller
/// computes. Empty input yields a default `PublicApi` (no matches).
pub fn resolve_public_api(entries: &[String], project_root: &Path) -> PublicApi {
    let project_root_abs =
        std::path::absolute(project_root).unwrap_or_else(|_| project_root.to_path_buf());

    let mut exact = HashSet::new();
    let mut glob_builder = GlobSetBuilder::new();
    let mut has_globs = false;

    for entry in entries {
        if entry.starts_with("package.json:") {
            // Resolution deferred — see follow-up bead. Listed here so
            // the schema accepts it; today these strings are dropped.
            continue;
        }
        let trimmed = entry.trim_start_matches("./");
        if is_glob_pattern(trimmed) {
            // Normalise to forward slashes before compiling so authors can
            // write `"components/ui/**/*.tsx"` without platform concerns.
            let normalised = trimmed.replace('\\', "/");
            match GlobBuilder::new(&normalised)
                .literal_separator(true)
                .build()
            {
                Ok(g) => {
                    glob_builder.add(g);
                    has_globs = true;
                }
                Err(_) => {
                    // Invalid glob syntax — skip silently. A bad pattern
                    // exempts nothing; it doesn't crash the run.
                }
            }
        } else {
            let abs = project_root_abs.join(trimmed);
            exact.insert(path_key(&abs));
        }
    }

    let globs = if has_globs {
        glob_builder.build().unwrap_or_default()
    } else {
        GlobSet::empty()
    };

    PublicApi {
        exact,
        globs,
        root_key: path_key(&project_root_abs),
    }
}

/// Returns true when `entry` contains at least one glob metacharacter.
pub fn is_glob_pattern(entry: &str) -> bool {
    entry.contains('*') || entry.contains('?') || entry.contains('[') || entry.contains('{')
}

/// Forward-slash, case-insensitive-on-Windows path key. Matches the
/// per-check `path_key` helpers in `design.rs` / `warning.rs` /
/// `refactor.rs`; shared here so the `PublicApi` table keys agree
/// with whatever the call-site uses.
pub fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}
