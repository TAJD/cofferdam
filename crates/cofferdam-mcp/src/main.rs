//! `cofferdam-mcp` — minimal MCP (Model Context Protocol) stdio server.
//!
//! Exposes a single tool, `cofferdam.check`, which wraps `cofferdam-engine`'s
//! analysis over a filesystem path (file or directory) and returns findings
//! as the same JSON schema `cofferdam check --robot` produces. Stateless,
//! newline-delimited JSON-RPC 2.0 over stdin/stdout — no HTTP/SSE, no auth,
//! no streaming, no caching.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use cofferdam_checks::all_builtins;
use cofferdam_engine::{config, discover, DiscoveryOptions, Engine};
use cofferdam_formatters::json::JsonFormatter;
use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2024-11-05";
const TOOL_NAME: &str = "cofferdam.check";

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(response) = handle_line(trimmed) {
            let _ = writeln!(stdout, "{response}");
            let _ = stdout.flush();
        }
    }
}

/// Parse one JSON-RPC request line and dispatch it. Returns `None` for
/// notifications (no `id`) that need no reply, `Some(json)` otherwise.
fn handle_line(line: &str) -> Option<String> {
    let request: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            ))
        }
    };
    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned();

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "cofferdam-mcp",
                "version": env!("CARGO_PKG_VERSION"),
            }
        })),
        "notifications/initialized" => return None,
        "tools/list" => Ok(json!({ "tools": [tool_definition()] })),
        "tools/call" => handle_tools_call(params),
        _ => {
            let id = id?;
            return Some(error_response(
                id,
                -32601,
                &format!("method not found: {method}"),
            ));
        }
    };

    let id = id?;
    Some(match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }).to_string(),
        Err((code, message)) => error_response(id, code, &message),
    })
}

fn tool_definition() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Run cofferdam's checks against a file or directory and return findings as JSON.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Filesystem path to a file or directory to analyze.",
                }
            },
            "required": ["path"],
        }
    })
}

fn handle_tools_call(params: Option<Value>) -> Result<Value, (i32, String)> {
    let params = params.ok_or((-32602, "missing params".to_string()))?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing tool name".to_string()))?;
    if name != TOOL_NAME {
        return Err((-32602, format!("unknown tool: {name}")));
    }
    let path = params
        .get("arguments")
        .and_then(|a| a.get("path"))
        .and_then(Value::as_str)
        .ok_or((-32602, "missing required argument: path".to_string()))?;

    match run_check(path) {
        Ok(findings_json) => Ok(json!({
            "content": [ { "type": "text", "text": findings_json } ],
            "isError": false,
        })),
        Err(message) => Ok(json!({
            "content": [ { "type": "text", "text": message } ],
            "isError": true,
        })),
    }
}

/// Run cofferdam's built-in checks against `path` (a file or directory) and
/// return the findings rendered by `JsonFormatter::render`. Loads
/// `cofferdam.toml` when one is discoverable upward from `path`; otherwise
/// runs with default check options.
fn run_check(path: &str) -> Result<String, String> {
    let target = PathBuf::from(path);
    if !target.exists() {
        return Err(format!("path does not exist: {path}"));
    }

    let files = discover(&[&target], &DiscoveryOptions::default())
        .map_err(|e| format!("failed to discover files under {path}: {e}"))?;

    let start_dir: &Path = if target.is_dir() {
        &target
    } else {
        target.parent().unwrap_or_else(|| Path::new("."))
    };
    let (project_config, config_path, _diags) =
        config::resolve_with_invariants(None, start_dir, false)
            .map_err(|e| format!("failed to load cofferdam.toml: {e}"))?;

    let engine = match project_config.as_ref() {
        Some(cfg) => {
            let cfg_path = config_path
                .as_deref()
                .unwrap_or_else(|| Path::new("cofferdam.invariants.toml"));
            Engine::with_config(all_builtins(), cfg, cfg_path)
                .map_err(|e| format!("failed to build engine from cofferdam.toml: {e}"))?
        }
        None => Engine::new(all_builtins()),
    };

    let issues = engine
        .analyze(&files)
        .map_err(|e| format!("analysis failed: {e}"))?;

    Ok(JsonFormatter::render(&issues))
}

fn error_response(id: Value, code: i32, message: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trips a `tools/call` request for `cofferdam.check` against
    /// one of the repo's own fixtures and asserts the response's embedded
    /// JSON matches `JsonFormatter::render`'s shape.
    #[test]
    fn tools_call_check_returns_json_formatter_shape() {
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/triple_equals.ts"
        );
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "cofferdam.check",
                "arguments": { "path": fixture }
            }
        })
        .to_string();

        let response = handle_line(&request).expect("tools/call yields a response");
        let response: Value = serde_json::from_str(&response).expect("valid JSON-RPC response");
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);

        let content = &response["result"]["content"][0]["text"];
        let findings_text = content.as_str().expect("text content");
        let findings: Value = serde_json::from_str(findings_text).expect("findings are valid JSON");

        assert!(findings["summary"]["total"].as_u64().unwrap() > 0);
        assert!(!findings["findings"].as_array().unwrap().is_empty());
        let first = &findings["findings"][0];
        assert_eq!(first["id"], "Warning.TripleEquals");
        assert!(first["file"]
            .as_str()
            .unwrap()
            .ends_with("triple_equals.ts"));
    }

    #[test]
    fn tools_list_advertises_cofferdam_check() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
        })
        .to_string();

        let response = handle_line(&request).expect("tools/list yields a response");
        let response: Value = serde_json::from_str(&response).expect("valid JSON-RPC response");
        let tools = response["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "cofferdam.check");
    }

    #[test]
    fn notification_yields_no_response() {
        let request = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        })
        .to_string();
        assert!(handle_line(&request).is_none());
    }

    #[test]
    fn unknown_tool_call_is_reported_as_tool_error() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "cofferdam.explain", "arguments": {} }
        })
        .to_string();

        let response = handle_line(&request).expect("response");
        let response: Value = serde_json::from_str(&response).expect("valid JSON");
        assert!(response.get("error").is_some());
    }
}
