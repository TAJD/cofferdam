//! Plugin host driver — invokes Node-side plugins declared in
//! `cofferdam.toml`'s `plugins = [...]` array (cd-7e4 / cd-81a.7).
//!
//! Architecture (CD-33: streamed, wireVersion 2): the Rust CLI spawns
//! `node cofferdam-plugin-host.mjs` and exchanges NDJSON records over
//! stdin/stdout instead of one whole-repo JSON blob. Stdin carries a
//! `header` record (plugins + options), one `file` record per source
//! file, then an `end` record. Stdout carries `report` / `error` records
//! streamed as each file is processed, then a final `done` record. This
//! keeps peak memory on both sides at O(one file) instead of O(repo) —
//! see `design/sdk-ast-wire.md` for the wire spec. Writing (this
//! process) and reading (a background thread) run concurrently so a
//! large volume of streamed findings can't back up the stdout pipe while
//! we're still writing input (the classic bidirectional-pipe deadlock).
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
//!   - Host exits successfully but the stdout stream ends without ever
//!     emitting the closing `done` record (stdout truncated under load,
//!     or the process terminated mid-finalize): treated as
//!     `Warning.PluginHostFailed`, NOT as an empty finding set (cd-41).
//!     `done` is the completion handshake — its absence is the only
//!     reliable signal that the host didn't actually finish.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use cofferdam_core::layers::{self, LayerMatcher};
use cofferdam_core::lines::Lines;
use cofferdam_core::{
    parse_into, Allocator, Issue, Language, Location, Priority, RawOptionValue, Severity,
    SourceFile, Span,
};
use serde::{Deserialize, Serialize};

const HOST_SCRIPT: &str = include_str!("../scripts/plugin-host.mjs");
const HOST_SCRIPT_NAME: &str = concat!("cofferdam-plugin-host-", env!("CARGO_PKG_VERSION"), ".mjs");

/// CD-81/CD-89: `plugin-host.mjs` imports `./type-host-core.mjs` (shared
/// ts-morph plumbing with `type_host.rs`'s worker) as a relative ESM
/// specifier, so the materialised copy must sit next to it under this
/// exact name. The pair lives inside a version-scoped subdirectory (see
/// `materialise_host_script` / `scripts_dir`) rather than flat in the OS
/// temp dir: two different cofferdam versions running concurrently on
/// the same machine (e.g. two projects each on their own npm-installed
/// version) previously raced to overwrite this one shared, unversioned
/// path, so a host process could import a core module from a different
/// build than the one that spawned it.
const CORE_SCRIPT: &str = include_str!("../scripts/type-host-core.mjs");
const CORE_SCRIPT_NAME: &str = "type-host-core.mjs";

/// Version- and user-scoped subdirectory of the OS temp dir that holds
/// this build's materialised host + core scripts (CD-89). Shared by
/// `plugins.rs` and `type_host.rs`, which each ensure it exists
/// independently. The user component matters on a shared multi-user
/// Linux box: the OS temp dir is commonly world-writable, so without it
/// the first user to run on a given version would own the directory and
/// every other user's `fs::write` into it would fail with `EACCES`.
fn scripts_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "cofferdam-scripts-{}-{}",
        env!("CARGO_PKG_VERSION"),
        current_user_tag()
    ))
}

