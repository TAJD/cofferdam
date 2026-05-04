//! Warning checks — likely bugs. Default severity is `Error` for this
//! category once the per-category severity defaults wire up (phase 3).

use cofferdam_core::span_from_bytes;
use cofferdam_core::{Category, CheckMeta, Issue, Priority, Severity, SourceFile, TextEdit};
use cofferdam_ts::oxc_ast::ast::{
    BinaryExpression, BinaryOperator, CallExpression, DebuggerStatement, Expression, NewExpression,
};
use cofferdam_ts::oxc_ast_visit::Visit;
use cofferdam_ts::{Check, CheckContext};

/// `Warning.TripleEquals` — flags `==` and `!=` (vs `===` / `!==`).
///
/// Implementation note: pattern B from the SDK design (see
/// rovikore_host_credo_checks memory). Walks every BinaryExpression
/// and checks the operator. The visitor is one-shot per file; phase-1
/// engine creates a fresh allocator + parsed view per file, runs all
/// checks, drops everything.
pub struct TripleEquals;

const META: CheckMeta = CheckMeta {
    id: "Warning.TripleEquals",
    category: Category::Warning,
    base_priority: 15,
    default_severity: Severity::High,
    explanation: "`==` and `!=` perform type coercion and are almost always a bug. Use `===` and `!==` instead.",
    body: include_str!("../docs/Warning.TripleEquals.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    files: None,
};

impl Check for TripleEquals {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = Collector {
            file,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }

    /// Replace the loose-equality operator with its strict equivalent.
    ///
    /// The issue span covers the whole `BinaryExpression` (e.g. `a == b`).
    /// We scan the source bytes within that span to locate just the operator
    /// token, taking care to skip `===`/`!==` and to handle `!=` before `==`
    /// so a `!=` sequence isn't misidentified as `=`.
    fn autofix(&self, issue: &Issue, source: &SourceFile) -> Option<TextEdit> {
        let start = issue.span.start_byte as usize;
        let end = issue.span.end_byte as usize;
        let slice = source.text.get(start..end)?;
        let bytes = slice.as_bytes();
        let len = bytes.len();

        // Scan for the operator token. We look for `!=` (not `!==`) and
        // `==` (not `===`). The approach: walk byte-by-byte and test each
        // position.
        let mut i = 0usize;
        while i < len {
            // Try `!=` at position i.
            if i + 1 < len && bytes[i] == b'!' && bytes[i + 1] == b'=' {
                // Make sure it's `!=` not `!==`.
                let is_strict = i + 2 < len && bytes[i + 2] == b'=';
                if !is_strict {
                    let op_start = (start + i) as u32;
                    let op_end = op_start + 2;
                    let op_span = span_from_bytes(&source.text, op_start, op_end);
                    return Some(TextEdit {
                        span: op_span,
                        replacement: "!==".to_string(),
                    });
                }
                // Skip past `!==` entirely so we don't re-test the `=` bytes.
                i += 3;
                continue;
            }
            // Try `==` at position i (not `===`).
            if i + 1 < len && bytes[i] == b'=' && bytes[i + 1] == b'=' {
                let is_strict = i + 2 < len && bytes[i + 2] == b'=';
                if !is_strict {
                    let op_start = (start + i) as u32;
                    let op_end = op_start + 2;
                    let op_span = span_from_bytes(&source.text, op_start, op_end);
                    return Some(TextEdit {
                        span: op_span,
                        replacement: "===".to_string(),
                    });
                }
                // Skip past `===`.
                i += 3;
                continue;
            }
            i += 1;
        }
        None
    }
}

struct Collector<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
}

impl<'a> Visit<'a> for Collector<'a> {
    fn visit_binary_expression(&mut self, node: &BinaryExpression<'a>) {
        if matches!(
            node.operator,
            BinaryOperator::Equality | BinaryOperator::Inequality
        ) {
            let preferred = match node.operator {
                BinaryOperator::Equality => "===",
                BinaryOperator::Inequality => "!==",
                _ => unreachable!(),
            };
            let actual = match node.operator {
                BinaryOperator::Equality => "==",
                BinaryOperator::Inequality => "!=",
                _ => unreachable!(),
            };
            // Point at the whole expression. A more precise span on the
            // operator token itself is a cd-81a.6 (autofix) follow-up.
            let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
            self.issues.push(Issue {
                check_id: META.id.to_string(),
                message: format!("use `{}` instead of `{}`", preferred, actual),
                file: self.file.path.clone(),
                span,
                priority: Priority(META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
                fix: None,
            });
        }
        // Walk into children — `==` can appear inside other binary ops
        // (e.g. `a && b == c`).
        cofferdam_ts::oxc_ast_visit::walk::walk_binary_expression(self, node);
    }
}

