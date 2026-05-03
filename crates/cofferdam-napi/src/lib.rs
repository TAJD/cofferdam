//! `cofferdam-napi` — Node.js bindings for the cofferdam engine and the
//! plugin host (cd-81a.7).
//!
//! Two surfaces:
//!
//! 1. **`analyze(paths, options) -> AnalyzeResult`** — the headline
//!    function. Drives the engine over the given file list using the
//!    built-in checks, returns `(findings, errors)`. Plugins are not
//!    invoked here; the JS-side loader (cd-81a.7 part 2) is responsible
//!    for spawning worker threads, calling each plugin's `run()` per
//!    file, and merging the resulting findings into the engine output.
//!
//! 2. **`PluginHostFile` + `PluginHostLineView` + `PluginHostAst`** —
//!    plain-data view of a single source file, suitable for sending
//!    across the worker_thread postMessage boundary. The TS loader
//!    builds one per file from the engine's parsed text and hands it to
//!    each plugin's `run(file, ctx, opts)` callback. Spans round-trip
//!    via `start_byte`/`end_byte` so any issue the plugin reports can
//!    be re-anchored on the Rust side without ambiguity.
//!
//! Why split the surface this way: keeping the engine-driver and the
//! plugin-host as separate napi exports means the loader can iterate
//! files in pure JS (where worker_threads live) without re-entering
//! Rust per file, and the Rust side stays ignorant of plugin authors'
//! code — crash containment is the JS loader's job.

#![deny(clippy::all)]

use std::path::PathBuf;

use cofferdam_checks::all_builtins;
use cofferdam_core::{Issue, Span};
use cofferdam_engine::Engine;
use napi::bindgen_prelude::*;
use napi_derive::napi;

// ---- analyze ---------------------------------------------------------

/// Options for `analyze`. Mirrors the CLI's `cofferdam check` flags so
/// the JS wrapper can pass through arguments without re-parsing.
#[napi(object)]
pub struct AnalyzeOptions {
    /// Optional cofferdam.toml path. When provided, severity overrides
    /// and per-check options are sourced from it.
    pub config_path: Option<String>,
}

/// Plain-data version of [`Issue`] for the napi boundary. All fields
/// are owned strings/numbers so the JS side gets a serialisable object
/// without lifetime concerns.
#[napi(object)]
pub struct JsIssue {
    pub check_id: String,
    pub message: String,
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub start_byte: u32,
    pub end_byte: u32,
    pub priority: i32,
    pub severity: String,
}

impl From<&Issue> for JsIssue {
    fn from(issue: &Issue) -> Self {
        Self {
            check_id: issue.check_id.clone(),
            message: issue.message.clone(),
            file: issue.file.to_string_lossy().replace('\\', "/"),
            line: issue.span.line,
            column: issue.span.column,
            start_byte: issue.span.start_byte,
            end_byte: issue.span.end_byte,
            priority: issue.priority.0 as i32,
            severity: issue.severity.as_str().to_string(),
        }
    }
}

/// Run cofferdam's built-in checks over `paths`. Plugins are not
/// invoked here; the JS-side loader merges plugin findings into the
/// returned vector.
///
/// Returns `Vec<JsIssue>` on success. Failure to read a file produces a
/// `napi::Error` so the JS side gets a thrown exception with a clear
/// message — matches the CLI's exit-code-2 path.
#[napi]
pub fn analyze(paths: Vec<String>, options: Option<AnalyzeOptions>) -> Result<Vec<JsIssue>> {
    let path_bufs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();

    let engine = match options.as_ref().and_then(|o| o.config_path.as_ref()) {
        Some(cfg_path) => {
            let cfg_path = PathBuf::from(cfg_path);
            let cfg = cofferdam_engine::config::load(&cfg_path).map_err(napi_err)?;
            Engine::with_config(all_builtins(), &cfg, &cfg_path).map_err(napi_err)?
        }
        None => Engine::new(all_builtins()),
    };

    let issues = engine
        .analyze(&path_bufs)
        .map_err(|e| napi::Error::from_reason(format!("engine error: {e}")))?;

    Ok(issues.iter().map(JsIssue::from).collect())
}

