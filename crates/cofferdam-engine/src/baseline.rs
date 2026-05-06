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
//! - **Default = SHA-256 of the trimmed offending span text.** Robust to
//!   line moves and surrounding edits; only changes when the offending
//!   code itself changes. Aggressive trim (whitespace) absorbs reformats
//!   that touch indentation.
//!
//! Per-check overrides for `rule_signature` will land later (e.g.
//! `Refactor.CognitiveComplexity` may want to hash the AST shape rather
//! than text), but the default covers every check shipped today.
//!
//! ## File format
//!
//! Versioned JSON, pretty-printed with 2-space indent and entries sorted
//! by `(file, check_id, rule_signature)` so diffs are reviewable.
//!
//! ```json
//! {
//!   "version": 1,
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

use cofferdam_core::{Issue, Span};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Current baseline schema version. Bump on incompatible changes only.
pub const VERSION: u32 = 1;

/// Default baseline file path, relative to the project root. Kept here
/// so the CLI and any other consumer agree on the convention.
pub const DEFAULT_PATH: &str = ".cofferdam/baseline.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct BaselineEntry {
    /// Forward-slash relative path to the file containing the finding.
    /// Stored relative to the baseline file's parent directory's parent
    /// (typically the repo root) so baselines remain stable across
    /// different developers' workspace prefixes.
    pub file: String,
    pub check_id: String,
    /// Hex-encoded SHA-256 of the trimmed offending span text. See
    /// module docs for why this is text-based, not line-based.
    pub rule_signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub version: u32,
    pub findings: Vec<BaselineEntry>,
}

impl Baseline {
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
        "baseline {path} declares unsupported version {found}; this binary supports {supported}"
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

/// Compute the canonical signature for a span's text. Trims aggressively
/// so reformats that only touch surrounding whitespace don't invalidate
/// the baseline.
///
/// Returns hex-encoded SHA-256 (64 chars). We keep the full digest rather
/// than truncating: collisions on truncated hashes inside a single repo
/// are unlikely but the cost of avoiding them is one constant.
pub fn signature_for_span(file_text: &str, span: &Span) -> String {
    let start = span.start_byte as usize;
    let end = span.end_byte as usize;
    let snippet = if start <= end && end <= file_text.len() {
        &file_text[start..end]
    } else {
        ""
    };
    let trimmed = snippet.trim();

    let mut hasher = Sha256::new();
    hasher.update(trimmed.as_bytes());
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
        let span = Span {
            start_byte: 4,
            end_byte: 10,
            line: 1,
            column: 5,
        };
        let s1 = signature_for_span(text, &span);
        let s2 = signature_for_span(text, &span);
        assert_eq!(s1, s2);
        assert_eq!(s1.len(), 64);
    }

    #[test]
    fn signature_ignores_surrounding_whitespace() {
        let s_a = signature_for_span(
            "x === y",
            &Span {
                start_byte: 0,
                end_byte: 7,
                line: 1,
                column: 1,
            },
        );
        let s_b = signature_for_span(
            "  x === y  ",
            &Span {
                start_byte: 0,
                end_byte: 11,
                line: 1,
                column: 1,
            },
        );
        assert_eq!(s_a, s_b, "trim should make these equal");
    }

    #[test]
    fn signature_changes_with_content() {
        let span = Span {
            start_byte: 0,
            end_byte: 7,
            line: 1,
            column: 1,
        };
        let a = signature_for_span("x === y", &span);
        let b = signature_for_span("a === b", &span);
        assert_ne!(a, b);
    }

    #[test]
    fn signature_handles_out_of_bounds_span() {
        let span = Span {
            start_byte: 100,
            end_byte: 200,
            line: 1,
            column: 1,
        };
        // Should not panic; produces the SHA of an empty string.
        let s = signature_for_span("short", &span);
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
