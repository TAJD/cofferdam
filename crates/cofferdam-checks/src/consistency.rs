//! Consistency checks. Two-pass mode: pass 1 collects per-file evidence;
//! pass 2 emits findings against the collected baseline.

use std::collections::HashMap;
use std::path::PathBuf;

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    Category, Check, CheckContext, CheckMeta, CorpusKey, Issue, Priority, Severity, SourceFile,
    Span,
};
use oxc_ast::ast::{JSXAttributeValue, StringLiteral};
use oxc_ast_visit::Visit;

// ─── Consistency.QuoteStyle ─────────────────────────────────────────────────

/// Per-file statistics collected during pass 1.
#[derive(Default, Clone)]
struct FileQuoteStats {
    /// Count of single-quoted string literals in this file.
    single: u32,
    /// Count of double-quoted string literals in this file.
    double: u32,
    /// Spans of ALL observed string literals, tagged with their quote char.
    /// Pass 2 picks the dominant style and re-filters.
    spans: Vec<(Span, u8)>, // (span, quote_byte: b'\'' or b'"')
}

/// Corpus slot: keyed by canonical file path.
static QUOTE_STATS: CorpusKey<HashMap<PathBuf, FileQuoteStats>> =
    CorpusKey::new("Consistency.QuoteStyle.stats");

pub struct QuoteStyle;

const META: CheckMeta = CheckMeta {
    id: "Consistency.QuoteStyle",
    category: Category::Consistency,
    base_priority: -5,
    default_severity: Severity::Info,
    explanation: "Mixed quote styles within a file hurt scanability. Use a consistent quote character (single or double) throughout.",
    body: include_str!("../docs/Consistency.QuoteStyle.md"),
    requires_types: false,
    consistency: true,
    options: &[],
};

impl Check for QuoteStyle {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    /// Pass 1: walk AST and collect per-file quote-usage statistics.
    /// Skips JSX attribute string values (they have different style rules)
    /// and strings whose content forces the alternate quote (e.g. `"don't"`).
    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };

        let mut collector = QuoteCollector {
            file,
            stats: FileQuoteStats::default(),
        };
        collector.visit_program(parsed.program);
        let stats = collector.stats;

        ctx.corpus.with_slot(&QUOTE_STATS, |slot| {
            slot.insert(file.path.clone(), stats);
        });

        Vec::new()
    }

    /// Pass 2: read the corpus slot for this file, determine dominant style,
    /// emit one issue per span that deviates from the dominant.
    fn pass2(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let stats = ctx
            .corpus
            .with_slot(&QUOTE_STATS, |slot| slot.get(&file.path).cloned());

        let stats = match stats {
            Some(s) => s,
            None => return Vec::new(),
        };

        let total = stats.single + stats.double;
        if total == 0 {
            return Vec::new();
        }

        // Strict majority required: >50% of total. Ties or one-sided files
        // emit nothing.
        let dominant = if stats.single > total / 2 && stats.single > stats.double {
            b'\''
        } else if stats.double > total / 2 && stats.double > stats.single {
            b'"'
        } else {
            // 50/50 tie or one side is zero (can't tie with zero, but be safe)
            return Vec::new();
        };

        let dominant_name = if dominant == b'\'' {
            "single"
        } else {
            "double"
        };

        let mut issues = Vec::new();
        for (span, quote) in &stats.spans {
            if *quote != dominant {
                issues.push(Issue {
                    check_id: META.id.to_string(),
                    message: format!(
                        "use {dominant_name} quotes consistently (dominant style in this file)",
                    ),
                    file: file.path.clone(),
                    span: *span,
                    priority: Priority(META.base_priority),
                    severity: Severity::Info,
                    related: Vec::new(),
                });
            }
        }
        issues
    }
}

// ─── Visitor ────────────────────────────────────────────────────────────────

struct QuoteCollector<'a> {
    file: &'a SourceFile,
    stats: FileQuoteStats,
}

impl<'a> QuoteCollector<'a> {
    fn record_literal(&mut self, lit: &StringLiteral<'a>) {
        // Determine the quote character from the source byte at span.start.
        let start = lit.span.start as usize;
        let src_bytes = self.file.text.as_bytes();
        let quote = match src_bytes.get(start) {
            Some(&b'\'') => b'\'',
            Some(&b'"') => b'"',
            // Backtick = template literal (shouldn't appear here since
            // StringLiteral excludes templates), or synthetic node — skip.
            _ => return,
        };

        // Skip strings whose content requires the alternate quote to avoid
        // spurious escaping. E.g. `"don't"` legitimately uses double quotes
        // because the content contains a single quote. Flagging it would
        // force the developer to write `'don\'t'` — pure churn.
        //
        // Strategy: check whether the raw value contains the dominant-style
        // quote character. If the string contains the *other* quote, we'd be
        // forcing an escape — skip it.
        let value_str = lit.value.as_str();
        let alternate = if quote == b'\'' { b'"' } else { b'\'' };
        // If the value contains the alternate quote character *and* the
        // string uses the alternate quote, that means the content forces
        // use of the current quote to avoid escaping. We can determine this
        // without knowing the dominant style yet — we just track whether the
        // string's *alternate* quote appears in its content:
        if value_str.as_bytes().contains(&alternate) {
            // Using this quote style because switching would require escaping.
            // Still count it (it's a valid string) but flag it as "forced"
            // by tagging it with a sentinel (we don't include it in spans
            // to avoid flagging it in pass 2).
            // For the purposes of dominant-style counting we include it,
            // but we do NOT add it to `spans` so pass 2 won't flag it.
            match quote {
                b'\'' => self.stats.single += 1,
                b'"' => self.stats.double += 1,
                _ => {}
            }
            return;
        }

        let span = span_from_bytes(&self.file.text, lit.span.start, lit.span.end);
        match quote {
            b'\'' => self.stats.single += 1,
            b'"' => self.stats.double += 1,
            _ => {}
        }
        self.stats.spans.push((span, quote));
    }
}

