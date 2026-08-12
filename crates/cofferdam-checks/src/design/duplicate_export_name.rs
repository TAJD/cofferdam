use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use cofferdam_core::graph::{InvariantsRuntime, INVARIANTS as GRAPH_INVARIANTS};
use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, Location,
    OptionDefault, OptionKind, OptionSpec, Priority, RelatedSpan, Severity, SourceFile, Span,
};
use oxc_ast::ast::{Declaration, ExportNamedDeclaration, VariableDeclaration};
use oxc_ast_visit::Visit;

/// One observed export, collected during the per-file pass.
#[derive(Clone)]
struct NamedExport {
    name: String,
    file: PathBuf,
    span: Span,
}

/// Incremental index (CD-40 lever 5): a name's group is only
/// recomputed when a file contributing to it changed since the last
/// `finalize()` call, instead of re-grouping every export on every
/// call. `by_file` holds each file's exact contribution so a removed
/// file can be subtracted precisely rather than re-derived from
/// already-mutated corpus state (see CLAUDE.md's CD-40 lever-3 trap,
/// which generalises to every incremental aggregate in this family).
#[derive(Default)]
struct DenIndex {
    by_file: HashMap<PathBuf, Vec<NamedExport>>,
    by_name: BTreeMap<String, Vec<NamedExport>>,
    /// Names whose group has changed since the last `finalize()`.
    dirty: BTreeSet<String>,
    /// Cached issue per emitting group, keyed by name. A name absent
    /// here either has <2 occurrences or is boundary-exempt.
    emitted: BTreeMap<String, Issue>,
    /// Hash of (exempt_boundary_pairs, project_root) as of the last
    /// finalize. A change invalidates every cached group, since
    /// boundary exemption can flip for any of them.
    opts_fingerprint: Option<u64>,
}

/// Per-process slot in the corpus. All `DuplicateExportName` instances share
/// it (only one is registered today, but the API permits sharing).
static DEN_INDEX: CorpusKey<DenIndex> = CorpusKey::new("Design.DuplicateExportName.index");

const DEN_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "exempt_boundary_pairs",
    kind: OptionKind::StringList,
    default: OptionDefault::StringList(&[]),
    // One entry per boundary, sides separated by `|`. A collision is
    // exempt only when every occurrence lands on a *different* side of
    // one entry, so two files inside the same side still collide and a
    // third file outside the boundary un-exempts the whole group. A flat
    // list of paths can't express that — it would mute the file
    // everywhere, not just against its declared counterpart. `|` is the
    // separator because it is not a glob metacharacter and is illegal in
    // Windows paths.
    doc: "Boundaries across which the same export name is mirrored on purpose. Each entry lists two or more path globs separated by `|` (e.g. `client/**|server/**`); a duplicate set is exempt only if each occurrence matches a distinct side of one entry. Globs are matched against the project-root-relative path and, when unanchored, at any depth.",
}];

/// `Design.DuplicateExportName` — finalize-stage cross-file check
/// that flags identifier names exported from more than one file. See
/// `CheckMeta` for the full emission rules.
pub struct DuplicateExportName;

const DEN_META: CheckMeta = CheckMeta {
    id: "Design.DuplicateExportName",
    category: Category::Design,
    base_priority: 6,
    default_severity: Severity::Medium,
    explanation: "The same name is exported from multiple files. Barrel re-exports collide silently and importers can't tell which one they got.",
    body: include_str!("../../docs/Design.DuplicateExportName.md"),
    requires_types: false,
    consistency: false,
    options: DEN_OPTIONS,
    autofix: false,
    // Writes per-file ExportRecords into the EXPORTS slot during run();
    // skipping run() on cache hit would drop those contributions and
    // break the cross-file dedupe in finalize(). cp3's corpus-snapshot
    // replay lifts this restriction.
    pure_run: false,
};

