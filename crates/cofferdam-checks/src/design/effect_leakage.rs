use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use cofferdam_core::graph::{ImportRecord, IMPORTS as GRAPH_IMPORTS};
use cofferdam_core::{path_key, span_from_bytes};
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, Location,
    Priority, Severity, SourceFile, Span,
};

/// Node.js built-ins and common database/queue client packages treated as
/// inherently side-effecting. Matched against the import specifier with
/// any `node:` prefix stripped, so `import "node:fs"` and `import "fs"`
/// are equivalent.
const SIDE_EFFECTING_MODULES: &[&str] = &[
    "fs",
    "fs/promises",
    "net",
    "http",
    "https",
    "http2",
    "dgram",
    "tls",
    "dns",
    "child_process",
    "cluster",
    "pg",
    "mysql",
    "mysql2",
    "mongodb",
    "mongoose",
    "sqlite3",
    "better-sqlite3",
    "redis",
    "ioredis",
    "knex",
    "sequelize",
    "typeorm",
    "prisma",
    "@prisma/client",
];

/// One file that opted into a purity contract via a `@pure` JSDoc tag.
#[derive(Clone)]
struct PureFile {
    file: PathBuf,
    span: Span,
}

/// Per-process slot: every file this check saw that carries a top-level
/// `@pure` tag. Populated in `run()`, consumed in `finalize()` alongside
/// the shared import graph.
static PURE_FILES: CorpusKey<Vec<PureFile>> = CorpusKey::new("Design.EffectLeakage.pure_files");

const META: CheckMeta = CheckMeta {
    id: "Design.EffectLeakage",
    category: Category::Design,
    base_priority: 8,
    default_severity: Severity::Medium,
    explanation: "A module opted into a `@pure` contract, but transitively imports a known \
        side-effecting module (filesystem, network, a database client) somewhere down its \
        import chain — the annotation is making a promise the code doesn't keep.",
    body: include_str!("../../docs/Design.EffectLeakage.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    // Writes into PURE_FILES during run(); skipping on cache hit would
    // drop that file's contribution and silently under-report in
    // finalize(), mirroring Design.DuplicateExportName.
    pure_run: false,
};

/// `Design.EffectLeakage` — cross-file check (CD-127) that flags a file
/// annotated `@pure` (in a JSDoc-style comment anywhere in the file) whose
/// transitive import chain reaches a known side-effecting module.
///
/// Scope: file-level only — `@pure` is read as a whole-module contract,
/// not a per-function one, since the transitive walk operates on the
/// file-level import graph the engine already builds. The side-effecting
/// module list is a fixed denylist (mirrors other checks' static
/// denylists, e.g. `Refactor.SideEffectInMapCallback`'s `console.*`); it
/// isn't configurable in v1. Only imports that resolve to a known
/// external specifier are checked — an internal file that itself wraps
/// a side-effecting module (e.g. a thin `fs` adapter) is treated the same
/// as the side-effecting module itself would be, since the walk follows
/// resolved internal edges transitively until it hits an unresolved
/// (external) specifier.
pub struct EffectLeakage;

impl Check for EffectLeakage {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&PURE_FILES, |slot, path| slot.retain(|p| p.file != path));
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let pure_span = parsed
            .program
            .comments
            .iter()
            .find(|c| {
                file.text
                    .get(c.span.start as usize..c.span.end as usize)
                    .is_some_and(has_pure_tag)
            })
            .map(|c| span_from_bytes(&file.text, c.span.start, c.span.end));
        if let Some(span) = pure_span {
            ctx.corpus.with_slot(&PURE_FILES, |slot| {
                slot.push(PureFile {
                    file: file.path.clone(),
                    span,
                });
            });
        }
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let pure_files: Vec<PureFile> = ctx.corpus.with_slot(&PURE_FILES, |slot| slot.clone());
        if pure_files.is_empty() {
            return Vec::new();
        }
        let imports: Vec<ImportRecord> = ctx.corpus.with_slot(&GRAPH_IMPORTS, |slot| slot.clone());
        compute_leaks(&pure_files, &imports)
    }
}

/// True if `text` (a comment's full contents, oxc's `Comment.span` may or
/// may not include the `//`/`/* */` delimiters depending on comment kind)
/// contains an actual `@pure` JSDoc-style tag — a line whose content,
/// after stripping comment delimiters/asterisks and whitespace from both
/// ends, is exactly `@pure` or starts with `@pure` followed by
/// whitespace. This deliberately rejects prose that merely mentions
/// `@pure` (e.g. `// TODO: mark this @pure eventually`), which a plain
/// substring search would misfire on.
fn has_pure_tag(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line
            .trim()
            .trim_matches(|c: char| c == '/' || c == '*')
            .trim();
        trimmed == "@pure" || trimmed.starts_with("@pure ") || trimmed.starts_with("@pure\t")
    })
}

/// The side-effecting module name a specifier matches, if any, after
/// stripping a `node:` prefix.
fn side_effecting_match(specifier: &str) -> Option<&'static str> {
    let normalized = specifier.strip_prefix("node:").unwrap_or(specifier);
    SIDE_EFFECTING_MODULES
        .iter()
        .find(|&&m| m == normalized)
        .copied()
}

fn compute_leaks(pure_files: &[PureFile], imports: &[ImportRecord]) -> Vec<Issue> {
    // Adjacency by resolved internal file, plus the external specifiers
    // each file imports directly (resolver returned None).
    let mut internal_edges: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut external_edges: HashMap<String, Vec<String>> = HashMap::new();
    for imp in imports {
        let key = path_key(&imp.from_file);
        match &imp.resolved {
            Some(resolved) => internal_edges
                .entry(key)
                .or_default()
                .push(resolved.clone()),
            None => external_edges
                .entry(key)
                .or_default()
                .push(imp.source_specifier.clone()),
        }
    }

    let mut issues = Vec::new();
    for pure in pure_files {
        let start_key = path_key(&pure.file);
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        queue.push_back(start_key.clone());
        visited.insert(start_key);

        let mut culprit: Option<&'static str> = None;
        while let Some(key) = queue.pop_front() {
            if let Some(specifiers) = external_edges.get(&key) {
                for spec in specifiers {
                    if let Some(name) = side_effecting_match(spec) {
                        culprit = Some(name);
                        break;
                    }
                }
            }
            if culprit.is_some() {
                break;
            }
            if let Some(next_files) = internal_edges.get(&key) {
                for next in next_files {
                    let next_key = path_key(next);
                    if visited.insert(next_key.clone()) {
                        queue.push_back(next_key);
                    }
                }
            }
        }

        if let Some(name) = culprit {
            issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "declared `@pure`, but transitively imports `{name}`, a known side-effecting module"
                ),
                file: pure.file.clone(),
                location: Location::from_span(&pure.file, pure.span),
                priority: Priority(META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        }
    }

    issues.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.location.start_byte().cmp(&b.location.start_byte()))
    });
    issues
}
