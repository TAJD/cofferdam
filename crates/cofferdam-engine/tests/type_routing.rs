//! Engine routing for type-aware checks (cd-9hp.2 cp2).
//!
//! Proves the `requires_types` dispatch contract without a Node worker:
//! a pure-Rust stub [`TypeOracle`] stands in for the ts-morph host. The
//! real worker-backed path is exercised by a gated test in
//! `cofferdam-cli` (`type_host.rs`).
//!
//! Contract under test:
//!   - A check declaring `requires_types = true` is SKIPPED when the
//!     engine has no oracle installed.
//!   - The same check RUNS when an oracle is installed, sees it via
//!     `ctx.types`, and its findings merge into the normal issue stream.
//!   - Non-type-aware checks are unaffected either way.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, Location, Priority, Severity, Span, TypeFacts,
    TypeOracle,
};
use cofferdam_engine::Engine;
use tempfile::TempDir;

/// Stub oracle: returns canned facts and counts how many times it was
/// asked, so the test can assert the engine actually routed through it.
struct StubOracle {
    facts: TypeFacts,
    calls: Arc<AtomicUsize>,
}

impl TypeOracle for StubOracle {
    fn type_at(&self, _file: &Path, _start: u32, _end: u32) -> Option<TypeFacts> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Some(self.facts.clone())
    }
}

/// Test-only type-aware check. Emits one finding per file when the
/// oracle reports the queried span's type includes null. Mirrors the
/// shape cp3's `Warning.UnusedNullCheck` will take.
struct NullFactCheck;

static NULL_FACT_META: CheckMeta = CheckMeta {
    id: "Warning.TestTypeAware",
    category: Category::Warning,
    base_priority: 15,
    default_severity: Severity::Medium,
    explanation: "test-only type-aware check",
    body: "",
    requires_types: true,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: false,
};

impl Check for NullFactCheck {
    fn meta(&self) -> &'static CheckMeta {
        &NULL_FACT_META
    }

    fn run(&self, file: &cofferdam_core::SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        // A type-aware check the engine routed here always has an
        // oracle — but guard rather than unwrap so a misconfiguration
        // degrades to "no finding" instead of a panic.
        let Some(oracle) = ctx.types else {
            return Vec::new();
        };
        let Some(facts) = oracle.type_at(&file.path, 0, 1) else {
            return Vec::new();
        };
        if !facts.includes_null {
            return Vec::new();
        }
        let path = file.path.clone();
        vec![Issue {
            check_id: NULL_FACT_META.id.to_string(),
            message: format!("type `{}` includes null", facts.text),
            location: Location::from_span(
                &path,
                Span {
                    start_byte: 0,
                    end_byte: 1,
                    line: 1,
                    column: 1,
                },
            ),
            file: path,
            priority: Priority(15),
            severity: Severity::Medium,
            related: Vec::new(),
        }]
    }
}

fn write_ts(dir: &TempDir, name: &str, body: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, body).expect("write fixture");
    path
}

#[test]
fn type_aware_check_is_skipped_without_oracle() {
    let dir = TempDir::new().unwrap();
    let file = write_ts(&dir, "a.ts", "const x: string | null = null;\n");

    let engine = Engine::new(vec![Box::new(NullFactCheck)]);
    assert!(
        engine.needs_type_oracle(),
        "a registered requires_types check should report needs_type_oracle()"
    );
    let issues = engine.analyze(&[file]).expect("analyze");
    assert!(
        issues.iter().all(|i| i.check_id != "Warning.TestTypeAware"),
        "type-aware check must be skipped when no oracle is installed; got {issues:?}"
    );
}

#[test]
fn type_aware_check_runs_with_oracle_and_findings_merge() {
    let dir = TempDir::new().unwrap();
    let file = write_ts(&dir, "a.ts", "const x: string | null = null;\n");

    let calls = Arc::new(AtomicUsize::new(0));
    let oracle = StubOracle {
        facts: TypeFacts {
            text: "string | null".into(),
            is_nullable: true,
            includes_null: true,
            includes_undefined: false,
            is_any: false,
        },
        calls: calls.clone(),
    };

    let engine = Engine::new(vec![Box::new(NullFactCheck)]).with_type_oracle(Box::new(oracle));
    let issues = engine.analyze(&[file]).expect("analyze");

    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "engine should route exactly one type query through the oracle"
    );
    let found: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Warning.TestTypeAware")
        .collect();
    assert_eq!(
        found.len(),
        1,
        "type-aware finding should merge into the issue stream; got {issues:?}"
    );
    assert!(found[0].message.contains("includes null"));
}

#[test]
fn oracle_reporting_non_null_emits_nothing() {
    let dir = TempDir::new().unwrap();
    let file = write_ts(&dir, "a.ts", "const x: string = \"hi\";\n");

    let oracle = StubOracle {
        facts: TypeFacts {
            text: "string".into(),
            is_nullable: false,
            includes_null: false,
            includes_undefined: false,
            is_any: false,
        },
        calls: Arc::new(AtomicUsize::new(0)),
    };

    let engine = Engine::new(vec![Box::new(NullFactCheck)]).with_type_oracle(Box::new(oracle));
    let issues = engine.analyze(&[file]).expect("analyze");
    assert!(
        issues.iter().all(|i| i.check_id != "Warning.TestTypeAware"),
        "no finding expected when the oracle reports a non-null type; got {issues:?}"
    );
}
