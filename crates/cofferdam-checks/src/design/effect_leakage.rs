use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use oxc_ast::ast::{
    Declaration, ExportDefaultDeclarationKind, Expression, ImportDeclarationSpecifier, Statement,
};
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;

use cofferdam_core::graph::{ImportRecord, IMPORTS as GRAPH_IMPORTS};
use cofferdam_core::{path_key, span_from_bytes};
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext, Issue, Location,
    OptionDefault, OptionKind, OptionSpec, Priority, Severity, SourceFile, Span,
};

/// Node.js built-ins and common database/queue client packages treated as
/// inherently side-effecting. Matched against the import specifier with
/// any `node:` prefix stripped, so `import "node:fs"` and `import "fs"`
/// are equivalent. A subpath of a listed module (`mysql2/promise`) also
/// matches (CD-138).
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

const EL_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "extra_side_effect_modules",
    kind: OptionKind::StringList,
    default: OptionDefault::StringList(&[]),
    doc: "Additional module specifiers (beyond the built-in Node/database/queue denylist) \
        treated as side-effecting. Matched the same way as the built-in list: exact specifier, \
        `node:`-prefix-stripped, or a subpath of a listed module.",
}];

/// One file that opted into a whole-file purity contract via a leading
/// `@pure` JSDoc tag (a comment attached to the file's very first
/// statement — see `run()`).
#[derive(Clone)]
struct PureFile {
    file: PathBuf,
    span: Span,
}

/// One function whose own `@pure` JSDoc tag is attached specifically to
/// it (CD-138) — as opposed to a whole-file leading tag. Only the
/// specifiers this function's body actually references count toward its
/// purity check, so an unrelated side-effecting import elsewhere in the
/// same file is never a false positive for this function.
#[derive(Clone)]
struct PureFunction {
    file: PathBuf,
    span: Span,
    /// Import specifiers (verbatim, as written in this file's own import
    /// declarations) referenced by an identifier used inside the
    /// function's body.
    used_specifiers: Vec<String>,
}

/// Per-process slots: every file/function this check saw carrying a
/// `@pure` tag. Populated in `run()`, consumed in `finalize()` alongside
/// the shared import graph.
static PURE_FILES: CorpusKey<Vec<PureFile>> = CorpusKey::new("Design.EffectLeakage.pure_files");
static PURE_FUNCTIONS: CorpusKey<Vec<PureFunction>> =
    CorpusKey::new("Design.EffectLeakage.pure_functions");

const META: CheckMeta = CheckMeta {
    id: "Design.EffectLeakage",
    category: Category::Design,
    base_priority: 8,
    default_severity: Severity::Medium,
    explanation: "A module (or a specific function within one) opted into a `@pure` contract, \
        but transitively imports a known side-effecting module (filesystem, network, a database \
        client) somewhere down its import chain — the annotation is making a promise the code \
        doesn't keep.",
    body: include_str!("../../docs/Design.EffectLeakage.md"),
    requires_types: false,
    consistency: false,
    options: EL_OPTIONS,
    autofix: false,
    // Writes into PURE_FILES/PURE_FUNCTIONS during run(); skipping on
    // cache hit would drop that file's contribution and silently
    // under-report in finalize(), mirroring Design.DuplicateExportName.
    pure_run: false,
};

/// `Design.EffectLeakage` — cross-file check (CD-127, function-level
/// granularity added CD-138) that flags a `@pure`-annotated module or
/// function whose transitive import chain reaches a known side-effecting
/// module.
///
/// Scope: a `@pure` tag counts as a whole-file contract only when it's
/// attached to the file's very first statement (a genuine file-header
/// comment) — in that case every import anywhere in the file feeds the
/// transitive walk, exactly as before CD-138. A `@pure` tag attached to a
/// specific, later function declaration instead scopes the check to that
/// function alone: only import specifiers actually referenced by an
/// identifier inside that function's own body seed the transitive walk
/// (once the walk crosses into another file, that downstream file is
/// still checked as a whole, since usage isn't tracked past the first
/// hop). A `@pure` tag that is neither a file header nor attached to a
/// function-like declaration (e.g. attached to a class, or to a
/// multi-declarator `const` statement) has no effect. The side-effecting
/// module list is a fixed denylist plus an optional user-supplied
/// `extra_side_effect_modules` addition; matching allows a subpath of a
/// listed module (`mysql2/promise` matches `mysql2`).
pub struct EffectLeakage;

