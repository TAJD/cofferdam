//! Baseline workflow — snapshot today's findings, fail CI only on what's new.
//!
//! This is the headline adoption unlock (cd-d31): without it, dropping
//! cofferdam into any pre-existing TS project breaks the build instantly
//! because every legitimate finding produces exit code 1.
//!
//! ## Identity
//!
//! Each finding is keyed by `(file, check_id, rule_signature)`. The first
//! two are obvious; `rule_signature` is the load-bearing decision and
//! deserves explanation:
//!
//! - **NOT line numbers.** Line numbers churn on every reformat — basing
//!   identity on them invalidates the baseline the first time anyone runs
//!   `prettier`.
//! - **Default = SHA-256 of the offending span text with all internal
//!   whitespace runs collapsed to a single space.** Robust to line moves,
//!   re-indentation, line-wrap shifts, CRLF/LF differences, and alignment
//!   padding — only changes when the offending tokens themselves change.
//!
//! - **Per-check override (cd-9, "rulesig"): function-scoped checks hash
//!   the enclosing function's header, not its body.** `Refactor.LongAndComplex`
//!   flags a whole function/method — hashing the full span text meant ANY
//!   edit inside a large flagged function (even one that doesn't change its
//!   size/complexity classification) drifted the signature and forced a
//!   spurious re-baseline. See [`FUNCTION_SCOPED_SIGNATURE_CHECK_IDS`] and
//!   [`symbol_signature_for_span`].
//!
//! ## File format
//!
//! Versioned JSON, pretty-printed with 2-space indent and entries sorted
//! by `(file, check_id, rule_signature)` so diffs are reviewable.
//!
//! ```json
//! {
//!   "version": 2,
//!   "findings": [
//!     {
//!       "file": "src/foo.ts",
//!       "check_id": "Warning.TripleEquals",
//!       "rule_signature": "5b1c..."
//!     }
//!   ]
//! }
//! ```
//!
//! Schema is additive — new fields are fine, renames/type changes break
//! existing baselines.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Component, Path, PathBuf};

use cofferdam_core::Issue;
#[cfg(test)]
use cofferdam_core::{Location, Priority, Severity, Span};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Current baseline schema version. Bump on incompatible changes only.
///
/// V2 (cd-1h7): `signature_for_span` collapses internal whitespace runs
/// before hashing, so `prettier --write` and similar reformatters can no
/// longer drift signatures. V1 baselines must be regenerated via
/// `cofferdam baseline write` once after upgrade — `read` rejects them
/// with `BaselineError::Version` so the user sees a clear failure mode
/// rather than silent invalidation of every baselined entry.
///
/// V3 (cd-9, "rulesig"): function-scoped checks (see
/// [`FUNCTION_SCOPED_SIGNATURE_CHECK_IDS`]) now hash the enclosing
/// function's header instead of its whole body, so unrelated edits inside
/// a flagged function no longer drift the signature. V2 baselines must be
/// regenerated the same way as the V1→V2 transition.
pub const VERSION: u32 = 3;

/// Default baseline file path, relative to the project root. Kept here
/// so the CLI and any other consumer agree on the convention.
pub const DEFAULT_PATH: &str = ".cofferdam/baseline.json";

/// One row in the on-disk baseline file. Keys a known-accepted
/// finding by (file, check_id, rule_signature) so a fresh analyzer
/// run can decide whether each finding is new or pre-existing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct BaselineEntry {
    /// Forward-slash relative path to the file containing the finding.
    /// Stored relative to the baseline file's parent directory's parent
    /// (typically the repo root) so baselines remain stable across
    /// different developers' workspace prefixes.
    pub file: String,
    /// Dotted check ID from `CheckMeta` — the same identifier used in
    /// config, suppression directives, and SARIF rule IDs.
    pub check_id: String,
    /// Hex-encoded SHA-256 of the offending span text with internal
    /// whitespace runs collapsed (V2). See module docs for why this is
    /// text-based, not line-based.
    pub rule_signature: String,
}

/// On-disk baseline document — schema-versioned bundle of accepted
/// findings. Persisted as JSON at `.cofferdam/baseline.json` by
/// default; the path is configurable via `--baseline`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    /// Schema version. Loader rejects mismatches via
    /// [`BaselineError::Version`] so a future format change can't be
    /// misread by an older binary.
    pub version: u32,
    /// Sorted list of accepted findings. Sort is stable on
    /// (file, check_id, rule_signature) so the file diffs cleanly
    /// in PR review.
    pub findings: Vec<BaselineEntry>,
}

