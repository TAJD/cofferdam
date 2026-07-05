//! `cofferdam baseline ratchet` lowers a `[budgets]` entry to the current
//! finding count and rewrites `cofferdam.toml` in place. Its whole selling
//! point is a format-preserving edit (toml_edit) that leaves comments and
//! layout untouched — this cover asserts both the value change and that a
//! neighbouring comment survives.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

#[test]
fn ratchet_lowers_budget_and_preserves_comments() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    // A clean file yields zero Warning.TripleEquals findings, so the current
    // count for that key is a deterministic 0 — below the budget of 5.
    std::fs::write(dir.path().join("a.ts"), "export const a = 1;\n").expect("write ts");
    let config = "\
# top comment must survive
[budgets]
# keep me — inline rationale for the cap
\"Warning.TripleEquals\" = 5
";
    let config_path = dir.path().join("cofferdam.toml");
    std::fs::write(&config_path, config).expect("write config");

    let out = Command::new(cofferdam_bin())
        .args(["baseline", "ratchet"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    assert!(
        out.status.success(),
        "ratchet should exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let rewritten = std::fs::read_to_string(&config_path).expect("read config back");
    assert!(
        rewritten.contains("\"Warning.TripleEquals\" = 0"),
        "budget must be ratcheted 5 -> 0; got:\n{rewritten}"
    );
    assert!(
        rewritten.contains("# top comment must survive")
            && rewritten.contains("# keep me — inline rationale for the cap"),
        "comments must be preserved by the format-preserving rewrite; got:\n{rewritten}"
    );
}

#[test]
fn ratchet_dry_run_does_not_write() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("a.ts"), "export const a = 1;\n").expect("write ts");
    let config = "[budgets]\n\"Warning.TripleEquals\" = 5\n";
    let config_path = dir.path().join("cofferdam.toml");
    std::fs::write(&config_path, config).expect("write config");

    let out = Command::new(cofferdam_bin())
        .args(["baseline", "ratchet", "--dry-run"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    assert!(out.status.success(), "dry-run should exit 0");

    let after = std::fs::read_to_string(&config_path).expect("read config back");
    assert_eq!(after, config, "--dry-run must not modify the file");
}
