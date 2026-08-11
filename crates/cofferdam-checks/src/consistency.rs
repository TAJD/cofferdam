//! Consistency checks. Two-pass mode: pass 1 collects per-file evidence;
//! pass 2 emits findings against the collected baseline.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use cofferdam_core::span_from_bytes;
use cofferdam_core::{
    looks_like_check_id, Category, Check, CheckContext, CheckMeta, CorpusKey, FinalizeContext,
    Issue, Location, OptionDefault, OptionKind, OptionSpec, Priority, Severity, SourceFile, Span,
    ALL_PRE_FILTER_FINDINGS, REGISTERED_CHECK_IDS,
};
use oxc_ast::ast::{
    Expression, JSXAttribute, JSXAttributeName, JSXAttributeValue, ObjectProperty, PropertyKey,
    StringLiteral,
};
use oxc_ast_visit::Visit;
use oxc_span::GetSpan;

// ─── Consistency.QuoteStyle ─────────────────────────────────────────────────

/// Per-file statistics collected during pass 1.
#[derive(Default, Clone)]
struct FileQuoteStats {
    /// Count of single-quoted string literals in this file.
    single: u32,
    /// Count of double-quoted string literals in this file.
    double: u32,
    /// Byte range of ALL observed string literals, tagged with their quote
    /// char. Pass 2 picks the dominant style and re-filters. Kept as raw
    /// byte offsets (not resolved `Span`s) so the O(start_byte) cost of
    /// `span_from_bytes` is only paid for the minority spans pass 2 flags,
    /// not every literal in the file.
    spans: Vec<(u32, u32, u8)>, // (start_byte, end_byte, quote_byte: b'\'' or b'"')
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

    /// `pass2` below reads only `QUOTE_STATS[file.path]` — the entry
    /// this same file's own `run()` wrote — never another file's
    /// entry, so an unchanged file's verdict can't be affected by an
    /// edit elsewhere (CD-40 lever 4).
    fn pass2_is_file_local(&self) -> bool {
        true
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
        let mut line_index = None;
        for (start_byte, end_byte, quote) in &stats.spans {
            if *quote != dominant {
                let line_index =
                    line_index.get_or_insert_with(|| cofferdam_core::LineIndex::new(&file.text));
                let span = line_index.span_from_bytes(*start_byte, *end_byte);
                issues.push(Issue {
                    check_id: META.id.to_string(),
                    message: format!(
                        "use {dominant_name} quotes consistently (dominant style in this file)",
                    ),
                    file: file.path.clone(),
                    location: Location::from_span(&file.path, span),
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

        match quote {
            b'\'' => self.stats.single += 1,
            b'"' => self.stats.double += 1,
            _ => {}
        }
        self.stats.spans.push((lit.span.start, lit.span.end, quote));
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

// ─── Consistency.ErrorHandlingIdiom ────────────────────────────────────────

/// Field names that make an object-literal return count as "returning an
/// error" — mirrors `Refactor.MixedThrowAndReturnError`'s heuristic
/// deliberately, so the two checks agree on what counts as an
/// error-shaped return.
const EHI_ERROR_SHAPE_FIELDS: &[&str] = &["error", "ok", "success"];

/// Below this many total occurrences (throws + error-shaped returns)
/// project-wide, "dominant idiom" is statistically meaningless — skip
/// emitting anything.
const MIN_TOTAL_OCCURRENCES: usize = 4;

/// Per-file occurrence spans collected during pass 1.
#[derive(Default, Clone)]
struct FileErrorIdiomStats {
    throw_spans: Vec<Span>,
    error_return_spans: Vec<Span>,
}

static ERROR_IDIOM_STATS: CorpusKey<HashMap<PathBuf, FileErrorIdiomStats>> =
    CorpusKey::new("Consistency.ErrorHandlingIdiom.stats");

/// `Consistency.ErrorHandlingIdiom` — two-pass check (CD-129) that
/// learns the project-wide dominant error-handling idiom (throwing vs.
/// returning an error-shaped object) in pass 1, then flags every
/// occurrence of the minority idiom in pass 2.
///
/// v1 deliberately does NOT attempt to group occurrences by "same kind
/// of failure" (e.g. by directory, domain, or error class/name) — the
/// ticket flagged that grouping heuristic as the main open design
/// question, and the chosen v1 scope is to learn one idiom for the
/// whole project rather than try to resolve it. This means the check
/// is a project-wide idiom-consistency signal, not a same-failure-class
/// comparison; a project that legitimately mixes idioms across
/// unrelated domains (e.g. throwing for programmer errors, returning
/// `Result`-shaped values for expected validation failures) will see
/// this as noise. It overlaps somewhat with the per-function
/// `Refactor.MixedThrowAndReturnError` (CD-124) — that check flags a
/// single function mixing both idioms; this one flags idiom
/// inconsistency across the whole project.
///
/// The "throw" tally also counts `Promise.reject(...)` calls (the
/// async-idiom equivalent of a throw), and the "return" tally also
/// counts a concise arrow-function body evaluating to an error-shaped
/// object (`const parse = (s) => ({ error: "bad" })`), not just block-
/// bodied `return` statements (CD-136). A `catch (e) { throw e; }`
/// re-throw of the caught parameter is excluded from the throw tally
/// entirely — it's propagation, not a deliberate idiom choice, so
/// counting it would overstate how dominant "throw" is in a codebase
/// that mostly re-throws errors originated elsewhere.
pub struct ErrorHandlingIdiom;

const EHI_META: CheckMeta = CheckMeta {
    id: "Consistency.ErrorHandlingIdiom",
    category: Category::Consistency,
    base_priority: -5,
    default_severity: Severity::Info,
    explanation: "The project predominantly uses one error-handling idiom (throwing, or \
        returning an error-shaped value) — this file deviates from it, hurting consistency of \
        error paths for callers.",
    body: include_str!("../docs/Consistency.ErrorHandlingIdiom.md"),
    requires_types: false,
    consistency: false,
    options: &[],
    autofix: false,
    pure_run: false,
};

impl Check for ErrorHandlingIdiom {
    fn meta(&self) -> &'static CheckMeta {
        &EHI_META
    }

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&ERROR_IDIOM_STATS, |slot, path| {
            slot.remove(path);
        });
    }

    /// Pass 1: walk the whole file (any nesting depth — unlike CD-124,
    /// this check doesn't need per-function scoping) and record every
    /// throw statement and every error-shaped return statement.
    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };
        let mut collector = ErrorIdiomCollector {
            file,
            stats: FileErrorIdiomStats::default(),
            catch_param_stack: Vec::new(),
        };
        collector.visit_program(parsed.program);
        let stats = collector.stats;

        ctx.corpus.with_slot(&ERROR_IDIOM_STATS, |slot| {
            slot.insert(file.path.clone(), stats);
        });

        Vec::new()
    }

    /// Finalize (CD-139 perf fix): sum occurrences across every file in
    /// the corpus ONCE to learn the project-wide dominant idiom, then
    /// emit findings for every file's minority-idiom occurrences in one
    /// pass. This check was originally written as `pass2` (per-file
    /// second pass), which the engine invokes once PER FILE — since this
    /// check's aggregation is project-wide rather than per-file (unlike
    /// `Consistency.QuoteStyle`, which pass2 suits fine), that meant
    /// cloning the entire cross-file corpus slot once per file: O(files)
    /// work repeated O(files) times. `finalize` runs exactly once, so
    /// the same aggregation now costs O(files) total, matching the
    /// pattern used by `Design.DuplicateExportName` and friends.
    ///
    /// Read-only (cd-32): a draining read would empty the slot as a side
    /// effect of finalize, corrupting `Engine::analyze_incremental`'s
    /// persistent `AnalysisState` for the next incremental call.
    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let all_stats: HashMap<PathBuf, FileErrorIdiomStats> = ctx
            .corpus
            .with_slot(&ERROR_IDIOM_STATS, |slot| slot.clone());

        let total_throw: usize = all_stats.values().map(|s| s.throw_spans.len()).sum();
        let total_return: usize = all_stats.values().map(|s| s.error_return_spans.len()).sum();
        let total = total_throw + total_return;
        if total < MIN_TOTAL_OCCURRENCES {
            return Vec::new();
        }

        // Strict majority required, mirroring Consistency.QuoteStyle: a
        // 50/50 split (or anything short of a true majority) means
        // there's no dominant idiom to deviate from.
        let dominant_is_throw = if total_throw > total / 2 && total_throw > total_return {
            true
        } else if total_return > total / 2 && total_return > total_throw {
            false
        } else {
            return Vec::new();
        };

        let mut issues = Vec::new();
        for (path, stats) in &all_stats {
            if dominant_is_throw {
                for span in &stats.error_return_spans {
                    issues.push(ehi_issue(
                        path,
                        *span,
                        "this file returns an error-shaped value, but the project predominantly \
                         throws for errors — consider throwing instead for consistency",
                    ));
                }
            } else {
                for span in &stats.throw_spans {
                    issues.push(ehi_issue(
                        path,
                        *span,
                        "this file throws, but the project predominantly returns an error-shaped \
                         value for errors — consider returning one instead for consistency",
                    ));
                }
            }
        }
        issues
    }
}

