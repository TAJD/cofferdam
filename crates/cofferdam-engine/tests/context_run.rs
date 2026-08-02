use cofferdam_core::{
    Category, ChangeSet, Check, CheckContext, CheckMeta, ContextItem, FinalizeContext, Issue,
    Severity, SourceFile,
};
use cofferdam_engine::Engine;
use std::path::PathBuf;

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
    let out = engine.analyze_context(vec![(src, "if (a == b) { console.log(1) }\n".into())], &cs);
    assert!(!out.issues.is_empty());
    assert!(out.items.is_empty());
}
