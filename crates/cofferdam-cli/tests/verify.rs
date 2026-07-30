//! `cofferdam verify --dist <dir>` (CD-85) — opt-in check mode for built
//! HTML output. Asserts findings are produced and labeled with their
//! build-output provenance, and that a plain `cofferdam check` run is
//! completely blind to the same dist tree.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

#[test]
fn verify_dist_reports_labeled_findings() {
    let out = Command::new(cofferdam_bin())
        .args([
            "verify",
            "--dist",
            "examples/dist-fixture",
            "--format",
            "json",
        ])
        .current_dir(repo_root())
        .output()
        .expect("spawn cofferdam verify");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Html.MissingLangAttribute"),
        "expected a Html.MissingLangAttribute finding; stdout={stdout}"
    );
    assert!(
        stdout.contains(r#""origin":"build_output""#),
        "expected origin: build_output in JSON output; stdout={stdout}"
    );
    assert!(
        stdout.contains(r#""dist":"#),
        "expected a dist field in JSON output; stdout={stdout}"
    );
}

// `Check::output_mode() == true` only ADDS eligibility for `verify
// --dist` — see the doc comment on `Check::output_mode` — it never
// removes a check from `cofferdam check`'s normal dispatch. So
// `Html.MissingLangAttribute` fires under `cofferdam check
// examples/dist-fixture` exactly as it did before CD-85 (CD-85 never
// touches `run_check`/discovery for the default command at all); the
// isolation CD-85 actually guarantees is that verify --dist's own
// discovery+origin-tagged pipeline is a wholly separate code path, and
// that a normal `cofferdam check` invocation with no explicit dist
// argument doesn't discover an (ordinarily gitignored) dist directory
// on its own — not that pointing `check` explicitly at a directory
// full of `.html` files suppresses findings there.
#[test]
fn plain_check_still_applies_normal_checks_to_dist_fixture_html() {
    let out = Command::new(cofferdam_bin())
        .args([
            "check",
            "examples/dist-fixture",
            "--format",
            "json",
            "--no-baseline",
        ])
        .current_dir(repo_root())
        .output()
        .expect("spawn cofferdam check");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Html.MissingLangAttribute"),
        "output-mode-eligible checks must still run normally under plain `cofferdam check`; stdout={stdout}"
    );
}

fn node_present() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// CD-88: `verify --dist` must never invoke a non-output-mode plugin check
// at all — not just filter its findings out afterward — so it can't leak
// a synthetic host-error finding (or anything else) into verify output.
// `TsOnly` below is scoped to `.ts` (never matches the `.html` dist tree)
// and throws unconditionally if `run()` is ever called; its presence in
// `cofferdam.toml` alongside a real output-mode HTML check must not
// affect verify's output at all.
#[test]
fn verify_dist_never_invokes_non_output_mode_plugin_checks() {
    if !node_present() {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let html_plugin = dir.path().join("plugin-html");
    let ts_plugin = dir.path().join("plugin-ts");
    std::fs::create_dir_all(&html_plugin).expect("mkdir plugin-html");
    std::fs::create_dir_all(&ts_plugin).expect("mkdir plugin-ts");
    std::fs::write(
        html_plugin.join("index.mjs"),
        r#"
export default {
  id: "HtmlOut",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "output-mode html check",
  requiresTypes: false,
  outputMode: true,
  options: {},
  files: { extensions: ["html", "htm"] },
  run(file, ctx) {
    ctx.report({ message: "html finding", span: { line: 1, column: 1, start_byte: 0, end_byte: 0 } });
  },
};
"#,
    )
    .expect("write plugin-html");
    std::fs::write(
        ts_plugin.join("index.mjs"),
        r#"
export default {
  id: "TsOnly",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "non-output-mode ts-only check that must never run under verify --dist",
  requiresTypes: false,
  outputMode: false,
  options: {},
  files: { extensions: ["ts"] },
  run(file, ctx) {
    throw new Error("should never run under verify --dist");
  },
};
"#,
    )
    .expect("write plugin-ts");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "plugins = [\"./plugin-html\", \"./plugin-ts\"]\n",
    )
    .expect("write cofferdam.toml");
    std::fs::create_dir_all(dir.path().join("dist")).expect("mkdir dist");
    std::fs::write(
        dir.path().join("dist/index.html"),
        "<html><head><title>Home</title></head><body>ok</body></html>",
    )
    .expect("write dist/index.html");

    let out = Command::new(cofferdam_bin())
        .args(["verify", "--dist", "dist", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam verify");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("Warning.HtmlOut"),
        "the real output-mode check must still fire; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("TsOnly") && !stdout.contains("PluginCrashed"),
        "the non-output-mode check must never be invoked, so it can't leak a \
         finding or a synthetic error; stdout={stdout}\nstderr={stderr}"
    );
}

// CD-95: `verify --dist` must still scan the named directory in full even
// when it's itself gitignored (the whole point of naming it explicitly),
// but must not blanket-sweep genuinely-nested ignored subdirectories
// inside it (a coverage report or cache dir that happens to sit under the
// build output) — a prior blanket `respect_ignore: false` swept those too.
#[test]
fn verify_dist_skips_nested_gitignored_subdirectory_but_scans_the_rest() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    // Strip any GIT_DIR / GIT_WORK_TREE / etc. this process inherited
    // (e.g. from a `pre-push` hook, or from running inside a git
    // worktree checkout) — otherwise `git init` here targets the
    // *inherited* repo instead of `dir`, so `dir` never gets its own
    // `.git` and the ignore crate's repo-boundary detection walks up
    // into the wrong tree. Mirrors `GIT_ENV_VARS_TO_CLEAR` in
    // `tests/orphan_since_report_scope.rs`.
    let mut git_init = Command::new("git");
    git_init.args(["init", "-q"]).current_dir(dir.path());
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
    ] {
        git_init.env_remove(var);
    }
    git_init.output().expect("git init");
    std::fs::write(dir.path().join(".gitignore"), "dist/\ndist/coverage/\n")
        .expect("write .gitignore");
    std::fs::create_dir_all(dir.path().join("dist/coverage")).expect("mkdir");
    std::fs::write(
        dir.path().join("dist/index.html"),
        "<html><head><title>Home</title></head><body>ok</body></html>",
    )
    .expect("write dist/index.html");
    std::fs::write(
        dir.path().join("dist/coverage/report.html"),
        "<html><body>coverage noise</body></html>",
    )
    .expect("write dist/coverage/report.html");

    let out = Command::new(cofferdam_bin())
        .args(["verify", "--dist", "dist", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam verify");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("dist/index.html") || stdout.contains(r"dist\\index.html"),
        "the explicitly-named dist root must still be scanned even though dist/ is gitignored; stdout={stdout}"
    );
    assert!(
        !stdout.contains("coverage"),
        "a nested gitignored subdirectory inside dist must not be swept; stdout={stdout}"
    );
}

// A missing --dist directory (typo, renamed build output, a build step
// that silently never ran) must be a hard error — "0 files found, exit
// 0" would make a broken CI pipeline look like a passing verify gate.
#[test]
fn verify_dist_errors_on_nonexistent_directory() {
    let out = Command::new(cofferdam_bin())
        .args([
            "verify",
            "--dist",
            "examples/this-directory-does-not-exist",
            "--format",
            "json",
        ])
        .current_dir(repo_root())
        .output()
        .expect("spawn cofferdam verify");

    assert!(
        !out.status.success(),
        "verify --dist against a nonexistent directory must not exit 0"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not exist"),
        "expected a clear does-not-exist error; stderr={stderr}"
    );
}
