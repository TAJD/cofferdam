//! Regression harness for tree-sitter-rust grammar quirks.
//!
//! Each test parses a real Rust idiom that historically produced
//! false-positive ERROR nodes. These tests assert clean parses today;
//! if a future tree-sitter-rust upgrade reintroduces a quirk, the
//! offending case fails here BEFORE it pollutes the dogfood baseline
//! with spurious `Warning.ParseError` findings.
//!
//! History: cd-0039 originally tried to emit `Warning.ParseError` on
//! `has_errors()` trees but had to silent-skip them because
//! tree-sitter-rust 0.23 produced false positives on valid Rust like
//! `&raw` where `raw` is a variable name (the grammar tried to parse
//! it as the start of a raw-reference expression `&raw const x`).
//! Upgrading to 0.24 (cd-0039 followup) fixed all known quirks; the
//! engine now emits Warning.ParseError on `has_errors()` trees again.

use cofferdam_rust::parse_rust;

#[test]
fn raw_variable_name_after_ampersand() {
    // Idiom: `&raw` where `raw` is a variable. tree-sitter-rust 0.23
    // tries to parse this as the start of a raw-reference expression
    // (`&raw const x` / `&raw mut x`, stable since Rust 1.82). When
    // it doesn't find `const` or `mut` next, recovery emits an ERROR.
    //
    // Real source examples from cofferdam itself:
    //   crates/cofferdam-engine/src/baseline.rs:321
    //     `serde_json::from_str(&raw)`
    //   crates/cofferdam-engine/src/config.rs:205
    //     `parse(path, &raw)`
    //   crates/cofferdam-core/src/invariants.rs:386
    //     `parse(path, &raw)`
    let src = r#"
fn load(path: &str) -> Result<(), ()> {
    let raw = std::fs::read_to_string(path).map_err(|_| ())?;
    parse(path, &raw)
}

fn parse(_p: &str, _r: &str) -> Result<(), ()> { Ok(()) }
"#;
    let tree = parse_rust(src).expect("parse_rust returns a tree");
    assert!(
        !tree.has_errors(),
        "tree-sitter-rust regression: `&raw` where `raw` is a variable parses with errors. \
         If this fires after a tree-sitter-rust upgrade, the engine's `Warning.ParseError` \
         emission on `has_errors()` trees in cofferdam-engine::lib.rs will start firing on \
         valid cofferdam source — revert the upgrade or revive the silent-skip policy. \
         Error spans: {:?}",
        tree.error_spans()
    );
}

#[test]
fn raw_keyword_legitimately_used() {
    // Positive control: real `&raw const x` / `&raw mut x` from Rust
    // 1.82+ — the construct tree-sitter-rust IS trying to handle.
    let src = r#"
fn legit() {
    let x = 1u32;
    let _p = &raw const x;
}
"#;
    let tree = parse_rust(src).expect("parse_rust returns a tree");
    assert!(
        !tree.has_errors(),
        "legit raw-reference expression should parse cleanly; error spans: {:?}",
        tree.error_spans()
    );
}

#[test]
fn workspace_real_world_baseline_rs_line_321() {
    // Verbatim slice from crates/cofferdam-engine/src/baseline.rs
    // around line 321, the first false-positive site flagged by the
    // initial cd-0039 attempt to emit Warning.ParseError. Pinning the
    // exact pattern means a future fix won't accidentally regress.
    let src = r#"
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum BaselineError {
    Read { path: PathBuf, source: std::io::Error },
    Parse { path: PathBuf, source: serde_json::Error },
    Version { path: PathBuf, found: u32, supported: u32 },
}

pub fn read(path: &Path) -> Result<u32, BaselineError> {
    let raw = std::fs::read_to_string(path).map_err(|source| BaselineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let _baseline: u32 = serde_json::from_str(&raw).map_err(|source| BaselineError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(0)
}
"#;
    let tree = parse_rust(src).expect("parse_rust returns a tree");
    assert!(
        !tree.has_errors(),
        "verbatim baseline.rs:321-shape pattern regressed; error spans: {:?}",
        tree.error_spans()
    );
}