fn ehi_issue(path: &std::path::Path, span: Span, message: &str) -> Issue {
    Issue {
        check_id: EHI_META.id.to_string(),
        message: message.to_string(),
        file: path.to_path_buf(),
        location: Location::from_span(path, span),
        priority: Priority(EHI_META.base_priority),
        severity: Severity::Info,
        related: Vec::new(),
    }
}

/// A field named `error`/`ok`/`success` signals *failure* only for
/// certain values — see `Refactor.MixedThrowAndReturnError`'s
/// `signals_failure` for the identical reasoning (`{ error: null }` /
/// `{ ok: true }` are a Result-shaped success arm, not a competing
/// error idiom).
fn ehi_signals_failure(field_name: &str, value: &oxc_ast::ast::Expression<'_>) -> bool {
    use oxc_ast::ast::Expression;
    match field_name {
        "error" => {
            !matches!(value, Expression::NullLiteral(_))
                && !matches!(value, Expression::Identifier(id) if id.name.as_str() == "undefined")
        }
        "ok" | "success" => !matches!(value, Expression::BooleanLiteral(lit) if lit.value),
        _ => true,
    }
}

/// Peel any wrapping `ParenthesizedExpression` nodes — `oxc_parser`
/// preserves parens by default (needed for e.g. `(s) => ({ error })`,
/// an arrow whose concise body would otherwise be parsed as a block),
/// so a parenthesized object literal like `return ({ error: 'x' })` or
/// an arrow's parenthesized concise body must be unwrapped before
/// matching on `Expression::ObjectExpression` (CD-136).
fn ehi_unwrap_parens<'a, 'b>(
    mut expr: &'b oxc_ast::ast::Expression<'a>,
) -> &'b oxc_ast::ast::Expression<'a> {
    while let oxc_ast::ast::Expression::ParenthesizedExpression(inner) = expr {
        expr = &inner.expression;
    }
    expr
}

fn ehi_is_error_shaped(expr: &oxc_ast::ast::Expression<'_>) -> bool {
    use oxc_ast::ast::{Expression, ObjectPropertyKind, PropertyKey};
    let Expression::ObjectExpression(obj) = ehi_unwrap_parens(expr) else {
        return false;
    };
    obj.properties.iter().any(|prop| {
        let ObjectPropertyKind::ObjectProperty(prop) = prop else {
            return false;
        };
        let name: Option<&str> = match &prop.key {
            PropertyKey::StaticIdentifier(id) => Some(id.name.as_str()),
            PropertyKey::StringLiteral(lit) => Some(lit.value.as_str()),
            _ => None,
        };
        name.is_some_and(|n| {
            EHI_ERROR_SHAPE_FIELDS.contains(&n) && ehi_signals_failure(n, &prop.value)
        })
    })
}

/// True when `callee` is exactly `Promise.reject` — the async-idiom
/// equivalent of a `throw` (CD-136).
fn ehi_is_promise_reject_callee(callee: &oxc_ast::ast::Expression<'_>) -> bool {
    let oxc_ast::ast::Expression::StaticMemberExpression(member) = callee else {
        return false;
    };
    if member.property.name.as_str() != "reject" {
        return false;
    }
    matches!(&member.object, oxc_ast::ast::Expression::Identifier(id) if id.name.as_str() == "Promise")
}

/// The catch parameter's binding name, if it's a simple identifier
/// (`catch (e)`) rather than a destructuring pattern or no parameter at
/// all — only the simple-identifier case can be a re-throw target.
fn ehi_catch_param_name(param: &oxc_ast::ast::CatchParameter<'_>) -> Option<String> {
    match &param.pattern {
        oxc_ast::ast::BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
        _ => None,
    }
}

struct ErrorIdiomCollector<'a> {
    file: &'a SourceFile,
    stats: FileErrorIdiomStats,
    /// Innermost enclosing catch clauses' bound parameter names, pushed
    /// on entry and popped on exit — `None` for a catch with no simple-
    /// identifier parameter (destructured, or omitted entirely).
    catch_param_stack: Vec<Option<String>>,
}

