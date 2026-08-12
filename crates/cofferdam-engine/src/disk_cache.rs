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
//! ## Format (v2 — CD-185)
//!
//! Still JSON (no new dependency, still greppable), but *normalised*.
//! The v1 layout wrote one self-contained record per cache entry, and
//! at 5000 files that was pathological:
//!
//! - Every record carried its `content_hash` and `config_hash` as
//!   `[u8; 32]`, which serde renders as a 32-element decimal array
//!   (~110 bytes each). Across 140k findings entries that was ~31 MB
//!   of nothing but hashes.
//! - 133k of those 140k entries held an *empty* `issues` array — the
//!   "this check found nothing here" fact, which is exactly what makes
//!   the cache useful, but which was paying full key overhead.
//! - Every `Issue` embedded its absolute path twice (`file` plus the
//!   `file://`-prefixed `location.uri`) and its check id in full.
//! - `run.json` accumulated one whole-project snapshot per input-set
//!   fingerprint ever seen, so it grew by ~25 MB per edit, forever.
//!
//! v2 fixes all four:
//!
//! - One string table per file; issues reference `check_id`, `message`,
//!   `file` and `uri` by `u32` index.
//! - Findings entries are grouped by content hash (hex string, one per
//!   *file*, not one per file×check), with the checks that found
//!   nothing listed as bare indices.
//! - `LocationRange::Bytes` — the shape every TS finding uses — is a
//!   4-element array rather than a tagged object; other variants fall
//!   back to the full tagged form.
//! - `save_run` persists only the most recently inserted entry.
//!
//! Measured on a 5000-file synthetic repo: `findings.json` 50.4 MB →
//! 4.0 MB, `run.json` 25.0 MB (unbounded) → 5.5 MB (bounded).
//!
//! A v1 file fails to deserialise as v2 and is discarded by the
//! corruption path below, which is the correct outcome — a build cache
//! is rebuildable by definition.
//!
//! ## Skipping redundant writes
//!
//! Both caches track whether anything was *inserted* since they were
//! hydrated. A re-run with no changes hits on everything, so the file
//! on disk is already correct and the caller skips the rewrite
//! entirely — see [`crate::findings_cache::FindingsCache::is_dirty`].
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

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use cofferdam_core::{Issue, Location, LocationRange, Priority, RelatedSpan, Severity, Uri};
use serde::{Deserialize, Serialize};

use crate::cache::ContentHash;
use crate::findings_cache::{ConfigHash, FindingsCache, FindingsKey, ENGINE_VERSION};
use crate::run_cache::{RunCache, RunKey};

/// On-disk layout revision. Bumped when the shape changes in a way a
/// prior reader can't interpret; a mismatch drops the cache.
const FORMAT_VERSION: u32 = 2;

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

// ---------------------------------------------------------------------------
// hex helpers for the 32-byte hash keys
// ---------------------------------------------------------------------------

fn hex_encode(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap_or('0'));
    }
    s
}

fn hex_decode(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = (bytes[i * 2] as char).to_digit(16)?;
        let lo = (bytes[i * 2 + 1] as char).to_digit(16)?;
        *slot = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// string interning
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Interner {
    map: HashMap<String, u32>,
    list: Vec<String>,
}

impl Interner {
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(idx) = self.map.get(s) {
            return *idx;
        }
        let idx = self.list.len() as u32;
        self.list.push(s.to_string());
        self.map.insert(s.to_string(), idx);
        idx
    }
}

fn resolve(strings: &[String], idx: u32) -> Option<&str> {
    strings.get(idx as usize).map(String::as_str)
}

// ---------------------------------------------------------------------------
// compact issue representation
// ---------------------------------------------------------------------------

/// `LocationRange::Bytes` — every TS finding — as a bare
/// `[start, end, line, column]` array. Anything else round-trips
/// through the full tagged enum. Untagged: an array and an object are
/// unambiguous to serde.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum RangeOnDisk {
    Bytes([u32; 4]),
    Other(LocationRange),
}

impl RangeOnDisk {
    fn encode(range: &LocationRange) -> RangeOnDisk {
        match range {
            LocationRange::Bytes {
                start,
                end,
                line,
                column,
            } => RangeOnDisk::Bytes([*start, *end, *line, *column]),
            other => RangeOnDisk::Other(other.clone()),
        }
    }

