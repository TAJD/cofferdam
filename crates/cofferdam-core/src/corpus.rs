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
//!
//! ## Two surfaces: built-in vs plugin
//!
//! Built-in checks ship as a single workspace where `CorpusKey<T>`
//! constants are compile-time-reviewed: two built-ins sharing a name
//! with mismatched `T` is a logic bug caught by the panicking
//! `with_slot` on the first run. Plugins are hostile-by-default — two
//! unrelated plugin authors could reasonably pick
//! `"export.fingerprints"` and collide. To avoid that, plugin contexts
//! use the namespaced fallible API:
//!
//! * `with_slot` / `try_with_slot` — raw access. Built-ins.
//! * `try_with_namespaced_slot(check_id, key, ...)` — prefixes the
//!   slot name with the calling check's id. Plugins.

use std::any::{Any, TypeId};
use std::borrow::Cow;
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
    /// Construct a corpus slot identifier. `name` is the storage key
    /// and uniquely identifies the slot across the run. Two
    /// `CorpusKey<T>` constants sharing a `name` AND `T` reference
    /// the same slot; sharing `name` with mismatched `T` is a logic
    /// bug that surfaces as `CorpusError::TypeMismatch` on the
    /// fallible API or a panic on the built-in path.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _t: PhantomData,
        }
    }

    /// The storage key this `CorpusKey<T>` references.
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

/// Errors from the fallible corpus API. The panicking `with_slot`
/// converts these into panics for the built-in path; plugins surface
/// them as ordinary errors.
#[derive(Debug, thiserror::Error)]
pub enum CorpusError {
    #[error(
        "CorpusKey type mismatch on {name:?}: existing slot is `{existing}`, requested `{requested}`"
    )]
    TypeMismatch {
        name: String,
        existing: &'static str,
        requested: &'static str,
    },
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
    slots: RwLock<HashMap<Cow<'static, str>, Arc<SlotEntry>>>,
}

impl CorpusIndex {
    /// Build an empty corpus. The engine creates one per analysis
    /// run; tests usually want `CorpusIndex::default()` (same shape).
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
    ///
    /// Plugin code that can't statically guarantee single-author key
    /// names should use [`Self::try_with_namespaced_slot`] instead, which
    /// auto-prefixes by check id and returns an error rather than
    /// panicking.
    pub fn with_slot<T, R>(&self, key: &CorpusKey<T>, f: impl FnOnce(&mut T) -> R) -> R
    where
        T: 'static + Send + Default,
    {
        match self.try_with_slot(key, f) {
            Ok(r) => r,
            Err(e) => panic!("{e}"),
        }
    }

    /// Fallible variant of [`Self::with_slot`]: returns
    /// `CorpusError::TypeMismatch` instead of panicking when a slot was
    /// previously registered under the same name with a different type.
    /// Use this from plugin contexts; built-ins continue to prefer
    /// `with_slot` so logic bugs surface loudly at code-review time.
    pub fn try_with_slot<T, R>(
        &self,
        key: &CorpusKey<T>,
        f: impl FnOnce(&mut T) -> R,
    ) -> Result<R, CorpusError>
    where
        T: 'static + Send + Default,
    {
        let entry = self.entry::<T>(Cow::Borrowed(key.name()))?;
        let mut guard = entry.value.lock().expect("CorpusIndex slot poisoned");
        let typed = guard
            .downcast_mut::<T>()
            .expect("CorpusIndex slot type id matched but downcast failed");
        Ok(f(typed))
    }

    /// Plugin-safe slot access: namespaces the lookup key by the calling
    /// check's id, so two plugin authors picking the same naive
    /// `CorpusKey` name don't collide. Returns `CorpusError::TypeMismatch`
    /// when the *same* check id reuses a name with mismatched `T`.
    ///
    /// The effective storage key is `format!("{check_id}::{name}")`. Two
    /// distinct check ids therefore see independent slots even when their
    /// `CorpusKey<T>` constants share a name.
    ///
    /// Built-ins that genuinely want to share storage across check
    /// boundaries (e.g. the IMPORTS / EXPORTS slots in
    /// `cofferdam_core::graph`) should keep using [`Self::with_slot`]
    /// directly — the namespace is intentional isolation, not a default.
    pub fn try_with_namespaced_slot<T, R>(
        &self,
        check_id: &str,
        key: &CorpusKey<T>,
        f: impl FnOnce(&mut T) -> R,
    ) -> Result<R, CorpusError>
    where
        T: 'static + Send + Default,
    {
        let effective = Cow::Owned(namespaced_key(check_id, key.name()));
        let entry = self.entry::<T>(effective)?;
        let mut guard = entry.value.lock().expect("CorpusIndex slot poisoned");
        let typed = guard
            .downcast_mut::<T>()
            .expect("CorpusIndex slot type id matched but downcast failed");
        Ok(f(typed))
    }

