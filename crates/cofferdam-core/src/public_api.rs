//! `[public_api]` allowlist resolution, shared between checks that need
//! to exempt the project's published surface from "nobody-imports-it"
//! findings (`Design.OrphanExport`, `Warning.UnusedImport`).
//!
//! Entries in `[public_api].exports` (from `cofferdam.invariants.toml`)
//! that contain no glob metacharacters (`* ? [ {`) are stored in
//! `exact` for O(1) lookup. Glob entries compile into a `GlobSet` and
//! match against the project-root-relative, forward-slash form of a
//! file key. A `package.json:<key>` entry reads that key out of the
//! project root's `package.json` and stores every string leaf below it
//! as an extension-stripped key in `entry_points`, so a manifest
//! pointing at built output (`./dist/index.js`) still matches the
//! source it was built from (`src/index.ts`).
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
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

/// Resolved public-API allowlist. Build via [`resolve_public_api`] and
/// test file keys via [`PublicApi::is_match`].
pub struct PublicApi {
    exact: HashSet<String>,
    globs: GlobSet,
    /// Extension-stripped keys resolved from `package.json:<key>`
    /// entries, matched against the extension-stripped form of a file
    /// key. Stripping both sides is what lets a manifest written in
    /// terms of `.js` output match a `.ts` source.
    entry_points: HashSet<String>,
    /// `package.json:<key>` entries that resolved to nothing — a missing
    /// or malformed manifest, or a key that is not there.
    unresolved: Vec<String>,
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
        if !self.entry_points.is_empty()
            && self.entry_points.contains(&strip_extension_key(file_key))
        {
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

    /// `package.json:<key>` entries that pointed at nothing. Callers that
    /// report on the allowlist use this to say the surface is incomplete
    /// rather than treating it as fully resolved.
    pub fn unresolved(&self) -> &[String] {
        &self.unresolved
    }
}

impl Default for PublicApi {
    fn default() -> Self {
        PublicApi {
            exact: HashSet::new(),
            globs: GlobSet::empty(),
            entry_points: HashSet::new(),
            unresolved: Vec::new(),
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
    let mut entry_points = HashSet::new();
    let mut unresolved = Vec::new();
    let mut manifest: Option<Option<serde_json::Value>> = None;

    for entry in entries {
        if let Some(key) = entry.strip_prefix("package.json:") {
            let json = manifest.get_or_insert_with(|| read_manifest(&project_root_abs));
            let resolved = json
                .as_ref()
                .map(|j| manifest_entry_points(j, key, &project_root_abs))
                .unwrap_or_default();
            if resolved.is_empty() {
                unresolved.push(entry.clone());
            } else {
                entry_points.extend(resolved);
            }
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
        entry_points,
        unresolved,
        root_key: path_key(&project_root_abs),
    }
}

fn read_manifest(project_root_abs: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(project_root_abs.join("package.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Every string leaf below `key` in the manifest, joined onto the project
/// root and stored extension-stripped. `exports` ranges from a plain
/// string to nested subpath/condition objects, so leaves are collected
/// recursively; over-collecting only widens the allowlist, which is the
/// safe direction for a check that exempts rather than flags.
fn manifest_entry_points(
    manifest: &serde_json::Value,
    key: &str,
    project_root_abs: &Path,
) -> HashSet<String> {
    let mut candidates = Vec::new();
    if let Some(value) = manifest.get(key) {
        collect_string_leaves(value, &mut candidates);
    }

    let mut keys = HashSet::new();
    for candidate in candidates {
        let abs = project_root_abs.join(candidate.trim_start_matches("./"));
        keys.insert(path_key(&strip_extension(&abs)));
        if let Some(sibling) = with_src_sibling(&abs) {
            keys.insert(path_key(&strip_extension(&sibling)));
        }
    }
    keys
}

fn collect_string_leaves(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Object(map) => map.values().for_each(|v| collect_string_leaves(v, out)),
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_string_leaves(v, out)),
        _ => {}
    }
}

/// Path components conventionally holding build output. A manifest points
/// at the artifact (`./dist/index.js`); the analysed source sits in a
/// parallel `src/` tree.
const BUILD_DIR_ALIASES: &[&str] = &["dist", "build", "lib", "out", "output"];

/// `path` with the first [`BUILD_DIR_ALIASES`] component replaced by
/// `src`. Component-wise, so a `distributed/` directory is left alone.
fn with_src_sibling(path: &Path) -> Option<PathBuf> {
    let mut components: Vec<_> = path.components().collect();
    let index = components.iter().position(|c| {
        matches!(c, std::path::Component::Normal(name) if name
            .to_str()
            .is_some_and(|s| BUILD_DIR_ALIASES.contains(&s)))
    })?;
    components[index] = std::path::Component::Normal(std::ffi::OsStr::new("src"));
    Some(components.iter().collect())
}

fn strip_extension(path: &Path) -> PathBuf {
    match path.file_stem() {
        Some(stem) => path.with_file_name(stem),
        None => path.to_path_buf(),
    }
}

/// [`strip_extension`] over an already-normalised path key. Only a
/// trailing extension counts — a dot in a directory name must not
/// truncate the path.
fn strip_extension_key(file_key: &str) -> String {
    match file_key.rsplit_once('/') {
        Some((dir, name)) => match name.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => format!("{dir}/{stem}"),
            _ => file_key.to_string(),
        },
        None => match file_key.rsplit_once('.') {
            Some((stem, _)) if !stem.is_empty() => stem.to_string(),
            _ => file_key.to_string(),
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    struct TempProject(PathBuf);

    impl TempProject {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "cofferdam-public-api-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::remove_dir_all(&root).ok();
            std::fs::create_dir_all(&root).unwrap();
            TempProject(root)
        }

        fn manifest(&self, json: &str) {
            std::fs::write(self.0.join("package.json"), json).unwrap();
        }

        fn key(&self, rel: &str) -> String {
            path_key(&self.0.join(rel))
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn package_json_pointer_exempts_the_declared_entry_point() {
        let project = TempProject::new("exports");
        project.manifest(r#"{"exports": {".": "./src/index.ts"}}"#);

        let api = resolve_public_api(&["package.json:exports".to_string()], &project.0);
        assert!(api.is_match(&project.key("src/index.ts")));
        assert!(!api.is_match(&project.key("src/other.ts")));
        assert!(api.unresolved().is_empty());
    }

    #[test]
    fn a_manifest_pointing_at_built_output_matches_the_source_it_came_from() {
        // The manifest names `dist/index.js`; the analysed file is
        // `src/index.ts`. Without the build-dir swap and the extension
        // strip, the pointer would resolve and still exempt nothing.
        let project = TempProject::new("dist-src");
        project.manifest(r#"{"main": "./dist/index.js"}"#);

        let api = resolve_public_api(&["package.json:main".to_string()], &project.0);
        assert!(api.is_match(&project.key("src/index.ts")));
        assert!(!api.is_match(&project.key("src/helpers.ts")));
    }

    #[test]
    fn nested_export_conditions_all_count() {
        let project = TempProject::new("conditions");
        project.manifest(
            r#"{"exports": {".": {"import": "./src/index.ts"},
                            "./cli": {"import": "./src/cli.ts"}}}"#,
        );

        let api = resolve_public_api(&["package.json:exports".to_string()], &project.0);
        assert!(api.is_match(&project.key("src/index.ts")));
        assert!(api.is_match(&project.key("src/cli.ts")));
    }

    #[test]
    fn a_pointer_at_a_missing_key_is_reported_rather_than_ignored() {
        let project = TempProject::new("missing-key");
        project.manifest(r#"{"main": "./src/index.ts"}"#);

        let api = resolve_public_api(&["package.json:exports".to_string()], &project.0);
        assert_eq!(api.unresolved(), ["package.json:exports".to_string()]);
    }

    #[test]
    fn a_missing_or_malformed_manifest_is_reported() {
        let missing = TempProject::new("no-manifest");
        let api = resolve_public_api(&["package.json:exports".to_string()], &missing.0);
        assert_eq!(api.unresolved().len(), 1);

        let malformed = TempProject::new("bad-manifest");
        malformed.manifest("not json");
        let api = resolve_public_api(&["package.json:exports".to_string()], &malformed.0);
        assert_eq!(api.unresolved().len(), 1);
    }

    #[test]
    fn a_dot_in_a_directory_name_does_not_truncate_the_key() {
        assert_eq!(strip_extension_key("/p/v1.2/index.ts"), "/p/v1.2/index");
        assert_eq!(strip_extension_key("/p/v1.2/README"), "/p/v1.2/README");
    }
}
