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

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use cofferdam_core::{LiteralFacts, TypeFacts, TypeOracle, UnionFacts};
use serde::{Deserialize, Serialize};

const HOST_SCRIPT: &str = include_str!("../scripts/type-host.mjs");
const HOST_SCRIPT_NAME: &str = concat!("cofferdam-type-host-", env!("CARGO_PKG_VERSION"), ".mjs");

/// CD-81/CD-89: `type-host.mjs` imports `./type-host-core.mjs` (shared
/// with the plugin host — see `plugins.rs::CORE_SCRIPT`) as a relative
/// ESM specifier; the materialised copy must sit next to it under this
/// exact name, inside the version-scoped directory `scripts_dir` builds
/// (see `plugins.rs::scripts_dir` for why: an unversioned, flat temp
/// path let two concurrently-running cofferdam versions race to
/// overwrite each other's copy of this file).
const CORE_SCRIPT: &str = include_str!("../scripts/type-host-core.mjs");
const CORE_SCRIPT_NAME: &str = "type-host-core.mjs";

/// Mirrors `plugins.rs::scripts_dir` (including the per-user component,
/// CD-89) — each materialiser ensures the same version- and user-scoped
/// directory independently rather than sharing state across the two
/// call sites.
fn scripts_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "cofferdam-scripts-{}-{}",
        env!("CARGO_PKG_VERSION"),
        current_user_tag()
    ))
}

/// Mirrors `plugins.rs::current_user_tag`.
fn current_user_tag() -> String {
    let raw = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let sanitized: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// Materialise the embedded host script to the OS temp dir on first
/// call, reuse the path on subsequent calls in this process. Same
/// pattern as `plugins.rs::materialise_host_script`.
fn materialise_host_script() -> std::io::Result<PathBuf> {
    static CACHED: OnceLock<std::io::Result<PathBuf>> = OnceLock::new();
    let result = CACHED.get_or_init(|| {
        let dir = scripts_dir();
        std::fs::create_dir_all(&dir)?;

        // See `plugins.rs::materialise_host_script` — both host scripts
        // share this file and each ensures it's present so either one
        // can be spawned independently of the other.
        //
        // CD-266: `write_atomic`, not `fs::write` directly. `scripts_dir`
        // is scoped by version and user (CD-89) but not by process — two
        // same-version `cofferdam` invocations racing (a CI matrix
        // sharing a runner, a watch-mode rebuild overlapping a manual
        // run) could have one process's non-atomic truncate-then-write
        // caught mid-write by another process's `node` opening the same
        // path, producing a `SyntaxError` from a torn `.mjs` file that
        // looks exactly like the class of flake CD-238 was filed for.
        let core_path = dir.join(CORE_SCRIPT_NAME);
        write_atomic(&core_path, CORE_SCRIPT)?;

        let path = dir.join(HOST_SCRIPT_NAME);
        write_atomic(&path, HOST_SCRIPT)?;
        Ok(path)
    });
    match result {
        Ok(p) => Ok(p.clone()),
        Err(e) => Err(std::io::Error::new(e.kind(), e.to_string())),
    }
}

/// Write `contents` to `path` without a window where a concurrent
/// reader could observe a truncated/partial file — write to a
/// process-unique sibling path, then `rename` into place. `rename` is
/// atomic on both POSIX and Windows for a same-directory replace.
/// Mirrors `plugins.rs::write_atomic`.
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let tmp_path = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp_path, contents)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
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

/// CD-282: each worker opens a full ts-morph `Project` (parses the whole
/// TS program), so the default pool size is capped here rather than left
/// to track `available_parallelism()` unbounded — a 64-core CI runner
/// would otherwise spawn 64 concurrent full-project parses, a plausible
/// OOM. The per-file engine loop is still single-threaded (CD-30 open),
/// so today every worker past the first buys no throughput anyway; 4 is
/// enough to keep a few requests in flight without the unbounded blast
/// radius. `COFFERDAM_TYPE_HOST_POOL_SIZE` still overrides this
/// unconditionally for anyone who has measured their own workload.
const DEFAULT_POOL_SIZE_CAP: usize = 4;

/// Number of Node workers in the type-host pool (CD-31). Default is the
/// host's available parallelism, capped at [`DEFAULT_POOL_SIZE_CAP`]
/// (falling back to 1 if parallelism can't be determined); override via
/// `COFFERDAM_TYPE_HOST_POOL_SIZE=<n>`, which is not capped.
pub fn pool_size() -> usize {
    std::env::var("COFFERDAM_TYPE_HOST_POOL_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| {
            default_pool_size(
                std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1),
            )
        })
}

/// Pure capping logic behind [`pool_size`]'s uncapped-env-var-absent
/// branch, split out so it's unit-testable without mutating process-wide
/// env state (forbidden by the workspace's no-`unsafe` lint, which
/// `std::env::set_var` requires since Rust 2024).
fn default_pool_size(available_parallelism: usize) -> usize {
    available_parallelism.min(DEFAULT_POOL_SIZE_CAP)
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResolveLiteralRpcParams<'a> {
    tsconfig_path: &'a str,
    file: &'a str,
    start_byte: u32,
    end_byte: u32,
}

/// Worker-side projection of `LiteralFacts` (CD-82). Mapped into the
/// core type by the oracle. A `null` JSON response (nothing resolvable)
/// deserialises to `None` at the call site, not here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiteralFactsWire {
    literal_string: Option<String>,
    is_nullable: bool,
    is_empty_object: bool,
}

