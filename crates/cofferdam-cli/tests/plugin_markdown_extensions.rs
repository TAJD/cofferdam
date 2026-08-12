//! Integration tests for CD-68: `.md`/`.mdx` files reaching plugin
//! checks via `[engine] extra_extensions`, with no whole-file parse
//! (`file.ast === null`).
//!
//! Since CD-316 a built-in can reach them too — `Check::languages()`
//! returns a set, and `Consistency.SpellingDialect` declares Markdown
//! alongside TypeScript. Discovery is still opt-in, so a project that
//! does not list `md` sees no change.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

/// Plugin scoped to `.md` files. Reports whether `file.ast` is null and
/// the byte length of `file.text`, so the test can assert both without
/// a second round-trip.
const MARKDOWN_PLUGIN: &str = r#"
export default {
  id: "Test.MarkdownPlugin",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "cd-68 integration test plugin — fires on markdown files",
  requiresTypes: false,
  options: {},
  files: { extensions: ["md"] },
  run(file, ctx) {
    ctx.report({
      message: "markdown plugin saw " + file.text.length + " bytes, ast=" + (file.ast === null ? "null" : "non-null"),
      span: { start_byte: 0, end_byte: 1 },
    });
  },
};
"#;

fn node_present() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Mirrors `plugin_findings.rs`'s lock — one live plugin host at a time
/// per test binary avoids the starvation flake documented there.
static PLUGIN_HOST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serialize_plugin_host() -> std::sync::MutexGuard<'static, ()> {
    PLUGIN_HOST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_project(dir: &std::path::Path, cofferdam_toml: &str) {
    let plugin_dir = dir.join("plugin");
    std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    std::fs::write(plugin_dir.join("index.mjs"), MARKDOWN_PLUGIN).expect("write plugin");
    std::fs::write(dir.join("cofferdam.toml"), cofferdam_toml).expect("write toml");
    std::fs::write(dir.join("a.ts"), "export const a = 1;\n").expect("write ts");
    std::fs::write(dir.join("post.md"), "# Title\n\nSome body text.\n").expect("write md");
}

#[test]
fn markdown_file_unreachable_without_extra_extensions() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let dir = tempfile::TempDir::new().expect("temp dir");
    write_project(dir.path(), "plugins = [\"./plugin\"]\n");

    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stdout.contains("Test.MarkdownPlugin"),
        "without [engine] extra_extensions, .md files must not be discovered at all; \
         stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn markdown_file_reaches_plugin_via_extra_extensions_with_no_ast() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let dir = tempfile::TempDir::new().expect("temp dir");
    write_project(
        dir.path(),
        "plugins = [\"./plugin\"]\n\n[engine]\nextra_extensions = [\"md\"]\n",
    );

    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("Test.MarkdownPlugin"),
        "[engine] extra_extensions = [\"md\"] should let the plugin see post.md; \
         stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("ast=null"),
        "markdown files get no whole-file parse — file.ast must be null; \
         stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("ParseError"),
        "markdown files must never produce Warning.ParseError; stdout={stdout}\nstderr={stderr}"
    );
}

/// CD-316: `Consistency.SpellingDialect` reads Markdown as well as
/// TypeScript, which is the corpus that motivated it. No plugin, so no
/// node dependency and no host lock.
#[test]
fn builtin_spelling_dialect_reaches_markdown() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "[engine]\nextra_extensions = [\"md\"]\n\n\
         [checks.\"Consistency.SpellingDialect\"]\ndialect = \"british\"\n",
    )
    .expect("write toml");
    std::fs::write(dir.path().join("a.ts"), "export const a = 1;\n").expect("write ts");
    std::fs::write(
        dir.path().join("post.md"),
        "# Title\n\nWe analyze the behavior here, but not `normalize`.\n",
    )
    .expect("write md");

    let out = Command::new(cofferdam_bin())
        .args([
            "check",
            "--no-baseline",
            "--no-cache",
            "--only",
            "Consistency.SpellingDialect",
            "--format",
            "json",
        ])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("analyze") && stdout.contains("behavior"),
        "prose in post.md should be scanned; stdout={stdout}"
    );
    assert!(
        !stdout.contains("normalize"),
        "an inline code span is quoted code, not prose; stdout={stdout}"
    );
}
