//! Integration tests for plugin findings flowing through the full
//! cofferdam pipeline: default text output, the `--fail-on` gate, and
//! `baseline write`. Regression cover for cd-1c7 (plugin findings were
//! visible only via `--no-baseline --format=json`).
//!
//! Each test stands up a tempdir with a minimal ESM plugin that fires
//! one finding on every TypeScript file, then drives the `cofferdam`
//! binary against it.

use std::path::PathBuf;
use std::process::Command;

fn cofferdam_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cofferdam"))
}

/// Plugin that reports one Medium finding on the first byte of every
/// `.ts` file it sees. Plain ESM, no SDK dependency.
const ALWAYS_FIRES_PLUGIN: &str = r#"
export default {
  id: "Test.AlwaysFires",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "always-fires plugin used for cd-1c7 integration tests",
  requiresTypes: false,
  options: {},
  run(file, ctx) {
    ctx.report({
      message: "plugin finding from Test.AlwaysFires",
      span: { start_byte: 0, end_byte: 1 },
    });
  },
};
"#;

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self::with_plugin(ALWAYS_FIRES_PLUGIN)
    }

    fn with_plugin(plugin_source: &str) -> Self {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let plugin_dir = dir.path().join("plugin");
        std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
        std::fs::write(plugin_dir.join("index.mjs"), plugin_source).expect("write plugin");
        std::fs::write(
            dir.path().join("cofferdam.toml"),
            "plugins = [\"./plugin\"]\n",
        )
        .expect("write toml");
        // One trivial TS file the plugin can fire on. Content irrelevant —
        // the plugin reports unconditionally.
        std::fs::write(dir.path().join("a.ts"), "export const a = 1;\n").expect("write ts");
        Env { dir }
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(cofferdam_bin())
            .args(args)
            .current_dir(self.dir.path())
            .output()
            .expect("spawn cofferdam")
    }

    fn baseline_path(&self) -> PathBuf {
        self.dir.path().join(".cofferdam").join("baseline.json")
    }
}

/// Skip the body of a test when Node isn't on PATH — cd-1c7 tests all
/// require the plugin host runtime. Returns true when the test should
/// continue, false when it should be skipped silently.
fn node_present() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Serialize the plugin-host tests. Each spawns a `cofferdam` process
/// that launches a Node plugin host plus a type-host worker pool;
/// cargo runs the tests in this file concurrently by default, so on a
/// constrained CI runner several node hosts start at once, starve, and
/// intermittently return zero findings with no error surfaced. Holding
/// this lock for the body of each test keeps exactly one host live at a
/// time. `into_inner` recovers from a poisoned lock (a panicking test)
/// so one failure doesn't cascade into the rest.
static PLUGIN_HOST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serialize_plugin_host() -> std::sync::MutexGuard<'static, ()> {
    PLUGIN_HOST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Plugin whose `finalize` reports one finding on a path that was never
/// part of the analyzed file set, and one on a real analyzed file (the
/// path is stashed during `run`). Regression cover for cd-neav: the
/// out-of-scope report must be dropped and replaced by an aggregated
/// `Warning.PluginHostFailed`, while the in-scope report survives.
const OUT_OF_SCOPE_PLUGIN: &str = r#"
export default {
  id: "Test.OutOfScope",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "finalize-mode plugin that reports an out-of-scope path",
  requiresTypes: false,
  options: {},
  run(file, ctx) { globalThis.__cofferdamSeenPath = file.path; },
  finalize(ctx) {
    ctx.report({
      message: "bogus out-of-scope finding",
      file: "/definitely/not/analyzed/evil.ts",
      span: { start_byte: 0, end_byte: 1 },
    });
    ctx.report({
      message: "legit in-scope finding",
      file: globalThis.__cofferdamSeenPath,
      span: { start_byte: 0, end_byte: 1 },
    });
  },
};
"#;

#[test]
fn out_of_scope_plugin_report_is_dropped_with_warning() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let env = Env::with_plugin(OUT_OF_SCOPE_PLUGIN);
    let out = env.run(&["check", "--no-baseline"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("legit in-scope finding"),
        "in-scope finalize report must survive; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("bogus out-of-scope finding"),
        "out-of-scope finalize report must be dropped; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("outside the analyzed file set"),
        "dropped report must surface as an aggregated PluginHostFailed warning; \
         stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn plugin_finding_appears_in_default_text_output() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let env = Env::new();
    let out = env.run(&["check", "--no-baseline"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("Test.AlwaysFires"),
        "default text output should include plugin finding; stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn plugin_finding_triggers_fail_on_gate() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let env = Env::new();
    // Plugin finding is severity=medium; --fail-on=medium should fail.
    let out = env.run(&["check", "--no-baseline", "--fail-on", "medium"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "plugin finding at fail-on threshold must trigger non-zero exit; \
         stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn plugin_finding_lands_in_baseline_write() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let env = Env::new();
    let out = env.run(&["baseline", "write"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "baseline write should succeed; stderr={stderr}"
    );
    let baseline = std::fs::read_to_string(env.baseline_path()).expect("read baseline");
    assert!(
        baseline.contains("Test.AlwaysFires"),
        "baseline should record plugin findings; got: {baseline}"
    );
}

#[test]
fn plugin_finding_baselined_is_not_a_new_failure() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let env = Env::new();
    // Write the baseline first so the plugin finding is acknowledged...
    let out = env.run(&["baseline", "write"]);
    assert!(out.status.success(), "baseline write should succeed");
    // ...then re-run check at fail-on=medium. The same finding now
    // matches the baseline and must NOT trigger the gate.
    let out = env.run(&["check", "--fail-on", "medium"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "baselined plugin finding must not re-trigger the gate; \
         stdout={stdout}\nstderr={stderr}"
    );
}