impl From<LiteralFactsWire> for LiteralFacts {
    fn from(w: LiteralFactsWire) -> Self {
        LiteralFacts {
            literal_string: w.literal_string,
            is_nullable: w.is_nullable,
            is_empty_object: w.is_empty_object,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UnionMembersRpcParams<'a> {
    tsconfig_path: &'a str,
    file: &'a str,
    start_byte: u32,
    end_byte: u32,
}

/// Worker-side projection of `UnionFacts` (CD-118). Mapped into the core
/// type by the oracle. A `null` JSON response (not a literal-only union)
/// deserialises to `None` at the call site, not here.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnionFactsWire {
    members: Vec<String>,
}

impl From<UnionFactsWire> for UnionFacts {
    fn from(w: UnionFactsWire) -> Self {
        UnionFacts { members: w.members }
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

/// Cap on retained stderr text (CD-245): a worker that dumps a large
/// diagnostic (deprecation spam, a `console.error` in a big project)
/// shouldn't grow this unboundedly for the lifetime of the worker —
/// only the tail is useful for the CD-238 error message anyway.
const STDERR_BUF_CAP: usize = 64 * 1024;

/// CD-245: drain `stderr` on a background thread into a bounded shared
/// buffer for the worker's whole lifetime, rather than leaving the pipe
/// unread. An unread `Stdio::piped()` stderr fills its OS pipe buffer
/// (typically 8-64 KB) once the worker writes past it — Node
/// deprecation warnings, a ts-morph diagnostic dump, a stray
/// `console.error` — at which point the worker blocks in `write(2)`
/// and never produces its NDJSON response, hanging `Worker::request`
/// (which has no read timeout). Draining continuously keeps the pipe
/// empty so the worker can't wedge itself this way, and gives the
/// CD-238 stdout-EOF error path a non-blocking source of diagnostics
/// (previously it did a blocking `read_to_string`, which could hang
/// forever if the child was still alive — CD-244).
///
/// CD-253: buffered as raw bytes rather than `String`. A worker's
/// stderr isn't guaranteed to be valid UTF-8 or to break cleanly on
/// character boundaries at the `STDERR_BUF_CAP` trim point — reading
/// via `read_line`/`String` panicked `replace_range` on a multi-byte
/// char straddling the cut, and `read_line` itself errors (treated as
/// EOF, permanently stopping the drain) on invalid UTF-8. `read_until`
/// has no such encoding concerns; decoding is deferred to
/// [`Worker::request_nullable`]'s error path via `from_utf8_lossy`.
///
/// CD-254: also returns a `Receiver` that fires once when the drain
/// loop exits (i.e. the child's stderr pipe has closed and everything
/// written to it has been drained into the buffer). Nothing previously
/// synchronised with this thread, so the stdout-EOF error path could
/// snapshot the buffer before the last few already-written lines had
/// been read out of the OS pipe — for the common "prints one stack
/// trace and exits" failure shape, that raced the CD-238 diagnostic
/// message back down to no stderr tail at all, nondeterministically.
/// [`Worker::request_nullable`]'s EOF branch does a short bounded
/// `recv_timeout` on this before snapshotting.
///
/// CD-259: reads fixed-size chunks via [`Read::read`] rather than
/// line-framing via `read_until(b'\n', ...)`. `read_until` grows its
/// destination `Vec` without limit until it finds the delimiter —
/// `STDERR_BUF_CAP` was only ever applied *after* a full line had
/// accumulated, so newline-free output (a `\r`-only progress bar, a
/// `console.error` of one huge JSON blob) grew unboundedly and could
/// OOM the whole CLI in a code path whose entire purpose is defensive
/// robustness. A fixed chunk size bounds the transient allocation
/// regardless of whether the input ever contains a newline.
///
/// CD-260: the loop no longer treats every `Err` or a poisoned lock as
/// a reason to stop draining for the worker's remaining lifetime — both
/// silently reopened the CD-245 deadlock this thread exists to prevent.
/// See [`drain_into`].
fn spawn_stderr_drain(stderr: ChildStderr) -> (Arc<Mutex<Vec<u8>>>, mpsc::Receiver<()>) {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let buf_writer = Arc::clone(&buf);
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        drain_into(stderr, &buf_writer);
        let _ = done_tx.send(());
    });
    (buf, done_rx)
}

/// Read `reader` to EOF in fixed-size chunks, appending each into `buf`
/// via [`append_capped`]. Split out from [`spawn_stderr_drain`] so the
/// CD-259 bounded-memory behaviour is testable against a plain in-memory
/// reader without a real child process.
const DRAIN_CHUNK_SIZE: usize = 8 * 1024;

/// CD-260: a genuine EOF (`Ok(0)`) is the only intended way out of the
/// loop. A poisoned lock is ignored rather than treated as fatal — this
/// buffer holds a lossy diagnostic tail with no invariant a panicking
/// holder (`Worker::request_nullable`'s CD-238 error path) could
/// corrupt, so honouring the poison and giving up on draining forever
/// is strictly worse than reading through it.
///
/// CD-270: this arm is for errors that *might* be transient on some
/// `R: Read` — `WouldBlock`/`TimedOut` — not "any error that isn't
/// `Interrupted`". The real reader this runs against (`ChildStderr`, a
/// blocking OS pipe) can't actually produce those; what it produces on
/// failure is permanent errors (`BrokenPipe`, `EIO`, ...), which now
/// break immediately instead of paying a pointless sleep first. That
/// sleep used to delay the `done_tx` signal
/// `Worker::request_nullable`'s CD-254 join waits on — harmless while
/// the retry budget is small relative to that timeout, but a latent
/// coupling between two unrelated constants. The bound exists for a
/// hypothetical future non-`ChildStderr` reader (this helper is a
/// generic `R: Read`, tested directly against synthetic readers) where
/// `WouldBlock` is real.
const MAX_TRANSIENT_READ_RETRIES: u32 = 8;

/// CD-270: `Interrupted` (EINTR) is retried unconditionally — matching
/// `std`'s own `read_to_end` — but must still be bounded. An unbounded
/// retry here is exactly the "spin forever on a dead reader" risk this
/// function exists to avoid, just relocated to a different error kind:
/// a signal handler installed without `SA_RESTART` firing repeatedly
/// would otherwise busy-spin this thread at 100% CPU forever and never
/// send `done_tx`. Large (not `MAX_TRANSIENT_READ_RETRIES`-sized)
/// because genuine EINTR storms under real interrupt load can
/// legitimately recur far more than 8 times; still finite.
const MAX_INTERRUPTED_RETRIES: u32 = 10_000;

fn drain_into<R: Read>(mut reader: R, buf: &Mutex<Vec<u8>>) {
    let mut chunk = [0u8; DRAIN_CHUNK_SIZE];
    let mut transient_retries_left = MAX_TRANSIENT_READ_RETRIES;
    let mut interrupted_retries_left = MAX_INTERRUPTED_RETRIES;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                transient_retries_left = MAX_TRANSIENT_READ_RETRIES;
                interrupted_retries_left = MAX_INTERRUPTED_RETRIES;
                let mut bytes = buf.lock().unwrap_or_else(PoisonError::into_inner);
                append_capped(&mut bytes, &chunk[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                if interrupted_retries_left == 0 {
                    break;
                }
                interrupted_retries_left -= 1;
                // No sleep — EINTR means "the syscall didn't run at
                // all, try again immediately", not "wait for something
                // to change".
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) && transient_retries_left > 0 =>
            {
                transient_retries_left -= 1;
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break,
        }
    }
}

/// Bounded wait for the stderr drain thread to finish, used only on the
/// stdout-EOF path (CD-254) — the child has just exited (or is about
/// to), so its stderr pipe closes promptly; this exists to close the
/// race, not to wait indefinitely. If the thread hasn't finished within
/// the bound, snapshot whatever is buffered so far rather than block —
/// CD-244's "never block indefinitely" property still holds.
const STDERR_DRAIN_JOIN_TIMEOUT: Duration = Duration::from_millis(200);

/// CD-261: bound on how many stray (non-matching-id) stdout lines
/// `Worker::request_nullable` will skip while resyncing before giving
/// up and returning an error. Bounded so a worker stuck emitting
/// endless non-RPC stdout output can't hang the caller reading forever.
const MAX_RESPONSE_RESYNC_ATTEMPTS: u32 = 32;

/// Append `chunk` to `buf`, trimming from the front once `buf` exceeds
/// `STDERR_BUF_CAP`. Byte-indexed, not char-boundary-aware — `buf`
/// holds raw stderr bytes of unknown/possibly-invalid encoding, so
/// there is no "boundary" to respect (see [`spawn_stderr_drain`]'s
/// CD-253 doc). Split out from the drain loop so it's unit-testable
/// without a real child process.
fn append_capped(buf: &mut Vec<u8>, chunk: &[u8]) {
    buf.extend_from_slice(chunk);
    if buf.len() > STDERR_BUF_CAP {
        let excess = buf.len() - STDERR_BUF_CAP;
        buf.drain(0..excess);
    }
}

/// One running type-host process. Keeps stdin/stdout open so callers
/// can issue multiple requests against a warm worker. Always drop via
/// `close()` to flush stdin and reap the child; `Drop` falls back to
/// `kill()` if the caller forgets.
pub struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    /// Tail of the worker's stderr output (raw bytes — CD-253), kept
    /// current by a background drain thread spawned in [`spawn_worker`]
    /// — see [`spawn_stderr_drain`].
    stderr_buf: Arc<Mutex<Vec<u8>>>,
    /// Fires once when the drain thread exits (CD-254) — see
    /// [`spawn_stderr_drain`] and [`STDERR_DRAIN_JOIN_TIMEOUT`].
    stderr_drain_done: mpsc::Receiver<()>,
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

