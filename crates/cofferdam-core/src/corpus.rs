//! Shared storage for cross-file checks.
//!
//! `Check::run` is called once per file, but some checks (DRY/duplicate
//! detection, project-graph rules like orphaned exports, context-boundary
//! analysis) need to see *all* files before they can emit findings. They
//! collect fingerprints into a `CorpusIndex` slot during the per-file
//! pass, then read the slot back in `Check::finalize`.
//!
//! Two checks share storage by referencing the same `CorpusKey<T>`
//! constant; distinct keys (or distinct `T`s) get distinct slots. Slots
//! are lazily initialised with `T::default()` on first access.
//!
//! Type erasure is contained inside `slots` — checks see only
//! `corpus.with_slot(&KEY, |t| ...)` with full type inference.

use std::any::Any;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Mutex;

/// Typed handle to a corpus slot. Two checks sharing the same `CorpusKey`
/// constant share storage. The `&'static str` name is the storage key;
/// the `T` parameter ties the key to one collection type at compile time.
pub struct CorpusKey<T> {
    name: &'static str,
    _t: PhantomData<fn() -> T>,
}

impl<T: 'static + Send + Default> CorpusKey<T> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _t: PhantomData,
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }
}

/// Run-scoped shared store. The engine builds one per analysis run,
/// passes it by `&` into every `CheckContext`, and hands the same instance
/// to `FinalizeContext`. Slots survive across all `Check::run` calls so
/// `finalize` can see the aggregated state.
///
/// All access serialises through a single `Mutex<HashMap>`. That's fine
/// for today's sequential per-file loop. When per-file parallelism lands
/// (cd-6ad), the inner type swaps to per-slot locks; the public API
/// (`CorpusKey` + `with_slot`) does not change.
#[derive(Default)]
pub struct CorpusIndex {
    slots: Mutex<HashMap<&'static str, Box<dyn Any + Send>>>,
}

impl CorpusIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrow the typed slot for `key`, lazily initialising with
    /// `T::default()` on first access. Holds the corpus lock for the
    /// duration of `f` — keep `f` short.
    ///
    /// # Panics
    ///
    /// Panics only if a previous holder of `key.name()` registered a slot
    /// of a different type, which can only happen via two `CorpusKey<T>`
    /// constants sharing a name across mismatched `T`s. That's a
    /// programmer error caught immediately on the first run.
    pub fn with_slot<T, R>(&self, key: &CorpusKey<T>, f: impl FnOnce(&mut T) -> R) -> R
    where
        T: 'static + Send + Default,
    {
        let mut guard = self.slots.lock().expect("CorpusIndex mutex poisoned");
        let slot = guard
            .entry(key.name)
            .or_insert_with(|| Box::<T>::default() as Box<dyn Any + Send>);
        let typed = slot
            .downcast_mut::<T>()
            .expect("CorpusKey type mismatch: two keys share a name with different T");
        f(typed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static COUNTERS: CorpusKey<Vec<u32>> = CorpusKey::new("test.counters");

    #[test]
    fn slot_initialises_lazily_with_default() {
        let corpus = CorpusIndex::new();
        let len = corpus.with_slot(&COUNTERS, |v| v.len());
        assert_eq!(len, 0);
    }

    #[test]
    fn slot_persists_across_calls() {
        let corpus = CorpusIndex::new();
        corpus.with_slot(&COUNTERS, |v| v.extend([1, 2, 3]));
        corpus.with_slot(&COUNTERS, |v| v.extend([4, 5]));
        let total: u32 = corpus.with_slot(&COUNTERS, |v| v.iter().sum());
        assert_eq!(total, 15);
    }

    #[test]
    fn distinct_keys_get_distinct_slots() {
        static A: CorpusKey<Vec<u32>> = CorpusKey::new("test.a");
        static B: CorpusKey<Vec<u32>> = CorpusKey::new("test.b");
        let corpus = CorpusIndex::new();
        corpus.with_slot(&A, |v| v.push(1));
        corpus.with_slot(&B, |v| v.push(2));
        assert_eq!(corpus.with_slot(&A, |v| v.clone()), vec![1]);
        assert_eq!(corpus.with_slot(&B, |v| v.clone()), vec![2]);
    }

    #[test]
    #[should_panic(expected = "CorpusKey type mismatch")]
    fn type_mismatch_panics() {
        static AS_VEC: CorpusKey<Vec<u32>> = CorpusKey::new("test.collide");
        static AS_STRING: CorpusKey<String> = CorpusKey::new("test.collide");
        let corpus = CorpusIndex::new();
        corpus.with_slot(&AS_VEC, |v| v.push(1));
        corpus.with_slot(&AS_STRING, |s| s.push_str("boom"));
    }
}
