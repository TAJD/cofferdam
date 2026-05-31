//! `--since <ref>` PR-mode helpers.
//!
//! The user wants `cofferdam check --since main` to report findings only
//! on files that changed on the current branch since `main`. The full
//! project tree is still analysed (cross-file checks need a complete
//! graph); this module resolves the *changed-file set* that the CLI uses
//! to filter findings after analysis. Mechanism: shell out to
//! `git diff --name-only --diff-filter=AMR <ref>...HEAD`, intersect with
//! the file set discovery already produced.
//!
//! Why subprocess and not libgit2: libgit2's Rust bindings double the
//! release-mode binary size. For a single `diff --name-only` call we
//! don't need a managed object database — `git` on PATH is enough.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Errors that can arise resolving the `--since <git-ref>` filter to
/// a concrete file set. Covers git invocation failure, non-zero exit,
/// and unparseable output. Surfaced to the CLI which formats them
/// into a one-line warning rather than failing the run.
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

/// List the files changed in the working tree relative to `git_ref`
/// (added, modified, or renamed). Unlike [`changed_files_since`], this
/// includes uncommitted edits — both staged and unstaged — in addition
/// to commits ahead of `git_ref`. The semantics match `git diff <ref>`
/// (no `...HEAD`) which is "everything in the working tree that differs
/// from `<ref>`."
///
/// Used by `cofferdam advise --diff <ref>` to determine the set of
/// files whose state has diverged from `<ref>`. Same `--diff-filter=AMR`
/// rationale applies — deletions have no post-diff source to analyse.
pub fn working_tree_changed_files(
    repo_root: &Path,
    git_ref: &str,
) -> Result<Vec<PathBuf>, SinceError> {
    let out = Command::new("git")
        .args(["diff", "--name-only", "--diff-filter=AMR", git_ref])
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

/// Read the contents of `path` as it existed at `git_ref`. Uses
/// `git show <ref>:<repo-relative-path>`.
///
/// Returns `Ok(None)` when the file did not exist at `<ref>` (added file
/// in the diff), differentiated from genuine errors. The pre-diff source
/// for an added file is the empty set — caller treats this as "no
/// findings to compare against."
///
/// `path` may be absolute or relative; if absolute it must be under
/// `repo_root`. We strip the prefix because git wants repo-relative
/// paths in `<ref>:<path>` syntax.
pub fn read_at_ref(
    repo_root: &Path,
    git_ref: &str,
    path: &Path,
) -> Result<Option<String>, SinceError> {
    let rel: PathBuf = if path.is_absolute() {
        match path.strip_prefix(repo_root) {
            Ok(r) => r.to_path_buf(),
            Err(_) => return Ok(None), // outside repo — treat as nonexistent at ref
        }
    } else {
        path.to_path_buf()
    };
    // Git wants forward slashes regardless of host OS in the `<ref>:<path>` syntax.
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    let spec = format!("{}:{}", git_ref, rel_str);
    let out = Command::new("git")
        .args(["show", &spec])
        .current_dir(repo_root)
        .output()
        .map_err(SinceError::Spawn)?;
    if !out.status.success() {
        // Most common failure: file did not exist at <ref> (added in diff).
        // git emits "fatal: path '<path>' does not exist in '<ref>'" or
        // similar. Treat any non-zero as "absent at ref" rather than an
        // error — the caller's mental model is "compare WT to ref," and
        // a missing pre-source is just an empty pre-state.
        return Ok(None);
    }
    let text = String::from_utf8(out.stdout).map_err(|_| SinceError::NonUtf8Output)?;
    Ok(Some(text))
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
