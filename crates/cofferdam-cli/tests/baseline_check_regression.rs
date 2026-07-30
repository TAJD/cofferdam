//! CD-153: `cofferdam baseline write` then `cofferdam check --baseline`
//! re-matched only whole-file (line-1) findings — a `RunCache` hit
//! rebuilt its returned `texts` map from un-absolutized paths while the
//! replayed `Issue`s were already stamped absolute, so
//! `baseline::signature_for_issue` hashed an empty snippet on every
//! warm-cache lookup and every finding came back "new" even though
//! nothing had changed. Root cause fixed in
//! `cofferdam-engine::analyze_with_sources_full_impl` (shared
//! `absolutize_sources` helper). This file pins the end-to-end CLI
//! contract: a `check --baseline` run immediately after `baseline write`
//! must report everything as baselined, on both a cold and a
//! cache-warmed run, with the text and JSON formatters agreeing, and
//! `--hide-baselined` actually hiding matched findings.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

/// A file with findings spread across multiple lines (not just line 1),
/// so the regression can't hide behind the whole-file-finding special
/// case the bug report called out as still matching.
const FIXTURE: &str = "export function foo() {\n  console.log(\"hi\");\n  if (1 == 2) {\n    return 1;\n  }\n  return 2;\n}\n\nexport function bar(a, b, c, d, e, f) {\n  return a + b + c + d + e + f;\n}\n";

fn run(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(cofferdam_bin())
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn cofferdam")
}

#[test]
fn check_baseline_matches_on_both_cold_and_cache_warmed_runs() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("a.ts"), FIXTURE).expect("write fixture");

    let write_out = run(
        dir.path(),
        &["baseline", "write", "--output", "baseline.json"],
    );
    assert!(
        write_out.status.success(),
        "baseline write should exit 0; stderr={}",
        String::from_utf8_lossy(&write_out.stderr)
    );

    // Cold run: no `.cofferdam/cache` yet.
    let cold = run(
        dir.path(),
        &["check", "--baseline", "baseline.json", "--format", "json"],
    );
    assert!(
        cold.status.success(),
        "cold check must exit 0 (nothing new)"
    );
    let cold_json = String::from_utf8_lossy(&cold.stdout);
    assert!(
        cold_json.contains("\"new\":0"),
        "cold run: everything should be baselined; got: {cold_json}"
    );

    // Warm run: same input set + config as the cold run, so this hits
    // the RunCache entry the cold run just populated on disk.
    let warm = run(
        dir.path(),
        &["check", "--baseline", "baseline.json", "--format", "json"],
    );
    assert!(
        warm.status.success(),
        "warm (RunCache-hit) check must also exit 0 — nothing changed since baseline write; stdout={}",
        String::from_utf8_lossy(&warm.stdout)
    );
    let warm_json = String::from_utf8_lossy(&warm.stdout);
    assert!(
        warm_json.contains("\"new\":0"),
        "warm run: a RunCache hit must not change any finding's baseline signature; got: {warm_json}"
    );

    // Text and JSON formatters must agree on the same (warm) run.
    let warm_text = run(dir.path(), &["check", "--baseline", "baseline.json"]);
    assert!(warm_text.status.success());
    let warm_text_out = String::from_utf8_lossy(&warm_text.stdout);
    assert!(
        warm_text_out.contains("(0 new,"),
        "text formatter summary must agree with JSON's new:0; got: {warm_text_out}"
    );
}

#[test]
fn hide_baselined_hides_matched_findings_on_a_warm_run() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("a.ts"), FIXTURE).expect("write fixture");

    let write_out = run(
        dir.path(),
        &["baseline", "write", "--output", "baseline.json"],
    );
    assert!(write_out.status.success());

    // Prime the RunCache (same trigger as the test above).
    let prime = run(dir.path(), &["check", "--baseline", "baseline.json"]);
    assert!(prime.status.success());

    let hidden = run(
        dir.path(),
        &["check", "--baseline", "baseline.json", "--hide-baselined"],
    );
    assert!(hidden.status.success());
    let hidden_out = String::from_utf8_lossy(&hidden.stdout);
    assert!(
        !hidden_out.contains("Warning.TripleEquals")
            && !hidden_out.contains("Warning.NoConsoleLog"),
        "--hide-baselined must not print baselined findings; got: {hidden_out}"
    );
    assert!(
        hidden_out.contains("baselined — hidden") || hidden_out.contains("baselined)"),
        "summary should still report the hidden count; got: {hidden_out}"
    );
}
