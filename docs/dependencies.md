# Dependency rationale

Silent Nexus keeps its dependency surface deliberately small and audited
(`deny.toml` bans `openssl` and `git2` in favor of pure-Rust/rustls and native
implementations). Every third-party crate below earns its place; the rationale
is recorded so future contributors can weigh replacements.

| Crate | Why it is here | Why not roll our own |
|---|---|---|
| `tokio` | Async runtime for concurrent I/O (model streaming, sandbox exec, MCP stdio). | Writing a correct multi-threaded async runtime is a project unto itself. |
| `reqwest` (rustls-tls) | HTTP client for providers and web tools; rustls avoids OpenSSL. | TLS + HTTP/1.1/2 + redirects are error-prone to reimplement securely. |
| `rusqlite` (bundled) | Embedded SQLite for all durable state; bundled = no system dep. | SQLite is the correct embedded store; reimplementation is infeasible. |
| `serde` / `serde_json` / `toml` | Serialization for config, state, JSON-RPC, tool I/O. | The de-facto standard; hand-rolled parsers would be less safe. |
| `jsonschema` | Validates tool arguments against each tool's JSON Schema — a core safety boundary. | Correct JSON Schema validation is subtle; we must not get it wrong. |
| `schemars` | Derives the config JSON schema (`snx config schema`). | Keeps schema in sync with types automatically. |
| `ratatui` + `crossterm` | The TUI and terminal control. | Terminal handling across platforms is a large, thankless surface. |
| `clap` (+ `clap_complete`) | CLI parsing and shell completions. | Argument parsing with good UX and completions is substantial. |
| `regex` / `globset` / `ignore` | Redaction patterns, glob matching, gitignore-aware walking. | `ignore` powers ripgrep; matching its correctness by hand is wasteful. |
| `portable-pty` | Interactive terminal (PTY) tool. | Cross-platform PTY handling is intricate. |
| `libc` | `setrlimit`/`getrlimit`, namespaces, `setsid` for the process sandbox. | Direct syscalls are exactly what a sandbox needs. |
| `sha2` / `hex` | Content-addressed artifacts and file hashing. | Cryptographic primitives must come from a reviewed implementation. |
| `directories` | Locating per-user config/state dirs across platforms. | Platform path conventions are fiddly and easy to get wrong. |
| `tracing` (+ subscriber/appender) | Structured logging to files (never secrets, never stdout). | Structured, leveled, async-aware logging. |
| `chrono` | RFC-3339 timestamps for audit and state. | Correct calendar/time handling. |
| `async-trait` | Trait objects for providers, tools, sandbox backends, approvers. | Ergonomic async traits until the language feature stabilizes. |
| `thiserror` | Typed error enums with good messages. | Reduces boilerplate; no runtime cost. |

Dev-only: `tempfile`, `wiremock`, `insta`, `pretty_assertions` — testing
scaffolding, not shipped in the binary.

Run `cargo tree` to inspect the full transitive graph and `cargo deny check`
(config in `deny.toml`) to enforce the license/advisory/ban policy.
