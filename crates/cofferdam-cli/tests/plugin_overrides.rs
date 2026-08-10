//! Regression tests for CD-321: `[[overrides]] disabled = true` had no
//! effect on plugin-produced findings. Plugin checks run through the
//! Node host in `plugins.rs`, entirely outside the engine's
//! `self.checks` loop that `Engine::effective_options` /
//! `effective_severity` gate — so a `disabled = true` block silenced a
//! built-in check but left an identically-scoped plugin check firing.
//! `run_plugins_filtered` (`cofferdam-cli/src/main.rs`) now re-applies
//! `[[overrides]]` resolution to plugin issues the same way the engine
//! does for built-ins.

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

const ALWAYS_FLAGS_PLUGIN: &str = r#"
export default {
  id: "AlwaysFlags",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "cd-321 regression: flags every file unconditionally",
  requiresTypes: false,
  options: {},
  run(file, ctx) {
    const at = file.text.indexOf("export const a");
    ctx.report({
      message: "always flags",
      span: { line: 1, column: 1, start_byte: at, end_byte: at + 1 },
    });
  },
};
"#;

fn write_plugin_project(dir: &std::path::Path, toml_extra: &str) {
    let plugin_dir = dir.join("plugin");
    std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    std::fs::write(plugin_dir.join("index.mjs"), ALWAYS_FLAGS_PLUGIN).expect("write plugin");
    std::fs::write(
        dir.join("cofferdam.toml"),
        format!("plugins = [\"./plugin\"]\n{toml_extra}"),
    )
    .expect("write toml");
    std::fs::write(dir.join("a.ts"), "export const a = 1;\n").expect("write a.ts");
    std::fs::write(dir.join("b.ts"), "export const a = 1;\n").expect("write b.ts");
}

fn run_check(dir: &std::path::Path) -> String {
    let out = Command::new(cofferdam_bin())
        .args(["check", ".", "--format", "json", "--no-baseline"])
        .current_dir(dir)
        .output()
        .expect("spawn cofferdam");
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn wildcard_disabled_override_silences_plugin_check() {
    if !node_present() {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    write_plugin_project(
        dir.path(),
        r#"
[[overrides]]
paths = ["**"]
[overrides.checks."Warning.AlwaysFlags"]
disabled = true
"#,
    );
    let stdout = run_check(dir.path());
    assert!(
        !stdout.contains("Warning.AlwaysFlags"),
        "a `paths = [\"**\"]` disabled override must silence a plugin check entirely; \
         stdout={stdout}"
    );
}

#[test]
fn narrow_disabled_override_only_affects_matching_path() {
    if !node_present() {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    write_plugin_project(
        dir.path(),
        r#"
[[overrides]]
paths = ["a.ts"]
[overrides.checks."Warning.AlwaysFlags"]
disabled = true
"#,
    );
    let stdout = run_check(dir.path());
    let findings: serde_json::Value = serde_json::from_str(&stdout).expect("parse json");
    let files: Vec<&str> = findings["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|i| i["id"] == "Warning.AlwaysFlags")
        .map(|i| i["file"].as_str().unwrap_or_default())
        .collect();
    assert!(
        !files.iter().any(|f| f.ends_with("a.ts")),
        "a.ts should be disabled by the override; findings={stdout}"
    );
    assert!(
        files.iter().any(|f| f.ends_with("b.ts")),
        "b.ts is not matched by the override and must still report; findings={stdout}"
    );
}

#[test]
fn severity_override_restamps_plugin_finding() {
    if !node_present() {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    write_plugin_project(
        dir.path(),
        r#"
[[overrides]]
paths = ["**"]
[overrides.checks."Warning.AlwaysFlags"]
severity = "info"
"#,
    );
    let stdout = run_check(dir.path());
    let findings: serde_json::Value = serde_json::from_str(&stdout).expect("parse json");
    let sevs: Vec<&str> = findings["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|i| i["id"] == "Warning.AlwaysFlags")
        .map(|i| i["severity"].as_str().unwrap_or_default())
        .collect();
    assert!(!sevs.is_empty(), "plugin check must still report; {stdout}");
    assert!(
        sevs.iter().all(|s| *s == "info"),
        "the override must replace the plugin's own `defaultSeverity` of medium; {stdout}"
    );
}

#[test]
fn plugin_check_reports_normally_with_no_override() {
    if !node_present() {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    write_plugin_project(dir.path(), "");
    let stdout = run_check(dir.path());
    assert!(
        stdout.contains("Warning.AlwaysFlags"),
        "no regression: plugin check must still report with no [[overrides]] configured; \
         stdout={stdout}"
    );
}
