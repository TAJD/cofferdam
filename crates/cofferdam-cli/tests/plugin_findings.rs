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

/// Plugin that calls `process.exit(0)` from inside `run()`, before the
/// host ever reaches `finalizeAndEmit`'s closing `{"type":"done"}`
/// record. Reproduces the cd-41 gap directly through the real host
/// contract: the child exits with status 0 and an empty/truncated
/// stdout, exactly the "successful exit, no completion marker" case
/// that used to be indistinguishable from a legitimate empty result.
const EXITS_WITHOUT_DONE_MARKER_PLUGIN: &str = r#"
export default {
  id: "Test.ExitsWithoutDoneMarker",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "exits the host process before the completion marker is emitted",
  requiresTypes: false,
  options: {},
  run(file, ctx) {
    process.exit(0);
  },
};
"#;

#[test]
fn host_exit_without_completion_marker_surfaces_as_plugin_host_failed() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let env = Env::with_plugin(EXITS_WITHOUT_DONE_MARKER_PLUGIN);
    let out = env.run(&["check", "--no-baseline"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("PluginHostFailed"),
        "host exiting cleanly without ever emitting `done` must surface as \
         Warning.PluginHostFailed rather than a silent empty result; \
         stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("completion marker"),
        "the failure message should name the actual gap (missing completion \
         marker), not a generic host error; stdout={stdout}\nstderr={stderr}"
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

// ---- CD-81: requiresTypes plugin routing through the in-process ts-morph
// type host ------------------------------------------------------------
//
// Gated on COFFERDAM_TYPE_HOST_TS_MORPH_ROOT (same var `type_host.rs`'s
// Rust tests use) pointing at a directory whose `node_modules` contains
// ts-morph — skips silently when unset so CI without ts-morph stays
// green. The test project is created *inside* that directory (via
// `tempfile::Builder::tempdir_in`) so `plugin-host.mjs`'s
// `cwd`-relative `node_modules` walk (see
// `type-host-core.mjs::resolveTsMorph`) finds the same install.

/// Plugin that reports whether `ctx.types` was available and, if so,
/// whether the type at a fixed byte span includes `null`.
const TYPE_AWARE_PLUGIN: &str = r#"
export default {
  id: "Test.TypeAware",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "CD-81 integration test plugin",
  requiresTypes: true,
  options: {},
  async run(file, ctx) {
    if (!ctx.types) {
      ctx.report({ message: "ctx.types is undefined", span: { start_byte: 0, end_byte: 1 } });
      return;
    }
    // byte 6 is 'x' in "const x: string | null = null;"
    const facts = await ctx.types.typeAt(6, 7);
    ctx.report({
      message: facts && facts.includesNull ? "resolved includesNull=true" : "resolved includesNull=false",
      span: { start_byte: 0, end_byte: 1 },
    });
  },
};
"#;

#[test]
fn requires_types_plugin_resolves_type_via_in_process_ts_morph() {
    if !node_present() {
        return;
    }
    let Ok(ts_morph_root) = std::env::var("COFFERDAM_TYPE_HOST_TS_MORPH_ROOT") else {
        return; // not configured — skip
    };
    let _host = serialize_plugin_host();

    let dir = tempfile::Builder::new()
        .prefix("cofferdam-cd81-")
        .tempdir_in(&ts_morph_root)
        .expect("tempdir_in ts_morph_root");
    let plugin_dir = dir.path().join("plugin");
    std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    std::fs::write(plugin_dir.join("index.mjs"), TYPE_AWARE_PLUGIN).expect("write plugin");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "plugins = [\"./plugin\"]\n",
    )
    .expect("write toml");
    std::fs::write(
        dir.path().join("tsconfig.json"),
        r#"{ "compilerOptions": { "strict": true, "noEmit": true }, "include": ["a.ts"] }"#,
    )
    .expect("write tsconfig");
    std::fs::write(
        dir.path().join("a.ts"),
        "const x: string | null = null;\nexport { x };\n",
    )
    .expect("write a.ts");

    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("resolved includesNull=true"),
        "type-aware plugin check should resolve the nullable type via ts-morph; \
         stdout={stdout}\nstderr={stderr}"
    );
}

// ---- CD-82: ctx.types.resolveLiteral cross-file literal resolution ---

/// Plugin that resolves the `description` identifier — imported from
/// `constants.ts` — to its literal value via `ctx.types.resolveLiteral`.
const RESOLVE_LITERAL_PLUGIN: &str = r#"
export default {
  id: "Test.ResolveLiteral",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "CD-82 integration test plugin",
  requiresTypes: true,
  options: {},
  async run(file, ctx) {
    if (!ctx.types) {
      ctx.report({ message: "ctx.types is undefined", span: { start_byte: 0, end_byte: 1 } });
      return;
    }
    // "import { " is 9 bytes; "description" (11 chars) spans [9, 20).
    const facts = await ctx.types.resolveLiteral(9, 20);
    ctx.report({
      message: facts && facts.literalString !== undefined
        ? "resolved literalString=" + facts.literalString
        : "unresolved",
      span: { start_byte: 0, end_byte: 1 },
    });
  },
};
"#;

#[test]
fn requires_types_plugin_resolves_cross_file_literal_via_in_process_ts_morph() {
    if !node_present() {
        return;
    }
    let Ok(ts_morph_root) = std::env::var("COFFERDAM_TYPE_HOST_TS_MORPH_ROOT") else {
        return; // not configured — skip
    };
    let _host = serialize_plugin_host();

    let dir = tempfile::Builder::new()
        .prefix("cofferdam-cd82-")
        .tempdir_in(&ts_morph_root)
        .expect("tempdir_in ts_morph_root");
    let plugin_dir = dir.path().join("plugin");
    std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    std::fs::write(plugin_dir.join("index.mjs"), RESOLVE_LITERAL_PLUGIN).expect("write plugin");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "plugins = [\"./plugin\"]\n",
    )
    .expect("write toml");
    std::fs::write(
        dir.path().join("tsconfig.json"),
        r#"{ "compilerOptions": { "strict": true, "noEmit": true }, "include": ["constants.ts", "page.ts"] }"#,
    )
    .expect("write tsconfig");
    std::fs::write(
        dir.path().join("constants.ts"),
        "export const description = \"A great page about widgets\";\n",
    )
    .expect("write constants.ts");
    std::fs::write(
        dir.path().join("page.ts"),
        "import { description } from \"./constants\";\nexport { description };\n",
    )
    .expect("write page.ts");

    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("resolved literalString=A great page about widgets"),
        "type-aware plugin check should resolve the cross-file literal via ts-morph; \
         stdout={stdout}\nstderr={stderr}"
    );
}

/// Without a tsconfig in the project, a `requiresTypes: true` plugin
/// check must still run (no crash) with `ctx.types` undefined. Does not
/// require ts-morph on PATH — the no-tsconfig path is short-circuited
/// in the plugin host before ts-morph is ever touched.
#[test]
fn requires_types_plugin_runs_without_ctx_types_when_no_tsconfig() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let dir = tempfile::TempDir::new().expect("temp dir");
    let plugin_dir = dir.path().join("plugin");
    std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    std::fs::write(plugin_dir.join("index.mjs"), TYPE_AWARE_PLUGIN).expect("write plugin");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "plugins = [\"./plugin\"]\n",
    )
    .expect("write toml");
    std::fs::write(dir.path().join("a.ts"), "export const a = 1;\n").expect("write a.ts");

    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("ctx.types is undefined"),
        "with no tsconfig, ctx.types must be undefined and the plugin must still run cleanly; \
         stdout={stdout}\nstderr={stderr}"
    );
}
