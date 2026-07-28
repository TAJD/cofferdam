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

/// When --fail-on-type-unavailable is passed and a type-aware check is
/// registered but the oracle can't start (no tsconfig near the temp dir),
/// cofferdam check must exit with code 2. Does NOT require Node on PATH —
/// the no-tsconfig path is short-circuited before Node is ever spawned.
#[test]
fn check_fail_on_type_unavailable_exits_2_when_oracle_missing() {
    let dir = tempfile::tempdir().expect("create tempdir");
    std::fs::write(dir.path().join("a.ts"), b"const x: string | null = null;\n")
        .expect("write a.ts");
    let out = Command::new(cofferdam_bin())
        .args(["check", "--fail-on-type-unavailable", "--no-cache"])
        .arg(dir.path())
        .output()
        .expect("spawn cofferdam check");
    assert_eq!(
        out.status.code(),
        Some(2),
        "--fail-on-type-unavailable should exit 2 when oracle unavailable; \
         stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// CD-151: tsconfig.json discovery must anchor on the analyzed target,
/// not the invoking process's CWD. Before the fix, `find_tsconfig` was
/// always called with a CWD-derived root, so `cofferdam check <target>`
/// run from outside <target>'s tree reported "no tsconfig.json found
/// near <cwd>" even when <target> has its own valid tsconfig.json.
/// Doesn't require Node — the symptom (the wrong search root) shows up
/// in the warning message itself, before any ts-morph spawn is
/// attempted.
#[test]
fn check_finds_tsconfig_anchored_on_target_not_cwd() {
    let unrelated_cwd = tempfile::tempdir().expect("create unrelated cwd");
    let project = tempfile::tempdir().expect("create project dir");
    std::fs::write(project.path().join("tsconfig.json"), b"{}\n").expect("write tsconfig.json");
    std::fs::write(
        project.path().join("a.ts"),
        b"const x: string | null = null;\n",
    )
    .expect("write a.ts");

    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-cache"])
        .arg(project.path())
        .current_dir(unrelated_cwd.path())
        .output()
        .expect("spawn cofferdam check");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("no tsconfig.json found near"),
        "tsconfig discovery must anchor on the analyzed target, not the CWD; stderr={stderr}"
    );
}

/// Without --fail-on-type-unavailable the oracle failure is a warning only.
/// The process must NOT exit with code 2 on oracle unavailability alone.
#[test]
fn check_without_fail_flag_does_not_exit_2_when_oracle_missing() {
    let dir = tempfile::tempdir().expect("create tempdir");
    std::fs::write(dir.path().join("a.ts"), b"const x: string | null = null;\n")
        .expect("write a.ts");
    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-cache"])
        .arg(dir.path())
        .output()
        .expect("spawn cofferdam check");
    assert_ne!(
        out.status.code(),
        Some(2),
        "without --fail-on-type-unavailable, oracle failure must not exit 2; \
         stdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
