//! CD-311: an unknown key in `cofferdam.toml` used to parse cleanly and
//! do nothing. A wrong flag is rejected by clap; a wrong key was skipped
//! by serde, so the user got a green run and a rule that never fired —
//! and two docs pages spent months promising an `exclude` key that no
//! code has ever read.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

fn check_with_config(toml: &str) -> std::process::Output {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::create_dir_all(dir.path().join(".git")).expect("seal");
    std::fs::write(dir.path().join("cofferdam.toml"), toml).expect("write config");
    std::fs::write(dir.path().join("a.ts"), "export const a = 1;\n").expect("write ts");

    Command::new(cofferdam_bin())
        // `--fail-on=critical` so the exit code reflects config handling
        // alone; the fixture trips ordinary findings that would otherwise
        // gate the run and mask what these tests are asserting.
        .args([
            "check",
            "--no-baseline",
            "--format",
            "json",
            "--fail-on",
            "critical",
        ])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam")
}

#[test]
fn an_unknown_key_is_reported_with_the_answer() {
    let out = check_with_config("exclude = [\"dist/**\"]\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown key `exclude`"),
        "the key the docs once promised must be named; stderr={stderr}"
    );
    assert!(
        stderr.contains(".cofferdamignore"),
        "a key we have seen invented deserves the answer, not just a rejection; stderr={stderr}"
    );
}

/// Warn, do not reject. Rejecting outright would break every existing
/// config carrying a stray key, and the dangerous half of the problem is
/// the silence rather than the acceptance.
#[test]
fn an_unknown_key_does_not_fail_the_run() {
    let out = check_with_config("exclude = [\"dist/**\"]\n");
    assert_eq!(
        out.status.code(),
        Some(0),
        "an unknown key is a warning, not a build failure"
    );
}

#[test]
fn a_valid_config_warns_about_nothing() {
    let out = check_with_config(
        "[checks.\"Readability.MaxLineLength\"]\nlimit = 120\n\n[engine]\ntype_aware = false\n",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("unknown key"),
        "a correctly-configured project must see no key warnings; stderr={stderr}"
    );
}

/// Per-check option names come from each check's own schema, and layer
/// and budget keys are user-chosen. Reporting those would fire on every
/// correct project, which is how a warning gets ignored.
#[test]
fn user_named_keys_are_not_reported() {
    let out = check_with_config(
        "[layers]\nanything = [\"src/**\"]\n\n[budgets]\n\"Warning.NoConsoleLog\" = 5\n",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("unknown key"), "stderr={stderr}");
}
