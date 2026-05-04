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

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, RwLock};

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

struct SlotEntry {
    type_id: TypeId,
    type_name: &'static str,
    value: Mutex<Box<dyn Any + Send>>,
}

/// Run-scoped shared store. The engine builds one per analysis run,
/// passes it by `&` into every `CheckContext`, and hands the same instance
/// to `FinalizeContext`. Slots survive across all `Check::run` calls so
/// `finalize` can see the aggregated state.
///
/// Lock layout: an outer `RwLock<HashMap>` is held only for slot
/// lookup/insertion; each slot has its own `Mutex`. Two checks targeting
/// distinct keys can therefore run their `with_slot` closures
/// concurrently — only same-key calls serialise.
#[derive(Default)]
pub struct CorpusIndex {
    slots: RwLock<HashMap<&'static str, Arc<SlotEntry>>>,
}

impl CorpusIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrow the typed slot for `key`, lazily initialising with
    /// `T::default()` on first access. Holds the slot's mutex for the
    /// duration of `f`; the outer map lock is released before `f` runs,
    /// so `f` does not block access to other slots.
    ///
    /// # Panics
    ///
    /// Panics if a previous holder of `key.name()` registered a slot of a
    /// different type, which can only happen via two `CorpusKey<T>`
    /// constants sharing a name across mismatched `T`s. That's a
    /// programmer error caught immediately on the first run.
    pub fn with_slot<T, R>(&self, key: &CorpusKey<T>, f: impl FnOnce(&mut T) -> R) -> R
    where
        T: 'static + Send + Default,
    {
        let entry = self.entry::<T>(key.name);
        let mut guard = entry.value.lock().expect("CorpusIndex slot poisoned");
        let typed = guard
            .downcast_mut::<T>()
            .expect("CorpusIndex slot type id matched but downcast failed");
        f(typed)
    }

    fn entry<T>(&self, name: &'static str) -> Arc<SlotEntry>
    where
        T: 'static + Send + Default,
    {
        if let Some(existing) = self
            .slots
            .read()
            .expect("CorpusIndex map poisoned")
            .get(name)
            .cloned()
        {
            assert_type::<T>(&existing, name);
            return existing;
        }

        let mut write = self.slots.write().expect("CorpusIndex map poisoned");
        if let Some(existing) = write.get(name).cloned() {
            assert_type::<T>(&existing, name);
            return existing;
        }
        let entry = Arc::new(SlotEntry {
            type_id: TypeId::of::<T>(),
            type_name: std::any::type_name::<T>(),
            value: Mutex::new(Box::<T>::default() as Box<dyn Any + Send>),
        });
        write.insert(name, Arc::clone(&entry));
        entry
    }
}

fn assert_type<T: 'static>(entry: &SlotEntry, name: &'static str) {
    if entry.type_id != TypeId::of::<T>() {
        panic!(
            "CorpusKey type mismatch on {name:?}: existing slot is {existing}, requested {requested}",
            existing = entry.type_name,
            requested = std::any::type_name::<T>(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::thread;
    use std::time::{Duration, Instant};

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

    #[test]
    fn distinct_keys_do_not_serialise() {
        // Two threads holding two different slots concurrently: total
        // wall time should be ~one sleep, not two.
        static A: CorpusKey<Vec<u32>> = CorpusKey::new("test.parallel.a");
        static B: CorpusKey<Vec<u32>> = CorpusKey::new("test.parallel.b");
        let corpus = Arc::new(CorpusIndex::new());
        // Pre-create both slots so the write lock isn't part of the timing.
        corpus.with_slot(&A, |_| {});
        corpus.with_slot(&B, |_| {});

        let barrier = Arc::new(Barrier::new(2));
        let hold = Duration::from_millis(200);

        let c1 = Arc::clone(&corpus);
        let b1 = Arc::clone(&barrier);
        let t1 = thread::spawn(move || {
            c1.with_slot(&A, |v| {
                b1.wait();
                thread::sleep(hold);
                v.push(1);
            });
        });

        let c2 = Arc::clone(&corpus);
        let b2 = Arc::clone(&barrier);
        let start = Instant::now();
        let t2 = thread::spawn(move || {
            c2.with_slot(&B, |v| {
                b2.wait();
                thread::sleep(hold);
                v.push(2);
            });
        });

        t1.join().unwrap();
        t2.join().unwrap();
        let elapsed = start.elapsed();

        // If serialised we'd see ~2*hold. Allow generous slack for CI.
        assert!(
            elapsed < hold * 2,
            "expected concurrent slots, took {elapsed:?} (hold={hold:?})"
        );
    }

    #[test]
    fn same_key_serialises() {
        // Many threads pushing into the same slot; the per-slot mutex
        // must serialise them so no push is lost.
        static SHARED: CorpusKey<Vec<u32>> = CorpusKey::new("test.shared");
        let corpus = Arc::new(CorpusIndex::new());
        let threads = 8;
        let pushes_per_thread = 250;

        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let c = Arc::clone(&corpus);
                thread::spawn(move || {
                    for i in 0..pushes_per_thread {
                        c.with_slot(&SHARED, |v| v.push(t * pushes_per_thread + i));
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let total = corpus.with_slot(&SHARED, |v| v.len());
        assert_eq!(total, (threads * pushes_per_thread) as usize);
    }
}