        // CD-281: the id is checked against a *raw* envelope (result/error
        // left as `serde_json::Value`) before ever attempting to deserialise
        // into `R`/`HostErrorBody`. Deserialising those typed fields first
        // meant a response that matched our id but didn't parse cleanly
        // (e.g. an `ok:false` whose `error` body was missing `code`/
        // `message`) fell into the "stray/malformed line" arm below and was
        // discarded — silently consuming *our own* response and looping
        // back into a `read_line` for a reply that will never come, since
        // the worker already sent it.
        #[derive(Deserialize)]
        struct RawResponseEnvelope {
            id: String,
            #[serde(default)]
            ok: bool,
            result: Option<serde_json::Value>,
            error: Option<serde_json::Value>,
        }
        #[derive(Deserialize)]
        struct ResponseEnvelope<R> {
            ok: bool,
            result: Option<R>,
            error: Option<HostErrorBody>,
        }
        #[derive(Deserialize)]
        struct HostErrorBody {
            code: String,
            message: String,
        }

        // CD-261: cp2 is synchronous (one request, one response, in
        // order) but only for a *well-behaved* worker — a stray line on
        // stdout (a `console.log` reached through user config, a Node
        // loader/deprecation notice, ...) desyncs purely-positional
        // pairing permanently: every later response answers the wrong
        // request, silently attributing type facts to the wrong span
        // for the worker's remaining lifetime. `next_id()` already
        // generates unique ids; check them. On a mismatch, skip the
        // stray line and keep reading (bounded) rather than trust it —
        // resync, don't resign.
        let mut resync_attempts = 0u32;
        let env: ResponseEnvelope<R> = loop {
            // Read one NDJSON line.
            let mut buf = String::new();
            let n = self
                .stdout
                .read_line(&mut buf)
                .map_err(|e| TypeHostError::Io(format!("read response: {e}")))?;
            if n == 0 {
                // CD-238: the worker's stdout closed without a response —
                // almost always because the child process already exited.
                // Surface its exit status and any stderr output (a Node
                // stack trace, an ESM resolution error, ...) rather than
                // the bare "closed before response", which gave zero
                // diagnostic signal for the flake this was originally
                // reported as (CI-only "worker stdout closed before
                // response" with no visible cause).
                let status = self
                    .child
                    .try_wait()
                    .ok()
                    .flatten()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "still running".to_string());
                // CD-254: give the drain thread a short bounded window to
                // finish reading whatever the child already wrote before we
                // snapshot — otherwise the last few lines still sitting in
                // the OS pipe when we observed stdout EOF may not have been
                // drained yet, and the diagnostic below nondeterministically
                // comes back empty for the common "short trace, then exit"
                // failure shape.
                let _ = self
                    .stderr_drain_done
                    .recv_timeout(STDERR_DRAIN_JOIN_TIMEOUT);
                // CD-244: read from the drain thread's buffer rather than
                // the pipe directly — a direct `read_to_string` here blocks
                // until the child closes stderr, which (per the `status`
                // computed above) may never happen if the child is still
                // running.
                //
                // CD-260: a poisoned lock (this call is itself the only
                // realistic panic site touching this mutex) is read through
                // rather than treated as "no diagnostics" — see
                // `drain_into`'s CD-260 doc for why honouring poison here
                // would be strictly worse than ignoring it.
                let bytes = self
                    .stderr_buf
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let stderr_text = String::from_utf8_lossy(&bytes).trim().to_string();
                let stderr_text = stderr_text.as_str();
                return Err(TypeHostError::Io(format!(
                    "worker stdout closed before response (exit status: {status}){}",
                    if stderr_text.is_empty() {
                        String::new()
                    } else {
                        format!("; stderr: {stderr_text}")
                    }
                )));
            }

