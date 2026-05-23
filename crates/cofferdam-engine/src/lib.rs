//! `cofferdam-engine` — orchestration.
//!
//! Phase 1: parse each file with oxc, expose the AST through CheckContext,
//! emit Warning.ParseError on fatal parse failures. Still single-threaded;
//! rayon comes in a follow-up once the AST seam is stable.

pub mod baseline;
pub mod cache;
pub mod config;
pub mod discover;
pub mod disk_cache;
pub mod findings_cache;
pub mod graph;
pub mod run_cache;
pub mod since;
pub mod suppress;

pub use baseline::{Baseline, BaselineEntry, BaselineError};
pub use config::{ConfigError, ProjectConfig};
pub use discover::{discover, DiscoveryOptions, DEFAULT_EXTENSIONS};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use cofferdam_core::graph::{EXPORTS, IMPORTS};
use cofferdam_core::parser::{parse_fatal, parse_into, ParsedView};
use cofferdam_core::{
    Allocator, Check, CheckContext, CheckOptions, CorpusIndex, FinalizeContext, InvariantsRuntime,
    InvariantsSpec, Issue, Language, LayersConfig, Location, Priority, Severity, SourceFile, Span,
    TypeOracle, ALL_PRE_FILTER_FINDINGS, INVARIANTS, LAYERS, REGISTERED_CHECK_IDS,
};
use cofferdam_graph::{build_canonical_graph, CANONICAL_GRAPH};
use cofferdam_rust::{parse_rust, RustParseTree};

/// Errors the engine can surface independent of per-check findings —
/// today, I/O failures while reading input paths. Per-file parse
/// failures are NOT errors here; they emit `Warning.ParseError`
/// issues through the regular findings channel.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The analyzer's orchestrator. Owns the registered check set, their
/// resolved options + severity overrides, and the project-level
/// invariants spec (when one is loaded). Call `analyze` /
/// `analyze_with_text` / `analyze_with_signatures` to run a pass over
/// a list of input paths.
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
    /// Type oracle for `requires_types` checks (cd-9hp.2). `None` means
    /// no type host is available for this run — the per-file loop then
    /// skips every check declaring `CheckMeta::requires_types`. The CLI
    /// installs a worker-backed oracle via [`Engine::with_type_oracle`]
    /// when a type-aware check is registered and the user hasn't
    /// disabled `[engine] type_aware`. Not part of `config_hash`: it's
    /// a runtime resource, not configuration.
    type_oracle: Option<Box<dyn TypeOracle>>,
}