impl Check for DuplicateExportName {
    fn meta(&self) -> &'static CheckMeta {
        &DEN_META
    }

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&DEN_INDEX, |idx, path| {
            let Some(rows) = idx.by_file.remove(path) else {
                return;
            };
            for exp in &rows {
                idx.dirty.insert(exp.name.clone());
                if let Some(group) = idx.by_name.get_mut(&exp.name) {
                    group.retain(|e| e.file != path);
                    if group.is_empty() {
                        idx.by_name.remove(&exp.name);
                    }
                }
            }
        });
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = ExportCollector {
            file,
            collected: Vec::new(),
        };
        visitor.visit_program(parsed.program);

        // Hand off to the shared corpus. The lock is held only for the
        // length of this drain — every other check's per-file work
        // continues uncontended. The engine's incremental path already
        // ran `register_removable` for this file (if it existed
        // before) prior to calling `run`, so any old contribution
        // under this name is already subtracted.
        ctx.corpus.with_slot(&DEN_INDEX, |idx| {
            for exp in &visitor.collected {
                idx.dirty.insert(exp.name.clone());
                idx.by_name
                    .entry(exp.name.clone())
                    .or_default()
                    .push(exp.clone());
            }
            idx.by_file.insert(file.path.clone(), visitor.collected);
        });

        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let boundaries: Vec<Boundary> = ctx
            .options
            .get_string_list("exempt_boundary_pairs")
            .unwrap_or(&[])
            .iter()
            .map(|entry| Boundary::parse(entry))
            .collect();
        let runtime: Option<InvariantsRuntime> =
            ctx.corpus.with_slot(&GRAPH_INVARIANTS, |slot| slot.clone());
        let project_root = runtime
            .map(|r| r.project_root)
            .unwrap_or_else(|| PathBuf::from(""));

        let mut issues: Vec<Issue> = boundaries
            .iter()
            .filter(|b| !b.invalid_patterns.is_empty())
            .map(|b| {
                let patterns = b.invalid_patterns.join(", ");
                Issue {
                    check_id: DEN_META.id.to_string(),
                    message: format!(
                        "exempt_boundary_pairs entry `{}` has invalid glob pattern(s) [{}] — \
                         that side will never match any file, so no duplicate through it will \
                         ever be exempted",
                        b.raw, patterns
                    ),
                    file: project_root.clone(),
                    location: Location::from_span(
                        &project_root,
                        Span {
                            line: 1,
                            column: 1,
                            start_byte: 0,
                            end_byte: 0,
                        },
                    ),
                    priority: Priority(DEN_META.base_priority),
                    severity: Severity::Medium,
                    related: Vec::new(),
                }
            })
            .collect();

        let raw_pairs = ctx
            .options
            .get_string_list("exempt_boundary_pairs")
            .unwrap_or(&[]);
        let mut fp_hasher = DefaultHasher::new();
        raw_pairs.hash(&mut fp_hasher);
        project_root.hash(&mut fp_hasher);
        let opts_fingerprint = fp_hasher.finish();

        // Read-only aside from the `dirty`/`emitted` bookkeeping
        // (cd-32): the export contributions themselves must survive
        // this call unmutated for `Engine::analyze_incremental`'s
        // persistent `AnalysisState` — the next incremental call
        // needs `by_file`/`by_name` intact to subtract from on the
        // next edit.
        ctx.corpus.with_slot(&DEN_INDEX, |idx| {
            if idx.opts_fingerprint != Some(opts_fingerprint) {
                // Boundary config (or project root) changed since the
                // last call: exemption can flip for any group, so
                // every group must be re-evaluated.
                idx.dirty = idx.by_name.keys().cloned().collect();
                idx.opts_fingerprint = Some(opts_fingerprint);
            }
            let dirty = std::mem::take(&mut idx.dirty);
            for name in dirty {
                let Some(occurrences) = idx.by_name.get(&name) else {
                    idx.emitted.remove(&name);
                    continue;
                };
                if occurrences.len() < 2 {
                    idx.emitted.remove(&name);
                    continue;
                }
                let paths: Vec<&Path> = occurrences.iter().map(|e| e.file.as_path()).collect();
                if boundaries.iter().any(|b| b.exempts(&paths, &project_root)) {
                    idx.emitted.remove(&name);
                    continue;
                }
                // Stable order: first-seen by (file, start_byte). The
                // smallest (path, offset) becomes the primary; the
                // rest are `related`.
                let mut occurrences = occurrences.clone();
                occurrences.sort_by(|a, b| {
                    a.file
                        .cmp(&b.file)
                        .then_with(|| a.span.start_byte.cmp(&b.span.start_byte))
                });
                let primary = occurrences.remove(0);
                let related: Vec<RelatedSpan> = occurrences
                    .into_iter()
                    .map(|e| RelatedSpan {
                        location: Location::from_span(&e.file, e.span),
                        file: e.file,
                    })
                    .collect();
                idx.emitted.insert(
                    name.clone(),
                    Issue {
                        check_id: DEN_META.id.to_string(),
                        message: format!("`{}` is exported from {} files", name, related.len() + 1),
                        file: primary.file.clone(),
                        location: Location::from_span(&primary.file, primary.span),
                        priority: Priority(DEN_META.base_priority),
                        severity: Severity::Medium,
                        related,
                    },
                );
            }
            issues.extend(idx.emitted.values().cloned());
        });
        issues
    }
}