/// Best-effort, filesystem-safe tag for the current user, read straight
/// from the platform's conventional env var (no extra dependency for
/// something this approximate). Falls back to a fixed placeholder when
/// unset/empty/unusable so `scripts_dir` always returns a valid path.
fn current_user_tag() -> String {
    let raw = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let sanitized: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// Wire version for the streamed manifest protocol (CD-33). Bump and
/// document in `design/sdk-ast-wire.md` on any breaking change to the
/// record shapes below.
const WIRE_VERSION: u32 = 2;

/// First record on stdin: plugin list + per-check option overrides.
/// Field names match `scripts/plugin-host.mjs`'s reads (camelCase on the
/// JS side; Rust uses serde rename to match).
#[derive(Serialize)]
struct HeaderRecord<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(rename = "wireVersion")]
    wire_version: u32,
    cwd: String,
    plugins: &'a [String],
    options: &'a BTreeMap<String, BTreeMap<String, ManifestOptionValue>>,
    /// CD-81: absolute tsconfig path, forward-slashed, for type-aware
    /// plugin checks (`requiresTypes: true`) to resolve types against
    /// in-process via ts-morph. `None` (serialised as JSON `null`) when
    /// no tsconfig was discovered or type-awareness is disabled — the
    /// host treats `null` the same as an absent field.
    #[serde(rename = "tsconfigPath", skip_serializing_if = "Option::is_none")]
    tsconfig_path: Option<String>,
}

/// One streamed file record. Same fields the one-shot manifest used to
/// carry per-file, now serialised and written as its own NDJSON line as
/// soon as the file is parsed — the host processes it immediately
/// instead of waiting for the whole repo to arrive.
#[derive(Serialize)]
struct ManifestFile<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
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

/// Terminal record on stdin: no more files follow. Triggers `finalize()`
/// hooks and the closing `done` record on stdout.
#[derive(Serialize)]
struct EndRecord {
    #[serde(rename = "type")]
    kind: &'static str,
}

/// One record read back from stdout. Internally tagged on `type`;
/// `Report`/`Error` wrap the same shapes the one-shot `HostResponse`
/// used to carry as array elements.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum StreamRecord {
    #[serde(rename = "report")]
    Report(HostReport),
    #[serde(rename = "error")]
    Error(HostError),
    #[serde(rename = "done")]
    Done {
        /// CD-81: set only when at least one loaded plugin check declared
        /// `requiresTypes: true`. `None` (missing key or JSON `null`)
        /// means either no check needed types, or ts-morph resolved
        /// fine; `Some(reason)` means type-aware plugin checks ran with
        /// `ctx.types` undefined because the host couldn't load ts-morph
        /// or find a tsconfig.
        #[serde(rename = "typeHostUnavailable", default)]
        type_host_unavailable: Option<String>,
    },
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
    /// CD-100: span-aware counterpart to `is_string_literal` — byte
    /// ranges `[start, end)` relative to this line's own text, one per
    /// literal that touches the line. Empty when the line has no
    /// literal (or for languages/wire paths that only ever produce the
    /// whole-line flag, e.g. HTML/Astro).
    #[serde(rename = "stringLiteralRanges")]
    string_literal_ranges: Vec<(u32, u32)>,
    #[serde(rename = "isJsxText")]
    is_jsx_text: bool,
    #[serde(rename = "isPragma")]
    is_pragma: bool,
    /// CD-84: HTML-only — `true` when a tag (`start_tag`/`end_tag`/
    /// `self_closing_tag`/`doctype`) overlaps this line. Always `false`
    /// for TS/JS files.
    #[serde(rename = "isTag")]
    is_tag: bool,
    /// CD-84: HTML-only — `true` when an HTML `text` node overlaps this
    /// line. Always `false` for TS/JS files.
    #[serde(rename = "isText")]
    is_text: bool,
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

