//! `cofferdam-engine` — orchestration.
//!
//! Phase 1: parse each file with oxc, expose the AST through CheckContext,
//! emit Warning.ParseError on fatal parse failures. The per-file run loop
//! and pass 2 run in parallel via rayon when no external `ParseCache` is
//! supplied (CD-30); see `analyze_with_sources_caches_inner`'s dispatch
//! comment for why a caller-supplied `ParseCache` forces the sequential
//! path instead.

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
pub mod timing;

pub use baseline::{Baseline, BaselineEntry, BaselineError};
pub use config::{ConfigError, ProjectConfig};
pub use discover::{discover, DiscoveryOptions, DEFAULT_EXTENSIONS};
pub use timing::TimingCollector;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use std::collections::BTreeMap;

use cofferdam_core::graph::{FinalizeScope, EXPORTS, FINALIZE_SCOPE, IMPORTS};
use cofferdam_core::parser::{parse_fatal, parse_into, ParsedView};
use cofferdam_core::{
    validate_options, Allocator, Category, ChangeSet, Check, CheckContext, CheckOptions,
    ContextItem, CorpusIndex, FinalizeContext, InvariantsRuntime, InvariantsSpec, Issue, Language,
    LayersConfig, Location, OptionSpec, Priority, RawOptionValue, Severity, SourceFile, Span,
    TypeOracle, ALL_PRE_FILTER_FINDINGS, INVARIANTS, LAYERS, REGISTERED_CHECK_IDS,
};
use cofferdam_graph::{apply_records, build_canonical_graph, CANONICAL_GRAPH};
use cofferdam_html::{parse_html, HtmlParseTree};
use cofferdam_rust::{parse_rust, RustParseTree};
use rayon::prelude::*;

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

/// Result of a `cofferdam context` engine run: the ordinary findings
/// (feed Context.Findings summarization, CD-159) plus the advisory
/// items from Context-category providers.
pub struct ContextOutput {
    pub issues: Vec<Issue>,
    pub items: Vec<ContextItem>,
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
    /// `[[overrides]]` blocks from the project config (cd-m5tu). Empty
    /// for `Engine::new` and for configs that declare none — the
    /// per-file loop then takes a zero-cost fast path. Applied in
    /// declaration order; the last matching block wins per check+key.
    overrides: Vec<config::OverrideBlock>,
    /// Global per-check raw option bags (a clone of `ProjectConfig::checks`),
    /// retained so an override block's option keys can be overlaid onto
    /// the global config and re-validated per file. Only consulted when
    /// `overrides` is non-empty.
    raw_checks: BTreeMap<String, BTreeMap<String, RawOptionValue>>,
}

/// Retained state for [`Engine::analyze_incremental`] (cd-32).
///
/// Holds everything a from-scratch analyze would otherwise rebuild:
/// the corpus (cross-file evidence + finalize-stage slots), the graph
/// builder (its `oxc_resolver` instance), a content-hash-keyed parse
/// cache, and — per currently-known file — its text and the `Vec<Issue>`
/// its pass-1 `run()` (checks + graph extraction) produced. A caller
/// holds one `AnalysisState` across many `analyze_incremental` calls
/// (e.g. the watch loop, one per filesystem event); each call only
/// re-does pass-1 work for the files it's told changed.
///
/// Implicitly bound to the `Engine` it was first passed to (CD-40
/// lever 2): `initialized` gates `publish_static_slots`
/// (registered-check-ids, layers, invariants — all engine-config-
/// derived) to run once rather than every call, since none of that
/// changes between calls for one `Engine`. Reusing a state across a
/// config hot-reload (a *different* `Engine` instance, same state)
/// would leave those slots stale — no current caller does this (the
/// watch loop holds one `Engine` for its lifetime), but a caller that
/// wants to pick up new config should start a fresh `AnalysisState`.
#[derive(Default)]
pub struct AnalysisState {
    corpus: CorpusIndex,
    graph_builder: graph::GraphBuilder,
    parse_cache: cache::ParseCache,
    /// Text of every file currently contributing to the corpus.
    sources: HashMap<PathBuf, String>,
    /// Pass-1 issues (checks + graph extraction) per currently-known
    /// file, so an incremental call can reuse the ones whose file
    /// didn't change instead of re-running `Check::run`.
    pass1_issues: HashMap<PathBuf, Vec<Issue>>,
    /// Parsed inline suppression directives per file. Re-parsed only
    /// for changed files each incremental call — parsing every file's
    /// directives in finalize dominated the incremental cost (cd-32).
    suppressions: HashMap<PathBuf, suppress::Suppressions>,
    /// `SourceFile` per currently-known file (CD-40 lever 1), kept in
    /// sync with `sources` on every insert/remove. The pass-2
    /// consistency sweep in `analyze_incremental` previously rebuilt a
    /// `SourceFile` (a full text clone) for all ~N files on every
    /// call, even though only `changed`/`removed` files need one
    /// rebuilt — this cache eliminates that. `sources` itself stays a
    /// plain text map so `AnalysisState::sources()`'s public shape is
    /// unaffected.
    source_files: HashMap<PathBuf, SourceFile>,
    /// Cached pass-2 (consistency-check) issues per currently-known
    /// file (CD-40 lever 4). Only populated/consulted when every
    /// registered consistency check is `pass2_is_file_local` — see
    /// `analyze_incremental`, which skips recomputing pass 2 for
    /// unchanged files in that case and reuses this cache instead.
    pass2_issues: HashMap<PathBuf, Vec<Issue>>,
    /// Whether `register_removers`/`publish_static_slots` have already
    /// run against this state's corpus (CD-40 lever 2). Both are
    /// idempotent overwrites of engine-derived (not per-call) data, so
    /// re-running them on every `analyze_incremental` call — as
    /// happened pre-CD-40 — was pure waste once a state is seeded.
    initialized: bool,
}

impl AnalysisState {
    /// Empty state — as if no file had ever been analyzed. The first
    /// `analyze_incremental` call against it behaves like a
    /// from-scratch analyze of whatever `changed` it's given.
    pub fn new() -> Self {
        Self::default()
    }

    /// Paths currently contributing to the retained corpus, with
    /// their last-seen text. Mirrors the `texts` map a from-scratch
    /// analyze would return for the same file set.
    pub fn sources(&self) -> &HashMap<PathBuf, String> {
        &self.sources
    }

    /// The retained content-hash-keyed parse cache. Exposed for
    /// callers that want to report hit/miss stats (e.g. `cofferdam
    /// watch`'s diagnostics) — incremental analysis itself only ever
    /// reads it through `Engine::analyze_incremental`.
    pub fn parse_cache(&self) -> &cache::ParseCache {
        &self.parse_cache
    }
}

