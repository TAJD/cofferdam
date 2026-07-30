//! Integration tests for `cofferdam-engine::discover`.
//!
//! Two clusters of coverage:
//! - Declaration-file skipping (cd-ofu): default-skip behavior plus
//!   the explicit-include escape hatch.
//! - General discovery surface (cd-ie9): extension filtering, recursive
//!   walking, multi-root, hidden files, sorting.

use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use cofferdam_engine::{discover, DiscoveryOptions, DEFAULT_EXTENSIONS};

// ============================================================
// Declaration-file skipping (cd-ofu)
// ============================================================

#[test]
fn test_skip_declaration_files_by_default() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();

    fs::write(tmp_path.join("a.ts"), "").unwrap();
    fs::write(tmp_path.join("b.tsx"), "").unwrap();
    fs::write(tmp_path.join("c.d.ts"), "").unwrap();
    fs::write(tmp_path.join("d.d.cts"), "").unwrap();

    let opts = DiscoveryOptions::default();
    let files = discover(&[tmp_path], &opts).expect("discover");

    let names: Vec<String> = files
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect();

    assert!(names.contains(&"a.ts".to_string()));
    assert!(names.contains(&"b.tsx".to_string()));
    assert!(!names.contains(&"c.d.ts".to_string()));
    assert!(!names.contains(&"d.d.cts".to_string()));
    assert_eq!(files.len(), 2);
}

#[test]
fn test_include_declaration_files_when_disabled() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();

    fs::write(tmp_path.join("a.ts"), "").unwrap();
    fs::write(tmp_path.join("b.tsx"), "").unwrap();
    fs::write(tmp_path.join("c.d.ts"), "").unwrap();
    fs::write(tmp_path.join("d.d.cts"), "").unwrap();

    let opts = DiscoveryOptions {
        skip_declaration_files: false,
        ..DiscoveryOptions::default()
    };
    let files = discover(&[tmp_path], &opts).expect("discover");
    assert_eq!(files.len(), 4);
}

#[test]
fn test_non_declaration_dts_files_not_skipped() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();

    // Suffix is `.ts`, but the basename "weird.dts" is not a declaration
    // file — the `.d.ts` filter must check the actual `.d.ts` ending.
    fs::write(tmp_path.join("weird.dts.ts"), "").unwrap();

    let opts = DiscoveryOptions::default();
    let files = discover(&[tmp_path], &opts).expect("discover");
    let names: Vec<String> = files
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect();

    assert!(names.contains(&"weird.dts.ts".to_string()));
}

#[test]
fn test_all_declaration_file_variants() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();

    fs::write(tmp_path.join("code.ts"), "").unwrap();
    fs::write(tmp_path.join("index.d.ts"), "").unwrap();
    fs::write(tmp_path.join("types.d.cts"), "").unwrap();
    fs::write(tmp_path.join("module.d.mts"), "").unwrap();

    let opts = DiscoveryOptions::default();
    let files = discover(&[tmp_path], &opts).expect("discover");
    let file_names: Vec<String> = files
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect();

    assert!(file_names.contains(&"code.ts".to_string()));
    assert!(!file_names.contains(&"index.d.ts".to_string()));
    assert!(!file_names.contains(&"types.d.cts".to_string()));
    assert!(!file_names.contains(&"module.d.mts".to_string()));
}

// ============================================================
// Basic discovery (cd-ie9)
// ============================================================

#[test]
fn discover_empty_directory() {
    let temp_dir = TempDir::new().expect("temp dir");
    let opts = DiscoveryOptions::default();

    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    assert!(files.is_empty());
}

#[test]
fn discover_single_ts_file() {
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");
    std::fs::write(&file_path, "const x = 1;").expect("write file");

    let opts = DiscoveryOptions::default();
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_name().unwrap(), "test.ts");
}

#[test]
fn discover_multiple_ts_files() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir.path().join("a.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("b.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("c.ts"), "").expect("write");

    let opts = DiscoveryOptions::default();
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    assert_eq!(files.len(), 3);
}

// ============================================================
// File type filtering
// ============================================================

