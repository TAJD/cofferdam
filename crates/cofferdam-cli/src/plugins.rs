//! Plugin host driver — invokes Node-side plugins declared in
//! `cofferdam.toml`'s `plugins = [...]` array (cd-7e4 / cd-81a.7).
//!
//! Architecture: Rust CLI builds a JSON manifest carrying `(path, text,
//! lineViews)` per file plus the plugin paths, spawns `node
//! cofferdam-plugin-host.mjs` as a subprocess, pipes the manifest in
//! over stdin, parses the host's JSON response on stdout, converts
//! reports into engine `Issue`s, and returns them for merging into the
//! built-in finding set.
//!
//! The host script is embedded at compile time via `include_str!`. On
//! first call we materialise it into the OS temp dir and reuse the
//! same path for the rest of the process.
//!
//! Failure modes:
//!   - Node not installed: returns a single `Warning.PluginRuntimeUnavailable`
//!     issue and continues (fail-soft — existing built-in findings still
//!     ship).
//!   - Plugin module fails to load: surfaces as `Warning.PluginLoadFailed`
//!     with the plugin path in the message.
//!   - Plugin's run() throws on a file: surfaces as
//!     `Warning.PluginCrashed` for that file; other files continue.
//!   - Host script malformed JSON or non-zero exit: returns
//!     `Warning.PluginHostFailed` with the captured stderr.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cofferdam_core::layers::{self, LayerMatcher};
use cofferdam_core::lines::Lines;
use cofferdam_core::{
    parse_into, Allocator, Issue, Priority, RawOptionValue, Severity, SourceFile, Span,
};
use serde::{Deserialize, Serialize};

const HOST_SCRIPT: &str = include_str!("../scripts/plugin-host.mjs");
const HOST_SCRIPT_NAME: &str = "cofferdam-plugin-host.mjs";

/// Wire shape sent to the Node host on stdin. Field names match
/// `scripts/plugin-host.mjs`'s `manifest.*` reads (camelCase on the JS
/// side; Rust uses serde rename to match).
#[derive(Serialize)]
struct PluginManifest<'a> {
    cwd: String,
    plugins: Vec<String>,
    files: Vec<ManifestFile<'a>>,
    options: BTreeMap<String, BTreeMap<String, ManifestOptionValue>>,
}

#[derive(Serialize)]
struct ManifestFile<'a> {
    path: String,
    text: &'a str,
    #[serde(rename = "lineViews")]
    line_views: Vec<ManifestLineView>,
    /// Layer name from `cofferdam.invariants.toml` `[layers]`. `None`
    /// (serialised as JSON `null`) when no layer config is present or the
    /// file is not a member of any declared layer.
    layer: Option<String>,
    /// Flat-array AST per `design/sdk-ast-wire.md` (cd-svf). `None` when
    /// the file failed to parse — host treats that as `ast: null` and
    /// the engine has already emitted `Warning.ParseError` for the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    ast: Option<crate::ast_wire::AstWire>,
}

#[derive(Serialize)]
struct ManifestLineView {
    #[serde(rename = "lineNo")]
    line_no: u32,
    text: String,
    #[serde(rename = "isComment")]
    is_comment: bool,
    #[serde(rename = "isDocComment")]
    is_doc_comment: bool,
    #[serde(rename = "isStringLiteral")]
    is_string_literal: bool,
    #[serde(rename = "isJsxText")]
    is_jsx_text: bool,
    #[serde(rename = "isPragma")]
    is_pragma: bool,
    #[serde(rename = "lineStart")]
    line_start: u32,
}

/// JSON-friendly projection of `RawOptionValue` for the plugin host.
/// Mirrors the SDK's `OptionKind` accepted runtime values.
#[derive(Serialize)]
#[serde(untagged)]
enum ManifestOptionValue {
    Bool(bool),
    Int(i64),
    String(String),
    List(Vec<ManifestOptionValue>),
}

