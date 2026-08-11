//! CD-315: `--hide-baselined` was honoured by the text formatter alone —
//! `--format=json` (and compact/sarif) printed baselined findings anyway.
//! These tests exercise all four formats against the same baselined +
//! new finding shape and check: the baselined finding is absent from
//! the rendered output, the new finding still renders, and (for the
//! formats that carry summary counts — json, text) the counts are
//! unchanged by the flag.
//!
//! Repo/CLI-invocation helpers mirror `boundary_frozen_delta.rs`.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

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

fn run_cofferdam(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cofferdam"));
    cmd.args(args).current_dir(dir);
    for var in GIT_ENV_VARS_TO_CLEAR {
        cmd.env_remove(var);
    }
    cmd.output().expect("spawn cofferdam")
}

/// One frozen file (`old_a.ts`, baselined by the time the tests run
/// `check`) plus one file outside the boundary that's added *after* the
/// baseline is written, so it always shows up as a new, unbaselined
/// `Design.BoundaryFrozen` finding.
fn repo_with_one_baselined_and_one_new_finding(dir: &Path) {
    run_git(dir, &["init", "--quiet", "-b", "main"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "Cofferdam Test"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);

    std::fs::write(
        dir.join("cofferdam.invariants.toml"),
        "[boundaries]\n\"src/legacy/**\" = { frozen = true, reason = \"see ADR-0007\" }\n",
    )
    .expect("write invariants");
    std::fs::create_dir_all(dir.join("src").join("legacy")).expect("mkdir legacy");
    std::fs::write(
        dir.join("src").join("legacy").join("old_a.ts"),
        "export const a = 1;\n",
    )
    .expect("write old_a.ts");
    commit_all(dir, "baseline");

    let write_out = run_cofferdam(dir, &["baseline", "write", "--output", "baseline.json"]);
    assert!(
        write_out.status.success(),
        "baseline write should exit 0; stderr={}",
        String::from_utf8_lossy(&write_out.stderr)
    );

    std::fs::write(
        dir.join("src").join("legacy").join("new_b.ts"),
        "export const b = 2;\n",
    )
    .expect("write new_b.ts");
}

fn check(dir: &Path, format: &str, hide: bool) -> String {
    let mut args = vec![
        "check",
        "--baseline",
        "baseline.json",
        "--only",
        "Design.BoundaryFrozen",
        "--format",
        format,
    ];
    if hide {
        args.push("--hide-baselined");
    }
    let out = run_cofferdam(dir, &args);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn json_hide_baselined_drops_the_entry_but_keeps_summary_counts() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    repo_with_one_baselined_and_one_new_finding(dir);

    let shown = check(dir, "json", false);
    let hidden = check(dir, "json", true);

    let shown_v: serde_json::Value = serde_json::from_str(&shown).expect("valid JSON");
    let hidden_v: serde_json::Value = serde_json::from_str(&hidden).expect("valid JSON");

    assert_eq!(
        shown_v["findings"].as_array().unwrap().len(),
        2,
        "without --hide-baselined both findings render; got: {shown}"
    );
    let hidden_findings = hidden_v["findings"].as_array().unwrap();
    assert_eq!(
        hidden_findings.len(),
        1,
        "--hide-baselined must drop the baselined finding from `findings`; got: {hidden}"
    );
    assert!(
        hidden_findings[0]["file"]
            .as_str()
            .unwrap()
            .ends_with("new_b.ts"),
        "the surviving finding must be the new one; got: {hidden}"
    );
    assert!(
        !hidden_findings
            .iter()
            .any(|f| f["baselined"].as_bool() == Some(true)),
        "no baselined finding should remain; got: {hidden}"
    );

    assert_eq!(
        shown_v["summary"], hidden_v["summary"],
        "summary must be identical with and without --hide-baselined; shown={shown} hidden={hidden}"
    );
    assert_eq!(hidden_v["summary"]["total"], 2);
    assert_eq!(hidden_v["summary"]["new"], 1);
    assert_eq!(hidden_v["summary"]["baselined"], 1);
}

#[test]
fn text_hide_baselined_drops_the_entry_but_keeps_summary_counts() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    repo_with_one_baselined_and_one_new_finding(dir);

    let shown = check(dir, "text", false);
    let hidden = check(dir, "text", true);

    assert!(shown.contains("old_a.ts"), "got: {shown}");
    assert!(shown.contains("new_b.ts"), "got: {shown}");

    assert!(
        !hidden.contains("old_a.ts"),
        "the baselined finding must not render; got: {hidden}"
    );
    assert!(
        hidden.contains("new_b.ts"),
        "the new finding must still render; got: {hidden}"
    );
    assert!(
        hidden.contains("2 finding(s) (1 new, 1 baselined"),
        "the summary line must still report the full pre-filter counts; got: {hidden}"
    );
}

#[test]
fn compact_hide_baselined_drops_the_baselined_record() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    repo_with_one_baselined_and_one_new_finding(dir);

    let shown = check(dir, "compact", false);
    let hidden = check(dir, "compact", true);

    assert!(shown.contains("old_a.ts"), "got: {shown}");
    assert!(shown.contains("new_b.ts"), "got: {shown}");

    assert!(
        !hidden.contains("old_a.ts"),
        "the baselined finding must not render; got: {hidden}"
    );
    assert!(
        hidden.contains("new_b.ts"),
        "the new finding must still render; got: {hidden}"
    );
    // Header + exactly one record line.
    assert_eq!(hidden.lines().count(), 2, "got: {hidden}");
}

#[test]
fn sarif_hide_baselined_drops_the_baselined_result() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    repo_with_one_baselined_and_one_new_finding(dir);

    let shown = check(dir, "sarif", false);
    let hidden = check(dir, "sarif", true);

    let shown_v: serde_json::Value = serde_json::from_str(&shown).expect("valid SARIF/JSON");
    let hidden_v: serde_json::Value = serde_json::from_str(&hidden).expect("valid SARIF/JSON");

    assert_eq!(shown_v["runs"][0]["results"].as_array().unwrap().len(), 2);
    let hidden_results = hidden_v["runs"][0]["results"].as_array().unwrap();
    assert_eq!(
        hidden_results.len(),
        1,
        "--hide-baselined must drop the baselined result; got: {hidden}"
    );
    assert!(
        hidden_results[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .unwrap()
            .ends_with("new_b.ts"),
        "the surviving result must be the new finding; got: {hidden}"
    );
}