/// One configured boundary: the sides an intentional mirror may span.
struct Boundary {
    raw: String,
    sides: Vec<Side>,
    /// Side patterns that failed to compile as globs. `Side::new` skips
    /// a non-compiling matcher rather than erroring `Boundary::parse`
    /// (a boundary can still have other, valid sides), but a side with
    /// zero matchers can never match anything — silently defeating the
    /// exemption the user configured. Surfaced as a warning in
    /// `finalize()` instead of failing quietly (CD-150).
    invalid_patterns: Vec<String>,
}

struct Side {
    matchers: Vec<globset::GlobMatcher>,
}

impl Boundary {
    fn parse(entry: &str) -> Self {
        let mut invalid_patterns = Vec::new();
        let sides: Vec<Side> = entry
            .split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|pattern| {
                let side = Side::new(pattern);
                if side.matchers.is_empty() {
                    invalid_patterns.push(pattern.to_string());
                }
                side
            })
            .collect();
        Self {
            raw: entry.to_string(),
            sides,
            invalid_patterns,
        }
    }

    /// True when every occurrence sits on a side of this boundary and no
    /// two share one. Sides are claimed greedily in declaration order,
    /// so overlapping globs resolve deterministically.
    fn exempts(&self, paths: &[&Path], project_root: &Path) -> bool {
        if self.sides.len() < 2 {
            return false;
        }
        let mut claimed = vec![false; self.sides.len()];
        for path in paths {
            let rel = super::relative_normalised(project_root, path);
            let found = self
                .sides
                .iter()
                .enumerate()
                .position(|(i, side)| !claimed[i] && side.is_match(&rel));
            match found {
                Some(i) => claimed[i] = true,
                None => return false,
            }
        }
        true
    }
}

impl Side {
    fn new(pattern: &str) -> Self {
        let mut matchers = Vec::new();
        if let Ok(g) = globset::Glob::new(pattern) {
            matchers.push(g.compile_matcher());
        }
        // An unanchored `client/**` should still match a nested
        // `packages/app/client/foo.ts` — the project root isn't always
        // the directory the globs were written against.
        if !pattern.starts_with("**") && !pattern.starts_with('/') {
            if let Ok(g) = globset::Glob::new(&format!("**/{pattern}")) {
                matchers.push(g.compile_matcher());
            }
        }
        Self { matchers }
    }

    fn is_match(&self, rel: &str) -> bool {
        self.matchers.iter().any(|m| m.is_match(rel))
    }
}

struct ExportCollector<'a> {
    file: &'a SourceFile,
    collected: Vec<NamedExport>,
}

impl<'a> ExportCollector<'a> {
    fn record(&mut self, name: &str, start: u32, end: u32) {
        let span = span_from_bytes(&self.file.text, start, end);
        self.collected.push(NamedExport {
            name: name.to_string(),
            file: self.file.path.clone(),
            span,
        });
    }

    fn record_var_decl(&mut self, var: &VariableDeclaration<'a>) {
        for d in &var.declarations {
            if let Some(ident) = d.id.get_binding_identifier() {
                self.record(ident.name.as_str(), ident.span.start, ident.span.end);
            }
        }
    }
}

impl<'a> Visit<'a> for ExportCollector<'a> {
    fn visit_export_named_declaration(&mut self, node: &ExportNamedDeclaration<'a>) {
        if let Some(decl) = &node.declaration {
            match decl {
                Declaration::FunctionDeclaration(f) => {
                    if let Some(id) = &f.id {
                        self.record(id.name.as_str(), id.span.start, id.span.end);
                    }
                }
                Declaration::ClassDeclaration(c) => {
                    if let Some(id) = &c.id {
                        self.record(id.name.as_str(), id.span.start, id.span.end);
                    }
                }
                Declaration::VariableDeclaration(v) => {
                    self.record_var_decl(v);
                }
                _ => {}
            }
        }
        oxc_ast_visit::walk::walk_export_named_declaration(self, node);
    }
}
