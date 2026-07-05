# MCP server

`cofferdam-mcp` is a minimal [Model Context Protocol](https://modelcontextprotocol.io) stdio
server exposing five tools, all thin wrappers over the same library functions the `cofferdam`
CLI calls — CLI and MCP output are byte-for-byte identical:

- **`cofferdam.check`** — given a filesystem path (file or directory), runs cofferdam's
  built-in checks against it and returns findings as JSON (the same schema
  `cofferdam check --robot` produces).
- **`cofferdam.advise`** — static, per-file architectural advisory (layer membership,
  public-API status, and the rule constraints that apply) without running the engine. Same
  JSON shape as `cofferdam advise --format=json`.
- **`cofferdam.advise_diff`** — pre-flight check: diffs findings between a git ref and the
  working tree, returning `would_fire` / `would_clear` sets. Same shape as
  `cofferdam advise --diff <ref>`.
- **`cofferdam.explain`** — a check's metadata and prose explanation by ID (built-in or
  plugin-declared). Same shape as `cofferdam explain <id> --robot`.
- **`cofferdam.invariants`** — the parsed `cofferdam.invariants.toml` (layers, public_api,
  boundaries, invariants) as JSON, discovered upward from a given path. No CLI equivalent
  exists — this is MCP-only.

All five load `cofferdam.toml` / `cofferdam.invariants.toml` when discoverable upward from the
target path, otherwise run with default options.

Build the binary with `cargo build --release -p cofferdam-mcp`; the executable lands at
`target/release/cofferdam-mcp` (`.exe` on Windows).

## Example client config

```json
{
  "mcpServers": {
    "cofferdam": {
      "command": "/absolute/path/to/target/release/cofferdam-mcp"
    }
  }
}
```

This is the shape used by Claude Desktop's `claude_desktop_config.json` and Claude Code's stdio
MCP server config; point `command` at your built binary.

## Scope

No HTTP/SSE transport, no auth, no streaming, no caching — stdio only, stateless.
