//! Integration tests for per-glob check overrides (cd-m5tu / gh #46).
//!
//! `[[overrides]]` blocks scope a single check's `limit` / `severity` /
//! `disabled` to files matching their path globs, while every OTHER
//! check keeps running on those files. These tests drive the full
//! config → engine → analyze path against real temp-dir files so glob
//! matching exercises the same absolutization the production CLI uses.

use std::path::{Path, PathBuf};

use cofferdam_checks::all_builtins;
use cofferdam_core::{Issue, Severity};
use cofferdam_engine::{config, Engine};
use tempfile::TempDir;

/// 60-statement function body — well over the default
/// `Readability.MaxFunctionLength` limit of 50, under a relaxed 400.
fn long_function() -> String {
    let mut s = String::from("export function big() {\n");
    for i in 0..60 {
        s.push_str(&format!("  const v{i} = {i};\n"));
    }
    s.push_str("  return v0;\n}\n");
    s
}

/// Write `cofferdam.toml` + the given `(relative_path, contents)` files
/// into a fresh temp dir, build an engine from the loaded config, and
/// return (issues, the absolute file paths in input order).
fn analyze_project(toml: &str, files: &[(&str, String)]) -> (Vec<Issue>, Vec<PathBuf>) {
    let dir = TempDir::new().expect("tempdir");
    let toml_path = dir.path().join("cofferdam.toml");
    std::fs::write(&toml_path, toml).expect("write cofferdam.toml");

    let mut paths = Vec::new();
    for (rel, contents) in files {
        let p = dir.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("create dirs");
        }
        std::fs::write(&p, contents).expect("write source");
        paths.push(p);
    }

    let cfg = config::load(&toml_path).expect("load config");
    let engine = Engine::with_config(all_builtins(), &cfg, &toml_path).expect("engine builds");
    let refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
    let issues = engine.analyze(&refs).expect("analyze");
    (issues, paths)
}

fn fired_on(issues: &[Issue], check_id: &str, path: &Path) -> bool {
    issues
        .iter()
        .any(|i| i.check_id == check_id && i.file == *path)
}

#[test]
fn relaxed_limit_only_affects_matching_files() {
    let toml = r#"
[[overrides]]
paths = ["**/*.test.tsx"]
[overrides.checks."Readability.MaxFunctionLength"]
limit = 400
"#;
    let (issues, paths) = analyze_project(
        toml,
        &[
            ("src/Lobby.test.tsx", long_function()),
            ("src/Lobby.tsx", long_function()),
        ],
    );
    let test_file = &paths[0];
    let regular_file = &paths[1];

    assert!(
        !fired_on(&issues, "Readability.MaxFunctionLength", test_file),
        "relaxed limit (400) should silence the test file's long function"
    );
    assert!(
        fired_on(&issues, "Readability.MaxFunctionLength", regular_file),
        "the non-matching regular file keeps the default limit (50) and still fires"
    );
}

#[test]
fn disabled_skips_only_that_check_on_matching_files() {
    // The whole point of per-glob overrides: relax ONE check on test
    // files while every other check keeps running on them.
    let toml = r#"
[[overrides]]
paths = ["**/*.test.ts"]
[overrides.checks."Readability.MaxFunctionLength"]
disabled = true
"#;
    let mut body = long_function();
    body.push_str("const wrong = (1 == 2);\n"); // Warning.TripleEquals bait

    let (issues, paths) = analyze_project(toml, &[("src/thing.test.ts", body)]);
    let test_file = &paths[0];

    assert!(
        !fired_on(&issues, "Readability.MaxFunctionLength", test_file),
        "disabled override must skip MaxFunctionLength on the test file"
    );
    assert!(
        fired_on(&issues, "Warning.TripleEquals", test_file),
        "other checks must still run on the test file"
    );
}

#[test]
fn severity_override_applies_only_to_matching_files() {
    let toml = r#"
[[overrides]]
paths = ["**/*.test.ts"]
[overrides.checks."Warning.TripleEquals"]
severity = "info"
"#;
    let body = "export const a = (1 == 2);\n".to_string();
    let (issues, paths) =
        analyze_project(toml, &[("src/a.test.ts", body.clone()), ("src/a.ts", body)]);
    let test_file = &paths[0];
    let regular_file = &paths[1];

    let test_sev = issues
        .iter()
        .find(|i| i.check_id == "Warning.TripleEquals" && i.file == *test_file)
        .map(|i| i.severity);
    let regular_sev = issues
        .iter()
        .find(|i| i.check_id == "Warning.TripleEquals" && i.file == *regular_file)
        .map(|i| i.severity);

    assert_eq!(
        test_sev,
        Some(Severity::Info),
        "severity override should stamp Info on the matching test file"
    );
    assert!(
        regular_sev.is_some() && regular_sev != Some(Severity::Info),
        "the non-matching file keeps the check's default severity, got {regular_sev:?}"
    );
}

#[test]
fn last_matching_block_wins() {
    // Two blocks both match the test file and both set a limit; the
    // later block's value must win.
    let toml = r#"
[[overrides]]
paths = ["**/*.test.tsx"]
[overrides.checks."Readability.MaxFunctionLength"]
limit = 400

[[overrides]]
paths = ["src/**"]
[overrides.checks."Readability.MaxFunctionLength"]
limit = 10
"#;
    let (issues, paths) = analyze_project(toml, &[("src/Lobby.test.tsx", long_function())]);
    let test_file = &paths[0];

    // The second block (limit 10) wins → the 60-line function fires again.
    assert!(
        fired_on(&issues, "Readability.MaxFunctionLength", test_file),
        "last matching block (limit 10) should win over the earlier limit 400"
    );
}

#[test]
fn no_overrides_is_unaffected() {
    // Sanity: a config with no [[overrides]] behaves exactly as before.
    let (issues, paths) = analyze_project("", &[("src/Lobby.tsx", long_function())]);
    assert!(
        fired_on(&issues, "Readability.MaxFunctionLength", &paths[0]),
        "without overrides, the default limit applies"
    );
}
