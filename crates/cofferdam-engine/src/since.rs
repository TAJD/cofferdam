//! `--since <ref>` PR-mode helpers.
//!
//! The user wants `cofferdam check --since main` to run only against
//! files that changed on the current branch since `main`. Mechanism:
//! shell out to `git diff --name-only --diff-filter=AMR <ref>...HEAD`,
//! intersect with the file set discovery already produced.
//!
//! Why subprocess and not libgit2: libgit2's Rust bindings double the
//! release-mode binary size. For a single `diff --name-only` call we
//! don't need a managed object database — `git` on PATH is enough.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum SinceError {
    #[error("failed to invoke git: {0}")]
    Spawn(#[source] io::Error),
    #[error("`git rev-parse --show-toplevel` failed (exit {code}): {stderr}")]
    NotInRepo { code: i32, stderr: String },
    #[error("`git diff` failed (exit {code}): {stderr}")]
    DiffFailed { code: i32, stderr: String },
    #[error("git produced non-UTF8 output")]
    NonUtf8Output,
}

/// Resolve the repo root for `cwd` via `git rev-parse --show-toplevel`.
/// Returns the canonicalised root path on success.
pub fn repo_root(cwd: &Path) -> Result<PathBuf, SinceError> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .map_err(SinceError::Spawn)?;
    if !out.status.success() {
        return Err(SinceError::NotInRepo {
            code: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    let raw = std::str::from_utf8(&out.stdout).map_err(|_| SinceError::NonUtf8Output)?;
    Ok(PathBuf::from(raw.trim()))
}

/// List the files changed in `<git_ref>...HEAD` (added, modified, or
/// renamed). Paths are returned absolute, joined under `repo_root`.
///
/// `--diff-filter=AMR` deliberately excludes deletions — there's no
/// file left for cofferdam to analyse.
pub fn changed_files_since(repo_root: &Path, git_ref: &str) -> Result<Vec<PathBuf>, SinceError> {
    let range = format!("{}...HEAD", git_ref);
    let out = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=AMR", &range])
        .current_dir(repo_root)
        .output()
        .map_err(SinceError::Spawn)?;
    if !out.status.success() {
        return Err(SinceError::DiffFailed {
            code: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    let raw = std::str::from_utf8(&out.stdout).map_err(|_| SinceError::NonUtf8Output)?;
    let mut paths = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        paths.push(repo_root.join(trimmed));
    }
    Ok(paths)
}

/// Filter `discovered` to the subset that also appears in `changed`.
/// Comparison is canonicalised so paths from different sources
/// (relative discovery output vs git's repo-relative output) line up.
/// Files that don't exist (canonicalize fails) are dropped from the
/// match — `--diff-filter=AMR` should never produce one but defending
/// here is cheap.
pub fn intersect(discovered: &[PathBuf], changed: &[PathBuf]) -> Vec<PathBuf> {
    let changed_canonical: HashSet<PathBuf> = changed
        .iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect();

    discovered
        .iter()
        .filter(|p| {
            std::fs::canonicalize(p)
                .map(|c| changed_canonical.contains(&c))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// `intersect` returns only files present in both lists, comparing
    /// canonical paths.
    #[test]
    fn intersect_matches_canonical_paths() {
        let dir = tempdir().expect("tempdir");
        let a = dir.path().join("a.ts");
        let b = dir.path().join("b.ts");
        let c = dir.path().join("c.ts");
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        std::fs::write(&c, "").unwrap();

        let discovered = vec![a.clone(), b.clone(), c.clone()];
        let changed = vec![a.clone(), c.clone()];
        let got = intersect(&discovered, &changed);
        assert_eq!(got.len(), 2);
        // Order preserved from `discovered`.
        assert!(got[0].ends_with("a.ts"));
        assert!(got[1].ends_with("c.ts"));
    }

    #[test]
    fn intersect_drops_nonexistent_paths() {
        let dir = tempdir().expect("tempdir");
        let real = dir.path().join("real.ts");
        std::fs::write(&real, "").unwrap();
        let bogus = dir.path().join("bogus.ts");

        let discovered = vec![real.clone(), bogus.clone()];
        let changed = vec![real.clone()];
        let got = intersect(&discovered, &changed);
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with("real.ts"));
    }

    #[test]
    fn intersect_empty_changed_list_yields_empty() {
        let got = intersect(&[PathBuf::from("anything")], &[]);
        assert!(got.is_empty());
    }
}