impl Engine {
    /// Build an `Engine` from a check set with default options.
    /// Prefer `with_config` when you have a loaded `ProjectConfig`
    /// — the option/severity overrides flow from there.
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
            type_oracle: None,
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
            type_oracle: None,
        })
    }

    /// Install the type oracle for `requires_types` checks (cd-9hp.2).
    /// The CLI calls this after building the engine, once it has spawned
    /// the Node type host and opened the project's tsconfig. Without an
    /// oracle the engine silently skips every type-aware check, so this
    /// is the switch that activates the type-aware code path.
    pub fn with_type_oracle(mut self, oracle: Box<dyn TypeOracle>) -> Self {
        self.type_oracle = Some(oracle);
        self
    }

    /// True when at least one registered check declares
    /// `CheckMeta::requires_types`. The CLI consults this to decide
    /// whether to pay the type-host spawn + project-init cost: no
    /// type-aware check registered means no oracle is needed (the
    /// bead's auto-opt-out).
    pub fn needs_type_oracle(&self) -> bool {
        self.checks.iter().any(|c| c.meta().requires_types)
    }

    /// Stable hash of the engine's resolved configuration: per-check
    /// options, severity overrides, layers, invariants. Used as one
    /// axis of the per-file findings cache key
    /// ([`findings_cache::FindingsKey::config_hash`]) so a config
    /// edit invalidates cached findings.
    ///
    /// cp2's implementation hashes the Rust `Debug` format of each
    /// component — deterministic for the `BTreeMap` / `Option<T>`
    /// shapes the engine owns, but fragile across refactors. cp3
    /// swaps in a CBOR / postcard serializer for cross-process
    /// stability when the disk-backed cache lands.
    pub fn config_hash(&self) -> findings_cache::ConfigHash {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        // Per-check options, keyed by stable check id. The Vec
        // ordering mirrors the registration order, which is stable
        // within a build but not necessarily across rebuilds; we
        // sort by check_id so the hash doesn't drift on a registry
        // shuffle.
        let mut by_id: BTreeMap<&str, (String, String)> = BTreeMap::new();
        for (check, opts) in self.checks.iter().zip(self.options.iter()) {
            let id = check.meta().id;
            let sev = self
                .severities
                .get(id)
                .copied()
                .unwrap_or(check.meta().default_severity);
            by_id.insert(id, (format!("{:?}", opts), format!("{:?}", sev)));
        }
        h.update(format!("{:?}", by_id).as_bytes());
        h.update(format!("{:?}", self.layers).as_bytes());
        h.update(format!("{:?}", self.invariants).as_bytes());
        h.finalize().into()
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
        self.analyze_with_sources_cached(sources, None)
    }

    /// Variant of [`Engine::analyze_with_sources`] that consults a
    /// shared [`cache::ParseCache`] before invoking oxc on each
    /// TypeScript file. Daemon-mode callers (cd-9hp.4 cp1b's
    /// `--watch`) hold a long-lived cache across invocations so
    /// no-op re-runs skip parse for unchanged files.
    ///
    /// Behaviour matches `analyze_with_sources` byte-for-byte; the
    /// cache shaves parse cost only — corpus state is still
    /// rebuilt from scratch every cycle. Per-file findings,
    /// `(content, config, engine)`-keyed invalidation, and corpus
    /// snapshot replay land in cp2/cp3.
    pub fn analyze_with_sources_cached(
        &self,
        sources: Vec<(PathBuf, String)>,
        cache: Option<&cache::ParseCache>,
    ) -> (Vec<Issue>, HashMap<PathBuf, String>) {
        self.analyze_with_sources_caches(sources, cache, None)
    }

    /// Triple-cache entry point used by the watch loop and the cp2
    /// no-op-re-run bench. Adds a per-file findings cache on top of
    /// the parse cache: for each `CheckMeta::pure_run` check, the
    /// engine memoises `Vec<Issue>` under a
    /// `(content_hash, config_hash, check_id)` key and replays it
    /// on cache hit instead of calling `Check::run`. Non-pure
    /// checks always run.
    ///
    /// Behaviour matches `analyze_with_sources_cached` byte-for-byte
    /// modulo work skipped on hits — the findings appended to the
    /// returned issues vector are identical. cp3 layers the corpus
    /// snapshot replay on top so even non-pure checks can be
    /// skipped on hit.
    pub fn analyze_with_sources_caches(
        &self,
        sources: Vec<(PathBuf, String)>,
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
    ) -> (Vec<Issue>, HashMap<PathBuf, String>) {
        self.analyze_with_sources_full(sources, parse_cache, findings_cache, None)
    }

    /// Outermost-layer cache entry point (cd-9hp.4 cp3).
    ///
    /// Adds a [`run_cache::RunCache`] in front of the parse +
    /// findings caches. On hit (input fingerprint + config unchanged
    /// since the last analyze that populated the cache), returns the
    /// memoised issue list directly — the entire per-file loop,
    /// graph extract, non-pure check runs, and finalize are all
    /// skipped. On miss, runs the full analyze through the inner
    /// caches as normal and inserts the result.
    ///
    /// Trade-off documented in [`crate::run_cache`]: ANY file change
    /// flips the input fingerprint and invalidates the entry,
    /// falling through to a fresh analyze. Partial replay (file X
    /// changed → only X re-runs, others reuse contributions) is a
    /// follow-up bead.
    pub fn analyze_with_sources_full(
        &self,
        sources: Vec<(PathBuf, String)>,
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
        run_cache: Option<&run_cache::RunCache>,
    ) -> (Vec<Issue>, HashMap<PathBuf, String>) {
        // Outermost layer: if the run cache has an entry for this
        // exact input set + config, return its issues directly.
        // The text map is reconstructed cheaply from `sources` — no
        // point caching it. Texts come from disk (or a caller-
        // supplied vec); the cache only memoises analyzer output.
        if let Some(rc) = run_cache {
            let key = run_cache::RunKey {
                input_set: run_cache::input_set_hash(&sources),
                config_hash: self.config_hash(),
            };
            if let Some(cached) = rc.get(&key) {
                let texts: HashMap<PathBuf, String> = sources.iter().cloned().collect();
                return (cached, texts);
            }
            let (issues, texts) = self.run_cache_miss_path(sources, parse_cache, findings_cache);
            rc.insert(key, issues.clone());
            return (issues, texts);
        }
        self.run_cache_miss_path(sources, parse_cache, findings_cache)
    }

    /// The full analyze path used on a `RunCache` miss. Factored out
    /// so the outermost-cache branch can call it without code
    /// duplication.
    fn run_cache_miss_path(
        &self,
        sources: Vec<(PathBuf, String)>,
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
    ) -> (Vec<Issue>, HashMap<PathBuf, String>) {
        self.analyze_with_sources_caches_inner(sources, parse_cache, findings_cache)
    }

    /// Inner entry point — does the full per-file analysis through
    /// the parse + findings caches. Was the body of
    /// `analyze_with_sources_caches` before the cp3 outermost-layer
    /// `RunCache` was layered on top.
    fn analyze_with_sources_caches_inner(
        &self,
        sources: Vec<(PathBuf, String)>,
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
    ) -> (Vec<Issue>, HashMap<PathBuf, String>) {
        let cache = parse_cache;
        // Computed once per analysis. cp2 uses Debug-format hashing —
        // deterministic for BTreeMap-backed `CheckOptions` and the
        // derived `Debug` on `LayersConfig` / `InvariantsSpec`; good
        // enough for an in-memory cache that lives only for the
        // process. cp3's disk-backed cache will swap in a CBOR /
        // postcard serializer for cross-process stability.
        let config_hash = self.config_hash();
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
                scripted: spec.scripted.clone(),
            };
            if !runtime.is_empty() {
                corpus.with_slot(&INVARIANTS, |slot| *slot = Some(runtime));
            }
        }

        for (path, text) in &sources {
            let file = SourceFile::new(path.clone(), text.clone());
            texts.insert(path.clone(), text.clone());

            // Per-language dispatch (cd-91zc checkpoint 4 + cd-0039).
            // Non-TS files skip oxc parsing and graph extraction
            // entirely — the engine instead invokes the matching
            // language adapter's parser ONCE per file and installs the
            // resulting handle on `CheckContext.parsed_lang`. Each
            // language check downcasts via `ctx.parsed_as::<T>()` (TS
            // checks read `ctx.parsed` directly; this branch never
            // runs for them).
            //
            // Fatal parse failures emit a single `Warning.ParseError`
            // and short-circuit the per-check loop, mirroring TS
            // behaviour. This eliminated the silent no-op on malformed
            // .rs files that the pre-cd-0039 per-check `parse_rust` +
            // `has_errors()` preamble produced.
            if file.language == Language::Rust {
                // Three outcomes from parse_rust:
                //
                // * `Err(_)`: the parser failed to produce a tree at
                //   all (grammar load failure / cancellation / timeout).
                //   Surface as `Warning.ParseError` via
                //   `rust_load_error_issue`.
                //
                // * `Ok(tree)` with `has_errors() == true`: tree-sitter
                //   recovered with ERROR / MISSING nodes. Surface as
                //   `Warning.ParseError` pointing at the first error
                //   span. (Pre-cd-0039-followup this branch silently
                //   skipped — tree-sitter-rust 0.23 had false positives
                //   on valid Rust like `&raw` where `raw` is a variable
                //   name. The 0.24 grammar fixed those. The
                //   `diagnose_parse_errors` integration test in
                //   `cofferdam-rust` pins the regression: if the bug
                //   ever comes back, those tests start failing and the
                //   silent-skip would need to come back too.)
                //
                // * `Ok(tree)` clean: install on parsed_lang and run
                //   the matching checks.
                let tree = match parse_rust(&file.text) {
                    Ok(t) if !t.has_errors() => t,
                    Ok(t) => {
                        issues.push(rust_parse_error_issue(&file, &t));
                        continue;
                    }
                    Err(e) => {
                        issues.push(rust_load_error_issue(&file, &e));
                        continue;
                    }
                };
                for (check, opts) in self.checks.iter().zip(self.options.iter()) {
                    if check.language() != file.language {
                        continue;
                    }
                    let mut ctx = CheckContext::new(&file)
                        .with_options(opts)
                        .with_corpus(&corpus)
                        .with_parsed_lang(&tree);
                    issues.extend(check.run(&file, &mut ctx));
                }
                continue;
            }
            // Defensive catch-all for languages we recognise (via
            // `Language::from_path`) but haven't wired a parser for
            // yet. Falls through to the TS path so nothing crashes;
            // matching checks (none today) would see `parsed_lang =
            // None`.
            #[allow(clippy::collapsible_if)]
            if file.language != Language::TypeScript {
                for (check, opts) in self.checks.iter().zip(self.options.iter()) {
                    if check.language() != file.language {
                        continue;
                    }
                    let mut ctx = CheckContext::new(&file)
                        .with_options(opts)
                        .with_corpus(&corpus);
                    issues.extend(check.run(&file, &mut ctx));
                }
                continue;
            }

            // Post-parse pass for one TypeScript file. Pulled into a
            // closure so the cached and non-cached paths below share
            // one body — the cache branch invokes it inside
            // `ParseCache::with_parsed`'s callback (where the parsed
            // borrow can't escape), the non-cache branch invokes it
            // directly. `issues` is taken by `&mut` so the closure
            // doesn't have to capture it mutably.
            let run_ts = |parsed_return: &oxc_parser::ParserReturn<'_>, issues: &mut Vec<Issue>| {
                if parse_fatal(parsed_return) {
                    issues.push(parse_error_issue(&file, &parsed_return.errors));
                    return;
                }
                let parsed = ParsedView {
                    program: &parsed_return.program,
                    diagnostics: &parsed_return.errors,
                };

                // Pass-1 graph extraction: imports/exports for every
                // parsed file, into the well-known IMPORTS/EXPORTS
                // corpus slots. Graph-aware checks (orphan, cycle,
                // layer, dead) read these in their `finalize`. Always
                // runs — cp3 will lift graph extraction into a cached
                // (and replayable) step too.
                graph_builder.collect(&file, &parsed, &corpus);

                // Per-file content hash, computed once and reused
                // for every pure-check lookup against this file.
                // cache::hash_text is just a SHA-256 over the source
                // bytes — under a microsecond even for big files.
                let content_hash = findings_cache.map(|_| cache::hash_text(&file.text));

                for (check, opts) in self.checks.iter().zip(self.options.iter()) {
                    if check.language() != Language::TypeScript {
                        continue;
                    }
                    // Type-aware routing (cd-9hp.2). A check declaring
                    // `requires_types` only runs when the engine has a
                    // live type oracle; otherwise it's skipped entirely
                    // (no oracle → no way to answer its type queries).
                    // The findings cache never applies to type-aware
                    // checks: their results depend on the whole
                    // project's types, which the per-file content hash
                    // can't capture. A `requires_types` check must keep
                    // `pure_run = false` so the fast path below is never
                    // taken for it; the explicit guard here is belt and
                    // braces.
                    let requires_types = check.meta().requires_types;
                    if requires_types && self.type_oracle.is_none() {
                        continue;
                    }
                    // Findings-cache fast path: skip Check::run when
                    // (a) the check declares pure_run, and (b) a cache
                    // entry exists under (content, config, check_id).
                    // Non-pure checks always run — their findings may
                    // depend on corpus state, which the cache key
                    // can't capture today.
                    if check.meta().pure_run && !requires_types {
                        if let (Some(fc), Some(content_hash)) = (findings_cache, content_hash) {
                            let key = findings_cache::FindingsKey {
                                content_hash,
                                config_hash,
                                check_id: check.meta().id,
                            };
                            // Re-stamp the cached findings onto this
                            // file's path (cd-mwr6): the cache key has no
                            // path, so a byte-identical sibling file would
                            // otherwise inherit the path of whichever file
                            // first populated the entry.
                            if let Some(cached) = fc.get_for_path(&key, &file.path) {
                                issues.extend(cached);
                                continue;
                            }
                            let mut ctx = CheckContext::new(&file)
                                .with_parsed(&parsed)
                                .with_options(opts)
                                .with_corpus(&corpus);
                            let fresh = check.run(&file, &mut ctx);
                            fc.insert(key, fresh.clone());
                            issues.extend(fresh);
                            continue;
                        }
                    }
                    let mut ctx = CheckContext::new(&file)
                        .with_parsed(&parsed)
                        .with_options(opts)
                        .with_corpus(&corpus);
                    if let Some(oracle) = self.type_oracle.as_deref() {
                        ctx = ctx.with_types(oracle);
                    }
                    issues.extend(check.run(&file, &mut ctx));
                }
            };

            match cache {
                Some(c) => c.with_parsed(&file, |p| run_ts(p, &mut issues)),
                None => {
                    // Per-file allocator. Lives until the file's
                    // checks finish, then drops with the AST it owns.
                    // Bumpalo allocation makes this trivially cheap.
                    let allocator = Allocator::default();
                    let parsed_return = parse_into(&allocator, &file);
                    run_ts(&parsed_return, &mut issues);
                }
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
                // Consistency checks are TS-only today (cd-91zc). Skip
                // non-TS files so we don't re-parse them with oxc.
                if file.language != Language::TypeScript {
                    continue;
                }
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

        // Canonical-graph build (cd-9hp.9 cp3). Translates the flat
        // IMPORTS / EXPORTS slots populated above into the
        // typed-graph substrate from `cofferdam_graph`. Done once,
        // after pass 1 + pass 2 finish so every per-file record is in,
        // and before finalize runs so graph-migrated checks
        // (Design.OrphanExport first) can query it. The flat slots
        // stay populated for checks that haven't migrated yet.
        {
            let imports = corpus.with_slot(&IMPORTS, |slot| slot.clone());
            let exports = corpus.with_slot(&EXPORTS, |slot| slot.clone());
            let graph = build_canonical_graph(&imports, &exports);
            corpus.with_slot(&CANONICAL_GRAPH, |slot| *slot = graph);
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
                    .push((issue.check_id.clone(), issue.location.line()));
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
                !sup.is_suppressed(issue.location.line(), &issue.check_id)
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
                .then_with(|| a.location.line().cmp(&b.location.line()))
        });

        (issues, texts)
    }

    /// Cache-aware variant of [`Engine::analyze_with_text`]. Reads
    /// each path from disk, then routes through
    /// [`Engine::analyze_with_sources_full`] with the supplied caches.
    /// Used by the CLI to thread the cd-9hp.4 cp4 disk-backed cache
    /// into a one-shot `cofferdam check` without duplicating the file-
    /// reading prelude.
    pub fn analyze_with_text_full<P: AsRef<Path>>(
        &self,
        paths: &[P],
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
        run_cache: Option<&run_cache::RunCache>,
    ) -> Result<(Vec<Issue>, HashMap<PathBuf, String>), EngineError> {
        let mut sources: Vec<(PathBuf, String)> = Vec::with_capacity(paths.len());
        for path in paths {
            let path = path.as_ref();
            let text = std::fs::read_to_string(path).map_err(|source| EngineError::ReadFile {
                path: path.to_path_buf(),
                source,
            })?;
            sources.push((path.to_path_buf(), text));
        }
        Ok(self.analyze_with_sources_full(sources, parse_cache, findings_cache, run_cache))
    }

    /// Cache-aware variant of [`Engine::analyze_with_signatures`].
    /// Mirrors the baseline-signature post-pass on top of
    /// [`Engine::analyze_with_text_full`] so callers that need both
    /// caching and baseline signatures get a single call.
    pub fn analyze_with_signatures_full<P: AsRef<Path>>(
        &self,
        paths: &[P],
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
        run_cache: Option<&run_cache::RunCache>,
    ) -> Result<Vec<(Issue, String)>, EngineError> {
        let (issues, texts) =
            self.analyze_with_text_full(paths, parse_cache, findings_cache, run_cache)?;
        let empty = String::new();
        let out = issues
            .into_iter()
            .map(|issue| {
                let text = texts.get(&issue.file).unwrap_or(&empty);
                let sig = baseline::signature_for_span(text, &issue.location);
                (issue, sig)
            })
            .collect();
        Ok(out)
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
                let sig = baseline::signature_for_span(text, &issue.location);
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
        location: Location::from_span(
            &file.path,
            Span {
                start_byte: 0,
                end_byte: 0,
                line: 1,
                column: 1,
            },
        ),
        priority: Priority(20),
        severity: Severity::Critical,
        related: Vec::new(),
    }
}

