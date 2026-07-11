//! Regression test for cd-91: the plugin host must `await` `finalize()`,
//! not call it bare. An `async finalize` that rejects must surface as
//! `Warning.PluginCrashed` (finalize_threw) the same way a synchronous
//! throw does, and a `ctx.report()` call after an `await` inside finalize
//! must actually reach the host's output instead of racing past it.

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

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new(plugin_source: &str) -> Self {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let plugin_dir = dir.path().join("plugin");
        std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
        std::fs::write(plugin_dir.join("index.mjs"), plugin_source).expect("write plugin");
        std::fs::write(
            dir.path().join("cofferdam.toml"),
            "plugins = [\"./plugin\"]\n",
        )
        .expect("write toml");
        std::fs::write(dir.path().join("a.ts"), "export const a = 1;\n").expect("write ts");
        Env { dir }
    }

    fn run(&self) -> std::process::Output {
        Command::new(cofferdam_bin())
            .args(["check", "--format", "json", "--no-baseline"])
            .current_dir(self.dir.path())
            .output()
            .expect("spawn cofferdam")
    }
}

#[test]
fn async_finalize_rejection_surfaces_as_plugin_crashed() {
    if !node_present() {
        return;
    }
    let env = Env::new(
        r#"
export default {
  id: "AsyncFinalizeThrows",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "cd-91 regression: async finalize rejection must surface",
  requiresTypes: false,
  options: {},
  run() {},
  async finalize(ctx) {
    await new Promise((resolve) => setTimeout(resolve, 10));
    throw new Error("async finalize boom");
  },
};
"#,
    );
    let out = env.run();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("Warning.PluginCrashed") && stdout.contains("async finalize boom"),
        "an async finalize rejection must surface as Warning.PluginCrashed; \
         stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn async_finalize_report_after_await_is_not_lost() {
    if !node_present() {
        return;
    }
    let env = Env::new(
        r#"
// `file.path` (captured per-file in `run()`) is the wire's already-
// absolutized path (CD-99) — the same value real cross-file plugins
// (e.g. examples-plugins/seo's SeoDuplicateTitle) stash for later use in
// `finalize`, since the scoped-file lookup on the Rust side keys on
// that absolutized form.
let seenPath;
export default {
  id: "AsyncFinalizeReports",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "cd-91 regression: a report() call after an await inside finalize must not race the host's done record",
  requiresTypes: false,
  options: {},
  run(file) {
    seenPath = file.path;
  },
  async finalize(ctx) {
    await new Promise((resolve) => setTimeout(resolve, 10));
    ctx.report({
      message: "late report",
      file: seenPath,
      span: { line: 1, column: 1, start_byte: 0, end_byte: 0 },
    });
  },
};
"#,
    );
    let out = env.run();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("late report") && stdout.contains("Warning.AsyncFinalizeReports"),
        "a report() call after an await inside async finalize must reach output; \
         stdout={stdout}\nstderr={stderr}"
    );
}
