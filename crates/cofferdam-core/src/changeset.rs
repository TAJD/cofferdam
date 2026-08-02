//! The resolved change delta handed to Context-category checks.
//!
//! Built by the CLI (git diff modes or an explicit file list) and
//! threaded to `Check::context_items`. Paths are absolute.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 1-based inclusive post-image line range of one diff hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

/// The set of files (and, when resolved from a git diff, the changed
/// post-image line ranges) that a `cofferdam context` run considers
/// "the change". Explicit-files mode has `line_ranges` empty — providers
/// must treat a missing entry as "whole file changed".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChangeSet {
    pub files: BTreeSet<PathBuf>,
    pub line_ranges: BTreeMap<PathBuf, Vec<LineRange>>,
}

impl ChangeSet {
    pub fn from_files(files: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            files: files.into_iter().collect(),
            line_ranges: BTreeMap::new(),
        }
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.files.contains(path)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_files_dedupes_and_contains() {
        let cs = ChangeSet::from_files([PathBuf::from("/a/x.ts"), PathBuf::from("/a/x.ts")]);
        assert_eq!(cs.files.len(), 1);
        assert!(cs.contains(Path::new("/a/x.ts")));
        assert!(!cs.contains(Path::new("/a/y.ts")));
        assert!(!cs.is_empty());
    }

    #[test]
    fn empty_changeset_reports_empty() {
        let cs = ChangeSet::default();
        assert!(cs.is_empty());
        assert!(cs.line_ranges.is_empty());
    }

    #[test]
    fn line_range_serde_roundtrip() {
        let r = LineRange { start: 3, end: 7 };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<LineRange>(&json).unwrap(), r);
    }
}
