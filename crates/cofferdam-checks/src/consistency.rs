//! Consistency checks. Two-pass mode: pass 1 collects per-file evidence;
//! pass 2 emits findings against the collected baseline.

use std::collections::HashMap;
use std::path::PathBuf;

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    looks_like_check_id, Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext,
    Issue, Location, Priority, Severity, SourceFile, Span, ALL_PRE_FILTER_FINDINGS,
    REGISTERED_CHECK_IDS,
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

/// `Consistency.QuoteStyle` — two-pass check that learns the
/// dominant quote style per file in pass 1 and flags deviations in
/// pass 2. See `CheckMeta` and the per-check docs page for the full
/// emission contract.
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
    autofix: false,
    pure_run: false,
};

impl Check for QuoteStyle {
    fn meta(&self) -> &'static CheckMeta {
        &META
    }

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&QUOTE_STATS, |slot, path| {
            slot.remove(path);
        });
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
                    location: Location::from_span(&file.path, *span),
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
    explanation: "Broad-form `// cofferdam-ignore` (no check id) silences every check on the next line. Tighten to a scoped form so suppression intent is auditable: `// cofferdam-ignore: <CheckId>: <reason>` (colon-separator) or `// cofferdam-ignore <CheckId> — <reason>` (space-separator, em-dash or hyphen reason).",
    body: include_str!("../docs/Consistency.BroadSuppression.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: false,
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
                    message: "Broad `// cofferdam-ignore` (no check id) — narrow it to a specific id. Accepted forms: `// cofferdam-ignore: <CheckId>: <reason>` or `// cofferdam-ignore <CheckId> — <reason>`.".to_string(),
                    file: file.path.clone(),
                    location: Location::from_span(&file.path, Span {
                        start_byte: start,
                        end_byte: end,
                        line: line_no,
                        column: directive_col as u32 + 1,
                    }),
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
/// scoped form (colon or space-separator with a check-id-shaped first
/// token — cd-b77 / gh #42), the multi-line variants, and lines that
/// mention the directive only in prose. The directive must be the
/// comment's first non-whitespace token to count.
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

    // Colon form: `cofferdam-ignore: <Id>...` is scoped (not broad).
    if after.trim_start().starts_with(':') {
        return None;
    }

    // Space form: `cofferdam-ignore <CheckId> [— reason]` is also
    // scoped — the suppression parser now binds the first token as the
    // id (cd-b77). Match the same heuristic here so we don't flag a
    // directive that the engine actually treats as scoped.
    let stripped = after.trim_start().trim_end_matches("*/").trim();
    if let Some(first) = stripped.split_whitespace().next() {
        let candidate = first.trim_end_matches(':');
        if looks_like_check_id(candidate) {
            return None;
        }
    }

    let comment_marker_len = trimmed.len() - after_marker.len();
    Some(leading_ws + comment_marker_len + directive_offset_in_inner)
}

// ─── Consistency.UnusedSuppression ─────────────────────────────────────────

/// Kind of suppression directive, carrying the line range it targets.
#[derive(Debug, Clone)]
enum DirectiveKind {
    /// `// cofferdam-ignore: <id>` — silences the next non-blank line.
    NextLine { target_line: u32 },
    /// `// cofferdam-ignore-start: <id>` … `// cofferdam-ignore-end` — silences a range.
    Range { start_line: u32, end_line: u32 },
    /// `// cofferdam-ignore-file: <id>` — silences the whole file.
    File,
}

/// One parsed suppression directive with its location in the file.
#[derive(Debug, Clone)]
struct PerFileDirective {
    check_id: String,
    kind: DirectiveKind,
    /// 1-based line of the directive itself (where the issue will point).
    directive_line: u32,
    start_byte: u32,
    end_byte: u32,
}

/// Corpus slot: per-file parsed directives collected during pass 1 (`run`).
/// The slot is populated by `UnusedSuppression::run` and consumed in `finalize`.
static SUPPRESSION_DIRECTIVES: CorpusKey<HashMap<PathBuf, Vec<PerFileDirective>>> =
    CorpusKey::new("Consistency.UnusedSuppression.directives");