impl Check for EffectLeakage {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&PURE_FILES, |slot, path| slot.retain(|p| p.file != path));
        corpus.register_removable(&PURE_FUNCTIONS, |slot, path| {
            slot.retain(|p| p.file != path)
        });
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let program = parsed.program;
        // A leading directive (`"use strict";`) lives in `program.directives`,
        // not `program.body` — without this, a `@pure` comment attached to
        // such a directive would fail the whole-file check (since
        // `first_stmt_start` would point past it, at the first real
        // statement) and silently be dropped instead of read as a file
        // header.
        let first_stmt_start = program
            .directives
            .first()
            .map(|d| d.span.start)
            .or_else(|| program.body.first().map(|s| s.span().start));

        let mut pure_file: Option<PureFile> = None;
        let mut pure_functions: Vec<PureFunction> = Vec::new();
        let mut local_to_specifier: Option<HashMap<String, String>> = None;

        for comment in &program.comments {
            let Some(text) = file
                .text
                .get(comment.span.start as usize..comment.span.end as usize)
            else {
                continue;
            };
            if !has_pure_tag(text) {
                continue;
            }
            let span = span_from_bytes(&file.text, comment.span.start, comment.span.end);

            if pure_file.is_none() && Some(comment.attached_to) == first_stmt_start {
                pure_file = Some(PureFile {
                    file: file.path.clone(),
                    span,
                });
                continue;
            }

            let Some(stmt) = program
                .body
                .iter()
                .find(|s| s.span().start == comment.attached_to)
            else {
                continue;
            };
            let Some(body) = function_like_body(stmt) else {
                continue;
            };
            let local_to_specifier =
                local_to_specifier.get_or_insert_with(|| build_local_to_specifier(&program.body));
            let referenced = referenced_identifiers(body);
            let mut used_specifiers: Vec<String> = local_to_specifier
                .iter()
                .filter(|(local, _)| referenced.contains(local.as_str()))
                .map(|(_, specifier)| specifier.clone())
                .collect();
            used_specifiers.sort();
            used_specifiers.dedup();
            pure_functions.push(PureFunction {
                file: file.path.clone(),
                span,
                used_specifiers,
            });
        }

        if let Some(pure) = pure_file {
            ctx.corpus.with_slot(&PURE_FILES, |slot| slot.push(pure));
        }
        if !pure_functions.is_empty() {
            ctx.corpus
                .with_slot(&PURE_FUNCTIONS, |slot| slot.extend(pure_functions));
        }
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let pure_files: Vec<PureFile> = ctx.corpus.with_slot(&PURE_FILES, |slot| slot.clone());
        let pure_functions: Vec<PureFunction> =
            ctx.corpus.with_slot(&PURE_FUNCTIONS, |slot| slot.clone());
        if pure_files.is_empty() && pure_functions.is_empty() {
            return Vec::new();
        }
        let extra_modules: Vec<String> = ctx
            .options
            .get_string_list("extra_side_effect_modules")
            .map(|xs| xs.to_vec())
            .unwrap_or_default();
        let imports: Vec<ImportRecord> = ctx
            .corpus
            .with_slot(&GRAPH_IMPORTS, |slot| slot.records().cloned().collect());

        let mut issues = compute_leaks(&pure_files, &imports, &extra_modules);
        issues.extend(compute_function_leaks(
            &pure_functions,
            &imports,
            &extra_modules,
        ));
        issues.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.location.start_byte().cmp(&b.location.start_byte()))
        });
        issues
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
/// stripping a `node:` prefix. Matches an exact module name or a subpath
/// of one (`mysql2/promise` matches the listed `mysql2`).
pub(crate) fn side_effecting_match(specifier: &str, extra_modules: &[String]) -> Option<String> {
    let normalized = specifier.strip_prefix("node:").unwrap_or(specifier);
    let is_match = |m: &str| normalized == m || normalized.starts_with(&format!("{m}/"));
    SIDE_EFFECTING_MODULES
        .iter()
        .find(|&&m| is_match(m))
        .map(|m| (*m).to_string())
        .or_else(|| extra_modules.iter().find(|m| is_match(m)).cloned())
}

