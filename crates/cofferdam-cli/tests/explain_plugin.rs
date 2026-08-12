//! Integration tests for `cofferdam explain` with plugin-declared checks.
//!
//! These tests verify the plugin fallback path in explain.rs:
//!   - explain finds and renders a plugin check's explanation.
//!   - `--full` surfaces the long-form body (or the no-body note).
//!   - `--robot` emits valid JSON with the expected fields.
//!   - `explain UnknownId` includes plugin IDs in the suggestion list.
//!
//! Each test creates a temporary directory with a bare cofferdam.toml and
//! a minimal plugin written as a plain ESM module (no @cofferdam/check-sdk
//! import needed — the host accepts any object with `id` + `run`).

use std::path::PathBuf;
use std::process::Command;

/// Returns the path to the compiled `cofferdam` binary built by Cargo.
fn cofferdam_bin() -> PathBuf {
    // CARGO_BIN_EXE_cofferdam is set when the test is run via `cargo test`
    // on a crate that declares a `[[bin]]` with that name.
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

/// Minimal plugin source — a plain ESM default export with just the
/// fields the host validates (`id`, `run`). Does NOT import the SDK, so
/// it resolves cleanly in a bare tempdir without node_modules.
const PLUGIN_SOURCE: &str = r#"
export default {
  id: "Test.PluginCheck",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "this is a plugin check explanation",
  body: "long-form body for --full mode",
  requiresTypes: false,
  options: {},
  run() {},
};
"#;

/// Minimal plugin source **without** a body field (tests the no-body path).
const PLUGIN_NO_BODY_SOURCE: &str = r#"
export default {
  id: "Test.NobodyCheck",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "a check with no long-form body",
  requiresTypes: false,
  options: {},
  run() {},
};
"#;

struct TestEnv {
    dir: tempfile::TempDir,
}

impl TestEnv {
    /// Create a tempdir with `cofferdam.toml` pointing at `./plugin` and
    /// write `plugin/index.mjs` with the given source.
    fn with_plugin(plugin_src: &str) -> Self {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let plugin_dir = dir.path().join("plugin");
        std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
        std::fs::write(plugin_dir.join("index.mjs"), plugin_src).expect("write plugin");
        std::fs::write(
            dir.path().join("cofferdam.toml"),
            "plugins = [\"./plugin\"]\n",
        )
        .expect("write toml");
        TestEnv { dir }
    }

    /// Run `cofferdam explain <check_id> [extra_args...]` from the tempdir.
    fn explain(&self, check_id: &str, extra_args: &[&str]) -> std::process::Output {
        Command::new(cofferdam_bin())
            .arg("explain")
            .arg(check_id)
            .args(extra_args)
            .current_dir(self.dir.path())
            .output()
            .expect("spawn cofferdam")
    }
}

#[test]
fn explain_plugin_check_shows_explanation() {
    let env = TestEnv::with_plugin(PLUGIN_SOURCE);
    let out = env.explain("Test.PluginCheck", &[]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0; stderr={stderr}\nstdout={stdout}"
    );
    assert!(
        stdout.contains("this is a plugin check explanation"),
        "stdout should contain explanation; got: {stdout}"
    );
    assert!(
        stdout.contains("Test.PluginCheck"),
        "stdout should contain check id; got: {stdout}"
    );
}

#[test]
fn explain_plugin_check_full_shows_body() {
    let env = TestEnv::with_plugin(PLUGIN_SOURCE);
    let out = env.explain("Test.PluginCheck", &["--full"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0; stderr={stderr}\nstdout={stdout}"
    );
    assert!(
        stdout.contains("long-form body for --full mode"),
        "stdout should contain body; got: {stdout}"
    );
}

#[test]
fn explain_plugin_check_robot_emits_json() {
    let env = TestEnv::with_plugin(PLUGIN_SOURCE);
    let out = env.explain("Test.PluginCheck", &["--robot", "--pretty"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0; stderr={stderr}\nstdout={stdout}"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("robot output should be valid JSON");
    assert_eq!(parsed["id"], "Test.PluginCheck");
    assert_eq!(parsed["category"], "warning");
    assert_eq!(parsed["explanation"], "this is a plugin check explanation");
    // body absent when --full is not passed
    assert!(
        parsed["body"].is_null() || !parsed.as_object().unwrap().contains_key("body"),
        "body should be absent without --full; got: {parsed}"
    );
}

#[test]
fn explain_plugin_check_robot_full_includes_body() {
    let env = TestEnv::with_plugin(PLUGIN_SOURCE);
    let out = env.explain("Test.PluginCheck", &["--robot", "--full"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0; stderr={stderr}\nstdout={stdout}"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("robot output should be valid JSON");
    assert_eq!(parsed["body"], "long-form body for --full mode");
}

#[test]
fn explain_plugin_no_body_shows_note() {
    let env = TestEnv::with_plugin(PLUGIN_NO_BODY_SOURCE);
    let out = env.explain("Test.NobodyCheck", &["--full"]);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0; stderr={stderr}\nstdout={stdout}"
    );
    assert!(
        stdout.contains("does not ship a long-form body"),
        "should note missing body; got: {stdout}"
    );
}

#[test]
fn explain_unknown_id_suggests_plugin_ids() {
    let env = TestEnv::with_plugin(PLUGIN_SOURCE);
    // "PluginCheck" is a substring of "Test.PluginCheck"
    let out = env.explain("PluginCheck", &[]);

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "expected non-zero exit for unknown check"
    );
    assert!(
        stderr.contains("Test.PluginCheck"),
        "suggestions should include plugin id; stderr: {stderr}"
    );
}

#[test]
fn explain_builtin_check_still_works() {
    // Built-in lookup should be unaffected — no config needed for that path.
    let out = Command::new(cofferdam_bin())
        .arg("explain")
        .arg("Refactor.PreferConstOverLet")
        .arg("--no-config")
        .output()
        .expect("spawn cofferdam");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "expected exit 0; stdout={stdout}");
    assert!(
        stdout.contains("Refactor.PreferConstOverLet"),
        "should include check id; got: {stdout}"
    );
}