            // A stray line — one that isn't valid JSON at all, or is
            // valid JSON but answers a different request id — is
            // discarded and reading continues, up to a bounded number
            // of attempts. Both shapes count against the same budget:
            // either one is evidence stdout is (or was, transiently)
            // carrying something other than this request's response.
            let raw: RawResponseEnvelope = match serde_json::from_str(buf.trim_end()) {
                Ok(raw) => raw,
                Err(e) => {
                    resync_attempts += 1;
                    if resync_attempts > MAX_RESPONSE_RESYNC_ATTEMPTS {
                        return Err(TypeHostError::BadResponse(format!(
                            "{e}: {} (gave up resyncing after {resync_attempts} \
                             stray/malformed line(s))",
                            buf.trim_end()
                        )));
                    }
                    continue;
                }
            };
            if raw.id != id {
                resync_attempts += 1;
                if resync_attempts > MAX_RESPONSE_RESYNC_ATTEMPTS {
                    return Err(TypeHostError::BadResponse(format!(
                        "response id mismatch: expected {id:?}, got {:?} \
                         (gave up resyncing after {resync_attempts} stray line(s) — \
                         worker stdout may be permanently desynced)",
                        raw.id
                    )));
                }
                // Stray line that doesn't answer this request — discard it
                // and keep reading for the real response.
                continue;
            }
            // The id matches — this is our response. From here a parse
            // failure is a real protocol error, not a stray line, so it
            // must surface as `BadResponse` rather than being silently
            // skipped.
            let result = raw
                .result
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| TypeHostError::BadResponse(format!("response {id:?} result: {e}")))?;
            let error = raw
                .error
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| TypeHostError::BadResponse(format!("response {id:?} error: {e}")))?;
            break ResponseEnvelope {
                ok: raw.ok,
                result,
                error,
            };
        };
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

