//! Evaluator for the v1 predicate DSL.
//!
//! Walks an already-parsed [`ast::TopPredicate`] against a flat per-file
//! corpus view and returns a boolean verdict (or a string for operand
//! sub-expressions).
//!
//! # Scope
//!
//! This is checkpoint 3 of 5.  The evaluator is deliberately standalone:
//! it takes a plain [`EvalCtx`] struct, NOT a [`crate::check::CheckContext`].
//! Engine wiring and `Design.ScriptedInvariant` are checkpoint 4.
//!
//! # AST-extension judgement calls (documented per the checkpoint-2 convention)
//!
//! 1. **Implicit `file` subject** — `Identifier("file")` is handled by the
//!    `"file"` arm of `eval_subject_string`, returning the project-relative
//!    path string.  No special-casing beyond what the AST already represents.
//!
//! 2. **`Predicate::Operand` outside `exists(...)`** — the grammar allows a
//!    bare `Operand` node at predicate level (it arises when the parser sees a
//!    string in a position that expects a boolean).  The evaluator returns
//!    `TypeMismatch { expected: "boolean", got: "string" }` for this shape so
//!    callers get a clear error instead of silent misbehaviour.
//!
//! 3. **Bare `file` as `Call { args: [] }`** — when a `Call` node appears
//!    inside a function's arg list with a name that is in
//!    [`ast::KNOWN_BARE_SUBJECTS`] and no arguments, it is treated as a
//!    subject reference returning the file-path string.  This handles the
//!    common `basename(file)` / `dirname(file)` / `exists(file)` pattern.
//!
//! 4. **`core.*` / `ts.*` namespace calls** — v1 has no resolver for these.
//!    `Subject::Call(Call { name: "core.X" / "ts.X", ... })` always returns
//!    [`DslEvalError::UnsupportedNamespaceMember`].  Checkpoint 4 may wire
//!    some in; see the bead for the planned surface.
//!
//! # `TransitivelyImports`
//!
//! `transitively imports` walks the whole import graph, which the caller
//! supplies via [`EvalCtx::imports_by_file`]. A caller that leaves that
//! field `None` gets the direct-edge answer — the only one available
//! without a graph.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

use super::ast::{self, Operand, Predicate, Subject, TopPredicate, KNOWN_BARE_SUBJECTS};
use crate::graph::{ExportRecord, ImportRecord, LayersConfig};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Pre-compiled glob pattern cache.
///
/// Create one instance per analysis pass and attach it to every [`EvalCtx`] so
/// identical patterns are compiled at most once across all file evaluations.
pub struct GlobCache(RefCell<HashMap<String, GlobSet>>);

impl GlobCache {
    pub fn new() -> Self {
        GlobCache(RefCell::new(HashMap::new()))
    }

    fn is_match(&self, pattern: &str, path: &str) -> Result<bool, DslEvalError> {
        let mut map = self.0.borrow_mut();
        if let Some(gs) = map.get(pattern) {
            return Ok(gs.is_match(path));
        }
        let gs = build_globset_single(pattern)?;
        let result = gs.is_match(path);
        map.insert(pattern.to_string(), gs);
        Ok(result)
    }
}

impl Default for GlobCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-file evaluation context.  The caller is responsible for filtering
/// the corpus slices to only the records that belong to `file_path`.
pub struct EvalCtx<'a> {
    /// Absolute path of the file currently being evaluated.
    pub file_path: &'a Path,
    /// Absolute path of the project root (used to relativise globs and
    /// resolve relative paths for `exists(...)`).
    pub project_root: &'a Path,
    /// All import records that originated from `file_path`.
    pub imports: &'a [ImportRecord],
    /// All export records that originated from `file_path`.
    pub exports: &'a [ExportRecord],
    /// Every file's import records, keyed by the importing file. Only
    /// `transitively imports` reads this, to walk past the first hop.
    /// `None` collapses that operator onto direct edges, which is what a
    /// caller with no project graph can honestly answer.
    pub imports_by_file: Option<&'a HashMap<PathBuf, Vec<ImportRecord>>>,
    /// Layer configuration for the project, when available.
    pub layers: Option<&'a LayersConfig>,
    /// Shared glob compilation cache; lives for the duration of one analysis pass.
    pub glob_cache: &'a GlobCache,
}

/// Errors that can arise during DSL evaluation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DslEvalError {
    #[error("type mismatch: expected {expected} for `{context}`, got {got}")]
    TypeMismatch {
        expected: &'static str,
        got: &'static str,
        context: String,
    },
    #[error("unknown subject `{name}`; known bare subjects: {known:?}")]
    UnknownSubject { name: String, known: Vec<String> },
    #[error("unknown function `{name}`; known: {known:?}")]
    UnknownFunction { name: String, known: Vec<String> },
    #[error(
        "unsupported namespace member `{ns}.{member}` — v1 does not yet resolve this namespace"
    )]
    UnsupportedNamespaceMember { ns: String, member: String },
    #[error("wrong argument count for `{name}`: expected {expected}, got {got}")]
    ArityMismatch {
        name: String,
        expected: usize,
        got: usize,
    },
    #[error("operator `{op}` is not yet implemented in v1 (reserved for v2 graph)")]
    NotYetImplemented { op: &'static str },
    #[error("invalid glob pattern `{pattern}`: {reason}")]
    BadGlob { pattern: String, reason: String },
}

/// Evaluate a top-level predicate (`forbid` / `require` / bare) against the
/// given file context.
///
/// Returns `true` when the predicate passes (i.e. no violation), `false` when
/// it fails.  For `forbid p` that means `!eval(p)` — if the inner predicate
/// matched, the rule is violated.
pub fn eval_top(top: &TopPredicate, ctx: &EvalCtx<'_>) -> Result<bool, DslEvalError> {
    match top {
        TopPredicate::Forbid(inner) => Ok(!eval_predicate(inner, ctx)?),
        TopPredicate::Require(inner) => eval_predicate(inner, ctx),
        TopPredicate::Bare(inner) => eval_predicate(inner, ctx),
    }
}

