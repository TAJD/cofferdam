//! Inline suppression directives parser and matcher.
//!
//! Two surface families coexist (cd-81a.4):
//!
//! ## Biome-style (primary)
//! - `// cofferdam-ignore: <Check.Id>` — silence on next non-blank line
//! - `// cofferdam-ignore: <Check.Id>: <reason>` — same, with required-by-policy reason
//! - `// cofferdam-ignore` — silence ALL checks on next non-blank line (broad)
//! - `// cofferdam-ignore-start: <Check.Id>` … `// cofferdam-ignore-end` — range
//! - `// cofferdam-ignore-file: <Check.Id>` — silence for the whole file
//! - `// cofferdam-ignore-file` — silence ALL checks for the whole file
//!
//! ## ESLint-style (aliases, accepted but not canonical)
//! - `// cofferdam-disable-next-line [Check.Id, ...]` — next-line
//! - `/* cofferdam-disable [Check.Id, ...] */` … `/* cofferdam-enable [Check.Id, ...] */`
//!   — block range
//! - `// cofferdam-disable-file [Check.Id, ...]` — whole-file
//!
//! Both surfaces parse into the same internal `Suppressions` map; downstream
//! suppression filtering doesn't know which spelling produced an entry. The
//! formatter normalises to the Biome-style on autofix in a future bead.
//!
//! ## Reason field policy
//! Reasons (the text after the second colon in Biome-style) are parsed but
//! not yet enforced. cd-cmb tracks "Warning category requires a reason"
//! once it lands.

use std::collections::{HashMap, HashSet};

/// Parsed suppression state for a file.
///
/// Tracks which checks (or all checks) are suppressed on each line,
/// the active block suppressions as we scan forward, and the file-wide
/// suppressions parsed from `cofferdam-ignore-file:` / `cofferdam-disable-file`
/// directives.
pub struct Suppressions {
    /// Per-line suppression: line -> set of check IDs (empty = all checks suppressed)
    line_suppressions: HashMap<u32, HashSet<String>>,
    /// Per-line "suppress all" flag (faster than storing empty set)
    line_suppress_all: HashSet<u32>,
    /// Whole-file suppressions for specific check IDs.
    file_suppressions: HashSet<String>,
    /// Whole-file "suppress all checks" flag.
    file_suppress_all: bool,
}

impl Suppressions {
    /// Parse a file's text into a suppression map.
    pub fn parse(text: &str) -> Self {
        let mut line_suppressions: HashMap<u32, HashSet<String>> = HashMap::new();
        let mut line_suppress_all: HashSet<u32> = HashSet::new();
        let mut file_suppressions: HashSet<String> = HashSet::new();
        let mut file_suppress_all = false;

        let lines: Vec<&str> = text.lines().collect();
        let mut active_blocks: Vec<BlockSuppression> = Vec::new();

        for (idx, line) in lines.iter().enumerate() {
            let line_num = (idx + 1) as u32;
            let trimmed = line.trim();

            // ---- File-wide directives (Biome and ESLint spellings) ----
            // Whole-file suppressions parsed in scan order; the "must be
            // at top of file" convention is policy, not enforced here.
            if let Some(ids) = parse_file_directive(trimmed) {
                if ids.is_empty() {
                    file_suppress_all = true;
                } else {
                    file_suppressions.extend(ids);
                }
                continue;
            }

            // ---- Block start (Biome `-ignore-start` + ESLint `-disable`) ----
            if trimmed.starts_with("//") || trimmed.starts_with("/*") {
                if let Some(ids) = parse_block_start(trimmed) {
                    active_blocks.push(BlockSuppression { check_ids: ids });
                } else if let Some(ids) = parse_block_end(trimmed) {
                    if ids.is_empty() {
                        active_blocks.clear();
                    } else {
                        for id in &ids {
                            if let Some(pos) = active_blocks
                                .iter()
                                .rposition(|b| b.check_ids.is_empty() || b.check_ids.contains(id))
                            {
                                active_blocks.remove(pos);
                            }
                        }
                    }
                }
            }

            // Apply active block suppressions to this line
            for block in &active_blocks {
                if block.check_ids.is_empty() {
                    line_suppress_all.insert(line_num);
                } else {
                    line_suppressions
                        .entry(line_num)
                        .or_default()
                        .extend(block.check_ids.iter().cloned());
                }
            }

            // ---- Next-line directives (Biome + ESLint) ----
            if let Some(check_ids) = parse_next_line_directive(trimmed) {
                if let Some(next_line_num) = find_next_non_blank_line(&lines, idx) {
                    if check_ids.is_empty() {
                        line_suppress_all.insert(next_line_num);
                    } else {
                        line_suppressions
                            .entry(next_line_num)
                            .or_default()
                            .extend(check_ids);
                    }
                }
            }
        }

        Self {
            line_suppressions,
            line_suppress_all,
            file_suppressions,
            file_suppress_all,
        }
    }