/// The function body to scan for identifier references, if `stmt` is a
/// function-like top-level declaration this check can attach a
/// per-function `@pure` tag to: a plain or exported function declaration,
/// or a `const`/`let` declarator (single-declarator only) initialized to
/// an arrow function or function expression. `None` for anything else
/// (classes, multi-declarator statements, non-function expressions) —
/// those are an accepted no-op scope gap (see `EffectLeakage` doc
/// comment).
fn function_like_body<'a, 'b>(stmt: &'b Statement<'a>) -> Option<&'b oxc_ast::ast::FunctionBody<'a>>
where
    'a: 'b,
{
    match stmt {
        Statement::FunctionDeclaration(f) => f.body.as_deref(),
        Statement::ExportNamedDeclaration(decl) => match decl.declaration.as_ref()? {
            Declaration::FunctionDeclaration(f) => f.body.as_deref(),
            Declaration::VariableDeclaration(vd) => variable_declaration_body(vd),
            _ => None,
        },
        Statement::ExportDefaultDeclaration(decl) => match &decl.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(f) => f.body.as_deref(),
            ExportDefaultDeclarationKind::ArrowFunctionExpression(a) => Some(&a.body),
            _ => None,
        },
        Statement::VariableDeclaration(vd) => variable_declaration_body(vd),
        _ => None,
    }
}

fn variable_declaration_body<'a, 'b>(
    vd: &'b oxc_ast::ast::VariableDeclaration<'a>,
) -> Option<&'b oxc_ast::ast::FunctionBody<'a>>
where
    'a: 'b,
{
    if vd.declarations.len() != 1 {
        return None;
    }
    match vd.declarations[0].init.as_ref()? {
        Expression::ArrowFunctionExpression(a) => Some(&a.body),
        Expression::FunctionExpression(f) => f.body.as_deref(),
        _ => None,
    }
}

/// Every distinct identifier referenced anywhere inside `body`.
fn referenced_identifiers(body: &oxc_ast::ast::FunctionBody<'_>) -> HashSet<String> {
    struct IdentCollector {
        names: HashSet<String>,
    }
    impl<'a> Visit<'a> for IdentCollector {
        fn visit_identifier_reference(&mut self, it: &oxc_ast::ast::IdentifierReference<'a>) {
            self.names.insert(it.name.to_string());
        }
        // JSX element/member names (`<Foo/>`, `<Foo.Bar/>`) are
        // `JSXIdentifier`, not `IdentifierReference` — without this, a
        // `@pure` function in a `.tsx` file that renders an imported
        // component wouldn't attribute that import to its own usage.
        fn visit_jsx_identifier(&mut self, it: &oxc_ast::ast::JSXIdentifier<'a>) {
            self.names.insert(it.name.to_string());
        }
    }
    let mut collector = IdentCollector {
        names: HashSet::new(),
    };
    collector.visit_function_body(body);
    collector.names
}

