//! Shared `package.json` entry-point resolution, used by any finalize-stage
//! check that needs to exempt a project's real public entry point from a
//! "this file looks unusually central" finding (`Design.BarrelReexportBloat`,
//! `Design.ImportFanOutOutlier`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use cofferdam_core::path_key;

/// Walk up from `file`'s directory looking for the nearest
/// `package.json`. Mirrors `cofferdam-cli`'s `find_npm_package_json`
/// walk-up pattern (doctor.rs), scoped to an arbitrary source file
/// instead of the running executable.
fn find_nearest_package_json(file: &Path) -> Option<PathBuf> {
    let mut dir = file.parent();
    while let Some(d) = dir {
        let candidate = d.join("package.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// Recursively collect every string leaf under `value` into `out`.
/// Used against the `exports` field, whose shape ranges from a plain
/// string to nested subpath/condition objects — over-collecting is
/// safe here since any extra candidate only makes entry-point exclusion
/// more permissive, never less.
fn collect_string_leaves(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Object(map) => {
            for v in map.values() {
                collect_string_leaves(v, out);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_string_leaves(v, out);
            }
        }
        _ => {}
    }
}

/// Path with its final extension removed (single-suffix strip; e.g.
/// `/p/index.ts` -> `/p/index`). Used to compare a `package.json`
/// entry-point candidate (typically `.js`) against a TS source file
/// (`.ts`) in the same directory without hardcoding an extension map.
///
/// Strips only the LAST suffix, so a `types`/`typings` entry pointing
/// at a `.d.ts` declaration file strips to `foo.d`, not `foo` — it
/// won't match a source `foo.ts`. Harmless: `types`/`typings` point at
/// declaration artifacts rather than the analyzed source, so they
/// weren't expected to match a barrel/hub source file anyway.
fn strip_extension(path: &Path) -> PathBuf {
    match path.file_stem() {
        Some(stem) => path.with_file_name(stem),
        None => path.to_path_buf(),
    }
}

/// Resolve `pkg_json_path`'s `main`/`module`/`types`/`typings`/
/// `exports` fields into a set of extension-stripped, normalised path
/// keys. Returns an empty set on any read/parse failure — a
/// non-existent or malformed `package.json` excludes nothing rather
/// than aborting the check.
fn resolve_entry_point_keys(pkg_json_path: &Path) -> HashSet<String> {
    let mut keys = HashSet::new();
    let Ok(text) = std::fs::read_to_string(pkg_json_path) else {
        return keys;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return keys;
    };
    let Some(pkg_dir) = pkg_json_path.parent() else {
        return keys;
    };

    let mut candidates = Vec::new();
    for field in ["main", "module", "types", "typings"] {
        if let Some(s) = json.get(field).and_then(|v| v.as_str()) {
            candidates.push(s.to_string());
        }
    }
    if let Some(exports) = json.get("exports") {
        collect_string_leaves(exports, &mut candidates);
    }

    for candidate in candidates {
        let trimmed = candidate.trim_start_matches("./");
        let abs = pkg_dir.join(trimmed);
        keys.insert(path_key(&strip_extension(&abs)));
    }
    keys
}

/// Per-`package.json` resolved entry-point key cache, shared across
/// calls to [`is_package_entry_point`] within one `finalize()` pass.
pub type EntryPointCache = HashMap<PathBuf, HashSet<String>>;

/// True when `file` resolves as the nearest ancestor `package.json`'s
/// declared entry point (`main`/`module`/`types`/`typings`/`exports`).
/// `cache` memoises each `package.json`'s resolved key set so a
/// directory with many candidate files only reads/parses it once.
pub fn is_package_entry_point(file: &Path, cache: &mut EntryPointCache) -> bool {
    let Some(pkg_json_path) = find_nearest_package_json(file) else {
        return false;
    };
    let keys = cache
        .entry(pkg_json_path.clone())
        .or_insert_with(|| resolve_entry_point_keys(&pkg_json_path));
    keys.contains(&path_key(&strip_extension(file)))
}