/// Evaluate a boolean predicate.
pub fn eval_predicate(pred: &Predicate, ctx: &EvalCtx<'_>) -> Result<bool, DslEvalError> {
    match pred {
        Predicate::Or(lhs, rhs) => {
            if eval_predicate(lhs, ctx)? {
                Ok(true)
            } else {
                eval_predicate(rhs, ctx)
            }
        }
        Predicate::And(lhs, rhs) => {
            if !eval_predicate(lhs, ctx)? {
                Ok(false)
            } else {
                eval_predicate(rhs, ctx)
            }
        }
        Predicate::Not(inner) => Ok(!eval_predicate(inner, ctx)?),
        Predicate::Paren(inner) => eval_predicate(inner, ctx),
        Predicate::Comparison(cmp) => eval_comparison(cmp, ctx),
        Predicate::Call(call) => eval_call_as_bool(call, ctx),
        Predicate::Operand(_) => Err(DslEvalError::TypeMismatch {
            expected: "boolean",
            got: "string",
            context: "bare operand used as predicate".to_string(),
        }),
    }
}

/// Evaluate an operand expression to its string value.
pub(crate) fn eval_operand(op: &Operand, ctx: &EvalCtx<'_>) -> Result<String, DslEvalError> {
    match op {
        Operand::String(s) => Ok(s.clone()),
        Operand::Call(call) => eval_call_as_string(call, ctx),
        Operand::Concat(lhs, rhs) => {
            let l = eval_operand(lhs, ctx)?;
            let r = eval_operand(rhs, ctx)?;
            Ok(l + &r)
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: comparison dispatch
// ---------------------------------------------------------------------------

fn eval_comparison(cmp: &ast::Comparison, ctx: &EvalCtx<'_>) -> Result<bool, DslEvalError> {
    use ast::Op;

    match &cmp.op {
        Op::Matches => {
            let subject_str = eval_subject_string(&cmp.subject, ctx)?;
            let pattern = eval_operand(&cmp.operand, ctx)?;
            glob_matches_path(&pattern, &subject_str, ctx.glob_cache)
        }

        Op::Eq => {
            let subject_str = eval_subject_string(&cmp.subject, ctx)?;
            let operand_str = eval_operand(&cmp.operand, ctx)?;
            Ok(subject_str == operand_str)
        }

        Op::Ne => {
            let subject_str = eval_subject_string_nullable(&cmp.subject, ctx)?;
            let operand_str = eval_operand(&cmp.operand, ctx)?;
            match subject_str {
                Some(s) => Ok(s != operand_str),
                // A layer-less file is not equal to any layer, so != is true.
                None => Ok(true),
            }
        }

        Op::In => {
            // `file in '<layer>'` — true iff the file's layer equals the operand.
            let layer_name = eval_operand(&cmp.operand, ctx)?;
            Ok(compute_layer(ctx) == Some(layer_name))
        }

        Op::Imports => {
            let pattern = eval_operand(&cmp.operand, ctx)?;
            Ok(imports_any(ctx, &pattern))
        }

        Op::TransitivelyImports => {
            let pattern = eval_operand(&cmp.operand, ctx)?;
            Ok(transitively_imports(ctx, &pattern))
        }

        Op::ImportsAsType => {
            let pattern = eval_operand(&cmp.operand, ctx)?;
            Ok(imports_as_type(ctx, &pattern))
        }

        Op::ImportsAsValue => {
            let pattern = eval_operand(&cmp.operand, ctx)?;
            Ok(imports_as_value(ctx, &pattern))
        }

        Op::Exports => {
            let pattern = eval_operand(&cmp.operand, ctx)?;
            Ok(exports_any(ctx, &pattern))
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: subject resolution
// ---------------------------------------------------------------------------

/// Resolve a subject to its string representation.
/// Returns an error when the subject has no string form (e.g. namespace calls).
fn eval_subject_string(subject: &Subject, ctx: &EvalCtx<'_>) -> Result<String, DslEvalError> {
    match eval_subject_string_nullable(subject, ctx)? {
        Some(s) => Ok(s),
        None => {
            // Only `file.layer` can return None; for Eq, None means "no layer"
            // which evaluates to false.  But we need a string to compare.
            // Return an empty string — the comparison will be false since
            // no operand will be "".  (The Ne branch handles None directly.)
            Ok(String::new())
        }
    }
}

/// Like `eval_subject_string` but returns `None` when the subject genuinely
/// has no value (e.g. `file.layer` on a file with no assigned layer).
fn eval_subject_string_nullable(
    subject: &Subject,
    ctx: &EvalCtx<'_>,
) -> Result<Option<String>, DslEvalError> {
    match subject {
        Subject::Identifier(name) => match name.as_str() {
            "file" | "file.path" => {
                let rel = relative_path(ctx.file_path, ctx.project_root);
                Ok(Some(rel))
            }
            "file.layer" => Ok(compute_layer(ctx)),
            other => Err(DslEvalError::UnknownSubject {
                name: other.to_string(),
                known: KNOWN_BARE_SUBJECTS.iter().map(|s| s.to_string()).collect(),
            }),
        },
        Subject::Call(call) => {
            // v1 does not resolve core.* / ts.* — emit UnsupportedNamespaceMember.
            let (ns, member) = split_ns(&call.name);
            Err(DslEvalError::UnsupportedNamespaceMember {
                ns: ns.to_string(),
                member: member.to_string(),
            })
        }
        Subject::Paren(inner) => {
            // A parenthesised sub-predicate used as a subject is unusual in
            // practice, but the grammar admits it.  We have no way to turn a
            // bool into a string here — emit a type error.
            let _ = inner;
            Err(DslEvalError::TypeMismatch {
                expected: "string subject",
                got: "boolean predicate",
                context: "parenthesised predicate used as comparison subject".to_string(),
            })
        }
    }
}

/// Split "core.symbol" → ("core", "symbol").  Falls back to ("", name).
fn split_ns(name: &str) -> (&str, &str) {
    if let Some(dot) = name.find('.') {
        (&name[..dot], &name[dot + 1..])
    } else {
        ("", name)
    }
}

// ---------------------------------------------------------------------------
// Internal: call dispatch
// ---------------------------------------------------------------------------

/// Evaluate a `Call` node that should return a boolean (i.e. `exists`).
fn eval_call_as_bool(call: &ast::Call, ctx: &EvalCtx<'_>) -> Result<bool, DslEvalError> {
    match call.name.as_str() {
        "exists" => {
            if call.args.len() != 1 {
                return Err(DslEvalError::ArityMismatch {
                    name: "exists".to_string(),
                    expected: 1,
                    got: call.args.len(),
                });
            }
            let arg_str = eval_predicate_as_string(&call.args[0], ctx)?;
            let path = Path::new(&arg_str);
            if path.is_absolute() {
                Ok(path.exists())
            } else {
                Ok(ctx.project_root.join(path).exists())
            }
        }
        "basename" | "dirname" => {
            // These return strings, not booleans.
            Err(DslEvalError::TypeMismatch {
                expected: "boolean",
                got: "string",
                context: format!(
                    "`{}` returns a string and cannot be used as a boolean predicate",
                    call.name
                ),
            })
        }
        other => {
            // Check for namespace calls (core.*, ts.*).
            if other.contains('.') {
                let (ns, member) = split_ns(other);
                Err(DslEvalError::UnsupportedNamespaceMember {
                    ns: ns.to_string(),
                    member: member.to_string(),
                })
            } else {
                Err(DslEvalError::UnknownFunction {
                    name: other.to_string(),
                    known: ast::KNOWN_FUNCTIONS.iter().map(|s| s.to_string()).collect(),
                })
            }
        }
    }
}

/// Evaluate a `Call` node that should return a string (i.e. `basename`, `dirname`).
fn eval_call_as_string(call: &ast::Call, ctx: &EvalCtx<'_>) -> Result<String, DslEvalError> {
    // Bare subject references appearing as Call nodes (e.g. `file` in
    // `basename(file)` is parsed as `Call { name: "file", args: [] }`).
    if KNOWN_BARE_SUBJECTS.contains(&call.name.as_str()) && call.args.is_empty() {
        return eval_bare_subject_as_string(&call.name, ctx);
    }

    match call.name.as_str() {
        "basename" => {
            if call.args.len() != 1 {
                return Err(DslEvalError::ArityMismatch {
                    name: "basename".to_string(),
                    expected: 1,
                    got: call.args.len(),
                });
            }
            let s = eval_predicate_as_string(&call.args[0], ctx)?;
            Ok(Path::new(&s)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default())
        }
        "dirname" => {
            if call.args.len() != 1 {
                return Err(DslEvalError::ArityMismatch {
                    name: "dirname".to_string(),
                    expected: 1,
                    got: call.args.len(),
                });
            }
            let s = eval_predicate_as_string(&call.args[0], ctx)?;
            Ok(Path::new(&s)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default())
        }
        "exists" => {
            // `exists` returns bool, not string — type error.
            Err(DslEvalError::TypeMismatch {
                expected: "string",
                got: "boolean",
                context: "`exists` returns bool and cannot be used as a string operand".to_string(),
            })
        }
        other => {
            if other.contains('.') {
                let (ns, member) = split_ns(other);
                Err(DslEvalError::UnsupportedNamespaceMember {
                    ns: ns.to_string(),
                    member: member.to_string(),
                })
            } else {
                Err(DslEvalError::UnknownFunction {
                    name: other.to_string(),
                    known: ast::KNOWN_FUNCTIONS.iter().map(|s| s.to_string()).collect(),
                })
            }
        }
    }
}

/// Evaluate a bare subject identifier as its string value.
fn eval_bare_subject_as_string(name: &str, ctx: &EvalCtx<'_>) -> Result<String, DslEvalError> {
    match name {
        "file" | "file.path" => Ok(relative_path(ctx.file_path, ctx.project_root)),
        "file.layer" => Ok(compute_layer(ctx).unwrap_or_default()),
        other => Err(DslEvalError::UnknownSubject {
            name: other.to_string(),
            known: KNOWN_BARE_SUBJECTS.iter().map(|s| s.to_string()).collect(),
        }),
    }
}

/// Coerce a `Predicate` node to a string.  Used for call arguments where the
/// grammar says `predicate` but the semantic is a string operand (e.g.
/// `exists('tests/' + basename(file) + '.ts')`).
fn eval_predicate_as_string(pred: &Predicate, ctx: &EvalCtx<'_>) -> Result<String, DslEvalError> {
    match pred {
        // A bare Operand wrapper — evaluate directly.
        Predicate::Operand(op) => eval_operand(op, ctx),
        // A Call at predicate level that actually yields a string.
        Predicate::Call(call) => eval_call_as_string(call, ctx),
        // A Comparison whose subject resolves to a string (e.g.
        // `file == 'x'` used as an arg — degenerate but allowed by the
        // grammar; we just pull the subject string out).
        Predicate::Comparison(cmp) => eval_subject_string(&cmp.subject, ctx),
        // For all other shapes, attempt to get a string from a subject
        // identifier alone (covers `basename(file)` where `file` appears as
        // `Predicate::Call { name: "file", args: [] }` — handled above, but
        // belt-and-braces).
        other => Err(DslEvalError::TypeMismatch {
            expected: "string",
            got: "boolean",
            context: format!("predicate node `{other:?}` used as string argument"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Internal: helpers
// ---------------------------------------------------------------------------

/// Return the project-relative path of `abs` as a forward-slash string.
/// Falls back to the absolute path if stripping the prefix fails.
fn relative_path(abs: &Path, root: &Path) -> String {
    abs.strip_prefix(root)
        .unwrap_or(abs)
        .to_string_lossy()
        // Normalise Windows separators so glob patterns don't need to care.
        .replace('\\', "/")
}

/// Compute which layer `ctx.file_path` belongs to, according to `LayersConfig`.
/// First match wins.  Returns `None` when layers are not configured or the
/// file matches no layer glob.
fn compute_layer(ctx: &EvalCtx<'_>) -> Option<String> {
    let layers = ctx.layers?;
    let rel = relative_path(ctx.file_path, &layers.project_root);
    for (layer_name, globs) in &layers.layers {
        for pattern in globs {
            if ctx.glob_cache.is_match(pattern, &rel).unwrap_or(false) {
                return Some(layer_name.clone());
            }
        }
    }
    None
}

/// Build a `GlobSet` from a slice of gitignore-style glob patterns.
fn build_globset(patterns: &[String]) -> Result<GlobSet, DslEvalError> {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p).map_err(|e| DslEvalError::BadGlob {
            pattern: p.clone(),
            reason: e.to_string(),
        })?;
        builder.add(g);
    }
    builder.build().map_err(|e| DslEvalError::BadGlob {
        pattern: patterns.join(", "),
        reason: e.to_string(),
    })
}

/// Build a `GlobSet` from a single pattern string (convenience wrapper).
fn build_globset_single(pattern: &str) -> Result<GlobSet, DslEvalError> {
    build_globset(&[pattern.to_string()])
}

/// True if the given string `pattern` contains glob metacharacters.
fn is_glob(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?') || pattern.contains('[')
}

/// Match `path_str` against a gitignore-style glob pattern.
fn glob_matches_path(
    pattern: &str,
    path_str: &str,
    cache: &GlobCache,
) -> Result<bool, DslEvalError> {
    // Normalise Windows separators in the path being matched.
    let normalized = path_str.replace('\\', "/");
    cache.is_match(pattern, &normalized)
}

/// True iff any import record's specifier (or resolved path) matches `pattern`.
fn imports_any(ctx: &EvalCtx<'_>, pattern: &str) -> bool {
    let pattern_norm = pattern.replace('\\', "/");
    ctx.imports.iter().any(|imp| {
        specifier_matches(&imp.source_specifier, &pattern_norm, ctx.glob_cache)
            || imp
                .resolved
                .as_ref()
                .is_some_and(|r| path_matches(r, &pattern_norm, ctx.glob_cache))
    })
}

/// Breadth-first walk of the import graph from `ctx.file_path`, true as
/// soon as any edge on the way matches `pattern`.
///
/// Only edges that resolved to a file on disk extend the walk: an
/// unresolved specifier such as `react` can be matched but not followed,
/// because there is nothing to follow it to. Cycles are handled by the
/// visited set, so `a -> b -> a` terminates.
///
/// With no project graph attached this degrades to the direct-edge
/// answer rather than reporting a closure it cannot compute.
fn transitively_imports(ctx: &EvalCtx<'_>, pattern: &str) -> bool {
    let pattern_norm = pattern.replace('\\', "/");
    let matches = |imp: &ImportRecord| {
        specifier_matches(&imp.source_specifier, &pattern_norm, ctx.glob_cache)
            || imp
                .resolved
                .as_ref()
                .is_some_and(|r| path_matches(r, &pattern_norm, ctx.glob_cache))
    };

    let Some(graph) = ctx.imports_by_file else {
        return ctx.imports.iter().any(matches);
    };

    let mut visited: HashSet<PathBuf> = HashSet::from([ctx.file_path.to_path_buf()]);
    let mut queue: VecDeque<&[ImportRecord]> = VecDeque::from([ctx.imports]);
    while let Some(edges) = queue.pop_front() {
        for imp in edges {
            if matches(imp) {
                return true;
            }
            let Some(next) = imp.resolved.as_ref() else {
                continue;
            };
            if !visited.insert(next.clone()) {
                continue;
            }
            if let Some(further) = graph.get(next) {
                queue.push_back(further);
            }
        }
    }
    false
}

/// True iff any import edge has `type_only: true` (at the statement level) or
/// has at least one name with `type_only: true` whose specifier matches.
fn imports_as_type(ctx: &EvalCtx<'_>, pattern: &str) -> bool {
    let pattern_norm = pattern.replace('\\', "/");
    ctx.imports.iter().any(|imp| {
        let specifier_ok = specifier_matches(&imp.source_specifier, &pattern_norm, ctx.glob_cache)
            || imp
                .resolved
                .as_ref()
                .is_some_and(|r| path_matches(r, &pattern_norm, ctx.glob_cache));
        if !specifier_ok {
            return false;
        }
        // Whole-statement type-only.
        if imp.type_only {
            return true;
        }
        // At least one per-name type-only.
        imp.names.iter().any(|n| n.type_only)
    })
}

/// True iff an import edge has at least one value-mode binding (i.e. not
/// entirely type-only at the statement level and has at least one name that
/// is not `type_only`).
fn imports_as_value(ctx: &EvalCtx<'_>, pattern: &str) -> bool {
    let pattern_norm = pattern.replace('\\', "/");
    ctx.imports.iter().any(|imp| {
        let specifier_ok = specifier_matches(&imp.source_specifier, &pattern_norm, ctx.glob_cache)
            || imp
                .resolved
                .as_ref()
                .is_some_and(|r| path_matches(r, &pattern_norm, ctx.glob_cache));
        if !specifier_ok {
            return false;
        }
        // Exclude entirely type-only imports.
        if imp.type_only {
            return false;
        }
        // Side-effect-only import (no names) — value import.
        if imp.names.is_empty() {
            return true;
        }
        // At least one value-mode name.
        imp.names.iter().any(|n| !n.type_only)
    })
}

/// True iff any export record's name matches `pattern`.
fn exports_any(ctx: &EvalCtx<'_>, pattern: &str) -> bool {
    let pattern_norm = pattern.replace('\\', "/");
    ctx.exports
        .iter()
        .any(|exp| specifier_matches(&exp.name, &pattern_norm, ctx.glob_cache))
}

/// Match an import/export specifier string against `pattern`.
/// Uses glob-match when `pattern` contains metacharacters, otherwise
/// plain string equality.
fn specifier_matches(specifier: &str, pattern: &str, cache: &GlobCache) -> bool {
    if is_glob(pattern) {
        cache.is_match(pattern, specifier).unwrap_or(false)
    } else {
        specifier == pattern
    }
}

/// Match a resolved `PathBuf` against `pattern`.
fn path_matches(resolved: &Path, pattern: &str, cache: &GlobCache) -> bool {
    let s = resolved.to_string_lossy().replace('\\', "/");
    specifier_matches(&s, pattern, cache)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::path::{Path, PathBuf};

    use super::{eval_operand, eval_predicate, eval_top, DslEvalError, EvalCtx, GlobCache};
    use crate::dsl::ast::{Operand, Predicate};
    use crate::dsl::parser::{parse_predicate, parse_top};
    use crate::graph::{
        ExportKind, ExportRecord, ImportKind, ImportRecord, ImportedName, LayersConfig,
    };
    use crate::issue::Span;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn dummy_span() -> Span {
        Span {
            start_byte: 0,
            end_byte: 0,
            line: 1,
            column: 0,
        }
    }

    fn make_import(specifier: &str, type_only: bool) -> ImportRecord {
        ImportRecord {
            from_file: PathBuf::new(),
            source_specifier: specifier.to_string(),
            resolved: None,
            names: vec![],
            type_only,
            span: dummy_span(),
        }
    }

    fn make_import_named(
        specifier: &str,
        name: &str,
        type_only_stmt: bool,
        type_only_name: bool,
    ) -> ImportRecord {
        ImportRecord {
            from_file: PathBuf::new(),
            source_specifier: specifier.to_string(),
            resolved: None,
            names: vec![ImportedName {
                source_name: name.to_string(),
                local_name: name.to_string(),
                kind: ImportKind::Named,
                type_only: type_only_name,
                local_use_count: 0,
            }],
            type_only: type_only_stmt,
            span: dummy_span(),
        }
    }

    fn make_export(name: &str) -> ExportRecord {
        ExportRecord {
            file: PathBuf::new(),
            name: name.to_string(),
            kind: ExportKind::Named,
            type_only: false,
            span: dummy_span(),
            source_specifier: None,
            resolved_source: None,
        }
    }

    fn ctx<'a>(
        file_path: &'a Path,
        project_root: &'a Path,
        imports: &'a [ImportRecord],
        exports: &'a [ExportRecord],
        layers: Option<&'a LayersConfig>,
        cache: &'a GlobCache,
    ) -> EvalCtx<'a> {
        EvalCtx {
            file_path,
            project_root,
            imports,
            exports,
            imports_by_file: None,
            layers,
            glob_cache: cache,
        }
    }

    /// `ctx`, plus the project-wide graph `transitively imports` needs.
    fn ctx_with_graph<'a>(
        file_path: &'a Path,
        project_root: &'a Path,
        graph: &'a HashMap<PathBuf, Vec<ImportRecord>>,
        cache: &'a GlobCache,
        empty_exports: &'a [ExportRecord],
    ) -> EvalCtx<'a> {
        static NO_IMPORTS: &[ImportRecord] = &[];
        EvalCtx {
            file_path,
            project_root,
            imports: graph
                .get(file_path)
                .map(Vec::as_slice)
                .unwrap_or(NO_IMPORTS),
            exports: empty_exports,
            imports_by_file: Some(graph),
            layers: None,
            glob_cache: cache,
        }
    }

    fn eval_str(src: &str, c: &EvalCtx<'_>) -> bool {
        let top = parse_top(src).unwrap_or_else(|e| panic!("parse failed for `{src}`: {e}"));
        eval_top(&top, c).unwrap_or_else(|e| panic!("eval failed for `{src}`: {e}"))
    }

    fn eval_pred_str(src: &str, c: &EvalCtx<'_>) -> bool {
        let pred = parse_predicate(src).unwrap_or_else(|e| panic!("parse failed: {e}"));
        eval_predicate(&pred, c).unwrap_or_else(|e| panic!("eval failed: {e}"))
    }

    // -----------------------------------------------------------------------
    // 1. forbid flips
    // -----------------------------------------------------------------------

    #[test]
    fn forbid_flips_true_to_false() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        // The predicate `file matches 'src/**'` is true for this file.
        assert!(eval_pred_str("file matches 'src/**'", &c));
        // `forbid` that predicate → false (violation present, rule says forbid).
        assert!(!eval_str("forbid file matches 'src/**'", &c));
    }

    #[test]
    fn forbid_flips_false_to_true() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        // `file matches 'ui/**'` is false.
        assert!(!eval_pred_str("file matches 'ui/**'", &c));
        // `forbid` that → true (nothing to forbid).
        assert!(eval_str("forbid file matches 'ui/**'", &c));
    }

    // -----------------------------------------------------------------------
    // 2. require is identity
    // -----------------------------------------------------------------------

    #[test]
    fn require_is_identity() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        assert!(eval_str("require file matches 'src/**'", &c));
        assert!(!eval_str("require file matches 'ui/**'", &c));
    }

    // -----------------------------------------------------------------------
    // 3. and/or short-circuit
    // -----------------------------------------------------------------------

    #[test]
    fn or_short_circuits_on_true() {
        // The first branch is true, so the second branch (`exists` on a
        // definitely-missing path) must not be evaluated.  If it were
        // evaluated, it would return false but not error — the test just
        // verifies the result is `true` without needing the right branch.
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        // LHS true → whole OR is true without evaluating RHS.
        assert!(eval_pred_str(
            "file matches 'src/**' or exists('__nonexistent_xyz_12345__')",
            &c
        ));
    }

    #[test]
    fn and_short_circuits_on_false() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        // LHS false → whole AND is false without evaluating RHS.
        assert!(!eval_pred_str(
            "file matches 'ui/**' and exists('__nonexistent_xyz_12345__')",
            &c
        ));
    }

    // -----------------------------------------------------------------------
    // 4. not
    // -----------------------------------------------------------------------

    #[test]
    fn not_inverts() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        assert!(!eval_pred_str("not file matches 'src/**'", &c));
        assert!(eval_pred_str("not file matches 'ui/**'", &c));
    }

    // -----------------------------------------------------------------------
    // 5. file matches glob
    // -----------------------------------------------------------------------

    #[test]
    fn file_matches_glob_hit() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/controllers/user.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        assert!(eval_pred_str("file matches 'src/controllers/**'", &c));
    }

    #[test]
    fn file_matches_glob_miss() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/services/user.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        assert!(!eval_pred_str("file matches 'src/controllers/**'", &c));
    }

    // -----------------------------------------------------------------------
    // 6. file.path ==
    // -----------------------------------------------------------------------

    #[test]
    fn file_path_eq() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/index.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        assert!(eval_pred_str("file.path == 'src/index.ts'", &c));
        assert!(!eval_pred_str("file.path == 'src/other.ts'", &c));
    }

    // -----------------------------------------------------------------------
    // 7. file.layer == with LayersConfig
    // -----------------------------------------------------------------------

    #[test]
    fn file_layer_eq_with_config() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/core/service.ts");

        let mut layers = BTreeMap::new();
        layers.insert("core".to_string(), vec!["src/core/**".to_string()]);
        layers.insert("ui".to_string(), vec!["src/ui/**".to_string()]);

        let lc = LayersConfig {
            project_root: root.clone(),
            layers,
            allow: BTreeMap::new(),
        };

        let c = ctx(&file, &root, &[], &[], Some(&lc), &cache);
        assert!(eval_pred_str("file.layer == 'core'", &c));
        assert!(!eval_pred_str("file.layer == 'ui'", &c));
    }

    // -----------------------------------------------------------------------
    // 8. file.layer == with no LayersConfig
    // -----------------------------------------------------------------------

    #[test]
    fn file_layer_eq_no_config() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/core/service.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        // No layer config → layer is None → never equals any string.
        assert!(!eval_pred_str("file.layer == 'core'", &c));
        // != should be true (a layer-less file is not in any layer).
        assert!(eval_pred_str("file.layer != 'core'", &c));
    }

    // -----------------------------------------------------------------------
    // 9. file in '<layer>'
    // -----------------------------------------------------------------------

    #[test]
    fn file_in_layer_operator() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/ui/Button.tsx");

        let mut layers = BTreeMap::new();
        layers.insert("ui".to_string(), vec!["src/ui/**".to_string()]);

        let lc = LayersConfig {
            project_root: root.clone(),
            layers,
            allow: BTreeMap::new(),
        };

        let c = ctx(&file, &root, &[], &[], Some(&lc), &cache);
        assert!(eval_pred_str("file in 'ui'", &c));
        assert!(!eval_pred_str("file in 'core'", &c));
    }

    // -----------------------------------------------------------------------
    // 10. imports literal hit/miss
    // -----------------------------------------------------------------------

    #[test]
    fn imports_literal_hit() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let imports = vec![make_import("./bar", false)];
        let c = ctx(&file, &root, &imports, &[], None, &cache);
        assert!(eval_pred_str("file imports './bar'", &c));
    }

    #[test]
    fn imports_literal_miss() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let imports = vec![make_import("./baz", false)];
        let c = ctx(&file, &root, &imports, &[], None, &cache);
        assert!(!eval_pred_str("file imports './bar'", &c));
    }

    // -----------------------------------------------------------------------
    // 11. imports glob hit/miss
    // -----------------------------------------------------------------------

    #[test]
    fn imports_glob_hit() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let mut imp = make_import("src/util/strings.ts", false);
        imp.resolved = None; // specifier itself looks like a path
        let imports = vec![imp];
        let c = ctx(&file, &root, &imports, &[], None, &cache);
        assert!(eval_pred_str("file imports 'src/util/*.ts'", &c));
    }

    #[test]
    fn imports_glob_miss() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let imports = vec![make_import("src/models/user.ts", false)];
        let c = ctx(&file, &root, &imports, &[], None, &cache);
        assert!(!eval_pred_str("file imports 'src/util/*.ts'", &c));
    }

    // -----------------------------------------------------------------------
    // 12. transitively imports
    // -----------------------------------------------------------------------

    /// One edge of the graph: `from` imports `to`, resolved on disk.
    fn edge(root: &Path, from: &str, to: &str) -> ImportRecord {
        ImportRecord {
            from_file: root.join(from),
            source_specifier: format!("./{to}"),
            resolved: Some(root.join(to)),
            names: vec![],
            type_only: false,
            span: dummy_span(),
        }
    }

    fn graph_of(edges: Vec<ImportRecord>) -> HashMap<PathBuf, Vec<ImportRecord>> {
        let mut map: HashMap<PathBuf, Vec<ImportRecord>> = HashMap::new();
        for e in edges {
            map.entry(e.from_file.clone()).or_default().push(e);
        }
        map
    }

    #[test]
    fn transitively_imports_reaches_past_the_first_hop() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let graph = graph_of(vec![
            edge(&root, "src/foo.ts", "src/mid.ts"),
            edge(&root, "src/mid.ts", "src/db.ts"),
        ]);
        let file = root.join("src/foo.ts");
        let c = ctx_with_graph(&file, &root, &graph, &cache, &[]);

        assert!(eval_pred_str("file transitively imports './src/db.ts'", &c));
        assert!(
            !eval_pred_str("file imports './src/db.ts'", &c),
            "the direct operator must stay direct — that is the whole distinction"
        );
        assert!(!eval_pred_str(
            "file transitively imports './src/other.ts'",
            &c
        ));
    }

    #[test]
    fn a_cycle_terminates() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let graph = graph_of(vec![
            edge(&root, "src/a.ts", "src/b.ts"),
            edge(&root, "src/b.ts", "src/a.ts"),
        ]);
        let file = root.join("src/a.ts");
        let c = ctx_with_graph(&file, &root, &graph, &cache, &[]);

        assert!(eval_pred_str("file transitively imports './src/b.ts'", &c));
        assert!(!eval_pred_str("file transitively imports './src/z.ts'", &c));
    }

    /// A bare package specifier has nothing on disk to follow, so it can
    /// be matched at the hop that names it but cannot extend the walk.
    #[test]
    fn an_unresolved_specifier_matches_without_extending_the_walk() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let mut react = make_import("react", false);
        react.from_file = root.join("src/mid.ts");
        let graph = graph_of(vec![edge(&root, "src/foo.ts", "src/mid.ts"), react]);
        let file = root.join("src/foo.ts");
        let c = ctx_with_graph(&file, &root, &graph, &cache, &[]);

        assert!(eval_pred_str("file transitively imports 'react'", &c));
    }

    /// Without a graph the operator answers on direct edges only, rather
    /// than claiming a closure it has no way to compute.
    #[test]
    fn no_graph_degrades_to_direct_edges() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let imports = vec![make_import("react", false)];
        let c = ctx(&file, &root, &imports, &[], None, &cache);
        assert!(eval_pred_str("file transitively imports 'react'", &c));
        assert!(!eval_pred_str("file transitively imports 'lodash'", &c));
    }

    // -----------------------------------------------------------------------
    // 13. imports as type
    // -----------------------------------------------------------------------

    #[test]
    fn imports_as_type_fires_on_type_only() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        // Statement-level type import.
        let imports = vec![make_import("react", true)];
        let c = ctx(&file, &root, &imports, &[], None, &cache);
        assert!(eval_pred_str("file imports as type 'react'", &c));
    }

    #[test]
    fn imports_as_type_misses_value_import() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let imports = vec![make_import("react", false)];
        let c = ctx(&file, &root, &imports, &[], None, &cache);
        // No type-only names, statement is value import.
        assert!(!eval_pred_str("file imports as type 'react'", &c));
    }

    #[test]
    fn imports_as_type_per_name() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        // Per-name type-only.
        let imports = vec![make_import_named("react", "FC", false, true)];
        let c = ctx(&file, &root, &imports, &[], None, &cache);
        assert!(eval_pred_str("file imports as type 'react'", &c));
    }

    // -----------------------------------------------------------------------
    // 14. imports as value
    // -----------------------------------------------------------------------

    #[test]
    fn imports_as_value_fires_on_value_import() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let imports = vec![make_import_named("react", "useState", false, false)];
        let c = ctx(&file, &root, &imports, &[], None, &cache);
        assert!(eval_pred_str("file imports as value 'react'", &c));
    }

    #[test]
    fn imports_as_value_misses_type_only() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let imports = vec![make_import("react", true)];
        let c = ctx(&file, &root, &imports, &[], None, &cache);
        assert!(!eval_pred_str("file imports as value 'react'", &c));
    }

    // -----------------------------------------------------------------------
    // 15. exports literal hit/miss
    // -----------------------------------------------------------------------

    #[test]
    fn exports_literal_hit() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let exports = vec![make_export("MyComponent")];
        let c = ctx(&file, &root, &[], &exports, None, &cache);
        assert!(eval_pred_str("file exports 'MyComponent'", &c));
    }

    #[test]
    fn exports_literal_miss() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let exports = vec![make_export("OtherComponent")];
        let c = ctx(&file, &root, &[], &exports, None, &cache);
        assert!(!eval_pred_str("file exports 'MyComponent'", &c));
    }

    // -----------------------------------------------------------------------
    // 16. exists — true for real file (relative to project_root)
    // -----------------------------------------------------------------------

    #[test]
    fn exists_true_for_cargo_toml() {
        // The cofferdam repo always has Cargo.toml at the workspace root.
        // We cannot hard-code the absolute path, so we use the file's
        // crate manifest directory.
        let cache = GlobCache::new();
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        // Go up to the workspace root (cofferdam-core is at crates/cofferdam-core).
        let workspace_root = manifest.parent().unwrap().parent().unwrap();
        let file = workspace_root.join("src/dummy.ts");
        let c = ctx(&file, workspace_root, &[], &[], None, &cache);
        assert!(eval_pred_str("exists('Cargo.toml')", &c));
    }

    // -----------------------------------------------------------------------
    // 17. exists — false for nonexistent file
    // -----------------------------------------------------------------------

    #[test]
    fn exists_false_for_nonexistent() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        assert!(!eval_pred_str("exists('nonexistent.zzz')", &c));
    }

    // -----------------------------------------------------------------------
    // 18. exists with concat arg (spec round-trip example)
    // -----------------------------------------------------------------------

    #[test]
    fn exists_with_concat_arg_round_trip() {
        use tempfile::tempdir;
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        // Create a test file: tests/myfile.ts.
        let tests_dir = root.join("tests");
        std::fs::create_dir_all(&tests_dir).unwrap();
        std::fs::write(tests_dir.join("myfile.ts"), "").unwrap();

        let file = root.join("src").join("myfile.ts");
        let cache = GlobCache::new();
        let c = ctx(&file, &root, &[], &[], None, &cache);
        // `exists('tests/' + basename(file) + '.ts')`
        // basename('src/myfile.ts') → 'myfile.ts'
        // concat → 'tests/myfile.ts.ts' — wait, that's wrong.
        // Per the spec example: basename yields 'myfile.ts', concat gives
        // 'tests/' + 'myfile.ts' + '.ts' = 'tests/myfile.ts.ts'.
        // The test file we created is 'tests/myfile.ts', not 'tests/myfile.ts.ts'.
        // Re-read the spec: the example is for a controller:
        // `exists('tests/controllers/' + basename(file) + '.test.ts')`
        // So here we test the shape works end-to-end.
        // Create 'tests/myfile.ts.ts' to make exists return true.
        std::fs::write(tests_dir.join("myfile.ts.ts"), "").unwrap();

        assert!(eval_pred_str(
            "exists('tests/' + basename(file) + '.ts')",
            &c
        ));
    }

    // -----------------------------------------------------------------------
    // 19. core.symbol → UnsupportedNamespaceMember
    // -----------------------------------------------------------------------

    #[test]
    fn core_symbol_is_unsupported() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        let pred = parse_predicate("core.symbol('foo') == 'foo'").expect("parse");
        let err = eval_predicate(&pred, &c).expect_err("expected error");
        assert!(
            matches!(err, DslEvalError::UnsupportedNamespaceMember { ref ns, ref member }
                if ns == "core" && member == "symbol"),
            "expected UnsupportedNamespaceMember(core.symbol), got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // 20. ts.declaration → UnsupportedNamespaceMember
    // -----------------------------------------------------------------------

    #[test]
    fn ts_declaration_is_unsupported() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        let pred = parse_predicate("ts.declaration('Foo') == 'Foo'").expect("parse");
        let err = eval_predicate(&pred, &c).expect_err("expected error");
        assert!(
            matches!(err, DslEvalError::UnsupportedNamespaceMember { ref ns, ref member }
                if ns == "ts" && member == "declaration"),
            "expected UnsupportedNamespaceMember(ts.declaration), got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // 21. Operand::Concat as bare predicate → TypeMismatch
    // -----------------------------------------------------------------------

    #[test]
    fn bare_concat_operand_as_predicate_is_type_error() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/foo.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        // Construct the Operand directly (the parser won't produce this shape
        // at top level, but the evaluator must handle it).
        let concat = Predicate::Operand(Operand::Concat(
            Box::new(Operand::String("a".to_string())),
            Box::new(Operand::String("b".to_string())),
        ));
        let err = eval_predicate(&concat, &c).expect_err("expected type error");
        assert!(
            matches!(
                err,
                DslEvalError::TypeMismatch {
                    expected: "boolean",
                    got: "string",
                    ..
                }
            ),
            "expected TypeMismatch, got {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // 22. Full controller-test-pair example
    // -----------------------------------------------------------------------

    #[test]
    fn controller_test_pair_file_exists() {
        use tempfile::tempdir;
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        let controllers_dir = root.join("src").join("controllers");
        std::fs::create_dir_all(&controllers_dir).unwrap();

        let tests_dir = root.join("tests");
        std::fs::create_dir_all(&tests_dir).unwrap();

        // Create src/controllers/user.ts and tests/user.ts.
        let file = controllers_dir.join("user.ts");
        std::fs::write(&file, "").unwrap();
        std::fs::write(tests_dir.join("user.ts"), "").unwrap();

        let cache = GlobCache::new();
        let c = ctx(&file, &root, &[], &[], None, &cache);

        // `forbid file matches 'src/controllers/**' and not exists('tests/' + basename(file))`
        // basename('src/controllers/user.ts') → 'user.ts'
        // concat → 'tests/user.ts'  → exists → true → not exists → false
        // and → false → forbid false → true (no violation)
        let result = eval_str(
            "forbid file matches 'src/controllers/**' and not exists('tests/' + basename(file))",
            &c,
        );
        assert!(
            result,
            "test exists, so forbid condition should be false → rule passes"
        );
    }

    #[test]
    fn controller_test_pair_file_missing() {
        use tempfile::tempdir;
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        let controllers_dir = root.join("src").join("controllers");
        std::fs::create_dir_all(&controllers_dir).unwrap();
        // Do NOT create the test file.

        let file = controllers_dir.join("widget.ts");
        std::fs::write(&file, "").unwrap();

        let cache = GlobCache::new();
        let c = ctx(&file, &root, &[], &[], None, &cache);

        // Test file missing → exists → false → not exists → true
        // file matches → true → and → true → forbid true → false (violation!)
        let result = eval_str(
            "forbid file matches 'src/controllers/**' and not exists('tests/' + basename(file))",
            &c,
        );
        assert!(
            !result,
            "test missing, so forbid condition is true → rule fails"
        );
    }

    // -----------------------------------------------------------------------
    // 23. ui-no-localStorage example
    // -----------------------------------------------------------------------

    #[test]
    fn ui_no_localstorage_hit() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("ui/Dashboard.tsx");
        let imports = vec![make_import("localStorage", false)];
        let c = ctx(&file, &root, &imports, &[], None, &cache);
        // File is in ui/ AND imports localStorage → forbid fails (violation).
        assert!(!eval_str(
            "forbid file matches 'ui/**' and imports 'localStorage'",
            &c
        ));
    }

    #[test]
    fn ui_no_localstorage_miss() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("ui/Dashboard.tsx");
        // No localStorage import.
        let c = ctx(&file, &root, &[], &[], None, &cache);
        // No import → and is false → forbid false → true (no violation).
        assert!(eval_str(
            "forbid file matches 'ui/**' and imports 'localStorage'",
            &c
        ));
    }

    // -----------------------------------------------------------------------
    // 24. basename(file)
    // -----------------------------------------------------------------------

    #[test]
    fn basename_returns_bare_filename() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/controllers/user.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);
        // `file == basename(file)` — false because 'src/controllers/user.ts' != 'user.ts'
        // But let's verify the operand value directly.
        let pred = parse_predicate("file == basename(file)").expect("parse");
        // Just verify it evaluates without error and returns false (path != basename).
        let result = eval_predicate(&pred, &c).expect("eval");
        assert!(!result, "full path should not equal its basename");

        // Also verify basename returns the right string via operand eval.
        let op = crate::dsl::ast::Operand::Call(crate::dsl::ast::Call {
            name: "basename".to_string(),
            args: vec![Predicate::Call(crate::dsl::ast::Call {
                name: "file".to_string(),
                args: vec![],
            })],
        });
        let result = eval_operand(&op, &c).expect("operand eval");
        assert_eq!(result, "user.ts");
    }

    // -----------------------------------------------------------------------
    // 25. dirname(file)
    // -----------------------------------------------------------------------

    #[test]
    fn dirname_returns_parent() {
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");
        let file = root.join("src/controllers/user.ts");
        let c = ctx(&file, &root, &[], &[], None, &cache);

        let op = crate::dsl::ast::Operand::Call(crate::dsl::ast::Call {
            name: "dirname".to_string(),
            args: vec![Predicate::Call(crate::dsl::ast::Call {
                name: "file".to_string(),
                args: vec![],
            })],
        });
        let result = eval_operand(&op, &c).expect("operand eval");
        // The project-relative path is 'src/controllers/user.ts';
        // dirname → 'src/controllers'.
        assert_eq!(result, "src/controllers");
    }

    // -----------------------------------------------------------------------
    // 26. GlobCache reuse across multiple files
    // -----------------------------------------------------------------------

    #[test]
    fn glob_cache_reuses_pattern_across_files() {
        // One shared cache, three different files — verifies the cached path
        // is exercised without regressions in match/no-match semantics.
        let cache = GlobCache::new();
        let root = PathBuf::from("/proj");

        let file_a = root.join("src/a.ts");
        let ca = ctx(&file_a, &root, &[], &[], None, &cache);
        assert!(eval_pred_str("file matches 'src/**'", &ca));

        let file_b = root.join("src/b.ts");
        let cb = ctx(&file_b, &root, &[], &[], None, &cache);
        assert!(eval_pred_str("file matches 'src/**'", &cb));

        let file_c = root.join("ui/c.tsx");
        let cc = ctx(&file_c, &root, &[], &[], None, &cache);
        assert!(!eval_pred_str("file matches 'src/**'", &cc));
    }
}
