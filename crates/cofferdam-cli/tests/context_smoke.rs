//! Integration tests for `cofferdam context`.
//!
//! No Context providers are registered yet (CD-157/CD-158 is the
//! foundations checkpoint; providers land in CP3-CP7), so every digest
//! is honestly empty — these tests assert the invocation contract
//! (exit codes, changeset resolution modes, JSON shape) rather than
//! any provider output.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn init_repo(dir: &Path) {
    run_git(dir, &["init", "--quiet", "-b", "main"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "Cofferdam Test"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
}

const GIT_ENV_VARS_TO_CLEAR: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_PREFIX",
];

fn run_git(dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    for var in GIT_ENV_VARS_TO_CLEAR {
        cmd.env_remove(var);
    }
    let out = cmd.output().unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit_all(dir: &Path, message: &str) {
    run_git(dir, &["add", "-A"]);
    run_git(dir, &["commit", "--quiet", "--no-verify", "-m", message]);
}

fn cofferdam_bin() -> &'static str {
    env!("CARGO_BIN_EXE_cofferdam")
}

fn cofferdam_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new(cofferdam_bin());
    cmd.current_dir(dir);
    for var in GIT_ENV_VARS_TO_CLEAR {
        cmd.env_remove(var);
    }
    cmd
}

#[test]
fn context_on_clean_tree_reports_no_changes_and_exits_zero() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    std::fs::write(dir.join("a.ts"), "export const x = 1;\n").expect("write");
    commit_all(dir, "init");

    let out = cofferdam_cmd(dir)
        .args(["context"])
        .output()
        .expect("invoke cofferdam");

    assert!(
        out.status.success(),
        "expected exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("No relevant context found for 0 changed file(s)."),
        "got: {stdout}"
    );
}

#[test]
fn context_with_working_tree_edit_exits_zero_with_digest_or_empty_message() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    let file = dir.join("a.ts");
    std::fs::write(&file, "export const x = 1;\n").expect("write");
    commit_all(dir, "init");
    std::fs::write(&file, "export const x = 2;\n").expect("edit");

    let out = cofferdam_cmd(dir)
        .args(["context"])
        .output()
        .expect("invoke cofferdam");

    assert!(
        out.status.success(),
        "expected exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("changed file(s)"), "got: {stdout}");
}

#[test]
fn context_json_shape_is_stable() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    let file = dir.join("a.ts");
    std::fs::write(&file, "export const x = 1;\n").expect("write");
    commit_all(dir, "init");
    std::fs::write(&file, "export const x = 2;\n").expect("edit");

    let out = cofferdam_cmd(dir)
        .args(["context", "--format", "json"])
        .output()
        .expect("invoke cofferdam");

    assert!(
        out.status.success(),
        "expected exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["changed_files"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["items"].as_array().unwrap().len(), 0);
    assert_eq!(parsed["omitted"], 0);
    assert_eq!(parsed["budget"], 2000);
}

#[test]
fn context_explicit_files_work_without_git() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    std::fs::write(dir.join("a.ts"), "export const x = 1;\n").expect("write");

    let out = cofferdam_cmd(dir)
        .args(["context", "a.ts"])
        .output()
        .expect("invoke cofferdam");

    assert!(
        out.status.success(),
        "expected exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1 changed file(s)"), "got: {stdout}");
}

#[test]
fn context_bad_base_ref_is_a_usage_error() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);
    std::fs::write(dir.join("a.ts"), "export const x = 1;\n").expect("write");
    commit_all(dir, "init");

    let out = cofferdam_cmd(dir)
        .args(["context", "--base", "no-such-ref"])
        .output()
        .expect("invoke cofferdam");

    assert!(!out.status.success(), "expected nonzero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.is_empty(), "expected a diagnostic on stderr");
}
