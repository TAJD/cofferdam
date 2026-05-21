//! Disk-backed findings + run cache (cd-9hp.4 cp4).
//!
//! Layered on top of cp2's [`crate::findings_cache::FindingsCache`]
//! and cp3's [`crate::run_cache::RunCache`]: those caches live in
//! memory for the life of one process. cp4 persists their contents
//! to disk between runs so a fresh `cofferdam check` can hydrate the
//! in-memory caches from disk before analyzing.
//!
//! ## Layout
//!
//! ```text
//! <cache_dir>/<engine_version>/findings.json
//! <cache_dir>/<engine_version>/run.json
//! ```
//!
//! Engine-version-scoped directory means a `cofferdam` upgrade
//! ignores prior caches without explicit invalidation — different
//! checks can emit different findings on the same input, so
//! cross-version reuse is incorrect by construction.
//!
//! The default `<cache_dir>` is `.cofferdam/cache/` under the CLI
//! invocation's CWD; callers can override.
//!
//! ## Format
//!
//! Plain JSON. cp4 sign-off picked JSON over postcard / bincode
//! because:
//!
//! - `serde_json` is already a workspace dep (zero new crates).
//! - Cache files for `bestefforttools` (~325 files × ~20 checks)
//!   serialise to ~1–3 MB — well within the budget for sub-50ms
//!   read/parse on modern SSDs.
//! - JSON is greppable for debugging.
//!
//! If the cache ever grows past low-MB territory or ser/des becomes
//! a measurable cost, the swap to postcard is a self-contained
//! change.
//!
//! ## Atomicity
//!
//! Writes go through `<file>.tmp` followed by `std::fs::rename`.
//! Rust's std-library rename on Windows uses `MoveFileExW` with
//! `MOVEFILE_REPLACE_EXISTING`, which is atomic at the NTFS layer.
//! On POSIX, `rename(2)` is atomic. Interruption mid-write at worst
//! leaves a `<file>.tmp` orphan; the cache itself stays consistent.
//!
//! ## Corruption recovery
//!
//! Load errors (missing file, malformed JSON, schema mismatch) are
//! NOT fatal — the loader returns `Ok(0)` after silently discarding
//! the broken cache. The next analyze rebuilds from scratch and the
//! next save overwrites the bad file. A build cache is rebuildable
//! by definition; surfacing the error to the user would be noise.

use std::fs;
use std::path::{Path, PathBuf};

use cofferdam_core::Issue;
use serde::{Deserialize, Serialize};

use crate::cache::ContentHash;
use crate::findings_cache::{ConfigHash, FindingsCache, FindingsKey, ENGINE_VERSION};
use crate::run_cache::{InputSetHash, RunCache, RunKey};