    /// True when an issue with the given check_id at the given 1-based line
    /// is suppressed.
    pub fn is_suppressed(&self, line: u32, check_id: &str) -> bool {
        if self.file_suppress_all {
            return true;
        }
        if self.file_suppressions.contains(check_id) {
            return true;
        }
        if self.line_suppress_all.contains(&line) {
            return true;
        }
        if let Some(ids) = self.line_suppressions.get(&line) {
            if ids.contains(check_id) {
                return true;
            }
        }
        false
    }
}

/// An active block suppression: tracks which check IDs it applies to.
struct BlockSuppression {
    /// Empty = all checks, non-empty = specific checks
    check_ids: HashSet<String>,
}

/// Parse `// cofferdam-disable-next-line` or `// cofferdam-disable-next-line Check.Id, Other.Id`
/// Returns Some(check_ids) where check_ids is empty if ALL checks, non-empty for specific checks.
/// Returns None if not a disable-next-line directive.
fn parse_disable_next_line(line: &str) -> Option<Vec<String>> {
    if !line.contains("cofferdam-disable-next-line") {
        return None;
    }

    // Find the directive marker
    if let Some(idx) = line.find("cofferdam-disable-next-line") {
        let after_marker = &line[idx + "cofferdam-disable-next-line".len()..];
        let check_ids = parse_check_ids(after_marker);
        Some(check_ids)
    } else {
        None
    }
}

/// Parse Biome-style next-line directives. Accepted spellings:
///   - `// cofferdam-ignore: <CheckId>[: <reason>]`           (canonical)
///   - `// cofferdam-ignore <CheckId>[ <sep> <reason>]`       (space form, cd-b77)
///   - `// cofferdam-ignore` (broad form, suppresses ALL checks)
///
/// Returns `Some([])` for the broad form, `Some([id])` for the scoped form,
/// `None` when the line does not contain a Biome ignore directive.
///
/// The space form (cd-b77 / gh #42) accepts any reason separator a human
/// would naturally write: ASCII hyphen `-`, em-dash `—`, colon `:`, or
/// just whitespace. The parser doesn't validate the separator — it takes
/// the first whitespace-delimited token (with any trailing colon trimmed)
/// and binds it as the check id if the token looks check-id-shaped
/// (`Category.Name`). Prose comments that happen to mention
/// `cofferdam-ignore` but whose next token is not a check id are left
/// alone, matching the prior behaviour.
fn parse_biome_ignore_next_line(line: &str) -> Option<Vec<String>> {
    // Reject the multi-form spellings (-start / -end / -file) — they have
    // their own parsers. We also need to reject the ESLint `-disable-*`
    // variants which contain "ignore" as a substring of "cofferdam-ignore"
    // is not a substring of "cofferdam-disable", so those are safe.
    let needle = "cofferdam-ignore";
    let idx = line.find(needle)?;
    let after = &line[idx + needle.len()..];

    // Distinguish the multi-form spellings — bail if any of them present.
    if after.starts_with("-start") || after.starts_with("-end") || after.starts_with("-file") {
        return None;
    }

    let after_trim = after.trim_start();
    if !after_trim.starts_with(':') {
        // No colon after the marker. Three sub-cases:
        //   (a) empty post-marker → broad form.
        //   (b) post-marker first token is check-id-shaped → space-form
        //       scoped directive (cd-b77). Binds the token as the id.
        //   (c) post-marker is prose → not a directive; return None.
        let stripped = after_trim.trim_end_matches("*/").trim();
        if stripped.is_empty() {
            return Some(Vec::new());
        }
        let first_token = first_token_as_id(stripped);
        if let Some(id) = first_token {
            return Some(vec![id]);
        }
        return None;
    }

    // `// cofferdam-ignore: <Id>[: <reason>]` — strip the leading `:`,
    // then take the first colon-delimited segment as the ID; everything
    // after the second colon is the optional reason (parsed but ignored
    // for now — cd-cmb tracks the policy follow-up).
    let payload = &after_trim[1..]; // past the first colon
    let payload = payload.trim_end_matches("*/").trim();
    let id = match payload.split_once(':') {
        Some((id, _reason)) => id.trim(),
        None => payload,
    };
    if id.is_empty() {
        // `// cofferdam-ignore:` with nothing after — treat as broad,
        // matching the bare-form semantics.
        return Some(Vec::new());
    }
    Some(vec![id.to_string()])
}

