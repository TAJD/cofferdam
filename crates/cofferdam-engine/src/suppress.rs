//! Inline suppression directives parser and matcher.
//!
//! Supports three directive forms:
//! - `// cofferdam-disable-next-line` — silences all findings on next non-blank line
//! - `// cofferdam-disable-next-line Check.Id, Other.Check` — silences specific checks
//! - `/* cofferdam-disable */` … `/* cofferdam-enable */` — block form
//!
//! Block directives can be scoped:
//! - `/* cofferdam-disable Check.Id */` … `/* cofferdam-enable Check.Id */`
//! - `/* cofferdam-disable Check.One, Check.Two */` — multiple ids

use std::collections::{HashMap, HashSet};

/// Parsed suppression state for a file.
///
/// Tracks which checks (or all checks) are suppressed on each line,
/// and the active block suppressions as we scan forward.
pub struct Suppressions {
    /// Per-line suppression: line -> set of check IDs (empty = all checks suppressed)
    line_suppressions: HashMap<u32, HashSet<String>>,
    /// Per-line "suppress all" flag (faster than storing empty set)
    line_suppress_all: HashSet<u32>,
}

impl Suppressions {
    /// Parse a file's text into a suppression map.
    pub fn parse(text: &str) -> Self {
        let mut line_suppressions: HashMap<u32, HashSet<String>> = HashMap::new();
        let mut line_suppress_all: HashSet<u32> = HashSet::new();

        let lines: Vec<&str> = text.lines().collect();
        let mut active_blocks: Vec<BlockSuppression> = Vec::new();

        for (idx, line) in lines.iter().enumerate() {
            let line_num = (idx + 1) as u32;
            let trimmed = line.trim();

            // Check for block control directives (must appear at line start)
            if trimmed.starts_with("/*") {
                if let Some(disable_ids) = parse_disable_block_start(trimmed) {
                    // Start a new block suppression
                    active_blocks.push(BlockSuppression {
                        check_ids: disable_ids,
                    });
                } else if let Some(enable_ids) = parse_enable_block_end(trimmed) {
                    // End matching active block(s)
                    // For simplicity: remove the most recent block that matches
                    // (empty enable_ids means "disable all" blocks, scoped ones match their ids)
                    if enable_ids.is_empty() {
                        // /* cofferdam-enable */ — ends all active blocks
                        active_blocks.clear();
                    } else {
                        // /* cofferdam-enable Check.Id */ — ends blocks with those ids
                        // Last-wins: remove the last block containing any of these ids
                        for id in &enable_ids {
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
                    // All checks suppressed in this block
                    line_suppress_all.insert(line_num);
                } else {
                    // Specific checks suppressed
                    line_suppressions
                        .entry(line_num)
                        .or_default()
                        .extend(block.check_ids.iter().cloned());
                }
            }

            // Check for next-line directives
            if let Some(check_ids) = parse_disable_next_line(trimmed) {
                // Find the next non-blank line
                if let Some(next_line_num) = find_next_non_blank_line(&lines, idx) {
                    if check_ids.is_empty() {
                        // All checks suppressed on next non-blank line
                        line_suppress_all.insert(next_line_num);
                    } else {
                        // Specific checks suppressed
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
        }
    }

    /// True when an issue with the given check_id at the given 1-based line
    /// is suppressed.
    pub fn is_suppressed(&self, line: u32, check_id: &str) -> bool {
        // Line suppresses all checks?
        if self.line_suppress_all.contains(&line) {
            return true;
        }

        // Line suppresses this specific check?
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
}
