//! Server state: open documents, engine handle, caches, workspace root.
//!
//! Held by [`crate::server::run_stdio_server`] for the life of one
//! LSP session. Open-document text takes precedence over disk when
//! the engine reads sources — so unsaved edits are analyzed live.
//!
//! Disk reads still happen for files the editor hasn't opened — the
//! engine needs every file in the workspace to populate corpus
//! slots for cross-file checks. Closed files come from the
//! filesystem on each re-analyze, which the cp4 disk cache + cp2/3
//! in-memory caches make cheap.

use std::collections::HashMap;
use std::path::PathBuf;

use cofferdam_engine::{findings_cache::FindingsCache, run_cache::RunCache, Engine};
use lsp_types::Url;

/// Mutable LSP session state. Single-threaded by construction —
/// the receive loop in [`crate::server`] owns this and never shares
/// it across threads. The engine itself is sync.
pub struct ServerState {
    /// Workspace root, set from the `initialize` rootUri/rootPath.
    /// All file discovery is anchored here.
    pub workspace_root: PathBuf,
    /// Open documents indexed by URI. The editor's in-memory copy
    /// overrides disk for analysis purposes. `didOpen` adds an
    /// entry; `didChange` replaces it; `didClose` removes it.
    pub open_docs: HashMap<Url, String>,
    /// Engine instance, built once from the project config at
    /// startup. Re-created if the config file changes between
    /// analyses — but cp5 doesn't watch the config; restart the
    /// LSP to pick up config changes.
    pub engine: Engine,
    /// Disk-backed per-file findings cache. Hydrated from
    /// `.cofferdam/cache/<version>/findings.json` on startup;
    /// persisted on shutdown.
    pub findings_cache: FindingsCache,
    /// Disk-backed full-run cache. Same lifecycle as
    /// [`Self::findings_cache`].
    pub run_cache: RunCache,
    /// Cache directory in use. `None` if the user disabled caching
    /// via the LSP launch args (not yet wired — cp5 ships caching
    /// always on).
    pub cache_dir: Option<PathBuf>,
}

impl ServerState {
    /// Construct a fresh server state with empty document store.
    pub fn new(workspace_root: PathBuf, engine: Engine, cache_dir: Option<PathBuf>) -> Self {
        Self {
            workspace_root,
            open_docs: HashMap::new(),
            engine,
            findings_cache: FindingsCache::new(),
            run_cache: RunCache::new(),
            cache_dir,
        }
    }
}