impl Baseline {
    /// Construct a `Baseline` from a list of entries, sorting them
    /// into the canonical order so the on-disk JSON is reproducible.
    /// Stamps `version` with the current `VERSION` constant.
    pub fn new(findings: Vec<BaselineEntry>) -> Self {
        let mut findings = findings;
        sort_entries(&mut findings);
        Self {
            version: VERSION,
            findings,
        }
    }

    /// Build a `HashSet` view for O(1) membership tests when matching
    /// fresh findings against this baseline.
    pub fn lookup_set(&self) -> HashSet<&BaselineEntry> {
        self.findings.iter().collect()
    }
}

/// Errors that can prevent a baseline from loading. IO failure,
/// malformed JSON, and schema-version mismatches each have their own
/// variant so the CLI can format actionable diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum BaselineError {
    #[error("failed to read baseline {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse baseline {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "baseline {path} declares unsupported version {found}; this binary supports {supported}. \
         Run `cofferdam baseline write` to regenerate it under the current schema."
    )]
    Version {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    #[error("failed to write baseline {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to serialize baseline: {source}")]
    Serialize {
        #[source]
        source: serde_json::Error,
    },
}

/// Compute the canonical signature for a span's text.
///
/// Collapses every run of ASCII whitespace (spaces, tabs, newlines, CR)
/// down to a single space and trims the ends before hashing. This is the
/// V2 algorithm (cd-1h7): V1 only `.trim()`d the ends, so internal
/// whitespace shifts from `prettier --write` (re-indent, line-wrap, CRLF
/// vs LF) re-hashed the same logical finding to a new signature and
/// silently invalidated baselines on a no-op reformat.
///
/// The collapse is non-strict in one direction: whitespace inside string
/// literals inside the span also collapses, so `"a  b"` and `"a b"` hash
/// identically. That's a deliberate trade — the alternative would be a
/// lex pass per signature, and the precision gain is negligible since
/// baselines key by `(file, check_id, signature)` and the check_id
/// already separates "same span, different lint."
///
/// Returns hex-encoded SHA-256 (64 chars). We keep the full digest rather
/// than truncating: collisions on truncated hashes inside a single repo
/// are unlikely but the cost of avoiding them is one constant.
pub fn signature_for_span(file_text: &str, location: &cofferdam_core::Location) -> String {
    hash_collapsed(span_snippet(file_text, location))
}

/// Check IDs whose finding is scoped to an entire enclosing function or
/// method, rather than to the specific tokens that make the span "the
/// finding". For these, [`signature_for_issue`] hashes the function's
/// header (name + parameter list) instead of the full span text, so an
/// edit anywhere in the body that doesn't change the reported
/// classification (line count, cyclomatic complexity) doesn't drift the
/// baseline signature. Every other check keeps hashing the full span
/// text via [`signature_for_span`] — that's still correct there because
/// the span IS the offending content.
pub const FUNCTION_SCOPED_SIGNATURE_CHECK_IDS: &[&str] = &["Refactor.LongAndComplex"];

/// Compute the canonical signature for one issue, dispatching to the
/// function-header strategy for [`FUNCTION_SCOPED_SIGNATURE_CHECK_IDS`]
/// and the default whole-span strategy for every other check. This is
/// the entry point baseline writers/readers should call — prefer it over
/// calling [`signature_for_span`] directly so new call sites don't have
/// to know about the per-check override.
pub fn signature_for_issue(file_text: &str, issue: &Issue) -> String {
    if FUNCTION_SCOPED_SIGNATURE_CHECK_IDS.contains(&issue.check_id.as_str()) {
        symbol_signature_for_span(file_text, &issue.location)
    } else {
        signature_for_span(file_text, &issue.location)
    }
}

