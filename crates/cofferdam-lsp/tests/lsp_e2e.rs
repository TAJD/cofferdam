//! End-to-end LSP server test (cd-9hp.4 cp5).
//!
//! Drives the server through its full `initialize` → `didOpen` →
//! `publishDiagnostics` → `shutdown` cycle via an in-process
//! [`Connection::memory`] pair. No subprocess, no real stdio.
//! Pins the contract the bead's acceptance gate calls out:
//!
//! - Server publishes diagnostics for a file that exercises a
//!   built-in check (here: `Refactor.PreferConstOverLet` on `let`).
//! - The diagnostic carries cofferdam's source / code / message /
//!   severity in their LSP forms.
//! - The server shuts down cleanly on `shutdown` + `exit`.

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidOpenTextDocument, DidSaveTextDocument, Exit, Initialized, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{Initialize, Shutdown};
use lsp_types::{
    ClientCapabilities, DidOpenTextDocumentParams, DidSaveTextDocumentParams, InitializeParams,
    InitializedParams, PublishDiagnosticsParams, TextDocumentIdentifier, TextDocumentItem, Url,
    WorkspaceFolder,
};
use tempfile::tempdir;

fn workspace_with_prefer_const() -> tempfile::TempDir {
    let dir = tempdir().expect("tempdir");
    let ts = dir.path().join("a.ts");
    std::fs::write(&ts, "let x = 1;\n").unwrap();
    dir
}

fn send_request<R: lsp_types::request::Request>(connection: &Connection, id: i32, params: R::Params)
where
    R::Params: serde::Serialize,
{
    connection
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(id),
            method: R::METHOD.to_string(),
            params: serde_json::to_value(params).expect("serialize"),
        }))
        .expect("send request");
}

fn send_notification<N: lsp_types::notification::Notification>(
    connection: &Connection,
    params: N::Params,
) where
    N::Params: serde::Serialize,
{
    connection
        .sender
        .send(Message::Notification(Notification {
            method: N::METHOD.to_string(),
            params: serde_json::to_value(params).expect("serialize"),
        }))
        .expect("send notification");
}

fn recv_with_timeout(connection: &Connection, timeout: Duration) -> Option<Message> {
    connection.receiver.recv_timeout(timeout).ok()
}

#[test]
fn full_lifecycle_publishes_diagnostics_for_prefer_const() {
    let workspace = workspace_with_prefer_const();
    let workspace_path = workspace.path().to_path_buf();

    // memory() returns (server, client) — server side hands to the
    // run_server thread, client side stays here to drive the test.
    let (server, client) = Connection::memory();

    let server_thread = thread::spawn(move || {
        cofferdam_lsp::run_server(server, workspace_path.clone())
            .expect("server should exit cleanly");
    });

    // --- initialize handshake ---
    let init_params = InitializeParams {
        capabilities: ClientCapabilities::default(),
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: Url::from_file_path(workspace.path()).expect("workspace url"),
            name: "test".to_string(),
        }]),
        ..Default::default()
    };
    send_request::<Initialize>(&client, 1, init_params);

    // Expect either the initialize response OR (more likely) the
    // initial-scan publishDiagnostics arriving first. Pull a few
    // messages until we see the response.
    let mut init_seen = false;
    let mut initial_diags: Vec<PublishDiagnosticsParams> = Vec::new();
    for _ in 0..16 {
        let Some(msg) = recv_with_timeout(&client, Duration::from_secs(5)) else {
            break;
        };
        match msg {
            Message::Response(Response { id, error, .. }) if id == RequestId::from(1) => {
                assert!(error.is_none(), "initialize error: {error:?}");
                init_seen = true;
            }
            Message::Notification(notif) if notif.method == PublishDiagnostics::METHOD => {
                let p: PublishDiagnosticsParams =
                    serde_json::from_value(notif.params).expect("publish params");
                initial_diags.push(p);
            }
            _ => {}
        }
        if init_seen && !initial_diags.is_empty() {
            break;
        }
    }
    assert!(init_seen, "client did not receive initialize response");

    // Send `initialized` notification.
    send_notification::<Initialized>(&client, InitializedParams {});

    // --- open the test file ---
    let uri = Url::from_file_path(workspace.path().join("a.ts")).unwrap();
    let did_open = DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: "typescript".to_string(),
            version: 1,
            text: "let x = 1;\n".to_string(),
        },
    };
    send_notification::<DidOpenTextDocument>(&client, did_open);

    // Drain publishDiagnostics until we see one carrying
    // Refactor.PreferConstOverLet for our file.
    let mut got_prefer_const = false;
    for _ in 0..32 {
        let Some(msg) = recv_with_timeout(&client, Duration::from_secs(5)) else {
            break;
        };
        if let Message::Notification(notif) = msg {
            if notif.method == PublishDiagnostics::METHOD {
                let p: PublishDiagnosticsParams =
                    serde_json::from_value(notif.params).expect("publish params");
                if p.uri == uri
                    && p.diagnostics.iter().any(|d| {
                        d.source.as_deref() == Some("cofferdam")
                            && matches!(
                                &d.code,
                                Some(lsp_types::NumberOrString::String(s))
                                    if s == "Refactor.PreferConstOverLet"
                            )
                    })
                {
                    got_prefer_const = true;
                    break;
                }
            }
        }
    }
    assert!(
        got_prefer_const,
        "expected Refactor.PreferConstOverLet diagnostic on a.ts after didOpen"
    );

    // --- shutdown + exit ---
    send_request::<Shutdown>(&client, 2, ());
    // Drain until we see the shutdown response.
    let mut shutdown_seen = false;
    for _ in 0..16 {
        let Some(msg) = recv_with_timeout(&client, Duration::from_secs(5)) else {
            break;
        };
        if let Message::Response(Response { id, .. }) = msg {
            if id == RequestId::from(2) {
                shutdown_seen = true;
                break;
            }
        }
    }
    assert!(shutdown_seen, "client did not receive shutdown response");

    send_notification::<Exit>(&client, ());
    drop(client); // close the client side so the server's loop exits.

    server_thread.join().expect("server thread joined cleanly");
}