    fn entry<T>(&self, name: Cow<'static, str>) -> Result<Arc<SlotEntry>, CorpusError>
    where
        T: 'static + Send + Default,
    {
        if let Some(existing) = self
            .slots
            .read()
            .expect("CorpusIndex map poisoned")
            .get(name.as_ref())
            .cloned()
        {
            check_type::<T>(&existing, name.as_ref())?;
            return Ok(existing);
        }

        let mut write = self.slots.write().expect("CorpusIndex map poisoned");
        if let Some(existing) = write.get(name.as_ref()).cloned() {
            check_type::<T>(&existing, name.as_ref())?;
            return Ok(existing);
        }
        let entry = Arc::new(SlotEntry {
            type_id: TypeId::of::<T>(),
            type_name: std::any::type_name::<T>(),
            value: Mutex::new(Box::<T>::default() as Box<dyn Any + Send>),
        });
        write.insert(name, Arc::clone(&entry));
        Ok(entry)
    }
}

fn namespaced_key(check_id: &str, slot_name: &str) -> String {
    let mut s = String::with_capacity(check_id.len() + 2 + slot_name.len());
    s.push_str(check_id);
    s.push_str("::");
    s.push_str(slot_name);
    s
}

fn check_type<T: 'static>(entry: &SlotEntry, name: &str) -> Result<(), CorpusError> {
    if entry.type_id != TypeId::of::<T>() {
        return Err(CorpusError::TypeMismatch {
            name: name.to_owned(),
            existing: entry.type_name,
            requested: std::any::type_name::<T>(),
        });
    }
    Ok(())
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
    fn try_with_slot_returns_err_on_type_mismatch() {
        static AS_VEC: CorpusKey<Vec<u32>> = CorpusKey::new("test.try.collide");
        static AS_STRING: CorpusKey<String> = CorpusKey::new("test.try.collide");
        let corpus = CorpusIndex::new();
        corpus.try_with_slot(&AS_VEC, |v| v.push(1)).expect("first");
        let err = corpus
            .try_with_slot(&AS_STRING, |s| s.push_str("boom"))
            .unwrap_err();
        assert!(matches!(err, CorpusError::TypeMismatch { .. }));
        // Original slot is unchanged — the fallible API doesn't mutate
        // anything when the type check fails.
        let len = corpus.with_slot(&AS_VEC, |v| v.len());
        assert_eq!(len, 1);
    }

    #[test]
    fn namespaced_slots_isolate_same_naive_name() {
        // cd-9hp.7 acceptance: two plugins picking the same naive slot
        // name no longer collide once routed through the namespaced API.
        static SHARED: CorpusKey<Vec<u32>> = CorpusKey::new("export.fingerprints");
        let corpus = CorpusIndex::new();
        corpus
            .try_with_namespaced_slot("Plugin.A.Check", &SHARED, |v| v.push(1))
            .expect("a write");
        corpus
            .try_with_namespaced_slot("Plugin.B.Check", &SHARED, |v| v.push(2))
            .expect("b write");
        let a = corpus
            .try_with_namespaced_slot("Plugin.A.Check", &SHARED, |v| v.clone())
            .expect("a read");
        let b = corpus
            .try_with_namespaced_slot("Plugin.B.Check", &SHARED, |v| v.clone())
            .expect("b read");
        assert_eq!(a, vec![1]);
        assert_eq!(b, vec![2]);
    }

    #[test]
    fn namespaced_and_raw_keys_are_distinct() {
        // A raw `with_slot(KEY)` and a `try_with_namespaced_slot(id, KEY)`
        // address different storage even when the key name matches —
        // namespacing is intentional isolation, not a transparent rename.
        static SHARED: CorpusKey<Vec<u32>> = CorpusKey::new("test.iso");
        let corpus = CorpusIndex::new();
        corpus.with_slot(&SHARED, |v| v.push(42));
        let plugin_view = corpus
            .try_with_namespaced_slot("Plugin.X", &SHARED, |v| v.clone())
            .expect("plugin read");
        assert!(plugin_view.is_empty(), "namespaced slot starts fresh");
        let raw_view = corpus.with_slot(&SHARED, |v| v.clone());
        assert_eq!(raw_view, vec![42]);
    }

    #[test]
    fn namespaced_slot_round_trips() {
        static KEY: CorpusKey<Vec<u32>> = CorpusKey::new("test.ns.round");
        let corpus = CorpusIndex::new();
        corpus
            .try_with_namespaced_slot("Plugin.A", &KEY, |v| v.extend([1, 2, 3]))
            .expect("write");
        let read = corpus
            .try_with_namespaced_slot("Plugin.A", &KEY, |v| v.clone())
            .expect("read");
        assert_eq!(read, vec![1, 2, 3]);
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
