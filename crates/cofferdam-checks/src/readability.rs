//! Readability checks.

use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, Issue, LineIndex, LineView, Lines, Location,
    OptionDefault, OptionKind, OptionSpec, Priority, Severity, SourceFile, Span,
};
use oxc_ast::ast::{
    ArrowFunctionExpression, Function, FunctionBody, JSXAttribute, JSXAttributeValue, Program,
    RegExpLiteral, Statement, StringLiteral, TemplateLiteral,
};
use oxc_ast_visit::{walk, Visit};
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
///
/// Two exceptions to the raw width comparison (CD-318, CD-322):
/// - A line is skipped when it is **unwrappable**: its widest atomic
///   token — a string/template/regex literal or a whole JSX attribute
///   with a literal value, kept whole; or, elsewhere on the line, a
///   plain whitespace-delimited run — exceeds `limit` once the line's
///   own indentation is added, which is the narrowest a formatter could
///   ever place it. A long prose comment or run of JSX text still
///   reports because its words are ordinary-width; a long string, a
///   Tailwind `className`, or an unbreakable identifier or import path
///   does not.
///   `report_unwrappable = true` restores flagging every over-limit line.
/// - A line that is itself a cofferdam suppression directive is skipped
///   outright — the engine has no mechanism to suppress a directive's
///   own line, so flagging it makes documenting a suppression with a
///   reason impossible without violating this check.
pub struct MaxLineLength {
    limit: u32,
    meta: &'static CheckMeta,
}

const MLL_OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        name: "limit",
        kind: OptionKind::Int,
        default: OptionDefault::Int(120),
        doc: "maximum line length in display columns",
    },
    OptionSpec {
        name: "report_unwrappable",
        kind: OptionKind::Bool,
        default: OptionDefault::Bool(false),
        doc: "report lines even when the overflow comes from a single atomic token \
            (a string/template/regex literal, or a JSX attribute with a literal \
            value) that no formatter can break; disabled by default since such \
            lines can't be shortened by hand either",
    },
];

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
        let report_unwrappable = ctx.options.get_bool("report_unwrappable").unwrap_or(false);

        // One walk collecting every atomic-token span up front (CD-318) —
        // resolving each over-limit line against it below is a cursor scan,
        // not a re-walk. `None` when parsing failed or produced no AST;
        // MLL then falls back to flagging every over-limit line, same as
        // a check with no AST need.
        let atomic = ctx.parsed.map(|parsed| {
            let mut collector = AtomicTokenCollector { spans: Vec::new() };
            collector.visit_program(parsed.program);
            collector.spans.sort_unstable_by_key(|s| s.0);
            (collector.spans, line_starts(&file.text))
        });

        let mut out = Vec::new();
        let mut byte_offset: u32 = 0;
        let mut cursor = 0usize;
        let mut active: Vec<(u32, u32)> = Vec::new();
        for (line_no, line) in file.lines() {
            // Byte length advances the span cursor and bounds the `Span`
            // (which is byte-based by contract). Display width is what we
            // compare against the limit and report to the user (cd-c8aq).
            let byte_len = line.len() as u32;
            if is_suppression_directive(line) {
                byte_offset = byte_offset.saturating_add(byte_len + 1);
                continue;
            }
            let width = UnicodeWidthStr::width(line) as u32;
            let unwrappable = !report_unwrappable
                && width > limit
                && atomic.as_ref().is_some_and(|(spans, starts)| {
                    starts
                        .get((line_no - 1) as usize)
                        .is_some_and(|&line_start| {
                            is_unwrappable(line_start, line, limit, spans, &mut cursor, &mut active)
                        })
                });
            if width > limit && !unwrappable {
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

/// Byte offset of the start of every line in `text`, index 0 == line 1.
/// Independent of the `byte_offset` cursor `MaxLineLength::run` advances
/// over `SourceFile::lines()` output (which trims `\r`, so it drifts by
/// one byte per CRLF line) — atomic-token spans come from oxc and are
/// real file offsets, so resolving them against lines needs an accurate
/// table, not the trimmed-length accumulation.
fn line_starts(text: &str) -> Vec<u32> {
    let mut starts = Vec::with_capacity(text.bytes().filter(|&b| b == b'\n').count() + 1);
    starts.push(0u32);
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i as u32 + 1);
        }
    }
    starts
}

