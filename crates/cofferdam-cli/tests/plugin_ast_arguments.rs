//! Regression test for cd-18 (github#51): a `CallExpression` whose first
//! argument is a template literal must not report `arguments[0]` as a
//! truncated nested `IdentifierReference` from inside a `${...}`
//! substitution.
//!
//! End-to-end through the real Node plugin host (`plugin-host.mjs`), using
//! the exact repro plugin/source from the bug report — proves the fix
//! flows through the full wire-serialize -> host-deserialize pipeline, not
//! just the Rust-side `WireBuilder` in isolation (see the unit tests in
//! `src/ast_wire.rs` for that).

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

/// Mirrors the bug report's repro plugin: for every `CallExpression` whose
/// callee is `<x>.prepare`, report the first argument's `kind` and a slice
/// of its own span.
const ARG0_INSPECTOR_PLUGIN: &str = r#"
export default {
  id: "Test.Arg0Inspector",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "cd-18 regression: inspect CallExpression.arguments[0]",
  requiresTypes: false,
  options: {},
  run(file, ctx) {
    if (!file.ast) return;
    for (const call of file.ast.findAll("CallExpression")) {
      const c = call.callee;
      if (!c || c.kind !== "MemberExpression" || c.property !== "prepare") continue;
      const a0 = call.arguments[0];
      const slice = a0 ? file.text.slice(a0.span.start_byte, a0.span.end_byte) : "<none>";
      ctx.report({
        message: `arg0kind=${a0 ? a0.kind : "undefined"} slice=<${slice}>`,
        span: call.span,
      });
    }
  },
};
"#;

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new(fixture_source: &str) -> Self {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let plugin_dir = dir.path().join("plugin");
        std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
        std::fs::write(plugin_dir.join("index.mjs"), ARG0_INSPECTOR_PLUGIN).expect("write plugin");
        std::fs::write(
            dir.path().join("cofferdam.toml"),
            "plugins = [\"./plugin\"]\n",
        )
        .expect("write toml");
        std::fs::write(dir.path().join("a.ts"), fixture_source).expect("write ts");
        Env { dir }
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(cofferdam_bin())
            .args(args)
            .current_dir(self.dir.path())
            .output()
            .expect("spawn cofferdam")
    }
}

#[test]
fn template_literal_argument_reports_correctly() {
    if !node_present() {
        return;
    }
    let env = Env::new("await db.prepare(`SELECT * FROM users WHERE id = ${userId}`).run();\n");
    let out = env.run(&["check", "--no-baseline"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stdout.contains("arg0kind=IdentifierReference"),
        "template-literal argument must not be misattributed as a nested \
         IdentifierReference; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        !stdout.contains("slice=<erId}"),
        "argument span must not be a truncated slice into the interpolation; \
         stdout={stdout}\nstderr={stderr}"
    );
}

#[test]
fn non_template_arguments_still_report_correctly() {
    if !node_present() {
        return;
    }
    let env = Env::new("db.prepare(sql).run();\n");
    let out = env.run(&["check", "--no-baseline"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stdout.contains("arg0kind=IdentifierReference slice=<sql>"),
        "identifier argument must still report its own correct kind/span; \
         stdout={stdout}\nstderr={stderr}"
    );
}
