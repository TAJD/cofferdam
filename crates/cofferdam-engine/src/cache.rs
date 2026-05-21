//! In-memory parse cache (cd-9hp.4 cp1).
//!
//! Every `cofferdam check` run reparses every file from scratch. On
//! a 300-file TypeScript repo, oxc parse is roughly half the per-file
//! cost — the rest is per-check AST walks plus graph extraction. A
//! daemon-mode (`--watch`) or repeated-invocation workflow that
//! re-parses identical content on every cycle leaves that work on the
//! table.
//!
//! [`ParseCache`] is a content-hash-keyed store of parsed ASTs. On
//! cache hit the engine skips the `parse_into` call entirely; the
//! cached `Allocator` + `ParserReturn` are handed back through a
//! callback so the borrow can't escape the cell. The architecture
//! sign-off for cd-9hp.4 settled on:
//!
//! - **Hybrid cache storage** — in-memory daemon today, disk-backed
//!   tier in a later checkpoint.
//! - **`(content, config, engine)` hash key** — correctness over a
//!   saved nanosecond. cp1 ships content-hash-only; cp2 tightens to
//!   the full triple.
//! - **Replay-affected-files for the corpus snapshot** — cp1 still
//!   rebuilds the corpus from scratch every cycle. The cache shaves
//!   parse cost only.
//!
//! ## Threading
//!
//! Today's engine is single-threaded; the cache uses [`std::cell`]
//! interior mutability rather than a [`std::sync::Mutex`] so cache
//! lookups stay branch-prediction-friendly on the hot path. When
//! per-file parallelism lands (cd-6ad's territory), the cache will
//! need a re-entrant lock — pinned to a follow-up bead under cd-9hp.4
//! so it can be designed alongside the corpus per-slot locking work.
//!
//! ## Lifetimes
//!
//! Each cache entry owns its `Allocator` + `SourceFile` (the parser
//! needs both to live as long as the returned AST). [`self_cell`]
//! wraps the pair with the borrowed `ParserReturn` so callers see a
//! single safe [`ParseCache::with_parsed`] callback.

use std::cell::{Cell, RefCell};
use std::collections::hash_map::Entry;
use std::collections::HashMap;

use cofferdam_core::parser::parse_into;
use cofferdam_core::{Allocator, SourceFile};
use oxc_parser::ParserReturn;
use sha2::{Digest, Sha256};

/// SHA-256 of a file's text. Cofferdam already depends on `sha2`
/// for canonical-graph node identity; reusing it here keeps the
/// crypto-dep footprint flat.
pub type ContentHash = [u8; 32];

/// Compute the content-hash for a UTF-8 source text.
pub fn hash_text(text: &str) -> ContentHash {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.finalize().into()
}

/// Owner side of a cached parse: the bumpalo arena plus the file
/// whose text the parser borrowed. Both must outlive the
/// [`ParserReturn`] kept alongside.
struct CachedOwner {
    allocator: Allocator,
    file: SourceFile,
}

self_cell::self_cell! {
    /// Bundles a `CachedOwner` with the `ParserReturn` borrowed from
    /// it. The `not_covariant` marker is conservative — oxc's
    /// `Program<'a>` uses invariant bumpalo collections internally —
    /// and is safe because the only access pattern is the
    /// `with_dependent` callback used by [`ParseCache::with_parsed`].
    struct CachedParse {
        owner: CachedOwner,

        #[not_covariant]
        dependent: ParserReturn,
    }
}

/// In-memory parse cache. Single-threaded today; hit/miss counters
/// surface through [`ParseCache::hits`] / [`ParseCache::misses`] so
/// tests and benches can confirm the cache is doing its job.
#[derive(Default)]
pub struct ParseCache {
    entries: RefCell<HashMap<ContentHash, CachedParse>>,
    hits: Cell<u64>,
    misses: Cell<u64>,
}