/// If the first whitespace-delimited token in `stripped` looks
/// check-id-shaped (`Category.Name` — contains a dot, ASCII identifier
/// chars), return it. The trailing `:` of a `id: reason` form is
/// trimmed before the shape test so authors can keep using the
/// colon-reason separator alongside the space-id form.
///
/// Conservative on purpose: prose like `cofferdam-ignore please` won't
/// bind "please" as an id; `cofferdam-ignore Design.OrphanExport reason`
/// binds `Design.OrphanExport`.
fn first_token_as_id(stripped: &str) -> Option<String> {
    let raw = stripped.split_whitespace().next()?;
    let candidate = raw.trim_end_matches(':');
    if looks_like_check_id(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

/// Heuristic: a token is plausibly a `Category.Name`-style check id if
/// it contains at least one dot, starts with an ASCII letter, and only
/// uses ASCII identifier chars / dots / underscores. Conservative —
/// avoids binding random English words as ids.
pub(crate) fn looks_like_check_id(s: &str) -> bool {
    if s.is_empty() || !s.contains('.') {
        return false;
    }
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}

/// Combined next-line parser: tries Biome form first, falls back to ESLint
/// `cofferdam-disable-next-line`. Returns `None` if neither matched.
fn parse_next_line_directive(line: &str) -> Option<Vec<String>> {
    if let Some(ids) = parse_biome_ignore_next_line(line) {
        return Some(ids);
    }
    parse_disable_next_line(line)
}

/// Parse Biome `-ignore-file:` / `-ignore-file` and ESLint
/// `cofferdam-disable-file`. Returns the IDs to suppress for the whole
/// file, or `Some([])` for "all checks". `None` when not a file directive.
fn parse_file_directive(line: &str) -> Option<Vec<String>> {
    // Biome: `// cofferdam-ignore-file: <Id>` or `// cofferdam-ignore-file`
    if let Some(idx) = line.find("cofferdam-ignore-file") {
        let after = &line[idx + "cofferdam-ignore-file".len()..];
        let after = after.trim_start();
        let payload = after.trim_start_matches(':').trim_end_matches("*/").trim();
        if payload.is_empty() {
            return Some(Vec::new());
        }
        let id = match payload.split_once(':') {
            Some((id, _reason)) => id.trim(),
            None => payload,
        };
        if id.is_empty() {
            return Some(Vec::new());
        }
        return Some(vec![id.to_string()]);
    }
    // ESLint: `// cofferdam-disable-file [Id, ...]`
    if let Some(idx) = line.find("cofferdam-disable-file") {
        let after = &line[idx + "cofferdam-disable-file".len()..];
        return Some(parse_check_ids(after));
    }
    None
}

/// Combined block-start parser: Biome `-ignore-start:` and ESLint
/// `cofferdam-disable` (block form).
fn parse_block_start(line: &str) -> Option<HashSet<String>> {
    // Biome `-ignore-start: <Id>` (single id; reason allowed)
    if let Some(idx) = line.find("cofferdam-ignore-start") {
        let after = &line[idx + "cofferdam-ignore-start".len()..];
        let after = after.trim_start();
        let payload = after.trim_start_matches(':').trim_end_matches("*/").trim();
        if payload.is_empty() {
            return Some(HashSet::new());
        }
        let id = match payload.split_once(':') {
            Some((id, _reason)) => id.trim(),
            None => payload,
        };
        if id.is_empty() {
            return Some(HashSet::new());
        }
        let mut set = HashSet::new();
        set.insert(id.to_string());
        return Some(set);
    }
    // ESLint `/* cofferdam-disable [Id, ...] */`
    parse_disable_block_start(line)
}

/// Combined block-end parser: Biome `-ignore-end` and ESLint `cofferdam-enable`.
fn parse_block_end(line: &str) -> Option<Vec<String>> {
    if let Some(idx) = line.find("cofferdam-ignore-end") {
        let after = &line[idx + "cofferdam-ignore-end".len()..];
        let after = after.trim_start();
        let payload = after.trim_start_matches(':').trim_end_matches("*/").trim();
        if payload.is_empty() {
            return Some(Vec::new());
        }
        return Some(vec![payload.to_string()]);
    }
    parse_enable_block_end(line)
}

/// Parse `/* cofferdam-disable */` or `/* cofferdam-disable Check.Id, Other.Id */`
/// Returns Some(check_ids) where check_ids is empty if ALL checks.
/// Returns None if not a disable block start.
fn parse_disable_block_start(line: &str) -> Option<HashSet<String>> {
    if !line.contains("cofferdam-disable") {
        return None;
    }

    // Check for "disable" (not "enable")
    if line.contains("cofferdam-enable") {
        return None;
    }

    if let Some(idx) = line.find("cofferdam-disable") {
        let after_marker = &line[idx + "cofferdam-disable".len()..];
        let check_ids = parse_check_ids(after_marker);
        Some(check_ids.into_iter().collect())
    } else {
        None
    }
}

/// Parse `/* cofferdam-enable */` or `/* cofferdam-enable Check.Id, Other.Id */`
/// Returns Some(check_ids) where check_ids is empty if ALL blocks should end.
/// Returns None if not an enable block end.
fn parse_enable_block_end(line: &str) -> Option<Vec<String>> {
    if !line.contains("cofferdam-enable") {
        return None;
    }

    if let Some(idx) = line.find("cofferdam-enable") {
        let after_marker = &line[idx + "cofferdam-enable".len()..];
        let check_ids = parse_check_ids(after_marker);
        Some(check_ids)
    } else {
        None
    }
}

/// The form of a suppression directive that covers a given line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuppressionForm {
    /// `// cofferdam-ignore: <Id>` or `// cofferdam-disable-next-line <Id>` —
    /// suppresses the next non-blank line.
    NextLine,
    /// `// cofferdam-ignore-start: <Id>` … `// cofferdam-ignore-end` or
    /// `/* cofferdam-disable <Id> */` … `/* cofferdam-enable <Id> */` block.
    Range,
    /// `// cofferdam-ignore-file: <Id>` or `// cofferdam-disable-file <Id>` —
    /// whole-file suppression.
    File,
}

/// An individual suppression entry with its form for audit purposes.
#[derive(Debug, Clone)]
pub struct SuppressionEntry {
    /// The check ID that is suppressed.
    pub check_id: String,
    /// The 1-based line number on which the finding is suppressed (for
    /// next-line and range forms: the covered code line; for file-wide: 0).
    pub line: u32,
    /// How the suppression was declared.
    pub form: SuppressionForm,
    /// The raw directive text (trimmed).
    pub directive_text: String,
}

/// Parse all suppression entries from a file's text, returning one
/// `SuppressionEntry` per (check_id, covered_line) pair.  Used by
/// `cofferdam baseline lint` to intersect against the baseline.
///
/// The "covered line" for next-line directives is the first non-blank
/// line after the directive; for range directives it is every line in the
/// range; for file directives it is every line in the file (represented
/// as 0 here for brevity — callers should treat 0 as "covers all lines").
pub fn parse_suppression_entries(text: &str) -> Vec<SuppressionEntry> {
    let mut out: Vec<SuppressionEntry> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut active_blocks: Vec<(String, String)> = Vec::new(); // (check_id, directive_text)

    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as u32;
        let trimmed = line.trim();

        // ---- File-wide directives ----
        if let Some(ids) = parse_file_directive(trimmed) {
            if ids.is_empty() {
                // Suppress all — we don't enumerate all check IDs here;
                // callers handle this by treating an empty check_id as "all".
                out.push(SuppressionEntry {
                    check_id: String::new(),
                    line: 0,
                    form: SuppressionForm::File,
                    directive_text: trimmed.to_string(),
                });
            } else {
                for id in ids {
                    out.push(SuppressionEntry {
                        check_id: id,
                        line: 0,
                        form: SuppressionForm::File,
                        directive_text: trimmed.to_string(),
                    });
                }
            }
            continue;
        }

        // ---- Block start ----
        if trimmed.starts_with("//") || trimmed.starts_with("/*") {
            if let Some(ids) = parse_block_start(trimmed) {
                for id in &ids {
                    active_blocks.push((id.clone(), trimmed.to_string()));
                }
                if ids.is_empty() {
                    active_blocks.push((String::new(), trimmed.to_string()));
                }
            } else if let Some(ids) = parse_block_end(trimmed) {
                if ids.is_empty() {
                    active_blocks.clear();
                } else {
                    for id in &ids {
                        if let Some(pos) = active_blocks
                            .iter()
                            .rposition(|(bid, _)| bid.is_empty() || bid == id)
                        {
                            active_blocks.remove(pos);
                        }
                    }
                }
            }
        }

        // Emit range entries for active blocks on this line.
        for (check_id, directive_text) in &active_blocks {
            out.push(SuppressionEntry {
                check_id: check_id.clone(),
                line: line_num,
                form: SuppressionForm::Range,
                directive_text: directive_text.clone(),
            });
        }

        // ---- Next-line directives ----
        if let Some(check_ids) = parse_next_line_directive(trimmed) {
            if let Some(next_line_num) = find_next_non_blank_line(&lines, idx) {
                if check_ids.is_empty() {
                    out.push(SuppressionEntry {
                        check_id: String::new(),
                        line: next_line_num,
                        form: SuppressionForm::NextLine,
                        directive_text: trimmed.to_string(),
                    });
                } else {
                    for id in check_ids {
                        out.push(SuppressionEntry {
                            check_id: id,
                            line: next_line_num,
                            form: SuppressionForm::NextLine,
                            directive_text: trimmed.to_string(),
                        });
                    }
                }
            }
        }
    }

    out
}