    fn decode(self) -> LocationRange {
        match self {
            RangeOnDisk::Bytes([start, end, line, column]) => LocationRange::Bytes {
                start,
                end,
                line,
                column,
            },
            RangeOnDisk::Other(r) => r,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelatedOnDisk {
    f: u32,
    u: u32,
    r: RangeOnDisk,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IssueOnDisk {
    /// check id (string-table index)
    c: u32,
    /// message (string-table index)
    m: u32,
    /// file path (string-table index)
    f: u32,
    /// location uri (string-table index)
    u: u32,
    r: RangeOnDisk,
    p: i8,
    s: Severity,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rel: Vec<RelatedOnDisk>,
}

fn encode_issue(issue: &Issue, si: &mut Interner) -> IssueOnDisk {
    IssueOnDisk {
        c: si.intern(&issue.check_id),
        m: si.intern(&issue.message),
        f: si.intern(&issue.file.to_string_lossy()),
        u: si.intern(issue.location.uri.as_str()),
        r: RangeOnDisk::encode(&issue.location.range),
        p: issue.priority.0,
        s: issue.severity,
        rel: issue
            .related
            .iter()
            .map(|rs| RelatedOnDisk {
                f: si.intern(&rs.file.to_string_lossy()),
                u: si.intern(rs.location.uri.as_str()),
                r: RangeOnDisk::encode(&rs.location.range),
            })
            .collect(),
    }
}

/// `None` when any string-table index is out of range — i.e. the file
/// is corrupt, and the caller discards the whole cache.
fn decode_issue(d: IssueOnDisk, strings: &[String]) -> Option<Issue> {
    let related = d
        .rel
        .into_iter()
        .map(|r| {
            Some(RelatedSpan {
                file: PathBuf::from(resolve(strings, r.f)?),
                location: Location {
                    uri: Uri::new(resolve(strings, r.u)?),
                    range: r.r.decode(),
                },
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Issue {
        check_id: resolve(strings, d.c)?.to_string(),
        message: resolve(strings, d.m)?.to_string(),
        file: PathBuf::from(resolve(strings, d.f)?),
        location: Location {
            uri: Uri::new(resolve(strings, d.u)?),
            range: d.r.decode(),
        },
        priority: Priority(d.p),
        severity: d.s,
        related,
    })
}

// ---------------------------------------------------------------------------
// findings.json
// ---------------------------------------------------------------------------

/// All cached results for one `(content_hash, config_hash)` pair — i.e.
/// for one *file*, across every pure check that ran on it. `empty`
/// lists the checks that found nothing (the common case by ~19:1) as
/// bare string-table indices; `found` carries the rest.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FindingsGroup {
    /// content hash, hex
    h: String,
    /// config-hash-table index
    g: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    empty: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    found: Vec<(u32, Vec<IssueOnDisk>)>,
}

#[derive(Debug, Serialize, Deserialize)]
struct FindingsFile {
    format: u32,
    /// Engine version this cache was written by. Read back for an
    /// extra integrity check; the directory layout already isolates
    /// versions, but a stray cache file in the wrong subtree gets
    /// caught here too.
    engine_version: String,
    strings: Vec<String>,
    /// Distinct config hashes, hex. In practice one per file.
    configs: Vec<String>,
    groups: Vec<FindingsGroup>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RunEntry {
    /// input-set hash, hex
    input_set: String,
    /// config hash, hex
    config_hash: String,
    issues: Vec<IssueOnDisk>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RunFile {
    format: u32,
    engine_version: String,
    strings: Vec<String>,
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
        Err(_) => return Ok(0), // malformed (or a v1 file) — discard, rebuild next save
    };
    if file.format != FORMAT_VERSION || file.engine_version != ENGINE_VERSION {
        return Ok(0);
    }

    // Resolve the string table's check ids to `'static` slices once,
    // rather than once per cached entry — v1 did a linear scan of the
    // registry per entry, which is 140k × |registry| comparisons at
    // 5000 files.
    let static_ids: Vec<Option<&'static str>> = file
        .strings
        .iter()
        .map(|s| registered_ids.iter().find(|id| *id == s).copied())
        .collect();
    let configs: Vec<Option<ConfigHash>> = file.configs.iter().map(|c| hex_decode(c)).collect();

    let mut hydrated = 0usize;
    for group in file.groups {
        let Some(content_hash) = hex_decode(&group.h) else {
            continue;
        };
        let Some(Some(config_hash)) = configs.get(group.g as usize).copied() else {
            continue;
        };
        let mut insert = |check_idx: u32, issues: Vec<Issue>| {
            let Some(Some(check_id)) = static_ids.get(check_idx as usize).copied() else {
                return; // check unregistered since the cache was written; drop
            };
            cache.insert(
                FindingsKey {
                    content_hash,
                    config_hash,
                    check_id,
                },
                issues,
            );
            hydrated += 1;
        };
        for check_idx in group.empty {
            insert(check_idx, Vec::new());
        }
        for (check_idx, issues) in group.found {
            let decoded: Option<Vec<Issue>> = issues
                .into_iter()
                .map(|i| decode_issue(i, &file.strings))
                .collect();
            match decoded {
                Some(issues) => insert(check_idx, issues),
                None => continue,
            }
        }
    }
    // Everything we hold now matches the file on disk.
    cache.mark_clean();
    Ok(hydrated)
}

/// Persist `cache`'s contents to disk. Overwrites any existing
/// findings.json under the version subdir. Returns the number of
/// entries written.
pub fn save_findings(cache_dir: &Path, cache: &FindingsCache) -> Result<usize, DiskCacheError> {
    let snapshot = cache.snapshot();
    let count = snapshot.len();

    let mut strings = Interner::default();
    let mut config_idx: HashMap<ConfigHash, u32> = HashMap::new();
    let mut configs: Vec<String> = Vec::new();
    // (content_hash, config index) → group position
    let mut group_idx: HashMap<(ContentHash, u32), usize> = HashMap::new();
    let mut groups: Vec<FindingsGroup> = Vec::new();

    for (key, issues) in snapshot {
        let g = *config_idx.entry(key.config_hash).or_insert_with(|| {
            configs.push(hex_encode(&key.config_hash));
            (configs.len() - 1) as u32
        });
        let pos = *group_idx.entry((key.content_hash, g)).or_insert_with(|| {
            groups.push(FindingsGroup {
                h: hex_encode(&key.content_hash),
                g,
                empty: Vec::new(),
                found: Vec::new(),
            });
            groups.len() - 1
        });
        let check_idx = strings.intern(key.check_id);
        if issues.is_empty() {
            groups[pos].empty.push(check_idx);
        } else {
            let encoded = issues
                .iter()
                .map(|i| encode_issue(i, &mut strings))
                .collect();
            groups[pos].found.push((check_idx, encoded));
        }
    }

    let file = FindingsFile {
        format: FORMAT_VERSION,
        engine_version: ENGINE_VERSION.to_string(),
        strings: strings.list,
        configs,
        groups,
    };
    let bytes = serde_json::to_vec(&file).map_err(DiskCacheError::Serialize)?;
    atomic_write(&findings_path(cache_dir), &bytes)?;
    cache.mark_clean();
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
    if file.format != FORMAT_VERSION || file.engine_version != ENGINE_VERSION {
        return Ok(0);
    }
    let mut hydrated = 0usize;
    for entry in file.entries {
        let (Some(input_set), Some(config_hash)) =
            (hex_decode(&entry.input_set), hex_decode(&entry.config_hash))
        else {
            continue;
        };
        let Some(issues) = entry
            .issues
            .into_iter()
            .map(|i| decode_issue(i, &file.strings))
            .collect::<Option<Vec<Issue>>>()
        else {
            continue;
        };
        cache.insert(
            RunKey {
                input_set,
                config_hash,
            },
            issues,
        );
        hydrated += 1;
    }
    cache.mark_clean();
    Ok(hydrated)
}

/// Persist the most recently inserted run-cache entry. Returns the
/// number of entries written (0 or 1).
///
/// Only the latest entry is kept: a whole-run snapshot is the entire
/// finding list for the project, and an input-set fingerprint the
/// project has already moved past can never be hit again. Persisting
/// every entry made `run.json` grow by a full snapshot per edit.
pub fn save_run(cache_dir: &Path, cache: &RunCache) -> Result<usize, DiskCacheError> {
    let mut strings = Interner::default();
    let entries: Vec<RunEntry> = cache
        .latest()
        .into_iter()
        .map(|(k, v)| RunEntry {
            input_set: hex_encode(&k.input_set),
            config_hash: hex_encode(&k.config_hash),
            issues: v.iter().map(|i| encode_issue(i, &mut strings)).collect(),
        })
        .collect();
    let count = entries.len();
    let file = RunFile {
        format: FORMAT_VERSION,
        engine_version: ENGINE_VERSION.to_string(),
        strings: strings.list,
        entries,
    };
    let bytes = serde_json::to_vec(&file).map_err(DiskCacheError::Serialize)?;
    atomic_write(&run_path(cache_dir), &bytes)?;
    cache.mark_clean();
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
    use cofferdam_core::Span;
    use tempfile::tempdir;

    fn mk_issue(check_id: &str, msg: &str) -> Issue {
        let file = PathBuf::from("a.ts");
        Issue {
            check_id: check_id.to_string(),
            message: msg.to_string(),
            location: Location::from_span(
                &file,
                Span {
                    start_byte: 0,
                    end_byte: 0,
                    line: 1,
                    column: 1,
                },
            ),
            file,
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
    fn hex_round_trips() {
        let h = [
            0u8, 1, 15, 16, 255, 128, 7, 9, 10, 11, 12, 13, 14, 200, 3, 4,
        ]
        .repeat(2);
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&h);
        assert_eq!(hex_decode(&hex_encode(&arr)), Some(arr));
    }

    #[test]
    fn hex_decode_rejects_bad_input() {
        assert_eq!(hex_decode("abc"), None);
        assert_eq!(hex_decode(&"z".repeat(64)), None);
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
        assert_eq!(got[0].file, PathBuf::from("a.ts"));
        assert_eq!(got[0].location.uri.as_str(), "file://a.ts");
        let got = dst.get(&fkey(2, 0, "Design.MaxParameters")).expect("hit");
        assert_eq!(got.len(), 2);
    }

    // The empty `Vec<Issue>` is the *point* of the findings cache — it
    // records "this check found nothing here" so the check doesn't
    // re-run. v2 stores those as bare indices; they must survive.
    #[test]
    fn findings_roundtrip_preserves_empty_results() {
        let dir = tempdir().unwrap();
        let src = FindingsCache::new();
        src.insert(fkey(1, 0, "Warning.TripleEquals"), Vec::new());
        save_findings(dir.path(), &src).unwrap();

        let dst = FindingsCache::new();
        assert_eq!(
            load_findings(dir.path(), &dst, &["Warning.TripleEquals"]).unwrap(),
            1
        );
        let got = dst.get(&fkey(1, 0, "Warning.TripleEquals")).expect("hit");
        assert!(got.is_empty());
    }

    #[test]
    fn findings_roundtrip_preserves_related_spans() {
        let dir = tempdir().unwrap();
        let src = FindingsCache::new();
        let other = PathBuf::from("b.ts");
        let issue = Issue {
            related: vec![RelatedSpan {
                location: Location::from_span(
                    &other,
                    Span {
                        start_byte: 5,
                        end_byte: 9,
                        line: 3,
                        column: 2,
                    },
                ),
                file: other,
            }],
            ..mk_issue("Refactor.DuplicateBlock", "dup")
        };
        src.insert(fkey(1, 0, "Refactor.DuplicateBlock"), vec![issue]);
        save_findings(dir.path(), &src).unwrap();

        let dst = FindingsCache::new();
        load_findings(dir.path(), &dst, &["Refactor.DuplicateBlock"]).unwrap();
        let got = dst.get(&fkey(1, 0, "Refactor.DuplicateBlock")).unwrap();
        assert_eq!(got[0].related.len(), 1);
        assert_eq!(got[0].related[0].file, PathBuf::from("b.ts"));
        assert_eq!(got[0].related[0].location.uri.as_str(), "file://b.ts");
        assert_eq!(
            got[0].related[0].location.range,
            LocationRange::Bytes {
                start: 5,
                end: 9,
                line: 3,
                column: 2,
            }
        );
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
    fn findings_separate_config_hashes_stay_distinct() {
        let dir = tempdir().unwrap();
        let src = FindingsCache::new();
        src.insert(
            fkey(1, 0, "Warning.TripleEquals"),
            vec![mk_issue("W", "cfg0")],
        );
        src.insert(
            fkey(1, 9, "Warning.TripleEquals"),
            vec![mk_issue("W", "cfg9")],
        );
        save_findings(dir.path(), &src).unwrap();

        let dst = FindingsCache::new();
        assert_eq!(
            load_findings(dir.path(), &dst, &["Warning.TripleEquals"]).unwrap(),
            2
        );
        assert_eq!(
            dst.get(&fkey(1, 0, "Warning.TripleEquals")).unwrap()[0].message,
            "cfg0"
        );
        assert_eq!(
            dst.get(&fkey(1, 9, "Warning.TripleEquals")).unwrap()[0].message,
            "cfg9"
        );
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
            format: FORMAT_VERSION,
            engine_version: "0.0.0-from-another-build".to_string(),
            strings: vec!["Warning.TripleEquals".to_string()],
            configs: vec![hex_encode(&[0u8; 32])],
            groups: vec![FindingsGroup {
                h: hex_encode(&[1u8; 32]),
                g: 0,
                empty: vec![0],
                found: Vec::new(),
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
    fn wrong_format_version_is_discarded() {
        let dir = tempdir().unwrap();
        let vdir = version_dir(dir.path());
        fs::create_dir_all(&vdir).unwrap();
        let bogus = FindingsFile {
            format: FORMAT_VERSION + 1,
            engine_version: ENGINE_VERSION.to_string(),
            strings: vec!["Warning.TripleEquals".to_string()],
            configs: vec![hex_encode(&[0u8; 32])],
            groups: vec![FindingsGroup {
                h: hex_encode(&[1u8; 32]),
                g: 0,
                empty: vec![0],
                found: Vec::new(),
            }],
        };
        fs::write(
            vdir.join("findings.json"),
            serde_json::to_vec(&bogus).unwrap(),
        )
        .unwrap();

        let dst = FindingsCache::new();
        assert_eq!(
            load_findings(dir.path(), &dst, &["Warning.TripleEquals"]).unwrap(),
            0
        );
    }

    #[test]
    fn run_cache_roundtrip_preserves_latest_entry() {
        let dir = tempdir().unwrap();
        let src = RunCache::new();
        src.insert(rkey(1, 0), vec![mk_issue("Warning.TripleEquals", "a")]);
        src.insert(rkey(2, 0), vec![mk_issue("X", "b"), mk_issue("Y", "c")]);

        // Only the most recent insert is persisted (CD-185).
        let saved = save_run(dir.path(), &src).expect("save");
        assert_eq!(saved, 1);

        let dst = RunCache::new();
        let hydrated = load_run(dir.path(), &dst).expect("load");
        assert_eq!(hydrated, 1);
        assert_eq!(dst.get(&rkey(2, 0)).unwrap().len(), 2);
        assert!(dst.get(&rkey(1, 0)).is_none());
    }

    #[test]
    fn run_cache_save_is_a_noop_when_nothing_was_inserted() {
        let dir = tempdir().unwrap();
        let src = RunCache::new();
        assert_eq!(save_run(dir.path(), &src).unwrap(), 0);
        let dst = RunCache::new();
        assert_eq!(load_run(dir.path(), &dst).unwrap(), 0);
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

    // Hydrating from disk must leave the cache "clean" so a run that
    // only reads it can skip the rewrite entirely (CD-185).
    #[test]
    fn load_leaves_caches_clean_and_insert_dirties_them() {
        let dir = tempdir().unwrap();
        let src = FindingsCache::new();
        src.insert(fkey(1, 0, "Warning.TripleEquals"), Vec::new());
        save_findings(dir.path(), &src).unwrap();

        let dst = FindingsCache::new();
        load_findings(dir.path(), &dst, &["Warning.TripleEquals"]).unwrap();
        assert!(!dst.is_dirty());
        dst.insert(fkey(2, 0, "Warning.TripleEquals"), Vec::new());
        assert!(dst.is_dirty());

        let rsrc = RunCache::new();
        rsrc.insert(rkey(1, 0), vec![mk_issue("X", "a")]);
        save_run(dir.path(), &rsrc).unwrap();
        let rdst = RunCache::new();
        load_run(dir.path(), &rdst).unwrap();
        assert!(!rdst.is_dirty());
        rdst.insert(rkey(2, 0), Vec::new());
        assert!(rdst.is_dirty());
    }
}
