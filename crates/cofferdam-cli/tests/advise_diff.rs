//! Integration tests for `cofferdam advise --diff <ref>`.
//!
//! These tests build a real git repo in a temp dir, commit a clean
//! source file, modify it in the working tree to introduce (or clear)
//! check violations, then shell out to the cofferdam binary and assert
//! the JSON shape. They depend on `git` being on PATH — the same
//! requirement as `cofferdam check --since`.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Initialise a fresh git repo in `dir`, configure a local user (so
/// `git commit` doesn't bail on author detection), set a stable
/// initial branch, and disable hooks/signing so the harness behaves
/// the same on every machine.
fn init_repo(dir: &Path) {
    run_git(dir, &["init", "--quiet", "-b", "main"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "Cofferdam Test"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
}

/// GIT_* vars git sets when this test binary is itself invoked from a
/// git hook (e.g. `cargo test` under `pre-push`). Left in place, they
/// redirect this helper's `git init`/`git commit` at the *real* repo
/// (or its worktree common dir) instead of the fresh temp dir.
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

/// `cofferdam advise --diff` shells out to `git show <ref>:<path>`
/// internally, so it needs the same env isolation as `run_git` above.
fn cofferdam_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new(cofferdam_bin());
    cmd.current_dir(dir);
    for var in GIT_ENV_VARS_TO_CLEAR {
        cmd.env_remove(var);
    }
    cmd
}

#[test]
fn diff_introduces_violation_shows_in_would_fire() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);

    // Baseline: clean source with strict equality — passes
    // `Warning.TripleEquals`.
    let file = dir.join("src.ts");
    std::fs::write(
        &file,
        "export function eq(a: number, b: number) { return a === b; }\n",
    )
    .expect("write baseline");
    commit_all(dir, "baseline");

    // Working-tree edit: switch to loose equality, which the check flags.
    std::fs::write(
        &file,
        "export function eq(a: number, b: number) { return a == b; }\n",
    )
    .expect("write modified");

    let out = cofferdam_cmd(dir)
        .args(["advise", "--diff", "HEAD"])
        .output()
        .expect("invoke cofferdam");

    assert!(
        out.status.success(),
        "cofferdam exit non-zero ({:?}): stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"check_id\":\"Warning.TripleEquals\""),
        "expected TripleEquals in would_fire, got: {stdout}"
    );
    assert!(
        stdout.contains("\"would_fire\":1"),
        "expected would_fire summary count of 1, got: {stdout}"
    );
    assert!(
        stdout.contains("\"would_clear\":0"),
        "expected would_clear summary count of 0, got: {stdout}"
    );
}

#[test]
fn diff_clears_violation_shows_in_would_clear() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);

    // Baseline: dirty source with loose equality.
    let file = dir.join("src.ts");
    std::fs::write(
        &file,
        "export function eq(a: number, b: number) { return a == b; }\n",
    )
    .expect("write baseline");
    commit_all(dir, "baseline");

    // Working-tree edit: clears the violation.
    std::fs::write(
        &file,
        "export function eq(a: number, b: number) { return a === b; }\n",
    )
    .expect("write modified");

    let out = cofferdam_cmd(dir)
        .args(["advise", "--diff", "HEAD"])
        .output()
        .expect("invoke cofferdam");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "cofferdam exit non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("\"would_fire\":0"),
        "expected would_fire summary count of 0, got: {stdout}"
    );
    assert!(
        stdout.contains("\"would_clear\":1"),
        "expected would_clear summary count of 1, got: {stdout}"
    );
    assert!(
        stdout.contains("\"check_id\":\"Warning.TripleEquals\""),
        "expected TripleEquals in would_clear, got: {stdout}"
    );
}

#[test]
fn diff_no_changes_emits_empty_report() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);

    let file = dir.join("src.ts");
    std::fs::write(&file, "export const x = 1;\n").expect("write baseline");
    commit_all(dir, "baseline");

    let out = cofferdam_cmd(dir)
        .args(["advise", "--diff", "HEAD"])
        .output()
        .expect("invoke cofferdam");

    assert!(out.status.success(), "cofferdam exit non-zero");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"would_fire\":0"), "got: {stdout}");
    assert!(stdout.contains("\"would_clear\":0"), "got: {stdout}");
}

#[test]
fn fail_on_medium_exits_one_when_would_fire_at_or_above() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);

    let file = dir.join("src.ts");
    std::fs::write(
        &file,
        "export function eq(a: number, b: number) { return a === b; }\n",
    )
    .expect("write baseline");
    commit_all(dir, "baseline");
    std::fs::write(
        &file,
        "export function eq(a: number, b: number) { return a == b; }\n",
    )
    .expect("write modified");

    // Default fail_on threshold is unset → exit 0 even with would_fire.
    let no_gate = cofferdam_cmd(dir)
        .args(["advise", "--diff", "HEAD"])
        .output()
        .expect("invoke cofferdam");
    assert!(
        no_gate.status.success(),
        "expected exit 0 without --fail-on; stderr={}",
        String::from_utf8_lossy(&no_gate.stderr)
    );

    // With --fail-on=medium and Warning.TripleEquals at Medium, exit 1.
    let gated = cofferdam_cmd(dir)
        .args(["advise", "--diff", "HEAD", "--fail-on", "medium"])
        .output()
        .expect("invoke cofferdam");
    assert_eq!(
        gated.status.code(),
        Some(1),
        "expected exit 1 with --fail-on=medium; stdout={} stderr={}",
        String::from_utf8_lossy(&gated.stdout),
        String::from_utf8_lossy(&gated.stderr),
    );

    // With --fail-on=critical, the Medium finding is below threshold → exit 0.
    let gated_high = cofferdam_cmd(dir)
        .args(["advise", "--diff", "HEAD", "--fail-on", "critical"])
        .output()
        .expect("invoke cofferdam");
    assert!(
        gated_high.status.success(),
        "expected exit 0 with --fail-on=critical (above Medium); stdout={}",
        String::from_utf8_lossy(&gated_high.stdout)
    );
}
