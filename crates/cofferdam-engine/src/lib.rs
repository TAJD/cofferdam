//! `cofferdam-engine` — orchestration.
//!
//! Phase 1: parse each file with oxc, expose the AST through CheckContext,
//! emit Warning.ParseError on fatal parse failures. Still single-threaded;
//! rayon comes in a follow-up once the AST seam is stable.

pub mod baseline;
pub mod config;
pub mod discover;
pub mod graph;
pub mod since;
pub mod suppress;

pub use baseline::{Baseline, BaselineEntry, BaselineError};
pub use config::{ConfigError, ProjectConfig};
pub use discover::{discover, DiscoveryOptions, DEFAULT_EXTENSIONS};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use cofferdam_core::parser::{parse_fatal, parse_into, ParsedView};
use cofferdam_core::{
    Allocator, Check, CheckContext, CheckOptions, CorpusIndex, FinalizeContext, Issue, Priority,
    Severity, SourceFile, Span,
};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub struct Engine {
    checks: Vec<Box<dyn Check>>,
    /// Resolved options per check, parallel to `checks`. Built once
    /// from each check's schema defaults, with `cofferdam.toml`
    /// overrides applied via `with_config`.
    options: Vec<CheckOptions>,
    /// Resolved severity per check_id. Built from each check's
    /// `meta.default_severity`, with `cofferdam.toml`
    /// `[checks."X.Y"] severity = "..."` overrides applied. The
    /// post-pass in `analyze_with_text` stamps each emitted issue with
    /// the value from this map — checks don't set severity themselves.
    severities: BTreeMap<String, Severity>,
}

impl Engine {
    pub fn new(checks: Vec<Box<dyn Check>>) -> Self {
        let options = checks
            .iter()
            .map(|c| CheckOptions::defaults_from(c.meta().options))
            .collect();
        let severities = checks
            .iter()
            .map(|c| {
                let meta = c.meta();
                (meta.id.to_string(), meta.default_severity)
            })
            .collect();
        Self {
            checks,
            options,
            severities,
        }
    }

    /// Build an engine with per-check option overrides AND severity
    /// overrides sourced from a `ProjectConfig` (typically loaded from
    /// `cofferdam.toml`). Unknown check IDs in the config are not
    /// treated as errors here — the CLI surfaces them via
    /// `config::unknown_check_ids` so typos are visible without
    /// breaking the build.
    #[allow(clippy::result_large_err)] // matches config::options_for
    pub fn with_config(
        checks: Vec<Box<dyn Check>>,
        config: &ProjectConfig,
        config_path: &Path,
    ) -> Result<Self, ConfigError> {
        let mut options = Vec::with_capacity(checks.len());
        let mut severities = BTreeMap::new();
        for check in &checks {
            let meta = check.meta();
            options.push(config::options_for(
                config,
                config_path,
                meta.id,
                meta.options,
            )?);
            let sev = config
                .severity_overrides
                .get(meta.id)
                .copied()
                .unwrap_or(meta.default_severity);
            severities.insert(meta.id.to_string(), sev);
        }
        Ok(Self {
            checks,
            options,
            severities,
        })
    }

    /// Run all configured checks against every file in `paths`.
    ///
    /// For each file: read text, parse with oxc, build CheckContext with
    /// the parsed view, run every check. Fatal parse failures emit a
    /// `Warning.ParseError` issue and the per-file check loop is skipped
    /// — checks that need the AST would have nothing to operate on.
    /// Non-fatal parse errors still produce a usable AST; checks run and
    /// the diagnostics are exposed via `ParsedView.diagnostics`.
    pub fn analyze<P: AsRef<Path>>(&self, paths: &[P]) -> Result<Vec<Issue>, EngineError> {
        let (issues, _texts) = self.analyze_with_text(paths)?;
        Ok(issues)
    }

    /// Same as `analyze` but also returns the per-file text cache, keyed
    /// by canonical input path. Required for any post-pass that needs to
    /// inspect the offending source — baseline signature computation is
    /// the first such consumer.
    pub fn analyze_with_text<P: AsRef<Path>>(
        &self,
        paths: &[P],
    ) -> Result<(Vec<Issue>, HashMap<PathBuf, String>), EngineError> {
        let mut issues = Vec::new();
        let mut texts: HashMap<PathBuf, String> = HashMap::with_capacity(paths.len());
        // One corpus per analysis run: shared by every per-file CheckContext
        // and the post-pass FinalizeContext below. Cross-file checks (DRY,
        // export-graph rules) collect into it during run() and read it back
        // in finalize(). Per-check checks ignore it.
        let corpus = CorpusIndex::new();
        let graph_builder = graph::GraphBuilder::new();

        for path in paths {
            let path = path.as_ref();
            let text = std::fs::read_to_string(path).map_err(|source| EngineError::ReadFile {
                path: path.to_path_buf(),
                source,
            })?;
            let file = SourceFile::new(path.to_path_buf(), text.clone());
            texts.insert(path.to_path_buf(), text);

            // Per-file allocator. Lives until the file's checks finish,
            // then drops with the AST it owns. Bumpalo allocation makes
            // this trivially cheap.
            let allocator = Allocator::default();
            let parsed_return = parse_into(&allocator, &file);

            if parse_fatal(&parsed_return) {
                issues.push(parse_error_issue(&file, &parsed_return.errors));
                continue;
            }

            let parsed = ParsedView {
                program: &parsed_return.program,
                diagnostics: &parsed_return.errors,
            };

            // Pass-1 graph extraction: imports/exports for every parsed
            // file, into the well-known IMPORTS/EXPORTS corpus slots.
            // Graph-aware checks (orphan, cycle, layer, dead) read these
            // in their `finalize`. Doing this BEFORE checks run means a
            // future check could even consume the graph from inside its
            // own per-file `run`.
            graph_builder.collect(&file, &parsed, &corpus);

            for (check, opts) in self.checks.iter().zip(self.options.iter()) {
                let mut ctx = CheckContext::new(&file)
                    .with_parsed(&parsed)
                    .with_options(opts)
                    .with_corpus(&corpus);
                issues.extend(check.run(&file, &mut ctx));
            }
        }

        // Pass 2: iterate every file again for consistency checks.
        // Only checks with `meta().consistency == true` are called.
        // This runs AFTER all files' pass-1 `run()` is complete so that
        // corpus slots are fully populated (e.g. per-file quote stats).
        let consistency_checks: Vec<(usize, &dyn Check)> = self
            .checks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.meta().consistency)
            .map(|(i, c)| (i, c.as_ref()))
            .collect();