impl From<&RawOptionValue> for ManifestOptionValue {
    fn from(v: &RawOptionValue) -> Self {
        match v {
            RawOptionValue::Bool(b) => Self::Bool(*b),
            RawOptionValue::Int(i) => Self::Int(*i),
            RawOptionValue::String(s) => Self::String(s.clone()),
            RawOptionValue::List(items) => {
                Self::List(items.iter().map(ManifestOptionValue::from).collect())
            }
        }
    }
}

#[derive(Deserialize)]
struct HostResponse {
    #[serde(default)]
    reports: Vec<HostReport>,
    #[serde(default)]
    errors: Vec<HostError>,
}

#[derive(Deserialize)]
struct HostReport {
    #[serde(rename = "checkId")]
    check_id: String,
    /// Category supplied by the plugin's `defineCheck({ category })`.
    /// Used to prefix the displayed check_id (`Warning.BrandCasing`)
    /// when the plugin's id is bare — keeps formatter output
    /// consistent with built-ins which all use the dotted form.
    #[serde(default)]
    category: String,
    message: String,
    file: String,
    #[serde(rename = "startByte")]
    start_byte: u32,
    #[serde(rename = "endByte")]
    end_byte: u32,
    #[serde(default)]
    severity: String,
    /// Secondary locations participating in the same finding. Used by
    /// cross-file plugin checks emitted from `finalize` (cd-9hp.6).
    /// Empty for per-file findings.
    #[serde(default)]
    related: Vec<HostRelated>,
}

#[derive(Deserialize)]
struct HostRelated {
    file: String,
    span: HostRelatedSpan,
}

#[derive(Deserialize)]
struct HostRelatedSpan {
    #[serde(default)]
    line: u32,
    #[serde(default)]
    column: u32,
    #[serde(default)]
    start_byte: u32,
    #[serde(default)]
    end_byte: u32,
}

#[derive(Deserialize)]
struct HostError {
    kind: String,
    plugin: String,
    #[serde(default)]
    file: String,
    message: String,
}

/// File-scope filter from a plugin's `defineCheck({ files: ... })`.
/// Fields mirror the SDK's `FileScope` interface; `None` means
/// "any file". Used by `advise` to decide whether a plugin check
/// applies to a given path.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginFileScope {
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Layer allowlist — when non-empty, only files whose resolved
    /// layer is in this set match. Files outside every declared layer
    /// (`layer == None`) never match a non-empty `layers` filter.
    #[serde(default)]
    pub layers: Vec<String>,
    #[serde(default)]
    pub path_pattern: Option<String>,
    #[serde(default)]
    pub path_patterns: Vec<String>,
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

/// One option entry returned by the plugin host's metadata mode.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginOptionMeta {
    pub name: String,
    pub kind: String,
    pub default: serde_json::Value,
    pub doc: String,
}

/// Lightweight metadata about a plugin check — returned by
/// `query_plugin_metadata` for the advise and explain subcommands.
#[derive(Debug, Clone)]
pub struct PluginCheckMeta {
    pub id: String,
    pub category: String,
    pub base_priority: i64,
    pub explanation: String,
    pub default_severity: String,
    /// Long-form markdown body (from `defineCheck({ body: "..." })`).
    /// `None` when the plugin didn't ship a body.
    pub body: Option<String>,
    pub requires_types: bool,
    pub options: Vec<PluginOptionMeta>,
    /// `None` means "applies to every file".
    pub files: Option<PluginFileScope>,
}

#[derive(Deserialize)]
struct MetadataHostResponse {
    #[serde(default)]
    checks: Vec<MetadataCheckEntry>,
    #[serde(default)]
    errors: Vec<HostError>,
}

#[derive(Deserialize)]
struct MetadataCheckEntry {
    id: String,
    #[serde(default)]
    category: String,
    #[serde(rename = "basePriority", default)]
    base_priority: i64,
    #[serde(default)]
    explanation: String,
    #[serde(rename = "defaultSeverity", default)]
    default_severity: String,
    /// `None` when the check did not ship a body.
    #[serde(default)]
    body: Option<String>,
    #[serde(rename = "requiresTypes", default)]
    requires_types: bool,
    #[serde(default)]
    options: Vec<PluginOptionMeta>,
    /// `None` when the check has no file-scope filter.
    files: Option<PluginFileScope>,
}