/// Errors surfacing from a disk-cache save. Loads never fail (see
/// module docs — corruption is silently discarded), so this enum is
/// write-side only. The CLI surfaces these as one-line warnings; a
/// failed cache write does not fail the analyze itself.
#[derive(Debug, thiserror::Error)]
pub enum DiskCacheError {
    #[error("failed to create cache directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write cache file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to rename cache file {tmp} -> {dest}: {source}")]
    Rename {
        tmp: PathBuf,
        dest: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialise cache: {0}")]
    Serialize(#[source] serde_json::Error),
}

/// Sidecar shape for the `FindingsKey` on disk. `FindingsKey` in-
/// memory uses `&'static str` for `check_id` (every registered ID
/// is a `'static` string in the registry), which doesn't round-trip
/// through `Deserialize`. The sidecar uses `String` and the loader
/// rejects entries whose `check_id` doesn't match a currently-
/// registered ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FindingsKeyOnDisk {
    content_hash: ContentHash,
    config_hash: ConfigHash,
    check_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FindingsEntry {
    key: FindingsKeyOnDisk,
    issues: Vec<Issue>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FindingsFile {
    /// Engine version this cache was written by. Read back for an
    /// extra integrity check; the directory layout already isolates
    /// versions, but a stray cache file in the wrong subtree gets
    /// caught here too.
    engine_version: String,
    entries: Vec<FindingsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunKeyOnDisk {
    input_set: InputSetHash,
    config_hash: ConfigHash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunEntry {
    key: RunKeyOnDisk,
    issues: Vec<Issue>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RunFile {
    engine_version: String,
    entries: Vec<RunEntry>,
}

/// Compute the per-version subdirectory under a user-supplied cache
/// root. `<cache_dir>/<engine_version>/` keeps each cofferdam build's
/// caches isolated.
pub fn version_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join(ENGINE_VERSION)
}

fn findings_path(cache_dir: &Path) -> PathBuf {
    version_dir(cache_dir).join("findings.json")
}

fn run_path(cache_dir: &Path) -> PathBuf {
    version_dir(cache_dir).join("run.json")
}

/// Load the disk findings cache into `cache`. `registered_ids` is the
/// set of `&'static str` IDs the engine currently knows about — used
/// to map deserialised `String` IDs back to `'static` slices so the
/// in-memory `FindingsKey` stays cheap.
///
/// Returns the number of entries hydrated. Missing or malformed cache
/// files return `Ok(0)` — see the corruption-recovery note in the
/// module docs.
pub fn load_findings(
    cache_dir: &Path,
    cache: &FindingsCache,
    registered_ids: &[&'static str],
) -> std::io::Result<usize> {
    let path = findings_path(cache_dir);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let file: FindingsFile = match serde_json::from_slice(&bytes) {
        Ok(f) => f,
        Err(_) => return Ok(0), // malformed — discard, rebuild next save
    };
    if file.engine_version != ENGINE_VERSION {
        return Ok(0); // wrong-version cache file in the right-version dir; discard
    }

    let mut hydrated = 0usize;
    for entry in file.entries {
        let static_id = match registered_ids.iter().find(|id| **id == entry.key.check_id) {
            Some(id) => *id,
            None => continue, // check was unregistered since the cache was written; drop
        };
        let key = FindingsKey {
            content_hash: entry.key.content_hash,
            config_hash: entry.key.config_hash,
            check_id: static_id,
        };
        cache.insert(key, entry.issues);
        hydrated += 1;
    }
    Ok(hydrated)
}

/// Persist `cache`'s contents to disk. Overwrites any existing
/// findings.json under the version subdir. Returns the number of
/// entries written.
pub fn save_findings(cache_dir: &Path, cache: &FindingsCache) -> Result<usize, DiskCacheError> {
    let entries: Vec<FindingsEntry> = cache
        .snapshot()
        .into_iter()
        .map(|(k, v)| FindingsEntry {
            key: FindingsKeyOnDisk {
                content_hash: k.content_hash,
                config_hash: k.config_hash,
                check_id: k.check_id.to_string(),
            },
            issues: v,
        })
        .collect();
    let count = entries.len();
    let file = FindingsFile {
        engine_version: ENGINE_VERSION.to_string(),
        entries,
    };
    let bytes = serde_json::to_vec(&file).map_err(DiskCacheError::Serialize)?;
    let dest = findings_path(cache_dir);
    atomic_write(&dest, &bytes)?;
    Ok(count)
}

/// Load the disk run-cache into `cache`. Same corruption-tolerant
/// semantics as [`load_findings`].
pub fn load_run(cache_dir: &Path, cache: &RunCache) -> std::io::Result<usize> {
    let path = run_path(cache_dir);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let file: RunFile = match serde_json::from_slice(&bytes) {
        Ok(f) => f,
        Err(_) => return Ok(0),
    };
    if file.engine_version != ENGINE_VERSION {
        return Ok(0);
    }
    let mut hydrated = 0usize;
    for entry in file.entries {
        let key = RunKey {
            input_set: entry.key.input_set,
            config_hash: entry.key.config_hash,
        };
        cache.insert(key, entry.issues);
        hydrated += 1;
    }
    Ok(hydrated)
}

/// Persist `cache`'s contents to disk. Returns the number of entries
/// written.
pub fn save_run(cache_dir: &Path, cache: &RunCache) -> Result<usize, DiskCacheError> {
    let entries: Vec<RunEntry> = cache
        .snapshot()
        .into_iter()
        .map(|(k, v)| RunEntry {
            key: RunKeyOnDisk {
                input_set: k.input_set,
                config_hash: k.config_hash,
            },
            issues: v,
        })
        .collect();
    let count = entries.len();
    let file = RunFile {
        engine_version: ENGINE_VERSION.to_string(),
        entries,
    };
    let bytes = serde_json::to_vec(&file).map_err(DiskCacheError::Serialize)?;
    let dest = run_path(cache_dir);
    atomic_write(&dest, &bytes)?;
    Ok(count)
}

fn atomic_write(dest: &Path, bytes: &[u8]) -> Result<(), DiskCacheError> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|source| DiskCacheError::CreateDir {
        path: parent.to_path_buf(),
        source,
    })?;
    let tmp = dest.with_extension("json.tmp");
    fs::write(&tmp, bytes).map_err(|source| DiskCacheError::Write {
        path: tmp.clone(),
        source,
    })?;
    fs::rename(&tmp, dest).map_err(|source| DiskCacheError::Rename {
        tmp: tmp.clone(),
        dest: dest.to_path_buf(),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::{Priority, Severity, Span};
    use tempfile::tempdir;

    fn mk_issue(check_id: &str, msg: &str) -> Issue {
        Issue {
            check_id: check_id.to_string(),
            message: msg.to_string(),
            file: PathBuf::from("a.ts"),
            span: Span {
                start_byte: 0,
                end_byte: 0,
                line: 1,
                column: 1,
            },
            priority: Priority(0),
            severity: Severity::Low,
            related: Vec::new(),
        }
    }

    fn fkey(content: u8, config: u8, check_id: &'static str) -> FindingsKey {
        FindingsKey {
            content_hash: [content; 32],
            config_hash: [config; 32],
            check_id,
        }
    }

    fn rkey(input: u8, config: u8) -> RunKey {
        RunKey {
            input_set: [input; 32],
            config_hash: [config; 32],
        }
    }

    #[test]
    fn findings_roundtrip_preserves_entries() {
        let dir = tempdir().unwrap();
        let src = FindingsCache::new();
        src.insert(
            fkey(1, 0, "Warning.TripleEquals"),
            vec![mk_issue("Warning.TripleEquals", "a")],
        );
        src.insert(
            fkey(2, 0, "Design.MaxParameters"),
            vec![
                mk_issue("Design.MaxParameters", "b"),
                mk_issue("Design.MaxParameters", "c"),
            ],
        );

        let saved = save_findings(dir.path(), &src).expect("save");
        assert_eq!(saved, 2);

        let dst = FindingsCache::new();
        let hydrated = load_findings(
            dir.path(),
            &dst,
            &["Warning.TripleEquals", "Design.MaxParameters"],
        )
        .expect("load");
        assert_eq!(hydrated, 2);

        let got = dst.get(&fkey(1, 0, "Warning.TripleEquals")).expect("hit");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, "a");
        let got = dst.get(&fkey(2, 0, "Design.MaxParameters")).expect("hit");
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn findings_load_drops_unregistered_check_ids() {
        let dir = tempdir().unwrap();
        let src = FindingsCache::new();
        src.insert(
            fkey(1, 0, "Warning.TripleEquals"),
            vec![mk_issue("Warning.TripleEquals", "a")],
        );
        src.insert(
            fkey(2, 0, "Removed.Check"),
            vec![mk_issue("Removed.Check", "stale")],
        );
        save_findings(dir.path(), &src).unwrap();

        let dst = FindingsCache::new();
        // Only register one of the two IDs — the other should be discarded.
        let hydrated = load_findings(dir.path(), &dst, &["Warning.TripleEquals"]).expect("load");
        assert_eq!(hydrated, 1);
        assert!(dst.get(&fkey(1, 0, "Warning.TripleEquals")).is_some());
    }

    #[test]
    fn missing_file_yields_zero_hydrated_no_error() {
        let dir = tempdir().unwrap();
        let dst = FindingsCache::new();
        let n = load_findings(dir.path(), &dst, &["Warning.TripleEquals"]).expect("load");
        assert_eq!(n, 0);
        assert!(dst.is_empty());
    }

    #[test]
    fn malformed_file_is_silently_discarded() {
        let dir = tempdir().unwrap();
        let vdir = version_dir(dir.path());
        fs::create_dir_all(&vdir).unwrap();
        fs::write(vdir.join("findings.json"), b"not valid json").unwrap();

        let dst = FindingsCache::new();
        let n = load_findings(dir.path(), &dst, &["Warning.TripleEquals"]).expect("load");
        assert_eq!(
            n, 0,
            "malformed JSON must be treated as no-cache, not error"
        );
    }

    #[test]
    fn wrong_engine_version_is_discarded() {
        let dir = tempdir().unwrap();
        let vdir = version_dir(dir.path());
        fs::create_dir_all(&vdir).unwrap();
        let bogus = FindingsFile {
            engine_version: "0.0.0-from-another-build".to_string(),
            entries: vec![FindingsEntry {
                key: FindingsKeyOnDisk {
                    content_hash: [1; 32],
                    config_hash: [0; 32],
                    check_id: "Warning.TripleEquals".to_string(),
                },
                issues: vec![mk_issue("Warning.TripleEquals", "x")],
            }],
        };
        fs::write(
            vdir.join("findings.json"),
            serde_json::to_vec(&bogus).unwrap(),
        )
        .unwrap();

        let dst = FindingsCache::new();
        let n = load_findings(dir.path(), &dst, &["Warning.TripleEquals"]).expect("load");
        assert_eq!(n, 0, "engine_version mismatch must be treated as no-cache");
    }

    #[test]
    fn run_cache_roundtrip_preserves_entries() {
        let dir = tempdir().unwrap();
        let src = RunCache::new();
        src.insert(rkey(1, 0), vec![mk_issue("Warning.TripleEquals", "a")]);
        src.insert(rkey(2, 0), vec![mk_issue("X", "b"), mk_issue("Y", "c")]);

        let saved = save_run(dir.path(), &src).expect("save");
        assert_eq!(saved, 2);

        let dst = RunCache::new();
        let hydrated = load_run(dir.path(), &dst).expect("load");
        assert_eq!(hydrated, 2);
        assert_eq!(dst.get(&rkey(1, 0)).unwrap()[0].message, "a");
        assert_eq!(dst.get(&rkey(2, 0)).unwrap().len(), 2);
    }

    #[test]
    fn version_dir_isolates_per_engine_build() {
        let dir = tempdir().unwrap();
        let vdir = version_dir(dir.path());
        assert!(vdir.ends_with(ENGINE_VERSION));
    }

    #[test]
    fn save_then_load_survives_overwrite() {
        // Two consecutive saves overwrite; the second must be the one
        // that loads back.
        let dir = tempdir().unwrap();
        let c = FindingsCache::new();
        c.insert(
            fkey(1, 0, "Warning.TripleEquals"),
            vec![mk_issue("Warning.TripleEquals", "v1")],
        );
        save_findings(dir.path(), &c).unwrap();

        c.clear();
        c.insert(
            fkey(1, 0, "Warning.TripleEquals"),
            vec![mk_issue("Warning.TripleEquals", "v2")],
        );
        save_findings(dir.path(), &c).unwrap();

        let dst = FindingsCache::new();
        load_findings(dir.path(), &dst, &["Warning.TripleEquals"]).unwrap();
        assert_eq!(
            dst.get(&fkey(1, 0, "Warning.TripleEquals")).unwrap()[0].message,
            "v2"
        );
    }
}