impl<'a> Visit<'a> for ErrorIdiomCollector<'a> {
    fn visit_catch_clause(&mut self, node: &oxc_ast::ast::CatchClause<'a>) {
        self.catch_param_stack
            .push(node.param.as_ref().and_then(ehi_catch_param_name));
        oxc_ast_visit::walk::walk_catch_clause(self, node);
        self.catch_param_stack.pop();
    }

    fn visit_throw_statement(&mut self, node: &oxc_ast::ast::ThrowStatement<'a>) {
        // A bare re-throw of the innermost enclosing catch's own bound
        // parameter (`catch (e) { throw e; }`) is propagation, not a
        // deliberate idiom choice — exclude it from the tally entirely
        // rather than counting it as a "throw" (CD-136).
        let is_rethrow = matches!(&node.argument, oxc_ast::ast::Expression::Identifier(id)
            if self
                .catch_param_stack
                .last()
                .and_then(|name| name.as_deref())
                == Some(id.name.as_str()));
        if !is_rethrow {
            let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
            self.stats.throw_spans.push(span);
        }
        oxc_ast_visit::walk::walk_throw_statement(self, node);
    }

    fn visit_return_statement(&mut self, node: &oxc_ast::ast::ReturnStatement<'a>) {
        if let Some(arg) = &node.argument {
            if ehi_is_error_shaped(arg) {
                let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
                self.stats.error_return_spans.push(span);
            }
        }
        oxc_ast_visit::walk::walk_return_statement(self, node);
    }

    fn visit_arrow_function_expression(
        &mut self,
        node: &oxc_ast::ast::ArrowFunctionExpression<'a>,
    ) {
        // A concise arrow body (`() => ({ error: ... })`) is a `return`
        // equivalent invisible to `visit_return_statement` — there's no
        // `ReturnStatement` node for it at all (CD-136).
        if let Some(expr) = node.get_expression() {
            if ehi_is_error_shaped(expr) {
                let span = span_from_bytes(&self.file.text, expr.span().start, expr.span().end);
                self.stats.error_return_spans.push(span);
            }
        }
        // A nested function scope can bind its own parameter with the same
        // name as an enclosing catch's (`catch (e) { xs.forEach((e) => {
        // throw e; }); }`) — that `e` refers to the callback's own
        // parameter, not the catch's. Push a boundary so the re-throw check
        // never treats it as the outer catch's parameter (CD-136).
        self.catch_param_stack.push(None);
        oxc_ast_visit::walk::walk_arrow_function_expression(self, node);
        self.catch_param_stack.pop();
    }

    fn visit_function(
        &mut self,
        node: &oxc_ast::ast::Function<'a>,
        flags: oxc_syntax::scope::ScopeFlags,
    ) {
        // Same shadowing boundary as `visit_arrow_function_expression`, for
        // regular function declarations/expressions (CD-136).
        self.catch_param_stack.push(None);
        oxc_ast_visit::walk::walk_function(self, node, flags);
        self.catch_param_stack.pop();
    }

    fn visit_call_expression(&mut self, node: &oxc_ast::ast::CallExpression<'a>) {
        if ehi_is_promise_reject_callee(&node.callee) {
            let span = span_from_bytes(&self.file.text, node.span.start, node.span.end);
            self.stats.throw_spans.push(span);
        }
        oxc_ast_visit::walk::walk_call_expression(self, node);
    }
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

    // ── ErrorHandlingIdiom tests ─────────────────────────────────────────────

    /// Runs `ErrorHandlingIdiom`'s run+finalize cycle over each
    /// `(path, source)` fixture and returns every issue from `finalize`,
    /// sorted for deterministic assertions (finalize iterates a
    /// `HashMap`, so raw order isn't stable).
    fn run_error_handling_idiom(fixtures: &[(&str, &str)]) -> Vec<Issue> {
        let corpus = CorpusIndex::new();
        let check = ErrorHandlingIdiom;
        let files: Vec<SourceFile> = fixtures
            .iter()
            .map(|(path, src)| SourceFile::new(PathBuf::from(path), *src))
            .collect();

        for file in &files {
            let allocator = Allocator::default();
            let parser_return = parse_into(&allocator, file);
            let parsed = ParsedView {
                program: &parser_return.program,
                diagnostics: &parser_return.errors,
            };
            let mut ctx = CheckContext::new(file)
                .with_parsed(&parsed)
                .with_corpus(&corpus);
            check.run(file, &mut ctx);
        }

        let mut finalize_ctx = cofferdam_core::FinalizeContext::new(&corpus);
        let mut issues = check.finalize(&mut finalize_ctx);
        issues.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.message.cmp(&b.message)));
        issues
    }

    #[test]
    fn minority_return_idiom_is_flagged_when_throw_dominates() {
        let issues = run_error_handling_idiom(&[
            ("a.ts", "function f() { throw new Error('a'); }"),
            ("b.ts", "function f() { throw new Error('b'); }"),
            ("c.ts", "function f() { throw new Error('c'); }"),
            ("d.ts", "function f() { throw new Error('d'); }"),
            ("e.ts", "function f() { return { error: 'e' }; }"),
        ]);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert_eq!(issues[0].file, PathBuf::from("e.ts"));
    }

    #[test]
    fn minority_throw_idiom_is_flagged_when_return_dominates() {
        let issues = run_error_handling_idiom(&[
            ("a.ts", "function f() { return { error: 'a' }; }"),
            ("b.ts", "function f() { return { error: 'b' }; }"),
            ("c.ts", "function f() { return { error: 'c' }; }"),
            ("d.ts", "function f() { return { error: 'd' }; }"),
            ("e.ts", "function f() { throw new Error('e'); }"),
        ]);
        assert_eq!(issues.len(), 1, "expected one finding; got {issues:?}");
        assert_eq!(issues[0].file, PathBuf::from("e.ts"));
    }

    #[test]
    fn tie_emits_no_findings() {
        let issues = run_error_handling_idiom(&[
            ("a.ts", "function f() { throw new Error('a'); }"),
            ("b.ts", "function f() { throw new Error('b'); }"),
            ("c.ts", "function f() { return { error: 'c' }; }"),
            ("d.ts", "function f() { return { error: 'd' }; }"),
        ]);
        assert!(
            issues.is_empty(),
            "a 50/50 split must not flag either idiom; got {issues:?}"
        );
    }

    #[test]
    fn below_min_total_emits_no_findings() {
        // Only 3 total occurrences, below MIN_TOTAL_OCCURRENCES (4).
        let issues = run_error_handling_idiom(&[
            ("a.ts", "function f() { throw new Error('a'); }"),
            ("b.ts", "function f() { throw new Error('b'); }"),
            ("c.ts", "function f() { return { error: 'c' }; }"),
        ]);
        assert!(
            issues.is_empty(),
            "too few total occurrences must not flag anything; got {issues:?}"
        );
    }

    #[test]
    fn success_shaped_return_is_not_counted_either_way() {
        // `{ error: null }` is a Result-shaped success arm, not a
        // competing error idiom — it must not count toward the return
        // tally, so the project below is still throw-dominant and the
        // success-shaped return itself is never flagged.
        let issues = run_error_handling_idiom(&[
            ("a.ts", "function f() { throw new Error('a'); }"),
            ("b.ts", "function f() { throw new Error('b'); }"),
            ("c.ts", "function f() { throw new Error('c'); }"),
            ("d.ts", "function f() { throw new Error('d'); }"),
            ("e.ts", "function f() { return { error: null, value: 1 }; }"),
        ]);
        assert!(
            issues.is_empty(),
            "a success-shaped `{{ error: null }}` return must not be flagged; got {issues:?}"
        );
    }

    #[test]
    fn arrow_body_error_return_is_counted() {
        // A concise arrow-function body (`() => ({ error: ... })`) has no
        // `ReturnStatement` node at all — before CD-136 it was invisible
        // to the collector entirely.
        let issues = run_error_handling_idiom(&[
            ("a.ts", "function f() { throw new Error('a'); }"),
            ("b.ts", "function f() { throw new Error('b'); }"),
            ("c.ts", "function f() { throw new Error('c'); }"),
            ("d.ts", "function f() { throw new Error('d'); }"),
            ("e.ts", "const parse = (s) => ({ error: 'e' });"),
        ]);
        assert_eq!(
            issues.len(),
            1,
            "expected the arrow-body return to be flagged as the minority idiom; got {issues:?}"
        );
        assert_eq!(issues[0].file, PathBuf::from("e.ts"));
    }

    #[test]
    fn promise_reject_call_is_counted_as_throw_idiom() {
        let issues = run_error_handling_idiom(&[
            ("a.ts", "function f() { return { error: 'a' }; }"),
            ("b.ts", "function f() { return { error: 'b' }; }"),
            ("c.ts", "function f() { return { error: 'c' }; }"),
            ("d.ts", "function f() { return { error: 'd' }; }"),
            (
                "e.ts",
                "function f() { return Promise.reject(new Error('e')); }",
            ),
        ]);
        assert_eq!(
            issues.len(),
            1,
            "expected the Promise.reject call to be flagged as the minority throw idiom; got {issues:?}"
        );
        assert_eq!(issues[0].file, PathBuf::from("e.ts"));
    }

    #[test]
    fn rethrow_of_caught_param_is_excluded_from_tally() {
        // Excluding the re-throw drops total occurrences to 3 (below
        // MIN_TOTAL_OCCURRENCES = 4); before CD-136 this re-throw would
        // have been miscounted as a genuine throw, reaching exactly 4
        // total occurrences and triggering a flag on d.ts as the
        // minority idiom.
        let issues = run_error_handling_idiom(&[
            ("a.ts", "function f() { return { error: 'a' }; }"),
            ("b.ts", "function f() { return { error: 'b' }; }"),
            ("c.ts", "function f() { return { error: 'c' }; }"),
            (
                "d.ts",
                "function f() { try { risky(); } catch (e) { throw e; } }",
            ),
        ]);
        assert!(
            issues.is_empty(),
            "a bare re-throw of the caught parameter must not count toward the throw tally; got {issues:?}"
        );
    }

    #[test]
    fn throw_of_new_error_inside_catch_is_still_counted() {
        // A throw inside a catch block that does NOT re-throw the caught
        // parameter (a wrapped/replaced error) is a genuine idiom choice
        // and must still count normally.
        let issues = run_error_handling_idiom(&[
            ("a.ts", "function f() { return { error: 'a' }; }"),
            ("b.ts", "function f() { return { error: 'b' }; }"),
            ("c.ts", "function f() { return { error: 'c' }; }"),
            (
                "d.ts",
                "function f() { try { risky(); } catch (e) { throw new Error('wrapped'); } }",
            ),
        ]);
        assert_eq!(
            issues.len(),
            1,
            "a throw of a new error inside catch (not a bare re-throw) must still count toward \
             the throw tally and be flagged; got {issues:?}"
        );
        assert_eq!(issues[0].file, PathBuf::from("d.ts"));
    }

    #[test]
    fn throw_of_shadowing_param_inside_nested_callback_is_still_counted() {
        // A nested callback's own parameter can share a name with an
        // enclosing catch's bound parameter without referring to it —
        // `throw e` here re-throws the callback's own `e`, not the outer
        // catch's, and must still count toward the throw tally (CD-136).
        let issues = run_error_handling_idiom(&[
            ("a.ts", "function f() { return { error: 'a' }; }"),
            ("b.ts", "function f() { return { error: 'b' }; }"),
            ("c.ts", "function f() { return { error: 'c' }; }"),
            (
                "d.ts",
                "function f() { try { risky(); } catch (e) { xs.forEach((e) => { throw e; }); } }",
            ),
        ]);
        assert_eq!(
            issues.len(),
            1,
            "a throw of a nested callback's own shadowing parameter must not be mistaken for a \
             re-throw of the outer catch's parameter; got {issues:?}"
        );
        assert_eq!(issues[0].file, PathBuf::from("d.ts"));
    }
}

// ─── Consistency.SpellingDialect ────────────────────────────────────────────

