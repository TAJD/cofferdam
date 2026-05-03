//! Property-based tests for `cofferdam-engine::discover`.
//!
//! **Invariant**: calling `discover` N times on the same directory + options
//! must produce identical results — same length, same order, same elements.
//!
//! We generate a small set of file names / extensions from a property strategy,
//! write them into a `TempDir`, and call `discover` three times, asserting all
//! results are equal.
//!
//! Discovery is already deterministic because `discover` calls `out.sort()`.
//! This suite drives a property-based confirmation across a wide variety of
//! generated directory structures, catching any future regression where that
//! guarantee might be accidentally removed (e.g. parallel walker + missing
//! sort).

use std::fs;
use std::path::PathBuf;

use proptest::prelude::*;
use tempfile::TempDir;

use cofferdam_engine::{discover, DiscoveryOptions};

// ──────────────────────────────────────────────────────────────────────────
// Strategies
// ──────────────────────────────────────────────────────────────────────────

/// Valid file-name stem: 1–8 alphanumeric characters (no separators that
/// could produce surprising path behavior on Windows).
fn file_stem() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9]{0,7}".prop_map(|s| s)
}

/// One of the four extensions cofferdam understands by default.
fn ts_extension() -> impl Strategy<Value = &'static str> {
    prop_oneof![Just("ts"), Just("tsx"), Just("mts"), Just("cts"),]
}

/// A single file name: `<stem>.<ext>`, never a declaration file so we don't
/// have to track the skip_declaration_files flag.
fn ts_filename() -> impl Strategy<Value = String> {
    (file_stem(), ts_extension()).prop_map(|(stem, ext)| format!("{}.{}", stem, ext))
}

/// A list of 0–8 distinct file names to write into the temp dir.
///
/// We deduplicate because the same name in the same directory is meaningless
/// and would write over an existing file silently, making the count wrong.
fn file_set() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(ts_filename(), 0..=8).prop_map(|mut names| {
        names.sort();
        names.dedup();
        names
    })
}

// ──────────────────────────────────────────────────────────────────────────
// Property tests
// ──────────────────────────────────────────────────────────────────────────

proptest! {
    /// Calling `discover` three times on the same directory returns the same
    /// `Vec<PathBuf>` each time (same length, same order, same elements).
    #[test]
    fn prop_discover_is_deterministic(names in file_set()) {
        let temp_dir = TempDir::new().expect("temp dir");

        for name in &names {
            fs::write(temp_dir.path().join(name), b"").expect("write file");
        }

        let opts = DiscoveryOptions {
            // Turn off .gitignore processing so temp dirs outside a git repo
            // behave consistently.
            respect_ignore: false,
            ..DiscoveryOptions::default()
        };

        let root: &[&std::path::Path] = &[temp_dir.path()];

        let first: Vec<PathBuf>  = discover(root, &opts).expect("discover call 1");
        let second: Vec<PathBuf> = discover(root, &opts).expect("discover call 2");
        let third: Vec<PathBuf>  = discover(root, &opts).expect("discover call 3");

        prop_assert_eq!(&first, &second,
            "discover call 1 and 2 differ for names={:?}", names);
        prop_assert_eq!(&second, &third,
            "discover call 2 and 3 differ for names={:?}", names);

        // Also assert the count matches what we wrote (sanity check that our
        // file-set strategy produced distinct names).
        prop_assert_eq!(
            first.len(),
            names.len(),
            "expected {} files, got {}: names={:?} paths={:?}",
            names.len(),
            first.len(),
            names,
            first
        );
    }

    /// Results are sorted (PathBuf lexicographic order).
    #[test]
    fn prop_discover_result_is_sorted(names in file_set()) {
        let temp_dir = TempDir::new().expect("temp dir");

        for name in &names {
            fs::write(temp_dir.path().join(name), b"").expect("write file");
        }

        let opts = DiscoveryOptions {
            respect_ignore: false,
            ..DiscoveryOptions::default()
        };

        let files: Vec<PathBuf> =
            discover(&[temp_dir.path()], &opts).expect("discover");

        let mut sorted = files.clone();
        sorted.sort();

        prop_assert_eq!(&files, &sorted,
            "discover result is not sorted for names={:?}", names);
    }
}
