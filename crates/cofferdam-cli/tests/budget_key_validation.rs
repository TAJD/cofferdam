//! A `[budgets]` key that matches no known check id or category resolves to a
//! count of 0 and silently never fires — a typo defeats the very gate it was
//! meant to be. `cofferdam check` must warn on such keys; a valid key must not
//! trip the warning.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

fn run_check_with_budget(budget_line: &str) -> std::process::Output {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("a.ts"), "export const a = 1;\n").expect("write ts");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        format!("[budgets]\n{budget_line}\n"),
    )
    .expect("write config");

    Command::new(cofferdam_bin())
        .arg("check")
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam")
}

#[test]
fn unknown_budget_key_warns() {
    let out = run_check_with_budget(r#""Refactor.CognitiveComplexty" = 5"#);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Refactor.CognitiveComplexty") && stderr.contains("no known check id"),
        "a typo'd budget key must warn; stderr={stderr}"
    );
}

#[test]
fn valid_budget_key_does_not_warn() {
    let out = run_check_with_budget(r#""Refactor.NearDuplicateBlock" = 5"#);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("no known check id"),
        "a valid check-id budget key must not warn; stderr={stderr}"
    );

    let out = run_check_with_budget("Refactor = 100");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("no known check id"),
        "a valid category budget key must not warn; stderr={stderr}"
    );
}
