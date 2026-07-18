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
///
/// `dir_cache` memoises the resolved `package.json` path (or its
/// absence) per directory visited, and every directory walked through
/// on the way to an answer is backfilled with that same answer before
/// returning — so a project with many files sharing an ancestor chain
/// only walks each unique directory once, rather than re-stat-ing every
/// ancestor for every file in `finalize()`.
fn find_nearest_package_json(
    file: &Path,
    dir_cache: &mut HashMap<PathBuf, Option<PathBuf>>,
) -> Option<PathBuf> {
    let mut visited = Vec::new();
    let mut dir = file.parent();
    let result = loop {
        let Some(d) = dir else { break None };
        if let Some(cached) = dir_cache.get(d) {
            break cached.clone();
        }
        visited.push(d.to_path_buf());
        let candidate = d.join("package.json");
        if candidate.is_file() {
            break Some(candidate);
        }
        dir = d.parent();
    };
    for d in visited {
        dir_cache.insert(d, result.clone());
    }
    result
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

/// Memoization shared across calls to [`is_package_entry_point`] within
/// one `finalize()` pass: which directory resolves to which (if any)
/// `package.json`, and each `package.json`'s resolved entry-point keys.
#[derive(Default)]
pub struct EntryPointCache {
    package_json_by_dir: HashMap<PathBuf, Option<PathBuf>>,
    keys_by_package_json: HashMap<PathBuf, HashSet<String>>,
}

/// True when `file` resolves as the nearest ancestor `package.json`'s
/// declared entry point (`main`/`module`/`types`/`typings`/`exports`).
pub fn is_package_entry_point(file: &Path, cache: &mut EntryPointCache) -> bool {
    let Some(pkg_json_path) = find_nearest_package_json(file, &mut cache.package_json_by_dir)
    else {
        return false;
    };
    let keys = cache
        .keys_by_package_json
        .entry(pkg_json_path.clone())
        .or_insert_with(|| resolve_entry_point_keys(&pkg_json_path));
    keys.contains(&path_key(&strip_extension(file)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "cofferdam-test-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            TempProject { root }
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    #[test]
    fn nested_files_under_one_package_json_resolve_through_the_cache() {
        // Two files at different depths below the same package.json.
        // The directory cache backfills every directory visited on the
        // way to an answer — this must not corrupt the answer for a
        // shallower or deeper sibling that shares part of the walk.
        let project = TempProject::new("nested-entry");
        let nested_dir = project.root.join("src").join("deep");
        std::fs::create_dir_all(&nested_dir).unwrap();
        let mut pkg = std::fs::File::create(project.root.join("package.json")).unwrap();
        write!(pkg, r#"{{"main": "./src/deep/entry.ts"}}"#).unwrap();
        drop(pkg);

        let mut cache = EntryPointCache::default();
        let entry = nested_dir.join("entry.ts");
        let sibling = nested_dir.join("sibling.ts");
        let shallow = project.root.join("src").join("other.ts");

        assert!(
            is_package_entry_point(&entry, &mut cache),
            "entry.ts must resolve as the declared main entry point"
        );
        assert!(
            !is_package_entry_point(&sibling, &mut cache),
            "a non-entry file in the same directory must not be swept in by the cache"
        );
        assert!(
            !is_package_entry_point(&shallow, &mut cache),
            "a non-entry file in a shallower directory sharing the walk-up path must not be \
             swept in by the cache"
        );
    }

    #[test]
    fn directory_with_no_package_json_resolves_to_none_and_stays_none() {
        // No package.json anywhere in this tree — every call must return
        // false, and repeated calls (exercising the cached `None` path)
        // must keep returning false rather than panicking or flipping.
        let project = TempProject::new("no-entry");
        let nested_dir = project.root.join("a").join("b");
        std::fs::create_dir_all(&nested_dir).unwrap();

        let mut cache = EntryPointCache::default();
        let file = nested_dir.join("f.ts");
        assert!(!is_package_entry_point(&file, &mut cache));
        assert!(
            !is_package_entry_point(&file, &mut cache),
            "a cached absence must still resolve to false on a second call"
        );
    }

    #[test]
    fn malformed_package_json_excludes_nothing() {
        let project = TempProject::new("malformed-entry");
        let mut pkg = std::fs::File::create(project.root.join("package.json")).unwrap();
        write!(pkg, "not valid json").unwrap();
        drop(pkg);

        let mut cache = EntryPointCache::default();
        let file = project.root.join("index.ts");
        assert!(
            !is_package_entry_point(&file, &mut cache),
            "a malformed package.json must exclude nothing rather than panicking"
        );
    }
}
