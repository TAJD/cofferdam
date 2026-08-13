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

/// A 6-parameter function — over the default `Design.MaxParameters`
/// limit of 5, under a relaxed limit like 10.
fn long_function() -> String {
    "export function big(a, b, c, d, e, f) {\n  return a + b + c + d + e + f;\n}\n".to_string()
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
[overrides.checks."Design.MaxParameters"]
limit = 10
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
        !fired_on(&issues, "Design.MaxParameters", test_file),
        "relaxed limit (10) should silence the test file's 6-param function"
    );
    assert!(
        fired_on(&issues, "Design.MaxParameters", regular_file),
        "the non-matching regular file keeps the default limit (5) and still fires"
    );
}

#[test]
fn disabled_skips_only_that_check_on_matching_files() {
    // The whole point of per-glob overrides: relax ONE check on test
    // files while every other check keeps running on them.
    let toml = r#"
[[overrides]]
paths = ["**/*.test.ts"]
[overrides.checks."Design.MaxParameters"]
disabled = true
"#;
    let mut body = long_function();
    body.push_str("export function g(items: number[]) {\n  return items.length;\n}\n"); // Design.ReadonlyArrayParam bait

    let (issues, paths) = analyze_project(toml, &[("src/thing.test.ts", body)]);
    let test_file = &paths[0];

    assert!(
        !fired_on(&issues, "Design.MaxParameters", test_file),
        "disabled override must skip MaxParameters on the test file"
    );
    assert!(
        fired_on(&issues, "Design.ReadonlyArrayParam", test_file),
        "other checks must still run on the test file"
    );
}

#[test]
fn severity_override_applies_only_to_matching_files() {
    let toml = r#"
[[overrides]]
paths = ["**/*.test.ts"]
[overrides.checks."Design.ReadonlyArrayParam"]
severity = "info"
"#;
    let body = "export function f(items: number[]) {\n  return items.length;\n}\n".to_string();
    let (issues, paths) =
        analyze_project(toml, &[("src/a.test.ts", body.clone()), ("src/a.ts", body)]);
    let test_file = &paths[0];
    let regular_file = &paths[1];

    let test_sev = issues
        .iter()
        .find(|i| i.check_id == "Design.ReadonlyArrayParam" && i.file == *test_file)
        .map(|i| i.severity);
    let regular_sev = issues
        .iter()
        .find(|i| i.check_id == "Design.ReadonlyArrayParam" && i.file == *regular_file)
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
[overrides.checks."Design.MaxParameters"]
limit = 10

[[overrides]]
paths = ["src/**"]
[overrides.checks."Design.MaxParameters"]
limit = 3
"#;
    let (issues, paths) = analyze_project(toml, &[("src/Lobby.test.tsx", long_function())]);
    let test_file = &paths[0];

    // The second block (limit 3) wins → the 6-param function fires again.
    assert!(
        fired_on(&issues, "Design.MaxParameters", test_file),
        "last matching block (limit 3) should win over the earlier limit 10"
    );
}

#[test]
fn disabled_override_should_suppress_finalize_emitted_checks() {
    // CD-97: `disabled = true` must also suppress findings emitted from a
    // check's `finalize()` (cross-file checks like Design.OrphanExport),
    // not just per-file `run()` output.
    let toml = r#"
[[overrides]]
paths = ["src/unused.ts"]
[overrides.checks."Design.OrphanExport"]
disabled = true
"#;
    let (issues, paths) = analyze_project(
        toml,
        &[(
            "src/unused.ts",
            "export const neverImported = 1;\n".to_string(),
        )],
    );
    let f = &paths[0];
    assert!(
        !fired_on(&issues, "Design.OrphanExport", f),
        "disabled override should suppress finalize-emitted OrphanExport too, got: {:?}",
        issues.iter().map(|i| &i.check_id).collect::<Vec<_>>()
    );
}

#[test]
fn no_overrides_is_unaffected() {
    // Sanity: a config with no [[overrides]] behaves exactly as before.
    let (issues, paths) = analyze_project("", &[("src/Lobby.tsx", long_function())]);
    assert!(
        fired_on(&issues, "Design.MaxParameters", &paths[0]),
        "without overrides, the default limit applies"
    );
}
