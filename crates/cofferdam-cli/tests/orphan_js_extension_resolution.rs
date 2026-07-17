//! Regression test for `Design.OrphanExport` false-positives on TypeScript's
//! NodeNext/ESM `.js`-extension import convention (CD-104).
//!
//! Under `"module": "NodeNext"` / `"moduleResolution": "NodeNext"`, Node's ESM
//! loader requires import specifiers to carry the *compiled output*
//! extension (`.js`) even though the source file on disk is `.ts` — e.g.
//! `import { x } from "./foo.js"` where only `foo.ts` exists. Without
//! resolving `.js` specifiers against sibling `.ts`/`.tsx` files, every such
//! import fails to resolve and the target's export is reported orphan.

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
    v["findings"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|f| f["id"].as_str() == Some("Design.OrphanExport"))
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

/// The exact CD-104 repro: a same-directory relative import written with a
/// `.js` specifier against a `.ts` file on disk, under NodeNext resolution.
#[test]
fn orphan_export_not_flagged_for_js_extension_relative_import() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let root = dir.path();

    std::fs::write(
        root.join("package.json"),
        r#"{"name": "repro", "type": "module"}"#,
    )
    .expect("write package.json");

    std::fs::write(
        root.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "module": "NodeNext",
    "moduleResolution": "NodeNext"
  }
}"#,
    )
    .expect("write tsconfig");

    std::fs::write(
        root.join("difficulty.ts"),
        "export function deriveDifficultyProfile() { return 1; }\n",
    )
    .expect("write difficulty.ts");

    std::fs::write(
        root.join("aiTactics.ts"),
        "import { deriveDifficultyProfile } from \"./difficulty.js\";\nconsole.log(deriveDifficultyProfile());\n",
    )
    .expect("write aiTactics.ts");

    write_genuine_orphan(root);
    let (v, stdout) = run_check(root);
    let orphans = orphan_files(&v);

    assert!(
        orphans.iter().any(|f| f.contains("dead")),
        "expected lib/dead.ts to be orphan — confirms OrphanExport is running.\n\
         orphans={orphans:?}"
    );

    assert!(
        !orphans.iter().any(|f| f.contains("difficulty")),
        "deriveDifficultyProfile is imported via \"./difficulty.js\" from aiTactics.ts \
         (NodeNext .js-extension convention against a .ts file on disk) — must not be orphan.\n\
         Got OrphanExport on: {orphans:?}\nstdout={stdout}"
    );
}

/// Same convention one directory level down, and with a `.tsx` target, to
/// confirm the extension-alias fallback isn't limited to same-dir `.ts`.
#[test]
fn orphan_export_not_flagged_for_js_extension_nested_tsx_import() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let root = dir.path();

    std::fs::write(
        root.join("package.json"),
        r#"{"name": "repro", "type": "module"}"#,
    )
    .expect("write package.json");

    std::fs::write(
        root.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "jsx": "react-jsx"
  }
}"#,
    )
    .expect("write tsconfig");

    std::fs::create_dir_all(root.join("components")).expect("mkdir components");
    std::fs::write(
        root.join("components").join("Widget.tsx"),
        "export function Widget() { return null; }\n",
    )
    .expect("write Widget.tsx");

    std::fs::write(
        root.join("main.ts"),
        "import { Widget } from \"./components/Widget.js\";\nconsole.log(Widget);\n",
    )
    .expect("write main.ts");

    write_genuine_orphan(root);
    let (v, stdout) = run_check(root);
    let orphans = orphan_files(&v);

    assert!(
        orphans.iter().any(|f| f.contains("dead")),
        "expected lib/dead.ts to be orphan — sanity check. orphans={orphans:?}"
    );

    assert!(
        !orphans.iter().any(|f| f.contains("Widget")),
        "Widget is imported via \"./components/Widget.js\" (NodeNext convention against \
         a .tsx file on disk) — must not be orphan.\nGot OrphanExport on: {orphans:?}\nstdout={stdout}"
    );
}