/// `Consistency.UnusedSuppression` — second-phase finalize observer
/// that flags `// cofferdam-ignore` directives whose target check
/// emitted no finding on the suppressed line. See `CheckMeta` for the
/// two-phase finalize semantics and `FINALIZE_OBSERVER_CHECK_IDS`.
pub struct UnusedSuppression;

const US_META: CheckMeta = CheckMeta {
    id: "Consistency.UnusedSuppression",
    category: Category::Consistency,
    base_priority: -5,
    default_severity: Severity::Low,
    explanation: "A `cofferdam-ignore` directive (next-line, range, or file-wide) targets a check ID that has no current finding in scope. The underlying issue was likely fixed or the code was deleted — the directive is now dead weight.",
    body: include_str!("../docs/Consistency.UnusedSuppression.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: false,
};

impl Check for UnusedSuppression {
    fn meta(&self) -> &'static CheckMeta {
        &US_META
    }

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&SUPPRESSION_DIRECTIVES, |slot, path| {
            slot.remove(path);
        });
    }

    /// Pass 1: scan the file text for scoped suppression directives and
    /// store them into the corpus so `finalize` can compare against findings.
    /// Broad-form directives (no check id) are skipped — that's
    /// `Consistency.BroadSuppression`'s territory.
    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let directives = parse_scoped_directives(&file.text);
        ctx.corpus.with_slot(&SUPPRESSION_DIRECTIVES, |slot| {
            slot.insert(file.path.clone(), directives);
        });
        Vec::new()
    }

    /// Post-run: read pre-filter findings + per-file directives; emit one
    /// finding per directive that covers zero matching findings.
    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        // Read the set of known check IDs. A directive targeting an unknown
        // check ID is `Consistency.UnknownCheckId`'s territory — skip it.
        let known_ids = ctx
            .corpus
            .with_slot(&REGISTERED_CHECK_IDS, |ids| ids.clone());

        // Read the pre-filter findings map.
        let findings_by_file = ctx
            .corpus
            .with_slot(&ALL_PRE_FILTER_FINDINGS, |m| m.clone());

        // Read the suppression directives collected during run().
        let directives_by_file = ctx.corpus.with_slot(&SUPPRESSION_DIRECTIVES, |m| m.clone());

        let mut out = Vec::new();

        for (file_path, directives) in &directives_by_file {
            // findings for this file: list of (check_id, line) pairs
            let empty = Vec::new();
            let findings = findings_by_file.get(file_path).unwrap_or(&empty);

            for directive in directives {
                // Skip if the targeted check ID is not registered — unknown
                // check id linting is a separate concern.
                if !known_ids.contains(&directive.check_id) {
                    continue;
                }

                let has_match = match &directive.kind {
                    DirectiveKind::NextLine { target_line } => findings
                        .iter()
                        .any(|(id, line)| id == &directive.check_id && line == target_line),
                    DirectiveKind::Range {
                        start_line,
                        end_line,
                    } => findings.iter().any(|(id, line)| {
                        id == &directive.check_id && line >= start_line && line <= end_line
                    }),
                    DirectiveKind::File => findings.iter().any(|(id, _)| id == &directive.check_id),
                };

                if !has_match {
                    out.push(Issue {
                        check_id: US_META.id.to_string(),
                        message: format!(
                            "suppression directive for `{}` covers no findings — the directive is stale",
                            directive.check_id,
                        ),
                        file: file_path.clone(),
                        location: Location::from_span(file_path, Span {
                            start_byte: directive.start_byte,
                            end_byte: directive.end_byte,
                            line: directive.directive_line,
                            column: 1,
                        }),
                        priority: Priority(US_META.base_priority),
                        severity: US_META.default_severity,
                        related: Vec::new(),
                    });
                }
            }
        }

        out
    }
}