/// True when the widest atomic token on this line, on its own, already
/// exceeds `limit` — i.e. no formatter could bring the line under
/// `limit` because that single token can't be broken (CD-318).
///
/// "Atomic token" here is: each string/template/JSX-text/regex literal
/// overlapping the line (kept whole, spaces and all — `spans`), plus
/// every whitespace-delimited run of the line's remaining text (which
/// covers ordinary code tokens, long identifiers/import-path text, and
/// comment words alike — comments get no special casing, so a long
/// prose comment made of ordinary-width words still reports).
///
/// `spans` is sorted by start byte; `cursor`/`active` are threaded
/// across consecutive calls so resolving the whole file is one forward
/// scan, not a re-walk per line.
fn is_unwrappable(
    line_start: u32,
    line_text: &str,
    limit: u32,
    spans: &[(u32, u32)],
    cursor: &mut usize,
    active: &mut Vec<(u32, u32)>,
) -> bool {
    let line_end = line_start + line_text.len() as u32;
    // Pull in every span that starts before this line ends, THEN purge —
    // a call can be skipped for several lines in a row (only over-limit
    // lines invoke this), so a single catch-up pull may sweep in a span
    // that both starts and ends before `line_start` (fully on an
    // intervening skipped line). Purging first would miss it.
    while *cursor < spans.len() && spans[*cursor].0 < line_end {
        active.push(spans[*cursor]);
        *cursor += 1;
    }
    active.retain(|&(_, end)| end > line_start);

    let mut segments: Vec<(u32, u32)> = active
        .iter()
        .filter_map(|&(s, e)| {
            let seg_start = s.max(line_start) - line_start;
            let seg_end = e.min(line_end) - line_start;
            (seg_start < seg_end).then_some((seg_start, seg_end))
        })
        .collect();
    segments.sort_unstable();

    let mut widest = 0u32;
    let mut cur = 0u32;
    for (seg_start, seg_end) in segments {
        if cur < seg_start {
            widest = widest.max(widest_word(&line_text[cur as usize..seg_start as usize]));
        }
        widest = widest
            .max(UnicodeWidthStr::width(&line_text[seg_start as usize..seg_end as usize]) as u32);
        cur = cur.max(seg_end);
    }
    if (cur as usize) < line_text.len() {
        widest = widest.max(widest_word(&line_text[cur as usize..]));
    }

    // The best a formatter can do is put that token on a line of its own,
    // and it still has to indent it at least as far as this line is
    // indented. Counting the indentation is what catches the common JSX
    // case — a class list comfortably under the limit that only breaches
    // it once nesting is added.
    let indent =
        UnicodeWidthStr::width(&line_text[..line_text.len() - line_text.trim_start().len()]) as u32;
    widest.saturating_add(indent) > limit
}

/// Widest whitespace-delimited run in `s`, in display columns. 0 for
/// all-whitespace or empty input.
fn widest_word(s: &str) -> u32 {
    s.split_whitespace()
        .map(|w| UnicodeWidthStr::width(w) as u32)
        .max()
        .unwrap_or(0)
}

/// String/template/regex literal spans — the "atomic" tokens a formatter
/// cannot break (CD-318). Comments and JSX text are deliberately NOT
/// collected: both are prose that reflows at spaces, so they must keep
/// reporting like ordinary whitespace-split code.
struct AtomicTokenCollector {
    spans: Vec<(u32, u32)>,
}

