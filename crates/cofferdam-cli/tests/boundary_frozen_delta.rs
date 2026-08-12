//! CD-304: `Design.BoundaryFrozen`'s docs promised delta enforcement
//! "under a follow-up bead", as though nothing today could tell a new
//! addition to a frozen area from the files already in it. Two shipped
//! mechanisms already do: `--since` scopes findings to the diff, and
//! `--baseline` records the existing ones. These tests guard the recipe
//! the docs now name, so the promise and the behaviour move together.
//!
//! Requires `git` on PATH (same as `orphan_since_report_scope.rs`).

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

/// A repo with `src/legacy/**` frozen, holding two files in the boundary
/// and one outside it, all committed.
fn frozen_repo(dir: &Path) {
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
    std::fs::create_dir_all(dir.join("src").join("app")).expect("mkdir app");
    for (path, body) in [
        ("src/legacy/old_a.ts", "export const a = 1;\n"),
        ("src/legacy/old_b.ts", "export const b = 2;\n"),
        ("src/app/live.ts", "export const live = 3;\n"),
    ] {
        std::fs::write(dir.join(path), body).expect("write source");
    }
    commit_all(dir, "baseline");
}

fn run_cofferdam(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cofferdam"));
    cmd.args(args).current_dir(dir);
    for var in GIT_ENV_VARS_TO_CLEAR {
        cmd.env_remove(var);
    }
    cmd.output().expect("spawn cofferdam")
}

/// Every `Design.BoundaryFrozen` finding as (forward-slash path,
/// baselined).
fn frozen_findings(dir: &Path, extra: &[&str]) -> Vec<(String, bool)> {
    let mut args = vec!["check", "--format=json", "--only", "Design.BoundaryFrozen"];
    args.extend_from_slice(extra);
    let out = run_cofferdam(dir, &args);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "stdout not valid JSON: {e}\nstdout={stdout}\nstderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    v["findings"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|f| {
                    (
                        f["file"].as_str().unwrap_or_default().replace('\\', "/"),
                        f["baselined"].as_bool().unwrap_or(false),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Files flagged by `Design.BoundaryFrozen`, as forward-slash paths.
fn frozen_files(dir: &Path, extra: &[&str]) -> Vec<String> {
    frozen_findings(dir, extra)
        .into_iter()
        .map(|(file, _)| file)
        .collect()
}

fn any_ends_with(paths: &[String], suffix: &str) -> bool {
    paths.iter().any(|p| p.ends_with(suffix))
}

#[test]
fn a_whole_project_run_names_every_file_in_the_frozen_boundary() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    frozen_repo(dir);

    let flagged = frozen_files(dir, &["--no-baseline"]);
    assert!(
        any_ends_with(&flagged, "src/legacy/old_a.ts"),
        "{flagged:?}"
    );
    assert!(
        any_ends_with(&flagged, "src/legacy/old_b.ts"),
        "{flagged:?}"
    );
    assert!(
        !any_ends_with(&flagged, "src/app/live.ts"),
        "a file outside the boundary must not be flagged; {flagged:?}"
    );
}

/// The delta recipe. A commit that adds one file to the frozen area and
/// leaves the rest alone must produce exactly one finding — that file.
#[test]
fn since_narrows_the_boundary_to_what_the_commit_touched() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    frozen_repo(dir);

    std::fs::write(
        dir.join("src").join("legacy").join("new_c.ts"),
        "export const c = 4;\n",
    )
    .expect("write new file");
    commit_all(dir, "add to the frozen area");

    let flagged = frozen_files(dir, &["--no-baseline", "--since", "HEAD~1"]);
    assert!(
        any_ends_with(&flagged, "src/legacy/new_c.ts"),
        "the file this commit added to the frozen area must be flagged; {flagged:?}"
    );
    assert!(
        !any_ends_with(&flagged, "src/legacy/old_a.ts"),
        "an untouched file already in the boundary must not be re-reported; {flagged:?}"
    );
    assert_eq!(flagged.len(), 1, "{flagged:?}");
}

/// Editing a file already inside the boundary is a change to a frozen
/// area, so `--since` reports it. "Frozen" means no new code here, not
/// merely no new files.
#[test]
fn since_reports_an_edit_to_a_file_already_inside_the_boundary() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    frozen_repo(dir);

    std::fs::write(
        dir.join("src").join("legacy").join("old_a.ts"),
        "export const a = 99;\n",
    )
    .expect("edit legacy file");
    commit_all(dir, "edit the frozen area");

    let flagged = frozen_files(dir, &["--no-baseline", "--since", "HEAD~1"]);
    assert_eq!(flagged.len(), 1, "{flagged:?}");
    assert!(
        any_ends_with(&flagged, "src/legacy/old_a.ts"),
        "{flagged:?}"
    );
}

/// A commit that leaves the frozen area alone reports nothing, so the
/// recipe can gate CI without failing every unrelated pull request.
#[test]
fn since_stays_silent_when_the_commit_avoids_the_boundary() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    frozen_repo(dir);

    std::fs::write(
        dir.join("src").join("app").join("live.ts"),
        "export const live = 4;\n",
    )
    .expect("edit live file");
    commit_all(dir, "work outside the frozen area");

    let flagged = frozen_files(dir, &["--no-baseline", "--since", "HEAD~1"]);
    assert!(flagged.is_empty(), "{flagged:?}");
}

/// The other half of the recipe. A baseline written today accepts the
/// files already inside the boundary; only a later addition is left
/// unmatched, and only unmatched findings trip `--fail-on`.
///
/// The assertion reads the `baselined` flag rather than passing
/// `--hide-baselined`, which the JSON formatter ignores (CD-315).
#[test]
fn a_baseline_accepts_the_existing_files_and_leaves_the_new_one_visible() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    frozen_repo(dir);

    let out = run_cofferdam(dir, &["baseline", "write", "--output", "baseline.json"]);
    assert!(
        out.status.success(),
        "baseline write should exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    std::fs::write(
        dir.join("src").join("legacy").join("new_c.ts"),
        "export const c = 4;\n",
    )
    .expect("write new file");

    let findings = frozen_findings(dir, &["--baseline", "baseline.json"]);
    let unmatched: Vec<String> = findings
        .iter()
        .filter(|(_, baselined)| !baselined)
        .map(|(file, _)| file.clone())
        .collect();
    assert_eq!(unmatched.len(), 1, "{findings:?}");
    assert!(
        any_ends_with(&unmatched, "src/legacy/new_c.ts"),
        "{findings:?}"
    );
    assert_eq!(findings.len(), 3, "the boundary still holds three files");
}
