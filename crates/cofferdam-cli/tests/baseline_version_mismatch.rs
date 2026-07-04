//! Regression cover for CD-42: a baseline written under an older/newer
//! schema version must hard-fail `cofferdam check` with exit code 2 and
//! a prominent stderr error, not silently fall through to reporting every
//! previously-accepted finding as new.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

#[test]
fn stale_schema_baseline_hard_fails_check() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("a.ts"), "export const a = 1;\n").expect("write ts");

    let baseline_dir = dir.path().join(".cofferdam");
    std::fs::create_dir_all(&baseline_dir).expect("create .cofferdam dir");
    // version 0 is not a schema this binary supports.
    std::fs::write(
        baseline_dir.join("baseline.json"),
        r#"{"version":0,"findings":[]}"#,
    )
    .expect("write stale baseline");

    let out = Command::new(cofferdam_bin())
        .arg("check")
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(2),
        "stale-schema baseline must hard-fail with exit code 2; stderr={stderr}"
    );
    assert!(
        stderr.contains("unsupported version") && stderr.contains("cofferdam baseline write"),
        "stderr should name the fix command; stderr={stderr}"
    );
}

#[test]
fn missing_baseline_is_not_a_hard_failure() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("a.ts"), "export const a = 1;\n").expect("write ts");

    let out = Command::new(cofferdam_bin())
        .arg("check")
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");

    assert_ne!(
        out.status.code(),
        Some(2),
        "a missing baseline must not be treated as a schema hard failure"
    );
}