#[derive(Serialize)]
struct MetadataManifest<'a> {
    mode: &'static str,
    cwd: String,
    plugins: &'a [String],
}

/// Run the plugin host in metadata-only mode. Returns one
/// `PluginCheckMeta` per successfully loaded plugin. Load errors are
/// silently dropped (they'd be surfaced again in `run_plugins` anyway).
pub fn query_plugin_metadata(
    plugin_paths: &[PathBuf],
    project_root: &Path,
) -> Vec<PluginCheckMeta> {
    if plugin_paths.is_empty() {
        return Vec::new();
    }

    let host_script = match materialise_host_script() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    let plugin_paths_str: Vec<String> = plugin_paths
        .iter()
        .map(|p| {
            let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            let s = abs.to_string_lossy().replace('\\', "/");
            s.trim_start_matches("//?/").to_string()
        })
        .collect();

    let manifest = MetadataManifest {
        mode: "metadata",
        cwd: forward_slash(project_root),
        plugins: &plugin_paths_str,
    };
    let manifest_json = match serde_json::to_string(&manifest) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut child = match Command::new("node")
        .arg(&host_script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(manifest_json.as_bytes()).is_err() {
            return Vec::new();
        }
    }

    let timeout = host_timeout();
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Vec::new();
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return Vec::new(),
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    if !output.status.success() {
        return Vec::new();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let response: MetadataHostResponse = match serde_json::from_str(&stdout) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    // Errors from load_failed in metadata mode are silently ignored —
    // they'll be surfaced again when `run_plugins` is called.
    let _ = response.errors;

    response
        .checks
        .into_iter()
        .map(|e| PluginCheckMeta {
            id: e.id,
            category: e.category,
            base_priority: e.base_priority,
            explanation: e.explanation,
            default_severity: e.default_severity,
            body: e.body,
            requires_types: e.requires_types,
            options: e.options,
            files: e.files,
        })
        .collect()
}

/// Run every plugin in `plugins` over every file in `files` via the
/// Node-side host. Returns engine-shape `Issue`s, including synthetic
/// `Warning.Plugin*` findings for any host-side failure.
///
/// `layers_cfg` is the project's layer configuration (from
/// `cofferdam.toml` `[layers]` or `cofferdam.invariants.toml` `[layers]`).
/// When `Some`, each file's layer membership is computed once and injected
/// into the JSON payload as the `layer` field; plugins read it from
/// `file.layer`. `None` (no config) serialises to JSON `null`.
pub fn run_plugins(
    plugin_paths: &[PathBuf],
    files: &[PathBuf],
    project_root: &Path,
    check_options: &BTreeMap<String, BTreeMap<String, RawOptionValue>>,
    layers_cfg: Option<&cofferdam_core::graph::LayersConfig>,
) -> Vec<Issue> {
    // Disk-read entry: hydrate `(path, text)` from the working tree,
    // then delegate to the source-driven entry point. Files that fail
    // to read are silently dropped; the engine emits its own Warning
    // for that case via the parallel pipeline.
    let mut sources = Vec::with_capacity(files.len());
    for path in files {
        if let Ok(text) = std::fs::read_to_string(path) {
            sources.push((path.clone(), text));
        }
    }
    run_plugins_with_sources(
        plugin_paths,
        &sources,
        project_root,
        check_options,
        layers_cfg,
    )
}

/// Source-driven counterpart to `run_plugins` — accepts pre-loaded
/// `(path, text)` pairs instead of reading from disk. Required for
/// `cofferdam advise --diff <ref>`, which materialises pre-diff source
/// from `git show <ref>:<path>` and must run plugins against it (not
/// the working tree) for the would_clear half of the diff to be
/// honest.
///
/// All other behaviour is identical to `run_plugins`. The scoped file
/// set, layer membership, options bag, and host timeout all flow
/// through unchanged.
pub fn run_plugins_with_sources(
    plugin_paths: &[PathBuf],
    sources: &[(PathBuf, String)],
    project_root: &Path,
    check_options: &BTreeMap<String, BTreeMap<String, RawOptionValue>>,
    layers_cfg: Option<&cofferdam_core::graph::LayersConfig>,
) -> Vec<Issue> {
    if plugin_paths.is_empty() {
        return Vec::new();
    }

    let host_script = match materialise_host_script() {
        Ok(p) => p,
        Err(e) => return vec![host_unavailable_issue(&format!("host script: {e}"))],
    };

    // Build layer matchers once — shared across all files. `None` when no
    // layer config is present; layer field serialises to JSON `null`.
    let layer_matchers: Vec<LayerMatcher> =
        layers_cfg.map(layers::build_matchers).unwrap_or_default();

    let mut manifest_files = Vec::with_capacity(sources.len());
    for (path, text) in sources {
        let file = SourceFile::new(path.clone(), text.clone());
        let allocator = Allocator::default();
        let parsed = parse_into(&allocator, &file);
        let line_views: Vec<ManifestLineView> = Lines::build(text, &parsed.program)
            .map(|lv| ManifestLineView {
                line_no: lv.line_no,
                text: lv.text.to_string(),
                is_comment: lv.is_comment,
                is_doc_comment: lv.is_doc_comment,
                is_string_literal: lv.is_string_literal,
                is_jsx_text: lv.is_jsx_text,
                is_pragma: lv.is_pragma,
                line_start: lv.line_start,
            })
            .collect();
        // Resolve layer membership for this file. `None` → JSON `null`.
        let layer: Option<String> =
            layers_cfg.and_then(|cfg| layers::layer_for(&layer_matchers, &cfg.project_root, path));
        // Build the flat-array AST wire (cd-svf). One Visit pass per file;
        // re-uses the parse already done for line views.
        let ast = crate::ast_wire::WireBuilder::new(text).build(&parsed.program);
        manifest_files.push(ManifestFile {
            path: forward_slash(path),
            text,
            line_views,
            layer,
            ast: Some(ast),
        });
    }

    // Canonicalise to absolute paths before handing to the host. The
    // host's `resolvePath(cwd, plugin)` is a no-op when the second arg
    // is absolute, so we avoid double-joining when ProjectConfig has
    // already prefixed the plugin path with the config-file's directory.
    let plugin_paths_str: Vec<String> = plugin_paths
        .iter()
        .map(|p| {
            let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            // Strip Windows verbatim prefix (`\\?\`) for cleaner JSON.
            let s = abs.to_string_lossy().replace('\\', "/");
            s.trim_start_matches("//?/").to_string()
        })
        .collect();

    let mut options_payload: BTreeMap<String, BTreeMap<String, ManifestOptionValue>> =
        BTreeMap::new();
    for (check_id, opts) in check_options {
        let mut converted = BTreeMap::new();
        for (k, v) in opts {
            converted.insert(k.clone(), ManifestOptionValue::from(v));
        }
        options_payload.insert(check_id.clone(), converted);
    }

    let manifest = PluginManifest {
        cwd: forward_slash(project_root),
        plugins: plugin_paths_str,
        files: manifest_files,
        options: options_payload,
    };
    let manifest_json = match serde_json::to_string(&manifest) {
        Ok(s) => s,
        Err(e) => return vec![host_failed_issue(&format!("manifest serialize: {e}"))],
    };

    let mut child = match Command::new("node")
        .arg(&host_script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return vec![host_unavailable_issue(&format!("spawn node: {e}"))],
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(manifest_json.as_bytes()) {
            return vec![host_failed_issue(&format!("write manifest: {e}"))];
        }
        // Drop closes stdin → host script's `readFileSync(0, 'utf8')` returns.
    }

    // cd-81a.7 acceptance criterion #3: enforce a wall-clock timeout on
    // plugin host execution. A genuinely-stuck plugin (infinite loop in
    // run(), runaway regex, deadlock in user code) would otherwise hang
    // cofferdam indefinitely. Default 60s — generous for cold Node
    // startup + dynamic-import cache miss + processing thousands of
    // files; tightened via COFFERDAM_PLUGIN_HOST_TIMEOUT_SECS.
    //
    // Per-plugin / per-file timeouts (the bead's original aspiration)
    // require worker_threads inside the host so a stuck synchronous
    // run() can be terminated without taking down its siblings.
    // Subprocess containment can't preempt synchronous JS; the process
    // boundary is the kill grain, so the timeout applies to the whole
    // host call rather than each plugin × file pair. Recorded in the
    // bead notes as a deferred refinement.
    let timeout = host_timeout();
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return vec![host_failed_issue(&format!(
                        "plugin host exceeded {}s timeout (set COFFERDAM_PLUGIN_HOST_TIMEOUT_SECS to change)",
                        timeout.as_secs()
                    ))];
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return vec![host_failed_issue(&format!("try_wait: {e}"))],
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return vec![host_failed_issue(&format!("wait: {e}"))],
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return vec![host_failed_issue(&format!(
            "host exited with status {:?}: {}",
            output.status.code(),
            stderr.trim()
        ))];
    }

    if std::env::var_os("COFFERDAM_PLUGIN_HOST_DEBUG").is_some() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let response: HostResponse = match serde_json::from_str(&stdout) {
        Ok(r) => r,
        Err(e) => {
            return vec![host_failed_issue(&format!(
                "parse host response: {e}; stdout was: {}",
                stdout.chars().take(200).collect::<String>()
            ))];
        }
    };

    let mut issues = Vec::with_capacity(response.reports.len() + response.errors.len());
    let text_index: BTreeMap<&Path, &str> = sources
        .iter()
        .map(|(p, t)| (p.as_path(), t.as_str()))
        .collect();

    for r in response.reports {
        let file = PathBuf::from(&r.file);
        let span = match text_index.get(file.as_path()) {
            Some(text) => cofferdam_core::span_from_bytes(text, r.start_byte, r.end_byte),
            None => Span {
                line: 1,
                column: 1,
                start_byte: r.start_byte,
                end_byte: r.end_byte,
            },
        };
        let severity = parse_severity(&r.severity).unwrap_or(Severity::Medium);
        // Prefix bare plugin IDs with their declared category so the
        // formatter's `category_of(check_id)` derives the right bucket.
        // Built-in IDs already follow `Category.Name`; plugins commonly
        // supply just `Name`. Idempotent: ids that already contain `.`
        // are passed through unchanged.
        let check_id = if r.check_id.contains('.') {
            r.check_id
        } else {
            let prefix = capitalize_category(&r.category).unwrap_or("Warning");
            format!("{}.{}", prefix, r.check_id)
        };
        let related = r
            .related
            .into_iter()
            .map(|rel| {
                let rel_file = PathBuf::from(&rel.file);
                let rel_span = match text_index.get(rel_file.as_path()) {
                    Some(text) => cofferdam_core::span_from_bytes(
                        text,
                        rel.span.start_byte,
                        rel.span.end_byte,
                    ),
                    None => Span {
                        line: if rel.span.line == 0 { 1 } else { rel.span.line },
                        column: if rel.span.column == 0 {
                            1
                        } else {
                            rel.span.column
                        },
                        start_byte: rel.span.start_byte,
                        end_byte: rel.span.end_byte,
                    },
                };
                cofferdam_core::RelatedSpan {
                    file: rel_file,
                    span: rel_span,
                }
            })
            .collect();
        issues.push(Issue {
            check_id,
            priority: Priority(15),
            severity,
            file,
            span,
            message: r.message,
            related,
        });
    }

    for err in response.errors {
        let (check_id, message) = match err.kind.as_str() {
            "load_failed" => (
                "Warning.PluginLoadFailed",
                format!("plugin '{}' failed to load: {}", err.plugin, err.message),
            ),
            "run_threw" => (
                "Warning.PluginCrashed",
                format!(
                    "plugin '{}' threw on file '{}': {}",
                    err.plugin, err.file, err.message
                ),
            ),
            "finalize_threw" => (
                "Warning.PluginCrashed",
                format!(
                    "plugin '{}' threw in finalize(): {}",
                    err.plugin, err.message
                ),
            ),
            other => (
                "Warning.PluginHostFailed",
                format!("plugin host error ({}): {}", other, err.message),
            ),
        };
        let file = if err.file.is_empty() {
            PathBuf::from(&err.plugin)
        } else {
            PathBuf::from(&err.file)
        };
        issues.push(synthetic_warning(check_id, &file, message));
    }

    issues
}

