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
    run_check_args(&["--only", only], source)
}

fn run_check_args(extra: &[&str], source: &str) -> std::process::Output {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("a.ts"), source).expect("write ts");

    let mut args = vec!["check"];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["--format", "json", "--no-baseline"]);
    Command::new(cofferdam_bin())
        .args(args)
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam")
}

/// An over-long line built from short tokens, exporting an unimported
/// `a`, so it trips both `Readability.MaxLineLength` and
/// `Design.OrphanExport`. Built from many short tokens on purpose: since
/// CD-318, `MaxLineLength` skips a line whose over-length comes from one
/// atomic token, so a single long string literal no longer fires it.
fn long_wrappable_line() -> String {
    format!("export const a = [{}];\n", "1, ".repeat(60))
}

#[test]
fn only_restricts_to_the_named_check() {
    // A long line with an exported, unimported const trips both
    // Readability.MaxLineLength and Design.OrphanExport — the latter is
    // the real regression guard: it actually co-fires on this fixture, so
    // its absence from `--only Readability.MaxLineLength` output proves
    // the filter works, unlike an arbitrary check that would never have
    // fired here anyway.
    let long_line = long_wrappable_line();
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

/// CD-307: several ids in one run, comma-separated. The fixture fires
/// both named checks, so seeing both proves the set is a union rather
/// than last-one-wins.
#[test]
fn only_accepts_a_comma_separated_set() {
    let long_line = long_wrappable_line();
    let out = run_check_args(
        &["--only", "Readability.MaxLineLength,Design.OrphanExport"],
        &long_line,
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Readability.MaxLineLength") && stdout.contains("Design.OrphanExport"),
        "both named checks should survive the filter; stdout={stdout}"
    );
}

/// CD-307: the repeated-flag form is equivalent to the comma-separated
/// one — clap's `value_delimiter` plus the default append behaviour.
#[test]
fn only_accepts_a_repeated_flag() {
    let long_line = long_wrappable_line();
    let out = run_check_args(
        &[
            "--only",
            "Readability.MaxLineLength",
            "--only",
            "Design.OrphanExport",
        ],
        &long_line,
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Readability.MaxLineLength") && stdout.contains("Design.OrphanExport"),
        "both named checks should survive the filter; stdout={stdout}"
    );
}

/// CD-307: one bad id among good ones must fail the whole run. Accepting
/// the good ones and dropping the typo would silently shrink a CI gate —
/// the failure mode `--only` exists to prevent, made likelier by sets.
#[test]
fn only_with_one_bad_id_among_good_ones_errors() {
    let long_line = long_wrappable_line();
    let out = run_check_args(
        &[
            "--only",
            "Readability.MaxLineLength,Warning.TripleEqualz,Design.OrphanExport",
        ],
        &long_line,
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(2),
        "one unknown id must fail the run, not narrow it; stderr={stderr}"
    );
    assert!(
        stderr.contains("Warning.TripleEqualz"),
        "the error should name the offending id; stderr={stderr}"
    );
    assert!(
        !stderr.contains("Readability.MaxLineLength"),
        "the error should name only the offending id, not the valid ones; stderr={stderr}"
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

fn node_present() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

const ECHO_PLUGIN: &str = r#"
export default {
  id: "Loud",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "CD-96 regression fixture: reports once per file.",
  requiresTypes: false,
  options: {},
  run(file, ctx) {
    ctx.report({ message: "loud", span: { line: 1, column: 1, start_byte: 0, end_byte: 0 } });
  },
};
"#;

fn plugin_env(source: &str) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let plugin_dir = dir.path().join("plugin");
    std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    std::fs::write(plugin_dir.join("index.mjs"), ECHO_PLUGIN).expect("write plugin");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "plugins = [\"./plugin\"]\n",
    )
    .expect("write toml");
    std::fs::write(dir.path().join("a.ts"), source).expect("write ts");
    dir
}

/// CD-96: `--only <bare-plugin-id>` (the id a plugin author wrote in
/// their own `defineCheck`) must resolve to the runtime category-prefixed
/// id, not silently match zero findings.
#[test]
fn only_resolves_bare_plugin_check_id() {
    if !node_present() {
        return;
    }
    let dir = plugin_env("export const a = 1;\n");
    let out = Command::new(cofferdam_bin())
        .args([
            "check",
            "--only",
            "Loud",
            "--format",
            "json",
            "--no-baseline",
        ])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("Warning.Loud"),
        "bare plugin id should resolve to its prefixed runtime id; stdout={stdout}\nstderr={stderr}"
    );
}

/// CD-96: a genuinely unknown `--only` id must still hard-error even when
/// plugins are configured, instead of the previous blanket skip.
#[test]
fn only_typo_errors_even_with_plugins_configured() {
    if !node_present() {
        return;
    }
    let dir = plugin_env("export const a = 1;\n");
    let out = Command::new(cofferdam_bin())
        .args([
            "check",
            "--only",
            "Warning.TotallyNotReal",
            "--format",
            "json",
            "--no-baseline",
        ])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no known check id"),
        "a typo'd --only value must error even with plugins configured; stderr={stderr}"
    );
    assert_eq!(out.status.code(), Some(2));
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