/// Wire shape for a related span. The host may also send `line`/`column`
/// hints; they are ignored — line/column are always recomputed from the
/// in-scope file's text via `span_from_bytes` (out-of-scope related files
/// are rejected, see cd-neav).
#[derive(Deserialize)]
struct HostRelatedSpan {
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
    /// The `cofferdam.toml` `plugins = [...]` entry (as sent to the host)
    /// this check was loaded from — lets callers correlate a metadata
    /// entry back to the config path that produced it, e.g. to filter
    /// `cfg.plugins` down to only output-mode-eligible entries before
    /// invoking the host again for `verify --dist`.
    pub path: String,
    pub id: String,
    pub category: String,
    pub base_priority: i64,
    pub explanation: String,
    pub default_severity: String,
    /// Long-form markdown body (from `defineCheck({ body: "..." })`).
    /// `None` when the plugin didn't ship a body.
    pub body: Option<String>,
    pub requires_types: bool,
    /// Whether this check is eligible to run against build-output files
    /// discovered by `cofferdam verify --dist` (CD-85). Mirrors
    /// `requires_types`'s plumbing end to end.
    pub output_mode: bool,
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
    #[serde(default)]
    path: String,
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
    #[serde(rename = "outputMode", default)]
    output_mode: bool,
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

/// Canonicalize a plugin path the same way it's sent to the host, so a
/// `PluginCheckMeta::path` (round-tripped verbatim through the host) can
/// be matched back against a `cofferdam.toml` `plugins = [...]` entry.
pub fn canonical_plugin_path_str(p: &Path) -> String {
    let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    s.trim_start_matches("//?/").to_string()
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
        .map(|p| canonical_plugin_path_str(p))
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
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
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
            path: e.path,
            id: e.id,
            category: e.category,
            base_priority: e.base_priority,
            explanation: e.explanation,
            default_severity: e.default_severity,
            body: e.body,
            requires_types: e.requires_types,
            output_mode: e.output_mode,
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
    tsconfig_path: Option<&Path>,
    warn_on_type_unavailable: bool,
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
        tsconfig_path,
        warn_on_type_unavailable,
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
///
/// `warn_on_type_unavailable` mirrors `install_type_oracle_if_needed`'s
/// silent-skip contract for the built-in oracle: callers that don't
/// install a tsconfig on purpose (`baseline write/prune/ratchet`,
/// `advise --diff`) pass `false` so a `requiresTypes: true` plugin check
/// doesn't bake a `Warning.PluginTypeHostUnavailable` finding into
/// `baseline.json` on every run — the built-in oracle produces no
/// Issue at all on those paths (CD-81 review).
pub fn run_plugins_with_sources(
    plugin_paths: &[PathBuf],
    sources: &[(PathBuf, String)],
    project_root: &Path,
    check_options: &BTreeMap<String, BTreeMap<String, RawOptionValue>>,
    layers_cfg: Option<&cofferdam_core::graph::LayersConfig>,
    tsconfig_path: Option<&Path>,
    warn_on_type_unavailable: bool,
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

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return vec![host_failed_issue("no host stdout")],
    };
    let stderr = match child.stderr.take() {
        Some(s) => s,
        None => return vec![host_failed_issue("no host stderr")],
    };
    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => return vec![host_failed_issue("no host stdin")],
    };

    // Read stdout/stderr on background threads so a large volume of
    // streamed findings can't fill the pipe and block Node while this
    // thread is still writing input (see the module doc's deadlock
    // note). Both threads terminate on EOF, which happens once the
    // child exits (normally or via the timeout kill below).
    let reader_handle = thread::spawn(move || read_stream_records(stdout));
    let stderr_handle = thread::spawn(move || {
        let mut buf = String::new();
        let mut reader = BufReader::new(stderr);
        std::io::Read::read_to_string(&mut reader, &mut buf).ok();
        buf
    });

    let header = HeaderRecord {
        kind: "header",
        wire_version: WIRE_VERSION,
        cwd: forward_slash(project_root),
        plugins: &plugin_paths_str,
        options: &options_payload,
        tsconfig_path: tsconfig_path.map(forward_slash),
    };
    let mut write_error = write_record(&mut stdin, &header).err();