impl<'a> Visit<'a> for QuoteCollector<'a> {
    fn visit_string_literal(&mut self, lit: &StringLiteral<'a>) {
        self.record_literal(lit);
        // StringLiteral has no sub-nodes that need walking.
        oxc_ast_visit::walk::walk_string_literal(self, lit);
    }

    /// Override JSX attribute value visitor to skip string literals that
    /// appear as JSX attribute values (e.g. `<Foo bar="value" />`).
    /// JSX attribute strings follow different style conventions and must
    /// not influence the per-file quote stats.
    fn visit_jsx_attribute_value(&mut self, _it: &JSXAttributeValue<'a>) {
        // Intentionally do NOT walk into JSX attribute values — we skip
        // their string literals entirely.
    }
}

// ─── Consistency.BroadSuppression ──────────────────────────────────────────

/// Flags `// cofferdam-ignore` (with no check id) — the broad form
/// silences every check on the next non-blank line, which makes
/// suppression audits hard. Per cd-81a.4: the engine accepts the broad
/// form but emits an info-level diagnostic at the directive line so
/// users notice and (usually) tighten it to `// cofferdam-ignore: <id>`.
///
/// Self-suppression is possible via the explicit form
/// `// cofferdam-ignore: Consistency.BroadSuppression: <reason>` — the
/// broad form on a previous line never suppresses this check on the
/// same line (suppression targets the next non-blank line, not the
/// directive line itself), which is what makes flagging on the
/// directive line the right anchor.
pub struct BroadSuppression;

const BS_META: CheckMeta = CheckMeta {
    id: "Consistency.BroadSuppression",
    category: Category::Consistency,
    base_priority: 0,
    default_severity: Severity::Info,
    explanation: "Broad-form `// cofferdam-ignore` (no check id) silences every check on the next line. Tighten to `// cofferdam-ignore: <CheckId>: <reason>` so suppression intent is auditable.",
    body: include_str!("../docs/Consistency.BroadSuppression.md"),
    requires_types: false,
    consistency: false,
    options: &[],
};

impl Check for BroadSuppression {
    fn meta(&self) -> &'static CheckMeta {
        &BS_META
    }

    fn run(&self, file: &SourceFile, _ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let mut out = Vec::new();
        let mut byte_offset: u32 = 0;

        for (line_no, line) in file.lines() {
            if let Some(directive_col) = find_broad_suppression(line) {
                let start = byte_offset + directive_col as u32;
                let end = byte_offset + line.len() as u32;
                out.push(Issue {
                    check_id: BS_META.id.to_string(),
                    message: "Broad `// cofferdam-ignore` (no check id) — narrow it to a specific id and add a reason.".to_string(),
                    file: file.path.clone(),
                    span: Span {
                        start_byte: start,
                        end_byte: end,
                        line: line_no,
                        column: directive_col as u32 + 1,
                    },
                    priority: Priority(BS_META.base_priority),
                    severity: BS_META.default_severity,
                    related: Vec::new(),
                });
            }
            byte_offset = byte_offset.saturating_add(line.len() as u32 + 1);
        }

        out
    }
}

