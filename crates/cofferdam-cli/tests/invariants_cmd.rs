//! `cofferdam invariants` (CD-308) — the CLI's answer to "what spec did
//! you actually load?". Until this existed only the MCP server could
//! answer, which inverted the "MCP wraps the CLI" contract the server
//! documents.

use std::path::{Path, PathBuf};
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

/// Seal the temp dir with a `.git` marker so walk-up discovery cannot
/// reach a real `cofferdam.toml` above the system temp directory.
fn sealed_dir() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::create_dir_all(dir.path().join(".git")).expect("seal");
    dir
}

fn run(args: &[&str], dir: &Path) -> std::process::Output {
    Command::new(cofferdam_bin())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn cofferdam")
}

const FULL_SPEC: &str = r#"schema_version = "1.0"

[layers]
domain = ["src/domain/**"]
ui = ["src/ui/**"]

[layers.allow]
ui = ["domain"]
domain = []

[public_api]
exports = ["src/index.ts"]

[boundaries."src/legacy/**"]
frozen = true
reason = "rewritten twice"

[invariants."no-ui-in-domain"]
forbid_imports = ["src/ui/"]
from_layers = ["domain"]
"#;

#[test]
fn show_reports_every_section_of_the_spec() {
    let dir = sealed_dir();
    std::fs::write(dir.path().join("cofferdam.invariants.toml"), FULL_SPEC).expect("write spec");

    let out = run(&["invariants", "show"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout={stdout}");
    for expected in [
        "domain",
        "src/domain/**",
        "src/index.ts",
        "src/legacy/**",
        "rewritten twice",
        "no-ui-in-domain",
    ] {
        assert!(
            stdout.contains(expected),
            "expected {expected:?} in the rendered spec; stdout={stdout}"
        );
    }
}

/// The load-bearing detail: `cofferdam.invariants.toml` `[layers]`
/// replaces `cofferdam.toml`'s wholesale, and a user staring at a rule
/// that will not fire is often looking at the file that lost. `show`
/// names the winner rather than making them infer it.
#[test]
fn show_names_which_file_the_layers_came_from() {
    let dir = sealed_dir();
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "[layers]\nfrom_toml = [\"src/a/**\"]\n",
    )
    .expect("write toml");

    let only_toml = run(&["invariants", "show"], dir.path());
    let stdout = String::from_utf8_lossy(&only_toml.stdout);
    assert!(
        stdout.contains("Layers — from cofferdam.toml") && stdout.contains("from_toml"),
        "cofferdam.toml layers should be reported as in force; stdout={stdout}"
    );

    std::fs::write(
        dir.path().join("cofferdam.invariants.toml"),
        "[layers]\nfrom_invariants = [\"src/b/**\"]\n",
    )
    .expect("write spec");

    let both = run(&["invariants", "show"], dir.path());
    let stdout = String::from_utf8_lossy(&both.stdout);
    assert!(
        stdout.contains("Layers — from cofferdam.invariants.toml"),
        "invariants.toml layers win the merge; stdout={stdout}"
    );
    assert!(
        stdout.contains("from_invariants") && !stdout.contains("from_toml"),
        "the losing file's layers must not be reported as in force; stdout={stdout}"
    );
}

#[test]
fn show_json_is_machine_readable() {
    let dir = sealed_dir();
    std::fs::write(dir.path().join("cofferdam.invariants.toml"), FULL_SPEC).expect("write spec");

    let out = run(&["invariants", "show", "--robot"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("--robot output must parse as JSON ({e}); stdout={stdout}"));
    assert_eq!(parsed["layers_source"], "invariants");
    assert_eq!(parsed["schema_version"], "1.0");
    assert_eq!(parsed["public_api_exports"][0], "src/index.ts");
    assert_eq!(parsed["boundaries"]["src/legacy/**"]["frozen"], true);
}

/// "You have no spec" and "your spec is empty" send a user to different
/// fixes, so `show` says so rather than printing an empty skeleton.
#[test]
fn show_says_so_when_nothing_is_declared() {
    let dir = sealed_dir();
    let out = run(&["invariants", "show"], dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "an absent spec is not an error");
    assert!(
        stdout.contains("No config file found") && stdout.contains("Nothing declared"),
        "stdout={stdout}"
    );
}

/// The CI-friendly shape: gate on the config alone, before and
/// independently of the findings it produces.
#[test]
fn validate_fails_on_a_malformed_predicate() {
    let dir = sealed_dir();
    std::fs::write(
        dir.path().join("cofferdam.invariants.toml"),
        "[invariants.scripted.\"bad\"]\nforbid = \"file imports\"\nmessage = \"x\"\n",
    )
    .expect("write spec");

    let out = run(&["invariants", "validate"], dir.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a spec that cannot load must fail the gate; stderr={stderr}"
    );
    assert!(
        stderr.contains("DSL parse failed"),
        "the error should say what is wrong; stderr={stderr}"
    );
}

#[test]
fn validate_passes_on_a_good_spec_and_on_no_spec() {
    let dir = sealed_dir();
    assert_eq!(
        run(&["invariants", "validate"], dir.path()).status.code(),
        Some(0),
        "no spec is not a validation failure — most repos have none"
    );

    std::fs::write(dir.path().join("cofferdam.invariants.toml"), FULL_SPEC).expect("write spec");
    assert_eq!(
        run(&["invariants", "validate"], dir.path()).status.code(),
        Some(0)
    );
}

/// A spec with no `schema_version` loads fine but warns. `--strict` is
/// how a project that has decided to care turns that into a gate.
#[test]
fn validate_strict_turns_warnings_into_failures() {
    let dir = sealed_dir();
    std::fs::write(
        dir.path().join("cofferdam.invariants.toml"),
        "[layers]\ndomain = [\"src/domain/**\"]\n",
    )
    .expect("write spec");

    let lenient = run(&["invariants", "validate"], dir.path());
    let stderr = String::from_utf8_lossy(&lenient.stderr);
    assert_eq!(lenient.status.code(), Some(0), "stderr={stderr}");
    assert!(
        stderr.contains("schema_version"),
        "the warning should still be reported; stderr={stderr}"
    );

    let strict = run(&["invariants", "validate", "--strict"], dir.path());
    assert_eq!(strict.status.code(), Some(1));
}

/// Normalising a spec and reloading it must produce the same spec —
/// otherwise the canonical form is not canonical.
#[test]
fn normalize_round_trips() {
    let dir = sealed_dir();
    std::fs::write(dir.path().join("cofferdam.invariants.toml"), FULL_SPEC).expect("write spec");

    let first = run(&["invariants", "normalize"], dir.path());
    let normalized = String::from_utf8_lossy(&first.stdout).to_string();
    assert_eq!(
        first.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(normalized.contains("schema_version"));

    std::fs::write(dir.path().join("cofferdam.invariants.toml"), &normalized).expect("rewrite");
    let second = run(&["invariants", "normalize"], dir.path());
    assert_eq!(
        String::from_utf8_lossy(&second.stdout),
        normalized,
        "normalising an already-normalised spec must be a no-op"
    );
}

#[test]
fn normalize_fails_when_there_is_nothing_to_normalize() {
    let dir = sealed_dir();
    let out = run(&["invariants", "normalize"], dir.path());
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no cofferdam.invariants.toml found"));
}
