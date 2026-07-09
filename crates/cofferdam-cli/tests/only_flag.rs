//! `cofferdam check --only <CheckId>` (CD-74) restricts output to one
//! check's findings, so a CI hook can gate on a single project-specific
//! check without turning on the full suite. A typo hard-errors (exit 2)
//! rather than silently returning zero findings, since the whole point is
//! a CI gate on the exit code — a typo must not make that gate pass. A
//! valid but currently-quiet check id must not error.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

fn run_check(only: &str, source: &str) -> std::process::Output {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("a.ts"), source).expect("write ts");

    Command::new(cofferdam_bin())
        .args(["check", "--only", only, "--format", "json", "--no-baseline"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam")
}

#[test]
fn only_restricts_to_the_named_check() {
    // A long line with an exported, unimported const trips both
    // Readability.MaxLineLength and Design.OrphanExport — the latter is
    // the real regression guard: it actually co-fires on this fixture, so
    // its absence from `--only Readability.MaxLineLength` output proves
    // the filter works, unlike an arbitrary check that would never have
    // fired here anyway.
    let long_line = format!("export const a = \"{}\";\n", "x".repeat(200));
    let out = run_check("Readability.MaxLineLength", &long_line);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Readability.MaxLineLength"),
        "expected the named check's finding; stdout={stdout}"
    );
    assert!(
        !stdout.contains("Design.OrphanExport"),
        "expected no findings from other checks; stdout={stdout}"
    );
}

#[test]
fn only_with_typo_errors() {
    let out = run_check("Warning.TripleEqualz", "export const a = 1 == 1;\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Warning.TripleEqualz") && stderr.contains("no known check id"),
        "a typo'd --only value must report an error; stderr={stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "a typo'd --only value must hard-error (exit 2), not silently return zero findings \
         and exit 0 — the whole point is a CI gate on the exit code; stderr={stderr}"
    );
}

#[test]
fn only_valid_but_quiet_check_does_not_error() {
    // TripleEquals is a real check id; this source has no `==` to flag.
    let out = run_check("Warning.TripleEquals", "export const a = 1;\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("no known check id"),
        "a valid check id with zero findings this run must not error; stderr={stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "expected success exit code; stderr={stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(r#""findings":[]"#),
        "expected no findings; stdout={stdout}"
    );
}