/// How [`Engine::finalize_and_filter`] should get the `CANONICAL_GRAPH`
/// corpus slot to its post-this-call state (CD-40 lever 3). Also drives
/// the `FINALIZE_SCOPE` corpus slot published for finalize-stage checks
/// (CD-40 lever 5 PR 2).
#[derive(Clone, Copy)]
enum GraphUpdate<'a> {
    /// Build a fresh graph from every IMPORTS/EXPORTS record in the
    /// corpus. Used by every from-scratch `analyze_*` entry point,
    /// where there's no prior graph to reuse.
    Rebuild,
    /// Reuse the graph already persisted in the corpus's
    /// `CANONICAL_GRAPH` slot. Its `removed`/stale-`changed`
    /// contributions were already stripped by the registered remover
    /// (see `Engine::register_removers`) when `analyze_incremental`
    /// called `corpus.remove_file` earlier in the same call; only
    /// `changed`'s freshly re-populated records still need replaying
    /// in via `apply_records`.
    Incremental {
        changed: &'a [PathBuf],
        removed: &'a [PathBuf],
    },
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
            overrides: Vec::new(),
            raw_checks: BTreeMap::new(),
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
        // Schema per check id, so we can validate per-glob option
        // overrides (below) against the same schema the global options
        // were validated against.
        let mut schema_by_id: BTreeMap<&str, &'static [OptionSpec]> = BTreeMap::new();
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
            schema_by_id.insert(meta.id, meta.options);
        }

        // Validate per-glob override option values eagerly (cd-m5tu) so
        // a bad `limit = "x"` in an [[overrides]] block fails at load
        // time with the same diagnostics as the global [checks."X"]
        // block, rather than silently per file. Overrides naming an
        // unregistered check id are left to `config::unknown_check_ids`
        // (a warning, not an error), so they're skipped here.
        for block in &config.overrides {
            for (check_id, oc) in &block.checks {
                if oc.options.is_empty() {
                    continue;
                }
                let Some(schema) = schema_by_id.get(check_id.as_str()) else {
                    continue;
                };
                let merged = merge_raw(config.checks.get(check_id), &oc.options);
                config::options_for_raw(config_path, check_id, schema, &merged)?;
            }
        }

        Ok(Self {
            checks,
            options,
            severities,
            layers: config.layers.clone(),
            invariants: config.invariants.clone(),
            type_oracle: None,
            overrides: config.overrides.clone(),
            raw_checks: config.checks.clone(),
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
    /// options, severity overrides, layers, invariants, and whether a
    /// type oracle is installed. Used as one axis of both the per-file
    /// findings cache key ([`findings_cache::FindingsKey::config_hash`])
    /// and the whole-run cache key ([`run_cache::RunKey::config_hash`])
    /// so a config edit invalidates cached findings.
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
        // Per-glob overrides (cd-m5tu): two configs that differ only in
        // an [[overrides]] block must hash differently so a toggled
        // override busts the disk findings cache. Hash the raw patterns
        // + per-check data (the compiled GlobSet isn't part of the
        // logical config).
        for block in &self.overrides {
            h.update(format!("{:?}", (&block.paths, &block.checks)).as_bytes());
        }
        // CD-152: whether the type oracle (ts-morph host) is installed
        // changes what every `requires_types` check emits for identical
        // (content, per-check-config) inputs, but none of the axes
        // above capture it. Without this, the whole-run disk cache
        // (`run_cache::RunCache`, persisted across separate `cofferdam
        // check` invocations) replays a prior run's type-aware findings
        // — or their absence — across a ts-morph install/uninstall that
        // didn't touch any file or config.
        h.update([self.type_oracle.is_some() as u8]);
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
        self.analyze_with_sources_full_impl(
            sources,
            parse_cache,
            findings_cache,
            run_cache,
            None,
            None,
        )
    }

    /// Same as [`Engine::analyze_with_sources_full`] but records
    /// per-check and per-phase timing into `timing` (CD-34,
    /// `cofferdam check --time-checks`). Findings are byte-identical
    /// to the untimed path — timing is a side channel only.
    pub fn analyze_with_sources_full_timed(
        &self,
        sources: Vec<(PathBuf, String)>,
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
        run_cache: Option<&run_cache::RunCache>,
        timing: &TimingCollector,
    ) -> (Vec<Issue>, HashMap<PathBuf, String>) {
        self.analyze_with_sources_full_impl(
            sources,
            parse_cache,
            findings_cache,
            run_cache,
            Some(timing),
            None,
        )
    }

    /// Run a `cofferdam context` analysis: the normal from-scratch
    /// analyze (no caches in v1 — cache reuse is a follow-up), then
    /// Context-category providers with `changeset` against the
    /// completed corpus. `issues` are the ordinary findings; the
    /// caller decides what to do with them (CD-159 summarizes).
    pub fn analyze_context(
        &self,
        sources: Vec<(PathBuf, String)>,
        changeset: &ChangeSet,
    ) -> ContextOutput {
        let mut items = Vec::new();
        let (issues, _texts) = self.analyze_with_sources_full_impl(
            sources,
            None,
            None,
            None,
            None,
            Some((changeset, &mut items)),
        );
        ContextOutput { issues, items }
    }

    #[allow(clippy::too_many_arguments)]
    fn analyze_with_sources_full_impl(
        &self,
        sources: Vec<(PathBuf, String)>,
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
        run_cache: Option<&run_cache::RunCache>,
        timing: Option<&TimingCollector>,
        context: Option<(&ChangeSet, &mut Vec<ContextItem>)>,
    ) -> (Vec<Issue>, HashMap<PathBuf, String>) {
        // Absolutize once, up front, so every returned `texts` map is
        // keyed identically to every returned `Issue.file` regardless
        // of which branch below serves the result. Previously this
        // normalization happened only inside `analyze_with_sources_caches_inner`
        // (the miss path), so a `RunCache` hit reconstructed `texts`
        // from the raw (often relative) `sources` while replaying
        // `Issue`s already stamped with absolute paths from the miss
        // pass that first populated the cache — `texts.get(&issue.file)`
        // then missed on every lookup, silently degrading baseline
        // signatures to the hash of an empty snippet (CD-153).
        let sources = absolutize_sources(sources);
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
                return ((*cached).clone(), texts);
            }
            let (issues, texts) =
                self.run_cache_miss_path(sources, parse_cache, findings_cache, timing, context);
            rc.insert(key, issues.clone());
            return (issues, texts);
        }
        self.run_cache_miss_path(sources, parse_cache, findings_cache, timing, context)
    }

    /// The full analyze path used on a `RunCache` miss. Factored out
    /// so the outermost-cache branch can call it without code
    /// duplication.
    #[allow(clippy::too_many_arguments)]
    fn run_cache_miss_path(
        &self,
        sources: Vec<(PathBuf, String)>,
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
        timing: Option<&TimingCollector>,
        context: Option<(&ChangeSet, &mut Vec<ContextItem>)>,
    ) -> (Vec<Issue>, HashMap<PathBuf, String>) {
        self.analyze_with_sources_caches_inner(
            sources,
            parse_cache,
            findings_cache,
            timing,
            context,
        )
    }

    /// Inner entry point — does the full per-file analysis through
    /// the parse + findings caches. Was the body of
    /// `analyze_with_sources_caches` before the cp3 outermost-layer
    /// `RunCache` was layered on top.
    #[allow(clippy::too_many_arguments)]
    fn analyze_with_sources_caches_inner(
        &self,
        sources: Vec<(PathBuf, String)>,
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
        timing: Option<&TimingCollector>,
        context: Option<(&ChangeSet, &mut Vec<ContextItem>)>,
    ) -> (Vec<Issue>, HashMap<PathBuf, String>) {
        // Two execution modes, chosen by whether the caller supplied a
        // long-lived `ParseCache` (CD-30):
        //
        // * `Some(cache)` — sequential, single-threaded. Pass 1
        //   populates the cache; pass 2 reads the same entry back by
        //   content hash instead of re-parsing (CD-29). This is the
        //   only mode a `ParseCache` can run in: oxc's arena-based
        //   `ParserReturn` holds raw pointers internally and is not
        //   `Send`, so a cached parse can never cross a thread
        //   boundary (see `cache.rs`).
        // * `None` — parallel via rayon. Each file's parse stays local
        //   to whichever worker thread handles it (own `Allocator`,
        //   never shared), so there's nothing to make `Send`. Pass 2
        //   re-parses each file fresh, in parallel, rather than
        //   reusing pass 1's AST — a small redundant-parse cost traded
        //   for real multi-core scaling on the dominant run-loop phase.
        //
        // No production caller passes `Some` today (CLI and LSP both
        // pass `None`) — the sequential path exists for callers that
        // need cross-call reuse (e.g. a future daemon/watch mode) and
        // is exercised directly by `parse_cache.rs` / `run_cache.rs`.
        //
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
        // normalisation we want. Idempotent: `analyze_with_sources_full_impl`
        // already absolutized `sources` before reaching here (CD-153), and
        // every other caller of this inner function needs the same form.
        let sources = absolutize_sources(sources);
        let mut issues = Vec::new();
        // Every input path gets an entry regardless of parse outcome
        // (that's how callers clear stale results for a file that
        // used to have issues) — built upfront from `sources` rather
        // than incrementally so the per-file loop below has no
        // shared-mutation step to make thread-safe.
        let texts: HashMap<PathBuf, String> = sources.iter().cloned().collect();
        // One corpus per analysis run: shared by every per-file CheckContext
        // and the post-pass FinalizeContext below. Cross-file checks (DRY,
        // export-graph rules) collect into it during run() and read it back
        // in finalize(). Per-check checks ignore it.
        let corpus = CorpusIndex::new();
        let graph_builder = graph::GraphBuilder::new();
        self.publish_static_slots(&corpus);

        let run_loop_start = Instant::now();
        match parse_cache {
            Some(cache) => {
                // Sequential: shares one `ParseCache` across pass 1 and
                // pass 2 (CD-29). Must stay single-threaded — see the
                // dispatch comment above.
                for (path, text) in &sources {
                    issues.extend(self.pass1_one_file(
                        path,
                        text,
                        &corpus,
                        &graph_builder,
                        findings_cache,
                        config_hash,
                        timing,
                        Some(cache),
                    ));
                }
            }
            None => {
                // CD-30: one rayon task per file. Each task's TS parse
                // (if any) lives entirely inside `pass1_one_file` —
                // never shared, so nothing here needs to be `Send`
                // beyond the `Vec<Issue>` results `collect()` gathers.
                let per_file: Vec<Vec<Issue>> = sources
                    .par_iter()
                    .map(|(path, text)| {
                        self.pass1_one_file(
                            path,
                            text,
                            &corpus,
                            &graph_builder,
                            findings_cache,
                            config_hash,
                            timing,
                            None,
                        )
                    })
                    .collect();
                issues.extend(per_file.into_iter().flatten());
            }
        }
        if let Some(t) = timing {
            t.record_phase("run_loop", run_loop_start.elapsed());
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

        let pass2_start = Instant::now();
        if !consistency_checks.is_empty() {
            match parse_cache {
                Some(cache) => {
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
                        // Same content hash as pass 1's parse of this file
                        // (byte-identical text) — this hits the cache instead
                        // of re-parsing (CD-29).
                        let file_issues = cache.with_parsed(&file, |parsed_return| {
                            if parse_fatal(parsed_return) {
                                return Vec::new();
                            }
                            let parsed = ParsedView {
                                program: &parsed_return.program,
                                diagnostics: &parsed_return.errors,
                            };
                            self.pass2_ts_file(&file, &parsed, &consistency_checks, &corpus, timing)
                        });
                        issues.extend(file_issues);
                    }
                }
                None => {
                    // CD-30: no shared cache to reuse pass 1's parse
                    // across threads (see the dispatch comment above),
                    // so each rayon task re-parses its file fresh.
                    let per_file: Vec<Vec<Issue>> = sources
                        .par_iter()
                        .filter_map(|(path, _)| {
                            let text = texts.get(path)?;
                            let file = SourceFile::new(path.clone(), text.clone());
                            if file.language != Language::TypeScript {
                                return None;
                            }
                            let allocator = Allocator::default();
                            let parsed_return = parse_into(&allocator, &file);
                            if parse_fatal(&parsed_return) {
                                return None;
                            }
                            let parsed = ParsedView {
                                program: &parsed_return.program,
                                diagnostics: &parsed_return.errors,
                            };
                            Some(self.pass2_ts_file(
                                &file,
                                &parsed,
                                &consistency_checks,
                                &corpus,
                                timing,
                            ))
                        })
                        .collect();
                    issues.extend(per_file.into_iter().flatten());
                }
            }
        }
        if let Some(t) = timing {
            t.record_phase("pass2", pass2_start.elapsed());
        }

        let suppressions_by_file: HashMap<PathBuf, suppress::Suppressions> = texts
            .par_iter()
            .map(|(path, text)| (path.clone(), suppress::Suppressions::parse(text)))
            .collect();
        let issues = self.finalize_and_filter(
            &corpus,
            issues,
            &suppressions_by_file,
            timing,
            GraphUpdate::Rebuild,
            context,
        );
        (issues, texts)
    }

    /// Publish the run-scoped slots that don't accumulate per-file
    /// evidence — they're fully overwritten rather than appended to,
    /// so republishing them is idempotent and safe to call again on
    /// every [`Engine::analyze_incremental`] call.
    fn publish_static_slots(&self, corpus: &CorpusIndex) {
        // Publish the set of registered check IDs so that
        // `Consistency.UnusedSuppression` can distinguish stale-by-cause
        // suppressions from stale-by-config ones (unknown check IDs are
        // `Consistency.UnknownCheckId`'s territory, not ours).
        let ids: std::collections::HashSet<String> = self
            .checks
            .iter()
            .map(|c| c.meta().id.to_string())
            .collect();
        corpus.with_slot(&REGISTERED_CHECK_IDS, |slot| *slot = ids);

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
    }

    /// Shared tail of every analyze path (cd-32): canonical-graph
    /// build, two-phase finalize, suppression filter, severity
    /// stamp, and the final deterministic sort. Factored out so
    /// [`Engine::analyze_incremental`] produces byte-identical output
    /// to a from-scratch [`Engine::analyze_with_sources`] given the
    /// same current file set — both funnel through this one method.
    ///
    /// `graph_update` (CD-40 lever 3) selects how the `CANONICAL_GRAPH`
    /// corpus slot gets to its post-this-call state: see [`GraphUpdate`].
    #[allow(clippy::too_many_arguments)]
    fn finalize_and_filter(
        &self,
        corpus: &CorpusIndex,
        mut issues: Vec<Issue>,
        suppressions_by_file: &HashMap<PathBuf, suppress::Suppressions>,
        timing: Option<&TimingCollector>,
        graph_update: GraphUpdate<'_>,
        context: Option<(&ChangeSet, &mut Vec<ContextItem>)>,
    ) -> Vec<Issue> {
        // Canonical-graph build (cd-9hp.9 cp3; incrementalised in
        // CD-40 lever 3). Translates the flat IMPORTS / EXPORTS slots
        // populated above into the typed-graph substrate from
        // `cofferdam_graph`. Done once, after pass 1 + pass 2 finish
        // so every per-file record is in, and before finalize runs so
        // graph-migrated checks (Design.OrphanExport first) can query
        // it. The flat slots stay populated for checks that haven't
        // migrated yet.
        {
            let graph_build_start = Instant::now();
            match graph_update {
                GraphUpdate::Rebuild => {
                    corpus.with_slot(&IMPORTS, |imports| {
                        corpus.with_slot(&EXPORTS, |exports| {
                            let all_imports = imports.to_vec();
                            let all_exports = exports.to_vec();
                            let graph = build_canonical_graph(&all_imports, &all_exports);
                            corpus.with_slot(&CANONICAL_GRAPH, |slot| *slot = graph);
                        });
                    });
                }
                GraphUpdate::Incremental { changed, .. } => {
                    // `removed`/stale-`changed` contributions were
                    // already dropped from the persisted
                    // `CANONICAL_GRAPH` slot by the registered
                    // remover (see `register_removers`) when
                    // `analyze_incremental` called
                    // `corpus.remove_file` earlier this call — so all
                    // that's left is replaying `changed`'s FRESH
                    // records (just re-populated into IMPORTS/EXPORTS
                    // by this call's `pass1_one_file` runs) against
                    // the live graph, exactly like
                    // `Graph::remove_file`'s own doc comment describes
                    // for incremental callers.
                    //
                    // Per-file storage (CD-40 lever 5 PR 2) turns this
                    // into O(|changed|) direct lookups instead of an
                    // O(total records) scan filtered by
                    // `changed.contains(..)` per record.
                    corpus.with_slot(&IMPORTS, |imports| {
                        corpus.with_slot(&EXPORTS, |exports| {
                            let filtered_imports: Vec<_> = changed
                                .iter()
                                .flat_map(|f| imports.for_file(f))
                                .cloned()
                                .collect();
                            let filtered_exports: Vec<_> = changed
                                .iter()
                                .flat_map(|f| exports.for_file(f))
                                .cloned()
                                .collect();
                            corpus.with_slot(&CANONICAL_GRAPH, |graph| {
                                apply_records(graph, &filtered_imports, &filtered_exports);
                            });
                        });
                    });
                }
            }
            if let Some(t) = timing {
                t.record_phase("graph_build", graph_build_start.elapsed());
            }
        }

        corpus.with_slot(&FINALIZE_SCOPE, |scope| {
            *scope = match graph_update {
                GraphUpdate::Rebuild => FinalizeScope::Full,
                GraphUpdate::Incremental { changed, removed } => FinalizeScope::Delta {
                    changed: changed.to_vec(),
                    removed: removed.to_vec(),
                },
            };
        });

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
        let finalize_a_start = Instant::now();
        // Cross-file finalize emitters are independent (each only reads
        // the corpus and returns its findings) so they run in parallel.
        // `collect` is index-ordered, so the emitted sequence is
        // identical to the sequential order regardless of thread timing
        // — the deterministic sort below still normalises, but this keeps
        // the pre-sort snapshot stable too (cd-32 perf: finalize_a was
        // ~5ms, the largest remaining whole-corpus cost per edit).
        let phase_a_checks: Vec<_> = self
            .checks
            .iter()
            .zip(self.options.iter())
            .filter(|(check, _)| !cofferdam_core::is_finalize_observer(check.meta().id))
            .collect();
        let phase_a: Vec<Issue> = phase_a_checks
            .par_iter()
            .flat_map_iter(|(check, opts)| {
                let mut finalize_ctx = FinalizeContext::new(corpus).with_options(opts);
                timed_run(timing, check.meta().id, || {
                    check.finalize(&mut finalize_ctx)
                })
            })
            .collect();
        issues.extend(phase_a);
        if let Some(t) = timing {
            t.record_phase("finalize_a", finalize_a_start.elapsed());
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
        let finalize_b_start = Instant::now();
        for (check, opts) in self.checks.iter().zip(self.options.iter()) {
            if cofferdam_core::is_finalize_observer(check.meta().id) {
                let mut finalize_ctx = FinalizeContext::new(corpus).with_options(opts);
                issues.extend(timed_run(timing, check.meta().id, || {
                    check.finalize(&mut finalize_ctx)
                }));
            }
        }
        if let Some(t) = timing {
            t.record_phase("finalize_b", finalize_b_start.elapsed());
        }

        // Context-category providers (CD-158): run only for
        // `Engine::analyze_context` callers, after finalize Phase B so
        // the corpus (including CANONICAL_GRAPH) is fully populated.
        // Ordinary `cofferdam check` runs pass `context: None` and never
        // reach this branch.
        if let Some((changeset, items_out)) = context {
            for (check, opts) in self.checks.iter().zip(self.options.iter()) {
                if check.meta().category != Category::Context {
                    continue;
                }
                let mut finalize_ctx = FinalizeContext::new(corpus).with_options(opts);
                items_out.extend(check.context_items(changeset, &mut finalize_ctx));
            }
        }

        // Per-glob `disabled` overrides (cd-97): `run()` call sites already
        // skip a disabled check per-file (cd-m5tu), but finalize-emitted
        // checks (OrphanExport, DeadExport, ImportCycle, etc.) run once over
        // the whole corpus with no per-file gate, so a `disabled = true`
        // block previously had no effect on their findings. Apply it here,
        // post-hoc, keyed on each issue's own file. Cheap no-op when the
        // project has no `[[overrides]]` at all.
        if !self.overrides.is_empty() {
            issues.retain(|issue| {
                let disabled = self
                    .checks
                    .iter()
                    .find(|c| c.meta().id == issue.check_id)
                    .is_some_and(|check| {
                        let file_key = path_key(&issue.file);
                        self.effective_options(&file_key, &issue.check_id, check.meta().options)
                            .0
                    });
                !disabled
            });
        }

        // Post-collection filter (cd-5t7): suppress findings based on
        // inline directives. The per-file suppression map is built by the
        // caller (from-scratch parses it fresh; incremental keeps a
        // persistent map and re-parses only changed files — cd-32 perf:
        // parsing all files' directives here dominated the incremental
        // finalize, ~7.7ms on a 325-file repo).
        //
        // CD-77: an Issue with `related` spans (e.g. Refactor.DuplicateBlock's
        // paired occurrences) is suppressed if a `cofferdam-ignore` sits at
        // EITHER the primary location OR any related location — not just the
        // primary. Without this, only the occurrence the engine happened to
        // pick as "primary" (deterministic but not user-visible: earliest
        // file path, then earliest byte offset) could be suppressed; an
        // ignore comment placed at the other side of the duplicate had no
        // effect, which is surprising since both spans describe the same
        // finding.
        issues.retain(|issue| {
            let is_suppressed_at = |file: &std::path::Path, line: u32| {
                suppressions_by_file
                    .get(file)
                    .is_some_and(|sup| sup.is_suppressed(line, &issue.check_id))
            };
            let suppressed = is_suppressed_at(&issue.file, issue.location.line())
                || issue
                    .related
                    .iter()
                    .any(|r| is_suppressed_at(&r.file, r.location.line()));
            !suppressed
        });

        // Severity post-pass (cd-t1a): stamp each issue with its
        // configured severity. Checks emit Severity::Medium as a
        // placeholder; the engine is the single source of truth for
        // what severity each check has. Engine-internal issues like
        // `Warning.ParseError` aren't in the per-check map; their
        // severity (Critical, set in `parse_error_issue`) is left
        // untouched.
        for issue in &mut issues {
            if let Some(sev) = self.effective_severity(&issue.check_id, &issue.file) {
                issue.severity = sev;
            }
        }

        issues.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.check_id.cmp(&b.check_id))
                .then_with(|| a.file.cmp(&b.file))
                .then_with(|| a.location.line().cmp(&b.location.line()))
        });

        issues
    }

    /// Pass-1 work for one file: language dispatch, parse, and every
    /// matching check's `run()`. Shared by both the sequential
    /// (`parse_cache: Some`) and rayon-parallel (`parse_cache: None`)
    /// run-loop dispatch in `analyze_with_sources_caches_inner` (CD-30)
    /// — the only difference between the two call sites is whether a
    /// shared `ParseCache` is available to short-circuit the TS parse.
    #[allow(clippy::too_many_arguments)]
    fn pass1_one_file(
        &self,
        path: &Path,
        text: &str,
        corpus: &CorpusIndex,
        graph_builder: &graph::GraphBuilder,
        findings_cache: Option<&findings_cache::FindingsCache>,
        config_hash: findings_cache::ConfigHash,
        timing: Option<&TimingCollector>,
        parse_cache: Option<&cache::ParseCache>,
    ) -> Vec<Issue> {
        let file = SourceFile::new(path.to_path_buf(), text.to_string());
        let mut issues = Vec::new();

        // Per-glob override key (cd-m5tu). `None` when no
        // `[[overrides]]` blocks are configured — the per-check
        // resolution below then short-circuits to the global config
        // with zero extra work for the overwhelmingly common case.
        let file_key = (!self.overrides.is_empty()).then(|| path_key(&file.path));

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
                    return issues;
                }
                Err(e) => {
                    issues.push(rust_load_error_issue(&file, &e));
                    return issues;
                }
            };
            for (check, opts) in self.checks.iter().zip(self.options.iter()) {
                if !check.languages().contains(&file.language) {
                    continue;
                }
                let (disabled, ov_opts) = match &file_key {
                    Some(k) => self.effective_options(k, check.meta().id, check.meta().options),
                    None => (false, None),
                };
                if disabled {
                    continue;
                }
                let opts = ov_opts.as_ref().unwrap_or(opts);
                let mut ctx = CheckContext::new(&file)
                    .with_options(opts)
                    .with_corpus(corpus)
                    .with_parsed_lang(&tree);
                issues.extend(timed_run(timing, check.meta().id, || {
                    check.run(&file, &mut ctx)
                }));
            }
            return issues;
        }
        // HTML (CD-83): same shape as the Rust branch above — parse
        // once via `cofferdam_html::parse_html`, install the tree on
        // `parsed_lang`, and dispatch to checks declaring
        // `Language::Html`. Must run BEFORE the Astro branch and the
        // defensive catch-all below, otherwise the catch-all (which
        // never parses) would swallow `.html` files with
        // `parsed_lang = None`.
        if file.language == Language::Html {
            let tree = match parse_html(&file.text) {
                Ok(t) if !t.has_errors() => t,
                Ok(t) => {
                    issues.push(html_parse_error_issue(&file, &t));
                    return issues;
                }
                Err(e) => {
                    issues.push(html_load_error_issue(&file, &e));
                    return issues;
                }
            };
            for (check, opts) in self.checks.iter().zip(self.options.iter()) {
                if !check.languages().contains(&file.language) {
                    continue;
                }
                let (disabled, ov_opts) = match &file_key {
                    Some(k) => self.effective_options(k, check.meta().id, check.meta().options),
                    None => (false, None),
                };
                if disabled {
                    continue;
                }
                let opts = ov_opts.as_ref().unwrap_or(opts);
                let mut ctx = CheckContext::new(&file)
                    .with_options(opts)
                    .with_corpus(corpus)
                    .with_parsed_lang(&tree);
                issues.extend(timed_run(timing, check.meta().id, || {
                    check.run(&file, &mut ctx)
                }));
            }
            return issues;
        }
        // Astro (cd-45): no whole-file parse — only the frontmatter
        // fence's imports get pulled into the shared IMPORTS slot so
        // islands referenced solely from `.astro` pages aren't flagged
        // as orphan exports. No built-in check declares
        // `Language::Astro`, so there's nothing else to dispatch here.
        if file.language == Language::Astro {
            graph_builder.collect_astro(&file, corpus);
            return issues;
        }

        // Defensive catch-all for languages we recognise (via
        // `Language::from_path`) but haven't wired a parser for
        // yet. Falls through to the TS path so nothing crashes;
        // matching checks see `parsed_lang = None` and work off raw
        // text. This is the branch that dispatches
        // `Consistency.SpellingDialect` to Markdown (CD-316).
        #[allow(clippy::collapsible_if)]
        if file.language != Language::TypeScript {
            for (check, opts) in self.checks.iter().zip(self.options.iter()) {
                if !check.languages().contains(&file.language) {
                    continue;
                }
                let (disabled, ov_opts) = match &file_key {
                    Some(k) => self.effective_options(k, check.meta().id, check.meta().options),
                    None => (false, None),
                };
                if disabled {
                    continue;
                }
                let opts = ov_opts.as_ref().unwrap_or(opts);
                let mut ctx = CheckContext::new(&file)
                    .with_options(opts)
                    .with_corpus(corpus);
                issues.extend(timed_run(timing, check.meta().id, || {
                    check.run(&file, &mut ctx)
                }));
            }
            return issues;
        }

        // TypeScript: reuse a shared `ParseCache` entry when the
        // caller supplied one (sequential path — CD-29's pass-1/pass-2
        // reuse), otherwise parse fresh with a local `Allocator` (the
        // rayon-parallel path — CD-30). Either way the parse never
        // outlives this call, so `run_ts_checks` doesn't care which
        // branch produced it.
        match parse_cache {
            Some(cache) => {
                cache.with_parsed(&file, |parsed_return| {
                    issues.extend(self.run_ts_checks(
                        &file,
                        parsed_return,
                        &file_key,
                        corpus,
                        graph_builder,
                        findings_cache,
                        config_hash,
                        timing,
                    ));
                });
            }
            None => {
                let allocator = Allocator::default();
                let parsed_return = parse_into(&allocator, &file);
                issues.extend(self.run_ts_checks(
                    &file,
                    &parsed_return,
                    &file_key,
                    corpus,
                    graph_builder,
                    findings_cache,
                    config_hash,
                    timing,
                ));
            }
        }
        issues
    }

    /// Pass-1 checks for one already-parsed TypeScript file: graph
    /// extraction, then every TS check's `run()` (with the
    /// findings-cache fast path). Called from `pass1_one_file` with
    /// either a cached or a freshly-parsed `ParserReturn` — the parse
    /// itself never crosses a thread boundary, so this method doesn't
    /// need to be anything more than an ordinary `&self` call.
    #[allow(clippy::too_many_arguments)]
    fn run_ts_checks(
        &self,
        file: &SourceFile,
        parsed_return: &oxc_parser::ParserReturn<'_>,
        file_key: &Option<String>,
        corpus: &CorpusIndex,
        graph_builder: &graph::GraphBuilder,
        findings_cache: Option<&findings_cache::FindingsCache>,
        config_hash: findings_cache::ConfigHash,
        timing: Option<&TimingCollector>,
    ) -> Vec<Issue> {
        let mut issues = Vec::new();
        if parse_fatal(parsed_return) {
            issues.push(parse_error_issue(file, &parsed_return.errors));
            return issues;
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
        graph_builder.collect(file, &parsed, corpus);

        // Per-file content hash, computed once and reused
        // for every pure-check lookup against this file.
        // cache::hash_text is just a SHA-256 over the source
        // bytes — under a microsecond even for big files.
        let content_hash = findings_cache.map(|_| cache::hash_text(&file.text));

        for (check, opts) in self.checks.iter().zip(self.options.iter()) {
            if !check.languages().contains(&Language::TypeScript) {
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
            // Per-glob overrides (cd-m5tu). `disabled` skips the
            // check on this file; `ov_opts` carries this file's
            // effective options when an override changed them.
            let (disabled, ov_opts) = match file_key {
                Some(k) => self.effective_options(k, check.meta().id, check.meta().options),
                None => (false, None),
            };
            if disabled {
                continue;
            }
            let opts = ov_opts.as_ref().unwrap_or(opts);
            // Findings-cache fast path: skip Check::run when
            // (a) the check declares pure_run, (b) no per-glob
            // override changed this file's options (the cache key
            // can't capture per-file option deltas — cd-m5tu),
            // and (c) a cache entry exists under
            // (content, config, check_id). Non-pure checks always
            // run — their findings may depend on corpus state,
            // which the cache key can't capture today.
            if check.meta().pure_run && !requires_types && ov_opts.is_none() {
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
                    let mut ctx = CheckContext::new(file)
                        .with_parsed(&parsed)
                        .with_options(opts)
                        .with_corpus(corpus);
                    let fresh = timed_run(timing, check.meta().id, || check.run(file, &mut ctx));
                    fc.insert(key, fresh.clone());
                    issues.extend(fresh);
                    continue;
                }
            }
            let mut ctx = CheckContext::new(file)
                .with_parsed(&parsed)
                .with_options(opts)
                .with_corpus(corpus);
            if let Some(oracle) = self.type_oracle.as_deref() {
                ctx = ctx.with_types(oracle);
            }
            issues.extend(timed_run(timing, check.meta().id, || {
                check.run(file, &mut ctx)
            }));
        }
        issues
    }

    /// Pass-2 consistency checks for one already-parsed TypeScript
    /// file. Called from both the sequential (shared `ParseCache`) and
    /// parallel (fresh per-file parse) pass-2 dispatch in
    /// `analyze_with_sources_caches_inner` (CD-30).
    fn pass2_ts_file(
        &self,
        file: &SourceFile,
        parsed: &ParsedView<'_>,
        consistency_checks: &[(usize, &dyn Check)],
        corpus: &CorpusIndex,
        timing: Option<&TimingCollector>,
    ) -> Vec<Issue> {
        let mut issues = Vec::new();
        for (idx, check) in consistency_checks {
            let mut ctx = CheckContext::new(file)
                .with_parsed(parsed)
                .with_options(&self.options[*idx])
                .with_corpus(corpus);
            issues.extend(timed_run(timing, check.meta().id, || {
                check.pass2(file, &mut ctx)
            }));
        }
        issues
    }

    /// Register every check's cross-file corpus remover plus the
    /// engine-owned `IMPORTS`/`EXPORTS`/`CANONICAL_GRAPH` removers on
    /// `state`'s corpus. Idempotent — re-registering the same slot
    /// name just overwrites the previous closure — but as of CD-40
    /// lever 2 only called once per `AnalysisState` (see
    /// `AnalysisState::initialized`) rather than on every call, since
    /// none of this is call-specific.
    ///
    /// The `CANONICAL_GRAPH` remover feeds CD-40 lever 3: it must key
    /// on the SAME normalised path `apply_records` used as owner
    /// (`cofferdam_graph::normalized_file_path`), not the raw absolute
    /// path `analyze_incremental` passes to `CorpusIndex::remove_file`
    /// — those differ on Windows (case-folding), and a mismatch here
    /// would silently no-op the removal, leaking stale graph nodes/edges.
    fn register_removers(&self, corpus: &CorpusIndex) {
        corpus.register_removable(&IMPORTS, |slot, path| slot.remove_file(path));
        corpus.register_removable(&EXPORTS, |slot, path| slot.remove_file(path));
        corpus.register_removable(&CANONICAL_GRAPH, |graph, path| {
            graph.remove_file(&cofferdam_graph::normalized_file_path(path));
        });
        for check in &self.checks {
            check.register_removable(corpus);
        }
    }

    /// Incremental analyze (cd-32): re-analyzes only `changed` files
    /// and drops `removed` ones, reusing everything else from `state`.
    ///
    /// Flow: drop `removed`/stale `changed` files' corpus
    /// contributions via [`CorpusIndex::remove_file`], re-run pass 1
    /// for `changed` files only (repopulating the corpus and
    /// `state`'s per-file issue cache), reuse every other file's
    /// cached pass-1 issues, then re-run pass 2 + the full finalize
    /// pipeline over the complete, now-current file set — mirroring
    /// [`Engine::finalize_and_filter`], the same tail a from-scratch
    /// analyze funnels through, so output is byte-identical to
    /// calling `analyze_with_sources` on `state`'s resulting file set.
    ///
    /// `changed` covers both edited and newly-added files — either
    /// way the file's prior contributions (if any) are dropped and
    /// pass 1 re-runs against the new text. `analyze_with_sources*`
    /// entry points are unaffected by this method's existence.
    pub fn analyze_incremental(
        &self,
        state: &mut AnalysisState,
        changed: &[(PathBuf, String)],
        removed: &[PathBuf],
    ) -> Vec<Issue> {
        let changed: Vec<(PathBuf, String)> = changed
            .iter()
            .map(|(p, t)| {
                (
                    std::path::absolute(p).unwrap_or_else(|_| p.clone()),
                    t.clone(),
                )
            })
            .collect();
        let removed: Vec<PathBuf> = removed
            .iter()
            .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))
            .collect();

        let config_hash = self.config_hash();
        if !state.initialized {
            self.register_removers(&state.corpus);
            self.publish_static_slots(&state.corpus);
            state.initialized = true;
        }

        for path in &removed {
            state.corpus.remove_file(path);
            state.sources.remove(path);
            state.source_files.remove(path);
            state.pass1_issues.remove(path);
            state.pass2_issues.remove(path);
            state.suppressions.remove(path);
        }

        for (path, text) in &changed {
            // Drop this file's prior contributions (a no-op the first
            // time a path is seen — `remove_file` is harmless for
            // paths with no registered contributions).
            state.corpus.remove_file(path);
            let file_issues = self.pass1_one_file(
                path,
                text,
                &state.corpus,
                &state.graph_builder,
                None,
                config_hash,
                None,
                Some(&state.parse_cache),
            );
            state
                .suppressions
                .insert(path.clone(), suppress::Suppressions::parse(text));
            state.sources.insert(path.clone(), text.clone());
            state
                .source_files
                .insert(path.clone(), SourceFile::new(path.clone(), text.clone()));
            state.pass1_issues.insert(path.clone(), file_issues);
        }

        let mut issues: Vec<Issue> = state.pass1_issues.values().flatten().cloned().collect();

        // Pass 2: consistency checks normally re-run over every
        // currently-known file, not just `changed` — a consistency
        // check's pass-2 verdict for an untouched file can in general
        // depend on pass-1 evidence from a file that DID change (e.g.
        // a hypothetical project-wide dominant style). Parses go
        // through `state.parse_cache`, so unchanged files are a
        // content-hash cache hit rather than a re-parse. Each file's
        // `SourceFile` comes from `state.source_files` (CD-40 lever 1)
        // instead of being rebuilt here — rebuilding meant a full text
        // clone for every currently-known file on every incremental
        // call, dominating the per-edit floor on large corpora.
        //
        // CD-40 lever 4: when every registered consistency check is
        // `pass2_is_file_local` (its verdict provably depends only on
        // that file's own pass-1 evidence), an unchanged file's pass-2
        // output cannot have shifted since the last call — skip the
        // parse-cache lookup and `pass2` dispatch entirely and reuse
        // `state.pass2_issues` instead. A single check that reads
        // cross-file evidence in pass2 falls back to recomputing every
        // file on every call, same as before this lever.
        let consistency_checks: Vec<(usize, &dyn Check)> = self
            .checks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.meta().consistency)
            .map(|(i, c)| (i, c.as_ref()))
            .collect();
        if !consistency_checks.is_empty() {
            let cacheable = consistency_checks
                .iter()
                .all(|(_, c)| c.pass2_is_file_local());
            // A linear scan, not a `HashSet`: `changed` is almost always
            // a single file (the common single-edit call this lever
            // targets), where hashing costs more than it saves. Only
            // built/consulted at all when `cacheable`.
            let is_changed = |path: &Path| changed.iter().any(|(p, _)| p.as_path() == path);

            for (path, file) in &state.source_files {
                if file.language != Language::TypeScript {
                    continue;
                }
                // A cache miss here means this path was never populated —
                // only reachable by reusing `state` against a different
                // `Engine` (see `AnalysisState`'s doc comment), a usage
                // no current caller does. Falling through to recompute
                // rather than treating a miss as "zero issues" keeps
                // that unsupported case merely stale instead of silently
                // dropping findings.
                if cacheable && !is_changed(path) {
                    if let Some(cached) = state.pass2_issues.get(path) {
                        issues.extend(cached.iter().cloned());
                        continue;
                    }
                }
                let file_issues = state.parse_cache.with_parsed(file, |parsed_return| {
                    if parse_fatal(parsed_return) {
                        return Vec::new();
                    }
                    let parsed = ParsedView {
                        program: &parsed_return.program,
                        diagnostics: &parsed_return.errors,
                    };
                    self.pass2_ts_file(file, &parsed, &consistency_checks, &state.corpus, None)
                });
                if cacheable {
                    state.pass2_issues.insert(path.clone(), file_issues.clone());
                }
                issues.extend(file_issues);
            }
        }

        // finalize reads the persistent per-file suppression map — only
        // changed files were re-parsed above (cd-32 perf).
        let changed_paths: Vec<PathBuf> = changed.iter().map(|(p, _)| p.clone()).collect();
        self.finalize_and_filter(
            &state.corpus,
            issues,
            &state.suppressions,
            None,
            GraphUpdate::Incremental {
                changed: &changed_paths,
                removed: &removed,
            },
            None,
        )
    }

    /// Cache-aware variant of [`Engine::analyze_with_text`]. Reads
    /// each path from disk, then routes through
    /// [`Engine::analyze_with_sources_full`] with the supplied caches.
    /// Used by the CLI to thread the cd-9hp.4 cp4 disk-backed cache
    /// into a one-shot `cofferdam check` without duplicating the file-
    /// reading prelude.
    pub fn analyze_with_text_full<P: AsRef<Path> + Sync>(
        &self,
        paths: &[P],
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
        run_cache: Option<&run_cache::RunCache>,
    ) -> Result<(Vec<Issue>, HashMap<PathBuf, String>), EngineError> {
        let sources = Self::read_sources(paths)?;
        Ok(self.analyze_with_sources_full(sources, parse_cache, findings_cache, run_cache))
    }

    /// Timed variant of [`Engine::analyze_with_text_full`] (CD-34).
    pub fn analyze_with_text_full_timed<P: AsRef<Path> + Sync>(
        &self,
        paths: &[P],
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
        run_cache: Option<&run_cache::RunCache>,
        timing: &TimingCollector,
    ) -> Result<(Vec<Issue>, HashMap<PathBuf, String>), EngineError> {
        let sources = Self::read_sources(paths)?;
        Ok(self.analyze_with_sources_full_timed(
            sources,
            parse_cache,
            findings_cache,
            run_cache,
            timing,
        ))
    }

    fn read_sources<P: AsRef<Path> + Sync>(
        paths: &[P],
    ) -> Result<Vec<(PathBuf, String)>, EngineError> {
        paths
            .par_iter()
            .map(|path| {
                let path = path.as_ref();
                let text =
                    std::fs::read_to_string(path).map_err(|source| EngineError::ReadFile {
                        path: path.to_path_buf(),
                        source,
                    })?;
                Ok((path.to_path_buf(), text))
            })
            .collect()
    }

    /// Cache-aware variant of [`Engine::analyze_with_signatures`].
    /// Mirrors the baseline-signature post-pass on top of
    /// [`Engine::analyze_with_text_full`] so callers that need both
    /// caching and baseline signatures get a single call.
    pub fn analyze_with_signatures_full<P: AsRef<Path> + Sync>(
        &self,
        paths: &[P],
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
        run_cache: Option<&run_cache::RunCache>,
    ) -> Result<Vec<(Issue, String)>, EngineError> {
        let (issues, texts) =
            self.analyze_with_text_full(paths, parse_cache, findings_cache, run_cache)?;
        Ok(Self::sign_issues(issues, &texts))
    }

    /// Timed variant of [`Engine::analyze_with_signatures_full`] (CD-34).
    /// Used by `cofferdam check --time-checks`.
    pub fn analyze_with_signatures_full_timed<P: AsRef<Path> + Sync>(
        &self,
        paths: &[P],
        parse_cache: Option<&cache::ParseCache>,
        findings_cache: Option<&findings_cache::FindingsCache>,
        run_cache: Option<&run_cache::RunCache>,
        timing: &TimingCollector,
    ) -> Result<Vec<(Issue, String)>, EngineError> {
        let (issues, texts) = self.analyze_with_text_full_timed(
            paths,
            parse_cache,
            findings_cache,
            run_cache,
            timing,
        )?;
        Ok(Self::sign_issues(issues, &texts))
    }

    fn sign_issues(issues: Vec<Issue>, texts: &HashMap<PathBuf, String>) -> Vec<(Issue, String)> {
        let empty = String::new();
        issues
            .into_iter()
            .map(|issue| {
                let text = texts.get(&issue.file).unwrap_or(&empty);
                let sig = baseline::signature_for_issue(text, &issue);
                (issue, sig)
            })
            .collect()
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
                let sig = baseline::signature_for_issue(text, &issue);
                (issue, sig)
            })
            .collect();
        Ok(out)
    }

    /// Resolve the effective per-file config for one check (cd-m5tu).
    /// Returns `(disabled, options)`:
    /// * `disabled` — true when the last matching `[[overrides]]` block
    ///   that mentions `disabled` set it to `true` for this check.
    /// * `options` — `Some` when a matching block sets at least one
    ///   option key: the global per-check bag overlaid with the override
    ///   keys (last block wins), re-validated against the check schema.
    ///   `None` means "use the engine's global options".
    ///
    /// `file_key` is a normalised key from [`path_key`]. Callers take a
    /// fast path when `self.overrides` is empty, so this always walks at
    /// least one block when reached.
    fn effective_options(
        &self,
        file_key: &str,
        check_id: &str,
        schema: &[OptionSpec],
    ) -> (bool, Option<CheckOptions>) {
        let mut disabled: Option<bool> = None;
        let mut merged: Option<BTreeMap<String, RawOptionValue>> = None;
        for block in &self.overrides {
            if !block.is_match(file_key) {
                continue;
            }
            let Some(oc) = block.checks.get(check_id) else {
                continue;
            };
            if let Some(d) = oc.disabled {
                disabled = Some(d);
            }
            if !oc.options.is_empty() {
                let mut base = merged
                    .take()
                    .unwrap_or_else(|| self.raw_checks.get(check_id).cloned().unwrap_or_default());
                for (k, v) in &oc.options {
                    base.insert(k.clone(), v.clone());
                }
                merged = Some(base);
            }
        }
        // Option values were validated at engine-build time, so this
        // re-validation should not fail; on the impossible error path we
        // fall back to the engine's global options rather than panic.
        let options = merged.and_then(|raw| validate_options(check_id, schema, &raw).ok());
        (disabled.unwrap_or(false), options)
    }

    /// Effective severity for one finding (cd-m5tu, on top of cd-t1a).
    /// Global per-check severity first, then any matching `[[overrides]]`
    /// block that sets `severity` for this check (last match wins).
    /// `None` only for check ids absent from the global map (e.g. the
    /// engine-internal `Warning.ParseError`), leaving them untouched.
    fn effective_severity(&self, check_id: &str, file: &Path) -> Option<Severity> {
        let mut sev = self.severities.get(check_id).copied();
        if !self.overrides.is_empty() {
            let key = path_key(file);
            for block in &self.overrides {
                if block.is_match(&key) {
                    if let Some(oc) = block.checks.get(check_id) {
                        if let Some(s) = oc.severity {
                            sev = Some(s);
                        }
                    }
                }
            }
        }
        sev
    }
}

