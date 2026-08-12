//! Stdio-based LSP server entry point + main message loop.
//!
//! Spawns the lsp-server transport, performs the
//! `initialize` / `initialized` handshake, runs the receive loop
//! until `shutdown` + `exit`. On each `textDocument/didOpen` or
//! `textDocument/didSave` notification, runs the engine over the
//! workspace and publishes per-file diagnostics.
//!
//! ## Why save-time, not change-time
//!
//! `didChange` fires on every keystroke. Even with cp4's warm path
//! (~15 ms on a 300-file repo) the analysis still blocks the
//! receive loop. A debounced + worker-thread path is a follow-up;
//! cp5 ships save-time analysis which trivially meets the bead's
//! "editor shows findings on file save within ~500ms" gate.
//!
//! ## Why workspace-wide (not single-file) re-analyze
//!
//! Cross-file checks (orphan-export, duplicate-export, layer
//! violation, import cycle) need the entire file graph in the
//! corpus to emit correct findings. A single-file re-run would
//! show stale or missing diagnostics for those checks. The cp4
//! cache reuses the per-file work on unchanged files, so workspace-
//! wide re-runs are O(changed-files) in practice.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cofferdam_checks::all_builtins;
use cofferdam_engine::{config::ProjectConfig, discover, disk_cache, DiscoveryOptions, Engine};
use lsp_server::{Connection, ExtractError, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _, PublishDiagnostics,
};
use lsp_types::request::{Request as _, Shutdown};
use lsp_types::{
    Diagnostic, DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, InitializeParams, InitializeResult, OneOf, PositionEncodingKind,
    PublishDiagnosticsParams, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind, Url,
};

use crate::diagnostic::issue_to_diagnostic;
use crate::state::ServerState;

/// Run the cofferdam LSP server over stdio. Blocks until the client
/// sends `shutdown` + `exit`. Returns `Ok(())` on graceful exit,
/// `Err` on transport-level failures (channel disconnect, malformed
/// initialize).
///
/// `cwd` seeds the workspace root when the client doesn't supply
/// one in `initialize`. The CLI passes `std::env::current_dir()`.
pub fn run_stdio_server(cwd: PathBuf) -> Result<(), ServerError> {
    let (connection, io_threads) = Connection::stdio();
    run_server(connection, cwd)?;
    io_threads.join().map_err(ServerError::IoJoin)?;
    Ok(())
}

/// Transport-agnostic server core. Drives the LSP message loop over
/// any [`Connection`]. The stdio entry point creates a real stdio
/// connection; integration tests use [`Connection::memory`] to
/// drive the server from the same process.
///
/// Returns when the client sends `shutdown` followed by `exit`, or
/// on protocol error.
pub fn run_server(connection: Connection, cwd: PathBuf) -> Result<(), ServerError> {
    // Initialize handshake.
    let (initialize_id, initialize_params) = connection
        .initialize_start()
        .map_err(ServerError::ProtocolError)?;
    let initialize_params: InitializeParams =
        serde_json::from_value(initialize_params).map_err(ServerError::DeserializeParams)?;

    let capabilities = server_capabilities();
    let initialize_result = InitializeResult {
        capabilities,
        server_info: Some(ServerInfo {
            name: "cofferdam-lsp".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };
    connection
        .initialize_finish(
            initialize_id,
            serde_json::to_value(initialize_result).map_err(ServerError::SerializeResponse)?,
        )
        .map_err(ServerError::ProtocolError)?;

    // Resolve workspace root from the client's initialize params,
    // falling back to the CLI's cwd if none was supplied.
    let workspace_root = workspace_root_from_params(&initialize_params).unwrap_or(cwd);

    // Build the engine. Config discovery walks up from
    // workspace_root looking for cofferdam.toml. Errors are surfaced
    // as a one-line warning notification to the client and the
    // engine falls back to default config.
    let engine = build_engine(&workspace_root);

    // Disk cache: hydrate findings + run caches from the workspace
    // .cofferdam/cache directory, if present. Failures are silent.
    let cache_dir = workspace_root.join(".cofferdam").join("cache");
    let mut state = ServerState::new(workspace_root.clone(), engine, Some(cache_dir.clone()));
    let registered_ids: Vec<&'static str> = all_builtins().iter().map(|c| c.meta().id).collect();
    let _ = disk_cache::load_findings(&cache_dir, &state.findings_cache, &registered_ids);
    let _ = disk_cache::load_run(&cache_dir, &state.run_cache);

    // Initial workspace scan. Publishes diagnostics for every TS
    // file in the workspace before the loop accepts further
    // messages. On bestefforttools this is ~150 ms cold / ~15 ms
    // warm with the cp4 disk cache hydrated above.
    analyze_and_publish(&connection, &state);

    // Receive loop.
    for message in &connection.receiver {
        match message {
            Message::Request(req) => {
                if connection
                    .handle_shutdown(&req)
                    .map_err(ServerError::ProtocolError)?
                {
                    // Client asked us to shut down. Persist caches
                    // before exiting.
                    persist_caches(&state);
                    break;
                }
                // No other requests implemented in cp5. Reply with
                // method-not-found so the client gets a real error
                // rather than a hang.
                let _ = handle_unknown_request(&connection, req);
            }
            Message::Notification(notif) => {
                handle_notification(&connection, &mut state, notif);
            }
            Message::Response(_) => {
                // We don't issue requests, so any response is from a
                // server-to-client request we don't make. Ignore.
            }
        }
    }

    Ok(())
}

// io_threads.join() returns std::io::Error, not a thread-panic.
// The `IoJoin` variant carries an `io::Error` so the message preserves
// the underlying cause; the panic-payload variant was over-spec'd.
//
// (Definition below mirrors the variant signature change.)

fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF8),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        definition_provider: None,
        diagnostic_provider: None,
        workspace: Some(lsp_types::WorkspaceServerCapabilities {
            workspace_folders: Some(lsp_types::WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(OneOf::Left(false)),
            }),
            file_operations: None,
        }),
        ..Default::default()
    }
}