/// British/American pairs, British form first. Deliberately a short list
/// rather than a dictionary: every entry is a word that appears in
/// software prose and whose two spellings mean the same thing.
///
/// `licence`/`license` and `programme`/`program` are absent on purpose.
/// The first splits by part of speech in British English, and "program"
/// is the British spelling for software, so both would flag correct prose.
const DIALECT_PAIRS: &[(&str, &str)] = &[
    ("analyse", "analyze"),
    ("analysed", "analyzed"),
    ("analyser", "analyzer"),
    ("analysing", "analyzing"),
    ("artefact", "artifact"),
    ("artefacts", "artifacts"),
    ("behaviour", "behavior"),
    ("behaviours", "behaviors"),
    ("cancelled", "canceled"),
    ("cancelling", "canceling"),
    ("catalogue", "catalog"),
    ("catalogues", "catalogs"),
    ("categorise", "categorize"),
    ("categorised", "categorized"),
    ("categorises", "categorizes"),
    ("centre", "center"),
    ("centred", "centered"),
    ("colour", "color"),
    ("coloured", "colored"),
    ("colours", "colors"),
    ("customise", "customize"),
    ("customised", "customized"),
    ("customises", "customizes"),
    ("defence", "defense"),
    ("deserialise", "deserialize"),
    ("deserialised", "deserialized"),
    ("deserialises", "deserializes"),
    ("favourite", "favorite"),
    ("grey", "gray"),
    ("initialise", "initialize"),
    ("initialised", "initialized"),
    ("initialising", "initializing"),
    ("initialises", "initializes"),
    ("labelled", "labeled"),
    ("labelling", "labeling"),
    ("labour", "labor"),
    ("maximise", "maximize"),
    ("maximised", "maximized"),
    ("maximises", "maximizes"),
    ("minimise", "minimize"),
    ("minimised", "minimized"),
    ("minimises", "minimizes"),
    ("modelling", "modeling"),
    ("neighbour", "neighbor"),
    ("neighbours", "neighbors"),
    ("normalise", "normalize"),
    ("normalised", "normalized"),
    ("normalising", "normalizing"),
    ("normalises", "normalizes"),
    ("normalisation", "normalization"),
    ("optimise", "optimize"),
    ("optimised", "optimized"),
    ("optimises", "optimizes"),
    ("optimisation", "optimization"),
    ("organise", "organize"),
    ("organised", "organized"),
    ("organises", "organizes"),
    ("organisation", "organization"),
    ("prioritise", "prioritize"),
    ("prioritised", "prioritized"),
    ("prioritises", "prioritizes"),
    ("recognise", "recognize"),
    ("recognised", "recognized"),
    ("recognises", "recognizes"),
    ("serialise", "serialize"),
    ("serialised", "serialized"),
    ("serialises", "serializes"),
    ("serialisation", "serialization"),
    ("summarise", "summarize"),
    ("summarised", "summarized"),
    ("summarises", "summarizes"),
    ("synchronise", "synchronize"),
    ("synchronised", "synchronized"),
    ("synchronises", "synchronizes"),
    ("travelling", "traveling"),
    ("utilise", "utilize"),
    ("utilises", "utilizes"),
    ("visualise", "visualize"),
    ("visualised", "visualized"),
    ("visualises", "visualizes"),
];

/// Below this many dialect-carrying words project-wide, "the majority
/// spelling" is not a convention, it is a coincidence.
const MIN_DIALECT_OCCURRENCES: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dialect {
    British,
    American,
}

impl Dialect {
    fn label(self) -> &'static str {
        match self {
            Dialect::British => "British",
            Dialect::American => "American",
        }
    }
}

/// One dialect-carrying word. The span is resolved during pass 1 because
/// `FinalizeContext` carries no file text to resolve it from later.
#[derive(Clone)]
struct DialectHit {
    span: Span,
    word: String,
    counterpart: &'static str,
    dialect: Dialect,
}

static DIALECT_HITS: CorpusKey<HashMap<PathBuf, Vec<DialectHit>>> =
    CorpusKey::new("Consistency.SpellingDialect.hits");

const SPELLING_DIALECT_OPTIONS: &[OptionSpec] = &[OptionSpec {
    name: "dialect",
    kind: OptionKind::String,
    default: OptionDefault::String("infer"),
    doc: "which dialect is correct: \"british\", \"american\", or \"infer\" (the default) to take \
          whichever the project already uses in most cases. Under \"infer\" the check stays \
          silent on a close split, since a project without a convention has nothing to deviate \
          from.",
}];

const SPELLING_DIALECT_META: CheckMeta = CheckMeta {
    id: "Consistency.SpellingDialect",
    category: Category::Consistency,
    base_priority: -5,
    default_severity: Severity::Info,
    explanation: "The project spells one way in most of its prose and another way here. Two \
        spellings of the same word are not two conventions — they are one convention and some \
        outliers.",
    body: include_str!("../docs/Consistency.SpellingDialect.md"),
    requires_types: false,
    consistency: false,
    options: SPELLING_DIALECT_OPTIONS,
    autofix: false,
    pure_run: false,
};

/// `Consistency.SpellingDialect` — tallies British against American
/// spellings across the project in pass 1 and flags the minority in
/// `finalize`.
///
/// It never reads identifiers. `normalize`, `serialize` and `initialize`
/// are API surface here and in most TypeScript projects, and renaming a
/// function to satisfy a prose convention is not a trade anyone would
/// make. Only comments and prose-shaped string literals are scanned; a
/// literal with no space in it is treated as a code token — an import
/// specifier, a key, a check id — and skipped. A `className`/`class`
/// attribute value and a CSS-property-keyed object value (`style={{
/// backgroundColor: ... }}`) are skipped even though they contain spaces —
/// a Tailwind class list is American by construction. A dialect word
/// hyphenated against a utility-class root (`items-center`) is skipped the
/// same way even outside those two positions.
///
/// The check ships no opinion on which dialect is right. Under the
/// default `dialect = "infer"` it reports the minority against whatever
/// the project already does, and says nothing when the split is close: a
/// project that has not chosen is not a project in violation. A team that
/// has chosen pins the answer in options instead.
pub struct SpellingDialect;

impl Check for SpellingDialect {
    fn meta(&self) -> &'static CheckMeta {
        &SPELLING_DIALECT_META
    }

    fn register_removable(&self, corpus: &cofferdam_core::CorpusIndex) {
        corpus.register_removable(&DIALECT_HITS, |slot, path| {
            slot.remove(path);
        });
    }

    fn languages(&self) -> &'static [cofferdam_core::Language] {
        &[
            cofferdam_core::Language::TypeScript,
            cofferdam_core::Language::Markdown,
        ]
    }

    fn run(&self, file: &SourceFile, ctx: &mut CheckContext<'_>) -> Vec<Issue> {
        if file.language == cofferdam_core::Language::Markdown {
            let mut hits = Vec::new();
            collect_markdown_dialect_hits(&file.text, &mut hits);
            ctx.corpus.with_slot(&DIALECT_HITS, |slot| {
                if hits.is_empty() {
                    slot.remove(&file.path);
                } else {
                    slot.insert(file.path.clone(), std::mem::take(&mut hits));
                }
            });
            return Vec::new();
        }

        let Some(parsed) = ctx.parsed else {
            return Vec::new();
        };

        let mut hits = Vec::new();
        for comment in &parsed.program.comments {
            let (start, end) = (comment.span.start, comment.span.end);
            if let Some(text) = file.text.get(start as usize..end as usize) {
                collect_dialect_hits(text, start, &file.text, &mut hits);
            }
        }

        let mut collector = ProseLiteralCollector {
            text: &file.text,
            hits: &mut hits,
        };
        collector.visit_program(parsed.program);

        ctx.corpus.with_slot(&DIALECT_HITS, |slot| {
            if hits.is_empty() {
                slot.remove(&file.path);
            } else {
                slot.insert(file.path.clone(), std::mem::take(&mut hits));
            }
        });
        Vec::new()
    }

    fn finalize(&self, ctx: &mut FinalizeContext<'_>) -> Vec<Issue> {
        let all: HashMap<PathBuf, Vec<DialectHit>> =
            ctx.corpus.with_slot(&DIALECT_HITS, |slot| slot.clone());

        let correct = match ctx.options.get_string("dialect") {
            Some("british") => Dialect::British,
            Some("american") => Dialect::American,
            _ => {
                let british = count_dialect(&all, Dialect::British);
                let american = count_dialect(&all, Dialect::American);
                let total = british + american;
                if total < MIN_DIALECT_OCCURRENCES {
                    return Vec::new();
                }
                // Strict majority, as Consistency.QuoteStyle requires. A
                // near-even split is a project that has not chosen.
                if british > total / 2 && british > american {
                    Dialect::British
                } else if american > total / 2 && american > british {
                    Dialect::American
                } else {
                    return Vec::new();
                }
            }
        };

        let mut paths: Vec<&PathBuf> = all.keys().collect();
        paths.sort();
        let mut issues = Vec::new();
        for path in paths {
            for hit in &all[path] {
                if hit.dialect == correct {
                    continue;
                }
                issues.push(Issue {
                    check_id: SPELLING_DIALECT_META.id.to_string(),
                    message: format!(
                        "`{}` is the {} spelling; the project uses {} elsewhere — write `{}`",
                        hit.word,
                        hit.dialect.label(),
                        correct.label(),
                        match_leading_case(&hit.word, hit.counterpart)
                    ),
                    file: path.clone(),
                    location: Location::from_span(path, hit.span),
                    priority: Priority(SPELLING_DIALECT_META.base_priority),
                    severity: Severity::Info,
                    related: Vec::new(),
                });
            }
        }
        issues
    }
}

