//! Per-file findings cache (cd-9hp.4 cp2).
//!
//! Layered on top of the cp1 [`crate::cache::ParseCache`]. Where the
//! parse cache shaves the oxc-parse cost on no-op re-runs, the
//! findings cache shaves the per-file check work — for every check
//! that opts into [`cofferdam_core::CheckMeta::pure_run`], the
//! engine memoises `Vec<Issue>` under a
//! `(content_hash, config_hash, engine_version, check_id)` key and
//! replays it on hit instead of calling `Check::run()`.
//!
//! ## Why a triple key
//!
//! Three things change a check's output for a fixed AST:
//!
//! 1. **The file's text.** Captured by the content hash already
//!    computed by [`crate::cache::hash_text`].
//! 2. **Configuration.** Per-check options, severity overrides,
//!    layer assignments, public-API allowlists — all of these can
//!    shift a check's findings or change how the engine stamps
//!    them. The architecture sign-off pinned
//!    `(content, config, engine)` over content-only specifically to
//!    catch the "user edits cofferdam.toml; cached findings are
//!    silently re-used" failure mode.
//! 3. **The engine itself.** A check upgrade can emit different
//!    findings on the same input. We pin the `CARGO_PKG_VERSION` of
//!    `cofferdam-engine` to invalidate caches across version bumps.
//!
//! cp2 ships the full triple. cp3's disk-backed cache will use the
//! same key shape so an upgrade nukes prior content rather than
//! silently re-interpreting it.
//!
//! ## Purity contract
//!
//! Caching is opt-in via `CheckMeta::pure_run`. Pure checks
//! guarantee their `run()` is a deterministic function of
//! `(file content, resolved options)` — no corpus reads, no corpus
//! writes, no global state. Marking a non-pure check pure is a
//! correctness bug: the engine would replay findings without the
//! corresponding corpus contributions, silently dropping cross-file
//! evidence.
//!
//! ## Threading
//!
//! Single-threaded today (matches the parse cache). Per-file
//! parallelism (cd-6ad) is a separate piece of work; both caches
//! lift in tandem.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;

use cofferdam_core::{Issue, Uri};

use crate::cache::ContentHash;

/// SHA-256 of the engine's resolved configuration. The engine
/// computes this at construction time and threads it through every
/// cache lookup so a `cofferdam.toml` edit invalidates the right
/// entries.
pub type ConfigHash = [u8; 32];

/// Compile-time-baked version string of this `cofferdam-engine`
/// build. Different builds get different cache namespaces.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Identity of one cached findings entry. Keyed by (content,
/// config, engine_version, check_id) so any of the four axes can
/// invalidate independently.
///
/// The engine_version is folded in via the constant
/// [`ENGINE_VERSION`] — the runtime cache only sees a single
/// (content, config, check_id) triple per process, which is what
/// the `HashMap` lookup keys on. On a cofferdam upgrade the
/// in-memory cache is discarded with the process; cp3's disk-backed
/// flavour will scope the directory by engine version to make the
/// invalidation persistent.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct FindingsKey {
    pub content_hash: ContentHash,
    pub config_hash: ConfigHash,
    pub check_id: &'static str,
}

/// In-memory `(content, config, check_id) → Vec<Issue>` map. The
/// engine consults it before invoking a pure check; cache misses
/// flow through `Check::run` as normal and the result is inserted
/// before the engine appends to its issue buffer.
#[derive(Default)]
pub struct FindingsCache {
    entries: RefCell<HashMap<FindingsKey, Vec<Issue>>>,
    hits: Cell<u64>,
    misses: Cell<u64>,
}