    if write_error.is_none() {
        for (path, text) in sources {
            let file = SourceFile::new(path.clone(), text.clone());
            // Per-language dispatch (CD-84): oxc only understands
            // TS/JS/JSX — feeding it an `.html` file would parse
            // garbage. HTML gets its own tree-sitter-html parse +
            // wire builder; any other language (Rust, Astro,
            // unrecognised) gets no whole-file parse at all, mirroring
            // the engine's own per-language dispatch in
            // `cofferdam-engine/src/lib.rs::pass1_one_file` (a
            // separate code path from this one — plugins.rs has its
            // own oxc/tree-sitter calls).
            let (line_views, ast): (Vec<ManifestLineView>, Option<crate::ast_wire::AstWire>) =
                match file.language {
                    // Unlike the engine path (which rejects a tree with
                    // `has_errors()` and emits `Warning.ParseError`),
                    // plugins get a best-effort wire even from a
                    // partially error-recovered parse — plugins have no
                    // channel to emit a parse-error finding themselves,
                    // so a partial tree is more useful than none.
                    Language::Html => match cofferdam_html::parse_html(text) {
                        Ok(tree) => {
                            let line_views = cofferdam_html::html_lines(text, &tree)
                                .into_iter()
                                .map(|lv| ManifestLineView {
                                    line_no: lv.line_no,
                                    text: lv.text.to_string(),
                                    is_comment: lv.is_comment,
                                    is_doc_comment: false,
                                    is_string_literal: false,
                                    string_literal_ranges: Vec::new(),
                                    is_jsx_text: false,
                                    is_pragma: false,
                                    is_tag: lv.is_tag,
                                    is_text: lv.is_text,
                                    line_start: lv.line_start,
                                })
                                .collect();
                            let ast = crate::html_wire::HtmlWireBuilder::new(&tree).build();
                            (line_views, Some(ast))
                        }
                        Err(_) => (Vec::new(), None),
                    },
                    Language::TypeScript => {
                        let allocator = Allocator::default();
                        let parsed = parse_into(&allocator, &file);
                        let line_views = Lines::build(text, &parsed.program)
                            .map(|lv| ManifestLineView {
                                line_no: lv.line_no,
                                text: lv.text.to_string(),
                                is_comment: lv.is_comment,
                                is_doc_comment: lv.is_doc_comment,
                                is_string_literal: lv.is_string_literal,
                                string_literal_ranges: lv.string_literal_ranges.clone(),
                                is_jsx_text: lv.is_jsx_text,
                                is_pragma: lv.is_pragma,
                                is_tag: false,
                                is_text: false,
                                line_start: lv.line_start,
                            })
                            .collect();
                        // Build the flat-array AST wire (cd-svf). One
                        // Visit pass per file; re-uses the parse
                        // already done for line views.
                        let ast = crate::ast_wire::WireBuilder::new(text).build(&parsed.program);
                        (line_views, Some(ast))
                    }
                    // Astro has no oxc/tree-sitter adapter to build an
                    // AST from, but Pattern-A line-scan checks (CD-93)
                    // still need real line text/spans rather than
                    // silently iterating zero lines — `Lines::plain`
                    // gives unclassified (all flags false) line views.
                    Language::Astro => {
                        let line_views = cofferdam_core::Lines::plain(text)
                            .map(|lv| ManifestLineView {
                                line_no: lv.line_no,
                                text: lv.text.to_string(),
                                is_comment: false,
                                is_doc_comment: false,
                                is_string_literal: false,
                                string_literal_ranges: Vec::new(),
                                is_jsx_text: false,
                                is_pragma: false,
                                is_tag: false,
                                is_text: false,
                                line_start: lv.line_start,
                            })
                            .collect();
                        (line_views, None)
                    }
                    // Markdown/MDX (CD-68): same shape as Astro — no
                    // whole-file parse, but Pattern-A line-scan plugin
                    // checks (frontmatter length, heading structure,
                    // link density) still need real line text/spans.
                    Language::Markdown => {
                        let line_views = cofferdam_core::Lines::plain(text)
                            .map(|lv| ManifestLineView {
                                line_no: lv.line_no,
                                text: lv.text.to_string(),
                                is_comment: false,
                                is_doc_comment: false,
                                is_string_literal: false,
                                string_literal_ranges: Vec::new(),
                                is_jsx_text: false,
                                is_pragma: false,
                                is_tag: false,
                                is_text: false,
                                line_start: lv.line_start,
                            })
                            .collect();
                        (line_views, None)
                    }
                    // Rust, unrecognised: no whole-file parse for
                    // plugins today (matches the engine's catch-all —
                    // no built-in check declares Rust for the plugin
                    // surface yet).
                    Language::Rust => (Vec::new(), None),
                };
            // Resolve layer membership for this file. `None` → JSON `null`.
            let layer: Option<String> = layers_cfg
                .and_then(|cfg| layers::layer_for(&layer_matchers, &cfg.project_root, path));
            let record = ManifestFile {
                kind: "file",
                path: forward_slash(&absolutize(path)),
                text,
                line_views,
                layer,
                ast,
            };
            if let Err(e) = write_record(&mut stdin, &record) {
                write_error = Some(e);
                break;
            }
        }
    }