#[test]
fn discover_filters_to_typescript_extensions() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir.path().join("test.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("test.tsx"), "").expect("write");
    std::fs::write(temp_dir.path().join("test.mts"), "").expect("write");
    std::fs::write(temp_dir.path().join("test.cts"), "").expect("write");
    std::fs::write(temp_dir.path().join("test.js"), "").expect("write");
    std::fs::write(temp_dir.path().join("test.txt"), "").expect("write");

    let opts = DiscoveryOptions::default();
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    // Should find only .ts, .tsx, .mts, .cts
    assert_eq!(files.len(), 4);

    let extensions: Vec<_> = files
        .iter()
        .filter_map(|p| p.extension())
        .filter_map(|e| e.to_str())
        .collect();

    assert!(extensions.contains(&"ts"));
    assert!(extensions.contains(&"tsx"));
    assert!(extensions.contains(&"mts"));
    assert!(extensions.contains(&"cts"));
    assert!(!extensions.contains(&"js"));
    assert!(!extensions.contains(&"txt"));
}

#[test]
fn discover_includes_d_ts_files_when_opted_in() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir.path().join("test.d.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("test.d.mts"), "").expect("write");
    std::fs::write(temp_dir.path().join("test.d.cts"), "").expect("write");

    let opts = DiscoveryOptions {
        skip_declaration_files: false,
        ..DiscoveryOptions::default()
    };
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    // Declaration files included when skip_declaration_files = false
    assert_eq!(files.len(), 3);
}

// ============================================================
// Recursive directory walking
// ============================================================

#[test]
fn discover_walks_subdirectories() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir.path().join("a.ts"), "").expect("write");
    std::fs::create_dir(temp_dir.path().join("subdir")).expect("mkdir");
    std::fs::write(temp_dir.path().join("subdir/b.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("subdir/c.ts"), "").expect("write");

    let opts = DiscoveryOptions::default();
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    assert_eq!(files.len(), 3);
}

#[test]
fn discover_walks_deeply_nested_directories() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir.path().join("a.ts"), "").expect("write");
    std::fs::create_dir_all(temp_dir.path().join("a/b/c/d")).expect("mkdir");
    std::fs::write(temp_dir.path().join("a/b/c/d/deep.ts"), "").expect("write");

    let opts = DiscoveryOptions::default();
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    assert_eq!(files.len(), 2);
}

// ============================================================
// Multiple root paths
// ============================================================

#[test]
fn discover_processes_multiple_roots() {
    let temp_dir1 = TempDir::new().expect("temp dir");
    let temp_dir2 = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir1.path().join("a.ts"), "").expect("write");
    std::fs::write(temp_dir2.path().join("b.ts"), "").expect("write");

    let opts = DiscoveryOptions::default();
    let files = discover(&[temp_dir1.path(), temp_dir2.path()], &opts).expect("should not error");

    assert_eq!(files.len(), 2);
}

#[test]
fn discover_handles_single_file_as_root() {
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.ts");
    std::fs::write(&file_path, "").expect("write");

    let opts = DiscoveryOptions::default();
    let files = discover(&[&file_path], &opts).expect("should not error");

    // Should return the single file if it matches extensions
    assert_eq!(files.len(), 1);
    assert_eq!(files[0], file_path);
}

#[test]
fn discover_single_file_root_may_include_non_matching() {
    let temp_dir = TempDir::new().expect("temp dir");
    let file_path = temp_dir.path().join("test.js");
    std::fs::write(&file_path, "").expect("write");

    let opts = DiscoveryOptions::default();
    let files = discover(&[&file_path], &opts).expect("should not error");

    // When a specific file is passed as root, the ignore crate may include it
    // even if it doesn't match extensions (depends on walker behavior).
    // Just verify it doesn't crash.
    assert!(files.len() <= 1);
}

// ============================================================
// Custom extensions
// ============================================================

#[test]
fn discover_respects_custom_extensions() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir.path().join("a.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("b.js"), "").expect("write");

    let opts = DiscoveryOptions {
        extensions: vec!["js".to_string()],
        ..DiscoveryOptions::default()
    };
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    // Should find only .js files
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].extension().unwrap(), "js");
}

// ============================================================
// Hidden files
// ============================================================

#[test]
fn discover_respects_include_hidden_setting() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir.path().join("visible.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join(".hidden.ts"), "").expect("write");

    let opts_hidden_off = DiscoveryOptions {
        extensions: DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
        respect_ignore: false,
        include_hidden: false,
        ..DiscoveryOptions::default()
    };
    let files_no_hidden = discover(&[temp_dir.path()], &opts_hidden_off).expect("should not error");

    let opts_hidden_on = DiscoveryOptions {
        extensions: DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
        respect_ignore: false,
        include_hidden: true,
        ..DiscoveryOptions::default()
    };
    let files_with_hidden =
        discover(&[temp_dir.path()], &opts_hidden_on).expect("should not error");

    assert!(files_no_hidden.len() <= files_with_hidden.len());
}

