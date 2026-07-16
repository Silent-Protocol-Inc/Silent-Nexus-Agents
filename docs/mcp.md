# Model Context Protocol (MCP)

Silent Nexus is both an MCP **client** (it can use other servers' tools) and an
MCP **server** (it can expose a curated, read-only subset of its own
capabilities to other hosts). Both speak JSON-RPC 2.0 over stdio.

## Client

Registering a server is always an **explicit human action** — the model may at
most *propose* one. Registered servers are **untrusted by default**: their tools
require per-call approval until you mark the server trusted.

```sh
snx mcp add fs-tools --command my-mcp-server --arg --root --arg .
snx mcp list
snx mcp tools fs-tools      # launch + list its tools (with a scrubbed env)
snx mcp trust  fs-tools     # tools may then run under normal policy (still audited)
snx mcp untrust fs-tools
snx mcp remove fs-tools
```

The client launches the server subprocess with a **scrubbed environment**
(`env_clear` + a non-sensitive allowlist), performs the `initialize` handshake,
and **redacts** all tool output before it reaches the model or the UI.

## Server

```sh
snx mcp serve      # speaks JSON-RPC 2.0 on stdio
```

The server exposes only an explicitly curated, read-only set — currently
`nexus.search_code`, `nexus.read_file` (workspace-confined), and
`nexus.project_structure`. Destructive, privileged, and terminal capabilities
are **never** exposed. The exposed set is hand-written, not auto-derived from the
tool registry, so nothing dangerous can leak by accident.
