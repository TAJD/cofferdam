//! Fixture-repo integration tests for `Context.Knowledge` (CD-162).
//!
//! Exercises the real `Check` dispatch through `Engine::analyze_context`
//! (not just `knowledge::load_knowledge_dir` directly) — a layer
//! selector must fire on every edit inside the layer and never outside
//! it (spec success criterion 2).
//!
//! Deliberately does NOT use `std::env::set_current_dir`: parallel test
//! threads share one process CWD, so mutating it races (see
//! `crates/cofferdam-cli/src/doctor.rs`'s doc comment on the same
//! hazard). Instead the fixture supplies `ProjectConfig.layers` with an
//! explicit `project_root`, which `Context.Knowledge` prefers over a
//! CWD-anchored `.git` walk (see `resolve_project_root` in
//! `crates/cofferdam-checks/src/context/knowledge.rs`).

use std::collections::BTreeMap;
use std::path::Path;

use cofferdam_checks::all_context_providers;
use cofferdam_core::graph::LayersConfig;
use cofferdam_core::ChangeSet;
use cofferdam_engine::{Engine, ProjectConfig};

fn write(root: &Path, rel: &str, contents: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, contents).unwrap();
}

fn engine_with_billing_layer(root: &Path) -> Engine {
    let mut layers = BTreeMap::new();
    layers.insert("billing".to_string(), vec!["src/billing/**".to_string()]);
    let layers_config = LayersConfig {
        project_root: root.to_path_buf(),
        layers,
        allow: BTreeMap::new(),
    };
    let cfg = ProjectConfig {
        layers: Some(layers_config),
        ..ProjectConfig::default()
    };

    Engine::with_config(all_context_providers(), &cfg, &root.join("cofferdam.toml"))
        .expect("engine builds with a valid layers config")
}

#[test]
fn layer_selector_note_fires_on_every_edit_inside_the_layer() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    write(
        root,
        ".cofferdam/knowledge/billing.md",
        "---\ntitle: Billing invariants\nmatch:\n  layers: [\"billing\"]\npriority: high\n---\nNever round intermediate money values.\n",
    );
    let engine = engine_with_billing_layer(root);

    for rel in ["src/billing/invoice.ts", "src/billing/ledger/entry.ts"] {
        let file = root.join(rel);
        let cs = ChangeSet::from_files([file.clone()]);
        let out = engine.analyze_context(vec![(file, "export const x = 1;\n".into())], &cs);
        assert_eq!(out.items.len(), 1, "expected a fire for {rel}");
        assert_eq!(out.items[0].title, "Billing invariants");
        assert_eq!(out.items[0].check_id, "Context.Knowledge");
        assert!(out.items[0].pinned, "priority: high must pin the item");
    }
}

#[test]
fn layer_selector_note_never_fires_outside_the_layer() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    write(
        root,
        ".cofferdam/knowledge/billing.md",
        "---\ntitle: Billing invariants\nmatch:\n  layers: [\"billing\"]\n---\nNever round intermediate money values.\n",
    );
    let engine = engine_with_billing_layer(root);

    for rel in ["src/ui/widget.ts", "src/other/thing.ts"] {
        let file = root.join(rel);
        let cs = ChangeSet::from_files([file.clone()]);
        let out = engine.analyze_context(vec![(file, "export const x = 1;\n".into())], &cs);
        assert!(out.items.is_empty(), "unexpected fire for {rel}");
    }
}

#[test]
fn paths_glob_selector_fires_on_match_and_silent_off_match() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    write(
        root,
        ".cofferdam/knowledge/api.md",
        "---\ntitle: API surface note\nmatch:\n  paths: [\"src/api/**\"]\n---\nPublic API changes need a changelog entry.\n",
    );
    let cfg = ProjectConfig {
        layers: Some(LayersConfig {
            project_root: root.to_path_buf(),
            layers: BTreeMap::new(),
            allow: BTreeMap::new(),
        }),
        ..ProjectConfig::default()
    };
    let engine = Engine::with_config(all_context_providers(), &cfg, &root.join("cofferdam.toml"))
        .expect("engine builds");

    let inside = root.join("src/api/routes.ts");
    let cs = ChangeSet::from_files([inside.clone()]);
    let out = engine.analyze_context(vec![(inside, "export const x = 1;\n".into())], &cs);
    assert_eq!(out.items.len(), 1);
    assert_eq!(out.items[0].title, "API surface note");

    let outside = root.join("src/ui/widget.ts");
    let cs = ChangeSet::from_files([outside.clone()]);
    let out = engine.analyze_context(vec![(outside, "export const y = 1;\n".into())], &cs);
    assert!(out.items.is_empty());
}

#[test]
fn no_knowledge_dir_yields_no_items_and_no_warnings() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    let engine = engine_with_billing_layer(root);
    let file = root.join("src/billing/invoice.ts");
    let cs = ChangeSet::from_files([file.clone()]);
    let out = engine.analyze_context(vec![(file, "export const x = 1;\n".into())], &cs);
    assert!(out.items.is_empty());
    assert!(
        !out.issues.iter().any(|i| i.check_id == "Context.Knowledge"),
        "no knowledge dir should produce no Context.Knowledge issues: {:?}",
        out.issues
    );
}

#[test]
fn invalid_selector_warns_via_issue_not_silently() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    write(
        root,
        ".cofferdam/knowledge/broken.md",
        "---\ntitle: Broken glob note\nmatch:\n  paths: [\"src/[unterminated\"]\n---\nBody.\n",
    );
    let engine = engine_with_billing_layer(root);
    let file = root.join("src/billing/invoice.ts");
    let cs = ChangeSet::from_files([file.clone()]);
    let out = engine.analyze_context(vec![(file, "export const x = 1;\n".into())], &cs);

    let warning = out
        .issues
        .iter()
        .find(|i| i.check_id == "Context.Knowledge")
        .expect("a warning Issue for the broken glob");
    assert!(warning.message.contains("invalid glob"));
}
