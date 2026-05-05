//! Integration tests for `cofferdam.invariants.toml` end-to-end:
//! parser → engine wiring → BoundaryFrozen + InvariantViolation +
//! OrphanExport public_api allowlist.

use std::collections::BTreeMap;
use std::path::PathBuf;

use cofferdam_checks::all_builtins;
use cofferdam_core::invariants::{BoundarySpec, InvariantSpec, InvariantsSpec, PublicApiSpec};
use cofferdam_core::LayersConfig;
use cofferdam_engine::config::ProjectConfig;
use cofferdam_engine::Engine;
use tempfile::TempDir;

fn write(dir: &std::path::Path, rel: &str, body: &str) -> PathBuf {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
    std::fs::write(&path, body).expect("write");
    path
}

fn engine_with(spec: InvariantsSpec) -> Engine {
    let layers = if !spec.layers.is_empty() {
        Some(LayersConfig {
            project_root: spec.project_root.clone(),
            layers: spec.layers.clone(),
            allow: spec.layers_allow.clone(),
        })
    } else {
        None
    };
    let cfg = ProjectConfig {
        layers,
        invariants: Some(spec),
        ..ProjectConfig::default()
    };
    Engine::with_config(all_builtins(), &cfg, std::path::Path::new("/synthetic"))
        .expect("engine builds")
}

#[test]
fn boundary_frozen_emits_per_file_in_glob() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().to_path_buf();
    let inside = write(&root, "src/legacy/old.ts", "export const X = 1;\n");
    let outside = write(&root, "src/app/page.ts", "export const Y = 2;\n");

    let mut bounds = BTreeMap::new();
    bounds.insert(
        "src/legacy/**".to_string(),
        BoundarySpec {
            frozen: true,
            reason: Some("see ADR-0007".to_string()),
        },
    );
    let spec = InvariantsSpec {
        project_root: root.clone(),
        boundaries: bounds,
        ..InvariantsSpec::default()
    };

    let engine = engine_with(spec);
    let issues = engine.analyze(&[inside.clone(), outside]).expect("ok");

    let bf: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Design.BoundaryFrozen")
        .collect();
    assert_eq!(
        bf.len(),
        1,
        "one finding inside the glob, none outside; got: {:?}",
        issues
    );
    assert_eq!(bf[0].file, inside);
    assert!(bf[0].message.contains("src/legacy/**"));
    assert!(bf[0].message.contains("see ADR-0007"));
}

#[test]
fn invariant_forbid_imports_fires_on_layer_match() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().to_path_buf();
    let app_file = write(
        &root,
        "src/app/page.ts",
        "import { fetchFromDb } from '../infra/db/connection';\nexport const X = fetchFromDb;\n",
    );
    let _db_file = write(
        &root,
        "src/infra/db/connection.ts",
        "export function fetchFromDb() { return 1; }\n",
    );

    let mut layers = BTreeMap::new();
    layers.insert("app".to_string(), vec!["src/app/**".to_string()]);
    layers.insert("infra".to_string(), vec!["src/infra/**".to_string()]);

    let mut invs = BTreeMap::new();
    invs.insert(
        "no-direct-db-access".to_string(),
        InvariantSpec {
            forbid_imports: vec!["src/infra/db".to_string()],
            require_imports: vec![],
            from_layers: vec!["app".to_string()],
        },
    );
    let spec = InvariantsSpec {
        project_root: root.clone(),
        layers,
        invariants: invs,
        ..InvariantsSpec::default()
    };

    let engine = engine_with(spec);
    let issues = engine.analyze(std::slice::from_ref(&app_file)).expect("ok");

    let iv: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Design.InvariantViolation")
        .collect();
    assert!(
        iv.iter()
            .any(|i| i.file == app_file && i.message.contains("no-direct-db-access")),
        "expected forbid-imports finding from app-layer file, got: {:?}",
        issues
    );
}

#[test]
fn invariant_skips_when_from_layers_does_not_match() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().to_path_buf();
    let domain_file = write(
        &root,
        "src/domain/user.ts",
        "import { fetchFromDb } from '../infra/db/connection';\nexport const Y = fetchFromDb;\n",
    );
    let _db_file = write(
        &root,
        "src/infra/db/connection.ts",
        "export function fetchFromDb() { return 1; }\n",
    );

    let mut layers = BTreeMap::new();
    layers.insert("app".to_string(), vec!["src/app/**".to_string()]);
    layers.insert("domain".to_string(), vec!["src/domain/**".to_string()]);
    layers.insert("infra".to_string(), vec!["src/infra/**".to_string()]);

    let mut invs = BTreeMap::new();
    invs.insert(
        "no-direct-db-access".to_string(),
        InvariantSpec {
            forbid_imports: vec!["src/infra/db".to_string()],
            require_imports: vec![],
            from_layers: vec!["app".to_string()],
        },
    );
    let spec = InvariantsSpec {
        project_root: root.clone(),
        layers,
        invariants: invs,
        ..InvariantsSpec::default()
    };

    let engine = engine_with(spec);
    let issues = engine.analyze(&[domain_file]).expect("ok");

    assert!(
        !issues
            .iter()
            .any(|i| i.check_id == "Design.InvariantViolation"),
        "no findings expected from domain-layer file, got: {:?}",
        issues
    );
}

#[test]
fn orphan_export_skips_public_api_files() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().to_path_buf();
    let entry = write(&root, "src/index.ts", "export const PUBLIC = 1;\n");
    let other = write(&root, "src/util.ts", "export const PRIVATE = 2;\n");

    let spec = InvariantsSpec {
        project_root: root.clone(),
        public_api: PublicApiSpec {
            exports: vec!["src/index.ts".to_string()],
        },
        ..InvariantsSpec::default()
    };

    let engine = engine_with(spec);
    let issues = engine.analyze(&[entry.clone(), other.clone()]).expect("ok");

    let orphans: Vec<_> = issues
        .iter()
        .filter(|i| i.check_id == "Design.OrphanExport")
        .collect();
    assert!(
        orphans.iter().all(|i| i.file != entry),
        "src/index.ts should be exempt from OrphanExport (allowlisted), got: {:?}",
        issues
    );
    assert!(
        orphans.iter().any(|i| i.file == other),
        "control file should still be flagged, got: {:?}",
        issues
    );
}