/// Function-header variant of [`signature_for_span`]. Hashes the text
/// from the span's start up to (but excluding) the function's
/// body-opening `{` — i.e. the name + parameter list — rather than the
/// whole span. Falls back to hashing the whole snippet (identical to
/// [`signature_for_span`]) when no such brace can be found at
/// paren-depth 0, e.g. an arrow function with an expression body
/// (`(a, b) => a + b`) — those are rare for function-scoped checks
/// (which fire on size/complexity) and anonymous besides, so the
/// fallback's coarser invalidation is an acceptable trade.
///
/// Deliberately does NOT try to survive a function rename: the header
/// text includes the name, so renaming changes the signature like any
/// other check. Baseline entries are not expected to survive renames.
pub fn symbol_signature_for_span(file_text: &str, location: &cofferdam_core::Location) -> String {
    let snippet = span_snippet(file_text, location);
    hash_collapsed(function_header(snippet).unwrap_or(snippet))
}

/// Slice `file_text` to the byte range `location` covers, or `""` when
/// the range is out of bounds (defensive — callers should never pass a
/// stale location, but a corrupt/mismatched file must not panic).
fn span_snippet<'t>(file_text: &'t str, location: &cofferdam_core::Location) -> &'t str {
    let start = location.start_byte() as usize;
    let end = location.end_byte() as usize;
    if start <= end && end <= file_text.len() {
        &file_text[start..end]
    } else {
        ""
    }
}

/// Byte range of `snippet` up to (excluding) the first `{` encountered
/// at paren-depth `<= 0`, tracking `(`/`)` so default-parameter object
/// destructuring (`function f({a, b} = {}) {`) inside the parameter
/// list isn't mistaken for the body's opening brace. Returns `None` when
/// no such brace exists (e.g. an expression-bodied arrow function).
fn function_header(snippet: &str) -> Option<&str> {
    let mut depth: i32 = 0;
    for (i, ch) in snippet.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            '{' if depth <= 0 => return Some(&snippet[..i]),
            _ => {}
        }
    }
    None
}

/// Collapse every whitespace run in `snippet` to a single space, trim
/// the ends, and return the hex-encoded SHA-256 of the result. Shared by
/// [`signature_for_span`] and [`symbol_signature_for_span`] so both
/// strategies are equally robust to reformatting.
fn hash_collapsed(snippet: &str) -> String {
    // Collapse every whitespace run to a single space, then trim. The
    // resulting normal form is the same regardless of indentation,
    // line-wrapping, CRLF/LF, or alignment-padding shifts.
    let mut canonical = String::with_capacity(snippet.len());
    let mut last_was_space = true; // suppresses leading whitespace
    for ch in snippet.chars() {
        if ch.is_ascii_whitespace() {
            if !last_was_space {
                canonical.push(' ');
                last_was_space = true;
            }
        } else {
            canonical.push(ch);
            last_was_space = false;
        }
    }
    if canonical.ends_with(' ') {
        canonical.pop();
    }

    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();

    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{:02x}", byte);
    }
    out
}

/// Build a baseline entry from an issue + its computed signature, with
/// file path normalized to forward-slash relative to `root` when possible.
pub fn entry_for(issue: &Issue, signature: String, root: Option<&Path>) -> BaselineEntry {
    BaselineEntry {
        file: normalize_path(&issue.file, root),
        check_id: issue.check_id.clone(),
        rule_signature: signature,
    }
}

/// Normalise a file path for baseline storage: relative to `root` when
/// possible, then forward-slash separated. Falls back to the absolute
/// forward-slash form when the path can't be made relative.
pub fn normalize_path(path: &Path, root: Option<&Path>) -> String {
    let rel = match root {
        Some(r) => path.strip_prefix(r).unwrap_or(path),
        None => path,
    };
    // Filter `.` components introduced by `Path::new(".").join(...)` and
    // similar paths so the stored form is the cleanest representation.
    let cleaned: PathBuf = rel
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    cleaned.to_string_lossy().replace('\\', "/")
}

/// Net change between two baselines.
///
/// "Same finding" is defined by `rule_signature` matching within the same
/// `(file, check_id)` pair. Because `rule_signature` is a SHA-256 of the
/// trimmed span text it is stable across line moves (reformats), so a
/// finding that shifted lines but wasn't changed in content contributes 0
/// to the delta.
#[derive(Debug, Default)]
pub struct Delta {
    /// Number of findings present in `current` but absent from `prior`.
    pub added: usize,
    /// Number of findings present in `prior` but absent from `current`.
    pub removed: usize,
    /// Net change per check id: positive = net added, negative = net removed.
    pub by_check: BTreeMap<String, i64>,
}