        if !consistency_checks.is_empty() {
            for path in paths {
                let path = path.as_ref();
                let text = match texts.get(path) {
                    Some(t) => t,
                    None => continue, // file failed to parse in pass 1 — skip
                };
                let file = SourceFile::new(path.to_path_buf(), text.clone());
                let allocator = Allocator::default();
                let parsed_return = parse_into(&allocator, &file);
                if parse_fatal(&parsed_return) {
                    continue;
                }
                let parsed = ParsedView {
                    program: &parsed_return.program,
                    diagnostics: &parsed_return.errors,
                };
                for (idx, check) in &consistency_checks {
                    let mut ctx = CheckContext::new(&file)
                        .with_parsed(&parsed)
                        .with_options(&self.options[*idx])
                        .with_corpus(&corpus);
                    issues.extend(check.pass2(&file, &mut ctx));
                }
            }
        }

        let mut finalize_ctx = FinalizeContext::new(&corpus);
        for check in &self.checks {
            issues.extend(check.finalize(&mut finalize_ctx));
        }

        // Post-collection filter (cd-5t7): suppress findings based on inline directives.
        // Build a suppression map for each file and filter issues.
        let suppressions_by_file: HashMap<PathBuf, suppress::Suppressions> = texts
            .iter()
            .map(|(path, text)| (path.clone(), suppress::Suppressions::parse(text)))
            .collect();

        issues.retain(|issue| {
            if let Some(sup) = suppressions_by_file.get(&issue.file) {
                !sup.is_suppressed(issue.span.line, &issue.check_id)
            } else {
                true
            }
        });

        // Severity post-pass (cd-t1a): stamp each issue with its
        // configured severity. Checks emit Severity::Medium as a
        // placeholder; the engine is the single source of truth for
        // what severity each check has. Engine-internal issues like
        // `Warning.ParseError` aren't in the per-check map; their
        // severity (Critical, set in `parse_error_issue`) is left
        // untouched.
        for issue in &mut issues {
            if let Some(sev) = self.severities.get(&issue.check_id) {
                issue.severity = *sev;
            }
        }

        issues.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.check_id.cmp(&b.check_id))
                .then_with(|| a.file.cmp(&b.file))
                .then_with(|| a.span.line.cmp(&b.span.line))
        });

        Ok((issues, texts))
    }

    /// Run analysis and emit each issue paired with its baseline
    /// signature (SHA-256 of the trimmed offending span text). Used by
    /// `cofferdam baseline write` and `cofferdam check --baseline`.
    ///
    /// For finalize-time issues whose `file` we never read in the per-
    /// file loop (cross-file checks pointing into already-dropped texts,
    /// or unusual paths), the signature is computed against an empty
    /// snippet — a stable but coarse fallback. In practice every issue's
    /// `file` is one we just analyzed, so the cache hits.
    pub fn analyze_with_signatures<P: AsRef<Path>>(
        &self,
        paths: &[P],
    ) -> Result<Vec<(Issue, String)>, EngineError> {
        let (issues, texts) = self.analyze_with_text(paths)?;
        let empty = String::new();
        let out = issues
            .into_iter()
            .map(|issue| {
                let text = texts.get(&issue.file).unwrap_or(&empty);
                let sig = baseline::signature_for_span(text, &issue.span);
                (issue, sig)
            })
            .collect();
        Ok(out)
    }
}

/// Build a `Warning.ParseError` issue from oxc diagnostics. We surface the
/// first error verbatim — a fatal parse failure typically cascades, so
/// listing all of them adds noise without information.
fn parse_error_issue(file: &SourceFile, diagnostics: &[oxc_diagnostics::OxcDiagnostic]) -> Issue {
    let message = diagnostics
        .first()
        .map(|d| format!("parse error: {}", d.message))
        .unwrap_or_else(|| "parse error: oxc parser panicked".to_string());

    Issue {
        check_id: "Warning.ParseError".to_string(),
        message,
        file: file.path.clone(),
        // Phase-1: we don't yet map oxc spans into our Span shape (that's
        // the work in cd-81a.2 / A2). Point at line 1 col 1 so formatters
        // produce something coherent; the exact diagnostic location is
        // already in `message`.
        span: Span {
            start_byte: 0,
            end_byte: 0,
            line: 1,
            column: 1,
        },
        priority: Priority(20),
        severity: Severity::Critical,
        related: Vec::new(),
    }
}