impl<'a> Visit<'a> for AtomicTokenCollector {
    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) {
        self.spans.push((it.span.start, it.span.end));
        walk::walk_string_literal(self, it);
    }

    fn visit_template_literal(&mut self, it: &TemplateLiteral<'a>) {
        self.spans.push((it.span.start, it.span.end));
        walk::walk_template_literal(self, it);
    }

    fn visit_reg_exp_literal(&mut self, it: &RegExpLiteral<'a>) {
        self.spans.push((it.span.start, it.span.end));
        walk::walk_reg_exp_literal(self, it);
    }

    /// `className="…"` is atomic as a whole, not just its value: an
    /// attribute is the smallest unit a formatter can move to a line of
    /// its own, and it cannot break inside one. Without this, a class
    /// list narrower than the limit but pushed over it by the attribute
    /// name and its indentation reports with nothing anyone can do.
    fn visit_jsx_attribute(&mut self, it: &JSXAttribute<'a>) {
        if matches!(it.value, Some(JSXAttributeValue::StringLiteral(_))) {
            self.spans.push((it.span.start, it.span.end));
        }
        walk::walk_jsx_attribute(self, it);
    }
}

/// The suppression-directive markers recognised by
/// `cofferdam-engine::suppress` (kept local per the "no cofferdam-checks
/// -> cofferdam-engine dependency" rule — see the grammar doc at the top
/// of `crates/cofferdam-engine/src/suppress.rs`). `cofferdam-ignore` is a
/// substring of every Biome-style spelling (`-start`/`-end`/`-file`) and
/// `cofferdam-disable` of every ESLint-style one (`-next-line`/`-file`),
/// so three needles cover all documented spellings.
const SUPPRESSION_MARKERS: [&str; 3] =
    ["cofferdam-ignore", "cofferdam-disable", "cofferdam-enable"];

/// True when `line`, trimmed, opens with a comment marker and contains a
/// cofferdam suppression directive (CD-322). A prose comment that merely
/// mentions one of these words in passing may false-negative here — the
/// spec explicitly accepts that rather than parsing full directive
/// grammar in a line-length check.
fn is_suppression_directive(line: &str) -> bool {
    let trimmed = line.trim_start();
    let is_comment_line = trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || trimmed.starts_with('#')
        || trimmed.starts_with("<!--")
        // `{/* cofferdam-ignore: … */}` — the only way to write a
        // directive inside a JSX tree.
        || trimmed.starts_with("{/*");
    is_comment_line && SUPPRESSION_MARKERS.iter().any(|m| trimmed.contains(m))
}

/// Widest line in the file, in display columns — independent of `limit`.
/// Used by `cofferdam advise --analyze` (CD-65 A4). Returns 0 for an
/// empty file.
pub fn max_line_width_in_file(file: &SourceFile) -> u32 {
    file.lines()
        .map(|(_, line)| UnicodeWidthStr::width(line) as u32)
        .max()
        .unwrap_or(0)
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
        let line_index = LineIndex::new(&file.text);
        let mut visitor = MFLCollector {
            file,
            limit,
            line_views: &line_views,
            line_index: &line_index,
            issues: Vec::new(),
            max: 0,
        };
        visitor.visit_program(parsed.program);
        visitor.issues
    }
}

/// Longest effective function body (blanks/comments excluded), in lines,
/// found anywhere in the file — independent of `limit`. Used by
/// `cofferdam advise --analyze` (CD-65 A4). Returns 0 for a file with no
/// functions.
pub fn max_function_length_in_file(file: &SourceFile, program: &Program<'_>) -> u32 {
    let line_views: Vec<LineView<'_>> = Lines::build(&file.text, program).collect();
    let line_index = LineIndex::new(&file.text);
    let mut visitor = MFLCollector {
        file,
        limit: u32::MAX,
        line_views: &line_views,
        line_index: &line_index,
        issues: Vec::new(),
        max: 0,
    };
    visitor.visit_program(program);
    visitor.max
}

