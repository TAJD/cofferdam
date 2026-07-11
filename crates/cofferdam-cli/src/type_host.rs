//! Type-host driver — spawns the Node-side ts-morph worker and exchanges
//! NDJSON RPC requests with it (cd-9hp.2).
//!
//! Wire shape: `design/type-host-wire.md`.
//!
//! Methods: `ping` (cp1 diagnostics), `openProject` + `typeAt` (cp2 —
//! the type resolution that backs [`WorkerTypeOracle`]). cp3 ships the
//! first real type-aware built-in check that consumes this surface.
//!
//! Failure modes:
//!   - Node not installed → spawn error → caller surfaces a clear message.
//!   - ts-morph not resolvable → `ts_morph_unavailable` error in the
//!     response; caller surfaces and skips type-aware checks.
//!   - Worker timeout → child is killed; caller treats as fatal for the
//!     type-host run but keeps non-type-aware findings.
//!   - Worker dies mid-run → `type_at` returns `None`, silently
//!     disabling type findings for the rest of the run.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use cofferdam_core::{TypeFacts, TypeOracle};
use serde::{Deserialize, Serialize};

const HOST_SCRIPT: &str = include_str!("../scripts/type-host.mjs");
const HOST_SCRIPT_NAME: &str = concat!("cofferdam-type-host-", env!("CARGO_PKG_VERSION"), ".mjs");

/// CD-81: `type-host.mjs` imports `./type-host-core.mjs` (shared with
/// the plugin host — see `plugins.rs::CORE_SCRIPT`) as a relative ESM
/// specifier; the materialised copy must sit next to it under this
/// exact, unversioned name.
const CORE_SCRIPT: &str = include_str!("../scripts/type-host-core.mjs");
const CORE_SCRIPT_NAME: &str = "type-host-core.mjs";

/// Materialise the embedded host script to the OS temp dir on first
/// call, reuse the path on subsequent calls in this process. Same
/// pattern as `plugins.rs::materialise_host_script`.
fn materialise_host_script() -> std::io::Result<PathBuf> {
    static CACHED: OnceLock<std::io::Result<PathBuf>> = OnceLock::new();
    let result = CACHED.get_or_init(|| {
        // See `plugins.rs::materialise_host_script` — both host scripts
        // share this file and each ensures it's present so either one
        // can be spawned independently of the other.
        let core_path = std::env::temp_dir().join(CORE_SCRIPT_NAME);
        std::fs::write(&core_path, CORE_SCRIPT)?;

        let path = std::env::temp_dir().join(HOST_SCRIPT_NAME);
        std::fs::write(&path, HOST_SCRIPT)?;
        Ok(path)
    });
    match result {
        Ok(p) => Ok(p.clone()),
        Err(e) => Err(std::io::Error::new(e.kind(), e.to_string())),
    }
}