/// Extract all named check IDs referenced by suppression directives in `text`.
///
/// Returns a list of `(check_id, 1-based_line_number)` pairs. Only directives
/// that name at least one check ID are included — blanket suppressions
/// (e.g. `// cofferdam-disable-next-line` with no ID) are skipped because
/// they suppress *all* checks and don't reference any ID to validate.
///
/// Used by `cofferdam doctor` to validate that directive IDs match registered
/// checks without re-implementing the full suppression-parse logic.
pub fn mentioned_check_ids(text: &str) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line_num = (idx + 1) as u32;
        let trimmed = line.trim();

        // `// cofferdam-disable-next-line Check.Id`
        if let Some(ids) = parse_disable_next_line(trimmed) {
            for id in ids {
                if !id.is_empty() {
                    out.push((id, line_num));
                }
            }
        }

        // `/* cofferdam-disable Check.Id */`
        if trimmed.starts_with("/*") {
            if let Some(ids) = parse_disable_block_start(trimmed) {
                for id in ids {
                    if !id.is_empty() {
                        out.push((id, line_num));
                    }
                }
            }
        }
    }
    out
}

/// Parse a comma-separated list of check IDs from the remainder of a directive.
/// Returns empty vec if no IDs found, otherwise returns the parsed IDs.
fn parse_check_ids(remainder: &str) -> Vec<String> {
    remainder
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !s.starts_with("*/") && !s.starts_with("//"))
        .map(|s| {
            // Stop at comment end or next comment start
            if let Some(end_idx) = s.find("*/") {
                s[..end_idx].trim().to_string()
            } else {
                s.to_string()
            }
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Find the next non-blank line starting from the given index (exclusive).
fn find_next_non_blank_line(lines: &[&str], from_idx: usize) -> Option<u32> {
    for (offset, line) in lines.iter().skip(from_idx + 1).enumerate() {
        if !line.trim().is_empty() {
            return Some((from_idx + offset + 2) as u32);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disable_next_line_all_checks() {
        let text = "// cofferdam-disable-next-line\nif (a == b) { }";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(2, "Refactor.CyclomaticComplexity"));
        assert!(!sup.is_suppressed(1, "Warning.TripleEquals"));
    }

    #[test]
    fn test_disable_next_line_specific_checks() {
        let text = "// cofferdam-disable-next-line Warning.TripleEquals, Refactor.CyclomaticComplexity\nif (a == b) { }";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(2, "Refactor.CyclomaticComplexity"));
        assert!(!sup.is_suppressed(2, "Readability.MaxLineLength"));
    }

    #[test]
    fn test_disable_next_line_skips_blank_lines() {
        let text = "// cofferdam-disable-next-line\n\nif (a == b) { }";
        let sup = Suppressions::parse(text);
        assert!(!sup.is_suppressed(2, "Warning.TripleEquals")); // blank line
        assert!(sup.is_suppressed(3, "Warning.TripleEquals")); // first non-blank
    }

    #[test]
    fn test_block_disable_all() {
        let text =
            "/* cofferdam-disable */\nif (a == b) { }\n/* cofferdam-enable */\nif (x == y) { }";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(!sup.is_suppressed(4, "Warning.TripleEquals"));
    }

    #[test]
    fn test_block_disable_specific_checks() {
        let text = "/* cofferdam-disable Warning.TripleEquals */\nif (a == b) { }\n/* cofferdam-enable */\nif (x == y) { }";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(!sup.is_suppressed(2, "Readability.MaxLineLength"));
        assert!(!sup.is_suppressed(4, "Warning.TripleEquals"));
    }

    #[test]
    fn test_block_disable_multiple_checks() {
        let text = "/* cofferdam-disable Warning.TripleEquals, Design.MaxParameters */\nfunction f(a,b,c,d,e,f,g) { if (x == y) {} }\n/* cofferdam-enable */";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(2, "Design.MaxParameters"));
        assert!(!sup.is_suppressed(2, "Readability.MaxLineLength"));
    }

    #[test]
    fn test_block_no_matching_enable() {
        let text = "/* cofferdam-disable Warning.TripleEquals */\nif (a == b) { }\nif (x == y) { }";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(3, "Warning.TripleEquals")); // extends to EOF
    }

    #[test]
    fn test_directive_at_eof() {
        let text = "const x = 1;\n// cofferdam-disable-next-line";
        let sup = Suppressions::parse(text);
        // No next non-blank line exists; directive is a no-op
        assert!(!sup.is_suppressed(2, "Warning.TripleEquals"));
    }

    #[test]
    fn test_nested_blocks_last_wins() {
        let text = "/* cofferdam-disable Warning.TripleEquals */\n/* cofferdam-disable Design.MaxParameters */\nif (a == b) { }\n/* cofferdam-enable Design.MaxParameters */\nif (x == y) { }";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(3, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(3, "Design.MaxParameters"));
        // After enabling Design.MaxParameters, it should be unsuppressed but Warning.TripleEquals remains
        assert!(sup.is_suppressed(5, "Warning.TripleEquals"));
        assert!(!sup.is_suppressed(5, "Design.MaxParameters"));
    }

    #[test]
    fn test_unknown_check_ids_accepted() {
        let text = "// cofferdam-disable-next-line Bogus.Whatever\nif (a == b) { }";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Bogus.Whatever"));
        // Still suppressed even if check ID doesn't exist
    }

    #[test]
    fn test_whitespace_handling() {
        let text = "//  cofferdam-disable-next-line   Warning.TripleEquals  ,  Design.MaxParameters  \nif (a == b) { }";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(2, "Design.MaxParameters"));
    }

    #[test]
    fn test_block_enable_all_clears_all() {
        let text = "/* cofferdam-disable Warning.TripleEquals */\n/* cofferdam-disable Design.MaxParameters */\nif (a == b) { }\n/* cofferdam-enable */\nif (x == y) { }";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(3, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(3, "Design.MaxParameters"));
        assert!(!sup.is_suppressed(5, "Warning.TripleEquals"));
        assert!(!sup.is_suppressed(5, "Design.MaxParameters"));
    }

    // ---- cd-81a.4: Biome-style primary syntax ----

    #[test]
    fn biome_ignore_with_id_suppresses_next_line() {
        let text = "// cofferdam-ignore: Warning.TripleEquals\nif (a == b) { }";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(!sup.is_suppressed(2, "Design.MaxParameters"));
    }

    #[test]
    fn biome_ignore_with_id_and_reason_strips_reason() {
        let text =
            "// cofferdam-ignore: BrandCasing: see ROVI-481 — copywriter approved exception\nconst x = \"Examplco Spring\";";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "BrandCasing"));
        assert!(!sup.is_suppressed(2, "Warning.TripleEquals"));
    }

    #[test]
    fn biome_ignore_bare_form_suppresses_all_on_next_line() {
        let text = "// cofferdam-ignore\nif (a == b) { }";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(2, "Design.MaxParameters"));
    }

    #[test]
    fn biome_ignore_block_form_with_explicit_end() {
        let text = "// cofferdam-ignore-start: Warning.TripleEquals\nif (a == b) {}\nif (x == y) {}\n// cofferdam-ignore-end\nif (c == d) {}";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(3, "Warning.TripleEquals"));
        // After -ignore-end, suppression lifts.
        assert!(!sup.is_suppressed(5, "Warning.TripleEquals"));
    }

    #[test]
    fn biome_ignore_file_with_id_suppresses_whole_file() {
        let text = "// cofferdam-ignore-file: Warning.TripleEquals\nif (a == b) {}\nif (x == y) {}";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(3, "Warning.TripleEquals"));
        // Other checks still fire.
        assert!(!sup.is_suppressed(2, "Readability.MaxLineLength"));
    }

    #[test]
    fn biome_ignore_file_bare_suppresses_everything() {
        let text = "// cofferdam-ignore-file\nif (a == b) {}\nfunction f(a,b,c,d,e,f,g) {}";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(3, "Design.MaxParameters"));
    }

    #[test]
    fn eslint_disable_file_alias_works() {
        let text = "// cofferdam-disable-file Warning.TripleEquals\nif (a == b) {}\nif (x == y) {}";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
        assert!(sup.is_suppressed(3, "Warning.TripleEquals"));
        assert!(!sup.is_suppressed(2, "Design.MaxParameters"));
    }

    #[test]
    fn biome_and_eslint_styles_coexist() {
        // ESLint block + Biome next-line in the same file.
        let text = "/* cofferdam-disable Design.MaxParameters */\nfunction f(a,b,c,d,e,f,g) {\n  // cofferdam-ignore: Warning.TripleEquals\n  if (a == b) {}\n}\n/* cofferdam-enable */";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Design.MaxParameters")); // ESLint block
        assert!(sup.is_suppressed(4, "Warning.TripleEquals")); // Biome next-line
    }

    #[test]
    fn biome_ignore_on_block_comment_form() {
        // The Biome syntax also accepts `/* cofferdam-ignore: ... */`.
        let text = "/* cofferdam-ignore: Warning.TripleEquals */\nif (a == b) {}";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
    }

    // ---- cd-b77 / gh #42: space-separator next-line form ----

    #[test]
    fn space_form_em_dash_reason_suppresses() {
        // The form reported in gh #42: space-separated id, em-dash reason.
        let text = "// cofferdam-ignore Design.OrphanExport — Vite entry referenced via index.html\nexport default function App() {}";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Design.OrphanExport"));
        assert!(!sup.is_suppressed(2, "Warning.TripleEquals"));
    }

    #[test]
    fn space_form_ascii_hyphen_reason_suppresses() {
        let text =
            "// cofferdam-ignore Warning.TripleEquals - intentional == coercion\nif (a == b) {}";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
    }

    #[test]
    fn space_form_colon_reason_suppresses() {
        // No leading colon — id followed by `: reason`. Trailing colon
        // on the id token gets trimmed so this still binds the id.
        let text =
            "// cofferdam-ignore Design.MaxParameters: refactor pending\nfunction f(a,b,c,d,e,f,g) {}";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Design.MaxParameters"));
    }

    #[test]
    fn space_form_bare_id_no_reason_suppresses() {
        let text = "// cofferdam-ignore Warning.TripleEquals\nif (a == b) {}";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
    }

    #[test]
    fn space_form_prose_after_marker_not_a_directive() {
        // The first token isn't check-id-shaped (no dot) — leave it as prose.
        let text = "// cofferdam-ignore please understand this is intentional\nif (a == b) {}";
        let sup = Suppressions::parse(text);
        assert!(
            !sup.is_suppressed(2, "Warning.TripleEquals"),
            "prose mentioning the marker must not silently suppress",
        );
    }

    #[test]
    fn space_form_inside_block_comment_suppresses() {
        let text =
            "/* cofferdam-ignore Warning.TripleEquals — block-comment form */\nif (a == b) {}";
        let sup = Suppressions::parse(text);
        assert!(sup.is_suppressed(2, "Warning.TripleEquals"));
    }

    #[test]
    fn looks_like_check_id_heuristic() {
        assert!(looks_like_check_id("Design.OrphanExport"));
        assert!(looks_like_check_id("Warning.TripleEquals"));
        assert!(looks_like_check_id("Plugin.Custom.Subcheck"));
        assert!(!looks_like_check_id("please"));
        assert!(!looks_like_check_id(""));
        assert!(!looks_like_check_id("123.bad"));
        assert!(!looks_like_check_id("no-dashes-allowed.x"));
        assert!(!looks_like_check_id("Just_underscores"));
    }
}
