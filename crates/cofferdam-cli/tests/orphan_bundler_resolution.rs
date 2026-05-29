//! Regression tests for `Design.OrphanExport` false-positives when the
//! project's `tsconfig.json` sets `"moduleResolution": "bundler"` (Next.js 16
//! / TypeScript 5 bundler mode) and/or has an `"exclude"` list that covers
//! test files.
//!
//! Both patterns cause `oxc_resolver`'s `TsconfigDiscovery::Auto` to silently
//! return `None` for `@/`-aliased specifiers, leaving the import unresolved and
//! the importing file's consumption invisible to `OrphanExport`.
//!
//! Fixes tracked in GitHub issue #52.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

/// Writes a minimal project tree and returns the JSON findings from
/// `cofferdam check --no-baseline --format=json .`.
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

/// Sanity helper: a file that genuinely has no importer must be flagged.
/// Used by other tests to verify OrphanExport is actually running.
fn write_genuine_orphan(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("lib")).ok();
    std::fs::write(
        root.join("lib").join("dead.ts"),
        "export function dead() { return 0; }\n",
    )
    .expect("write dead");
}

/// `moduleResolution: "bundler"` — Next.js 16 / TS 5+ default.
///
/// A lib export consumed only from `app/**` via `@/` should not be orphan.
#[test]
fn orphan_export_not_flagged_with_bundler_module_resolution() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let root = dir.path();

    std::fs::write(
        root.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "moduleResolution": "bundler",
    "paths": { "@/*": ["./*"] }
  }
}"#,
    )
    .expect("write tsconfig");

    std::fs::create_dir_all(root.join("lib")).expect("mkdir lib");
    std::fs::write(
        root.join("lib").join("utils.ts"),
        "export function helper() { return 42; }\n",
    )
    .expect("write utils");

    // Nested app-router page — the importer is two levels deep.
    std::fs::create_dir_all(root.join("app").join("some-feature")).expect("mkdir app/some-feature");
    std::fs::write(
        root.join("app").join("some-feature").join("page.tsx"),
        "import { helper } from \"@/lib/utils\";\nexport default function Page() { return helper(); }\n",
    )
    .expect("write page");

    write_genuine_orphan(root);
    let (v, stdout) = run_check(root);
    let orphans = orphan_files(&v);

    // Sanity: lib/dead.ts has no importer — must be flagged.
    assert!(
        orphans.iter().any(|f| f.contains("dead")),
        "expected lib/dead.ts to be orphan — confirms OrphanExport is running.\n\
         orphans={orphans:?}"
    );

    assert!(
        !orphans.iter().any(|f| f.contains("utils")),
        "helper() is reachable via @/lib/utils from app/some-feature/page.tsx — must not be orphan.\n\
         Got OrphanExport on: {orphans:?}\nstdout={stdout}"
    );
}

