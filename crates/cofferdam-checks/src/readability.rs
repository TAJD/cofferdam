//! Readability checks.

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, LineView, Lines, Location, OptionDefault,
    OptionKind, OptionSpec, Priority, Severity, SourceFile, Span,
};
use oxc_ast::ast::{ArrowFunctionExpression, Function, FunctionBody, Statement};
use oxc_ast_visit::Visit;
use unicode_width::UnicodeWidthStr;

use crate::count_skippable_lines;

// ---------- Readability.MaxLineLength ----------

/// Flags any line whose display width exceeds `limit`.
///
/// Width is measured in terminal display columns via `unicode-width`
/// (cd-c8aq): a wide CJK glyph counts as 2, a zero-width combining mark
/// as 0, and control characters (including tabs and a trailing `\r` on
/// CRLF files) as 0. This matches what a reader sees in a monospace
/// editor — earlier we counted raw UTF-8 bytes, so box-drawing art,
/// em dashes, and accented letters were over-counted ~3x and falsely
/// flagged. The `Span` byte offsets stay byte-based (the `Span` contract);
/// only the limit comparison, the reported width, and the column are in
/// display columns.
pub struct MaxLineLength {
    limit: u32,
    meta: &'static CheckMeta,
}

const MLL_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "limit",
    kind: OptionKind::Int,
    default: OptionDefault::Int(120),
    doc: "maximum line length in display columns",
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
    autofix: false,
    pure_run: true,
};

impl MaxLineLength {
    /// Construct with a max line-length ceiling. `all_builtins`
    /// installs the default of 120; user config overrides via
    /// `[checks."Readability.MaxLineLength"].limit`.
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
            // Byte length advances the span cursor and bounds the `Span`
            // (which is byte-based by contract). Display width is what we
            // compare against the limit and report to the user (cd-c8aq).
            let byte_len = line.len() as u32;
            let width = UnicodeWidthStr::width(line) as u32;
            if width > limit {
                out.push(Issue {
                    check_id: self.meta.id.to_string(),
                    message: format!("line is {} columns, exceeds limit of {}", width, limit),
                    file: file.path.clone(),
                    location: Location::from_span(
                        &file.path,
                        Span {
                            start_byte: byte_offset,
                            end_byte: byte_offset + byte_len,
                            line: line_no,
                            // 1-based display column where the line first
                            // crosses the limit.
                            column: limit + 1,
                        },
                    ),
                    priority: Priority(self.meta.base_priority),
                    severity: Severity::Medium,
                    related: Vec::new(),
                });
            }
            // +1 for the trailing '\n' consumed by `split`. Off-by-one on the
            // final line if the file lacks a trailing newline; cosmetic only,
            // affects span offsets not line numbers.
            byte_offset = byte_offset.saturating_add(byte_len + 1);
        }
        out
    }
}

// ---------- Readability.MaxFunctionLength ----------

/// `Readability.MaxFunctionLength` — flag function bodies whose effective
/// line count exceeds `limit`.
///
/// "Effective" excludes blank lines and pure-comment lines (lines whose
/// trimmed text starts with `//`, `/*`, or `*` and oxc's comment table
/// flags as a comment). Trailing inline comments on otherwise-code lines
/// still count. Computed as `(end_line - start_line) - skippable_lines`
/// over the body block. Arrow functions with expression bodies
/// (`x => x + 1`) are skipped. Block-bodied arrows are measured.
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
    autofix: false,
    pure_run: true,
};

impl MaxFunctionLength {
    /// Construct with a max function-length ceiling. `all_builtins`
    /// installs the default of 50; user config overrides via
    /// `[checks."Readability.MaxFunctionLength"].limit`.
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
        let line_views: Vec<LineView<'_>> = Lines::build(&file.text, parsed.program).collect();
        let mut visitor = MFLCollector {
            file,
            limit,
            line_views: &line_views,
            issues: Vec::new(),
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

struct MFLCollector<'a> {
    file: &'a SourceFile,
    limit: u32,
    line_views: &'a [LineView<'a>],
    issues: Vec<Issue>,
}