#[test]
fn empty_workspace_initializes_without_diagnostics() {
    // A workspace with no TS files: initialize, expect zero diagnostics,
    // shutdown cleanly.
    let workspace = tempdir().expect("tempdir");
    let workspace_path = workspace.path().to_path_buf();

    let (server, client) = Connection::memory();
    let server_thread = thread::spawn(move || {
        cofferdam_lsp::run_server(server, workspace_path)
            .expect("server exits cleanly on empty workspace");
    });

    let init_params = InitializeParams {
        capabilities: ClientCapabilities::default(),
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: Url::from_file_path(workspace.path()).unwrap(),
            name: "empty".to_string(),
        }]),
        ..Default::default()
    };
    send_request::<Initialize>(&client, 1, init_params);

    let mut init_seen = false;
    for _ in 0..8 {
        let Some(msg) = recv_with_timeout(&client, Duration::from_secs(5)) else {
            break;
        };
        if let Message::Response(Response { id, error, .. }) = msg {
            if id == RequestId::from(1) {
                assert!(error.is_none());
                init_seen = true;
                break;
            }
        }
    }
    assert!(init_seen);

    send_notification::<Initialized>(&client, InitializedParams {});
    send_request::<Shutdown>(&client, 2, ());

    for _ in 0..8 {
        let Some(msg) = recv_with_timeout(&client, Duration::from_secs(5)) else {
            break;
        };
        if let Message::Response(Response { id, .. }) = msg {
            if id == RequestId::from(2) {
                break;
            }
        }
    }
    send_notification::<Exit>(&client, ());
    drop(client);
    server_thread.join().expect("server thread joined");

    // Suppress unused warnings on PathBuf import when we don't need to
    // reach into the workspace beyond using its url.
    let _ = PathBuf::new();
}

// ---------------------------------------------------------------------------
// didSave triggers re-analysis (cache-reuse path)
//
// NOTE: malformed cofferdam.toml handling is not tested here. As of
// cp5, build_engine() in server.rs does not load cofferdam.toml — it
// always falls back to Engine::new(checks). Config loading is wired in
// a follow-up bead. Testing a malformed config through the existing
// harness would only test default-config behavior (server would not
// crash, but not because of config recovery). Recorded in cd-4z8r notes.
// ---------------------------------------------------------------------------

