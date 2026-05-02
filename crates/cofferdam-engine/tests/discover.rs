use std::fs;
use tempfile::TempDir;

use cofferdam_engine::discover::{discover, DiscoveryOptions};

#[test]
fn test_skip_declaration_files_by_default() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();

    // Create test files
    fs::write(tmp_path.join("a.ts"), "").unwrap();
    fs::write(tmp_path.join("b.tsx"), "").unwrap();
    fs::write(tmp_path.join("c.d.ts"), "").unwrap();
    fs::write(tmp_path.join("d.d.cts"), "").unwrap();

    let opts = DiscoveryOptions::default();
    let results = discover(&[tmp_path], &opts).unwrap();

    let file_names: Vec<String> = results
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
        .collect();

    assert!(file_names.contains(&"a.ts".to_string()));
    assert!(file_names.contains(&"b.tsx".to_string()));
    assert!(!file_names.contains(&"c.d.ts".to_string()));
    assert!(!file_names.contains(&"d.d.cts".to_string()));
}

#[test]
fn test_include_declaration_files_when_disabled() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();

    // Create test files
    fs::write(tmp_path.join("a.ts"), "").unwrap();
    fs::write(tmp_path.join("b.tsx"), "").unwrap();
    fs::write(tmp_path.join("c.d.ts"), "").unwrap();
    fs::write(tmp_path.join("d.d.cts"), "").unwrap();

    let opts = DiscoveryOptions {
        skip_declaration_files: false,
        ..Default::default()
    };

    let results = discover(&[tmp_path], &opts).unwrap();

    let file_names: Vec<String> = results
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
        .collect();

    assert!(file_names.contains(&"a.ts".to_string()));
    assert!(file_names.contains(&"b.tsx".to_string()));
    assert!(file_names.contains(&"c.d.ts".to_string()));
    assert!(file_names.contains(&"d.d.cts".to_string()));
}

#[test]
fn test_non_declaration_dts_files_not_skipped() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();

    // Create a file that ends with .dts but is not a declaration file
    fs::write(tmp_path.join("weird.dts.ts"), "").unwrap();
    fs::write(tmp_path.join("a.d.ts"), "").unwrap();

    let opts = DiscoveryOptions::default();
    let results = discover(&[tmp_path], &opts).unwrap();

    let file_names: Vec<String> = results
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
        .collect();

    assert!(file_names.contains(&"weird.dts.ts".to_string()));
    assert!(!file_names.contains(&"a.d.ts".to_string()));
}

#[test]
fn test_all_declaration_file_variants() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();

    // Create test files
    fs::write(tmp_path.join("index.d.ts"), "").unwrap();
    fs::write(tmp_path.join("types.d.cts"), "").unwrap();
    fs::write(tmp_path.join("module.d.mts"), "").unwrap();
    fs::write(tmp_path.join("code.ts"), "").unwrap();

    let opts = DiscoveryOptions::default();
    let results = discover(&[tmp_path], &opts).unwrap();

    let file_names: Vec<String> = results
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
        .collect();

    assert!(file_names.contains(&"code.ts".to_string()));
    assert!(!file_names.contains(&"index.d.ts".to_string()));
    assert!(!file_names.contains(&"types.d.cts".to_string()));
    assert!(!file_names.contains(&"module.d.mts".to_string()));
}