/// Maps each locally-bound import name in this file's own top-level
/// `import` declarations to its source specifier (e.g. `readFile` ->
/// `"fs/promises"`). Built directly from this file's AST, independent of
/// the (possibly not-yet-resolved) project-wide import graph.
fn build_local_to_specifier(body: &[Statement<'_>]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for stmt in body {
        let Statement::ImportDeclaration(decl) = stmt else {
            continue;
        };
        let Some(specifiers) = &decl.specifiers else {
            continue;
        };
        let specifier = decl.source.value.to_string();
        for spec in specifiers {
            let local_name = match spec {
                ImportDeclarationSpecifier::ImportSpecifier(s) => s.local.name.to_string(),
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => s.local.name.to_string(),
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => s.local.name.to_string(),
            };
            map.insert(local_name, specifier.clone());
        }
    }
    map
}

/// Continues a transitive BFS from an already-seeded `queue`/`visited`
/// set, using the project-wide file-level import edges. Returns the
/// first side-effecting module name reached, if any.
fn expand_bfs(
    mut queue: VecDeque<String>,
    mut visited: HashSet<String>,
    external_edges: &HashMap<String, Vec<String>>,
    internal_edges: &HashMap<String, Vec<PathBuf>>,
    extra_modules: &[String],
) -> Option<String> {
    while let Some(key) = queue.pop_front() {
        if let Some(specifiers) = external_edges.get(&key) {
            for spec in specifiers {
                if let Some(name) = side_effecting_match(spec, extra_modules) {
                    return Some(name);
                }
            }
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
    None
}

fn build_edges(
    imports: &[ImportRecord],
) -> (HashMap<String, Vec<String>>, HashMap<String, Vec<PathBuf>>) {
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
    (external_edges, internal_edges)
}

fn compute_leaks(
    pure_files: &[PureFile],
    imports: &[ImportRecord],
    extra_modules: &[String],
) -> Vec<Issue> {
    let (external_edges, internal_edges) = build_edges(imports);

    let mut issues = Vec::new();
    for pure in pure_files {
        let start_key = path_key(&pure.file);
        let mut visited = HashSet::new();
        visited.insert(start_key.clone());
        let mut queue = VecDeque::new();
        queue.push_back(start_key);

        if let Some(name) = expand_bfs(
            queue,
            visited,
            &external_edges,
            &internal_edges,
            extra_modules,
        ) {
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
    issues
}

/// Same transitive-leak detection as `compute_leaks`, but seeded only
/// from the import specifiers a specific `@pure`-tagged function's body
/// actually references (CD-138), rather than every import in its file.
fn compute_function_leaks(
    pure_functions: &[PureFunction],
    imports: &[ImportRecord],
    extra_modules: &[String],
) -> Vec<Issue> {
    let (external_edges, internal_edges) = build_edges(imports);
    let mut resolved_by_spec: HashMap<(String, String), Option<PathBuf>> = HashMap::new();
    for imp in imports {
        resolved_by_spec.insert(
            (path_key(&imp.from_file), imp.source_specifier.clone()),
            imp.resolved.clone(),
        );
    }

    let mut issues = Vec::new();
    for pf in pure_functions {
        let file_key = path_key(&pf.file);
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(file_key.clone());
        let mut queue: VecDeque<String> = VecDeque::new();
        let mut culprit: Option<String> = None;

        for spec in &pf.used_specifiers {
            if let Some(name) = side_effecting_match(spec, extra_modules) {
                culprit = Some(name);
                break;
            }
            if let Some(Some(resolved)) = resolved_by_spec.get(&(file_key.clone(), spec.clone())) {
                let key = path_key(resolved);
                if visited.insert(key.clone()) {
                    queue.push_back(key);
                }
            }
        }
        if culprit.is_none() {
            culprit = expand_bfs(
                queue,
                visited,
                &external_edges,
                &internal_edges,
                extra_modules,
            );
        }

        if let Some(name) = culprit {
            issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!(
                    "declared `@pure`, but this function transitively imports `{name}`, a known \
                     side-effecting module"
                ),
                file: pf.file.clone(),
                location: Location::from_span(&pf.file, pf.span),
                priority: Priority(META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
            });
        }
    }
    issues
}