/// `"exclude": ["**/*.test.ts"]` in tsconfig — common pattern for projects
/// that use a separate jest/vitest tsconfig or want TS not to compile tests.
///
/// Exports consumed ONLY from a test file must not be reported as orphans even
/// when the tsconfig excludes that test file from compilation.
#[test]
fn orphan_export_not_flagged_when_importer_is_tsconfig_excluded_test_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let root = dir.path();

    std::fs::write(
        root.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": { "@/*": ["./*"] }
  },
  "exclude": ["node_modules", "**/*.test.ts", "**/*.test.tsx"]
}"#,
    )
    .expect("write tsconfig");

    std::fs::create_dir_all(root.join("lib")).expect("mkdir lib");
    std::fs::write(
        root.join("lib").join("math.ts"),
        "export function add(a: number, b: number) { return a + b; }\n",
    )
    .expect("write math");

    std::fs::write(
        root.join("lib").join("math.test.ts"),
        "import { add } from \"@/lib/math\";\nconsole.log(add(1, 2));\n",
    )
    .expect("write math test");

    write_genuine_orphan(root);
    let (v, stdout) = run_check(root);
    let orphans = orphan_files(&v);

    assert!(
        orphans.iter().any(|f| f.contains("dead")),
        "expected lib/dead.ts to be orphan — sanity check. orphans={orphans:?}"
    );

    assert!(
        !orphans.iter().any(|f| f.contains("math.ts") && !f.contains("math.test")),
        "add() is reachable from math.test.ts — must not be orphan even though test file is tsconfig-excluded.\n\
         Got OrphanExport on: {orphans:?}\nstdout={stdout}"
    );
}

/// Combined: `moduleResolution: "bundler"` + `exclude` test files + nested
/// app-router importer. This is the exact shape of the bestefforttools Next.js
/// 16 project that originally surfaced the issue.
#[test]
fn orphan_export_nextjs16_combined_pattern() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let root = dir.path();

    std::fs::write(
        root.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "baseUrl": ".",
    "moduleResolution": "bundler",
    "paths": { "@/*": ["./*"] }
  },
  "exclude": ["node_modules", "**/*.test.ts", "**/*.test.tsx"]
}"#,
    )
    .expect("write tsconfig");

    std::fs::create_dir_all(root.join("lib")).expect("mkdir lib");
    std::fs::write(
        root.join("lib").join("pacing.ts"),
        "export function calcPace(t: number) { return t / 10; }\nexport const VERSION = 1;\n",
    )
    .expect("write pacing");

    std::fs::write(
        root.join("lib").join("pacing.test.ts"),
        "import { calcPace } from \"@/lib/pacing\";\nconsole.log(calcPace(100));\n",
    )
    .expect("write pacing test");

    std::fs::create_dir_all(root.join("app").join("pacing-calculator"))
        .expect("mkdir app/pacing-calculator");
    std::fs::write(
        root.join("app").join("pacing-calculator").join("page.tsx"),
        "import { calcPace, VERSION } from \"@/lib/pacing\";\nexport default function Page() { return calcPace(VERSION); }\n",
    )
    .expect("write page");

    write_genuine_orphan(root);
    let (v, stdout) = run_check(root);
    let orphans = orphan_files(&v);

    assert!(
        orphans.iter().any(|f| f.contains("dead")),
        "expected lib/dead.ts to be orphan — sanity check. orphans={orphans:?}"
    );

    assert!(
        !orphans
            .iter()
            .any(|f| f.contains("pacing.ts") && !f.contains("pacing.test")),
        "calcPace/VERSION are reachable via @/lib/pacing from both page.tsx and pacing.test.ts.\n\
         Must not be flagged as orphan in bundler-mode + exclude project.\n\
         Got OrphanExport on: {orphans:?}\nstdout={stdout}"
    );
}

/// Full bestefforttools tsconfig shape — includes `plugins`, `include`,
/// `jsx`, and all other fields present in the real Next.js 16 project.
///
/// Reproduces the exact pattern that triggered issue #52.
#[test]
fn orphan_export_full_nextjs_tsconfig_shape() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let root = dir.path();

    std::fs::write(
        root.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["dom", "dom.iterable", "esnext"],
    "allowJs": true,
    "skipLibCheck": true,
    "strict": true,
    "noEmit": true,
    "esModuleInterop": true,
    "module": "esnext",
    "moduleResolution": "bundler",
    "resolveJsonModule": true,
    "isolatedModules": true,
    "jsx": "react-jsx",
    "incremental": true,
    "plugins": [{"name": "next"}],
    "paths": {"@/*": ["./*"]},
    "baseUrl": "."
  },
  "include": ["**/*.ts", "**/*.tsx"],
  "exclude": ["node_modules", "**/*.test.ts", "**/*.test.tsx", "tests"]
}"#,
    )
    .expect("write tsconfig");

    // components/BlogPageContent.tsx — the component imported by app/blog/page.tsx
    std::fs::create_dir_all(root.join("components")).expect("mkdir components");
    std::fs::write(
        root.join("components").join("BlogPageContent.tsx"),
        "export function BlogPageContent({ posts }: { posts: unknown[] }) { return null; }\n",
    )
    .expect("write BlogPageContent");

    // app/blog/page.tsx — Next.js App Router page that imports BlogPageContent
    std::fs::create_dir_all(root.join("app").join("blog")).expect("mkdir app/blog");
    std::fs::write(
        root.join("app").join("blog").join("page.tsx"),
        "import { BlogPageContent } from '@/components/BlogPageContent';\nexport default function BlogPage() { return <BlogPageContent posts={[]} />; }\n",
    )
    .expect("write blog page");

    // lib/utils.ts — imported by a test file (which is tsconfig-excluded)
    std::fs::create_dir_all(root.join("lib")).expect("mkdir lib");
    std::fs::write(
        root.join("lib").join("utils.ts"),
        "export function add(a: number, b: number) { return a + b; }\n",
    )
    .expect("write utils");
    std::fs::write(
        root.join("lib").join("utils.test.ts"),
        "import { add } from '@/lib/utils';\nconsole.log(add(1, 2));\n",
    )
    .expect("write utils test");

    write_genuine_orphan(root);
    let (v, stdout) = run_check(root);
    let orphans = orphan_files(&v);

    assert!(
        orphans.iter().any(|f| f.contains("dead")),
        "expected lib/dead.ts to be orphan — confirms OrphanExport is running.\n\
         orphans={orphans:?}"
    );

    assert!(
        !orphans.iter().any(|f| f.contains("BlogPageContent")),
        "BlogPageContent is imported by app/blog/page.tsx — must not be orphan.\n\
         Got OrphanExport on: {orphans:?}\nstdout={stdout}"
    );

    assert!(
        !orphans.iter().any(|f| f.contains("utils.ts") && !f.contains("utils.test")),
        "add() in lib/utils.ts is imported by lib/utils.test.ts — must not be orphan even with exclude.\n\
         Got OrphanExport on: {orphans:?}\nstdout={stdout}"
    );
}