struct MFLCollector<'a> {
    file: &'a SourceFile,
    limit: u32,
    line_views: &'a [LineView<'a>],
    line_index: &'a LineIndex,
    issues: Vec<Issue>,
    /// Longest effective length seen across any function so far, tracked
    /// regardless of `limit` so [`max_function_length_in_file`] can reuse
    /// this same visitor.
    max: u32,
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
        let start_span = self
            .line_index
            .span_from_bytes(body.span.start, body.span.start);
        let end_span = self
            .line_index
            .span_from_bytes(body.span.end, body.span.end);
        let raw = end_span.line.saturating_sub(start_span.line);
        // Discount blank + pure-comment lines strictly between the braces
        // — `start_line + 1 ..= end_line - 1`. The brace lines themselves
        // are outside the body's interior, so excluding them matches the
        // previous `end_line - start_line` semantics.
        let inner_lo = start_span.line.saturating_add(1);
        let inner_hi = end_span.line.saturating_sub(1);
        let skipped = count_skippable_lines(self.line_views, inner_lo, inner_hi);
        let length = raw.saturating_sub(skipped);
        self.max = self.max.max(length);
        if length > self.limit {
            let span = self.line_index.span_from_bytes(node_start, node_end);
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
    use cofferdam_core::parser::{parse_into, ParsedView};
    use cofferdam_core::{
        validate_options, Allocator, Check, CheckContext, CheckOptions, RawOptionValue, SourceFile,
    };
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// Run `MaxLineLength` against `src` with the given column limit, no AST
    /// (`ctx.parsed` stays `None`) — exercises the plain-text fallback path.
    fn run_mll(src: &str, limit: u32) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src);
        let mut ctx = CheckContext::new(&file);
        MaxLineLength::new(limit).run(&file, &mut ctx)
    }

    fn mll_opts(limit: i64, report_unwrappable: bool) -> CheckOptions {
        let mut raw: BTreeMap<String, RawOptionValue> = BTreeMap::new();
        raw.insert("limit".to_string(), RawOptionValue::Int(limit));
        raw.insert(
            "report_unwrappable".to_string(),
            RawOptionValue::Bool(report_unwrappable),
        );
        validate_options(MLL_META.id, MLL_OPTIONS, &raw).expect("valid options")
    }

    /// Parse `src` as TypeScript and run `MaxLineLength` with `opts` —
    /// exercises the atomic-token-aware path (CD-318).
    fn run_mll_parsed(src: &str, opts: &CheckOptions) -> Vec<Issue> {
        run_mll_parsed_path(src, opts, "test.ts")
    }

    /// Like [`run_mll_parsed`] but with a caller-chosen path, so JSX
    /// fixtures can parse under `.tsx`.
    fn run_mll_parsed_path(src: &str, opts: &CheckOptions, path: &str) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from(path), src);
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let mut ctx = CheckContext::new(&file)
            .with_parsed(&parsed)
            .with_options(opts);
        MaxLineLength::new(120).run(&file, &mut ctx)
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

    // ── CD-322: suppression-directive lines are exempt ───────────────────

    #[test]
    fn long_suppression_directive_not_flagged() {
        let reason = "x".repeat(90);
        let src = format!("// cofferdam-ignore: Design.OrphanExport: {reason}");
        assert!(
            UnicodeWidthStr::width(src.as_str()) as u32 > 120,
            "sanity: directive line must actually be over-limit"
        );
        assert!(
            run_mll(&src, 120).is_empty(),
            "a suppression directive's own line must never flag, or reasons become undocumentable"
        );
    }

    #[test]
    fn every_documented_directive_spelling_is_exempt() {
        let filler = "x".repeat(100);
        let directives = [
            format!("// cofferdam-ignore: Design.OrphanExport: {filler}"),
            format!("// cofferdam-ignore-start: Design.OrphanExport {filler}"),
            format!("// cofferdam-ignore-end {filler}"),
            format!("// cofferdam-ignore-file: Design.OrphanExport {filler}"),
            format!("// cofferdam-disable-next-line Design.OrphanExport {filler}"),
            format!("/* cofferdam-disable Design.OrphanExport {filler} */"),
            format!("/* cofferdam-enable Design.OrphanExport {filler} */"),
            format!("// cofferdam-disable-file Design.OrphanExport {filler}"),
            // The only way to write one inside a JSX tree.
            format!("{{/* cofferdam-ignore: Design.OrphanExport: {filler} */}}"),
        ];
        for directive in directives {
            assert!(
                UnicodeWidthStr::width(directive.as_str()) as u32 > 120,
                "sanity: directive line must actually be over-limit: {directive}"
            );
            assert!(
                run_mll(&directive, 120).is_empty(),
                "directive spelling must be exempt: {directive}"
            );
        }
    }

    #[test]
    fn ordinary_comment_mentioning_word_ignore_still_flags() {
        // Doesn't open with a directive marker at all — a prose comment
        // that happens to use the word "ignore" is not a directive.
        let src = format!(
            "// please ignore how long this comment is, it is not a cofferdam directive {}",
            "x".repeat(50)
        );
        assert!(
            UnicodeWidthStr::width(src.as_str()) as u32 > 120,
            "sanity: comment line must actually be over-limit"
        );
        assert_eq!(
            run_mll(&src, 120).len(),
            1,
            "a prose comment that isn't a directive must still flag"
        );
    }

    // ── CD-318: unwrappable lines are skipped ─────────────────────────────

    #[test]
    fn string_literal_with_internal_spaces_not_flagged_by_default() {
        let filler = "lorem ipsum ".repeat(10);
        let src = format!("const msg = '{filler}';");
        assert!(
            UnicodeWidthStr::width(src.as_str()) as u32 > 120,
            "sanity: line must actually be over-limit"
        );
        let default_opts = mll_opts(120, false);
        assert!(
            run_mll_parsed(&src, &default_opts).is_empty(),
            "a string literal wider than the limit on its own can't be wrapped, so it must not flag"
        );

        let report_opts = mll_opts(120, true);
        let issues = run_mll_parsed(&src, &report_opts);
        assert_eq!(
            issues.len(),
            1,
            "report_unwrappable = true must restore flagging"
        );
    }

    #[test]
    fn tailwind_classname_string_not_flagged() {
        let classes = "flex items-center justify-between p-4 mt-8 mb-8 mx-auto max-w-7xl \
            text-center rounded-lg shadow-xl border border-gray-200 dark:border-gray-700 \
            bg-white dark:bg-gray-900";
        let src = format!("const el = <div className=\"{classes}\" />;");
        assert!(
            UnicodeWidthStr::width(src.as_str()) as u32 > 120,
            "sanity: line must actually be over-limit"
        );
        let opts = mll_opts(120, false);
        assert!(
            run_mll_parsed_path(&src, &opts, "test.tsx").is_empty(),
            "a Tailwind class-list string has no legal break point and must not flag"
        );
    }

    /// The dominant real-world case: a class list comfortably under the
    /// limit on its own, pushed over it by the attribute name and the
    /// nesting it sits in. Counting the indentation is what catches it —
    /// without that term this line reports and nobody can act on it.
    #[test]
    fn indented_classname_shorter_than_the_limit_is_still_unwrappable() {
        let classes = "border-t border-ink/20 px-4 py-3 text-sm text-fg dark:border-bone/20 \
             dark:text-fg-muted rounded-none bg-bone";
        let indent = " ".repeat(14);
        let src = format!("{indent}<div className=\"{classes}\">text</div>");
        assert!(
            UnicodeWidthStr::width(classes) as u32 <= 120,
            "sanity: the class list alone is under the limit — the indent is what breaches it"
        );
        assert!(
            UnicodeWidthStr::width(src.as_str()) as u32 > 120,
            "sanity: line must actually be over-limit"
        );
        let opts = mll_opts(120, false);
        assert!(
            run_mll_parsed_path(&src, &opts, "test.tsx").is_empty(),
            "an attribute a formatter can only move, not break, must not flag"
        );
    }

    /// JSX text is prose that reflows at spaces, exactly like a comment,
    /// so it must keep reporting. Treating it as atomic silenced a long
    /// paragraph in a component while the identical sentence in a comment
    /// two lines above still flagged.
    #[test]
    fn long_jsx_prose_still_flags() {
        let prose = "This is a very long paragraph of ordinary English prose that any \
             formatter in the world would happily reflow at a space.";
        let src = format!("const el = <p>{prose}</p>;");
        assert!(
            UnicodeWidthStr::width(src.as_str()) as u32 > 120,
            "sanity: line must actually be over-limit"
        );
        let opts = mll_opts(120, false);
        assert_eq!(
            run_mll_parsed_path(&src, &opts, "test.tsx").len(),
            1,
            "JSX prose is wrappable and must report"
        );
    }

    #[test]
    fn long_call_chain_of_short_tokens_still_flags() {
        let src = "const result = fooBarBaz(argOne, argTwo, argThree).quxQuux(argFour, argFive)\
            .someMethod(argSix, argSeven).anotherOne(argEight).done();";
        assert!(
            UnicodeWidthStr::width(src) as u32 > 120,
            "sanity: line must actually be over-limit"
        );
        let opts = mll_opts(120, false);
        assert_eq!(
            run_mll_parsed(src, &opts).len(),
            1,
            "many short, comma-separated tokens are wrappable and must still flag"
        );
    }

    #[test]
    fn long_prose_comment_still_flags() {
        let src = "// this is a long, ordinary prose comment made of short words that runs \
            well past the one hundred twenty column line length limit for this test case";
        assert!(
            UnicodeWidthStr::width(src) as u32 > 120,
            "sanity: line must actually be over-limit"
        );
        let opts = mll_opts(120, false);
        assert_eq!(
            run_mll_parsed(src, &opts).len(),
            1,
            "a comment's words are ordinary-width and human-wrappable, so it must still flag"
        );
    }

    #[test]
    fn unbreakable_long_identifier_not_flagged() {
        let src = "const x: ThisIsAVeryLongTypeNameThatExceedsOneHundredAndTwentyColumnsWhen\
            WrittenOutInFullWithoutAnySpacesAtAllHereTodayForSureYesReallyLong = 1;";
        assert!(
            UnicodeWidthStr::width(src) as u32 > 120,
            "sanity: line must actually be over-limit"
        );
        let opts = mll_opts(120, false);
        assert!(
            run_mll_parsed(src, &opts).is_empty(),
            "a single unbreakable identifier wider than the limit must not flag"
        );
    }

    #[test]
    fn long_import_path_string_not_flagged() {
        let src = "import { someExport } from \
            './some/very/long/module/path/that/keeps/going/and/going/and/going/and/going/\
            forever/across/many/nested/directories/and/subdirectories/mod';";
        assert!(
            UnicodeWidthStr::width(src) as u32 > 120,
            "sanity: line must actually be over-limit"
        );
        let opts = mll_opts(120, false);
        assert!(
            run_mll_parsed(src, &opts).is_empty(),
            "an unbreakable import-path string literal must not flag"
        );
    }

    #[test]
    fn unparsed_file_falls_back_to_flagging_every_over_limit_line() {
        let filler = "lorem ipsum ".repeat(10);
        let src = format!("const msg = '{filler}';");
        assert_eq!(
            run_mll(&src, 120).len(),
            1,
            "without ctx.parsed, MLL has no atomic-token info and must flag every over-limit line"
        );
    }

    #[test]
    fn boundary_token_width_exactly_limit_reports_one_more_skips() {
        let opts = mll_opts(120, false);

        let at_limit = format!("x {}", "a".repeat(120));
        assert_eq!(
            run_mll_parsed(&at_limit, &opts).len(),
            1,
            "a token whose width exactly equals the limit is still wrappable-adjacent and must flag"
        );

        let one_more = "a".repeat(121);
        assert!(
            run_mll_parsed(&one_more, &opts).is_empty(),
            "one column over the limit tips the token into unwrappable"
        );
    }
}
