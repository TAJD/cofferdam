//! `cofferdam-mcp` — minimal MCP (Model Context Protocol) stdio server.
//!
//! Exposes five tools — `cofferdam.check`, `cofferdam.advise`,
//! `cofferdam.advise_diff`, `cofferdam.explain`, `cofferdam.invariants` —
//! all thin wrappers over the same library functions `cofferdam-cli`
//! calls, so CLI and MCP output are byte-for-byte identical (CD-60).
//! Stateless, newline-delimited JSON-RPC 2.0 over stdin/stdout — no
//! HTTP/SSE transport, no auth, no streaming, no caching.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use cofferdam_checks::all_builtins;
use cofferdam_cli::advise;
use cofferdam_cli::advise_diff;
use cofferdam_cli::explain::{self, ExplainOutcome};
use cofferdam_cli::invariants_tool;
use cofferdam_engine::{config, discover, DiscoveryOptions, Engine};
use cofferdam_formatters::json::JsonFormatter;
use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2024-11-05";

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
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
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

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "cofferdam.check",
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
        }),
        json!({
            "name": "cofferdam.advise",
            "description": "Static, per-file architectural advisory: layer membership, public-API status, and the rule constraints that apply — without running the engine.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Files to advise on. Defaults to ['.'] (the whole project) when omitted.",
                    }
                },
            }
        }),
        json!({
            "name": "cofferdam.advise_diff",
            "description": "Pre-flight check: diff findings between a git ref and the working tree, returning would_fire / would_clear sets.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": {
                        "type": "string",
                        "description": "Git ref to diff against, e.g. 'HEAD' or 'origin/main'.",
                    },
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Restrict the diff to these paths. Defaults to all changed files.",
                    }
                },
                "required": ["ref"],
            }
        }),
        json!({
            "name": "cofferdam.explain",
            "description": "Look up a check's metadata and prose explanation by ID (built-in or plugin-declared).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "check_id": {
                        "type": "string",
                        "description": "Dotted check ID, e.g. 'Refactor.PreferConstOverLet'.",
                    },
                    "full": {
                        "type": "boolean",
                        "description": "Include the long-form markdown body. Defaults to false.",
                    }
                },
                "required": ["check_id"],
            }
        }),
        json!({
            "name": "cofferdam.invariants",
            "description": "Return the parsed cofferdam.invariants.toml (layers, public_api, boundaries, invariants) as JSON, discovered upward from the given path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory to start the upward search from. Defaults to '.'.",
                    }
                },
            }
        }),
    ]
}