/// Write the embedded host script to the OS temp dir on first call,
/// reuse the same path on subsequent calls. Multiple cofferdam
/// invocations share the file (deterministic name); the content is
/// version-stamped via the build's compile-time string so a stale copy
/// from a previous build gets overwritten on the next first-call.
fn materialise_host_script() -> std::io::Result<PathBuf> {
    static CACHED: OnceLock<std::io::Result<PathBuf>> = OnceLock::new();
    let result = CACHED.get_or_init(|| {
        let path = std::env::temp_dir().join(HOST_SCRIPT_NAME);
        // Always overwrite — the embedded script changes when the CLI
        // is rebuilt. Compared to file-content-hash-named caching, this
        // is one extra write per process. Fine for a multi-second run.
        std::fs::write(&path, HOST_SCRIPT)?;
        Ok(path)
    });
    match result {
        Ok(p) => Ok(p.clone()),
        Err(e) => Err(std::io::Error::new(e.kind(), e.to_string())),
    }
}

fn capitalize_category(wire: &str) -> Option<&'static str> {
    match wire {
        "consistency" => Some("Consistency"),
        "design" => Some("Design"),
        "readability" => Some("Readability"),
        "refactor" => Some("Refactor"),
        "warning" => Some("Warning"),
        _ => None,
    }
}