/// Promote every input path to its absolute form (cd-q9f), resolved
/// against the process CWD without touching the filesystem (no
/// canonicalization, no symlink follow, no Windows `\\?\` prefix
/// surprises). The single definition every entry point into the
/// per-file analysis pipeline must use: every `Issue.file` the engine
/// produces is stamped with this form, so any `texts` map built
/// alongside those issues has to be keyed the same way or lookups by
/// `issue.file` (e.g. baseline signature computation) silently miss
/// (CD-153).
fn absolutize_sources(sources: Vec<(PathBuf, String)>) -> Vec<(PathBuf, String)> {
    sources
        .into_iter()
        .map(|(p, t)| (std::path::absolute(&p).unwrap_or(p), t))
        .collect()
}

/// Overlay an override block's option keys onto the global per-check
/// raw bag (cd-m5tu). Global keys first; override keys win on conflict.
fn merge_raw(
    global: Option<&BTreeMap<String, RawOptionValue>>,
    overlay: &BTreeMap<String, RawOptionValue>,
) -> BTreeMap<String, RawOptionValue> {
    let mut merged = global.cloned().unwrap_or_default();
    for (k, v) in overlay {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

/// Run one check invocation (`run` / `pass2` / `finalize`), recording its
/// elapsed time under `check_id` when a [`TimingCollector`] is present
/// (CD-34). A no-op wrapper when `timing` is `None` — no `Instant::now()`
/// call on the hot path for ordinary runs.
fn timed_run<F: FnOnce() -> Vec<Issue>>(
    timing: Option<&TimingCollector>,
    check_id: &'static str,
    f: F,
) -> Vec<Issue> {
    match timing {
        Some(t) => {
            let start = Instant::now();
            let issues = f();
            t.record_check(check_id, start.elapsed());
            issues
        }
        None => f(),
    }
}

/// Forward-slash, lowercase-on-Windows path key for override glob
/// matching (cd-m5tu). Mirrors `config::path_key` and the checks
/// crate's `path_key` so the engine's absolute file paths match globs
/// written project-root-relative.
fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
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

/// Build a `Warning.ParseError` for an HTML file whose tree-sitter
/// parse recovered with ERROR / MISSING nodes. Mirrors
/// `rust_parse_error_issue` — see its docs for the "first span only"
/// rationale.
fn html_parse_error_issue(file: &SourceFile, tree: &HtmlParseTree) -> Issue {
    let span = tree.error_spans().first().copied().unwrap_or(Span {
        start_byte: 0,
        end_byte: 0,
        line: 1,
        column: 1,
    });
    Issue {
        check_id: "Warning.ParseError".to_string(),
        message: "parse error: tree-sitter recovered with ERROR / MISSING nodes (HTML adapter)"
            .to_string(),
        file: file.path.clone(),
        location: Location::from_span(&file.path, span),
        priority: Priority(20),
        severity: Severity::Critical,
        related: Vec::new(),
    }
}

/// Build a `Warning.ParseError` for the rare case where tree-sitter
/// itself fails to produce a tree at all. Mirrors `rust_load_error_issue`.
fn html_load_error_issue(file: &SourceFile, err: &cofferdam_html::HtmlParseError) -> Issue {
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
