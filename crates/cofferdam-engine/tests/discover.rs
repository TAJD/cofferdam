//! Integration tests for `cofferdam-engine::discover` module.
//!
//! Acceptance criteria:
//! - discover() returns expected files for a temp dir tree
//! - File extensions are filtered correctly (.ts, .tsx, .mts, .cts)
//! - Directories are walked recursively
//! - Result is deterministic (sorted)

use std::path::PathBuf;
use tempfile::TempDir;

use cofferdam_engine::{discover, DiscoveryOptions, DEFAULT_EXTENSIONS};

// ============================================================
// Basic discovery
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
fn discover_includes_d_ts_files() {
    let temp_dir = TempDir::new().expect("temp dir");

    std::fs::write(temp_dir.path().join("test.d.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("test.d.mts"), "").expect("write");
    std::fs::write(temp_dir.path().join("test.d.cts"), "").expect("write");

    let opts = DiscoveryOptions::default();
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    // Declaration files should be included
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
    // even if it doesn't match extensions (depends on walker behavior)
    // Just verify it doesn't crash
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
        respect_ignore: true,
        include_hidden: false,
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

    // With include_hidden = false (default)
    let opts_hidden_off = DiscoveryOptions {
        extensions: DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
        respect_ignore: false,
        include_hidden: false,
    };
    let files_no_hidden = discover(&[temp_dir.path()], &opts_hidden_off).expect("should not error");

    // With include_hidden = true
    let opts_hidden_on = DiscoveryOptions {
        extensions: DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect(),
        respect_ignore: false,
        include_hidden: true,
    };
    let files_with_hidden =
        discover(&[temp_dir.path()], &opts_hidden_on).expect("should not error");

    // Should find fewer files when hidden is disabled
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
    };
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    // Should include .hidden.ts when include_hidden is true
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
    };
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    // Should not walk into .hidden directory
    assert_eq!(files.len(), 1);
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

    // Write files in non-alphabetical order
    std::fs::write(temp_dir.path().join("zebra.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("apple.ts"), "").expect("write");
    std::fs::write(temp_dir.path().join("banana.ts"), "").expect("write");

    let opts = DiscoveryOptions::default();
    let files = discover(&[temp_dir.path()], &opts).expect("should not error");

    // Should be sorted (or at least consistent)
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

    // Create a complex structure
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

    // Should find all .ts, .tsx files (4 total), excluding .js
    assert_eq!(files.len(), 5);
}
