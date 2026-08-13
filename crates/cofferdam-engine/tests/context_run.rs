use cofferdam_core::{
    Category, ChangeSet, Check, CheckContext, CheckMeta, ContextItem, FinalizeContext, Issue,
    Severity, SourceFile,
};
use cofferdam_engine::Engine;
use std::path::{Path, PathBuf};

struct EchoProvider;

const ECHO_META: CheckMeta = CheckMeta {
    id: "Context.TestEcho",
    category: Category::Context,
    base_priority: 0,
    default_severity: Severity::Info,
    explanation: "test provider double",
    body: "test provider double",
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: false,
};

impl Check for EchoProvider {
    fn meta(&self) -> &'static CheckMeta {
        &ECHO_META
    }
    fn run(&self, _file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        Vec::new()
    }
    fn context_items(
        &self,
        changeset: &ChangeSet,
        _ctx: &mut FinalizeContext<'_>,
    ) -> Vec<ContextItem> {
        vec![ContextItem {
            check_id: "Context.TestEcho".into(),
            title: format!("{} changed files", changeset.files.len()),
            body: "echo".into(),
            score: 1,
            pinned: false,
            related: vec![],
            explain: None,
        }]
    }
}

#[test]
fn analyze_context_runs_providers_with_changeset_after_finalize() {
    let checks: Vec<Box<dyn Check>> = vec![Box::new(EchoProvider)];
    let engine = Engine::new(checks);
    let src = PathBuf::from("/virtual/a.ts");
    let cs = ChangeSet::from_files([src.clone()]);
    let out = engine.analyze_context(vec![(src, "const x = 1;\n".into())], &cs);
    assert_eq!(out.items.len(), 1);
    assert_eq!(out.items[0].title, "1 changed files");
}

#[test]
fn analyze_context_still_returns_normal_issues() {
    // Engine built from all_builtins must emit ordinary findings for a
    // file that violates a default check, alongside (empty) items.
    let engine = Engine::new(cofferdam_checks::all_builtins());
    let src = PathBuf::from("/virtual/b.ts");
    let cs = ChangeSet::from_files([src.clone()]);
    let out = engine.analyze_context(
        vec![(
            src,
            "export function f(x: number[]) {\n  return x.length;\n}\n".into(),
        )],
        &cs,
    );
    assert!(!out.issues.is_empty());
    assert!(out.items.is_empty());
}

/// CD-159 acceptance: a fixture with a fresh finding (inside the diff's
/// changed line range) and a legacy finding (outside it) produces two
/// distinct `Context.Findings` signals — a fresh-findings summary and a
/// per-file legacy-debt rollup — and both survive as `pinned: true`.
#[test]
fn context_findings_provider_distinguishes_fresh_from_legacy() {
    use cofferdam_core::LineRange;

    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("examples/context-findings");
    // Match the engine's own `std::path::absolute` normalization (see
    // `absolutize_sources`) so the `ChangeSet` keys line up with the
    // `Issue.file` paths the engine stamps on findings.
    let dirty = std::path::absolute(fixture_dir.join("dirty.ts")).expect("absolute path");
    let clean = std::path::absolute(fixture_dir.join("clean.ts")).expect("absolute path");
    let dirty_text = std::fs::read_to_string(&dirty).expect("read dirty.ts fixture");
    let clean_text = std::fs::read_to_string(&clean).expect("read clean.ts fixture");

    // Deliberately a narrow check set (not `all_builtins()`) so this test
    // isn't coupled to unrelated builtins (e.g. `Design.OrphanExport`,
    // `Design.MissingTestFile`) that would also fire on an isolated
    // fixture pair and muddy the fresh/legacy counts being asserted.
    let checks: Vec<Box<dyn Check>> = vec![
        Box::new(cofferdam_checks::design::ReadonlyArrayParam),
        Box::new(cofferdam_checks::design::MaxParameters::new(5)),
        Box::new(cofferdam_checks::context::findings::Findings),
    ];
    let engine = Engine::new(checks);

    // The diff only touched the `touched` function (lines 7-10); the
    // `Design.ReadonlyArrayParam` finding on line 1 (inside `legacy`) is
    // outside that range and so counts as legacy debt, while the
    // `Design.MaxParameters` finding on line 8 (touched's 6-parameter
    // signature) is fresh.
    let cs = ChangeSet {
        files: [dirty.clone(), clean.clone()].into_iter().collect(),
        line_ranges: [(dirty.clone(), vec![LineRange { start: 8, end: 10 }])]
            .into_iter()
            .collect(),
    };

    let out = engine.analyze_context(vec![(dirty.clone(), dirty_text), (clean, clean_text)], &cs);

    let findings_items: Vec<_> = out
        .items
        .iter()
        .filter(|i| i.check_id == "Context.Findings")
        .collect();
    assert_eq!(
        findings_items.len(),
        2,
        "expected one fresh summary + one legacy rollup, got: {findings_items:?}"
    );
    assert!(findings_items.iter().all(|i| i.pinned));

    let fresh = findings_items
        .iter()
        .find(|i| i.title.contains("in changed lines"))
        .expect("fresh-findings item");
    assert!(
        fresh.body.contains("Design.MaxParameters"),
        "{}",
        fresh.body
    );
    assert!(
        !fresh.body.contains("Design.ReadonlyArrayParam"),
        "legacy finding must not appear in the fresh summary: {}",
        fresh.body
    );

    let legacy = findings_items
        .iter()
        .find(|i| i.title.contains("Legacy debt"))
        .expect("legacy-debt item");
    assert!(legacy.title.contains("dirty.ts"), "{}", legacy.title);
    assert!(
        legacy.body.contains("1 baselined finding"),
        "{}",
        legacy.body
    );
}