/// Parse all scoped (named) suppression directives from `text`.
/// Returns one `PerFileDirective` per directive that names a check ID.
/// Broad-form directives and ESLint-style `disable`/`enable` blocks are
/// intentionally excluded — they are `Consistency.BroadSuppression`'s
/// territory or future work.
fn parse_scoped_directives(text: &str) -> Vec<PerFileDirective> {
    let mut out: Vec<PerFileDirective> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    /// Active block: (check_id, 1-based start line of the -ignore-start directive, start_byte, end_byte)
    struct ActiveBlock {
        check_id: String,
        start_line: u32,
        start_byte: u32,
        end_byte: u32,
    }
    let mut active_blocks: Vec<ActiveBlock> = Vec::new();

    let mut byte_offset: u32 = 0;

    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as u32;
        let trimmed = line.trim();
        let line_start = byte_offset;
        let line_end = byte_offset + line.len() as u32;
        byte_offset = byte_offset.saturating_add(line.len() as u32 + 1); // +1 for \n

        // ── cofferdam-ignore-file: <id> ──────────────────────────────────────
        if let Some(id) = extract_file_directive(trimmed) {
            if !id.is_empty() {
                out.push(PerFileDirective {
                    check_id: id,
                    kind: DirectiveKind::File,
                    directive_line: line_num,
                    start_byte: line_start,
                    end_byte: line_end,
                });
            }
            continue;
        }

        // ── cofferdam-ignore-end ──────────────────────────────────────────────
        if is_block_end(trimmed) {
            // Close and record the most recent matching open block (any id since
            // -ignore-end has no id in the Biome grammar).
            if let Some(block) = active_blocks.pop() {
                out.push(PerFileDirective {
                    check_id: block.check_id,
                    kind: DirectiveKind::Range {
                        start_line: block.start_line,
                        end_line: line_num.saturating_sub(1), // up to the line before -end
                    },
                    directive_line: block.start_line,
                    start_byte: block.start_byte,
                    end_byte: block.end_byte,
                });
            }
            continue;
        }

        // ── cofferdam-ignore-start: <id> ─────────────────────────────────────
        if let Some(id) = extract_block_start(trimmed) {
            if !id.is_empty() {
                active_blocks.push(ActiveBlock {
                    check_id: id,
                    start_line: line_num,
                    start_byte: line_start,
                    end_byte: line_end,
                });
            }
            continue;
        }

        // ── cofferdam-ignore: <id> (next-line form) ──────────────────────────
        if let Some(id) = extract_next_line_directive(trimmed) {
            if !id.is_empty() {
                // Find next non-blank line number. If there is no next
                // non-blank line the directive targets nothing — use a
                // sentinel that will never match a finding so it is
                // correctly flagged as stale.
                let target = find_next_non_blank(idx, &lines).unwrap_or(u32::MAX);
                out.push(PerFileDirective {
                    check_id: id,
                    kind: DirectiveKind::NextLine {
                        target_line: target,
                    },
                    directive_line: line_num,
                    start_byte: line_start,
                    end_byte: line_end,
                });
            }
        }
    }

    // Any still-open blocks (missing -ignore-end) are treated as ranging to EOF.
    let total_lines = lines.len() as u32;
    for block in active_blocks {
        out.push(PerFileDirective {
            check_id: block.check_id,
            kind: DirectiveKind::Range {
                start_line: block.start_line,
                end_line: total_lines,
            },
            directive_line: block.start_line,
            start_byte: block.start_byte,
            end_byte: block.end_byte,
        });
    }

    out
}

/// If `trimmed` is a `cofferdam-ignore-file: <id>` directive, return the id.
/// Returns `Some("")` for the broad file form (no id) so the caller can skip it.
/// Returns `None` if not a file directive at all.
fn extract_file_directive(trimmed: &str) -> Option<String> {
    let needle = "cofferdam-ignore-file";
    let idx = trimmed.find(needle)?;
    let after = &trimmed[idx + needle.len()..];
    let after = after.trim_start();
    // Broad file form — no colon or colon with no id.
    if !after.starts_with(':') {
        return Some(String::new()); // broad form → skip
    }
    let payload = after[1..].trim().trim_end_matches("*/").trim();
    if payload.is_empty() {
        return Some(String::new()); // broad form
    }
    let id = match payload.split_once(':') {
        Some((id, _)) => id.trim(),
        None => payload,
    };
    if id.is_empty() {
        Some(String::new())
    } else {
        Some(id.to_string())
    }
}

