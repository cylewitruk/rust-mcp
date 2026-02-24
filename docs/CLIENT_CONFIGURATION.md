# Client Configuration Guide

This guide keeps client-specific MCP setup details out of the main README.

Use this doc when configuring:

- Copilot Chat in VS Code
- Claude Code CLI
- Claude Code in VS Code
- Codex CLI
- Codex in VS Code
- Gemini CLI
- Cursor

## Two transport patterns

Most clients fit one of these patterns.

### Pattern A: Direct HTTP(S) MCP (preferred)

Use when the client supports remote MCP servers directly.

- MCP URL: `http://127.0.0.1:43173/mcp` (local)
- MCP URL: `https://mcp.example.com/mcp` (remote/TLS)

Generic config shape:

```json
{
  "mcpServers": {
    "rust-mcp-http": {
      "url": "http://127.0.0.1:43173/mcp"
    }
  }
}
```

### Pattern B: stdio launcher via `rust-mcp-stdio`

Use when the client only supports process-launched MCP servers.

- command: `cargo`
- args:
  - `run`
  - `-p`
  - `rust-mcp-stdio`
  - `--`
  - `--mcp-url`
  - `http://127.0.0.1:43173/mcp`

Generic config shape:

```json
{
  "mcpServers": {
    "rust-mcp-stdio": {
      "command": "cargo",
      "args": [
        "run",
        "-p",
        "rust-mcp-stdio",
        "--",
        "--mcp-url",
        "http://127.0.0.1:43173/mcp"
      ],
      "env": {
        "MCP_PREFLIGHT_SCHEMA": "true"
      }
    }
  }
}
```

For HTTPS upstream, only change the `--mcp-url` value.

## Popular clients: practical mapping

Because client config schemas evolve quickly, field names may vary. Map each client to one of the two patterns above.

## Copilot Chat in VS Code

- Prefer Pattern A (direct HTTP(S)) when available.
- Use Pattern B for process-launch workflows.
- Map the client’s server URL field to `/mcp`.

## Claude Code CLI

- If remote MCP endpoints are supported in your installed version, use Pattern A.
- Otherwise configure a process-launched server using Pattern B.

## Claude Code in VS Code

- Use Pattern A if the extension supports remote MCP server URLs.
- Fall back to Pattern B when stdio process launch is the available mode.

## Codex CLI

- Use Pattern A for remote MCP support.
- Use Pattern B for local process-launch support.

## Codex in VS Code

- Prefer Pattern A (URL-based server config).
- Use Pattern B if your environment requires stdio process launch.

## Gemini CLI

- Use Pattern A when remote MCP URL config is available.
- Use Pattern B when configured as a local stdio command.

## Cursor

- Cursor MCP server definitions typically map cleanly to Pattern A (`url`) or Pattern B (`command` + `args`).
- If both work in your setup, prefer Pattern A for simpler operations.

## Adapter-specific environment variables

For `rust-mcp-stdio`:

- `MCP_URL` (default `http://127.0.0.1:43173/mcp`)
- `MCP_CONNECT_TIMEOUT_SECS` (default `10`)
- `MCP_REQUEST_TIMEOUT_SECS` (default `120`)
- `MCP_PREFLIGHT_SCHEMA` (default `true`)

## Verification checklist

After configuring any client:

1. Run `ping` and confirm a successful tool response.
2. Run `tools/list` and confirm expected tool inventory appears.
3. Run one representative call, for example `crate.search`.

## Notes

- `rust-mcp` server is HTTP-only.
- `rust-mcp-stdio` is a stdio-to-HTTP adapter and supports all tools via pass-through.
- Keep this doc focused on practical setup patterns; avoid coupling to rapidly changing client UI labels.
