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
    Allocator, Check, CheckContext, CheckOptions, CorpusIndex, FinalizeContext, InvariantsRuntime,
    InvariantsSpec, Issue, LayersConfig, Priority, Severity, SourceFile, Span,
    ALL_PRE_FILTER_FINDINGS, INVARIANTS, LAYERS, REGISTERED_CHECK_IDS,
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
    /// `[layers]` config from `cofferdam.toml`. `None` means the table
    /// was missing; `Some` is published into the `LAYERS` corpus slot at
    /// the start of each analysis run so `Design.LayerViolation` can
    /// read it in `finalize`.
    layers: Option<LayersConfig>,
    /// Parsed `cofferdam.invariants.toml` spec, published into the
    /// PUBLIC_API / BOUNDARIES / INVARIANTS corpus slots at the start of
    /// each analysis run. `None` skips publication, leaving the
    /// dependent checks no-ops.
    invariants: Option<InvariantsSpec>,
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
            layers: None,
            invariants: None,
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
            layers: config.layers.clone(),
            invariants: config.invariants.clone(),
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
        // Read each path from disk and delegate to the source-driven
        // entry point. Any I/O error short-circuits with `EngineError::ReadFile`.
        let mut sources: Vec<(PathBuf, String)> = Vec::with_capacity(paths.len());
        for path in paths {
            let path = path.as_ref();
            let text = std::fs::read_to_string(path).map_err(|source| EngineError::ReadFile {
                path: path.to_path_buf(),
                source,
            })?;
            sources.push((path.to_path_buf(), text));
        }
        Ok(self.analyze_with_sources(sources))
    }

    /// Run analysis against pre-loaded `(path, text)` pairs without
    /// touching the filesystem. Required for callers that materialise
    /// source from somewhere other than the working tree — `cofferdam
    /// advise --diff <ref>` is the load-bearing consumer; it resolves
    /// pre-diff source via `git show <ref>:<path>` and runs the full
    /// engine on it to compute baseline findings.
    ///
    /// Behaviour matches `analyze_with_text` byte-for-byte aside from
    /// the source. Returns the same `(issues, texts)` shape so callers
    /// can run baseline-signature computation on the result.
    pub fn analyze_with_sources(
        &self,
        sources: Vec<(PathBuf, String)>,
    ) -> (Vec<Issue>, HashMap<PathBuf, String>) {
        // Promote every input path to its absolute form before anything
        // downstream sees it. Two reasons (cd-q9f):
        //
        // 1. The graph builder's `oxc_resolver` runs `TsconfigDiscovery::Auto`
        //    by walking up from the importing file's directory looking for
        //    `tsconfig.json`. With a relative input like `./components/Bar.tsx`
        //    that walk runs out before reaching the project root, so
        //    `paths`/`baseUrl` aliases (`@/*`) silently fail to resolve and
        //    every aliased export is reported orphan.
        // 2. `Design.OrphanExport` keys imports by their resolved (absolute)
        //    path against exports keyed by `SourceFile.path`. Mixing relative
        //    inputs with absolute resolver outputs guarantees a key mismatch
        //    even when resolution succeeds.
        //
        // `std::path::absolute` resolves against the process CWD without
        // touching the filesystem (no canonicalization, no symlink follow,
        // no Windows `\\?\` prefix surprises) — exactly the lightweight
        // normalisation we want.
        let sources: Vec<(PathBuf, String)> = sources
            .into_iter()
            .map(|(p, t)| (std::path::absolute(&p).unwrap_or(p), t))
            .collect();
        let mut issues = Vec::new();
        let mut texts: HashMap<PathBuf, String> = HashMap::with_capacity(sources.len());
        // One corpus per analysis run: shared by every per-file CheckContext
        // and the post-pass FinalizeContext below. Cross-file checks (DRY,
        // export-graph rules) collect into it during run() and read it back
        // in finalize(). Per-check checks ignore it.
        let corpus = CorpusIndex::new();
        let graph_builder = graph::GraphBuilder::new();
        // Publish the set of registered check IDs so that
        // `Consistency.UnusedSuppression` can distinguish stale-by-cause
        // suppressions from stale-by-config ones (unknown check IDs are
        // `Consistency.UnknownCheckId`'s territory, not ours).
        {
            let ids: std::collections::HashSet<String> = self
                .checks
                .iter()
                .map(|c| c.meta().id.to_string())
                .collect();
            corpus.with_slot(&REGISTERED_CHECK_IDS, |slot| *slot = ids);
        }

        // Publish the layers config (if any) so finalize-stage checks
        // see it through the same corpus channel as the import/export
        // tables. Done before any check sees a file.
        if let Some(layers) = &self.layers {
            corpus.with_slot(&LAYERS, |slot| *slot = Some(layers.clone()));
        }
        // Publish invariants runtime when a spec was loaded. One slot,
        // one lock — checks (BoundaryFrozen, InvariantViolation, and
        // OrphanExport's public_api allowlist) read whichever slice
        // they care about from the same bundle. Empty bundles are
        // skipped so dependent checks no-op without touching the slot.
        if let Some(spec) = &self.invariants {
            let runtime = InvariantsRuntime {
                project_root: spec.project_root.clone(),
                public_api: spec.public_api.clone(),
                boundaries: spec.boundaries.clone(),
                invariants: spec.invariants.clone(),
            };
            if !runtime.is_empty() {
                corpus.with_slot(&INVARIANTS, |slot| *slot = Some(runtime));
            }
        }

        for (path, text) in &sources {
            let file = SourceFile::new(path.clone(), text.clone());
            texts.insert(path.clone(), text.clone());

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
            for (path, _) in &sources {
                let text = match texts.get(path) {
                    Some(t) => t,
                    None => continue, // file failed to parse in pass 1 — skip
                };
                let file = SourceFile::new(path.clone(), text.clone());
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

        // Two-phase finalize (cd-wqc; simplified in cd-9hp.5):
        //
        // Phase A — run finalize on every check that is NOT in the
        // finalize-observer set. These are the cross-file emitters:
        // OrphanExport, UnusedImport, DuplicateExportName, DeadExport,
        // ImportCycle, LayerViolation, etc.
        //
        // Dispatch by check ID (`cofferdam_core::is_finalize_observer`)
        // rather than via a generic `observes_findings` flag on `CheckMeta`.
        // The mechanism had one user in six months
        // (`Consistency.UnusedSuppression`); the flag was paying no rent.
        // If a second observer use case appears, extend
        // `FINALIZE_OBSERVER_CHECK_IDS` in cofferdam-core.
        for (check, opts) in self.checks.iter().zip(self.options.iter()) {
            if !cofferdam_core::is_finalize_observer(check.meta().id) {
                let mut finalize_ctx = FinalizeContext::new(&corpus).with_options(opts);
                issues.extend(check.finalize(&mut finalize_ctx));
            }
        }

        // Snapshot: re-build ALL_PRE_FILTER_FINDINGS from the union of
        // run/pass2 findings AND Phase A finalize findings. This gives
        // observer checks (Consistency.UnusedSuppression) a complete view
        // that includes finalize-only emitters like Warning.UnusedImport
        // and Design.OrphanExport, so they don't falsely flag live
        // suppression directives as stale.
        {
            let mut map: std::collections::HashMap<std::path::PathBuf, Vec<(String, u32)>> =
                std::collections::HashMap::new();
            for issue in &issues {
                map.entry(issue.file.clone())
                    .or_default()
                    .push((issue.check_id.clone(), issue.span.line));
            }
            corpus.with_slot(&ALL_PRE_FILTER_FINDINGS, |slot| *slot = map);
        }

        // Phase B — run finalize on every check in the observer set.
        // Today only `Consistency.UnusedSuppression` qualifies. Per-check
        // options flow in the same way as Phase A (cd-3uj).
        for (check, opts) in self.checks.iter().zip(self.options.iter()) {
            if cofferdam_core::is_finalize_observer(check.meta().id) {
                let mut finalize_ctx = FinalizeContext::new(&corpus).with_options(opts);
                issues.extend(check.finalize(&mut finalize_ctx));
            }
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

        (issues, texts)
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