/// True if `trimmed` is a `cofferdam-ignore-end` line (Biome range closer).
fn is_block_end(trimmed: &str) -> bool {
    trimmed.contains("cofferdam-ignore-end")
}

/// If `trimmed` opens a Biome range block (`cofferdam-ignore-start: <id>`),
/// return the check id. Returns `Some("")` for the broad block form.
fn extract_block_start(trimmed: &str) -> Option<String> {
    let needle = "cofferdam-ignore-start";
    let idx = trimmed.find(needle)?;
    let after = &trimmed[idx + needle.len()..];
    let after = after.trim_start();
    if !after.starts_with(':') {
        return Some(String::new()); // broad block — skip
    }
    let payload = after[1..].trim().trim_end_matches("*/").trim();
    if payload.is_empty() {
        return Some(String::new());
    }
    let id = match payload.split_once(':') {
        Some((id, _)) => id.trim(),
        None => payload,
    };
    if id.is_empty() {
        Some(String::new())
    } else {
        Some(id.to_string())
    }
}

/// If `trimmed` is a Biome next-line form (`cofferdam-ignore: <id>` or
/// the cd-b77 `cofferdam-ignore <id> [— reason]` space form), return
/// the check id. Returns `Some("")` for the broad form (no id, or
/// prose comment that happens to mention the marker). Returns
/// `None` for lines that aren't a next-line directive at all.
fn extract_next_line_directive(trimmed: &str) -> Option<String> {
    let needle = "cofferdam-ignore";
    let idx = trimmed.find(needle)?;
    let after = &trimmed[idx + needle.len()..];

    // Reject multi-form variants handled elsewhere.
    if after.starts_with("-start") || after.starts_with("-end") || after.starts_with("-file") {
        return None;
    }
    let after_trim = after.trim_start();
    if !after_trim.starts_with(':') {
        // No leading colon. Check for the space-separator scoped form
        // (cd-b77): a check-id-shaped first token binds the id.
        let stripped = after_trim.trim_end_matches("*/").trim();
        if let Some(raw) = stripped.split_whitespace().next() {
            let candidate = raw.trim_end_matches(':');
            if looks_like_check_id(candidate) {
                return Some(candidate.to_string());
            }
        }
        // Otherwise: broad next-line form — skip (BroadSuppression's territory).
        return Some(String::new());
    }
    let payload = after_trim[1..].trim().trim_end_matches("*/").trim();
    if payload.is_empty() {
        return Some(String::new()); // broad form with colon but no id
    }
    let id = match payload.split_once(':') {
        Some((id, _)) => id.trim(),
        None => payload,
    };
    if id.is_empty() {
        Some(String::new())
    } else {
        Some(id.to_string())
    }
}