/// Carry the matched word's leading capital onto the suggestion, so a
/// sentence-initial `Initializes` is answered with `Initialises` rather
/// than a replacement the writer would have to re-case by hand.
fn match_leading_case(matched: &str, suggestion: &str) -> String {
    if matched.starts_with(|c: char| c.is_ascii_uppercase()) {
        let mut out = suggestion.to_string();
        out[..1].make_ascii_uppercase();
        out
    } else {
        suggestion.to_string()
    }
}

/// Scan a Markdown file for dialect-carrying words. The whole document is
/// prose, so the TypeScript rule — comments and prose-shaped literals —
/// has no analogue here; what needs excluding instead is the code a
/// Markdown file quotes. Fenced blocks, inline code spans, link
/// destinations and YAML frontmatter are skipped, because a page
/// documenting a `normalize` option or linking to `color-scheme.md` is
/// reporting an American spelling, not writing one.
///
/// Indented code blocks are deliberately not excluded: four-space
/// indentation is also how a nested list continuation is written, and
/// silencing every one of those would cost more prose than the rule buys.
fn collect_markdown_dialect_hits(text: &str, out: &mut Vec<DialectHit>) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut offset = 0usize;
    let mut in_fence: Option<(u8, usize)> = None;
    let mut first = 0usize;

    // YAML frontmatter: a `---` line opening the file, up to its closer.
    // A document that opens with a thematic break and never closes one is
    // not frontmatter, so the scan restarts at the top rather than
    // swallowing the whole file.
    if text.starts_with("---\n") || text.starts_with("---\r\n") {
        if let Some(close) = lines.iter().skip(1).position(|l| l.trim_end() == "---") {
            first = close + 2;
            offset = lines[..first].iter().map(|l| l.len()).sum();
        }
    }

    for line in &lines[first..] {
        let len = line.len();
        let fence = fence_marker(line);
        let mut is_fence_line = false;
        match (in_fence, fence) {
            // CommonMark: an opening backtick fence's info string may not
            // contain a backtick, which is what keeps a line beginning
            // ```` ```const x``` ```` from reading as a fence.
            (None, Some((c, run, rest))) if c == b'~' || !rest.contains('`') => {
                in_fence = Some((c, run));
                is_fence_line = true;
            }
            // The closer must be at least as long as the opener and carry
            // no info string. Without the length rule a ```` ```` ````
            // fence quoting ``` examples — every page that documents
            // Markdown — desynchronises on the inner fence.
            (Some((open, open_run)), Some((c, run, rest)))
                if open == c && run >= open_run && rest.trim().is_empty() =>
            {
                in_fence = None;
                is_fence_line = true;
            }
            _ => {}
        }
        if in_fence.is_none() && !is_fence_line {
            for (start, end) in markdown_prose_spans(line) {
                collect_dialect_hits(&line[start..end], (offset + start) as u32, text, out);
            }
        }
        offset += len;
    }
}

/// A fence line's character, run length and whatever follows the run.
fn fence_marker(line: &str) -> Option<(u8, usize, &str)> {
    let trimmed = line.trim_start();
    let c = match trimmed.as_bytes().first()? {
        b'`' => b'`',
        b'~' => b'~',
        _ => return None,
    };
    let run = trimmed.bytes().take_while(|&b| b == c).count();
    if run < 3 {
        return None;
    }
    Some((c, run, &trimmed[run..]))
}

