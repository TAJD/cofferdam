//! Integration tests for CD-171: the Rust-side scope pre-filter that
//! skips wire construction (reparse + line-views + AST serialization)
//! for files no loaded plugin check's `files` scope will ever match.

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

/// Mirrors `plugin_findings.rs`'s lock — one live plugin host at a time
/// per test binary avoids the starvation flake documented there.
static PLUGIN_HOST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serialize_plugin_host() -> std::sync::MutexGuard<'static, ()> {
    PLUGIN_HOST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

const FOX_PLUGIN: &str = r#"
export default {
  id: "Test.FoxPlugin",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "cd-171 integration test plugin — scoped to src/included/**",
  requiresTypes: false,
  options: {},
  files: { pathPatterns: ["src/included/**"] },
  run(file, ctx) {
    ctx.report({
      message: "FOXSAW " + file.path,
      span: { start_byte: 0, end_byte: 1 },
    });
  },
};
"#;

const OWL_PLUGIN: &str = r#"
export default {
  id: "Test.OwlPlugin",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "cd-171 integration test plugin — no files scope, applies to every file",
  requiresTypes: false,
  options: {},
  run(file, ctx) {
    ctx.report({
      message: "OWLSAW " + file.path,
      span: { start_byte: 0, end_byte: 1 },
    });
  },
};
"#;

/// Same shape as `FOX_PLUGIN` (scoped, so the pre-filter engages) but
/// appends one line to `host-spawns.txt` at module-evaluation time. The
/// host `import()`s each plugin exactly once per process, so the line
/// count is a direct count of plugin-host `node` spawns.
const SPAWN_COUNTING_PLUGIN: &str = r#"
import { appendFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
appendFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "host-spawns.txt"), "spawn\n");

export default {
  id: "Test.SpawnCounter",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "cd-178 integration test plugin — counts plugin-host spawns",
  requiresTypes: false,
  options: {},
  files: { pathPatterns: ["src/included/**"] },
  run(file, ctx) {
    ctx.report({
      message: "COUNTERSAW " + file.path,
      span: { start_byte: 0, end_byte: 1 },
    });
  },
};
"#;

/// CD-183: `files` is a string instead of an object — a plausible author
/// mistake. Malformed on the wire, but must not block the scopes
/// channel: the Rust side tolerates it as `None` (applies to every file)
/// instead of failing `ScopeEntry` deserialization for the whole record.
const MALFORMED_FILES_PLUGIN: &str = r#"
export default {
  id: "Test.MalformedFilesPlugin",
  category: "warning",
  basePriority: 5,
  defaultSeverity: "medium",
  explanation: "cd-183 integration test plugin — files is a string, not an object",
  requiresTypes: false,
  options: {},
  files: "src/**",
  run(file, ctx) {
    ctx.report({
      message: "MALFORMEDSAW " + file.path,
      span: { start_byte: 0, end_byte: 1 },
    });
  },
};
"#;

fn write_sources(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("src/included")).expect("mkdir included");
    std::fs::create_dir_all(dir.join("src/excluded")).expect("mkdir excluded");
    std::fs::write(dir.join("src/included/in.ts"), "export const a = 1;\n").expect("write in.ts");
    std::fs::write(dir.join("src/excluded/out.ts"), "export const b = 2;\n").expect("write out.ts");
}

/// The pre-filter must not change JS-visible behaviour when only a
/// scoped check is loaded: findings still appear only for in-scope
/// files, whether or not the file was ever streamed to the host.
#[test]
fn scoped_plugin_only_reports_matching_files() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let dir = tempfile::TempDir::new().expect("temp dir");
    write_sources(dir.path());
    std::fs::create_dir_all(dir.path().join("plugin-fox")).expect("mkdir plugin");
    std::fs::write(dir.path().join("plugin-fox/index.mjs"), FOX_PLUGIN).expect("write plugin");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "plugins = [\"./plugin-fox\"]\n",
    )
    .expect("write toml");

    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("FOXSAW") && stdout.contains("in.ts"),
        "in-scope file must still be reported; stdout={stdout}\nstderr={stderr}"
    );
    let fox_saw_out = stdout
        .lines()
        .any(|l| l.contains("FOXSAW") && l.contains("out.ts"));
    assert!(
        !fox_saw_out,
        "out-of-scope file must never appear in the scoped plugin's own findings; \
         stdout={stdout}\nstderr={stderr}"
    );
}