/// `Warning.NoConsoleLog` — flags any `console.X(...)` call.
///
/// Scope: every method on `console` (`log`, `info`, `warn`, `error`,
/// `debug`, `trace`, …). Most projects either route logging through a
/// dedicated logger or strip console calls in CI; this check surfaces
/// the leaks without distinguishing methods. Teams that genuinely use
/// `console.error` for logging can suppress per-line via inline
/// directive or tune severity in `cofferdam.toml`.
///
/// We only match the *bare* identifier `console`. Aliasing (`const c =
/// console; c.log(...)`) escapes detection — that's a known limit of
/// AST-only checks and not worth the complexity until the type-aware
/// pass lands.
pub struct NoConsoleLog;

const NO_CONSOLE_LOG_META: CheckMeta = CheckMeta {
    id: "Warning.NoConsoleLog",
    category: Category::Warning,
    base_priority: -10,
    default_severity: Severity::Low,
    explanation: "`console.X(...)` calls are typically debugging leftovers. Route logs through a dedicated logger or strip them in CI.",
    body: include_str!("../docs/Warning.NoConsoleLog.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    files: None,
};

impl Check for NoConsoleLog {
    fn meta(&self) -> &'static CheckMeta {
        &NO_CONSOLE_LOG_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = ConsoleCollector {
            file,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct ConsoleCollector<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
}

impl<'a> Visit<'a> for ConsoleCollector<'a> {
    fn visit_call_expression(&mut self, node: &CallExpression<'a>) {
        if let Expression::StaticMemberExpression(member) = &node.callee {
            if let Expression::Identifier(ident) = &member.object {
                if ident.name.as_str() == "console" {
                    let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
                    self.issues.push(Issue {
                        check_id: NO_CONSOLE_LOG_META.id.to_string(),
                        message: format!(
                            "remove `console.{}` call (use a logger or strip in CI)",
                            member.property.name.as_str()
                        ),
                        file: self.file.path.clone(),
                        span,
                        priority: Priority(NO_CONSOLE_LOG_META.base_priority),
                        severity: Severity::Medium,
                        related: Vec::new(),
                        fix: None,
                    });
                }
            }
        }
        cofferdam_ts::oxc_ast_visit::walk::walk_call_expression(self, node);
    }
}

/// `Warning.NoDebugger` — flags every `debugger;` statement.
///
/// `debugger` halts execution under any attached devtools. Always a
/// debugging leftover in shipped code — there's no benign use case in
/// production builds. The fix is mechanical: delete the line.
pub struct NoDebugger;

const NO_DEBUGGER_META: CheckMeta = CheckMeta {
    id: "Warning.NoDebugger",
    category: Category::Warning,
    base_priority: 10,
    default_severity: Severity::Medium,
    explanation:
        "`debugger` statements halt execution under attached devtools. Remove before shipping.",
    body: include_str!("../docs/Warning.NoDebugger.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    files: None,
};

impl Check for NoDebugger {
    fn meta(&self) -> &'static CheckMeta {
        &NO_DEBUGGER_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = DebuggerCollector {
            file,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct DebuggerCollector<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
}

impl<'a> Visit<'a> for DebuggerCollector<'a> {
    fn visit_debugger_statement(&mut self, node: &DebuggerStatement) {
        let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
        self.issues.push(Issue {
            check_id: NO_DEBUGGER_META.id.to_string(),
            message: "remove `debugger` statement".to_string(),
            file: self.file.path.clone(),
            span,
            priority: Priority(NO_DEBUGGER_META.base_priority),
            severity: Severity::Medium,
            related: Vec::new(),
            fix: None,
        });
        // DebuggerStatement has no children, but call walk for symmetry
        // and so future oxc versions adding fields don't silently drop
        // them.
        cofferdam_ts::oxc_ast_visit::walk::walk_debugger_statement(self, node);
    }
}

/// `Warning.NoEval` — flags `eval(...)` calls and `new Function(...)`.
///
/// Both forms execute arbitrary strings as code: `eval` directly,
/// `new Function(body)` via the Function constructor. Universally
/// banned in security-conscious codebases; this check has no opt-in
/// for a reason. Suppress per-line if you have a vetted, isolated use.
///
/// Aliasing (`const f = eval; f("...")`) is out of scope for the same
/// reason `NoConsoleLog` doesn't track aliases — bare-identifier match
/// is the line we draw without type info.
pub struct NoEval;

const NO_EVAL_META: CheckMeta = CheckMeta {
    id: "Warning.NoEval",
    category: Category::Warning,
    base_priority: 18,
    default_severity: Severity::High,
    explanation: "`eval(...)` and `new Function(...)` execute arbitrary strings as code. Universally banned for security and performance reasons.",
    body: include_str!("../docs/Warning.NoEval.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    files: None,
};

impl Check for NoEval {
    fn meta(&self) -> &'static CheckMeta {
        &NO_EVAL_META
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut visitor = EvalCollector {
            file,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct EvalCollector<'a> {
    file: &'a SourceFile,
    issues: Vec<Issue>,
}

impl<'a> Visit<'a> for EvalCollector<'a> {
    fn visit_call_expression(&mut self, node: &CallExpression<'a>) {
        if let Expression::Identifier(ident) = &node.callee {
            if ident.name.as_str() == "eval" {
                let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
                self.issues.push(Issue {
                    check_id: NO_EVAL_META.id.to_string(),
                    message: "avoid `eval(...)` — executes arbitrary strings as code".to_string(),
                    file: self.file.path.clone(),
                    span,
                    priority: Priority(NO_EVAL_META.base_priority),
                    severity: Severity::Medium,
                    related: Vec::new(),
                    fix: None,
                });
            }
        }
        cofferdam_ts::oxc_ast_visit::walk::walk_call_expression(self, node);
    }

    fn visit_new_expression(&mut self, node: &NewExpression<'a>) {
        if let Expression::Identifier(ident) = &node.callee {
            if ident.name.as_str() == "Function" {
                let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
                self.issues.push(Issue {
                    check_id: NO_EVAL_META.id.to_string(),
                    message: "avoid `new Function(...)` — eval-equivalent code execution"
                        .to_string(),
                    file: self.file.path.clone(),
                    span,
                    priority: Priority(NO_EVAL_META.base_priority),
                    severity: Severity::Medium,
                    related: Vec::new(),
                    fix: None,
                });
            }
        }
        cofferdam_ts::oxc_ast_visit::walk::walk_new_expression(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::SourceFile;
    use cofferdam_ts::{parse_into, Allocator, Check, CheckContext, ParsedView};
    use std::path::PathBuf;

    /// Parse `src` as TypeScript, run `TripleEquals`, and return all issues.
    fn run_triple_equals(src: &str) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src);
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file).with_parsed(&parsed);
        TripleEquals.run(&file, &mut ctx)
    }

    #[test]
    fn autofix_equality_returns_triple_equals() {
        // `a == b` — the operator is `==`, replacement should be `===`.
        let src = "const r = a == b;";
        let issues = run_triple_equals(src);
        assert_eq!(issues.len(), 1, "expected exactly one issue for `==`");
        let edit = TripleEquals
            .autofix(&issues[0], &SourceFile::new(PathBuf::from("test.ts"), src))
            .expect("autofix should return Some for `==`");
        // The replacement text must be the strict form.
        assert_eq!(edit.replacement, "===");
        // The edit span must cover only the operator, not the whole expression.
        let op_slice = &src[edit.span.start_byte as usize..edit.span.end_byte as usize];
        assert_eq!(op_slice, "==");
    }

    #[test]
    fn autofix_inequality_returns_strict_not_equal() {
        // `a != b` — the operator is `!=`, replacement should be `!==`.
        let src = "const r = a != b;";
        let issues = run_triple_equals(src);
        assert_eq!(issues.len(), 1, "expected exactly one issue for `!=`");
        let edit = TripleEquals
            .autofix(&issues[0], &SourceFile::new(PathBuf::from("test.ts"), src))
            .expect("autofix should return Some for `!=`");
        assert_eq!(edit.replacement, "!==");
        let op_slice = &src[edit.span.start_byte as usize..edit.span.end_byte as usize];
        assert_eq!(op_slice, "!=");
    }

    #[test]
    fn autofix_strict_equality_returns_none() {
        // `===` is already strict — no issue, so autofix is never called,
        // but verify the check produces zero issues.
        let src = "const r = a === b;";
        let issues = run_triple_equals(src);
        assert!(issues.is_empty(), "`===` should not be flagged");
    }

    #[test]
    fn autofix_strict_inequality_returns_none() {
        let src = "const r = a !== b;";
        let issues = run_triple_equals(src);
        assert!(issues.is_empty(), "`!==` should not be flagged");
    }
}
