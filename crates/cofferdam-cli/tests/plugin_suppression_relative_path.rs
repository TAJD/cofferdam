//! Regression test for CD-140: a `cofferdam-ignore` comment must still
//! suppress a plugin-check finding when discovery yields a relative path
//! (the common case — `cofferdam check .` from the project root).
//!
//! CD-99 made the wire path sent to the plugin host absolute
//! (`forward_slash(&absolutize(path))`), so a plugin's echoed
//! `report.file` is always absolute. 0.3.8 then stamped that absolute
//! path directly onto the resulting `Issue.file`, while the engine's
//! suppression map is keyed by the (possibly relative) discovery path —
//! an absolute path never equals a relative one, so every plugin-check
//! suppression silently stopped working. The fix recovers the original
//! discovery path for `Issue.file` and only uses the absolutized form
//! for the host round-trip / scope lookup.

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

#[test]
fn suppression_comment_still_suppresses_plugin_finding_with_relative_discovery_path() {
    if !node_present() {
        return;
    }
    let dir = tempfile::TempDir::new().expect("temp dir");
    let plugin_dir = dir.path().join("plugin");
    std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
    std::fs::write(
        plugin_dir.join("index.mjs"),
        r#"
export default {
  id: "AlwaysFlags",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "cd-140 regression: flags every file unconditionally",
  requiresTypes: false,
  options: {},
  run(file, ctx) {
    const at = file.text.indexOf("export const a");
    ctx.report({
      message: "always flags",
      span: { line: 2, column: 1, start_byte: at, end_byte: at + 1 },
    });
  },
};
"#,
    )
    .expect("write plugin");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "plugins = [\"./plugin\"]\n",
    )
    .expect("write toml");
    // Suppression comment on line 1 targets the finding the plugin
    // reports at line 2. Discovery is invoked with a relative target
    // (`.`) from `dir`, so the resulting source path is relative —
    // exactly the CD-140 repro condition.
    std::fs::write(
        dir.path().join("a.ts"),
        "// cofferdam-ignore: Warning.AlwaysFlags: intentional\nexport const a = 1;\n",
    )
    .expect("write ts");

    let out = Command::new(cofferdam_bin())
        .args(["check", ".", "--format", "json", "--no-baseline"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stdout.contains("Warning.AlwaysFlags"),
        "cofferdam-ignore comment must suppress plugin-check findings under relative \
         discovery paths; stdout={stdout}\nstderr={stderr}"
    );
}