impl FindingsCache {
    /// Empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up cached findings for `(content, config, check_id)`.
    /// Increments hit / miss counters on every call.
    pub fn get(&self, key: &FindingsKey) -> Option<Vec<Issue>> {
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

    /// Insert the freshly-computed findings for a `(content, config,
    /// check_id)` triple. Overwrites any previous entry — the engine
    /// only inserts after `Check::run` has produced a result, so an
    /// overwrite means a corpus / engine change invalidated the
    /// prior entry's premise.
    pub fn insert(&self, key: FindingsKey, issues: Vec<Issue>) {
        self.entries.borrow_mut().insert(key, issues);
    }

    /// Drop every cached entry. Useful when the caller knows the
    /// config or engine has changed in a way the key doesn't (yet)
    /// capture — the watch loop calls this on config reload.
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

    /// Look up cached findings and re-stamp them onto `path` before
    /// returning (cd-mwr6). This is the only lookup the per-file engine
    /// loop should use: the key is `(content, config, check)` with **no
    /// path**, so two byte-identical files share one entry, and the
    /// stored `Issue`s carry the path of whichever file populated it.
    /// Without the re-stamp a cache hit for a different file would report
    /// the wrong file path — making warm-cache output disagree with
    /// `--no-cache`.
    pub fn get_for_path(&self, key: &FindingsKey, path: &Path) -> Option<Vec<Issue>> {
        self.get(key).map(|mut issues| {
            restamp_path(&mut issues, path);
            issues
        })
    }

    /// Clone every `(key, issues)` pair out for serialisation. Used
    /// by `crate::disk_cache::save_findings` to persist the cache
    /// across runs. The map is held under a short-lived borrow so
    /// the snapshot's allocation cost is paid once, off the hot
    /// path.
    pub fn snapshot(&self) -> Vec<(FindingsKey, Vec<Issue>)> {
        self.entries
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Rewrite every file path on `issues` to `path` (cd-mwr6).
///
/// A `pure_run` check has no corpus access, so every path it emits —
/// `Issue.file`, `Issue.location.uri`, and any `related` spans — refers to
/// the single file it was handed. When those issues are replayed from the
/// content-keyed cache against a *different* file with identical content,
/// overwriting all of them onto the consumer's `path` is both correct and
/// sufficient. The byte offsets / line / column in each `Location.range`
/// are unchanged: identical content yields identical ranges.
pub fn restamp_path(issues: &mut [Issue], path: &Path) {
    let uri = Uri::from_path(path);
    for issue in issues.iter_mut() {
        issue.file = path.to_path_buf();
        issue.location.uri = uri.clone();
        for related in issue.related.iter_mut() {
            related.file = path.to_path_buf();
            related.location.uri = uri.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{Location, Priority, RelatedSpan, Severity, Span};
    use std::path::PathBuf;

    fn key(content: u8, config: u8, check_id: &'static str) -> FindingsKey {
        FindingsKey {
            content_hash: [content; 32],
            config_hash: [config; 32],
            check_id,
        }
    }

    fn issue(msg: &str) -> Issue {
        let file = PathBuf::from("a.ts");
        Issue {
            check_id: "Test.Check".to_string(),
            message: msg.to_string(),
            location: Location::from_span(
                &file,
                Span {
                    start_byte: 0,
                    end_byte: 0,
                    line: 1,
                    column: 1,
                },
            ),
            file,
            priority: Priority(0),
            severity: Severity::Low,
            related: Vec::new(),
        }
    }

    #[test]
    fn empty_cache_is_a_miss() {
        let c = FindingsCache::new();
        assert!(c.get(&key(0, 0, "X")).is_none());
        assert_eq!(c.misses(), 1);
        assert_eq!(c.hits(), 0);
    }

    #[test]
    fn insert_then_get_is_a_hit() {
        let c = FindingsCache::new();
        let k = key(0, 0, "X");
        c.insert(k.clone(), vec![issue("first")]);
        let got = c.get(&k).expect("hit");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "first");
        assert_eq!(c.hits(), 1);
    }

    #[test]
    fn different_content_hashes_are_distinct_entries() {
        let c = FindingsCache::new();
        c.insert(key(0, 0, "X"), vec![issue("a")]);
        c.insert(key(1, 0, "X"), vec![issue("b")]);
        assert_eq!(c.get(&key(0, 0, "X")).unwrap()[0].message, "a");
        assert_eq!(c.get(&key(1, 0, "X")).unwrap()[0].message, "b");
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn different_config_hashes_are_distinct_entries() {
        // `(content, check) but different config` was the
        // architectural call to capture — this test pins that the
        // key fan-out works on the config axis even when content
        // matches.
        let c = FindingsCache::new();
        c.insert(key(0, 0, "X"), vec![issue("default options")]);
        c.insert(key(0, 1, "X"), vec![issue("custom options")]);
        assert_eq!(
            c.get(&key(0, 0, "X")).unwrap()[0].message,
            "default options"
        );
        assert_eq!(c.get(&key(0, 1, "X")).unwrap()[0].message, "custom options");
    }

    #[test]
    fn different_checks_get_distinct_entries() {
        let c = FindingsCache::new();
        c.insert(key(0, 0, "X.A"), vec![issue("a")]);
        c.insert(key(0, 0, "X.B"), vec![issue("b")]);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn insert_overwrites_existing_entry() {
        let c = FindingsCache::new();
        let k = key(0, 0, "X");
        c.insert(k.clone(), vec![issue("v1")]);
        c.insert(k.clone(), vec![issue("v2"), issue("v3")]);
        let got = c.get(&k).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].message, "v2");
        assert_eq!(got[1].message, "v3");
    }

    #[test]
    fn clear_drops_entries_and_counters() {
        let c = FindingsCache::new();
        c.insert(key(0, 0, "X"), vec![issue("a")]);
        let _ = c.get(&key(0, 0, "X"));
        assert_eq!(c.hits(), 1);
        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.hits(), 0);
        assert_eq!(c.misses(), 0);
        let _ = c.get(&key(0, 0, "X"));
        assert_eq!(c.misses(), 1);
    }

    #[test]
    fn engine_version_is_a_non_empty_string() {
        // Belt-and-braces — the constant should at minimum be
        // non-empty so the disk-backed cache directory in cp3 has a
        // unique prefix per cofferdam build.
        assert!(!ENGINE_VERSION.is_empty());
    }

    // cd-mwr6: a finding built on `a.ts` must report `b.ts` after a
    // re-stamp, with its byte range left intact.
    #[test]
    fn restamp_rewrites_file_and_uri_preserving_range() {
        let mut issues = vec![issue("unused")];
        let range_before = issues[0].location.range.clone();
        restamp_path(&mut issues, &PathBuf::from("b.ts"));
        assert_eq!(issues[0].file, PathBuf::from("b.ts"));
        assert_eq!(issues[0].location.uri.as_str(), "file://b.ts");
        assert_eq!(
            issues[0].location.range, range_before,
            "byte range must survive the re-stamp unchanged"
        );
    }

    // Pure checks only ever emit same-file related spans, so those must
    // follow the primary path on a re-stamp too.
    #[test]
    fn restamp_rewrites_related_spans() {
        let a = PathBuf::from("a.ts");
        let related_loc = Location::from_span(
            &a,
            Span {
                start_byte: 10,
                end_byte: 20,
                line: 2,
                column: 1,
            },
        );
        let mut issues = vec![Issue {
            related: vec![RelatedSpan {
                file: a.clone(),
                location: related_loc,
            }],
            ..issue("dup")
        }];
        restamp_path(&mut issues, &PathBuf::from("b.ts"));
        assert_eq!(issues[0].related[0].file, PathBuf::from("b.ts"));
        assert_eq!(issues[0].related[0].location.uri.as_str(), "file://b.ts");
    }

    // The cache entry itself is path-free: one stored entry serves many
    // identical-content files, each getting its own path on the way out.
    #[test]
    fn get_for_path_restamps_without_mutating_the_entry() {
        let c = FindingsCache::new();
        let k = key(0, 0, "X");
        c.insert(k.clone(), vec![issue("shared")]); // built on a.ts

        let from_b = c.get_for_path(&k, &PathBuf::from("b.ts")).unwrap();
        assert_eq!(from_b[0].file, PathBuf::from("b.ts"));

        // A second consumer of the same entry gets ITS path, proving the
        // stored copy wasn't mutated to b.ts by the first lookup.
        let from_c = c.get_for_path(&k, &PathBuf::from("c.ts")).unwrap();
        assert_eq!(from_c[0].file, PathBuf::from("c.ts"));
        assert_eq!(from_c[0].location.uri.as_str(), "file://c.ts");
    }
}
