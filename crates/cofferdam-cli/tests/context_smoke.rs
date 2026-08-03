//! Integration tests for `cofferdam context`.
//!
//! `Context.Findings` (CD-159) is the first registered provider; these
//! tests assert the invocation contract (exit codes, changeset
//! resolution modes, JSON shape) using fixtures deliberately chosen not
//! to trip any builtin check (e.g. non-exported bindings, so
//! `Design.OrphanExport` doesn't fire), so the assertions stay about the
//! contract rather than any particular provider's output.

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
    // Non-exported binding: nothing for a builtin check (e.g.
    // `Design.OrphanExport`) to flag, so the digest stays empty and this
    // test can assert on JSON shape alone, independent of provider output.
    let file = dir.join("a.ts");
    std::fs::write(&file, "const x = 1;\n").expect("write");
    commit_all(dir, "init");
    std::fs::write(&file, "const x = 2;\n").expect("edit");

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

/// CD-202: explicit-path mode must resolve the same absolute paths the
/// engine's discovery pass produces, and must discover from the repo
/// root (not cwd) so the cross-file graph the `Context.BlastRadius`
/// provider needs is actually built. Regression test: before the fix,
/// `std::fs::canonicalize` on Windows produced `\\?\`-prefixed paths
/// that never equalled the (unprefixed) discovered paths, so explicit-
/// path mode silently returned zero items while git-diff mode — same
/// file, same repo — returned the real digest.
#[test]
fn context_explicit_path_matches_git_diff_mode_on_blast_radius_fixture() {
    let tmp = TempDir::new().expect("temp dir");
    let dir = tmp.path();
    init_repo(dir);

    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join("blast_radius");
    for entry in std::fs::read_dir(&fixture_dir).expect("read fixture dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name();
        std::fs::copy(entry.path(), dir.join(&name)).expect("copy fixture file");
    }
    commit_all(dir, "init");

    // Synthetic signature change to `doThing`, matching the blast-radius
    // fixture's documented scenario (see examples/blast_radius/lib.ts).
    std::fs::write(
        dir.join("lib.ts"),
        "export function doThing(x: number, y: number): string {\n  return String(x + y);\n}\n",
    )
    .expect("edit lib.ts");

    let git_diff_out = cofferdam_cmd(dir)
        .args(["context", "--format", "json"])
        .output()
        .expect("invoke cofferdam (git-diff mode)");
    assert!(
        git_diff_out.status.success(),
        "git-diff mode: expected exit 0; stderr={}",
        String::from_utf8_lossy(&git_diff_out.stderr)
    );

    let explicit_out = cofferdam_cmd(dir)
        .args(["context", "lib.ts", "--format", "json"])
        .output()
        .expect("invoke cofferdam (explicit-path mode)");
    assert!(
        explicit_out.status.success(),
        "explicit-path mode: expected exit 0; stderr={}",
        String::from_utf8_lossy(&explicit_out.stderr)
    );

    let git_diff_json: serde_json::Value =
        serde_json::from_slice(&git_diff_out.stdout).expect("valid JSON (git-diff mode)");
    let explicit_json: serde_json::Value =
        serde_json::from_slice(&explicit_out.stdout).expect("valid JSON (explicit-path mode)");

    let git_diff_items = git_diff_json["items"].as_array().expect("items array");
    assert!(
        !git_diff_items.is_empty(),
        "expected non-empty items in git-diff mode; got {git_diff_json}"
    );
    assert_eq!(
        git_diff_json["items"], explicit_json["items"],
        "explicit-path mode must yield the same items as git-diff mode for the same change"
    );
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