/// Compute the delta between `prior` and `current` baselines.
///
/// Comparison key: `rule_signature`. Two entries with the same signature
/// (but potentially different line numbers) are considered the same finding.
/// A finding with no matching signature in the other baseline counts as
/// added or removed.
pub fn compute_delta(prior: &Baseline, current: &Baseline) -> Delta {
    // Build multisets (signature → count) from each baseline.
    // We key by (check_id, rule_signature) so that identical spans in
    // different checks are treated as distinct findings.
    let mut prior_map: std::collections::HashMap<(&str, &str), usize> =
        std::collections::HashMap::new();
    for e in &prior.findings {
        *prior_map
            .entry((e.check_id.as_str(), e.rule_signature.as_str()))
            .or_default() += 1;
    }

    let mut current_map: std::collections::HashMap<(&str, &str), usize> =
        std::collections::HashMap::new();
    for e in &current.findings {
        *current_map
            .entry((e.check_id.as_str(), e.rule_signature.as_str()))
            .or_default() += 1;
    }

    let mut added: usize = 0;
    let mut removed: usize = 0;
    let mut by_check: BTreeMap<String, i64> = BTreeMap::new();

    // Findings in current not in prior → added.
    for (e, &cur_count) in &current_map {
        let (check_id, _sig) = e;
        let pri_count = prior_map.get(e).copied().unwrap_or(0);
        if cur_count > pri_count {
            let delta = (cur_count - pri_count) as i64;
            added += delta as usize;
            *by_check.entry(check_id.to_string()).or_default() += delta;
        }
    }

    // Findings in prior not in current → removed.
    for (e, &pri_count) in &prior_map {
        let (check_id, _sig) = e;
        let cur_count = current_map.get(e).copied().unwrap_or(0);
        if pri_count > cur_count {
            let delta = (pri_count - cur_count) as i64;
            removed += delta as usize;
            *by_check.entry(check_id.to_string()).or_default() -= delta;
        }
    }

    Delta {
        added,
        removed,
        by_check,
    }
}

fn sort_entries(entries: &mut [BaselineEntry]) {
    entries.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.check_id.cmp(&b.check_id))
            .then_with(|| a.rule_signature.cmp(&b.rule_signature))
    });
}