fn napi_err<E: std::fmt::Display>(e: E) -> napi::Error {
    napi::Error::from_reason(e.to_string())
}

// ---- plugin host scaffolding ----------------------------------------
//
// The TS loader (cd-81a.7 part 2) calls these helpers from inside a
// worker_thread to build the SourceFile + LineView projection a plugin's
// `run(file, ctx, opts)` callback expects. Spans round-trip via
// start_byte/end_byte so the JS-side `ctx.report` payload can be
// re-anchored on the Rust side without ambiguity.

/// One classified line — JS-shaped equivalent of `cofferdam_core::LineView`.
#[napi(object)]
pub struct JsLineView {
    pub line_no: u32,
    pub text: String,
    pub is_comment: bool,
    pub is_doc_comment: bool,
    pub is_string_literal: bool,
    pub is_jsx_text: bool,
    pub is_pragma: bool,
    pub line_start: u32,
}

/// Parse `text` as the file at `path` and return its classified line
/// views. Pure projection — no side effects, no engine state. Loader
/// calls this per file before invoking each plugin.
#[napi]
pub fn line_views(path: String, text: String) -> Result<Vec<JsLineView>> {
    use cofferdam_core::lines::Lines;
    use cofferdam_core::parser::{parse_into, source_type_for};
    use cofferdam_core::SourceFile;

    let file = SourceFile::new(PathBuf::from(&path), text.clone());
    let _source_type = source_type_for(&file.path);
    let allocator = cofferdam_core::Allocator::default();
    let parsed = parse_into(&allocator, &file);
    let views: Vec<JsLineView> = Lines::build(&file.text, &parsed.program)
        .map(|lv| JsLineView {
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
    Ok(views)
}

/// Build a span from byte offsets — convenience for the JS loader that
/// receives plugin-reported `(start_byte, end_byte)` pairs and needs to
/// produce line/column for the final `JsIssue`. Mirrors
/// `cofferdam_core::span_from_bytes`.
#[napi(object)]
pub struct JsSpan {
    pub line: u32,
    pub column: u32,
    pub start_byte: u32,
    pub end_byte: u32,
}

#[napi]
pub fn span_from_bytes(text: String, start_byte: u32, end_byte: u32) -> JsSpan {
    let span: Span = cofferdam_core::span_from_bytes(&text, start_byte, end_byte);
    JsSpan {
        line: span.line,
        column: span.column,
        start_byte: span.start_byte,
        end_byte: span.end_byte,
    }
}

/// What a plugin's `ctx.report({ ... })` produces back in JS. The TS
/// loader collects these per file and passes them to [`merge_plugin_findings`]
/// for re-anchoring + suppression-filter.
#[napi(object)]
pub struct JsReport {
    pub check_id: String,
    pub message: String,
    pub file: String,
    pub start_byte: u32,
    pub end_byte: u32,
    /// Optional severity override — empty string when the plugin didn't
    /// pass one. Lowercase form matches `Severity::as_str()`.
    pub severity: String,
}

/// Re-anchor plugin reports as engine-shape `JsIssue`s, using the source
/// `text` to derive line/column from `start_byte`. Suppression filtering
/// happens in the JS loader against the same suppression map the engine
/// builds — keeps both surfaces consistent.
#[napi]
pub fn merge_plugin_findings(text: String, reports: Vec<JsReport>) -> Vec<JsIssue> {
    reports
        .into_iter()
        .map(|r| {
            let span = cofferdam_core::span_from_bytes(&text, r.start_byte, r.end_byte);
            JsIssue {
                check_id: r.check_id,
                message: r.message,
                file: r.file.replace('\\', "/"),
                line: span.line,
                column: span.column,
                start_byte: span.start_byte,
                end_byte: span.end_byte,
                priority: 0, // plugin-supplied priority lands here once the SDK shape carries it
                severity: if r.severity.is_empty() {
                    "medium".to_string()
                } else {
                    r.severity
                },
            }
        })
        .collect()
}
