//! Integration: the example suppression fixtures actually exercise the
//! suppression-hygiene checks they're named for (CD-357 pass 2).
//!
//! `examples/broad_suppression.ts`, `examples/unused_suppression.ts`, and
//! `examples/suppressions.ts` all reference a target check by dotted ID
//! inside `cofferdam-ignore`/`cofferdam-disable` directives. If that ID
//! ever drifts to a check that's been removed from `all_builtins()`, the
//! directive is silently treated as targeting an unregistered check ID —
//! the suppression becomes a no-op with no diagnostic, and these fixtures
//! would stop proving anything. These tests pin non-empty (or, for the
//! "everything is genuinely live" fixture, exactly-empty-for-staleness)
//! finding sets so that drift is caught immediately instead of silently.

use std::path::PathBuf;

use cofferdam_checks::all_builtins;
use cofferdam_engine::Engine;

fn read_example(name: &str) -> (PathBuf, String) {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = manifest_dir
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("examples")
        .join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    (path, text)
}

/// `examples/broad_suppression.ts` has two `// cofferdam-ignore` (no check
/// id) lines marked FLAG — Consistency.BroadSuppression must still fire on
/// them.
#[test]
fn broad_suppression_fixture_yields_broadsuppression_findings() {
    let engine = Engine::new(all_builtins());
    let (path, text) = read_example("broad_suppression.ts");
    let (issues, _) = engine.analyze_with_sources(vec![(path, text)]);

    let broad: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Consistency.BroadSuppression")
        .collect();
    assert!(
        !broad.is_empty(),
        "expected non-empty Consistency.BroadSuppression findings on broad_suppression.ts \
         (a directive with an unregistered check ID would silently defeat this fixture)"
    );
    assert_eq!(
        broad.len(),
        2,
        "expected exactly the two FLAG lines: {broad:#?}"
    );
}

/// `examples/unused_suppression.ts` has two stale directives marked FLAG —
/// Consistency.UnusedSuppression must still fire on them.
#[test]
fn unused_suppression_fixture_yields_unusedsuppression_findings() {
    let engine = Engine::new(all_builtins());
    let (path, text) = read_example("unused_suppression.ts");
    let (issues, _) = engine.analyze_with_sources(vec![(path, text)]);

    let unused: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Consistency.UnusedSuppression")
        .collect();
    assert!(
        !unused.is_empty(),
        "expected non-empty Consistency.UnusedSuppression findings on unused_suppression.ts \
         (a directive with an unregistered check ID would silently defeat this fixture)"
    );
    assert_eq!(
        unused.len(),
        2,
        "expected exactly the two FLAG lines: {unused:#?}"
    );
}

/// `examples/suppressions.ts` demonstrates every directive form
/// successfully suppressing a genuine, live `Design.ReadonlyArrayParam`
/// finding. If the directive's check ID were unregistered, suppression
/// would silently no-op and the "suppressed" functions would leak a
/// finding — so a non-empty, correctly-scoped `ReadonlyArrayParam` result
/// (firing only on the one intentionally-unsuppressed function) proves the
/// directives are bound to a real, registered check.
#[test]
fn suppressions_fixture_directives_are_bound_to_a_live_check() {
    let engine = Engine::new(all_builtins());
    let (path, text) = read_example("suppressions.ts");
    let (issues, _) = engine.analyze_with_sources(vec![(path, text)]);

    let readonly_array_param: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Design.ReadonlyArrayParam")
        .collect();
    assert!(
        !readonly_array_param.is_empty(),
        "expected at least one live Design.ReadonlyArrayParam finding \
         (the unsuppressed `p` function): {issues:#?}"
    );
    assert!(
        readonly_array_param.iter().all(|i| i.location.line() == 29),
        "every ReadonlyArrayParam finding should be the unsuppressed `p` \
         function on line 29 — a finding anywhere else means a suppression \
         directive silently failed to bind: {readonly_array_param:#?}"
    );

    // No directive in this fixture is stale — every one covers a real,
    // live finding, so UnusedSuppression must stay silent.
    let unused: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Consistency.UnusedSuppression")
        .collect();
    assert!(
        unused.is_empty(),
        "no directive in suppressions.ts should be flagged stale: {unused:#?}"
    );
}
