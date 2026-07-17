//! Regression test for CD-105: `cofferdam check` reported every baselined
//! finding as new immediately after `baseline write`, with no code changes
//! in between.
//!
//! Root cause: `baseline::normalize_path` compared the freshly-discovered
//! `issue.file` against the project root using a plain `Path::strip_prefix`
//! on whatever raw form each side happened to be in. In an npm-workspaces
//! monorepo, `baseline write` is commonly run from the repo root (`roots =
//! ["."]`, so `issue.file` comes back relative, e.g. `pkg/src/a.ts`) while
//! `check` is run per-workspace from a package subdirectory (`roots =
//! ["."]` relative to that subdir, so `issue.file` comes back as `src/a.ts`)
//! with the shared root baseline auto-discovered by walking up from cwd.
//! The stored `pkg/src/a.ts` and the freshly computed `src/a.ts` never
//! string-match, so every finding looks new even though nothing changed.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

#[test]
fn check_from_workspace_subdir_matches_baseline_written_from_repo_root() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let root = dir.path();

    std::fs::create_dir_all(root.join("pkg/src")).expect("mkdir pkg/src");
    std::fs::write(
        root.join("pkg/src/a.ts"),
        "export function f() { if (x == 1) { return 1; } }\n",
    )
    .expect("write a.ts");

    // Baseline is written once from the repo root, covering the whole tree.
    let write_out = Command::new(cofferdam_bin())
        .args(["baseline", "write"])
        .current_dir(root)
        .output()
        .expect("spawn baseline write");
    assert!(
        write_out.status.success(),
        "baseline write should succeed; stderr={}",
        String::from_utf8_lossy(&write_out.stderr)
    );

    let baseline_path = root.join(".cofferdam/baseline.json");
    assert!(baseline_path.is_file(), "baseline.json must be written");
    let baseline_contents = std::fs::read_to_string(&baseline_path).expect("read baseline");
    assert!(
        baseline_contents.contains("Warning.TripleEquals"),
        "sanity check: expected a TripleEquals finding in the baseline; got:\n{baseline_contents}"
    );

    // `check` runs from the package subdirectory (the npm-workspaces
    // per-package script pattern), with no explicit --baseline flag — it
    // must auto-discover the repo-root baseline by walking up.
    let check_out = Command::new(cofferdam_bin())
        .args(["check", "--format=json", "."])
        .current_dir(root.join("pkg"))
        .output()
        .expect("spawn check");

    let stdout = String::from_utf8_lossy(&check_out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "check stdout not valid JSON: {e}\nstdout={stdout}\nstderr={}",
            String::from_utf8_lossy(&check_out.stderr)
        )
    });

    let baselined_count = v["summary"]["baselined"].as_u64().unwrap_or(0);
    let new_count = v["summary"]["new"].as_u64().unwrap_or(0);

    assert!(
        baselined_count > 0 && new_count == 0,
        "findings present at baseline-write time must be suppressed as baselined when \
         `check` runs from a workspace subdirectory with the baseline auto-discovered \
         from the repo root; got new={new_count} baselined={baselined_count}\nstdout={stdout}"
    );
}
