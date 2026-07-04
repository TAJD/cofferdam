# MCP server

`cofferdam-mcp` is a minimal [Model Context Protocol](https://modelcontextprotocol.io) stdio
server exposing one tool, `cofferdam.check`: given a filesystem path (file or directory), it runs
cofferdam's built-in checks against it and returns findings as JSON (the same schema
`cofferdam check --robot` produces). It loads `cofferdam.toml` when one is discoverable upward
from the target path, otherwise runs with default check options.

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

Only `cofferdam.check` ships today. The broader tool set from the originating ticket
(`advise`, `advise_diff`, `explain`, `invariants`) depends on features that don't exist yet in
this codebase (no `advise` subcommand, no invariants-reader API). `cofferdam.explain` was
considered — `CheckMeta.explanation` is trivially reachable per check id via `all_builtins()` —
but was left out of this first slice to keep the initial MCP surface to the one tool the ticket
called out as shippable alone; a follow-up can add it without touching `cofferdam.check`.

No HTTP/SSE transport, no auth, no streaming, no caching — stdio only, stateless, single tool.
