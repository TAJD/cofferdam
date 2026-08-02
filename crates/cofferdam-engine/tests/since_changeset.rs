use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

use cofferdam_engine::since::{diff_changeset, DiffMode};

fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo(dir: &Path) {
    run_git(dir, &["init", "--quiet", "-b", "main"]);
    run_git(dir, &["config", "user.email", "t@example.com"]);
    run_git(dir, &["config", "user.name", "t"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
    run_git(dir, &["config", "core.hooksPath", "/dev/null"]);
}

#[test]
fn working_tree_mode_sees_staged_and_unstaged() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());
    std::fs::write(tmp.path().join("a.ts"), "const a = 1;\n").unwrap();
    std::fs::write(tmp.path().join("b.ts"), "const b = 1;\n").unwrap();
    run_git(tmp.path(), &["add", "."]);
    run_git(tmp.path(), &["commit", "-qm", "init"]);
    // staged edit to a.ts, unstaged edit to b.ts
    std::fs::write(tmp.path().join("a.ts"), "const a = 2;\n").unwrap();
    run_git(tmp.path(), &["add", "a.ts"]);
    std::fs::write(tmp.path().join("b.ts"), "const b = 2;\n").unwrap();

    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let cs = diff_changeset(&root, &DiffMode::WorkingTree).unwrap();
    assert!(cs.files.iter().any(|p| p.ends_with("a.ts")));
    assert!(cs.files.iter().any(|p| p.ends_with("b.ts")));
}

#[test]
fn staged_mode_sees_only_staged() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());
    std::fs::write(tmp.path().join("a.ts"), "const a = 1;\n").unwrap();
    std::fs::write(tmp.path().join("b.ts"), "const b = 1;\n").unwrap();
    run_git(tmp.path(), &["add", "."]);
    run_git(tmp.path(), &["commit", "-qm", "init"]);
    std::fs::write(tmp.path().join("a.ts"), "const a = 2;\n").unwrap();
    run_git(tmp.path(), &["add", "a.ts"]);
    std::fs::write(tmp.path().join("b.ts"), "const b = 2;\n").unwrap();

    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let cs = diff_changeset(&root, &DiffMode::Staged).unwrap();
    assert!(cs.files.iter().any(|p| p.ends_with("a.ts")));
    assert!(!cs.files.iter().any(|p| p.ends_with("b.ts")));
}

#[test]
fn base_mode_diffs_against_merge_base_and_includes_working_tree() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());
    std::fs::write(tmp.path().join("a.ts"), "const a = 1;\n").unwrap();
    run_git(tmp.path(), &["add", "."]);
    run_git(tmp.path(), &["commit", "-qm", "init"]);
    run_git(tmp.path(), &["checkout", "-qb", "feature"]);
    std::fs::write(tmp.path().join("c.ts"), "const c = 1;\n").unwrap();
    run_git(tmp.path(), &["add", "."]);
    run_git(tmp.path(), &["commit", "-qm", "branch work"]);
    std::fs::write(tmp.path().join("c.ts"), "const c = 2;\n").unwrap(); // uncommitted on top

    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let cs = diff_changeset(&root, &DiffMode::Base("main".into())).unwrap();
    assert!(cs.files.iter().any(|p| p.ends_with("c.ts")));
    assert!(!cs.files.iter().any(|p| p.ends_with("a.ts")));
    // post-image range reflects the working-tree content
    let c_path = cs.files.iter().find(|p| p.ends_with("c.ts")).unwrap();
    assert!(!cs.line_ranges[c_path].is_empty());
}

#[test]
fn bad_ref_is_an_error() {
    let tmp = TempDir::new().unwrap();
    init_repo(tmp.path());
    std::fs::write(tmp.path().join("a.ts"), "x\n").unwrap();
    run_git(tmp.path(), &["add", "."]);
    run_git(tmp.path(), &["commit", "-qm", "init"]);
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    assert!(diff_changeset(&root, &DiffMode::Base("no-such-ref".into())).is_err());
}
