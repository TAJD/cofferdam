//! Integration tests for the type-host wire (cd-9hp.2 cp1).
//!
//! These tests verify the round-trip between the Rust client and the
//! Node-side ts-morph worker, exercising both the success and failure
//! paths. They skip silently when Node isn't on PATH so CI environments
//! without Node still pass.
//!
//! cp1 surface tested:
//!   - `--ping --no-load` round-trips with `ok=true` and null timings.
//!   - `--ping` without ts-morph installed surfaces a
//!     `ts_morph_unavailable` error via `--robot`.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

fn node_present() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn type_host_ping_no_load_succeeds() {
    if !node_present() {
        return;
    }
    let tempdir = tempfile::tempdir().expect("create tempdir");
    let out = Command::new(cofferdam_bin())
        .args([
            "type-host",
            "--no-load",
            "--robot",
            "--project",
            tempdir.path().to_str().expect("utf8 tempdir"),
        ])
        .output()
        .expect("spawn cofferdam type-host");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "ping --no-load should succeed; stdout={stdout}\nstderr={stderr}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("ping JSON parses");
    assert_eq!(parsed["ok"], serde_json::Value::Bool(true));
    assert!(
        parsed["timings"]["tsMorphImportMs"].is_null(),
        "--no-load should not record import timing; got {parsed}"
    );
    assert!(
        parsed["timings"]["projectInitMs"].is_null(),
        "no tsconfig means no project-init timing; got {parsed}"
    );
}

#[test]
fn type_host_ping_without_ts_morph_surfaces_clear_error() {
    if !node_present() {
        return;
    }
    let tempdir = tempfile::tempdir().expect("create tempdir");
    // tempdir has no node_modules — ts-morph resolution must fail.
    let out = Command::new(cofferdam_bin())
        .args([
            "type-host",
            "--robot",
            "--project",
            tempdir.path().to_str().expect("utf8 tempdir"),
        ])
        .output()
        .expect("spawn cofferdam type-host");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "ping with no ts-morph must fail; stdout={stdout}"
    );
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("ping JSON parses");
    assert_eq!(parsed["ok"], serde_json::Value::Bool(false));
    let err = parsed["error"].as_str().expect("error field is a string");
    assert!(
        err.contains("ts_morph_unavailable"),
        "error must carry the ts_morph_unavailable wire code; got {err}"
    );
}