/// Byte ranges of `line` that are prose, i.e. everything outside inline
/// code spans, link destinations and markup.
fn markdown_prose_spans(line: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    // A reference-link definition (`[g]: ./analyze-guide.md`), an MDX
    // `import`/`export`, and a bare URL are all code tokens on a line
    // that otherwise looks like prose. You cannot rename someone else's
    // path to satisfy a spelling convention.
    if is_reference_definition(line) || is_mdx_module_line(line) {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b':' && bytes[i..].starts_with(b"://") {
            // Rewind over the scheme and skip to the end of the URL
            // token; `https://example.com/color-guide` is one word to the
            // reader and none of it is English.
            let mut word_start = i;
            while word_start > 0 && !bytes[word_start - 1].is_ascii_whitespace() {
                word_start -= 1;
            }
            if start < word_start {
                spans.push((start, word_start));
            }
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            start = i;
            continue;
        }
        match bytes[i] {
            // An HTML or JSX tag — `<Callout type="behavior">`, `<br/>`,
            // and the `<https://…>` autolink form.
            b'<' if bytes
                .get(i + 1)
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'/') =>
            {
                if start < i {
                    spans.push((start, i));
                }
                while i < bytes.len() && bytes[i] != b'>' {
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
                start = i;
            }
            b'`' => {
                let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
                let close = find_backtick_run(&bytes[i + ticks..], ticks);
                if start < i {
                    spans.push((start, i));
                }
                // An unclosed run is a stray backtick, not a code span:
                // treat the rest of the line as prose.
                match close {
                    Some(rel) => i += ticks + rel + ticks,
                    None => i += ticks,
                }
                start = i;
            }
            // `](https://…)` and `][ref]` — the destination is a URL or a
            // label, never prose.
            b']' if bytes.get(i + 1).is_some_and(|&b| b == b'(' || b == b'[') => {
                let closer = if bytes[i + 1] == b'(' { b')' } else { b']' };
                if start < i + 1 {
                    spans.push((start, i + 1));
                }
                i += 2;
                while i < bytes.len() && bytes[i] != closer {
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        spans.push((start, bytes.len()));
    }
    spans
}

/// `[label]: destination` at the head of a line.
fn is_reference_definition(line: &str) -> bool {
    let t = line.trim_start();
    let Some(rest) = t.strip_prefix('[') else {
        return false;
    };
    rest.find(']')
        .is_some_and(|i| rest[i + 1..].starts_with(':'))
}

/// An MDX `import`/`export` line. `.mdx` shares `Language::Markdown`, and
/// its module specifiers are code the TypeScript path would never have
/// scanned.
fn is_mdx_module_line(line: &str) -> bool {
    let t = line.trim_start();
    (t.starts_with("import ") || t.starts_with("export "))
        && (t.contains(" from ") || t.contains('{') || t.contains("default"))
}

/// Offset of the next run of exactly `len` backticks in `bytes`.
fn find_backtick_run(bytes: &[u8], len: usize) -> Option<usize> {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if run == len {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

fn count_dialect(all: &HashMap<PathBuf, Vec<DialectHit>>, dialect: Dialect) -> usize {
    all.values()
        .flat_map(|hits| hits.iter())
        .filter(|h| h.dialect == dialect)
        .count()
}

/// Scan `text` — a comment body or a string literal — for dialect-carrying
/// words, recording each as an absolute byte range via `base`.
///
/// Matching is case-insensitive and bounded on both sides by a non-word
/// character, so `serialise` inside `deserialiseThing` is not a hit.
fn collect_dialect_hits(text: &str, base: u32, full_text: &str, out: &mut Vec<DialectHit>) {
    // One pass over the text, splitting it into runs of word characters
    // and looking each run up. Scanning for each of the ~170 spellings in
    // turn is the obvious alternative and costs 170 passes over every
    // comment in the project, which showed up as a 20% whole-run
    // regression on the benchmark corpus.
    let bytes = text.as_bytes();
    let mut folded = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if !is_word_char(Some(bytes[i])) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_word_char(Some(bytes[i])) {
            i += 1;
        }
        let word = &text[start..i];
        if word.len() < DIALECT_MIN_LEN || word.len() > DIALECT_MAX_LEN {
            continue;
        }
        folded.clear();
        folded.extend(word.bytes().map(|b| b.to_ascii_lowercase() as char));
        let Some(&(dialect, counterpart)) = DIALECT_LOOKUP.get(folded.as_str()) else {
            continue;
        };
        // `items-center` / `background-color`: a hyphenated compound whose
        // other segment is a utility-class root reads as a class name, not
        // prose. `well-organised` has no such neighbour on either side, so
        // it is unaffected. Gated on the word being CSS vocabulary too,
        // because a utility root like `self` is also an English hyphen
        // prefix — without the gate `self-center` and `self-defence` are
        // indistinguishable, and the latter is a spelling we want to keep
        // reporting.
        let hyphen_class_shape = is_css_vocabulary_word(folded.as_str());
        let left_is_class = hyphen_class_shape && start > 0 && bytes[start - 1] == b'-' && {
            let mut j = start - 1;
            while j > 0 && is_word_char(Some(bytes[j - 1])) {
                j -= 1;
            }
            is_class_hyphen_segment(&text[j..start - 1])
        };
        let right_is_class = hyphen_class_shape && i < bytes.len() && bytes[i] == b'-' && {
            let mut j = i + 1;
            while j < bytes.len() && is_word_char(Some(bytes[j])) {
                j += 1;
            }
            is_class_hyphen_segment(&text[i + 1..j])
        };
        if left_is_class || right_is_class {
            continue;
        }
        out.push(DialectHit {
            span: span_from_bytes(full_text, base + start as u32, base + i as u32),
            word: word.to_string(),
            counterpart,
            dialect,
        });
    }
}

/// Every spelling in `DIALECT_PAIRS`, each mapped to the dialect it
/// belongs to and the spelling on the other side of the pair.
static DIALECT_LOOKUP: LazyLock<HashMap<&'static str, (Dialect, &'static str)>> =
    LazyLock::new(|| {
        DIALECT_PAIRS
            .iter()
            .flat_map(|(british, american)| {
                [
                    (*british, (Dialect::British, *american)),
                    (*american, (Dialect::American, *british)),
                ]
            })
            .collect()
    });

/// Bounds of the word list, used to skip the fold-and-look-up on the
/// overwhelming majority of words that cannot be in it.
const DIALECT_MIN_LEN: usize = 4;
const DIALECT_MAX_LEN: usize = 16;

fn is_word_char(byte: Option<u8>) -> bool {
    matches!(byte, Some(b) if b.is_ascii_alphanumeric() || b == b'_')
}

/// A string literal counts as prose only if it contains a space. Without
/// that filter an import specifier (`./normalize`), an object key or a
/// check id would be read as English and flagged — the identifier problem
/// arriving by another door.
struct ProseLiteralCollector<'a> {
    text: &'a str,
    hits: &'a mut Vec<DialectHit>,
}

impl<'a> Visit<'a> for ProseLiteralCollector<'_> {
    fn visit_string_literal(&mut self, lit: &StringLiteral<'a>) {
        if !lit.value.as_str().contains(' ') {
            return;
        }
        // Scan the raw source slice rather than the cooked value so byte
        // offsets stay true; escapes shift them otherwise.
        let (start, end) = (lit.span.start, lit.span.end);
        if let Some(raw) = self.text.get(start as usize..end as usize) {
            collect_dialect_hits(raw, start, self.text, self.hits);
        }
    }

    /// `className="flex items-center"` / `class="..."` is a space-separated
    /// utility-class list, not prose — a Tailwind class is American by
    /// construction (`items-center`, `justify-center`). Skip the attribute
    /// entirely rather than trying to tell a class list from a caption by
    /// its text alone.
    fn visit_jsx_attribute(&mut self, it: &JSXAttribute<'a>) {
        let is_class_attr = match &it.name {
            JSXAttributeName::Identifier(id) => {
                let name = id.name.as_str();
                name.eq_ignore_ascii_case("classname") || name.eq_ignore_ascii_case("class")
            }
            JSXAttributeName::NamespacedName(_) => false,
        };
        if is_class_attr {
            return;
        }
        oxc_ast_visit::walk::walk_jsx_attribute(self, it);
    }

    /// `style={{ backgroundColor: '...' }}` (and the string-keyed
    /// `'background-color'` form) pairs a CSS property with a CSS value —
    /// not prose, even when the value itself contains a space
    /// (`'width 200ms linear'`).
    ///
    /// Only the property's own string value is skipped, never the whole
    /// subtree: a dozen CSS property names (`content`, `color`, `order`,
    /// `filter`, `position`, …) are commoner as ordinary domain keys, and
    /// skipping the subtree took a chat message's `{ role, content }` out
    /// of the check entirely.
    fn visit_object_property(&mut self, it: &ObjectProperty<'a>) {
        let key_name: Option<&str> = match &it.key {
            PropertyKey::StaticIdentifier(id) => Some(id.name.as_str()),
            PropertyKey::StringLiteral(lit) => Some(lit.value.as_str()),
            _ => None,
        };
        if key_name.is_some_and(is_css_property_key)
            && matches!(&it.value, Expression::StringLiteral(_))
        {
            return;
        }
        oxc_ast_visit::walk::walk_object_property(self, it);
    }
}

/// A representative slice of the CSS property surface, in kebab-case.
/// Enough to recognize `style={{ backgroundColor: ... }}` and
/// `{ 'background-color': ... }` as CSS rather than prose. Not
/// exhaustive — an unrecognized key just falls through to the ordinary
/// prose/code-token rules, same as before this list existed.
///
/// Single words that are commoner as ordinary domain keys than as CSS —
/// `content`, `color`, `order`, `filter`, `position`, `width` — are
/// deliberately absent. Their CSS values (`'#fff'`, `'0 auto'`) almost
/// never contain a dialect word, so listing them bought nothing and cost
/// the check its view of every `{ role, content }` message object.
const CSS_PROPERTY_NAMES: &[&str] = &[
    "align-content",
    "align-items",
    "align-self",
    "animation",
    "animation-delay",
    "animation-duration",
    "animation-name",
    "animation-timing-function",
    "background",
    "background-color",
    "background-image",
    "background-position",
    "background-repeat",
    "background-size",
    "border",
    "border-color",
    "border-radius",
    "border-style",
    "border-width",
    "box-shadow",
    "box-sizing",
    "column-gap",
    "cursor",
    "flex",
    "flex-basis",
    "flex-direction",
    "flex-flow",
    "flex-grow",
    "flex-shrink",
    "flex-wrap",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "grid",
    "grid-area",
    "grid-column",
    "grid-gap",
    "grid-row",
    "grid-template",
    "grid-template-areas",
    "grid-template-columns",
    "grid-template-rows",
    "justify-content",
    "justify-items",
    "justify-self",
    "letter-spacing",
    "line-height",
    "list-style",
    "margin",
    "margin-bottom",
    "margin-left",
    "margin-right",
    "margin-top",
    "max-height",
    "max-width",
    "min-height",
    "min-width",
    "object-fit",
    "object-position",
    "opacity",
    "outline",
    "overflow-x",
    "overflow-y",
    "padding",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "padding-top",
    "place-content",
    "place-items",
    "place-self",
    "pointer-events",
    "row-gap",
    "stroke",
    "text-align",
    "text-decoration",
    "text-overflow",
    "text-shadow",
    "text-transform",
    "transform",
    "transform-origin",
    "transition",
    "transition-delay",
    "transition-duration",
    "transition-property",
    "transition-timing-function",
    "user-select",
    "vertical-align",
    "white-space",
    "word-break",
    "word-wrap",
    "z-index",
];

/// `backgroundColor` -> `background-color`; a key already containing a
/// hyphen (`'background-color'`) is assumed to be kebab-case already.
fn camel_to_kebab(name: &str) -> String {
    if name.contains('-') {
        return name.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn is_css_property_key(key: &str) -> bool {
    CSS_PROPERTY_NAMES.contains(&camel_to_kebab(key).as_str())
}

/// Utility-class segments that pair with a dialect word across a hyphen in
/// Tailwind-style class lists (`items-center`, `justify-center`,
/// `self-center`). Deliberately short — utility roots, not a dictionary —
/// mirroring `DIALECT_PAIRS`' own restraint. A hyphenated word is treated
/// as a class name rather than prose when either side of the hyphen names
/// one of these.
const CSS_CLASS_HYPHEN_SEGMENTS: &[&str] = &[
    "items",
    "justify",
    "content",
    "self",
    "place",
    "text",
    "bg",
    "border",
    "rounded",
    "gap",
    "flex",
    "grid",
    "space",
    "divide",
    "shadow",
    "ring",
    "outline",
    "opacity",
    "scale",
    "rotate",
    "translate",
    "skew",
    "origin",
    "cursor",
    "select",
    "resize",
    "overflow",
    "inset",
    "top",
    "bottom",
    "left",
    "right",
    "order",
    "col",
    "row",
    "auto",
    "w",
    "h",
    "min",
    "max",
    "p",
    "m",
    "px",
    "py",
    "pt",
    "pb",
    "pl",
    "pr",
    "mx",
    "my",
    "mt",
    "mb",
    "ml",
    "mr",
    "font",
    "tracking",
    "leading",
    "align",
    "whitespace",
    "decoration",
    "indent",
    "caret",
    "accent",
    "blur",
    "brightness",
    "contrast",
    "grayscale",
    "saturate",
    "invert",
    "sepia",
    "backdrop",
    "transition",
    "duration",
    "ease",
    "delay",
    "animate",
    "transform",
    "hover",
    "focus",
    "active",
    "disabled",
    "sm",
    "md",
    "lg",
    "xl",
    "z",
    // Media-query and CSS-property neighbours: `prefers-color-scheme`,
    // `forced-colors`, `scrollbar-color`.
    "prefers",
    "scheme",
    "forced",
    "scrollbar",
    "background",
];

fn is_class_hyphen_segment(segment: &str) -> bool {
    CSS_CLASS_HYPHEN_SEGMENTS.contains(&segment.to_ascii_lowercase().as_str())
}

/// The handful of dialect-carrying words that CSS and Tailwind actually
/// use, in both spellings. Only these get the hyphen-neighbour treatment
/// above; everything else is prose wherever it appears.
const CSS_VOCABULARY_WORDS: &[&str] = &[
    "center",
    "centre",
    "centered",
    "centred",
    "color",
    "colour",
    "colors",
    "colours",
    "gray",
    "grey",
    "normalize",
    "normalise",
    "capitalize",
    "capitalise",
    "behavior",
    "behaviour",
];

/// `folded` is already lowercased by the caller.
fn is_css_vocabulary_word(folded: &str) -> bool {
    CSS_VOCABULARY_WORDS.contains(&folded)
}

#[cfg(test)]
mod spelling_dialect_tests {
    use super::*;
    use cofferdam_core::parser::{parse_into, ParsedView};
    use cofferdam_core::{Allocator, Check, CheckContext, CorpusIndex, SourceFile};
    use std::path::PathBuf;

    // ─── Consistency.SpellingDialect ────────────────────────────────────────

    /// The length guard in `collect_dialect_hits` skips a word before it is
    /// ever looked up, so a pair outside the bounds would be silently
    /// unenforceable.
    #[test]
    fn every_spelling_falls_inside_the_length_guard() {
        for (british, american) in DIALECT_PAIRS {
            for word in [british, american] {
                assert!(
                    (DIALECT_MIN_LEN..=DIALECT_MAX_LEN).contains(&word.len()),
                    "{word} is outside DIALECT_MIN_LEN..=DIALECT_MAX_LEN and would never match"
                );
            }
        }
    }

    /// Run the check over `files` (path, source) and return `finalize`'s
    /// issues. `dialect` pins the option when `Some`.
    fn run_spelling(files: &[(&str, &str)], dialect: Option<&str>) -> Vec<Issue> {
        let corpus = CorpusIndex::new();
        let check = SpellingDialect;

        for (path, src) in files {
            let file = SourceFile::new(PathBuf::from(path), *src);
            // Markdown never reaches the parser — the engine hands it to
            // `run` with `parsed: None` (CD-316).
            if file.language == cofferdam_core::Language::Markdown {
                let mut ctx = CheckContext::new(&file).with_corpus(&corpus);
                assert!(
                    check.run(&file, &mut ctx).is_empty(),
                    "pass 1 emits nothing"
                );
                continue;
            }
            let allocator = Allocator::default();
            let parser_return = parse_into(&allocator, &file);
            let parsed = ParsedView {
                program: &parser_return.program,
                diagnostics: &parser_return.errors,
            };
            let mut ctx = CheckContext::new(&file)
                .with_parsed(&parsed)
                .with_corpus(&corpus);
            assert!(
                check.run(&file, &mut ctx).is_empty(),
                "pass 1 emits nothing"
            );
        }

        let options = dialect.map(|d| {
            let mut raw: std::collections::BTreeMap<String, cofferdam_core::RawOptionValue> =
                std::collections::BTreeMap::new();
            raw.insert(
                "dialect".to_string(),
                cofferdam_core::RawOptionValue::String(d.to_string()),
            );
            cofferdam_core::validate_options(
                SPELLING_DIALECT_META.id,
                SPELLING_DIALECT_META.options,
                &raw,
            )
            .expect("valid options")
        });
        let mut ctx = FinalizeContext::new(&corpus);
        if let Some(opts) = options.as_ref() {
            ctx = ctx.with_options(opts);
        }
        check.finalize(&mut ctx)
    }

    /// Enough British spellings to establish a majority, plus a couple of
    /// American ones to be flagged.
    const BRITISH_MAJORITY: &str = "\
// The colour table, the behaviour of the analyser, the catalogue order,
// the artefact list, an unrecognised colour and a centred behaviour.
// This one is the odd behavior out, next to a lone color.
export const x = 1;
";

    #[test]
    fn the_minority_spelling_is_flagged_and_the_majority_is_not() {
        let issues = run_spelling(&[("a.ts", BRITISH_MAJORITY)], None);
        let words: Vec<&str> = issues
            .iter()
            .map(|i| i.message.split('`').nth(1).unwrap_or(""))
            .collect();
        assert_eq!(words, ["behavior", "color"], "{words:?}");
        assert!(
            issues[0].message.contains("write `behaviour`"),
            "{}",
            issues[0].message
        );
    }

    /// CD-316: the condition that motivated the check was a docs corpus
    /// split between "analyz*" and "analys*", which it could not reach
    /// while a check declared one language.
    #[test]
    fn markdown_prose_is_scanned() {
        let md = "Here we analyze the behavior of the colour table.\n";
        let issues = run_spelling(&[("a.ts", BRITISH_MAJORITY), ("docs/a.md", md)], None);
        let words: Vec<&str> = issues
            .iter()
            .map(|i| i.message.split('`').nth(1).unwrap_or(""))
            .collect();
        assert_eq!(
            words,
            ["behavior", "color", "analyze", "behavior"],
            "{words:?}"
        );
    }

    /// A page documenting a `normalize` option, quoting code, or linking
    /// to `color-scheme.md` is reporting an American spelling, not
    /// writing one.
    #[test]
    fn markdown_code_and_link_destinations_are_not_prose() {
        let md = "\
---
title: color test
---

Call `normalize` first, then see [the guide](./color-scheme.md).

```ts
const normalize = 1; // color
```

~~~
analyze
~~~
";
        assert!(
            run_spelling(&[("a.ts", BRITISH_MAJORITY), ("docs/a.md", md)], None).len() == 2,
            "only the TypeScript fixture's own two outliers should remain"
        );
    }

    /// A four-backtick fence quoting three-backtick examples is how every
    /// page that documents Markdown is written. Closing on the inner
    /// fence desynchronises the state machine, which reports the quoted
    /// code and silences the prose after it.
    #[test]
    fn a_longer_fence_is_not_closed_by_a_shorter_one() {
        let md = "\
Intro prose about behaviour.

````markdown
```ts
const behavior = 1;
```

Analyze the color output above.
````

Closing prose about the color scheme.
";
        let issues = run_spelling(&[("a.ts", BRITISH_MAJORITY), ("docs/a.md", md)], None);
        let words: Vec<&str> = issues
            .iter()
            .map(|i| i.message.split('`').nth(1).unwrap_or(""))
            .collect();
        assert_eq!(words, ["behavior", "color", "color"], "{words:?}");
    }

    /// A line opening with an inline code span is not a fence: an opening
    /// backtick fence's info string may not contain a backtick.
    #[test]
    fn an_inline_span_at_line_start_is_not_a_fence() {
        let md = "```const x``` is inline code.\n\nLater prose about the color.\n";
        let issues = run_spelling(&[("a.ts", BRITISH_MAJORITY), ("docs/a.md", md)], None);
        assert_eq!(issues.len(), 3, "{issues:?}");
    }

    /// Reference definitions, autolinks and bare URLs are destinations
    /// too. You cannot rename someone else's path to satisfy a spelling
    /// convention.
    #[test]
    fn every_link_destination_shape_is_excluded() {
        let md = "\
See [the guide][g] and <https://example.com/analyze-guide> and
https://example.com/color-guide for details.

[g]: ./analyze-guide.md
[h]: https://example.com/behavior/x
";
        assert_eq!(
            run_spelling(&[("a.ts", BRITISH_MAJORITY), ("docs/a.md", md)], None).len(),
            2,
            "only the TypeScript fixture's own two outliers should remain"
        );
    }

    /// `.mdx` shares `Language::Markdown`. Its module specifiers and tag
    /// attributes are code the TypeScript path would never have scanned.
    #[test]
    fn mdx_imports_and_tags_are_not_prose() {
        let md = "\
import { Thing } from './analyze-utils';
import Chart from \"@site/src/components/color-chart\";

<Callout type=\"behavior\">Real behaviour prose.</Callout>
";
        assert_eq!(
            run_spelling(&[("a.ts", BRITISH_MAJORITY), ("docs/a.mdx", md)], None).len(),
            2,
            "only the TypeScript fixture's own two outliers should remain"
        );
    }

    /// A document opening with a thematic break is not frontmatter, and
    /// treating it as an unclosed one loses the whole body.
    #[test]
    fn a_leading_thematic_break_is_not_unclosed_frontmatter() {
        let md = "---\nThe behavior of the color table.\n";
        let issues = run_spelling(&[("a.ts", BRITISH_MAJORITY), ("docs/a.md", md)], None);
        assert_eq!(issues.len(), 4, "{issues:?}");
    }

    /// A stray backtick opens nothing, so the rest of the line stays
    /// prose rather than disappearing from the corpus.
    #[test]
    fn an_unclosed_backtick_does_not_swallow_the_line() {
        let md = "A ` stray tick and then the behavior of it.\n";
        let issues = run_spelling(&[("a.ts", BRITISH_MAJORITY), ("docs/a.md", md)], None);
        assert_eq!(issues.len(), 3, "{issues:?}");
    }

    /// The load-bearing constraint. `normalize`, `serialize` and
    /// `initialize` are API surface; a prose convention must never ask for
    /// them to be renamed.
    #[test]
    fn identifiers_are_never_read() {
        let src = "\
// The colour table, the behaviour of the analyser, the catalogue order,
// the artefact list, an unrecognised colour and a centred behaviour.
export function normalize(color: string) { return color; }
export const serializeColorCatalog = normalize;
class Analyzer { initialize() {} }
";
        assert!(run_spelling(&[("a.ts", src)], None).is_empty());
    }

    /// A string with no space is a code token — an import specifier, a
    /// key, a check id — not English.
    #[test]
    fn a_spaceless_string_literal_is_a_code_token_not_prose() {
        let src = "\
// The colour table, the behaviour of the analyser, the catalogue order,
// the artefact list, an unrecognised colour and a centred behaviour.
import x from './normalize';
export const k = 'color';
export const id = 'Design.SerializeColor';
export const y = x;
";
        assert!(run_spelling(&[("a.ts", src)], None).is_empty());
    }

    #[test]
    fn a_prose_string_literal_is_read() {
        let src = "\
// The colour table, the behaviour of the analyser, the catalogue order,
// the artefact list, an unrecognised colour and a centred behaviour.
export const msg = 'the analyzer keeps every artifact';
";
        let issues = run_spelling(&[("a.ts", src)], None);
        assert_eq!(issues.len(), 2, "{issues:?}");
    }

    /// A project that has not chosen is not a project in violation.
    #[test]
    fn an_even_split_says_nothing() {
        let src = "\
// colour behaviour analyser catalogue artefact
// color behavior analyzer catalog artifact
export const x = 1;
";
        assert!(run_spelling(&[("a.ts", src)], None).is_empty());
    }

    /// Below the floor a majority is a coincidence.
    #[test]
    fn too_few_occurrences_say_nothing() {
        let src = "// colour colour behaviour, and one color.\nexport const x = 1;\n";
        assert!(run_spelling(&[("a.ts", src)], None).is_empty());
    }

    /// With a dialect pinned, the floor and the majority rule both fall
    /// away — one deviation is enough.
    #[test]
    fn a_pinned_dialect_overrules_the_majority() {
        let src = "// colour colour colour behaviour, and one color.\nexport const x = 1;\n";
        let issues = run_spelling(&[("a.ts", src)], Some("american"));
        assert_eq!(
            issues.len(),
            4,
            "every British spelling now deviates: {issues:?}"
        );

        let issues = run_spelling(&[("a.ts", src)], Some("british"));
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].message.contains("`color`"),
            "{}",
            issues[0].message
        );
    }

    /// The majority is learned across the project, not per file, so a file
    /// that is internally consistent still deviates from its neighbours.
    #[test]
    fn the_majority_is_learned_across_files() {
        let british =
            "// colour behaviour analyser catalogue artefact centre\nexport const a = 1;\n";
        let american = "// color behavior\nexport const b = 2;\n";
        let issues = run_spelling(&[("a.ts", british), ("b.ts", american)], None);
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(
            issues.iter().all(|i| i.file.ends_with("b.ts")),
            "{issues:?}"
        );
    }

    /// A sentence-initial word keeps its capital in the suggestion.
    #[test]
    fn the_suggestion_keeps_the_leading_capital() {
        let src = "\
// The colour table, the behaviour of the analyser, the catalogue order,
// the artefact list, an unrecognised colour and a centred behaviour.
// Color is the odd one out.
export const x = 1;
";
        let issues = run_spelling(&[("a.ts", src)], None);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].message.contains("write `Colour`"),
            "{}",
            issues[0].message
        );
    }

    /// `deserialise` contains `serialise`; a word boundary on both sides
    /// keeps the inner match out of the tally.
    #[test]
    fn a_word_inside_a_longer_word_is_not_a_hit() {
        let mut hits = Vec::new();
        let text = "deserialiseThing and colourful";
        collect_dialect_hits(text, 0, text, &mut hits);
        assert!(
            hits.is_empty(),
            "{:?}",
            hits.iter().map(|h| &h.word).collect::<Vec<_>>()
        );
    }

    /// A Tailwind `className` is a class list, not prose — following the
    /// check's own advice would emit a class Tailwind never generated
    /// (CD-319).
    #[test]
    fn a_classname_attribute_is_not_scanned() {
        let src = "\
export function Panel() {
  return <div className=\"flex sm:items-center color-swatch\" />;
}
";
        assert!(run_spelling(&[("a.tsx", src)], Some("british")).is_empty());
    }

    /// A CSS-property-keyed object value (`style={{ backgroundColor: ...
    /// }}`) is a CSS value, not prose, even though it contains a space
    /// (CD-319).
    #[test]
    fn a_css_property_keyed_object_value_is_not_scanned() {
        let src = "\
export const styles = {
  backgroundColor: 'width 200ms linear, background-color 300ms',
};
";
        assert!(run_spelling(&[("a.ts", src)], Some("british")).is_empty());
    }

    /// The positional exclusions above are not a blanket "skip JSX" or
    /// "skip objects" rule — a genuine prose string literal elsewhere in
    /// the same file still reports.
    #[test]
    fn a_prose_string_literal_still_reports_alongside_excluded_positions() {
        let src = "\
export function Panel() {
  return <div className=\"flex sm:items-center\" title={'the analyzer keeps every artifact'} />;
}
";
        let issues = run_spelling(&[("a.tsx", src)], Some("british"));
        assert_eq!(issues.len(), 2, "{issues:?}");
    }

    /// A hyphenated compound whose neighbour is a utility-class root reads
    /// as a class name even outside a `className` attribute or a style
    /// object — e.g. a class list assembled with `clsx`/`cn` rather than
    /// written inline (CD-319).
    #[test]
    fn a_class_shaped_hyphen_compound_is_not_a_hit_outside_jsx() {
        let src = "export const classes = 'flex items-center justify-center';";
        assert!(run_spelling(&[("a.ts", src)], Some("british")).is_empty());
    }

    /// A genuine hyphenated English compound is unaffected by the
    /// class-shape exclusion — `well` is not a utility-class root
    /// (CD-319).
    #[test]
    fn a_genuine_hyphenated_compound_still_reports() {
        let src = "export const msg = 'a well-organised list of files';";
        let issues = run_spelling(&[("a.ts", src)], Some("american"));
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].message.contains("organised"),
            "{}",
            issues[0].message
        );
    }

    /// `self` is both a utility-class root (`self-center`) and an English
    /// hyphen prefix. Only the CSS vocabulary gets the class-shape
    /// exclusion, so `self-defence` still reports (CD-319).
    #[test]
    fn an_english_hyphen_prefix_shared_with_a_utility_root_still_reports() {
        let src = "export const msg = 'a matter of self-defence';";
        let issues = run_spelling(&[("a.ts", src)], Some("american"));
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].message.contains("defence"),
            "{}",
            issues[0].message
        );
    }

    /// The same prefix in a class list is still excluded (CD-319).
    #[test]
    fn a_utility_root_shared_with_an_english_prefix_is_still_excluded() {
        let src = "export const classes = 'self-center place-content-center';";
        assert!(run_spelling(&[("a.ts", src)], Some("british")).is_empty());
    }

    /// A dozen CSS property names — `content`, `order`, `filter`,
    /// `position` — are commoner as ordinary domain keys. Skipping the
    /// whole subtree took a chat message's `{ role, content }` out of the
    /// check entirely, so only the property's own string value is skipped
    /// (CD-319).
    #[test]
    fn prose_under_a_css_named_key_still_reports() {
        let src = "export const msgs = [{ role: 'user', content: 'analyze the colour of it' }];";
        let issues = run_spelling(&[("a.ts", src)], Some("american"));
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].message.contains("colour"),
            "{}",
            issues[0].message
        );
    }

    /// The nested form the subtree skip also swallowed (CD-319).
    #[test]
    fn prose_nested_under_a_css_named_key_still_reports() {
        let src = "export const cfg = { content: { title: 'analyze the colour of it' } };";
        assert_eq!(run_spelling(&[("a.ts", src)], Some("american")).len(), 1);
    }

    /// Rewriting a media query breaks it as surely as rewriting a class
    /// name does (CD-319).
    #[test]
    fn a_media_query_feature_name_is_not_scanned() {
        let src = "export const q = window.matchMedia('(prefers-color-scheme: dark)');";
        assert!(run_spelling(&[("a.ts", src)], Some("british")).is_empty());
    }
}
