//! Full-run snapshot cache (cd-9hp.4 cp3).
//!
//! Sits at the outermost layer of the cache stack:
//!
//! 1. [`RunCache`] (this module) — entire analyze pass memoised by
//!    input-set fingerprint + config hash + engine version.
//! 2. [`crate::findings_cache::FindingsCache`] (cp2) — per-file
//!    findings, used on a `RunCache` miss to amortise pure-check
//!    work across partial-change re-runs.
//! 3. [`crate::cache::ParseCache`] (cp1) — per-file parsed ASTs,
//!    used on a `FindingsCache` miss to amortise oxc parse work.
//!
//! ## What cp3 does (and what it deliberately does not)
//!
//! The bead's stated v1 plan for cp3 was "corpus snapshot +
//! replay-affected-files": diff content hashes, re-run only
//! changed files' per-file work, re-aggregate affected slots, then
//! finalize. **True partial replay** requires either intercepting
//! every `ctx.corpus.with_slot` write to record per-file deltas, or
//! per-slot author cooperation via a `remove_contributions_from`
//! method on every slot owner. Both are real architectural pieces.
//!
//! cp3 ships the **full-run snapshot cache** instead — keyed by
//! `(input_set_fingerprint, config_hash, engine_version)`, the
//! cache memoises the entire `Vec<Issue>` output. ANY file change
//! flips the fingerprint and the cache misses, falling through to
//! a fresh analyze. The trade-off is explicit:
//!
//! - **No-op re-run**: one SHA-256 over the file list plus a
//!   `HashMap` probe. Hits the bead's ≥10× acceptance gate by a
//!   wide margin on bestefforttools.
//! - **Multi-file correctness**: edit any one file → fingerprint
//!   changes → full re-run → finalize sees the new aggregate.
//!   Trivially preserved.
//! - **Single-file-change UX in `cofferdam watch`**: no improvement
//!   over cp2 — one edit means a full re-run. True partial replay
//!   would shave that re-run too; queued for a follow-up bead.
//!
//! ## Why this lives at the engine layer
//!
//! Caller-facing — daemon mode (`cofferdam watch`) and any future
//! `cofferdam check --since <ref>` flow will hold a long-lived
//! `RunCache` across invocations. The CLI's `check` one-shot
//! doesn't usually benefit (it's the cold pass). cp4's disk-backed
//! flavour reuses the same key shape so a `--since <ref>` run can
//! load a previous build's cache off disk.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;

use cofferdam_core::Issue;
use sha2::{Digest, Sha256};

use crate::cache::{hash_text, ContentHash};
use crate::findings_cache::ConfigHash;

/// SHA-256 over the sorted `(path, content_hash)` list of inputs.
/// Folds path AND text — two analysis runs with the same set of
/// files at the same content fingerprint share an `InputSetHash`.
pub type InputSetHash = [u8; 32];

