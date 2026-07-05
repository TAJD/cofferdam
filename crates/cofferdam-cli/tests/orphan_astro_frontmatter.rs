//! Regression test for `Design.OrphanExport` false-positives on components
//! imported only from `.astro` frontmatter (CD-45).
//!
//! An `.astro` file's template body isn't valid TS/JS, so the engine never
//! parses the whole file — but its frontmatter fence (`---\n...\n---`) is
//! plain ESM, and the engine now extracts imports from just that region
//! into the shared import graph.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

fn run_check(root: &std::path::Path) -> (serde_json::Value, String) {
    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline", "--format=json", "."])
        .current_dir(root)
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "cofferdam stdout not valid JSON: {e}\nstdout={stdout}\nstderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (v, stdout)
}

fn orphan_files(v: &serde_json::Value) -> Vec<String> {
    findings_for(v, "Design.OrphanExport")
}

fn findings_for(v: &serde_json::Value, check_id: &str) -> Vec<String> {
    v["findings"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|f| f["id"].as_str() == Some(check_id))
                .filter_map(|f| f["file"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn write_genuine_orphan(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("lib")).ok();
    std::fs::write(
        root.join("lib").join("dead.ts"),
        "export function dead() { return 0; }\n",
    )
    .expect("write dead");
}

/// `MyIssues.tsx`'s default export is imported only from `pages/my-issues.astro`'s
/// frontmatter — no `.tsx`/`.ts` file references it. Must not be orphan.
#[test]
fn orphan_export_not_flagged_when_imported_only_from_astro_frontmatter() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let root = dir.path();

    std::fs::create_dir_all(root.join("islands")).expect("mkdir islands");
    std::fs::write(
        root.join("islands").join("MyIssues.tsx"),
        "export default function MyIssues() { return null; }\n",
    )
    .expect("write MyIssues");

    std::fs::create_dir_all(root.join("pages")).expect("mkdir pages");
    std::fs::write(
        root.join("pages").join("my-issues.astro"),
        "---\nimport MyIssues from '../islands/MyIssues';\n---\n<MyIssues client:load />\n",
    )
    .expect("write my-issues.astro");

    write_genuine_orphan(root);
    let (v, stdout) = run_check(root);
    let orphans = orphan_files(&v);

    assert!(
        orphans.iter().any(|f| f.contains("dead")),
        "expected lib/dead.ts to be orphan — confirms OrphanExport is running.\n\
         orphans={orphans:?}"
    );

    assert!(
        !orphans.iter().any(|f| f.contains("MyIssues")),
        "MyIssues is imported from pages/my-issues.astro's frontmatter — must not be orphan.\n\
         Got OrphanExport on: {orphans:?}\nstdout={stdout}"
    );

    // The template body (`<MyIssues client:load />`) is invisible to the
    // frontmatter-only parse — Refactor.DeadExport's "imported but never
    // referenced" heuristic must not fire off that blind spot.
    let dead_exports = findings_for(&v, "Refactor.DeadExport");
    assert!(
        !dead_exports.iter().any(|f| f.contains("MyIssues")),
        "MyIssues is used in the .astro template, invisible to frontmatter parsing — \
         must not be flagged as a dead export.\nGot Refactor.DeadExport on: {dead_exports:?}\nstdout={stdout}"
    );
}

/// A component imported by no `.ts(x)` file AND no `.astro` file must still
/// be flagged — the frontmatter extraction shouldn't turn OrphanExport off
/// wholesale for the islands directory.
#[test]
fn orphan_export_still_flagged_when_no_astro_file_imports_it() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let root = dir.path();

    std::fs::create_dir_all(root.join("islands")).expect("mkdir islands");
    std::fs::write(
        root.join("islands").join("Unused.tsx"),
        "export default function Unused() { return null; }\n",
    )
    .expect("write Unused");

    std::fs::create_dir_all(root.join("pages")).expect("mkdir pages");
    std::fs::write(
        root.join("pages").join("home.astro"),
        "---\nconst title = 'Home';\n---\n<h1>{title}</h1>\n",
    )
    .expect("write home.astro");

    let (v, stdout) = run_check(root);
    let orphans = orphan_files(&v);

    assert!(
        orphans.iter().any(|f| f.contains("Unused")),
        "Unused.tsx has no importer anywhere, including .astro files — must be flagged.\n\
         Got OrphanExport on: {orphans:?}\nstdout={stdout}"
    );
}
