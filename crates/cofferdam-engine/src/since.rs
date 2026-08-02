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

use cofferdam_core::{ChangeSet, LineRange};
use std::collections::{BTreeMap, HashSet};
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

/// Parse `git diff -U0` output into a `ChangeSet`. Pure — testable
/// without git. Only `+++ b/<path>` headers and `@@` hunk headers are
/// read. Post-image ranges come from the `+<start>[,<count>]` field;
/// `count == 0` (pure deletion hunk) contributes no range but the file
/// still counts as changed. `+++ /dev/null` (deleted file) is skipped —
/// callers use `--diff-filter=AMR` so it should not appear, but defend.
pub fn parse_unified_changeset(raw: &str, repo_root: &Path) -> ChangeSet {
    let mut files = std::collections::BTreeSet::new();
    let mut line_ranges: BTreeMap<PathBuf, Vec<LineRange>> = BTreeMap::new();
    let mut current: Option<PathBuf> = None;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            if rest == "/dev/null" {
                current = None;
                continue;
            }
            let rel = rest.strip_prefix("b/").unwrap_or(rest);
            let abs = repo_root.join(rel);
            files.insert(abs.clone());
            current = Some(abs);
        } else if let (Some(path), Some(hunk)) = (&current, line.strip_prefix("@@ ")) {
            // Format: -<old>[,<n>] +<start>[,<count>] @@ ...
            let Some(plus) = hunk.split_whitespace().find_map(|t| t.strip_prefix('+')) else {
                continue;
            };
            let (start_s, count_s) = match plus.split_once(',') {
                Some((s, c)) => (s, c),
                None => (plus, "1"),
            };
            let (Ok(start), Ok(count)) = (start_s.parse::<u32>(), count_s.parse::<u32>()) else {
                continue;
            };
            if count == 0 {
                continue;
            }
            line_ranges
                .entry(path.clone())
                .or_default()
                .push(LineRange {
                    start,
                    end: start + count - 1,
                });
        }
    }
    ChangeSet { files, line_ranges }
}

/// Which delta a `cofferdam context` run resolves.
#[derive(Debug, Clone)]
pub enum DiffMode {
    /// Staged + unstaged vs HEAD (`git diff HEAD`). The default.
    WorkingTree,
    /// Staged only (`git diff --cached`).
    Staged,
    /// Working tree vs `merge-base(<ref>, HEAD)` — "everything on
    /// this branch", committed or not.
    Base(String),
}

/// Resolve `mode` to a `ChangeSet` with one `git diff -U0` call.
/// `-U0` gives exact post-image hunk ranges with zero context lines;
/// `--diff-filter=AMR` matches the module-wide deletion rationale.
pub fn diff_changeset(repo_root: &Path, mode: &DiffMode) -> Result<ChangeSet, SinceError> {
    let mut args: Vec<String> = vec![
        "diff".into(),
        "-U0".into(),
        "--no-color".into(),
        "--diff-filter=AMR".into(),
    ];
    match mode {
        DiffMode::WorkingTree => args.push("HEAD".into()),
        DiffMode::Staged => args.push("--cached".into()),
        DiffMode::Base(git_ref) => {
            let mb = merge_base(repo_root, git_ref)?;
            args.push(mb);
        }
    }
    let out = Command::new("git")
        .args(args.iter().map(String::as_str))
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
    Ok(parse_unified_changeset(raw, repo_root))
}

/// `git merge-base <ref> HEAD`. Errors surface as `DiffFailed` — the
/// caller's diagnostic ("bad ref") is the same class.
fn merge_base(repo_root: &Path, git_ref: &str) -> Result<String, SinceError> {
    let out = Command::new("git")
        .args(["merge-base", git_ref, "HEAD"])
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
    Ok(raw.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_unified_changeset_extracts_files_and_post_image_ranges() {
        let raw = "\
diff --git a/src/a.ts b/src/a.ts
index 111..222 100644
--- a/src/a.ts
+++ b/src/a.ts
@@ -10,2 +10,3 @@ fn ctx
+line
+line
+line
@@ -30,0 +34 @@
+single
diff --git a/src/gone.ts b/src/gone.ts
--- a/src/gone.ts
+++ b/src/gone.ts
@@ -5,2 +4,0 @@
-removed
-removed
";
        let root = Path::new("/repo");
        let cs = parse_unified_changeset(raw, root);
        let a = root.join("src/a.ts");
        let gone = root.join("src/gone.ts");
        assert!(cs.files.contains(&a));
        assert!(cs.files.contains(&gone)); // file changed even if hunk is pure deletion
        assert_eq!(
            cs.line_ranges[&a],
            vec![
                LineRange { start: 10, end: 12 },
                LineRange { start: 34, end: 34 }
            ]
        );
        assert!(!cs.line_ranges.contains_key(&gone)); // no post-image lines
    }

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