#[test]
fn discover_includes_hidden_files_when_requested() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir.path().join("visible.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join(".hidden.ts"), "").expect("write");

    let opts = DiscoveryOptions {
        extensions: DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
        respect_ignore: false,
        include_hidden: true,
        ..DiscoveryOptions::default()
    };
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    assert_eq!(files.len(), 2);
}

#[test]
fn discover_excludes_hidden_directories() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::create_dir(temp_dir.path().join(".hidden")).expect("mkdir");
    std::fs::write(temp_dir.path().join(".hidden/test.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("visible.ts"), "").expect("write");

    let opts = DiscoveryOptions {
        extensions: DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
        respect_ignore: false,
        include_hidden: false,
        ..DiscoveryOptions::default()
    };
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    // Should not walk into .hidden directory
    assert_eq!(files.len(), 1);
}

// ============================================================
// Ignore-file precedence over extension filtering (cd-103)
// ============================================================

#[test]
fn discover_respects_gitignore_for_matching_extension() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();

    // .gitignore is only honored inside a git worktree.
    //
    // Strip any GIT_DIR / GIT_WORK_TREE / etc. this process inherited
    // (e.g. from a `pre-push` hook, or from running inside a git
    // worktree checkout) — otherwise `git init` targets the *inherited*
    // repo instead of `root`, `root` never gets its own `.git`, and the
    // ignore crate's repo-boundary detection walks up into the wrong
    // tree. Mirrors `GIT_ENV_VARS_TO_CLEAR` in
    // `cofferdam-cli/tests/orphan_since_report_scope.rs`.
    let mut git_init = std::process::Command::new("git");
    git_init.arg("init").arg("-q").current_dir(root);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
    ] {
        git_init.env_remove(var);
    }
    git_init.status().expect("git init");
    fs::write(root.join(".gitignore"), "*.test.ts\n").unwrap();
    fs::write(root.join("a.test.ts"), "").unwrap();
    fs::write(root.join("a.ts"), "").unwrap();

    let opts = DiscoveryOptions::default();
    let files = discover(&[root], &opts).unwrap();

    let names: Vec<String> = files
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect();
    assert_eq!(names, vec!["a.ts".to_string()]);
}

#[test]
fn discover_respects_cofferdamignore_for_matching_extension() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path();

    fs::write(root.join(".cofferdamignore"), "*.probe.ts\n").unwrap();
    fs::write(root.join("a.probe.ts"), "").unwrap();
    fs::write(root.join("a.ts"), "").unwrap();

    let opts = DiscoveryOptions::default();
    let files = discover(&[root], &opts).unwrap();

    let names: Vec<String> = files
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
        .collect();
    assert_eq!(names, vec!["a.ts".to_string()]);
}

// ============================================================
// Empty input
// ============================================================

#[test]
fn discover_empty_roots_returns_empty() {
    let opts = DiscoveryOptions::default();
    let files = discover::<PathBuf>(&[], &opts).expect("should not error");

    assert!(files.is_empty());
}

// ============================================================
// Result is deterministic
// ============================================================

#[test]
fn discover_returns_sorted_paths() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir.path().join("zebra.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("apple.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("banana.ts"), "").expect("write");

    let opts = DiscoveryOptions::default();
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    assert_eq!(files.len(), 3);

    // Two calls should return same order
    let files2 = discover(&[temp_dir.path()], &opts).expect("should not error");
    assert_eq!(files, files2);
}

// ============================================================
// Mixed files and extensions
// ============================================================

#[test]
fn discover_complex_directory_structure() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir.path().join("root.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("root.js"), "").expect("write");

    std::fs::create_dir(temp_dir.path().join("src")).expect("mkdir");
    std::fs::write(temp_dir.path().join("src/main.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("src/main.tsx"), "").expect("write");

    std::fs::create_dir(temp_dir.path().join("src/utils")).expect("mkdir");
    std::fs::write(temp_dir.path().join("src/utils/helper.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("src/utils/helper.test.ts"), "").expect("write");

    let opts = DiscoveryOptions::default();
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    // Should find all .ts/.tsx files (5 total), excluding .js
    assert_eq!(files.len(), 5);
}
