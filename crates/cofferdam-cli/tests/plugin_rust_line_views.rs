//! Regression test for CD-93: the plugin wire's per-language dispatch
//! (`plugins.rs`) gave `.rs` files `(Vec::new(), None)` for `lineViews`/
//! `ast` while `file.text` remained populated — a Pattern-A line-scan
//! plugin check (`file.lines()`) scoped to `.rs` would silently iterate
//! zero lines instead of erroring or being skipped. `.astro` already got
//! `Lines::plain`-built (unclassified) line views for the same reason;
//! Rust gets the identical treatment here.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

fn node_present() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Reports one finding per line seen via `file.lines()`, scoped to `.rs`
/// only. Plain ESM, no SDK dependency (matches the style of other
/// plain-ESM plugin fixtures in this test suite).
const RUST_LINE_COUNTER_PLUGIN: &str = r#"
export default {
  id: "Test.RustLineCount",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "reports one finding per line seen via file.lines(), for CD-93 regression cover",
  requiresTypes: false,
  options: {},
  files: { extensions: ["rs"] },
  run(file, ctx) {
    for (const ln of file.lines()) {
      ctx.report({
        message: `line ${ln.lineNo}: ${ln.text}`,
        span: { start_byte: 0, end_byte: 1 },
      });
    }
  },
};
"#;

#[test]
fn rust_files_get_real_line_views_not_empty() {
    if !node_present() {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let plugin_dir = dir.path().join("plugin");
    std::fs::create_dir_all(&plugin_dir).expect("mkdir plugin");
    std::fs::write(plugin_dir.join("index.mjs"), RUST_LINE_COUNTER_PLUGIN).expect("write plugin");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "plugins = [\"./plugin\"]\n",
    )
    .expect("write toml");
    std::fs::write(
        dir.path().join("a.rs"),
        "fn main() {\n    println!(\"hi\");\n}\n",
    )
    .expect("write rs");

    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline", "--format=json", "."])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("cofferdam stdout not valid JSON: {e}\nstdout={stdout}\nstderr={stderr}")
    });

    let findings = v["findings"].as_array().expect("findings array");
    let rust_line_findings: Vec<&serde_json::Value> = findings
        .iter()
        .filter(|f| f["id"].as_str() == Some("Test.RustLineCount"))
        .collect();

    assert!(
        !rust_line_findings.is_empty(),
        "file.lines() must yield real line views for a .rs file, not silently \
         iterate zero; findings={findings:?}\nstderr={stderr}"
    );
}