/// Read a baseline from disk. Returns Err on IO, parse, or version
/// mismatch. Caller decides what to do with `Err` (typically: print a
/// warning and proceed without baseline).
pub fn read(path: &Path) -> Result<Baseline, BaselineError> {
    let raw = std::fs::read_to_string(path).map_err(|source| BaselineError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let baseline: Baseline = serde_json::from_str(&raw).map_err(|source| BaselineError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    if baseline.version != VERSION {
        return Err(BaselineError::Version {
            path: path.to_path_buf(),
            found: baseline.version,
            supported: VERSION,
        });
    }
    Ok(baseline)
}

/// Write a baseline to disk, creating parent directories as needed.
/// Pretty-prints with stable key order.
pub fn write(path: &Path, baseline: &Baseline) -> Result<(), BaselineError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|source| BaselineError::Write {
                path: path.to_path_buf(),
                source,
            })?;
        }
    }
    // Sort defensively even if `Baseline::new` was used; callers may have
    // hand-built the struct.
    let mut sorted = baseline.clone();
    sort_entries(&mut sorted.findings);
    let json = serde_json::to_string_pretty(&sorted)
        .map_err(|source| BaselineError::Serialize { source })?;
    // Trailing newline for tidy diffs.
    let mut out = json;
    out.push('\n');
    std::fs::write(path, out).map_err(|source| BaselineError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn signature_is_stable_for_same_input() {
        let text = "if (x == y) {}";
        let loc = Location::from_span(
            std::path::Path::new(""),
            Span {
                start_byte: 4,
                end_byte: 10,
                line: 1,
                column: 5,
            },
        );
        let s1 = signature_for_span(text, &loc);
        let s2 = signature_for_span(text, &loc);
        assert_eq!(s1, s2);
        assert_eq!(s1.len(), 64);
    }

    #[test]
    fn signature_ignores_surrounding_whitespace() {
        let s_a = signature_for_span(
            "x === y",
            &Location::from_span(
                std::path::Path::new(""),
                Span {
                    start_byte: 0,
                    end_byte: 7,
                    line: 1,
                    column: 1,
                },
            ),
        );
        let s_b = signature_for_span(
            "  x === y  ",
            &Location::from_span(
                std::path::Path::new(""),
                Span {
                    start_byte: 0,
                    end_byte: 11,
                    line: 1,
                    column: 1,
                },
            ),
        );
        assert_eq!(s_a, s_b, "trim should make these equal");
    }

    /// cd-1h7: whitespace-only reformat (`prettier --write`) used to drift
    /// the signature. The post-fix algorithm collapses every internal
    /// whitespace run to a single space, so re-indentation, line wrapping,
    /// CRLF/LF, and alignment-padding all hash identically.
    #[test]
    fn signature_invariant_under_internal_whitespace_reformat() {
        // Pre-prettier: 4-space indent, body on its own line.
        let pre = "function foo(a,    b) {\n    return a   +   b;\n}";
        // Post-prettier: 2-space indent, single-space arg list, single-space body.
        let post = "function foo(a, b) {\n  return a + b;\n}";
        // CRLF variant of the post text — should also collapse to the same form.
        let post_crlf = "function foo(a, b) {\r\n  return a + b;\r\n}";

        let loc_pre = Location::from_span(
            std::path::Path::new(""),
            Span {
                start_byte: 0,
                end_byte: pre.len() as u32,
                line: 1,
                column: 1,
            },
        );
        let loc_post = Location::from_span(
            std::path::Path::new(""),
            Span {
                start_byte: 0,
                end_byte: post.len() as u32,
                line: 1,
                column: 1,
            },
        );
        let loc_crlf = Location::from_span(
            std::path::Path::new(""),
            Span {
                start_byte: 0,
                end_byte: post_crlf.len() as u32,
                line: 1,
                column: 1,
            },
        );

        let s_pre = signature_for_span(pre, &loc_pre);
        let s_post = signature_for_span(post, &loc_post);
        let s_crlf = signature_for_span(post_crlf, &loc_crlf);
        assert_eq!(
            s_pre, s_post,
            "indentation + alignment shifts must not change the signature"
        );
        assert_eq!(s_post, s_crlf, "CRLF vs LF must not change the signature");
    }

    /// Builds an `Issue` covering the whole of `text` (byte 0 to `text.len()`),
    /// for exercising [`signature_for_issue`] against different check IDs.
    fn whole_file_issue(check_id: &str, text: &str) -> Issue {
        Issue {
            check_id: check_id.to_string(),
            message: String::new(),
            file: PathBuf::from("test.ts"),
            location: Location::from_span(
                std::path::Path::new(""),
                Span {
                    start_byte: 0,
                    end_byte: text.len() as u32,
                    line: 1,
                    column: 1,
                },
            ),
            priority: Priority(0),
            severity: Severity::Medium,
            related: Vec::new(),
        }
    }

    /// cd-9 ("rulesig") regression: a function-scoped check's signature
    /// must survive an edit inside the function body that doesn't change
    /// the reported classification — only the header (name + params)
    /// should participate in the hash.
    #[test]
    fn function_scoped_signature_survives_unrelated_body_edit() {
        let before = "function foo(a, b) {\n  return a + b;\n}";
        let after =
            "function foo(a, b) {\n  // an unrelated comment added below\n  return a + b;\n}";

        for check_id in FUNCTION_SCOPED_SIGNATURE_CHECK_IDS {
            let sig_before = signature_for_issue(before, &whole_file_issue(check_id, before));
            let sig_after = signature_for_issue(after, &whole_file_issue(check_id, after));
            assert_eq!(
                sig_before, sig_after,
                "{check_id}: unrelated body edit must not drift the signature"
            );
        }
    }

    /// A function-scoped check's signature still changes when the header
    /// itself changes (rename, or added/removed parameter) — baseline
    /// entries are not expected to survive a rename.
    #[test]
    fn function_scoped_signature_changes_with_rename() {
        let original = "function foo(a, b) {\n  return a + b;\n}";
        let renamed = "function bar(a, b) {\n  return a + b;\n}";
        let sig_original = signature_for_issue(
            original,
            &whole_file_issue("Refactor.LongAndComplex", original),
        );
        let sig_renamed = signature_for_issue(
            renamed,
            &whole_file_issue("Refactor.LongAndComplex", renamed),
        );
        assert_ne!(
            sig_original, sig_renamed,
            "renaming the function is a header change and may re-baseline"
        );
    }

    /// Non-function-scoped checks (the vast majority) must keep the cd-1h7
    /// whole-span-text behavior — `signature_for_issue` should not
    /// accidentally widen the function-scoped override to every check.
    #[test]
    fn non_function_scoped_signature_still_invalidates_on_content_change() {
        let a = whole_file_issue("Warning.TripleEquals", "x === y");
        let b = whole_file_issue("Warning.TripleEquals", "a === b");
        assert_ne!(
            signature_for_issue("x === y", &a),
            signature_for_issue("a === b", &b),
            "TextHash checks must still invalidate on a real content edit"
        );
    }

    /// `symbol_signature_for_span` falls back to the whole-snippet hash
    /// (identical to `signature_for_span`) when no paren-depth-0 `{` is
    /// found — e.g. an arrow function with an expression body.
    #[test]
    fn symbol_signature_falls_back_to_text_hash_without_a_body_brace() {
        let snippet = "(a, b) => a + b";
        let loc = Location::from_span(
            std::path::Path::new(""),
            Span {
                start_byte: 0,
                end_byte: snippet.len() as u32,
                line: 1,
                column: 1,
            },
        );
        assert_eq!(
            symbol_signature_for_span(snippet, &loc),
            signature_for_span(snippet, &loc),
            "no body brace to key on → fall back to hashing the whole snippet"
        );
    }

    /// Token boundaries still matter — we only collapse whitespace, not
    /// glue tokens together. `a + b` and `a+b` should hash differently
    /// because they're textually distinct (and prettier chooses one form
    /// canonically anyway, so this never drifts in practice).
    #[test]
    fn signature_distinguishes_glued_tokens_from_spaced() {
        let glued = signature_for_span(
            "a + b",
            &Location::from_span(
                std::path::Path::new(""),
                Span {
                    start_byte: 0,
                    end_byte: 5,
                    line: 1,
                    column: 1,
                },
            ),
        );
        let spaced = signature_for_span(
            "a+b",
            &Location::from_span(
                std::path::Path::new(""),
                Span {
                    start_byte: 0,
                    end_byte: 3,
                    line: 1,
                    column: 1,
                },
            ),
        );
        assert_ne!(glued, spaced);
    }

    #[test]
    fn signature_changes_with_content() {
        let loc = Location::from_span(
            std::path::Path::new(""),
            Span {
                start_byte: 0,
                end_byte: 7,
                line: 1,
                column: 1,
            },
        );
        let a = signature_for_span("x === y", &loc);
        let b = signature_for_span("a === b", &loc);
        assert_ne!(a, b);
    }

    #[test]
    fn signature_handles_out_of_bounds_span() {
        let loc = Location::from_span(
            std::path::Path::new(""),
            Span {
                start_byte: 100,
                end_byte: 200,
                line: 1,
                column: 1,
            },
        );
        // Should not panic; produces the SHA of an empty string.
        let s = signature_for_span("short", &loc);
        assert_eq!(s.len(), 64);
    }

    #[test]
    fn normalize_path_makes_relative_and_forward_slash() {
        let root = PathBuf::from("/repo");
        let p = PathBuf::from("/repo/src/foo.ts");
        assert_eq!(normalize_path(&p, Some(&root)), "src/foo.ts");
    }

    #[test]
    fn normalize_path_strips_leading_dot() {
        let p = PathBuf::from("./src/foo.ts");
        assert_eq!(normalize_path(&p, None), "src/foo.ts");
    }

    #[test]
    fn write_then_read_roundtrips() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join(".cofferdam/baseline.json");
        let baseline = Baseline::new(vec![
            BaselineEntry {
                file: "src/b.ts".into(),
                check_id: "Warning.TripleEquals".into(),
                rule_signature: "abc123".into(),
            },
            BaselineEntry {
                file: "src/a.ts".into(),
                check_id: "Warning.TripleEquals".into(),
                rule_signature: "deadbeef".into(),
            },
        ]);
        write(&path, &baseline).expect("write");
        let read_back = read(&path).expect("read");
        assert_eq!(read_back.version, VERSION);
        assert_eq!(read_back.findings.len(), 2);
        // Sorted: a.ts before b.ts.
        assert_eq!(read_back.findings[0].file, "src/a.ts");
        assert_eq!(read_back.findings[1].file, "src/b.ts");
    }

    #[test]
    fn read_rejects_unknown_version() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("baseline.json");
        std::fs::write(&path, r#"{"version":99,"findings":[]}"#).unwrap();
        let err = read(&path).expect_err("should reject");
        assert!(matches!(err, BaselineError::Version { found: 99, .. }));
    }

    #[test]
    fn lookup_set_membership() {
        let entry = BaselineEntry {
            file: "src/a.ts".into(),
            check_id: "Warning.TripleEquals".into(),
            rule_signature: "abc".into(),
        };
        let baseline = Baseline::new(vec![entry.clone()]);
        let set = baseline.lookup_set();
        assert!(set.contains(&entry));

        let other = BaselineEntry {
            file: "src/a.ts".into(),
            check_id: "Warning.TripleEquals".into(),
            rule_signature: "different".into(),
        };
        assert!(!set.contains(&other));
    }

    // ── compute_delta tests ───────────────────────────────────────────────────

    fn make_entry(check_id: &str, sig: &str) -> BaselineEntry {
        BaselineEntry {
            file: "src/a.ts".into(),
            check_id: check_id.into(),
            rule_signature: sig.into(),
        }
    }

    #[test]
    fn delta_identical_baselines_is_zero() {
        let entry = make_entry("Warning.TripleEquals", "abc123");
        let b1 = Baseline::new(vec![entry.clone()]);
        let b2 = Baseline::new(vec![entry]);
        let d = compute_delta(&b1, &b2);
        assert_eq!(d.added, 0);
        assert_eq!(d.removed, 0);
        assert!(d.by_check.is_empty());
    }

    #[test]
    fn delta_one_finding_added() {
        let prior = Baseline::new(vec![]);
        let current = Baseline::new(vec![make_entry("Warning.TripleEquals", "abc123")]);
        let d = compute_delta(&prior, &current);
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 0);
        assert_eq!(d.by_check.get("Warning.TripleEquals").copied(), Some(1));
    }

    #[test]
    fn delta_one_finding_removed() {
        let prior = Baseline::new(vec![make_entry("Warning.TripleEquals", "abc123")]);
        let current = Baseline::new(vec![]);
        let d = compute_delta(&prior, &current);
        assert_eq!(d.added, 0);
        assert_eq!(d.removed, 1);
        assert_eq!(d.by_check.get("Warning.TripleEquals").copied(), Some(-1));
    }

    #[test]
    fn delta_mixed_three_added_five_removed() {
        let prior = Baseline::new(vec![
            make_entry("Warning.TripleEquals", "sig1"),
            make_entry("Warning.TripleEquals", "sig2"),
            make_entry("Refactor.CyclomaticComplexity", "sig3"),
            make_entry("Refactor.CyclomaticComplexity", "sig4"),
            make_entry("Refactor.CyclomaticComplexity", "sig5"),
        ]);
        let current = Baseline::new(vec![
            make_entry("Warning.TripleEquals", "sig1"),
            make_entry("Warning.TripleEquals", "sig6"), // new
            make_entry("Warning.TripleEquals", "sig7"), // new
            make_entry("Warning.TripleEquals", "sig8"), // new
        ]);
        let d = compute_delta(&prior, &current);
        assert_eq!(d.added, 3, "added");
        assert_eq!(d.removed, 4, "removed"); // sig2 + sig3,sig4,sig5
                                             // by_check: TripleEquals: net +2, Refactor: net -3
        assert_eq!(
            d.by_check.get("Warning.TripleEquals").copied(),
            Some(2),
            "TripleEquals net"
        );
        assert_eq!(
            d.by_check.get("Refactor.CyclomaticComplexity").copied(),
            Some(-3),
            "Cyclomatic net"
        );
    }

    #[test]
    fn delta_renamed_file_stable_signature_zero_net() {
        // Signature is the same even though the file name changed.
        // compute_delta keys by (check_id, signature) only, not file name.
        let prior = Baseline::new(vec![BaselineEntry {
            file: "src/old.ts".into(),
            check_id: "Warning.TripleEquals".into(),
            rule_signature: "stablehash".into(),
        }]);
        let current = Baseline::new(vec![BaselineEntry {
            file: "src/new.ts".into(),
            check_id: "Warning.TripleEquals".into(),
            rule_signature: "stablehash".into(),
        }]);
        let d = compute_delta(&prior, &current);
        assert_eq!(d.added, 0, "signature stable across rename → zero added");
        assert_eq!(
            d.removed, 0,
            "signature stable across rename → zero removed"
        );
        assert!(d.by_check.is_empty());
    }
}