/// Build a `Warning.ParseError` for a Rust file whose tree-sitter
/// parse recovered with ERROR / MISSING nodes. The finding points at
/// the first error span tree-sitter recovered. We emit the first one
/// only — cascading errors typically share a root cause and listing
/// them all adds noise without information (mirrors the TS adapter's
/// policy in `parse_error_issue`).
fn rust_parse_error_issue(file: &SourceFile, tree: &RustParseTree) -> Issue {
    let span = tree.error_spans().first().copied().unwrap_or(Span {
        start_byte: 0,
        end_byte: 0,
        line: 1,
        column: 1,
    });
    Issue {
        check_id: "Warning.ParseError".to_string(),
        message: "parse error: tree-sitter recovered with ERROR / MISSING nodes (Rust adapter)"
            .to_string(),
        file: file.path.clone(),
        location: Location::from_span(&file.path, span),
        priority: Priority(20),
        severity: Severity::Critical,
        related: Vec::new(),
    }
}

/// Build a `Warning.ParseError` for the rare case where tree-sitter
/// itself fails to produce a tree at all (grammar load failure, parser
/// cancellation, timeout). Distinct from `rust_parse_error_issue`
/// because there's no tree to extract error spans from.
fn rust_load_error_issue(file: &SourceFile, err: &cofferdam_rust::RustParseError) -> Issue {
    Issue {
        check_id: "Warning.ParseError".to_string(),
        message: format!("parse error: {err}"),
        file: file.path.clone(),
        location: Location::from_span(
            &file.path,
            Span {
                start_byte: 0,
                end_byte: 0,
                line: 1,
                column: 1,
            },
        ),
        priority: Priority(20),
        severity: Severity::Critical,
        related: Vec::new(),
    }
}
