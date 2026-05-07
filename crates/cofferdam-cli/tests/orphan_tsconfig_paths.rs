//! Regression test for cd-q9f — `Design.OrphanExport` must follow
//! `tsconfig.json` `paths`/`baseUrl` aliases (`@/*`) when the engine is
//! invoked with CWD-relative input.
//!
//! The original bug: running `cofferdam check .` from a Next.js-shaped
//! project with `paths: { "@/*": ["./*"] }` reported every export
//! consumed only via `@/...` as orphan. Cause: `oxc_resolver`'s
//! `TsconfigDiscovery::Auto` walks up from the importing file's
//! directory looking for `tsconfig.json` — with relative inputs like
//! `./components/Bar.tsx` that walk runs out before reaching the
//! project root, so the alias never resolves and the import doesn't
//! land in `OrphanExport`'s `touched` set.
//!
//! The fix lives in `cofferdam_engine::Engine::analyze_with_sources`,
//! which now promotes every input path to its absolute form via
//! `std::path::absolute` before any check sees it. We exercise it
//! end-to-end here by invoking the binary with `current_dir` set so
//! `.` is genuinely a relative input.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

#[test]
fn orphan_export_follows_tsconfig_paths_with_cwd_relative_target() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let root = dir.path();

    std::fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["./*"]}}}"#,
    )
    .expect("write tsconfig");

    std::fs::create_dir_all(root.join("lib")).expect("mkdir lib");
    std::fs::write(
        root.join("lib").join("useFoo.ts"),
        "export function useFoo() { return 1; }\n",
    )
    .expect("write useFoo");

    std::fs::create_dir_all(root.join("components")).expect("mkdir components");
    std::fs::write(
        root.join("components").join("Bar.tsx"),
        "import { useFoo } from \"@/lib/useFoo\";\nexport function Bar() { return useFoo(); }\n",
    )
    .expect("write Bar");

    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline", "--format=json", "."])
        .current_dir(root)
        .output()
        .expect("spawn cofferdam");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "cofferdam stdout was not valid JSON: {e}\nstdout={stdout}\nstderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    });

    let orphan_files: Vec<String> = v["findings"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|f| f["id"].as_str() == Some("Design.OrphanExport"))
                .filter_map(|f| f["file"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    assert!(
        !orphan_files.iter().any(|f| f.contains("useFoo")),
        "useFoo is reachable via @/lib/useFoo from Bar.tsx — must not be orphan. \
         Got OrphanExport on: {:?}\nstdout={stdout}",
        orphan_files
    );

    // Sanity: Bar IS orphan (nothing imports it). If even Bar isn't
    // flagged, the OrphanExport check isn't running and the assertion
    // above is vacuous — fail loudly instead of silently passing.
    assert!(
        orphan_files.iter().any(|f| f.contains("Bar.tsx")),
        "expected Bar to be orphan (no entry imports it) — sanity check, \
         confirms OrphanExport is actually running. orphan_files={:?}",
        orphan_files
    );
}