/// Return the 1-based line number of the first non-blank line after `from_idx`.
/// `from_idx` is 0-based (the index of the directive line in `lines`).
fn find_next_non_blank(from_idx: usize, lines: &[&str]) -> Option<u32> {
    for (offset, line) in lines.iter().skip(from_idx + 1).enumerate() {
        if !line.trim().is_empty() {
            return Some((from_idx + offset + 2) as u32);
        }
    }
    None
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
            issues[0].location.start_byte() as usize,
            idx,
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

    // ── UnusedSuppression tests ──────────────────────────────────────────────

    /// Helper: run UnusedSuppression's `run()` pass for `src` and then call
    /// `finalize()` with a corpus that has been seeded with the given pre-filter
    /// findings `[(check_id, line_1based)]` for the test file path, plus the
    /// given set of known check IDs.
    fn run_unused_suppression(
        src: &str,
        pre_filter_findings: &[(&str, u32)],
        known_check_ids: &[&str],
    ) -> Vec<Issue> {
        let path = PathBuf::from("test.ts");
        let file = SourceFile::new(path.clone(), src);
        let corpus = CorpusIndex::new();
        let check = UnusedSuppression;

        // Seed REGISTERED_CHECK_IDS.
        {
            let ids: std::collections::HashSet<String> =
                known_check_ids.iter().map(|s| s.to_string()).collect();
            corpus.with_slot(&REGISTERED_CHECK_IDS, |slot| *slot = ids);
        }

        // Seed ALL_PRE_FILTER_FINDINGS.
        {
            let mut map: std::collections::HashMap<std::path::PathBuf, Vec<(String, u32)>> =
                std::collections::HashMap::new();
            map.insert(
                path.clone(),
                pre_filter_findings
                    .iter()
                    .map(|(id, line)| (id.to_string(), *line))
                    .collect(),
            );
            corpus.with_slot(&ALL_PRE_FILTER_FINDINGS, |slot| *slot = map);
        }

        // Run pass 1 (populates SUPPRESSION_DIRECTIVES corpus slot).
        {
            let mut ctx = CheckContext::new(&file).with_corpus(&corpus);
            let pass1 = check.run(&file, &mut ctx);
            assert!(pass1.is_empty(), "run() must always return empty");
        }

        // Run finalize.
        {
            let mut finalize_ctx = cofferdam_core::FinalizeContext::new(&corpus);
            check.finalize(&mut finalize_ctx)
        }
    }

    #[test]
    fn flags_unused_next_line_directive() {
        // `// cofferdam-ignore: Warning.NoConsoleLog` on a line that has no
        // finding for that check → stale directive.
        let src = "// cofferdam-ignore: Warning.NoConsoleLog: suppressed\nconst x = 1;";
        // pre-filter findings: no NoConsoleLog finding on line 2.
        let issues = run_unused_suppression(src, &[], &["Warning.NoConsoleLog"]);
        assert_eq!(
            issues.len(),
            1,
            "should flag the stale next-line directive; got: {:?}",
            issues
        );
        assert_eq!(issues[0].check_id, "Consistency.UnusedSuppression");
        assert!(
            issues[0].message.contains("Warning.NoConsoleLog"),
            "message should name the targeted check; got: {}",
            issues[0].message
        );
        assert_eq!(
            issues[0].location.line(),
            1,
            "issue should point at the directive line"
        );
    }

    #[test]
    fn does_not_flag_when_finding_present() {
        // Same directive but with a matching pre-filter finding on line 2.
        let src = "// cofferdam-ignore: Warning.NoConsoleLog: reason\nconsole.log('hi');";
        let issues = run_unused_suppression(
            src,
            &[("Warning.NoConsoleLog", 2)],
            &["Warning.NoConsoleLog"],
        );
        assert!(
            issues.is_empty(),
            "should NOT flag when a matching finding is present; got: {:?}",
            issues
        );
    }

    #[test]
    fn flags_unused_range_with_no_findings_inside() {
        // `cofferdam-ignore-start: Warning.NoEval` .. `cofferdam-ignore-end`
        // with no eval inside → stale range directive.
        let src =
            "// cofferdam-ignore-start: Warning.NoEval\nconst safe = 1;\n// cofferdam-ignore-end\n";
        let issues = run_unused_suppression(src, &[], &["Warning.NoEval"]);
        assert_eq!(
            issues.len(),
            1,
            "should flag the stale range directive; got: {:?}",
            issues
        );
        assert_eq!(
            issues[0].location.line(),
            1,
            "issue should point at the -start line"
        );
        assert!(issues[0].message.contains("Warning.NoEval"));
    }

    #[test]
    fn flags_unused_file_directive() {
        // `cofferdam-ignore-file: Warning.TripleEquals` in a file with no == / !=.
        let src = "// cofferdam-ignore-file: Warning.TripleEquals\nconst x = 1;\n";
        let issues = run_unused_suppression(src, &[], &["Warning.TripleEquals"]);
        assert_eq!(
            issues.len(),
            1,
            "should flag the stale file directive; got: {:?}",
            issues
        );
        assert_eq!(issues[0].location.line(), 1);
        assert!(issues[0].message.contains("Warning.TripleEquals"));
    }

    #[test]
    fn silent_when_check_id_unknown_to_engine() {
        // `cofferdam-ignore: Custom.NotARealCheck` — check not registered.
        // Must NOT emit a finding (that's Consistency.UnknownCheckId's job).
        let src = "// cofferdam-ignore: Custom.NotARealCheck: reason\nconst x = 1;";
        // known_check_ids does NOT include Custom.NotARealCheck.
        let issues = run_unused_suppression(src, &[], &["Warning.NoConsoleLog"]);
        assert!(
            issues.is_empty(),
            "must not flag directives targeting unknown check IDs; got: {:?}",
            issues
        );
    }

    #[test]
    fn silent_for_broad_suppression() {
        // `// cofferdam-ignore` (no check id) — broad form.
        // UnusedSuppression must NOT emit a finding for these.
        let src = "// cofferdam-ignore\nconst x = 1;";
        let issues = run_unused_suppression(src, &[], &["Warning.NoConsoleLog"]);
        assert!(
            issues.is_empty(),
            "broad-form directive must not be flagged by UnusedSuppression; got: {:?}",
            issues
        );
    }

    // ---- cd-b77 / gh #42: space-separator next-line form ----

    #[test]
    fn broad_suppression_silent_on_space_form_em_dash() {
        // The form reported in gh #42: `cofferdam-ignore <Id> — reason`.
        // BroadSuppression must NOT fire on it (the suppression parser
        // binds the id; flagging it as broad is contradictory).
        let line = "// cofferdam-ignore Design.OrphanExport — Vite entry";
        assert!(
            find_broad_suppression(line).is_none(),
            "space-form scoped directive must not be flagged broad",
        );
    }

    #[test]
    fn broad_suppression_silent_on_space_form_ascii_hyphen() {
        let line = "// cofferdam-ignore Warning.TripleEquals - intentional";
        assert!(find_broad_suppression(line).is_none());
    }

    #[test]
    fn broad_suppression_silent_on_space_form_colon_reason() {
        let line = "// cofferdam-ignore Design.MaxParameters: refactor pending";
        assert!(find_broad_suppression(line).is_none());
    }

    #[test]
    fn broad_suppression_still_fires_on_truly_broad() {
        // No id, no separator — really broad. Must still fire.
        let line = "// cofferdam-ignore";
        assert!(find_broad_suppression(line).is_some());
    }

    #[test]
    fn broad_suppression_fires_on_prose() {
        // The first token isn't check-id-shaped — treat as prose and
        // emit the broad-form nudge.
        let line = "// cofferdam-ignore please understand this is intentional";
        assert!(find_broad_suppression(line).is_some());
    }

    #[test]
    fn unused_suppression_extracts_id_from_space_form() {
        // `extract_next_line_directive` returns the id for the space form,
        // so UnusedSuppression can match findings against it the same way
        // it does for the colon form.
        let line = "// cofferdam-ignore Design.OrphanExport — Vite entry";
        assert_eq!(
            extract_next_line_directive(line),
            Some("Design.OrphanExport".to_string()),
        );
    }

    #[test]
    fn unused_suppression_treats_prose_as_broad() {
        // No check-id-shaped first token → record as broad (empty string,
        // skipped by the caller). Same behaviour as the prior code.
        let line = "// cofferdam-ignore please";
        assert_eq!(extract_next_line_directive(line), Some(String::new()));
    }

    #[test]
    fn looks_like_check_id_matches_suppress_crate() {
        // Canonical implementation lives in cofferdam_core::util; both this
        // crate and the engine now call the same function.
        assert!(looks_like_check_id("Design.OrphanExport"));
        assert!(looks_like_check_id("Plugin.Custom.Subcheck"));
        assert!(!looks_like_check_id("please"));
        assert!(!looks_like_check_id(""));
        assert!(!looks_like_check_id("123.bad"));
    }
}