    if write_error.is_none() {
        write_error = write_record(&mut stdin, &EndRecord { kind: "end" }).err();
    }
    // Drop closes stdin → host script's readline sees EOF and finalizes.
    drop(stdin);

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
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };

    let (reports, errors, malformed, saw_done, type_host_unavailable) =
        reader_handle.join().unwrap_or_else(|_| {
            (
                Vec::new(),
                Vec::new(),
                vec!["reader thread panicked".to_string()],
                false,
                None,
            )
        });
    let stderr_text = stderr_handle.join().unwrap_or_default();

    if let Some(e) = write_error {
        return vec![host_failed_issue(&format!("write manifest: {e}"))];
    }
    let Some(status) = exit_status else {
        return vec![host_failed_issue(&format!(
            "plugin host exceeded {}s timeout (set COFFERDAM_PLUGIN_HOST_TIMEOUT_SECS to change)",
            timeout.as_secs()
        ))];
    };
    if !status.success() {
        return vec![host_failed_issue(&format!(
            "host exited with status {:?}: {}",
            status.code(),
            stderr_text.trim()
        ))];
    }
    if !saw_done {
        // cd-41: the child exited successfully but its stdout stream
        // ended without the closing `done` record ever arriving —
        // truncated output or a mid-stream death that still returned
        // exit code 0. Whatever reports/errors were collected up to
        // that point are unreliable (we can't know what else was lost),
        // so surface one clear failure instead of a silent (and
        // possibly empty) result.
        return vec![host_failed_issue(&format!(
            "host exited successfully but never emitted its completion marker \
             (stdout truncated or process terminated mid-run); \
             {} report(s) and {} error(s) seen before the stream ended{}",
            reports.len(),
            errors.len(),
            if stderr_text.trim().is_empty() {
                String::new()
            } else {
                format!("; stderr: {}", stderr_text.trim())
            }
        ))];
    }

    if std::env::var_os("COFFERDAM_PLUGIN_HOST_DEBUG").is_some() {
        eprint!("{stderr_text}");
    }

    let mut issues = Vec::with_capacity(reports.len() + errors.len() + malformed.len());
    for m in malformed {
        issues.push(host_failed_issue(&format!("malformed stream record: {m}")));
    }
    // Keyed by the same `absolutize()` transform used when building each
    // file's wire `path` (CD-99), so a report's echoed `file` — which is
    // that same absolutized, forward-slashed string round-tripped through
    // the host — looks itself up successfully instead of appearing
    // "outside the analyzed file set" whenever the original discovery
    // path was relative. The value carries the ORIGINAL (pre-absolutize)
    // path alongside the text: CD-140 found that stamping issues with the
    // absolutized key instead of this original path broke suppression
    // lookups, which are keyed by the engine's native (possibly relative)
    // discovery paths — an absolute path never equals a relative one, so
    // every plugin-check suppression comment silently stopped working.
    let text_index: BTreeMap<PathBuf, (&Path, &str)> = sources
        .iter()
        .map(|(p, t)| (absolutize(p).into_owned(), (p.as_path(), t.as_str())))
        .collect();

    // Reject reports whose path is outside the scoped source set (cd-neav).
    // Per-file `run` reports get their path stamped by the host, but
    // `finalize` reports carry a plugin-supplied path verbatim — a buggy
    // (or adversarial) plugin could otherwise inject findings for files
    // cofferdam never analyzed, and those paths flow into baseline
    // signatures (which read the file from disk) and formatter output.
    // Dropped locations surface as one aggregated Warning.PluginHostFailed
    // per plugin check id.
    let mut out_of_scope: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut record_out_of_scope = |check_id: &str, path: String| {
        let shown = if path.is_empty() {
            "<missing file>".to_string()
        } else {
            path
        };
        out_of_scope
            .entry(check_id.to_string())
            .or_default()
            .push(shown);
    };

    for r in reports {
        let wire_file = PathBuf::from(&r.file);
        let Some((orig_file, text)) = text_index.get(wire_file.as_path()).copied() else {
            record_out_of_scope(&r.check_id, r.file);
            continue;
        };
        let file = orig_file.to_path_buf();
        let span = cofferdam_core::span_from_bytes(text, r.start_byte, r.end_byte);
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
        // Same scope rule for secondary locations: an out-of-scope related
        // span is dropped (the finding itself survives) and counted in the
        // aggregated warning.
        let mut related = Vec::with_capacity(r.related.len());
        for rel in r.related {
            let rel_wire_file = PathBuf::from(&rel.file);
            let Some((rel_orig_file, rel_text)) = text_index.get(rel_wire_file.as_path()).copied()
            else {
                record_out_of_scope(&check_id, rel.file);
                continue;
            };
            let rel_file = rel_orig_file.to_path_buf();
            let rel_span =
                cofferdam_core::span_from_bytes(rel_text, rel.span.start_byte, rel.span.end_byte);
            let rel_location = Location::from_span(&rel_file, rel_span);
            related.push(cofferdam_core::RelatedSpan {
                location: rel_location,
                file: rel_file,
            });
        }
        let location = Location::from_span(&file, span);
        issues.push(Issue {
            check_id,
            priority: Priority(15),
            severity,
            location,
            file,
            message: r.message,
            related,
        });
    }

    for (check_id, paths) in out_of_scope {
        let total = paths.len();
        let preview: Vec<String> = paths.into_iter().take(3).collect();
        let more = if total > preview.len() {
            format!(" (+{} more)", total - preview.len())
        } else {
            String::new()
        };
        issues.push(synthetic_warning(
            "Warning.PluginHostFailed",
            project_root,
            format!(
                "plugin check '{}' reported {} location(s) outside the analyzed file set; dropped: {}{}",
                check_id,
                total,
                preview.join(", "),
                more
            ),
        ));
    }

    for err in errors {
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
            "zero_scope_match" => ("Warning.PluginZeroScopeMatch", err.message.clone()),
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

    // CD-81: at least one loaded plugin check declared `requiresTypes:
    // true` but the host couldn't resolve types for it (no tsconfig, or
    // ts-morph failed to load). Surfaced as a synthetic finding — same
    // pattern as `host_unavailable_issue`/`host_failed_issue` — rather
    // than a new return shape, so `--fail-on-type-unavailable` gating
    // can inspect the merged issue set the same way it already does for
    // the built-in type oracle. Gated on `warn_on_type_unavailable` so
    // callers that never intended to install types this run (baseline
    // write/prune/ratchet, advise --diff) don't bake this into their
    // snapshot — see the doc comment on `run_plugins_with_sources`.
    if let Some(reason) = type_host_unavailable.filter(|_| warn_on_type_unavailable) {
        issues.push(synthetic_warning(
            "Warning.PluginTypeHostUnavailable",
            project_root,
            format!(
                "type-aware plugin check(s) declared requiresTypes:true but the type host is \
                 unavailable ({reason}) — those checks ran with ctx.types undefined."
            ),
        ));
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
        let dir = scripts_dir();
        std::fs::create_dir_all(&dir)?;

        // Write the shared core module first — plugin-host.mjs's
        // `import "./type-host-core.mjs"` must resolve by the time the
        // host script is spawned. The version-scoped directory (CD-89)
        // keeps this from colliding with another cofferdam build's copy;
        // always overwritten for the same reason the host script is.
        let core_path = dir.join(CORE_SCRIPT_NAME);
        std::fs::write(&core_path, CORE_SCRIPT)?;

        let path = dir.join(HOST_SCRIPT_NAME);
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

pub fn capitalize_category(wire: &str) -> Option<&'static str> {
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

/// `SourceFile.path` is documented (check-sdk's `ast.d.ts`) as "Absolute
/// path as cofferdam discovered it" — but discovery yields whatever form
/// the CLI's target argument took, so a relative target (`cofferdam check
/// app`, the common case) previously produced relative wire paths,
/// silently breaking any plugin author's path-prefix logic that trusted
/// the doc (CD-99). Absolutizing here makes the wire's `path` field match
/// its documented contract; existing glob-based `pathPatterns` are
/// unaffected since `globMatch`'s `**/`-prefix fallback already matches
/// at any path depth, absolute or relative.
fn absolutize(path: &Path) -> std::borrow::Cow<'_, Path> {
    if path.is_absolute() {
        std::borrow::Cow::Borrowed(path)
    } else {
        match std::env::current_dir() {
            Ok(cwd) => std::borrow::Cow::Owned(cwd.join(path)),
            Err(_) => std::borrow::Cow::Borrowed(path),
        }
    }
}

/// Serialise one NDJSON record and write it (plus a trailing newline) to
/// the host's stdin, flushing immediately so the host's `readline` sees
/// the line without waiting for buffered data.
fn write_record<T: Serialize>(stdin: &mut impl Write, record: &T) -> std::io::Result<()> {
    let mut line = serde_json::to_string(record)
        .map_err(|e| std::io::Error::other(format!("serialise record: {e}")))?;
    line.push('\n');
    stdin.write_all(line.as_bytes())?;
    stdin.flush()
}

/// Read NDJSON `report`/`error`/`done` records from the host's stdout
/// until EOF or a `done` record. Runs on a background thread (see
/// `run_plugins_with_sources`) so it can drain the pipe concurrently
/// with this process still writing input. A line that fails to parse is
/// recorded in the third return slot rather than aborting the whole
/// read — one bad record shouldn't discard every finding streamed
/// before or after it.
///
/// The final `bool` reports whether a `done` record was actually
/// observed before the stream ended (cd-41). EOF without `done` means
/// the host's output was truncated or the process died mid-stream —
/// the caller must not treat that the same as a legitimate empty
/// result, even when the child's exit status is success.
fn read_stream_records(
    stdout: ChildStdout,
) -> (
    Vec<HostReport>,
    Vec<HostError>,
    Vec<String>,
    bool,
    Option<String>,
) {
    let mut reader = BufReader::new(stdout);
    let mut reports = Vec::new();
    let mut errors = Vec::new();
    let mut malformed = Vec::new();
    let mut saw_done = false;
    let mut type_host_unavailable = None;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim_end();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<StreamRecord>(trimmed) {
                    Ok(StreamRecord::Report(r)) => reports.push(r),
                    Ok(StreamRecord::Error(e)) => errors.push(e),
                    Ok(StreamRecord::Done {
                        type_host_unavailable: reason,
                    }) => {
                        saw_done = true;
                        type_host_unavailable = reason;
                        break;
                    }
                    Err(e) => malformed.push(format!(
                        "{e}: {}",
                        trimmed.chars().take(200).collect::<String>()
                    )),
                }
            }
            Err(_) => break,
        }
    }
    (reports, errors, malformed, saw_done, type_host_unavailable)
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
    let file_buf = file.to_path_buf();
    Issue {
        check_id: check_id.to_string(),
        priority: Priority(20),
        severity: Severity::High,
        location: Location::from_span(
            &file_buf,
            Span {
                line: 1,
                column: 1,
                start_byte: 0,
                end_byte: 0,
            },
        ),
        file: file_buf,
        message,
        related: Vec::new(),
    }
}