/// Wall-clock budget for one type-host request. Default 60s; override
/// via `COFFERDAM_TYPE_HOST_TIMEOUT_SECS=<n>`. Project init on large
/// repos can hit several seconds; 60 is comfortable headroom.
pub fn host_timeout() -> Duration {
    const DEFAULT: u64 = 60;
    let secs = std::env::var("COFFERDAM_TYPE_HOST_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT);
    Duration::from_secs(secs)
}

/// Number of Node workers in the type-host pool (CD-31). Default is the
/// host's available parallelism (falling back to 1); override via
/// `COFFERDAM_TYPE_HOST_POOL_SIZE=<n>`. Tying the default to core count
/// means the pool roughly matches the eventual rayon thread count
/// without hard-coding a dependency on the engine crate (which owns the
/// actual rayon setup — CD-30).
pub fn pool_size() -> usize {
    std::env::var("COFFERDAM_TYPE_HOST_POOL_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingParams {
    pub load_ts_morph: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_project: Option<OpenProjectParams>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenProjectParams {
    pub tsconfig_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PingResult {
    /// ts-morph package version (best-effort; may be `None` if discovery
    /// failed even though the import succeeded).
    pub ts_morph_version: Option<String>,
    pub timings: PingTimings,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PingTimings {
    /// Wall-clock ms from request receipt to `import("ts-morph")`
    /// resolution. `None` when `load_ts_morph: false`.
    pub ts_morph_import_ms: Option<u64>,
    /// Wall-clock ms to construct the ts-morph Project. `None` when
    /// `open_project` was omitted or the load failed.
    pub project_init_ms: Option<u64>,
    pub total_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenProjectRpcParams<'a> {
    tsconfig_path: &'a str,
}

/// Worker response to `openProject`. Fields mirror the wire contract in
/// `design/type-host-wire.md`; they're deserialised for shape
/// validation (a malformed response fails the open) even though the
/// oracle doesn't read them today. cp4's CI smoke test asserts on
/// `init_ms`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct OpenProjectResult {
    source_file_count: u64,
    init_ms: u64,
    cached: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TypeAtRpcParams<'a> {
    tsconfig_path: &'a str,
    file: &'a str,
    start_byte: u32,
    end_byte: u32,
}

/// Worker-side projection of `TypeFacts`. Mapped into the core type by
/// the oracle. A `null` JSON response (no resolvable type) deserialises
/// to `None` at the call site, not here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TypeFactsWire {
    text: String,
    is_nullable: bool,
    includes_null: bool,
    includes_undefined: bool,
    is_any: bool,
}

impl From<TypeFactsWire> for TypeFacts {
    fn from(w: TypeFactsWire) -> Self {
        TypeFacts {
            text: w.text,
            is_nullable: w.is_nullable,
            includes_null: w.includes_null,
            includes_undefined: w.includes_undefined,
            is_any: w.is_any,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TypeHostError {
    #[error("Node runtime not available: {0}")]
    NodeUnavailable(String),
    #[error("type-host script could not be written: {0}")]
    ScriptMaterialiseFailed(String),
    #[error("type-host I/O error: {0}")]
    Io(String),
    #[error("type-host returned malformed JSON: {0}")]
    BadResponse(String),
    #[error("type-host request timed out after {0:?}")]
    Timeout(Duration),
    #[error("type-host error [{code}]: {message}")]
    HostError { code: String, message: String },
}

/// Send a single `ping` RPC to a freshly-spawned worker, then close it.
/// `project_root` is the directory whose `node_modules` the host script
/// walks up from when resolving `ts-morph` (Node ESM ignores
/// `NODE_PATH`, so resolution is by hand against the project tree).
pub fn ping(project_root: &Path, params: PingParams) -> Result<PingResult, TypeHostError> {
    let mut worker = spawn_worker(project_root)?;
    let result: PingResult = worker.request("ping-1", "ping", &params)?;
    worker.close()?;
    Ok(result)
}

/// One running type-host process. Keeps stdin/stdout open so callers
/// can issue multiple requests against a warm worker. Always drop via
/// `close()` to flush stdin and reap the child; `Drop` falls back to
/// `kill()` if the caller forgets.
pub struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Worker {
    /// Send a request and block until the matching response arrives.
    /// `id` must be unique among in-flight requests; cp2 issues them
    /// sequentially (one outstanding at a time) so any stable string
    /// works. Errors if the response carries `ok: false` OR a null /
    /// absent result — use [`Worker::request_nullable`] for methods
    /// (like `typeAt`) where a null result is a valid "no answer".
    pub fn request<P: Serialize, R: for<'de> Deserialize<'de>>(
        &mut self,
        id: &str,
        method: &str,
        params: &P,
    ) -> Result<R, TypeHostError> {
        self.request_nullable(id, method, params)?
            .ok_or_else(|| TypeHostError::BadResponse("ok=true but no result field".into()))
    }

    /// Like [`Worker::request`] but a successful response with a `null`
    /// (or absent) result deserialises to `Ok(None)` instead of an
    /// error. `ok: false` still maps to [`TypeHostError::HostError`].
    pub fn request_nullable<P: Serialize, R: for<'de> Deserialize<'de>>(
        &mut self,
        id: &str,
        method: &str,
        params: &P,
    ) -> Result<Option<R>, TypeHostError> {
        #[derive(Serialize)]
        struct Envelope<'a, P> {
            id: &'a str,
            method: &'a str,
            params: &'a P,
        }
        let envelope = Envelope { id, method, params };
        let mut line = serde_json::to_string(&envelope)
            .map_err(|e| TypeHostError::Io(format!("serialise request: {e}")))?;
        line.push('\n');

        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| TypeHostError::Io("worker stdin already closed".into()))?;
        stdin
            .write_all(line.as_bytes())
            .map_err(|e| TypeHostError::Io(format!("write request: {e}")))?;
        stdin
            .flush()
            .map_err(|e| TypeHostError::Io(format!("flush request: {e}")))?;

        // Read one NDJSON line. cp2 is synchronous: one request, one
        // response, in order, so we don't need an out-of-order pairing
        // layer yet. Batched type queries (a follow-up) will switch to
        // a request-id index.
        let mut buf = String::new();
        let n = self
            .stdout
            .read_line(&mut buf)
            .map_err(|e| TypeHostError::Io(format!("read response: {e}")))?;
        if n == 0 {
            return Err(TypeHostError::Io(
                "worker stdout closed before response".into(),
            ));
        }

        #[derive(Deserialize)]
        struct ResponseEnvelope<R> {
            #[allow(dead_code)]
            id: String,
            #[serde(default)]
            ok: bool,
            result: Option<R>,
            error: Option<HostErrorBody>,
        }
        #[derive(Deserialize)]
        struct HostErrorBody {
            code: String,
            message: String,
        }
        let env: ResponseEnvelope<R> = serde_json::from_str(buf.trim_end())
            .map_err(|e| TypeHostError::BadResponse(format!("{e}: {}", buf.trim_end())))?;
        if env.ok {
            Ok(env.result)
        } else {
            let err = env.error.unwrap_or(HostErrorBody {
                code: "unknown".into(),
                message: "no error body".into(),
            });
            Err(TypeHostError::HostError {
                code: err.code,
                message: err.message,
            })
        }
    }

    /// Close stdin (signals the worker to exit), wait for the child to
    /// reap. Idempotent.
    pub fn close(mut self) -> Result<(), TypeHostError> {
        drop(self.stdin.take());
        // Bounded wait — the worker should exit promptly on EOF.
        let timeout = host_timeout();
        let start = std::time::Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        return Err(TypeHostError::Timeout(timeout));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return Err(TypeHostError::Io(format!("wait worker: {e}")));
                }
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            // Caller forgot to close(); be defensive — kill so we don't
            // leak a child process.
            drop(self.stdin.take());
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn spawn_worker(project_root: &Path) -> Result<Worker, TypeHostError> {
    let host_script = materialise_host_script()
        .map_err(|e| TypeHostError::ScriptMaterialiseFailed(e.to_string()))?;

    // Node ESM ignores NODE_PATH; the host script resolves `ts-morph`
    // by walking up from this env var looking for
    // `node_modules/ts-morph/package.json`.
    let project_root_abs =
        std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());

    let mut child = Command::new("node")
        .arg(&host_script)
        .env("COFFERDAM_TYPE_HOST_PROJECT_ROOT", &project_root_abs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| TypeHostError::NodeUnavailable(e.to_string()))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| TypeHostError::Io("no worker stdin".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| TypeHostError::Io("no worker stdout".into()))?;
    Ok(Worker {
        child,
        stdin: Some(stdin),
        stdout: BufReader::new(stdout),
    })
}

/// Type oracle backed by a pool of live Node ts-morph workers (CD-31,
/// following cd-9hp.2 cp2's single-worker design).
///
/// Each worker is a separate Node process with its own ts-morph
/// `Project` handle, held behind its own `Mutex` so the `&self`
/// [`TypeOracle`] API can drive the inherently `&mut` request channel.
/// `type_at` dispatches to workers round-robin via an atomic counter —
/// concurrent callers (once the engine's per-file loop is parallelized,
/// CD-30) fan out across the pool instead of serialising onto one
/// worker. A single-worker pool (the pre-CD-31 default on a 1-core
/// host, or `COFFERDAM_TYPE_HOST_POOL_SIZE=1`) behaves identically to
/// the old design.
///
/// `type_at` swallows worker/transport errors into `None`: a check can't
/// do anything useful with a transport failure mid-walk, and the right
/// behaviour is "emit no finding" rather than crash the run. A dead
/// worker therefore silently disables type findings from that worker's
/// share of the run — cp4's CI smoke test guards against that
/// regressing unnoticed.
pub struct WorkerTypeOracle {
    workers: Vec<Mutex<Worker>>,
    tsconfig_path: String,
    next_id: AtomicU64,
    next_worker: AtomicUsize,
}

impl WorkerTypeOracle {
    fn next_id(&self) -> String {
        format!("ta-{}", self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Pick the next worker round-robin. Panics only if the pool is
    /// empty, which [`build_type_oracle`] never constructs.
    fn next_worker(&self) -> &Mutex<Worker> {
        let idx = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        &self.workers[idx]
    }
}

impl TypeOracle for WorkerTypeOracle {
    fn type_at(&self, file: &Path, start_byte: u32, end_byte: u32) -> Option<TypeFacts> {
        let file_fwd = file.to_string_lossy().replace('\\', "/");
        let params = TypeAtRpcParams {
            tsconfig_path: &self.tsconfig_path,
            file: &file_fwd,
            start_byte,
            end_byte,
        };
        let id = self.next_id();
        let mut worker = self.next_worker().lock().ok()?;
        let wire: Option<TypeFactsWire> = worker.request_nullable(&id, "typeAt", &params).ok()?;
        wire.map(Into::into)
    }
}

/// Spawn a pool of type-host workers (CD-31; size from [`pool_size`]),
/// open the project's tsconfig on each (paying the init cost up front so
/// it isn't a mysterious mid-run stall), and return a ready-to-use
/// [`WorkerTypeOracle`].
///
/// `project_root` is the directory whose `node_modules` resolves
/// `ts-morph`; `tsconfig_path` is the tsconfig the ts-morph `Project` is
/// built from. The CLI calls this before constructing the engine when a
/// type-aware check is registered and the user hasn't disabled
/// `[engine] type_aware`.
///
/// Workers are opened concurrently (one thread per worker) so the pool's
/// wall-clock cost is close to a single worker's project-init time
/// rather than N times it. On any failure (Node missing, ts-morph not
/// installed, tsconfig invalid) already-spawned workers are torn down
/// and the error is returned so the caller can surface a single clear
/// diagnostic and fall back to running without type-aware checks.
pub fn build_type_oracle(
    project_root: &Path,
    tsconfig_path: &Path,
) -> Result<WorkerTypeOracle, TypeHostError> {
    build_type_oracle_with_pool_size(project_root, tsconfig_path, pool_size())
}

/// Same as [`build_type_oracle`] but with an explicit pool size, bypassing
/// the `COFFERDAM_TYPE_HOST_POOL_SIZE` env lookup. Exists so tests can pin
/// a pool size deterministically without mutating process-global env
/// state (the workspace forbids `unsafe`, which `std::env::set_var`
/// requires since Rust 2024).
fn build_type_oracle_with_pool_size(
    project_root: &Path,
    tsconfig_path: &Path,
    size: usize,
) -> Result<WorkerTypeOracle, TypeHostError> {
    let tsconfig = tsconfig_path.to_string_lossy().replace('\\', "/");

    let results: Vec<Result<Worker, TypeHostError>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..size)
            .map(|_| {
                let tsconfig = tsconfig.as_str();
                scope.spawn(move || -> Result<Worker, TypeHostError> {
                    let mut worker = spawn_worker(project_root)?;
                    let _open: OpenProjectResult = worker.request(
                        "open-1",
                        "openProject",
                        &OpenProjectRpcParams {
                            tsconfig_path: tsconfig,
                        },
                    )?;
                    Ok(worker)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join().unwrap_or_else(|_| {
                    Err(TypeHostError::Io("worker init thread panicked".into()))
                })
            })
            .collect()
    });

    let mut workers = Vec::with_capacity(size);
    let mut first_err = None;
    for result in results {
        match result {
            Ok(w) => workers.push(w),
            Err(e) if first_err.is_none() => first_err = Some(e),
            Err(_) => {}
        }
    }

    if let Some(err) = first_err {
        for w in workers {
            let _ = w.close();
        }
        return Err(err);
    }

    Ok(WorkerTypeOracle {
        workers: workers.into_iter().map(Mutex::new).collect(),
        tsconfig_path: tsconfig,
        next_id: AtomicU64::new(0),
        next_worker: AtomicUsize::new(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cofferdam_core::TypeOracle;

    /// Real worker end-to-end test (cd-9hp.2 cp2). Gated on a ts-morph
    /// install: set `COFFERDAM_TYPE_HOST_TS_MORPH_ROOT` to a directory
    /// whose `node_modules` contains `ts-morph` (e.g. the scratch dir
    /// `target/type-host-scratch` created during development, or any
    /// project with ts-morph installed). Skips silently when unset so
    /// CI without ts-morph stays green — cp4 wires a proper CI smoke
    /// test that always runs against a fixed fixture.
    ///
    /// Proves: a worker-backed oracle resolves the declared type of a
    /// `string | null` binding and reports `includes_null = true`, and a
    /// plain `string` binding reports `includes_null = false`.
    #[test]
    fn worker_oracle_resolves_nullable_type() {
        let Ok(ts_morph_root) = std::env::var("COFFERDAM_TYPE_HOST_TS_MORPH_ROOT") else {
            return; // not configured — skip
        };
        let ts_morph_root = PathBuf::from(ts_morph_root);

        // Self-contained project: tsconfig + one source file in a temp
        // dir. ts-morph itself is resolved from `ts_morph_root`.
        let project = tempfile::tempdir().expect("tempdir");
        let sample = project.path().join("sample.ts");
        // Byte layout matters: the test queries the `x` / `s` identifier
        // by byte offset. Keep this ASCII so byte == UTF-16 position.
        std::fs::write(
            &sample,
            "const x: string | null = null;\nconst s: string = \"hi\";\nexport { x, s };\n",
        )
        .expect("write sample");
        let tsconfig = project.path().join("tsconfig.json");
        std::fs::write(
            &tsconfig,
            r#"{ "compilerOptions": { "strict": true, "noEmit": true }, "include": ["sample.ts"] }"#,
        )
        .expect("write tsconfig");

        let oracle = match build_type_oracle(&ts_morph_root, &tsconfig) {
            Ok(o) => o,
            Err(e) => panic!(
                "build_type_oracle failed (is ts-morph installed at {}?): {e}",
                ts_morph_root.display()
            ),
        };

        // `const x: string | null` — `x` is the 7th byte (index 6).
        let facts_x = oracle
            .type_at(&sample, 6, 7)
            .expect("type_at should resolve x");
        assert!(
            facts_x.includes_null,
            "x: string | null should report includes_null; got {facts_x:?}"
        );
        assert!(facts_x.is_nullable, "x should be nullable; got {facts_x:?}");

        // `const s: string` is on line 2. Line 1 is 31 bytes
        // (30 chars + '\n'); `const ` is 6 more → `s` at byte 37.
        let facts_s = oracle
            .type_at(&sample, 37, 38)
            .expect("type_at should resolve s");
        assert!(
            !facts_s.includes_null,
            "s: string should NOT report includes_null; got {facts_s:?}"
        );
    }

    /// CD-31: a pool of >1 workers services concurrent `type_at` callers
    /// without serialising through a single mutex. Same gating as
    /// `worker_oracle_resolves_nullable_type`. Pins a 3-worker pool via
    /// `build_type_oracle_with_pool_size` (not the env var, to avoid
    /// `unsafe` env mutation), fires concurrent requests from multiple
    /// threads against the same oracle, and asserts every response is
    /// correct — the round-robin dispatch must not scramble results
    /// across workers.
    #[test]
    fn worker_pool_serves_concurrent_requests() {
        let Ok(ts_morph_root) = std::env::var("COFFERDAM_TYPE_HOST_TS_MORPH_ROOT") else {
            return; // not configured — skip
        };
        let ts_morph_root = PathBuf::from(ts_morph_root);

        let project = tempfile::tempdir().expect("tempdir");
        let sample = project.path().join("sample.ts");
        std::fs::write(
            &sample,
            "const x: string | null = null;\nconst s: string = \"hi\";\nexport { x, s };\n",
        )
        .expect("write sample");
        let tsconfig = project.path().join("tsconfig.json");
        std::fs::write(
            &tsconfig,
            r#"{ "compilerOptions": { "strict": true, "noEmit": true }, "include": ["sample.ts"] }"#,
        )
        .expect("write tsconfig");

        let oracle = match build_type_oracle_with_pool_size(&ts_morph_root, &tsconfig, 3) {
            Ok(o) => o,
            Err(e) => panic!(
                "build_type_oracle failed (is ts-morph installed at {}?): {e}",
                ts_morph_root.display()
            ),
        };
        assert_eq!(oracle.workers.len(), 3, "pool should have 3 workers");

        std::thread::scope(|scope| {
            for _ in 0..8 {
                let oracle = &oracle;
                let sample = &sample;
                scope.spawn(move || {
                    let facts_x = oracle
                        .type_at(sample, 6, 7)
                        .expect("type_at should resolve x");
                    assert!(facts_x.includes_null, "got {facts_x:?}");
                    let facts_s = oracle
                        .type_at(sample, 37, 38)
                        .expect("type_at should resolve s");
                    assert!(!facts_s.includes_null, "got {facts_s:?}");
                });
            }
        });
    }
}
