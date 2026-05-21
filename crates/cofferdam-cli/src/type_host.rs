//! Type-host driver — spawns the Node-side ts-morph worker and exchanges
//! NDJSON RPC requests with it (cd-9hp.2 cp1).
//!
//! Wire shape: `design/type-host-wire.md`.
//!
//! cp1 implements only `ping`. cp2 adds `resolveTypes` and persistent
//! per-tsconfig project handles. cp3 adds the first real type-aware
//! built-in check that consumes this surface.
//!
//! Failure modes:
//!   - Node not installed → spawn error → caller surfaces a clear message.
//!   - ts-morph not resolvable → `ts_morph_unavailable` error in the
//!     response; caller surfaces and skips type-aware checks.
//!   - Worker timeout → child is killed; caller treats as fatal for the
//!     type-host run but keeps non-type-aware findings.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const HOST_SCRIPT: &str = include_str!("../scripts/type-host.mjs");
const HOST_SCRIPT_NAME: &str = "cofferdam-type-host.mjs";

/// Materialise the embedded host script to the OS temp dir on first
/// call, reuse the path on subsequent calls in this process. Same
/// pattern as `plugins.rs::materialise_host_script`.
fn materialise_host_script() -> std::io::Result<PathBuf> {
    static CACHED: OnceLock<std::io::Result<PathBuf>> = OnceLock::new();
    let result = CACHED.get_or_init(|| {
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
    /// `id` must be unique among in-flight requests; cp1 issues one at
    /// a time so any stable string works.
    pub fn request<P: Serialize, R: for<'de> Deserialize<'de>>(
        &mut self,
        id: &str,
        method: &str,
        params: &P,
    ) -> Result<R, TypeHostError> {
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

        // Read one NDJSON line. cp1 is synchronous: one request, one
        // response, in order, so we don't need an out-of-order pairing
        // layer yet. cp2 will switch to a request-id index when batched
        // type queries land.
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
            env.result
                .ok_or_else(|| TypeHostError::BadResponse("ok=true but no result field".into()))
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
                Err(e) => return Err(TypeHostError::Io(format!("wait worker: {e}"))),
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
