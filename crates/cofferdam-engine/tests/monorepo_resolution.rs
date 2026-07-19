//! Regression test for CD-139: `Design.OrphanExport` mass false positives
//! on an npm-workspace monorepo with a solution-style root `tsconfig.json`
//! (project `references`, no top-level `include`).
//!
//! Root cause: `GraphBuilder`'s `oxc_resolver` config had no
//! `extension_alias`, so a specifier ending in `.js` (the ESM-standard
//! extension used even when the actual source is `.ts` — required under
//! `"moduleResolution": "bundler"`/`"node16"`/`"nodenext"`) only ever
//! matched a literal `.js` file. Every relative or barrel-forwarded
//! import written that way silently failed to resolve, so the exports it
//! should have counted as "consumed" looked orphaned instead — cascading
//! through barrel re-exports into a project-wide false-positive storm.
//!
//! This fixture reproduces the ticket's reported shape end-to-end on
//! real files on disk (required: `oxc_resolver` does real filesystem
//! resolution, so in-memory-only paths can't exercise this bug) via the
//! full `Engine`, not a hand-built graph — a resolver-config regression
//! like this one is invisible to any test that doesn't go through the
//! real resolver.

use std::path::{Path, PathBuf};

use cofferdam_checks::all_builtins;
use cofferdam_engine::Engine;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, content: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir -p");
    std::fs::write(&path, content).expect("write fixture file");
    path
}

/// Builds a 3-package npm-workspace monorepo on disk:
///
/// - `packages/sim/src/math.ts` — exports `wrap`, consumed cross-package.
/// - `packages/render/src/RaceScene.ts` — exports `RaceScene`, forwarded
///   through `packages/render/src/index.ts`'s barrel using a `.js`
///   specifier extension (`export { RaceScene } from "./RaceScene.js"`),
///   then consumed cross-package via the `@start-line/render` bare
///   specifier.
/// - `packages/app/src/simStep.ts` — exports `simStep`, consumed via a
///   same-package *relative* `.js`-extension import
///   (`import { simStep } from "./simStep.js"`) — repro #1 from CD-139.
/// - `node_modules/@start-line/{render,sim}` — package.json shims whose
///   `main` points back at the real `packages/*/src/index.ts` (no
///   symlink: avoids Windows symlink-privilege flakiness in CI while
///   still exercising real node_modules package resolution).
/// - Root `tsconfig.json` is solution-style: `references` to each
///   package's own tsconfig, no top-level `include` — the shape CD-139's
///   hypothesis focused on.
fn workspace_fixture() -> (TempDir, Vec<PathBuf>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    write(
        root,
        "tsconfig.json",
        r#"{"files":[],"references":[{"path":"packages/sim"},{"path":"packages/render"},{"path":"packages/app"}]}"#,
    );

    write(
        root,
        "packages/sim/package.json",
        r#"{"name":"@start-line/sim","main":"src/index.ts"}"#,
    );
    write(
        root,
        "packages/sim/tsconfig.json",
        r#"{"compilerOptions":{"composite":true,"moduleResolution":"bundler"},"include":["src"]}"#,
    );
    let sim_math = write(
        root,
        "packages/sim/src/math.ts",
        "export function wrap(x: number): number {\n  return x % 1;\n}\n",
    );
    let sim_index = write(
        root,
        "packages/sim/src/index.ts",
        "export { wrap } from \"./math.js\";\n",
    );

    write(
        root,
        "packages/render/package.json",
        r#"{"name":"@start-line/render","main":"src/index.ts"}"#,
    );
    write(
        root,
        "packages/render/tsconfig.json",
        r#"{"compilerOptions":{"composite":true,"moduleResolution":"bundler"},"include":["src"]}"#,
    );
    let render_race_scene = write(
        root,
        "packages/render/src/RaceScene.ts",
        "export class RaceScene {}\n",
    );
    let render_index = write(
        root,
        "packages/render/src/index.ts",
        "export { RaceScene } from \"./RaceScene.js\";\n",
    );

    write(
        root,
        "packages/app/package.json",
        r#"{"name":"@start-line/app"}"#,
    );
    write(
        root,
        "packages/app/tsconfig.json",
        r#"{"compilerOptions":{"composite":true,"moduleResolution":"bundler"},"include":["src"]}"#,
    );
    let app_sim_step = write(
        root,
        "packages/app/src/simStep.ts",
        "export function simStep(): number {\n  return 1;\n}\n",
    );
    // Deliberately not exported: StartLine.tsx is the fixture's "entry
    // point" file, nothing imports it, and it isn't the subject of
    // either test's assertions — exporting `run` would itself be a
    // (correctly-flagged) orphan and add noise to both tests.
    let start_line = write(
        root,
        "packages/app/src/StartLine.tsx",
        "import { simStep } from \"./simStep.js\";\n\
         import { RaceScene } from \"@start-line/render\";\n\
         import { wrap } from \"@start-line/sim\";\n\
         function run(): void {\n\
         \x20 console.log(simStep(), wrap(1), new RaceScene());\n\
         }\n\
         run();\n",
    );

    write(
        root,
        "node_modules/@start-line/render/package.json",
        r#"{"name":"@start-line/render","main":"../../../packages/render/src/index.ts"}"#,
    );
    write(
        root,
        "node_modules/@start-line/sim/package.json",
        r#"{"name":"@start-line/sim","main":"../../../packages/sim/src/index.ts"}"#,
    );

    let files = vec![
        sim_math,
        sim_index,
        render_race_scene,
        render_index,
        app_sim_step,
        start_line,
    ];
    (dir, files)
}

fn orphan_export_names(issues: &[cofferdam_core::Issue]) -> Vec<String> {
    issues
        .iter()
        .filter(|i| i.check_id == "Design.OrphanExport")
        .map(|i| i.message.clone())
        .collect()
}

#[test]
fn workspace_barrel_and_relative_js_imports_are_not_flagged_orphan() {
    let (_dir, files) = workspace_fixture();
    let engine = Engine::new(all_builtins());
    let issues = engine.analyze(&files).expect("analyze should not error");

    let orphans = orphan_export_names(&issues);
    assert!(
        orphans.is_empty(),
        "CD-139 regression: legitimately-consumed exports flagged as orphan: {orphans:?}"
    );
}

/// Sanity check on the other side of the fix: an export nothing imports —
/// same package, same `.js`-extension-barrel shape — must still be
/// flagged. Guards against a fix that's so permissive it stops detecting
/// real orphans.
#[test]
fn genuinely_unused_export_in_same_shaped_project_is_still_flagged() {
    let (dir, mut files) = workspace_fixture();
    let root = dir.path();
    let truly_orphaned = write(
        root,
        "packages/app/src/unused.ts",
        "export function neverImported(): number {\n  return 0;\n}\n",
    );
    files.push(truly_orphaned);

    let engine = Engine::new(all_builtins());
    let issues = engine.analyze(&files).expect("analyze should not error");

    let orphans = orphan_export_names(&issues);
    assert!(
        orphans.iter().any(|m| m.contains("neverImported")),
        "expected `neverImported` to still be flagged as orphan: {orphans:?}"
    );
    assert_eq!(
        orphans.len(),
        1,
        "only the genuinely unused export should be flagged: {orphans:?}"
    );
}