/// Mirrors `type-host-core.mjs`'s `findTsMorphPackageJson` walk: starting
/// at `start`, check `node_modules/ts-morph/package.json`, then walk up
/// to each parent directory in turn. Existence-only — doesn't validate
/// the package.json is well-formed or that `main`/`module` resolves,
/// since this is purely a fast-path gate to avoid spawning Node workers
/// that would immediately fail the same way the JS-side walk already
/// handles.
fn ts_morph_resolvable(start: &Path) -> bool {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join("node_modules")
            .join("ts-morph")
            .join("package.json")
            .is_file()
        {
            return true;
        }
        dir = d.parent();
    }
    false
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
    let (stderr_buf, stderr_drain_done) = match child.stderr.take() {
        Some(stderr) => spawn_stderr_drain(stderr),
        None => {
            // No stderr pipe to drain — synthesize an already-closed
            // channel so `recv_timeout` in the EOF path returns
            // immediately (`Disconnected`) instead of waiting out the
            // full timeout for a thread that was never spawned.
            let (tx, rx) = mpsc::channel();
            drop(tx);
            (Arc::new(Mutex::new(Vec::new())), rx)
        }
    };
    Ok(Worker {
        child,
        stdin: Some(stdin),
        stdout: BufReader::new(stdout),
        stderr_buf,
        stderr_drain_done,
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

    fn resolve_literal(&self, file: &Path, start_byte: u32, end_byte: u32) -> Option<LiteralFacts> {
        let file_fwd = file.to_string_lossy().replace('\\', "/");
        let params = ResolveLiteralRpcParams {
            tsconfig_path: &self.tsconfig_path,
            file: &file_fwd,
            start_byte,
            end_byte,
        };
        let id = self.next_id();
        let mut worker = self.next_worker().lock().ok()?;
        let wire: Option<LiteralFactsWire> = worker
            .request_nullable(&id, "resolveLiteral", &params)
            .ok()?;
        wire.map(Into::into)
    }

    fn union_members_at(&self, file: &Path, start_byte: u32, end_byte: u32) -> Option<UnionFacts> {
        let file_fwd = file.to_string_lossy().replace('\\', "/");
        let params = UnionMembersRpcParams {
            tsconfig_path: &self.tsconfig_path,
            file: &file_fwd,
            start_byte,
            end_byte,
        };
        let id = self.next_id();
        let mut worker = self.next_worker().lock().ok()?;
        let wire: Option<UnionFactsWire> =
            worker.request_nullable(&id, "unionMembers", &params).ok()?;
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
    // CD-172: `type-host-core.mjs`'s own `loadTsMorph` walks up from
    // `projectRoot` looking for `node_modules/ts-morph/package.json`
    // and fails with this exact `ts_morph_unavailable` error/code when
    // it isn't found — but only after a Node process has already been
    // spawned per pool worker (`pool_size()` of them, concurrently) to
    // discover that. Mirroring the same walk here in Rust turns the
    // extremely common "Node/ts-morph not installed" case (any
    // Node-less CI runner, any repo that hasn't opted into type-aware
    // checks) into a handful of `Path::exists` stats instead of N
    // concurrent `node` spawns — measured ~300ms of fixed overhead per
    // `cofferdam check` invocation on a real repo (CD-172). A
    // false-negative here (this check says "not found" but the JS
    // walk would have succeeded) just falls back to the exact same
    // error the JS path would have produced anyway, so this can only
    // remove overhead, never change behavior for the case where
    // ts-morph genuinely resolves.
    if !ts_morph_resolvable(project_root) {
        return Err(TypeHostError::HostError {
            code: "ts_morph_unavailable".to_string(),
            message: format!(
                "ts-morph not found in any node_modules walking up from {}. \
                 Install with: npm install --save-dev ts-morph",
                project_root.display()
            ),
        });
    }

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

    fn node_present() -> bool {
        Command::new("node")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// CD-253: a multi-byte UTF-8 character straddling the
    /// `STDERR_BUF_CAP` cut point must not panic — the old
    /// `String::replace_range` implementation panicked here because
    /// its trim index wasn't guaranteed to land on a char boundary.
    #[test]
    fn append_capped_does_not_panic_when_the_cap_lands_mid_char() {
        // A 3-byte UTF-8 char (e.g. '€') repeated so the cap boundary
        // is very likely to fall inside one of its bytes for at least
        // some cap/length combination; assert over a spread of chunk
        // sizes so the test doesn't depend on hitting one exact offset.
        for pad in 0..3 {
            let mut buf = vec![b'x'; STDERR_BUF_CAP - 1 + pad];
            let chunk = "€€€€€".as_bytes(); // 5 * 3 = 15 bytes, multi-byte throughout
            append_capped(&mut buf, chunk);
            assert!(buf.len() <= STDERR_BUF_CAP);
            // Must not panic, and the lossy decode used at the actual
            // call site must also not panic on a boundary-split lead
            // byte.
            let _ = String::from_utf8_lossy(&buf);
        }
    }

    /// CD-253: invalid UTF-8 bytes must not stop the drain — the old
    /// `read_line`-based loop treated a `read` returning `Err` (which
    /// `BufRead::read_line` does on invalid UTF-8) the same as EOF,
    /// silently and permanently ending the drain for the worker's
    /// remaining lifetime. `append_capped` operates on raw bytes with
    /// no encoding validation, so this is really a proof that invalid
    /// UTF-8 round-trips harmlessly through it and its `from_utf8_lossy`
    /// consumer.
    #[test]
    fn append_capped_tolerates_invalid_utf8() {
        let mut buf = Vec::new();
        let invalid = [0xFF, 0xFE, b'h', b'i', b'\n'];
        append_capped(&mut buf, &invalid);
        assert_eq!(buf, invalid);
        assert!(String::from_utf8_lossy(&buf).contains("hi"));
    }

    /// CD-259: newline-free input must not grow the drain's transient
    /// buffer past `DRAIN_CHUNK_SIZE` per read — before the fix,
    /// `read_until(b'\n', ...)` accumulated the entire input into one
    /// `Vec` before ever calling `append_capped`, so output with no `\n`
    /// (a `\r`-only progress bar, one huge `console.error` blob) grew
    /// unboundedly regardless of `STDERR_BUF_CAP`. Feeds several times
    /// `STDERR_BUF_CAP` worth of newline-free bytes through the drain
    /// helper directly (no child process needed) and asserts the final
    /// buffer still respects the cap.
    #[test]
    fn drain_into_bounds_memory_on_newline_free_input() {
        let data = vec![b'x'; STDERR_BUF_CAP * 4];
        let buf = Mutex::new(Vec::new());
        drain_into(std::io::Cursor::new(data), &buf);
        let bytes = buf.lock().expect("lock");
        assert!(
            bytes.len() <= STDERR_BUF_CAP,
            "expected the buffer to stay capped at {STDERR_BUF_CAP}, got {}",
            bytes.len()
        );
    }

    /// CD-260: a panic while `stderr_buf` is locked elsewhere (the only
    /// realistic site is `Worker::request_nullable`'s CD-238 error path)
    /// must not permanently stop the drain — the buffer holds a lossy
    /// diagnostic tail with no invariant a panic could corrupt, so
    /// honouring the poison and giving up forever is strictly worse
    /// than reading through it.
    #[test]
    fn drain_into_recovers_from_a_poisoned_lock() {
        let buf = Mutex::new(Vec::new());
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = buf.lock().expect("lock");
            panic!("simulate a panic while holding the lock");
        }));
        assert!(poisoned.is_err());
        assert!(buf.is_poisoned());

        drain_into(std::io::Cursor::new(b"after poison\n".to_vec()), &buf);

        let bytes = buf.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(&*bytes, b"after poison\n");
    }

    /// CD-260: a transient read error (e.g. `WouldBlock`) must not end
    /// the drain permanently — before the fix, `Err(_) => break` treated
    /// every error the same as EOF.
    #[test]
    fn drain_into_retries_transient_read_errors_instead_of_giving_up() {
        struct FlakyThenData {
            errors_left: u32,
            payload: Option<Vec<u8>>,
        }
        impl Read for FlakyThenData {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if self.errors_left > 0 {
                    self.errors_left -= 1;
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "simulated transient error",
                    ));
                }
                match self.payload.take() {
                    Some(payload) => {
                        out[..payload.len()].copy_from_slice(&payload);
                        Ok(payload.len())
                    }
                    None => Ok(0),
                }
            }
        }

        let reader = FlakyThenData {
            errors_left: MAX_TRANSIENT_READ_RETRIES - 1,
            payload: Some(b"hi\n".to_vec()),
        };
        let buf = Mutex::new(Vec::new());
        drain_into(reader, &buf);
        let bytes = buf.lock().expect("lock");
        assert_eq!(&*bytes, b"hi\n");
    }

    /// CD-260: a reader that errors forever must still make `drain_into`
    /// return promptly (bounded retries) rather than hang the thread.
    #[test]
    fn drain_into_gives_up_after_exhausting_retries_on_a_persistently_erroring_reader() {
        struct AlwaysErrors;
        impl Read for AlwaysErrors {
            fn read(&mut self, _out: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("simulated persistent error"))
            }
        }
        let buf = Mutex::new(Vec::new());
        let start = std::time::Instant::now();
        drain_into(AlwaysErrors, &buf);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "should give up after bounded retries, not hang"
        );
    }

    /// CD-270: a permanent error kind (anything that isn't `Interrupted`,
    /// `WouldBlock`, or `TimedOut` — e.g. `BrokenPipe`, what a real
    /// `ChildStderr` actually returns on failure) must break immediately,
    /// not pay the `MAX_TRANSIENT_READ_RETRIES` sleep budget first. That
    /// budget exists for a hypothetical non-blocking reader; wasting it
    /// on an error class it can never fix just delays the `done_tx`
    /// signal `Worker::request_nullable`'s CD-254 join is waiting on.
    #[test]
    fn drain_into_breaks_immediately_on_a_permanent_error_kind_without_retry_delay() {
        struct AlwaysBrokenPipe;
        impl Read for AlwaysBrokenPipe {
            fn read(&mut self, _out: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            }
        }
        let buf = Mutex::new(Vec::new());
        let start = std::time::Instant::now();
        drain_into(AlwaysBrokenPipe, &buf);
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "a permanent error kind should not be retried at all, took {:?}",
            start.elapsed()
        );
    }

    /// CD-270: `Interrupted` is retried unconditionally (matching
    /// `std::io::Read::read_to_end`'s own behaviour) but must still be
    /// bounded — an EINTR storm from a misbehaving signal handler must
    /// not busy-spin the drain thread forever. Uses a reader that always
    /// returns `Interrupted` and asserts `drain_into` still returns.
    #[test]
    fn drain_into_bounds_unconditional_interrupted_retries() {
        struct AlwaysInterrupted;
        impl Read for AlwaysInterrupted {
            fn read(&mut self, _out: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
            }
        }
        let buf = Mutex::new(Vec::new());
        let start = std::time::Instant::now();
        drain_into(AlwaysInterrupted, &buf);
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "an EINTR storm should still be bounded, not spin forever; took {:?}",
            start.elapsed()
        );
    }

    /// CD-244/CD-245: a worker that writes more than one OS pipe buffer
    /// (typically 8-64 KB) to stderr and then exits without ever
    /// producing a stdout response must not hang `request_nullable` —
    /// before the drain thread existed, an unread stderr pipe would
    /// block the child in `write(2)`, so it would never close stdout
    /// and the read on line ~362 would block forever. Also proves the
    /// EOF error message carries stderr diagnostics without doing a
    /// blocking read on the (now already-drained) pipe (CD-244).
    #[test]
    fn stderr_drain_prevents_deadlock_and_surfaces_diagnostics_on_stdout_eof() {
        if !node_present() {
            return;
        }
        let mut child = Command::new("node")
            .arg("-e")
            .arg(
                "for (let i = 0; i < 20000; i++) { console.error('x'.repeat(100)); } \
                 process.exit(1);",
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn node");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let (stderr_buf, stderr_drain_done) =
            spawn_stderr_drain(child.stderr.take().expect("child stderr"));
        let mut worker = Worker {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            stderr_buf,
            stderr_drain_done,
        };

        let start = std::time::Instant::now();
        let result: Result<Option<PingResult>, TypeHostError> =
            worker.request_nullable("t", "ping", &());
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(20),
            "request_nullable should not block on the undrained stderr pipe; took {elapsed:?}"
        );
        match result {
            Err(TypeHostError::Io(msg)) => {
                assert!(
                    msg.contains("stderr: "),
                    "expected the drained stderr tail in the error message, got: {msg}"
                );
            }
            other => panic!("expected TypeHostError::Io with stderr, got {other:?}"),
        }
        let _ = worker.close();
    }

    /// CD-254: a worker that writes a single short line to stderr and
    /// exits immediately — nothing here comes close to filling an OS
    /// pipe buffer, so before the CD-254 fix this raced the drain
    /// thread: the main thread could observe stdout EOF and snapshot
    /// `stderr_buf` before the drain thread had scheduled at all,
    /// nondeterministically losing the diagnostic. Run several times to
    /// make a reintroduced race very likely to surface as a flake.
    #[test]
    fn stderr_drain_join_makes_short_lived_failure_diagnostics_deterministic() {
        if !node_present() {
            return;
        }
        for _ in 0..20 {
            let mut child = Command::new("node")
                .arg("-e")
                .arg("console.error('boom: short trace'); process.exit(1);")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn node");
            let stdin = child.stdin.take().expect("child stdin");
            let stdout = child.stdout.take().expect("child stdout");
            let (stderr_buf, stderr_drain_done) =
                spawn_stderr_drain(child.stderr.take().expect("child stderr"));
            let mut worker = Worker {
                child,
                stdin: Some(stdin),
                stdout: BufReader::new(stdout),
                stderr_buf,
                stderr_drain_done,
            };

            let result: Result<Option<PingResult>, TypeHostError> =
                worker.request_nullable("t", "ping", &());
            match result {
                Err(TypeHostError::Io(msg)) => {
                    assert!(
                        msg.contains("boom: short trace"),
                        "expected the short stderr line in the error message every time, got: {msg}"
                    );
                }
                other => panic!("expected TypeHostError::Io with stderr, got {other:?}"),
            }
            let _ = worker.close();
        }
    }

    /// CD-261: a stray line on the worker's stdout — malformed JSON, or
    /// valid JSON that answers a different request id — must not
    /// permanently desync purely-positional response pairing. Uses a
    /// bare `node -e` "worker" (not the real type-host script) that
    /// emits one non-JSON line before responding to each request over a
    /// readline-framed stdin/stdout protocol, matching what
    /// `Worker::request_nullable` speaks.
    #[test]
    fn request_nullable_resyncs_past_a_stray_stdout_line_instead_of_desyncing() {
        if !node_present() {
            return;
        }
        let mut child = Command::new("node")
            .arg("-e")
            .arg(
                "const readline = require('readline'); \
                 console.log('stray line not json'); \
                 const rl = readline.createInterface({ input: process.stdin }); \
                 rl.on('line', (line) => { \
                   const req = JSON.parse(line); \
                   process.stdout.write(JSON.stringify({ id: req.id, ok: true, result: null }) + '\\n'); \
                 });",
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn node");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let (stderr_buf, stderr_drain_done) =
            spawn_stderr_drain(child.stderr.take().expect("child stderr"));
        let mut worker = Worker {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            stderr_buf,
            stderr_drain_done,
        };

        let result: Result<Option<PingResult>, TypeHostError> =
            worker.request_nullable("expected-id", "ping", &());
        match result {
            Ok(None) => {}
            other => panic!("expected Ok(None) after resyncing past the stray line, got {other:?}"),
        }
        let _ = worker.close();
    }

    /// CD-261: a response whose id never matches (a permanently desynced
    /// worker, or a genuinely different in-flight request that will
    /// never arrive) must give up after `MAX_RESPONSE_RESYNC_ATTEMPTS`
    /// rather than block the caller forever reading stray lines.
    #[test]
    fn request_nullable_gives_up_resyncing_after_the_bounded_attempt_limit() {
        if !node_present() {
            return;
        }
        let mut child = Command::new("node")
            .arg("-e")
            .arg(
                "const readline = require('readline'); \
                 const rl = readline.createInterface({ input: process.stdin }); \
                 rl.on('line', () => { \
                   for (let i = 0; i < 64; i++) { \
                     process.stdout.write(JSON.stringify({ id: 'never-matches', ok: true, result: null }) + '\\n'); \
                   } \
                 });",
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn node");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let (stderr_buf, stderr_drain_done) =
            spawn_stderr_drain(child.stderr.take().expect("child stderr"));
        let mut worker = Worker {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            stderr_buf,
            stderr_drain_done,
        };

        let start = std::time::Instant::now();
        let result: Result<Option<PingResult>, TypeHostError> =
            worker.request_nullable("expected-id", "ping", &());
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(20),
            "should give up after bounded resync attempts, not block; took {elapsed:?}"
        );
        match result {
            Err(TypeHostError::BadResponse(msg)) => {
                assert!(
                    msg.contains("resyncing"),
                    "expected a resync-exhausted error, got: {msg}"
                );
            }
            other => panic!("expected TypeHostError::BadResponse, got {other:?}"),
        }
        let _ = worker.close();
    }

    /// CD-282: the default pool size must never exceed the cap, even on
    /// a high-core-count host, but should stay under it on a low-core
    /// host rather than always saturating the cap.
    #[test]
    fn default_pool_size_caps_high_parallelism_but_passes_through_low_parallelism() {
        assert_eq!(default_pool_size(1), 1);
        assert_eq!(default_pool_size(2), 2);
        assert_eq!(
            default_pool_size(DEFAULT_POOL_SIZE_CAP),
            DEFAULT_POOL_SIZE_CAP
        );
        assert_eq!(default_pool_size(64), DEFAULT_POOL_SIZE_CAP);
    }

    /// CD-281: a response that matches our request id but fails to
    /// deserialise its typed fields (here, an `ok:false` whose `error`
    /// body is missing the required `code`/`message`) must surface as
    /// `BadResponse` immediately — not be discarded as a "stray line" and
    /// leave the caller blocked reading for a response the worker already
    /// sent. Regression test for the id-vs-parse ordering bug: the id
    /// must be checked on a raw envelope before attempting to deserialise
    /// `result`/`error` into their typed shapes.
    #[test]
    fn request_nullable_surfaces_a_malformed_response_for_our_own_id_instead_of_treating_it_as_stray(
    ) {
        if !node_present() {
            return;
        }
        let mut child = Command::new("node")
            .arg("-e")
            .arg(
                "const readline = require('readline'); \
                 const rl = readline.createInterface({ input: process.stdin }); \
                 rl.on('line', (line) => { \
                   const req = JSON.parse(line); \
                   process.stdout.write(JSON.stringify({ id: req.id, ok: false, error: {} }) + '\\n'); \
                 });",
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn node");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let (stderr_buf, stderr_drain_done) =
            spawn_stderr_drain(child.stderr.take().expect("child stderr"));
        let mut worker = Worker {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            stderr_buf,
            stderr_drain_done,
        };

        let start = std::time::Instant::now();
        let result: Result<Option<PingResult>, TypeHostError> =
            worker.request_nullable("expected-id", "ping", &());
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(20),
            "must fail fast on our own malformed response, not block; took {elapsed:?}"
        );
        match result {
            Err(TypeHostError::BadResponse(msg)) => {
                assert!(
                    msg.contains("expected-id"),
                    "expected the response id in the error message, got: {msg}"
                );
            }
            other => panic!("expected TypeHostError::BadResponse, got {other:?}"),
        }
        let _ = worker.close();
    }

    /// CD-172: the Rust-side pre-check must agree with what
    /// `type-host-core.mjs`'s JS walk would find, in both directions —
    /// no `node_modules/ts-morph` anywhere in the ancestor chain is
    /// `false`; a `package.json` directly under `start` is `true`.
    #[test]
    fn ts_morph_resolvable_false_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!ts_morph_resolvable(dir.path()));
    }

    #[test]
    fn ts_morph_resolvable_true_in_start_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg_dir = dir.path().join("node_modules").join("ts-morph");
        std::fs::create_dir_all(&pkg_dir).expect("mkdir");
        std::fs::write(pkg_dir.join("package.json"), b"{}").expect("write package.json");
        assert!(ts_morph_resolvable(dir.path()));
    }

    /// The JS walk climbs parent directories looking for
    /// `node_modules/ts-morph` (matching how Node itself resolves
    /// `node_modules` up a project tree); a nested source directory
    /// must find an ancestor's install just as readily as its own.
    #[test]
    fn ts_morph_resolvable_true_via_ancestor_walk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pkg_dir = dir.path().join("node_modules").join("ts-morph");
        std::fs::create_dir_all(&pkg_dir).expect("mkdir");
        std::fs::write(pkg_dir.join("package.json"), b"{}").expect("write package.json");

        let nested = dir.path().join("src").join("deeply").join("nested");
        std::fs::create_dir_all(&nested).expect("mkdir nested");
        assert!(ts_morph_resolvable(&nested));
    }

    /// CD-172: `build_type_oracle` must short-circuit with the same
    /// `ts_morph_unavailable` error the JS-side walk would produce
    /// (see `type_host_ping_without_ts_morph_surfaces_clear_error` in
    /// `tests/type_host.rs` for the JS-path equivalent), WITHOUT
    /// spawning any Node worker — this test needs no `node_present()`
    /// gate, which is itself proof the fast path never touches Node.
    #[test]
    fn build_type_oracle_short_circuits_when_ts_morph_absent() {
        let project = tempfile::tempdir().expect("tempdir");
        let tsconfig = project.path().join("tsconfig.json");
        std::fs::write(&tsconfig, b"{}").expect("write tsconfig");

        match build_type_oracle(project.path(), &tsconfig) {
            Err(TypeHostError::HostError { code, .. }) => {
                assert_eq!(code, "ts_morph_unavailable");
            }
            Err(other) => panic!("expected HostError{{code: ts_morph_unavailable}}, got {other}"),
            Ok(_) => panic!("no node_modules/ts-morph anywhere under a fresh tempdir"),
        }
    }

    /// CD-266: `write_atomic` must leave the destination path either
    /// absent or fully written — never a torn/partial file, since
    /// that's exactly what a concurrent `node` process opening the
    /// destination mid-write would otherwise observe (`SyntaxError:
    /// Unexpected end of input`). Also asserts the temp sibling is
    /// cleaned up (renamed away, not left behind).
    #[test]
    fn write_atomic_overwrite_leaves_no_partial_or_leftover_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("script.mjs");

        write_atomic(&path, "short").expect("first write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "short");

        let longer = "much longer content than before, to prove overwrite isn't append";
        write_atomic(&path, longer).expect("second write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), longer);

        let leftover_tmp: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .filter(|name| name != "script.mjs")
            .collect();
        assert!(
            leftover_tmp.is_empty(),
            "expected the tmp sibling to be renamed away, found: {leftover_tmp:?}"
        );
    }

    /// CD-266: a reader polling the destination path while many writers
    /// race `write_atomic` against it (the real production scenario —
    /// concurrent same-version `cofferdam` processes materialising the
    /// byte-identical embedded script) must never observe a prefix of
    /// the content; only "doesn't exist yet" or "fully written" are
    /// valid observations. Before the fix (`fs::write` directly, no
    /// rename), a reader could open the file mid-truncate-and-write.
    #[test]
    fn write_atomic_never_exposes_a_torn_file_to_a_concurrent_reader() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("script.mjs");
        let expected = "x".repeat(200_000); // large enough that a torn read is likely if unprotected

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_path = path.clone();
        let reader_expected = expected.clone();
        let reader_stop = Arc::clone(&stop);
        let reader = std::thread::spawn(move || {
            let mut torn = None;
            while !reader_stop.load(Ordering::Relaxed) {
                if let Ok(contents) = std::fs::read_to_string(&reader_path) {
                    if contents != reader_expected {
                        torn = Some(contents.len());
                        break;
                    }
                }
            }
            torn
        });

        for _ in 0..50 {
            write_atomic(&path, &expected).expect("write");
        }
        stop.store(true, Ordering::Relaxed);
        let torn_len = reader.join().expect("reader thread");
        assert!(
            torn_len.is_none(),
            "reader observed a torn file of length {torn_len:?}, expected {}",
            expected.len()
        );
    }

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

    /// CD-82: `WorkerTypeOracle::resolve_literal` resolves a cross-file
    /// string literal via the wire protocol — `constants.ts` declares
    /// `export const description = "..."`; `page.ts` imports it. ts-morph
    /// symbol resolution follows the import to the origin declaration
    /// since both files are part of the same opened Project. Same gating
    /// as `worker_oracle_resolves_nullable_type`.
    #[test]
    fn worker_oracle_resolves_cross_file_literal() {
        let Ok(ts_morph_root) = std::env::var("COFFERDAM_TYPE_HOST_TS_MORPH_ROOT") else {
            return; // not configured — skip
        };
        let ts_morph_root = PathBuf::from(ts_morph_root);

        let project = tempfile::tempdir().expect("tempdir");
        let constants = project.path().join("constants.ts");
        std::fs::write(
            &constants,
            "export const description = \"A great page about widgets\";\n",
        )
        .expect("write constants");
        let page = project.path().join("page.ts");
        // `description` (the imported identifier reference) starts at
        // byte 9 of the import line: `import { description } from "./constants";`
        std::fs::write(
            &page,
            "import { description } from \"./constants\";\nexport { description };\n",
        )
        .expect("write page");
        let tsconfig = project.path().join("tsconfig.json");
        std::fs::write(
            &tsconfig,
            r#"{ "compilerOptions": { "strict": true, "noEmit": true }, "include": ["constants.ts", "page.ts"] }"#,
        )
        .expect("write tsconfig");

        let oracle = match build_type_oracle(&ts_morph_root, &tsconfig) {
            Ok(o) => o,
            Err(e) => panic!(
                "build_type_oracle failed (is ts-morph installed at {}?): {e}",
                ts_morph_root.display()
            ),
        };

        // "import { " is 9 bytes ("import { "); `description` (11 chars)
        // spans [9, 20).
        let facts = oracle
            .resolve_literal(&page, 9, 20)
            .expect("resolve_literal should resolve the imported identifier");
        assert_eq!(
            facts.literal_string.as_deref(),
            Some("A great page about widgets"),
            "should resolve through the import to constants.ts's literal; got {facts:?}"
        );
        assert!(!facts.is_nullable, "got {facts:?}");
        assert!(!facts.is_empty_object, "got {facts:?}");
    }
}
