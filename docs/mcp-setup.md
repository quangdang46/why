# MCP setup

This guide documents the **currently implemented** `why` MCP server surface.

## What exists today

- Start the server with `why mcp`
- Transport is newline-delimited JSON-RPC 2.0 over stdio
- Implemented tools:
  - `why_symbol`
  - `why_split`
  - `why_time_bombs`
  - `why_hotspots`
  - `why_coupling`

## Important current behavior

The current MCP server is narrower than the long-term `PLAN.md` vision:

- `why_symbol` currently returns archaeology JSON from the local history pipeline
- the `no_llm` argument is accepted, but the current MCP path does not run the synthesized `WhyReport` formatter
- tools such as `why_diff`, `why_health`, `why_annotate`, and config-oriented MCP calls are **not** implemented yet

If you want the full CLI query flow, use the normal CLI directly outside MCP.

## Prerequisites

Make sure the `why` binary is on your `PATH`.

Quick check:

```bash
why --help
why mcp
```

If `why mcp` starts successfully, it will wait for JSON-RPC requests on stdin.

## Claude Code

Add the server to your Claude Code MCP settings:

```json
{
  "mcpServers": {
    "why": {
      "command": "why",
      "args": ["mcp"]
    }
  }
}
```

After Claude Code reloads its MCP configuration, the `why` tools should become available from the MCP server.

Suggested Claude Code usage:

- use MCP when you want `why` available as an editor-integrated tool
- prefer the normal `why ...` CLI when you want the richer human-facing query flow described in `README.md`
- before deleting or heavily refactoring unfamiliar code, start with `why_symbol` or the CLI equivalent on the exact symbol you plan to touch
- if the code looks historically messy, follow up with `why_split`, `why_coupling`, or the CLI `--team`/`--coupled` flow as appropriate

## Cursor

Cursor uses the same basic stdio server shape. Add an MCP server entry that runs:

```json
{
  "mcpServers": {
    "why": {
      "command": "why",
      "args": ["mcp"]
    }
  }
}
```

If your Cursor setup stores MCP config in a different wrapper format, keep the same command and args:

- command: `why`
- args: `[`"mcp"`]`

## Neovim

### `mcphub.nvim`

For Neovim setups using `mcphub.nvim`:

```lua
require('mcphub').setup({
  servers = {
    why = {
      command = 'why',
      args = { 'mcp' },
    },
  },
})
```

## Protocol notes

The server currently supports these JSON-RPC methods:

- `initialize`
- `tools/list`
- `tools/call`

Requests are newline-delimited JSON objects over stdin/stdout.

Example `initialize` request:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
```

Example `tools/list` request:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
```

Example `why_symbol` tool call:

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"why_symbol","arguments":{"target":"src/payment.rs:process_payment","no_llm":true}}}
```

## Implemented tool arguments

### `why_symbol`

```json
{
  "target": "src/payment.rs:process_payment",
  "lines": "40:55",
  "no_llm": true
}
```

Notes:
- `target` is required
- `lines` is optional for explicit ranges
- `no_llm` is accepted for compatibility, but the current MCP implementation returns archaeology JSON either way

### `why_split`

```json
{
  "target": "src/auth.rs:authenticate"
}
```

### `why_time_bombs`

```json
{
  "max_age_days": 30
}
```

### `why_hotspots`

```json
{
  "limit": 10
}
```

### `why_coupling`

```json
{
  "target": "src/schema.rs:1",
  "lines": "1:20",
  "limit": 5
}
```

## Troubleshooting

### `why: command not found`

Ensure the installed binary is on your `PATH`.

### Tool is configured but does not appear in the client

Check that:
- the client reloaded MCP configuration
- `why mcp` starts without crashing
- the client is invoking the stdio server from the repository or environment you expect

### Unknown tool errors

The current server only exposes these 5 tools:
- `why_symbol`
- `why_split`
- `why_time_bombs`
- `why_hotspots`
- `why_coupling`

### JSON-RPC parse errors

The server expects one complete JSON request per line.

## Source of truth

The implemented behavior in this document is based on:

- `crates/core/src/cli.rs`
- `crates/mcp/src/lib.rs`
- `tests/integration_mcp.rs`