fn parse_severity(s: &str) -> Option<Severity> {
    match s {
        "info" => Some(Severity::Info),
        "low" => Some(Severity::Low),
        "medium" => Some(Severity::Medium),
        "high" => Some(Severity::High),
        "critical" => Some(Severity::Critical),
        _ => None,
    }
}

fn forward_slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Wall-clock budget for one invocation of the plugin host. Default 60s;
/// override via `COFFERDAM_PLUGIN_HOST_TIMEOUT_SECS=<n>`. Invalid values
/// (non-numeric, zero) fall back to the default — bad config never
/// produces a no-timeout state.
fn host_timeout() -> Duration {
    const DEFAULT: u64 = 60;
    let secs = std::env::var("COFFERDAM_PLUGIN_HOST_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT);
    Duration::from_secs(secs)
}

fn host_unavailable_issue(detail: &str) -> Issue {
    synthetic_warning(
        "Warning.PluginRuntimeUnavailable",
        Path::new(""),
        format!(
            "node plugin host unavailable — plugins not run ({}). Install Node.js or remove `plugins = [...]` from cofferdam.toml.",
            detail
        ),
    )
}

fn host_failed_issue(detail: &str) -> Issue {
    synthetic_warning(
        "Warning.PluginHostFailed",
        Path::new(""),
        format!("plugin host failed: {detail}"),
    )
}

fn synthetic_warning(check_id: &'static str, file: &Path, message: String) -> Issue {
    Issue {
        check_id: check_id.to_string(),
        priority: Priority(20),
        severity: Severity::High,
        file: file.to_path_buf(),
        span: Span {
            line: 1,
            column: 1,
            start_byte: 0,
            end_byte: 0,
        },
        message,
        related: Vec::new(),
    }
}
