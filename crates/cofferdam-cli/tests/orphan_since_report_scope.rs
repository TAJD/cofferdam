//! Regression tests for GH #53 — `--since` used to build a partial
//! cross-file graph, causing false `Design.OrphanExport` positives on
//! exports whose only consumer lived in an unchanged file.
//!
//! Fix: the engine always analyses the full discovered set; `--since`
//! now scopes *findings* (not the analysis file set) to changed files.
//!
//! These tests require `git` on PATH (same as `advise_diff.rs`).

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn init_repo(dir: &Path) {
    run_git(dir, &["init", "--quiet", "-b", "main"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "Cofferdam Test"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
}

fn run_git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
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

/// Run `cofferdam check --since HEAD~1 --no-baseline --format=json` in `repo`.
/// Returns the parsed JSON value and raw stdout.
fn run_since_check(repo: &Path) -> (serde_json::Value, String) {
    let out = Command::new(cofferdam_bin())
        .args([
            "check",
            "--since",
            "HEAD~1",
            "--no-baseline",
            "--format=json",
        ])
        .current_dir(repo)
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "cofferdam stdout not valid JSON: {e}\nstdout={stdout}\nstderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (v, stdout)
}

/// Collect the `file` field of every `Design.OrphanExport` finding.
fn orphan_files(v: &serde_json::Value) -> Vec<String> {
    v["findings"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|f| f["id"].as_str() == Some("Design.OrphanExport"))
                .filter_map(|f| f["file"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn any_ends_with(paths: &[String], suffix: &str) -> bool {
    paths.iter().any(|p| p.replace('\\', "/").ends_with(suffix))
}

/// GH #53 literal repro: `Foo.tsx` is exported and consumed only by an
/// unchanged test file. After a trivial edit to `Foo.tsx`, the old (buggy)
/// `--since` narrowing stripped the test file from the analysis set, making
/// `Foo` look orphaned. The fix keeps the full graph; only findings are
/// filtered to the changed set.
#[test]
fn since_does_not_flag_export_consumed_only_by_unchanged_test_file() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    init_repo(dir);

    // Baseline commit: Foo exported, test file imports it.
    std::fs::create_dir_all(dir.join("components")).expect("mkdir components");
    std::fs::create_dir_all(dir.join("tests")).expect("mkdir tests");
    std::fs::write(
        dir.join("components").join("Foo.tsx"),
        "export function Foo() { return null; }\n",
    )
    .expect("write Foo");
    std::fs::write(
        dir.join("tests").join("Foo.test.tsx"),
        "import { Foo } from '../components/Foo';\nconsole.log(Foo);\n",
    )
    .expect("write test");
    commit_all(dir, "baseline");

    // Second commit: trivial change to Foo.tsx only (test file unchanged).
    std::fs::write(
        dir.join("components").join("Foo.tsx"),
        "export function Foo() { return null; }\n// updated\n",
    )
    .expect("modify Foo");
    commit_all(dir, "touch Foo");

    let (v, _stdout) = run_since_check(dir);
    let orphans = orphan_files(&v);
    assert!(
        !any_ends_with(&orphans, "components/Foo.tsx"),
        "Foo.tsx should NOT be orphan — it has a consumer in the unchanged test file; got orphans={orphans:?}"
    );
}

/// Broader class: a production lib file consumed only by an unchanged
/// production page must not be flagged when the lib file itself changes.
#[test]
fn since_does_not_flag_export_consumed_only_by_unchanged_production_file() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    init_repo(dir);

    // Baseline: lib/util.ts exported, app/page.ts imports it.
    std::fs::create_dir_all(dir.join("lib")).expect("mkdir lib");
    std::fs::create_dir_all(dir.join("app")).expect("mkdir app");
    std::fs::write(dir.join("lib").join("util.ts"), "export const util = 42;\n")
        .expect("write util");
    std::fs::write(
        dir.join("app").join("page.ts"),
        "import { util } from '../lib/util';\nexport default util;\n",
    )
    .expect("write page");
    commit_all(dir, "baseline");

    // Second commit: modify lib/util.ts only (page unchanged).
    std::fs::write(dir.join("lib").join("util.ts"), "export const util = 43;\n")
        .expect("modify util");
    commit_all(dir, "bump util");

    let (v, _stdout) = run_since_check(dir);
    let orphans = orphan_files(&v);
    assert!(
        !any_ends_with(&orphans, "lib/util.ts"),
        "lib/util.ts should NOT be orphan — it is consumed by app/page.ts; got orphans={orphans:?}"
    );
}

/// Positive control: a genuinely orphaned file that was ADDED in the second
/// commit (no importer anywhere) must still be flagged.
#[test]
fn since_still_flags_genuinely_orphaned_changed_file() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    init_repo(dir);

    // Baseline: a well-connected file so OrphanExport is exercised.
    std::fs::create_dir_all(dir.join("lib")).expect("mkdir lib");
    std::fs::create_dir_all(dir.join("app")).expect("mkdir app");
    std::fs::write(dir.join("lib").join("used.ts"), "export const x = 1;\n").expect("write used");
    std::fs::write(
        dir.join("app").join("index.ts"),
        "import { x } from '../lib/used';\nexport default x;\n",
    )
    .expect("write index");
    commit_all(dir, "baseline");

    // Second commit: add a new file with no importer anywhere.
    std::fs::write(
        dir.join("lib").join("dead.ts"),
        "export function dead() { return 0; }\n",
    )
    .expect("write dead");
    commit_all(dir, "add dead export");

    let (v, _stdout) = run_since_check(dir);
    let orphans = orphan_files(&v);
    assert!(
        any_ends_with(&orphans, "lib/dead.ts"),
        "lib/dead.ts SHOULD be flagged as orphan — it has no importer; got orphans={orphans:?}"
    );
}

/// Report-scope control: a pre-existing orphan in an *unchanged* file must
/// not appear in `--since` output even though the full graph sees it.
#[test]
fn since_does_not_report_preexisting_orphan_in_unchanged_file() {
    let tmp = TempDir::new().expect("tempdir");
    let dir = tmp.path();
    init_repo(dir);

    // Baseline: old_dead.ts is already orphaned; other.ts has a real consumer.
    std::fs::create_dir_all(dir.join("lib")).expect("mkdir lib");
    std::fs::create_dir_all(dir.join("app")).expect("mkdir app");
    std::fs::write(
        dir.join("lib").join("old_dead.ts"),
        "export function oldDead() { return 0; }\n",
    )
    .expect("write old_dead");
    std::fs::write(
        dir.join("lib").join("other.ts"),
        "export const other = 'v1';\n",
    )
    .expect("write other");
    std::fs::write(
        dir.join("app").join("consumer.ts"),
        "import { other } from '../lib/other';\nexport default other;\n",
    )
    .expect("write consumer");
    commit_all(dir, "baseline");

    // Second commit: modify other.ts only (old_dead.ts and consumer.ts unchanged).
    std::fs::write(
        dir.join("lib").join("other.ts"),
        "export const other = 'v2';\n",
    )
    .expect("modify other");
    commit_all(dir, "bump other");

    let (v, _stdout) = run_since_check(dir);
    let orphans = orphan_files(&v);
    assert!(
        !any_ends_with(&orphans, "lib/old_dead.ts"),
        "lib/old_dead.ts should NOT appear — it is in an unchanged file (report scope excludes it); got orphans={orphans:?}"
    );
}