impl ParseCache {
    /// Empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `f` against the cached `ParserReturn` for `file`. On
    /// cache miss the file is parsed and the result stored before
    /// `f` runs; on hit the cached AST is reused without
    /// re-parsing.
    ///
    /// The callback returns `ParserReturn` (not [`cofferdam_core::parser::ParsedView`])
    /// so callers can inspect `panicked` and other parse-error
    /// metadata without going through the view's narrower surface.
    /// Construct a `ParsedView` from `parsed.program` /
    /// `parsed.errors` inside the callback when that shape is
    /// needed by checks.
    ///
    /// The callback shape keeps the parsed-tree borrow inside the
    /// cell — the AST borrows from the cache entry's owned
    /// `Allocator`, which would otherwise dangle if returned.
    pub fn with_parsed<F, R>(&self, file: &SourceFile, f: F) -> R
    where
        F: FnOnce(&ParserReturn<'_>) -> R,
    {
        let hash = hash_text(&file.text);
        let mut map = self.entries.borrow_mut();
        let entry = match map.entry(hash) {
            Entry::Occupied(slot) => {
                self.hits.set(self.hits.get() + 1);
                slot.into_mut()
            }
            Entry::Vacant(slot) => {
                self.misses.set(self.misses.get() + 1);
                let owner = CachedOwner {
                    allocator: Allocator::default(),
                    file: file.clone(),
                };
                slot.insert(CachedParse::new(owner, |o| {
                    parse_into(&o.allocator, &o.file)
                }))
            }
        };
        entry.with_dependent(|_owner, parsed| f(parsed))
    }

    /// Number of cache hits since construction.
    pub fn hits(&self) -> u64 {
        self.hits.get()
    }

    /// Number of cache misses (i.e. parse calls) since construction.
    pub fn misses(&self) -> u64 {
        self.misses.get()
    }

    /// Number of distinct content-hashes currently retained.
    pub fn len(&self) -> usize {
        self.entries.borrow().len()
    }

    /// True when no files have been cached yet.
    pub fn is_empty(&self) -> bool {
        self.entries.borrow().is_empty()
    }

    /// Drop every cached parse. Useful when a config or
    /// engine-version change invalidates the entire cache (cp2 wires
    /// invalidation on those axes; cp1 just exposes the primitive).
    pub fn clear(&self) {
        self.entries.borrow_mut().clear();
        self.hits.set(0);
        self.misses.set(0);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn ts_file(name: &str, text: &str) -> SourceFile {
        SourceFile::new(PathBuf::from(name), text.to_string())
    }

    #[test]
    fn hash_is_deterministic() {
        let a = hash_text("export const x = 1;");
        let b = hash_text("export const x = 1;");
        assert_eq!(a, b);
    }

    #[test]
    fn hash_differs_on_byte_change() {
        let a = hash_text("export const x = 1;");
        let b = hash_text("export const x = 2;");
        assert_ne!(a, b);
    }

    #[test]
    fn first_call_is_a_miss_second_is_a_hit() {
        let cache = ParseCache::new();
        let file = ts_file("a.ts", "export const x = 1;\n");

        cache.with_parsed(&file, |parsed| {
            assert!(!parsed.program.body.is_empty());
        });
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);

        cache.with_parsed(&file, |parsed| {
            assert!(!parsed.program.body.is_empty());
        });
        assert_eq!(cache.misses(), 1, "second call must be a hit");
        assert_eq!(cache.hits(), 1);
    }

    #[test]
    fn distinct_content_takes_distinct_slots() {
        let cache = ParseCache::new();
        let a = ts_file("a.ts", "export const x = 1;\n");
        let b = ts_file("b.ts", "export const y = 2;\n");
        cache.with_parsed(&a, |_| {});
        cache.with_parsed(&b, |_| {});
        assert_eq!(cache.misses(), 2);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn identical_text_under_different_paths_collapses() {
        // Two files with byte-identical text get the same content
        // hash and therefore the same cache slot. That's fine for
        // cp1 — the parsed AST doesn't carry the path. Per-file
        // findings caches in cp2 will need a richer key.
        let cache = ParseCache::new();
        let a = ts_file("a.ts", "export const x = 1;\n");
        let b = ts_file("b.ts", "export const x = 1;\n");
        cache.with_parsed(&a, |_| {});
        cache.with_parsed(&b, |_| {});
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn clear_drops_entries_and_resets_counters() {
        let cache = ParseCache::new();
        let file = ts_file("a.ts", "export const x = 1;\n");
        cache.with_parsed(&file, |_| {});
        cache.with_parsed(&file, |_| {});
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 1);

        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.misses(), 0);

        cache.with_parsed(&file, |_| {});
        assert_eq!(cache.misses(), 1, "post-clear call is a miss again");
    }

    #[test]
    fn diagnostics_survive_the_round_trip() {
        // Parse a malformed file; the cached entry should expose
        // the same diagnostics on every read.
        let cache = ParseCache::new();
        let file = ts_file("bad.ts", "const = ;\n");

        let first: Vec<String> = cache.with_parsed(&file, |p| {
            p.errors.iter().map(|d| d.message.to_string()).collect()
        });
        let second: Vec<String> = cache.with_parsed(&file, |p| {
            p.errors.iter().map(|d| d.message.to_string()).collect()
        });

        assert_eq!(first, second);
        assert!(
            !first.is_empty(),
            "malformed input must produce diagnostics"
        );
        assert_eq!(cache.hits(), 1, "second read must be a cache hit");
    }
}