/// When ANY loaded plugin check declares no `files` scope, the
/// pre-filter must fall back to streaming every file — including files
/// that a sibling scoped check would reject. This exercises the
/// `Cow::Borrowed` (no-filter) branch of the CD-171 fix.
#[test]
fn unscoped_check_disables_prefilter_for_every_loaded_plugin() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let dir = tempfile::TempDir::new().expect("temp dir");
    write_sources(dir.path());
    std::fs::create_dir_all(dir.path().join("plugin-fox")).expect("mkdir plugin-fox");
    std::fs::write(dir.path().join("plugin-fox/index.mjs"), FOX_PLUGIN).expect("write plugin-fox");
    std::fs::create_dir_all(dir.path().join("plugin-owl")).expect("mkdir plugin-owl");
    std::fs::write(dir.path().join("plugin-owl/index.mjs"), OWL_PLUGIN).expect("write plugin-owl");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "plugins = [\"./plugin-fox\", \"./plugin-owl\"]\n",
    )
    .expect("write toml");

    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // The unscoped check must see BOTH files — proving out.ts was still
    // streamed to the host despite the scoped check's narrow pattern.
    assert!(
        stdout.contains("OWLSAW") && stdout.contains("in.ts"),
        "unscoped plugin must see in.ts; stdout={stdout}\nstderr={stderr}"
    );
    let owl_saw_out = stdout
        .lines()
        .any(|l| l.contains("OWLSAW") && l.contains("out.ts"));
    assert!(
        owl_saw_out,
        "unscoped plugin must still see out.ts even though a sibling check is scoped away \
         from it; stdout={stdout}\nstderr={stderr}"
    );

    // The scoped check must still only report in.ts — its own
    // host-side `files` filter is unaffected by the Rust pre-filter
    // being disabled.
    assert!(
        stdout.contains("FOXSAW") && stdout.contains("in.ts"),
        "scoped plugin must still report in.ts; stdout={stdout}\nstderr={stderr}"
    );
    let fox_saw_out = stdout
        .lines()
        .any(|l| l.contains("FOXSAW") && l.contains("out.ts"));
    assert!(
        !fox_saw_out,
        "scoped plugin must never report out.ts even when the file is streamed to the host; \
         stdout={stdout}"
    );
}

/// CD-178: the scope pre-filter must cost ZERO extra `node` processes.
/// CD-171's first cut learned each check's `files` scope from a second
/// spawn in metadata mode (~215ms on a real repo); the scopes now arrive
/// as one extra round-trip on the single real host spawn. It's easy to
/// "fix" this and still spawn twice, so guard the spawn count directly.
#[test]
fn scope_prefilter_costs_no_extra_host_spawn() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let dir = tempfile::TempDir::new().expect("temp dir");
    write_sources(dir.path());
    std::fs::create_dir_all(dir.path().join("plugin-counter")).expect("mkdir plugin");
    std::fs::write(
        dir.path().join("plugin-counter/index.mjs"),
        SPAWN_COUNTING_PLUGIN,
    )
    .expect("write plugin");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "plugins = [\"./plugin-counter\"]\n",
    )
    .expect("write toml");

    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    // Sanity: the plugin actually ran and the pre-filter actually filtered.
    assert!(
        stdout.contains("COUNTERSAW") && stdout.contains("in.ts"),
        "plugin must have run against the in-scope file; stdout={stdout}\nstderr={stderr}"
    );

    let marker = std::fs::read_to_string(dir.path().join("host-spawns.txt"))
        .expect("plugin must have been loaded at least once");
    let spawns = marker.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        spawns, 1,
        "the scope pre-filter must reuse the single real host spawn, not add a \
         metadata-mode one; observed {spawns} plugin-host spawn(s)\nstdout={stdout}\nstderr={stderr}"
    );
}

/// CD-183: a plugin declaring `files` as a malformed shape (a string
/// instead of an object) must not stall the scopes channel for the full
/// `host_timeout()` (60s default) before falling back to fail-open
/// streaming. Before the fix, `ScopeEntry` deserialization failed for
/// the whole record, the scopes channel never fired, and every run
/// against the plugin paid the full timeout. Assert the run completes
/// well under that timeout and findings are still reported (fail-open,
/// nothing dropped).
#[test]
fn malformed_files_scope_shape_fails_open_without_timeout() {
    if !node_present() {
        return;
    }
    let _host = serialize_plugin_host();
    let dir = tempfile::TempDir::new().expect("temp dir");
    write_sources(dir.path());
    std::fs::create_dir_all(dir.path().join("plugin-malformed")).expect("mkdir plugin");
    std::fs::write(
        dir.path().join("plugin-malformed/index.mjs"),
        MALFORMED_FILES_PLUGIN,
    )
    .expect("write plugin");
    std::fs::write(
        dir.path().join("cofferdam.toml"),
        "plugins = [\"./plugin-malformed\"]\n",
    )
    .expect("write toml");

    let start = std::time::Instant::now();
    let out = Command::new(cofferdam_bin())
        .args(["check", "--no-baseline"])
        .current_dir(dir.path())
        .output()
        .expect("spawn cofferdam");
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "malformed files scope must not stall on the host timeout; took {elapsed:?}\n\
         stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.contains("MALFORMEDSAW") && stdout.contains("in.ts"),
        "plugin must still report findings (fail-open, not dropped); \
         stdout={stdout}\nstderr={stderr}"
    );
}