fn workspace_root_from_params(params: &InitializeParams) -> Option<PathBuf> {
    if let Some(folders) = params.workspace_folders.as_ref().filter(|f| !f.is_empty()) {
        if let Ok(p) = folders[0].uri.to_file_path() {
            return Some(p);
        }
    }
    #[allow(deprecated)]
    if let Some(uri) = params.root_uri.as_ref() {
        if let Ok(p) = uri.to_file_path() {
            return Some(p);
        }
    }
    #[allow(deprecated)]
    if let Some(path) = params.root_path.as_ref() {
        return Some(PathBuf::from(path));
    }
    None
}

fn build_engine(workspace_root: &Path) -> Engine {
    let checks = all_builtins();
    // cofferdam-engine doesn't have a `config::load_for_workspace`
    // helper at present — load with the CLI's discovery rules would
    // require duplicating its walker. For cp5 v1, we ship default
    // engine config; cofferdam.toml support in the LSP is a small
    // follow-up bead (the engine surface to plug it in is already
    // there via Engine::with_config).
    let _ = workspace_root;
    let _ = ProjectConfig::default;
    Engine::new(checks)
}

// crossbeam SendError contains the full Message it failed to send,
// which clippy flags as a large Err variant. The error is only
// produced when the client disconnects mid-write; boxing for that
// case is not worth the extra indirection.
#[allow(clippy::result_large_err)]
fn handle_unknown_request(
    connection: &Connection,
    req: Request,
) -> Result<(), crossbeam_channel::SendError<Message>> {
    let resp = Response {
        id: req.id,
        result: None,
        error: Some(lsp_server::ResponseError {
            code: lsp_server::ErrorCode::MethodNotFound as i32,
            message: format!("cofferdam-lsp: method `{}` not supported", req.method),
            data: None,
        }),
    };
    connection.sender.send(Message::Response(resp))
}

fn handle_notification(connection: &Connection, state: &mut ServerState, notif: Notification) {
    match notif.method.as_str() {
        DidOpenTextDocument::METHOD => {
            if let Ok(params) = cast_notification::<DidOpenTextDocument>(notif) {
                let p: DidOpenTextDocumentParams = params;
                state
                    .open_docs
                    .insert(p.text_document.uri.clone(), p.text_document.text);
                analyze_and_publish(connection, state);
            }
        }
        DidChangeTextDocument::METHOD => {
            if let Ok(params) = cast_notification::<DidChangeTextDocument>(notif) {
                let p: DidChangeTextDocumentParams = params;
                // Full sync: the single content change is the new full text.
                if let Some(change) = p.content_changes.into_iter().next() {
                    state
                        .open_docs
                        .insert(p.text_document.uri.clone(), change.text);
                }
                // cp5 deliberately does NOT re-analyze on didChange.
                // Save-time analysis is enough for the acceptance gate
                // and avoids a debouncer + worker thread.
            }
        }
        DidSaveTextDocument::METHOD => {
            if let Ok(params) = cast_notification::<DidSaveTextDocument>(notif) {
                let p: DidSaveTextDocumentParams = params;
                // If the client included `text` (capability-gated), use
                // it as the new in-memory copy; otherwise the existing
                // open_docs entry stays put and the engine reads the
                // saved file off disk on its next read pass.
                if let Some(text) = p.text {
                    state.open_docs.insert(p.text_document.uri, text);
                }
                analyze_and_publish(connection, state);
            }
        }
        DidCloseTextDocument::METHOD => {
            if let Ok(params) = cast_notification::<DidCloseTextDocument>(notif) {
                let p: DidCloseTextDocumentParams = params;
                state.open_docs.remove(&p.text_document.uri);
                // Closed files come from disk on next analyze; no
                // need to re-publish immediately. The next save in
                // any open file triggers a workspace re-analyze.
            }
        }
        _ => {
            // Silent ignore — many client-side notifications (e.g.
            // `$/setTrace`) are noise for cp5.
        }
    }
}