/// If `line` is a Biome-style broad suppression (`cofferdam-ignore` with
/// no following `:` and no `-start` / `-end` / `-file` variant), return
/// the byte column where the marker starts. Returns `None` for the
/// scoped form, the multi-line variants, and lines that mention the
/// directive only in prose. The directive must be the comment's first
/// non-whitespace token to count.
fn find_broad_suppression(line: &str) -> Option<usize> {
    let needle = "cofferdam-ignore";
    let leading_ws = line.len() - line.trim_start().len();
    let trimmed = &line[leading_ws..];

    let after_marker = trimmed
        .strip_prefix("//")
        .or_else(|| trimmed.strip_prefix("/*"))?;

    let inner = after_marker.trim_start();
    let directive_offset_in_inner = after_marker.len() - inner.len();

    if !inner.starts_with(needle) {
        return None;
    }
    let after = &inner[needle.len()..];

    if after.starts_with("-start") || after.starts_with("-end") || after.starts_with("-file") {
        return None;
    }

    if after.trim_start().starts_with(':') {
        return None;
    }

    let comment_marker_len = trimmed.len() - after_marker.len();
    Some(leading_ws + comment_marker_len + directive_offset_in_inner)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::parser::{parse_into, ParsedView};
    use cofferdam_core::{Allocator, Check, CheckContext, CorpusIndex, SourceFile};
    use std::path::PathBuf;

    /// Run QuoteStyle against `src` (full two-pass cycle) and return the issues
    /// emitted by pass 2.
    fn run_quote_style(src: &str) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from("test.ts"), src);
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let corpus = CorpusIndex::new();
        let check = QuoteStyle;

        // Pass 1: collect stats.
        {
            let mut ctx = CheckContext::new(&file)
                .with_parsed(&parsed)
                .with_corpus(&corpus);
            let p1 = check.run(&file, &mut ctx);
            assert!(p1.is_empty(), "pass 1 should emit no issues");
        }

        // Pass 2: emit findings.
        {
            let mut ctx = CheckContext::new(&file)
                .with_parsed(&parsed)
                .with_corpus(&corpus);
            check.pass2(&file, &mut ctx)
        }
    }

    #[test]
    fn five_single_one_double_emits_one_finding() {
        // 5 single-quoted, 1 double-quoted → double is the deviant.
        let src = r#"
const a = 'one';
const b = 'two';
const c = 'three';
const d = 'four';
const e = 'five';
const f = "six";
"#;
        let issues = run_quote_style(src);
        assert_eq!(
            issues.len(),
            1,
            "expected 1 issue for the one double-quoted string; got: {:?}",
            issues
        );
        // The issue should point at the "six" string.
        let idx = src
            .find("\"six\"")
            .expect("test fixture must contain '\"six\"'");
        assert_eq!(
            issues[0].span.start_byte as usize, idx,
            "issue should point at the double-quoted string"
        );
    }

    #[test]
    fn fifty_fifty_emits_no_findings() {
        // 2 single, 2 double → tie → no findings.
        let src = r#"
const a = 'one';
const b = 'two';
const c = "three";
const d = "four";
"#;
        let issues = run_quote_style(src);
        assert!(
            issues.is_empty(),
            "50/50 tie should emit no findings; got: {:?}",
            issues
        );
    }

    #[test]
    fn forced_escape_string_not_flagged() {
        // `"don't"` uses double quotes because the content contains a single
        // quote. It must NOT be flagged even when single-quoted strings dominate.
        let src = r#"
const a = 'one';
const b = 'two';
const c = 'three';
const d = 'four';
const e = 'five';
const forced = "don't";
"#;
        // The dominant style is single (5 singles, 1 forced-double).
        // The forced-double should NOT be flagged.
        let issues = run_quote_style(src);
        assert!(
            issues.is_empty(),
            "forced-escape string should not be flagged; got: {:?}",
            issues
        );
    }

    #[test]
    fn all_same_style_emits_no_findings() {
        let src = r#"
const a = 'one';
const b = 'two';
const c = 'three';
"#;
        let issues = run_quote_style(src);
        assert!(
            issues.is_empty(),
            "uniform style should emit no findings; got: {:?}",
            issues
        );
    }

    /// Run QuoteStyle against `src` as a `.tsx` file (JSX-enabled parser).
    fn run_quote_style_tsx(src: &str) -> Vec<Issue> {
        let file = SourceFile::new(PathBuf::from("test.tsx"), src);
        let allocator = Allocator::default();
        let parser_return = parse_into(&allocator, &file);
        let parsed = ParsedView {
            program: &parser_return.program,
            diagnostics: &parser_return.errors,
        };
        let corpus = CorpusIndex::new();
        let check = QuoteStyle;
        {
            let mut ctx = CheckContext::new(&file)
                .with_parsed(&parsed)
                .with_corpus(&corpus);
            check.run(&file, &mut ctx);
        }
        {
            let mut ctx = CheckContext::new(&file)
                .with_parsed(&parsed)
                .with_corpus(&corpus);
            check.pass2(&file, &mut ctx)
        }
    }

    #[test]
    fn jsx_attribute_string_not_counted() {
        // JSX attribute string literals must not be counted or flagged.
        // 5 non-JSX single-quoted strings dominate; 1 non-JSX double-quoted
        // string should be flagged; the JSX attr `className="ignored"` must
        // NOT be counted as a double-quoted string (and must NOT be flagged).
        let src = r#"
const a = 'one';
const b = 'two';
const c = 'three';
const d = 'four';
const e = 'five';
const bad = "deviant";
const el = <Foo className="ignored" />;
"#;
        let issues = run_quote_style_tsx(src);
        // Only "deviant" should be flagged (JSX attr is ignored).
        assert_eq!(
            issues.len(),
            1,
            "only the non-JSX double-quoted string should be flagged; got: {:?}",
            issues
        );
    }
}
