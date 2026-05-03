//! Readability checks.

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, OptionDefault, OptionKind, OptionSpec,
    Priority, Severity, SourceFile, Span,
};
use oxc_ast::ast::{ArrowFunctionExpression, Function, FunctionBody, Statement};
use oxc_ast_visit::Visit;

// ---------- Readability.MaxLineLength ----------

/// Phase-0 canary check. Flags any line whose byte length exceeds `limit`.
///
/// Deliberately byte-based for now — switching to grapheme/column width is
/// a phase-1 task once we have the unicode crate in the tree. Good enough
/// to validate the architecture seam end-to-end.
pub struct MaxLineLength {
    limit: u32,
    meta: &'static CheckMeta,
}

const MLL_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "limit",
    kind: OptionKind::Int,
    default: OptionDefault::Int(120),
    doc: "maximum line length in bytes",
}];

const MLL_META: CheckMeta = CheckMeta {
    id: "Readability.MaxLineLength",
    category: Category::Readability,
    base_priority: -5,
    default_severity: Severity::Low,
    explanation: "Lines longer than the configured limit are harder to scan and review.",
    body: include_str!("../docs/Readability.MaxLineLength.md"),
    requires_types: false,
    consistency: false,
    options: MLL_OPTIONS,
};

impl MaxLineLength {
    pub fn new(limit: u32) -> Self {
        Self {
            limit,
            meta: &MLL_META,
        }
    }
}

impl Check for MaxLineLength {
    fn meta(&self) -> &'static CheckMeta {
        self.meta
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let limit = ctx
            .options
            .get_int("limit")
            .map(|v| v as u32)
            .unwrap_or(self.limit);
        let mut out = Vec::new();
        let mut byte_offset: u32 = 0;
        for (line_no, line) in file.lines() {
            let len = line.len() as u32;
            if len > limit {
                out.push(Issue {
                    check_id: self.meta.id.to_string(),
                    message: format!("line is {} characters, exceeds limit of {}", len, limit),
                    file: file.path.clone(),
                    span: Span {
                        start_byte: byte_offset,
                        end_byte: byte_offset + len,
                        line: line_no,
                        column: limit + 1,
                    },
                    priority: Priority(self.meta.base_priority),
                    severity: Severity::Medium,
                    related: Vec::new(),
                    fix: None,
                });
            }
            // +1 for the trailing '\n' consumed by `split`. Off-by-one on the
            // final line if the file lacks a trailing newline; cosmetic only,
            // affects span offsets not line numbers. Fixed in phase 1.
            byte_offset = byte_offset.saturating_add(len + 1);
        }
        out
    }
}

// ---------- Readability.MaxFunctionLength ----------

/// `Readability.MaxFunctionLength` — flag function bodies whose line span
/// exceeds `limit`.
///
/// Measures end_line - start_line on the body block. Arrow functions
/// with expression bodies (`x => x + 1`) are skipped — they're by
/// definition short. Block-bodied arrows (`x => { ... }`) are measured.
pub struct MaxFunctionLength {
    limit: u32,
    meta: &'static CheckMeta,
}

const MFL_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "limit",
    kind: OptionKind::Int,
    default: OptionDefault::Int(50),
    doc: "maximum function body length in lines",
}];

const MFL_META: CheckMeta = CheckMeta {
    id: "Readability.MaxFunctionLength",
    category: Category::Readability,
    base_priority: -5,
    default_severity: Severity::Low,
    explanation:
        "Functions longer than the configured limit are hard to follow. Break them into smaller helpers.",
    body: include_str!("../docs/Readability.MaxFunctionLength.md"),
    requires_types: false,
    consistency: false,
    options: MFL_OPTIONS,
};

impl MaxFunctionLength {
    pub fn new(limit: u32) -> Self {
        Self {
            limit,
            meta: &MFL_META,
        }
    }
}

impl Check for MaxFunctionLength {
    fn meta(&self) -> &'static CheckMeta {
        self.meta
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let limit = ctx
            .options
            .get_int("limit")
            .map(|v| v as u32)
            .unwrap_or(self.limit);
        let mut visitor = MFLCollector {
            file,
            limit,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct MFLCollector<'a> {
    file: &'a SourceFile,
    limit: u32,
    issues: Vec<Issue>,
}

impl<'a> MFLCollector<'a> {
    fn measure(&mut self, body: &FunctionBody<'_>, name: &str) {
        let start_span = span_from_bytes(&self.file.text, body.span.start, body.span.start);
        let end_span = span_from_bytes(&self.file.text, body.span.end, body.span.end);
        let length = end_span.line.saturating_sub(start_span.line);
        if length > self.limit {
            let span = span_from_bytes(&self.file.text, body.span.start, body.span.end);
            self.issues.push(Issue {
                check_id: MFL_META.id.to_string(),
                message: format!(
                    "{} is {} lines, exceeds limit of {}",
                    name, length, self.limit
                ),
                file: self.file.path.clone(),
                span,
                priority: Priority(MFL_META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
                fix: None,
            });
        }
    }
}

impl<'a> Visit<'a> for MFLCollector<'a> {
    fn visit_function(&mut self, node: &Function<'a>, flags: oxc_syntax::scope::ScopeFlags) {
        if let Some(body) = &node.body {
            let name = node
                .id
                .as_ref()
                .map(|id| id.name.as_str().to_string())
                .unwrap_or_else(|| "anonymous function".to_string());
            self.measure(body, &name);
        }
        oxc_ast_visit::walk::walk_function(self, node, flags);
    }

    fn visit_arrow_function_expression(&mut self, node: &ArrowFunctionExpression<'a>) {
        // Arrow functions are FunctionBody-bodied or expression-bodied.
        // The `expression` flag on oxc's ArrowFunctionExpression is true
        // when the body is a single expression (no braces). Skip those —
        // they're measurable but not useful to flag.
        if !node.expression {
            self.measure(&node.body, "arrow function");
        } else if let Some(Statement::ExpressionStatement(_)) = node.body.statements.first() {
            // Block stays. Expression-bodied arrow's body is wrapped in
            // a BlockStatement with one ExpressionStatement; nothing to
            // measure.
        }
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
    }
}
