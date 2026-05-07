//! Spec-contract fixtures (cd-9hp.8).
//!
//! Each subdirectory of `tests/spec_contract/` is a self-contained
//! "spec → engine → findings" snapshot:
//!
//! * `cofferdam.invariants.toml` (and optionally `cofferdam.toml`)
//! * a tree of `.ts`/`.tsx` files exercising the rule under test, and
//! * `expected.json` — the canonical findings the engine must emit.
//!
//! The runner discovers each fixture, runs the full builtin check
//! pipeline, normalises every `Issue` into a fixture-relative form
//! (project-relative path, just file/line/column/id/message), sorts
//! deterministically, and compares to `expected.json`.
//!
//! Refresh expected files after an intentional spec change with
//! `BLESS_SPEC_CONTRACT=1 cargo test -p cofferdam-engine spec_contract`.
//! The bless step writes pretty-printed JSON; review the resulting
//! `git diff` in PR.
//!
//! New fixtures: drop a directory in here, write the spec + sources,
//! add a `#[test]` at the bottom of this file, run with the bless var
//! once, commit. That is the canonical place to pin a new architectural
//! semantic.

use std::path::{Path, PathBuf};

use cofferdam_checks::all_builtins;
use cofferdam_engine::{config::resolve_with_invariants, Engine};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct SpecFinding {
    id: String,
    file: String,
    line: u32,
    column: u32,
    message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    related: Vec<RelatedRef>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct RelatedRef {
    file: String,
    line: u32,
    column: u32,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct SpecExpected {
    findings: Vec<SpecFinding>,
}

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/spec_contract")
}

/// Recursively gather `.ts` / `.tsx` files under `dir`. Skips any
/// `node_modules/` segment so a fixture remains self-contained even if a
/// developer adds one accidentally.
fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().and_then(|n| n.to_str()) == Some("node_modules") {
                continue;
            }
            collect_sources(&p, out);
        } else if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
            if matches!(ext, "ts" | "tsx") {
                out.push(p);
            }
        }
    }
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn run_fixture(dir: &Path) -> SpecExpected {
    let (cfg_opt, _path, _diags) =
        resolve_with_invariants(None, dir, false).expect("resolve invariants");
    // resolve_with_invariants returns None when neither cofferdam.toml
    // nor a populated invariants.toml exists — every fixture in this
    // suite is expected to ship an invariants.toml so missing config is
    // a fixture-authoring bug, not a runtime case worth tolerating.
    let cfg = cfg_opt.unwrap_or_else(|| {
        panic!(
            "fixture `{}` produced no ProjectConfig — does it have a cofferdam.invariants.toml?",
            dir.display()
        )
    });

    let mut sources = Vec::new();
    collect_sources(dir, &mut sources);
    sources.sort();

    let engine = Engine::with_config(all_builtins(), &cfg, dir).expect("engine builds");
    let issues = engine.analyze(&sources).expect("analyze");

    let mut findings: Vec<SpecFinding> = issues
        .into_iter()
        .map(|i| SpecFinding {
            id: i.check_id,
            file: rel(&i.file, dir),
            line: i.span.line,
            column: i.span.column,
            message: i.message,
            related: i
                .related
                .into_iter()
                .map(|r| RelatedRef {
                    file: rel(&r.file, dir),
                    line: r.span.line,
                    column: r.span.column,
                })
                .collect(),
        })
        .collect();

    findings.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
            .then(a.id.cmp(&b.id))
    });

    SpecExpected { findings }
}

fn check_fixture(name: &str) {
    let dir = fixtures_root().join(name);
    assert!(
        dir.exists(),
        "spec-contract fixture missing: {}",
        dir.display()
    );

    let actual = run_fixture(&dir);
    let expected_path = dir.join("expected.json");

    if std::env::var_os("BLESS_SPEC_CONTRACT").is_some() {
        let mut serialized = serde_json::to_string_pretty(&actual).expect("serialize");
        serialized.push('\n');
        std::fs::write(&expected_path, &serialized).expect("write expected.json");
        // Don't assert during bless — the next non-bless run does that.
        return;
    }

    let raw = std::fs::read_to_string(&expected_path).unwrap_or_else(|e| {
        panic!(
            "missing {}: {e}. Bless once with BLESS_SPEC_CONTRACT=1.",
            expected_path.display()
        )
    });
    let expected: SpecExpected = serde_json::from_str(&raw).expect("parse expected.json");

    if actual != expected {
        let want = serde_json::to_string_pretty(&expected).unwrap();
        let got = serde_json::to_string_pretty(&actual).unwrap();
        panic!(
            "spec-contract drift in `{name}`.\n\n--- expected.json (committed) ---\n{want}\n\n--- engine output ---\n{got}\n\nIf the change is intentional, refresh with:\n  BLESS_SPEC_CONTRACT=1 cargo test -p cofferdam-engine spec_contract"
        );
    }
}

// ─── Fixtures ─────────────────────────────────────────────────────────────
//
// One #[test] per fixture so a single rule's drift surfaces with a
// targeted name. The `check_fixture` helper does all the heavy lifting.

#[test]
fn spec_contract_layers_allowlist() {
    check_fixture("layers-allowlist");
}

#[test]
fn spec_contract_frozen_boundary() {
    check_fixture("frozen-boundary");
}

#[test]
fn spec_contract_public_api_orphans() {
    check_fixture("public-api-orphans");
}

#[test]
fn spec_contract_forbid_imports() {
    check_fixture("forbid-imports");
}

#[test]
fn spec_contract_require_imports() {
    check_fixture("require-imports");
}

#[test]
fn spec_contract_combo_multi_rule() {
    check_fixture("combo-multi-rule");
}