fn handle_tools_call(params: Option<Value>) -> Result<Value, (i32, String)> {
    let params = params.ok_or((-32602, "missing params".to_string()))?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing tool name".to_string()))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    let outcome = match name {
        "cofferdam.check" => run_check(&arguments),
        "cofferdam.advise" => run_advise(&arguments),
        "cofferdam.advise_diff" => run_advise_diff(&arguments),
        "cofferdam.explain" => run_explain(&arguments),
        "cofferdam.invariants" => run_invariants(&arguments),
        _ => return Err((-32602, format!("unknown tool: {name}"))),
    };

    match outcome {
        Ok(text) => Ok(json!({
            "content": [ { "type": "text", "text": text } ],
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
fn run_check(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or("missing required argument: path")?;

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

fn run_advise(args: &Value) -> Result<String, String> {
    let paths: Vec<PathBuf> = args
        .get("paths")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();

    let advisories = advise::build_advisories(paths, None, false, false, false)
        .map_err(|_| "advise failed: could not resolve config or discover files".to_string())?;

    let envelope = advise::AdviseEnvelope::new(&advisories);
    serde_json::to_string(&envelope).map_err(|e| format!("failed to serialize advisories: {e}"))
}

fn run_advise_diff(args: &Value) -> Result<String, String> {
    let diff_ref = args
        .get("ref")
        .and_then(Value::as_str)
        .ok_or("missing required argument: ref")?
        .to_string();

    let paths: Vec<PathBuf> = args
        .get("paths")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();

    let report = advise_diff::build_diff_report(diff_ref, paths, None, false).map_err(|_| {
        "advise_diff failed: could not resolve config or diff against ref".to_string()
    })?;

    serde_json::to_string(&report).map_err(|e| format!("failed to serialize diff report: {e}"))
}

fn run_explain(args: &Value) -> Result<String, String> {
    let check_id = args
        .get("check_id")
        .and_then(Value::as_str)
        .ok_or("missing required argument: check_id")?;
    let full = args.get("full").and_then(Value::as_bool).unwrap_or(false);

    match explain::explain_check(check_id, None, false, full) {
        ExplainOutcome::Builtin(report) => {
            serde_json::to_string(&report).map_err(|e| format!("failed to serialize: {e}"))
        }
        ExplainOutcome::Plugin(report) => {
            serde_json::to_string(&report).map_err(|e| format!("failed to serialize: {e}"))
        }
        ExplainOutcome::NotFound { error, suggestions } => {
            Err(json!({ "error": error, "suggestions": suggestions }).to_string())
        }
    }
}

fn run_invariants(args: &Value) -> Result<String, String> {
    let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let start = PathBuf::from(path);

    match invariants_tool::load_invariants(&start) {
        Ok(Some(spec)) => {
            serde_json::to_string(&spec).map_err(|e| format!("failed to serialize: {e}"))
        }
        Ok(None) => Err(format!(
            "no cofferdam.invariants.toml found searching upward from {path}"
        )),
        Err(e) => Err(format!("failed to load invariants: {e}")),
    }
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
            "/../../examples/smoke_fixture.ts"
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
        assert_eq!(first["id"], "Design.MaxParameters");
        assert!(first["file"]
            .as_str()
            .unwrap()
            .ends_with("smoke_fixture.ts"));
    }

    #[test]
    fn tools_list_advertises_all_five_tools() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
        })
        .to_string();

        let response = handle_line(&request).expect("tools/list yields a response");
        let response: Value = serde_json::from_str(&response).expect("valid JSON-RPC response");
        let tools = response["result"]["tools"].as_array().expect("tools array");
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "cofferdam.check",
                "cofferdam.advise",
                "cofferdam.advise_diff",
                "cofferdam.explain",
                "cofferdam.invariants",
            ]
        );
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
            "params": { "name": "cofferdam.nonexistent", "arguments": {} }
        })
        .to_string();

        let response = handle_line(&request).expect("response");
        let response: Value = serde_json::from_str(&response).expect("valid JSON");
        assert!(response.get("error").is_some());
    }

    #[test]
    fn tools_call_advise_returns_schema_versioned_envelope() {
        // CD-65 A5: the bare-array shape became a `{schema_version, files}`
        // envelope so `advise` matches `checks.json`'s convention. This is
        // a deliberate breaking change to the JSON output shape.
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/smoke_fixture.ts"
        );
        let request = json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "cofferdam.advise",
                "arguments": { "paths": [fixture] }
            }
        })
        .to_string();

        let response = handle_line(&request).expect("response");
        let response: Value = serde_json::from_str(&response).expect("valid JSON");
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("text content");
        let envelope: Value = serde_json::from_str(text).expect("valid JSON object");
        assert_eq!(envelope["schema_version"].as_u64(), Some(1));
        let files = envelope["files"].as_array().expect("files array");
        assert!(files[0]["path"].as_str().is_some());
    }

    #[test]
    fn tools_call_explain_returns_check_meta() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "cofferdam.explain",
                "arguments": { "check_id": "Refactor.PreferConstOverLet" }
            }
        })
        .to_string();

        let response = handle_line(&request).expect("response");
        let response: Value = serde_json::from_str(&response).expect("valid JSON");
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("text content");
        let report: Value = serde_json::from_str(text).expect("valid JSON");
        assert_eq!(report["id"], "Refactor.PreferConstOverLet");
    }

    #[test]
    fn tools_call_explain_unknown_check_is_error() {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "cofferdam.explain",
                "arguments": { "check_id": "Nonexistent.Check" }
            }
        })
        .to_string();

        let response = handle_line(&request).expect("response");
        let response: Value = serde_json::from_str(&response).expect("valid JSON");
        assert_eq!(response["result"]["isError"], true);
    }
}