impl<'a> MFLCollector<'a> {
    /// `node_start`/`node_end` are the enclosing function/arrow node's own
    /// span (name + parameter list + body), used only for the reported
    /// `Issue.location` — length is still measured from `body` alone. This
    /// makes the finding's span match `Refactor.CyclomaticComplexity` and
    /// `Refactor.CognitiveComplexity` (whole-function, not just the body),
    /// which baseline signature computation relies on to find the
    /// function's header (cd-9, "rulesig").
    fn measure(&mut self, body: &FunctionBody<'_>, node_start: u32, node_end: u32, name: &str) {
        let start_span = span_from_bytes(&self.file.text, body.span.start, body.span.start);
        let end_span = span_from_bytes(&self.file.text, body.span.end, body.span.end);
        let raw = end_span.line.saturating_sub(start_span.line);
        // Discount blank + pure-comment lines strictly between the braces
        // — `start_line + 1 ..= end_line - 1`. The brace lines themselves
        // are outside the body's interior, so excluding them matches the
        // previous `end_line - start_line` semantics.
        let inner_lo = start_span.line.saturating_add(1);
        let inner_hi = end_span.line.saturating_sub(1);
        let skipped = count_skippable_lines(self.line_views, inner_lo, inner_hi);
        let length = raw.saturating_sub(skipped);
        if length > self.limit {
            let span = span_from_bytes(&self.file.text, node_start, node_end);
            self.issues.push(Issue {
                check_id: MFL_META.id.to_string(),
                message: format!(
                    "{} is {} lines (excluding blanks and comments), exceeds limit of {}",
                    name, length, self.limit
                ),
                file: self.file.path.clone(),
                location: Location::from_span(&self.file.path, span),
                priority: Priority(MFL_META.base_priority),
                severity: Severity::Medium,
                related: Vec::new(),
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
            self.measure(body, node.span.start, node.span.end, &name);
        }
        oxc_ast_visit::walk::walk_function(self, node, flags);
    }

    fn visit_arrow_function_expression(&mut self, node: &ArrowFunctionExpression<'a>) {
        // Arrow functions are FunctionBody-bodied or expression-bodied.
        // The `expression` flag on oxc's ArrowFunctionExpression is true
        // when the body is a single expression (no braces). Skip those —
        // they're measurable but not useful to flag.
        if !node.expression {
            self.measure(&node.body, node.span.start, node.span.end, "arrow function");
        } else if let Some(Statement::ExpressionStatement(_)) = node.body.statements.first() {
            // Block stays. Expression-bodied arrow's body is wrapped in
            // a BlockStatement with one ExpressionStatement; nothing to
            // measure.
        }
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{Check, CheckContext, SourceFile};
    use std::path::PathBuf;

    /// Run `MaxLineLength` against `src` with the given column limit.
    fn run_mll(src: &str, limit: u32) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src);
        let mut ctx = CheckContext::new(&file);
        MaxLineLength::new(limit).run(&file, &mut ctx)
    }

    #[test]
    fn ascii_line_over_limit_flags_with_column_unit() {
        let src = "a".repeat(130);
        let issues = run_mll(&src, 120);
        assert_eq!(issues.len(), 1, "130-col ASCII line should flag at 120");
        assert_eq!(
            issues[0].message,
            "line is 130 columns, exceeds limit of 120"
        );
        // Column points at the first over-limit display column (limit + 1).
        assert_eq!(issues[0].location.column(), 121);
    }

    #[test]
    fn ascii_line_under_limit_ok() {
        let src = "a".repeat(120);
        assert!(
            run_mll(&src, 120).is_empty(),
            "a line exactly at the limit must not flag"
        );
    }

    // cd-c8aq regression: the original repro. `// ` + 45 box-drawing chars
    // (U+2500) is 48 display columns but 138 UTF-8 bytes. The old byte-based
    // check reported "138 characters" and flagged it; display width must not.
    #[test]
    fn box_drawing_banner_under_limit_not_flagged() {
        let src = format!("// {}", "\u{2500}".repeat(45));
        assert_eq!(src.len(), 138, "sanity: the line is 138 UTF-8 bytes");
        assert!(
            run_mll(&src, 120).is_empty(),
            "48-column box-drawing banner must not flag at a 120-column limit"
        );
    }

    // Wide CJK ideographs are 2 display columns each. 61 ideographs = 61
    // scalars (a scalar count would NOT flag at 120) but 122 columns (display
    // width DOES flag) — proving we measure width, not chars and not bytes.
    #[test]
    fn wide_cjk_counted_as_two_columns_each() {
        let src = "\u{4e00}".repeat(61); // 61 × U+4E00 '一'
        let issues = run_mll(&src, 120);
        assert_eq!(issues.len(), 1, "122 columns of CJK should flag at 120");
        assert_eq!(
            issues[0].message,
            "line is 122 columns, exceeds limit of 120"
        );
    }

    // The `Span` stays byte-based even when display width drives the finding:
    // 130 ideographs = 260 columns (reported) but 390 bytes (span end_byte).
    #[test]
    fn span_byte_offsets_remain_byte_based() {
        let src = "\u{4e00}".repeat(130);
        let issues = run_mll(&src, 120);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].location.start_byte(), 0);
        assert_eq!(
            issues[0].location.end_byte(),
            390,
            "end_byte must be the UTF-8 byte length (130 × 3), not the column count"
        );
        assert!(issues[0].message.contains("260 columns"));
    }
}