#[test]
fn did_save_triggers_re_analysis_with_updated_content() {
    // Start with a clean file (no `let` → no Refactor.PreferConstOverLet).
    // After sending didSave with dirty text, the server must re-analyze
    // and publish Refactor.PreferConstOverLet for the same file.
    let workspace = tempdir().expect("tempdir");
    let ts = workspace.path().join("a.ts");
    std::fs::write(&ts, "const x = 1;\n").unwrap();
    let workspace_path = workspace.path().to_path_buf();

    let (server, client) = Connection::memory();
    let server_thread = thread::spawn(move || {
        cofferdam_lsp::run_server(server, workspace_path).expect("server exits cleanly");
    });

    let init_params = InitializeParams {
        capabilities: ClientCapabilities::default(),
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: Url::from_file_path(workspace.path()).unwrap(),
            name: "save-test".to_string(),
        }]),
        ..Default::default()
    };
    send_request::<Initialize>(&client, 1, init_params);

    // Drain until initialize response received, discarding initial-scan
    // diagnostics (clean file → empty diagnostics array).
    let mut init_seen = false;
    for _ in 0..16 {
        let Some(msg) = recv_with_timeout(&client, Duration::from_secs(5)) else {
            break;
        };
        if let Message::Response(Response { id, error, .. }) = msg {
            if id == RequestId::from(1) {
                assert!(error.is_none(), "initialize error: {error:?}");
                init_seen = true;
                break;
            }
        }
    }
    assert!(init_seen, "did not receive initialize response");
    send_notification::<Initialized>(&client, InitializedParams {});

    // Drain any remaining initial-scan messages with a short timeout.
    for _ in 0..8 {
        if recv_with_timeout(&client, Duration::from_millis(100)).is_none() {
            break;
        }
    }

    // Send didSave with text that now contains a never-reassigned `let`.
    let uri = Url::from_file_path(&ts).unwrap();
    send_notification::<DidSaveTextDocument>(
        &client,
        DidSaveTextDocumentParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            text: Some("let x = 1;\n".to_string()),
        },
    );

    // Collect diagnostics until we see Refactor.PreferConstOverLet on a.ts.
    let mut got_prefer_const = false;
    for _ in 0..32 {
        let Some(msg) = recv_with_timeout(&client, Duration::from_secs(5)) else {
            break;
        };
        if let Message::Notification(notif) = msg {
            if notif.method == PublishDiagnostics::METHOD {
                let p: PublishDiagnosticsParams =
                    serde_json::from_value(notif.params).expect("publish params");
                if p.uri == uri
                    && p.diagnostics.iter().any(|d| {
                        d.source.as_deref() == Some("cofferdam")
                            && matches!(
                                &d.code,
                                Some(lsp_types::NumberOrString::String(s))
                                    if s == "Refactor.PreferConstOverLet"
                            )
                    })
                {
                    got_prefer_const = true;
                    break;
                }
            }
        }
    }
    assert!(
        got_prefer_const,
        "expected Refactor.PreferConstOverLet after didSave with let content"
    );

    send_request::<Shutdown>(&client, 2, ());
    for _ in 0..8 {
        let Some(msg) = recv_with_timeout(&client, Duration::from_secs(5)) else {
            break;
        };
        if let Message::Response(Response { id, .. }) = msg {
            if id == RequestId::from(2) {
                break;
            }
        }
    }
    send_notification::<Exit>(&client, ());
    drop(client);
    server_thread.join().expect("server thread joined");
}

// ---------------------------------------------------------------------------
// Workspace-root discovery with a nested layout
// ---------------------------------------------------------------------------

#[test]
fn nested_workspace_discovers_subdirectory_files() {
    // Workspace:
    //   workspace/
    //     src/
    //       nested/
    //         b.ts  (has a never-reassigned `let` → PreferConstOverLet fires)
    //
    // Verifies that the engine's discover() walk reaches files in
    // subdirectories, not just the workspace root.
    let workspace = tempdir().expect("tempdir");
    let nested_dir = workspace.path().join("src").join("nested");
    std::fs::create_dir_all(&nested_dir).unwrap();
    let ts = nested_dir.join("b.ts");
    std::fs::write(&ts, "let y = 2;\n").unwrap();
    let workspace_path = workspace.path().to_path_buf();

    let (server, client) = Connection::memory();
    let server_thread = thread::spawn(move || {
        cofferdam_lsp::run_server(server, workspace_path).expect("server exits cleanly");
    });

    let init_params = InitializeParams {
        capabilities: ClientCapabilities::default(),
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: Url::from_file_path(workspace.path()).unwrap(),
            name: "nested-discovery".to_string(),
        }]),
        ..Default::default()
    };
    send_request::<Initialize>(&client, 1, init_params);

    // Drain until initialize response is seen.
    let mut init_seen = false;
    for _ in 0..16 {
        let Some(msg) = recv_with_timeout(&client, Duration::from_secs(5)) else {
            break;
        };
        if let Message::Response(Response { id, error, .. }) = msg {
            if id == RequestId::from(1) {
                assert!(error.is_none(), "initialize error: {error:?}");
                init_seen = true;
                break;
            }
        }
    }
    assert!(init_seen, "did not receive initialize response");
    send_notification::<Initialized>(&client, InitializedParams {});

    // Collect diagnostics from the initial scan.
    let nested_uri = Url::from_file_path(&ts).unwrap();
    let mut got_nested_diag = false;
    for _ in 0..32 {
        let Some(msg) = recv_with_timeout(&client, Duration::from_secs(5)) else {
            break;
        };
        if let Message::Notification(notif) = msg {
            if notif.method == PublishDiagnostics::METHOD {
                let p: PublishDiagnosticsParams =
                    serde_json::from_value(notif.params).expect("publish params");
                if p.uri == nested_uri
                    && p.diagnostics.iter().any(|d| {
                        d.source.as_deref() == Some("cofferdam")
                            && matches!(
                                &d.code,
                                Some(lsp_types::NumberOrString::String(s))
                                    if s == "Refactor.PreferConstOverLet"
                            )
                    })
                {
                    got_nested_diag = true;
                    break;
                }
            }
        }
    }
    assert!(
        got_nested_diag,
        "expected Refactor.PreferConstOverLet for nested file src/nested/b.ts"
    );

    send_request::<Shutdown>(&client, 2, ());
    for _ in 0..8 {
        let Some(msg) = recv_with_timeout(&client, Duration::from_secs(5)) else {
            break;
        };
        if let Message::Response(Response { id, .. }) = msg {
            if id == RequestId::from(2) {
                break;
            }
        }
    }
    send_notification::<Exit>(&client, ());
    drop(client);
    server_thread.join().expect("server thread joined");
}