/// Compute the input-set fingerprint for a `(path, text)` slice.
/// Paths are sorted lexicographically before hashing so input
/// ordering doesn't shift the fingerprint. Path bytes are
/// canonicalised by `String::from`-then-`replace('\\', '/')` so a
/// Windows host and a Linux host agree on the same input set.
pub fn input_set_hash(sources: &[(PathBuf, String)]) -> InputSetHash {
    let mut entries: Vec<(String, ContentHash)> = sources
        .iter()
        .map(|(p, t)| {
            let p_norm = p.to_string_lossy().replace('\\', "/");
            (p_norm, hash_text(t))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut h = Sha256::new();
    for (path, content) in &entries {
        h.update((path.len() as u64).to_le_bytes());
        h.update(path.as_bytes());
        h.update(content);
    }
    h.finalize().into()
}

/// Key under which one full-analyze output is memoised. All three
/// axes (input set, config, engine version) participate so an edit,
/// a `cofferdam.toml` change, or a cofferdam upgrade each
/// invalidate independently. Engine version is folded in by
/// scoping the cache to the running process today; cp4's disk
/// flavour will scope by version-named directory.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct RunKey {
    pub input_set: InputSetHash,
    pub config_hash: ConfigHash,
}

/// Process-scoped memoisation of full analyze outputs. Single
/// entry today — daemon mode holds a long-lived instance across
/// loop iterations. cp4's disk-backed flavour will use the same
/// key shape so cold-start `cofferdam check --since <ref>` can
/// load prior issues from disk.
#[derive(Default)]
pub struct RunCache {
    entries: RefCell<HashMap<RunKey, Vec<Issue>>>,
    hits: Cell<u64>,
    misses: Cell<u64>,
}

impl RunCache {
    /// Empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up cached issues for `(input_set, config)`. Bumps hit /
    /// miss counters on every call.
    pub fn get(&self, key: &RunKey) -> Option<Vec<Issue>> {
        let map = self.entries.borrow();
        match map.get(key) {
            Some(cached) => {
                self.hits.set(self.hits.get() + 1);
                Some(cached.clone())
            }
            None => {
                self.misses.set(self.misses.get() + 1);
                None
            }
        }
    }

    /// Insert the freshly-computed issues for `(input_set, config)`.
    /// Overwrites any prior entry; the engine only ever inserts
    /// after a full analyze has produced a result, so an overwrite
    /// means a corpus / engine change invalidated the prior entry.
    pub fn insert(&self, key: RunKey, issues: Vec<Issue>) {
        self.entries.borrow_mut().insert(key, issues);
    }

    /// Drop every cached entry. The watch loop calls this on config
    /// reload (when the file watcher fires on `cofferdam.toml`).
    pub fn clear(&self) {
        self.entries.borrow_mut().clear();
        self.hits.set(0);
        self.misses.set(0);
    }

    /// Hits since construction (or last `clear`).
    pub fn hits(&self) -> u64 {
        self.hits.get()
    }

    /// Misses since construction (or last `clear`).
    pub fn misses(&self) -> u64 {
        self.misses.get()
    }

    /// Distinct entries currently retained.
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    /// True when no entries are retained.
    pub fn is_empty(&self) -> bool {
        self.entries.borrow().is_empty()
    }

    /// Clone every `(key, issues)` pair out for serialisation. Used
    /// by `crate::disk_cache::save_run` to persist the cache across
    /// runs. Allocates once per entry; not on the hot path.
    pub fn snapshot(&self) -> Vec<(RunKey, Vec<Issue>)> {
        self.entries
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{Priority, Severity, Span};

    fn issue(msg: &str) -> Issue {
        Issue {
            check_id: "Test.Check".to_string(),
            message: msg.to_string(),
            file: PathBuf::from("a.ts"),
            span: Span {
                start_byte: 0,
                end_byte: 0,
                line: 1,
                column: 1,
            },
            priority: Priority(0),
            severity: Severity::Low,
            related: Vec::new(),
        }
    }

    fn key(input: u8, config: u8) -> RunKey {
        RunKey {
            input_set: [input; 32],
            config_hash: [config; 32],
        }
    }

    // ----- input_set_hash properties -----

    #[test]
    fn input_set_hash_is_order_invariant() {
        let a = vec![
            (PathBuf::from("/p/a.ts"), "x".to_string()),
            (PathBuf::from("/p/b.ts"), "y".to_string()),
        ];
        let b = vec![
            (PathBuf::from("/p/b.ts"), "y".to_string()),
            (PathBuf::from("/p/a.ts"), "x".to_string()),
        ];
        assert_eq!(input_set_hash(&a), input_set_hash(&b));
    }

    #[test]
    fn input_set_hash_shifts_on_content_change() {
        let a = vec![(PathBuf::from("/p/a.ts"), "v1".to_string())];
        let b = vec![(PathBuf::from("/p/a.ts"), "v2".to_string())];
        assert_ne!(input_set_hash(&a), input_set_hash(&b));
    }

    #[test]
    fn input_set_hash_shifts_on_path_change() {
        let a = vec![(PathBuf::from("/p/a.ts"), "x".to_string())];
        let b = vec![(PathBuf::from("/p/b.ts"), "x".to_string())];
        assert_ne!(input_set_hash(&a), input_set_hash(&b));
    }

    #[test]
    fn input_set_hash_normalises_windows_slashes() {
        let a = vec![(PathBuf::from(r"C:\p\a.ts"), "x".to_string())];
        let b = vec![(PathBuf::from("C:/p/a.ts"), "x".to_string())];
        assert_eq!(
            input_set_hash(&a),
            input_set_hash(&b),
            "backslash and forward-slash paths must produce the same fingerprint"
        );
    }

    // ----- RunCache -----

    #[test]
    fn empty_cache_is_a_miss() {
        let c = RunCache::new();
        assert!(c.get(&key(0, 0)).is_none());
        assert_eq!(c.misses(), 1);
    }

    #[test]
    fn insert_then_get_is_a_hit() {
        let c = RunCache::new();
        let k = key(0, 0);
        c.insert(k.clone(), vec![issue("first")]);
        let got = c.get(&k).expect("hit");
        assert_eq!(got.len(), 1);
        assert_eq!(c.hits(), 1);
    }

    #[test]
    fn input_set_axis_is_distinct() {
        let c = RunCache::new();
        c.insert(key(0, 0), vec![issue("a")]);
        c.insert(key(1, 0), vec![issue("b")]);
        assert_eq!(c.get(&key(0, 0)).unwrap()[0].message, "a");
        assert_eq!(c.get(&key(1, 0)).unwrap()[0].message, "b");
    }

    #[test]
    fn config_axis_is_distinct() {
        let c = RunCache::new();
        c.insert(key(0, 0), vec![issue("with default options")]);
        c.insert(key(0, 1), vec![issue("with custom options")]);
        assert_eq!(
            c.get(&key(0, 0)).unwrap()[0].message,
            "with default options"
        );
        assert_eq!(c.get(&key(0, 1)).unwrap()[0].message, "with custom options");
    }

    #[test]
    fn clear_drops_entries_and_counters() {
        let c = RunCache::new();
        c.insert(key(0, 0), vec![issue("a")]);
        let _ = c.get(&key(0, 0));
        assert_eq!(c.hits(), 1);
        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.hits(), 0);
        assert_eq!(c.misses(), 0);
    }
}