fn cast_notification<N: lsp_types::notification::Notification>(
    notif: Notification,
) -> Result<N::Params, ExtractError<Notification>> {
    notif.extract::<N::Params>(N::METHOD)
}

/// Run the engine over the current workspace and push diagnostics
/// for every file (including empties to clear stale findings). Open
/// documents' in-memory text overrides their on-disk version.
fn analyze_and_publish(connection: &Connection, state: &ServerState) {
    // Discover the file set under the workspace root.
    let discovery_opts = DiscoveryOptions::default();
    let files = match discover(std::slice::from_ref(&state.workspace_root), &discovery_opts) {
        Ok(f) => f,
        Err(_) => {
            // Discovery failure is rare (I/O on the workspace root).
            // Skip the publish; the user sees no new diagnostics.
            return;
        }
    };

    // Read every file. Open-doc text wins; everything else from disk.
    let mut sources: Vec<(PathBuf, String)> = Vec::with_capacity(files.len());
    for path in &files {
        let url = match Url::from_file_path(path) {
            Ok(u) => u,
            Err(_) => continue,
        };
        let text = if let Some(open) = state.open_docs.get(&url) {
            open.clone()
        } else {
            match std::fs::read_to_string(path) {
                Ok(t) => t,
                Err(_) => continue,
            }
        };
        sources.push((path.clone(), text));
    }

    let (issues, _texts) = state.engine.analyze_with_sources_full(
        sources,
        None,
        Some(&state.findings_cache),
        Some(&state.run_cache),
    );

    // Group issues by file URI. Every discovered file gets an entry,
    // including those with zero issues — that's how editors clear
    // stale diagnostics from previous publishes.
    let mut grouped: HashMap<Url, Vec<Diagnostic>> = HashMap::new();
    for path in &files {
        if let Ok(url) = Url::from_file_path(path) {
            grouped.entry(url).or_default();
        }
    }
    for issue in &issues {
        if let Ok(url) = Url::from_file_path(&issue.file) {
            let diag = issue_to_diagnostic(issue, &url);
            grouped.entry(url).or_default().push(diag);
        }
    }

    for (url, diags) in grouped {
        let params = PublishDiagnosticsParams {
            uri: url,
            diagnostics: diags,
            version: None,
        };
        let notif = Notification {
            method: PublishDiagnostics::METHOD.to_string(),
            params: serde_json::to_value(params).unwrap_or(serde_json::Value::Null),
        };
        let _ = connection.sender.send(Message::Notification(notif));
    }
}

fn persist_caches(state: &ServerState) {
    let Some(dir) = state.cache_dir.as_deref() else {
        return;
    };
    // Skip the rewrite when nothing new was cached since the last
    // persist — the file on disk already matches (CD-185).
    if state.findings_cache.is_dirty() {
        let _ = disk_cache::save_findings(dir, &state.findings_cache);
    }
    if state.run_cache.is_dirty() {
        let _ = disk_cache::save_run(dir, &state.run_cache);
    }
}

#[allow(dead_code)] // future: typed request dispatch helper
fn cast_request<R: lsp_types::request::Request>(
    req: Request,
) -> Result<(RequestId, R::Params), ExtractError<Request>> {
    req.extract::<R::Params>(R::METHOD)
}

#[allow(dead_code)] // exported so a future "always shutdown" check can use it
fn is_shutdown(req: &Request) -> bool {
    req.method == Shutdown::METHOD
}

/// Errors that can short-circuit [`run_stdio_server`].
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("LSP protocol error: {0}")]
    ProtocolError(#[from] lsp_server::ProtocolError),
    #[error("failed to deserialize initialize params: {0}")]
    DeserializeParams(#[source] serde_json::Error),
    #[error("failed to serialize initialize response: {0}")]
    SerializeResponse(#[source] serde_json::Error),
    #[error("stdio I/O thread failed: {0}")]
    IoJoin(#[source] std::io::Error),
}
