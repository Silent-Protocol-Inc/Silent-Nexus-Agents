# Historical 0.2 TUI fix goals

> Archived after completion. This document describes the pre-1.0 TUI repair
> effort and is not an active release plan.

## Original specification

> Task spec stored 2026-07-16. Refer to this document during all build/fix work
> on this effort. Task: FIX SILENT NEXUS TUI, INTERACTIVE COMMANDS, LOGIN,
> MODEL PROVIDERS, AND CYBERPUNK DESIGN.

## Scope

Work only inside `Silent-Nexus/`. Inspect the current implementation, identify
why the TUI command system and interactive flows are incomplete, and fully
repair them. Do not rebuild from scratch unless a subsystem is fundamentally
unusable. Do not modify unrelated projects, repos, services, websites, bots, or
production infrastructure.

Target quality bar: polished, user-friendly, modern terminal harness comparable
in usability to Hermes Agent / OpenCode / Odysseus — but a completely original
Silent Protocol identity. Do not copy their source, branding, layouts, command
names, assets, or proprietary UI.

## Current problems

1. Commands/tools only work pre-TUI (`snx goal ...`, `snx status ...`, `snx model ...`).
2. Inside the full-screen TUI (`snx`), slash commands are missing or placeholders.
3. All relevant commands must work inside the TUI: `/status /goal /goals /resume
   /pause /cancel /model /models /login /logout /btw /tools /agents /agent
   /sandbox /mcp /memory /skills /context /compact /diff /changes /revert /test
   /logs /config /theme /help /exit`.
4. Tools, agent actions, model selection, sessions, goals, args must be usable
   interactively without leaving the TUI.
5. `/login` lacks a polished interactive provider authentication system.
6. `/model` lacks a complete interactive provider/model selection interface.
7. Codex auth must use isolated credentials that never touch an existing Codex
   CLI login.
8. TUI is not sufficiently cyberpunk/polished/premium/distinctive.
9. Interaction model must feel like a professional agent harness.

## Primary outcome

`snx` alone must support the full workflow in-TUI: create/resume goals, switch
models, authenticate providers, inspect status, select agents, manage sessions,
execute tools, inspect changes, review permissions, manage memory, MCP, sandbox
config, run tests, inspect logs, all slash commands. Non-interactive CLI stays
available for scripting with equivalent behavior.

## First action: audit

Inspect: workspace layout, TUI input handler, slash-command parser, CLI command
handlers, provider/auth abstractions, model config, session/goal persistence,
theme system, `/btw`, duplicated CLI/TUI logic, placeholders/fake values, and
run the app to reproduce broken behavior. Plan before major changes.

## Shared command architecture

One unified command registry powering both CLI and TUI — no duplicate TUI-only
business logic. Each `CommandDefinition`: name, aliases, description, usage,
category, interactive, non_interactive, requires_confirmation, argument schema,
availability, modal-vs-immediate, permissions, autocomplete metadata, handler.
`snx goal create "..."` and `/goal create ...` must share the same handler.

## TUI command input

Three modes: natural-language message, slash command, shell/tool shortcut
(where supported). Parser must: trim whitespace, quoted args, escapes, flags,
subcommands, multiline NL input, distinguish slash commands from URLs, helpful
parse errors, never crash on malformed input, command history + arrow keys,
autocomplete, fuzzy matching, "Did you mean /goal?" suggestions for typos.

## Command palette

Trigger: `/` on empty input, or Ctrl+K. Provides fuzzy search, categories,
descriptions, keyboard nav, argument hints, disabled-state explanations,
recently used, aliases, provider/model/tool/goal/session quick actions. Typing
`/go` shows `/goal`, `/goals`. Commands needing more input get inline argument
completion or an interactive modal. No syntax memorization required.

## Required interactive commands

Fully support at minimum: `/help /new /clear /exit /status /model /models
/login /logout /agent /agents /goal /goals /plan /resume /pause /cancel
/context /compact /memory /skills /mcp /tools /permissions /sandbox /diff
/changes /revert /test /logs /config /theme /about /btw`. Preserve `/btw`'s
intended behavior. No removals/renames without alias/migration. For each:
verify behavior, arg parsing, error handling, empty state, keyboard-only
operation, persistence, real data; add tests.

## /status

Real status panel: session, goal + state, active agent, provider, model, auth
state, endpoint health, sandbox backend + isolation, internet access, MCP
count/health, workspace, git branch, modified files, context usage, tool-call
count, runtime, pending approvals, last error, CPU/mem where available. No
static/invented values. `r` refreshes.

## /goal and /resume

`/goal` opens interactive interface: create/view/list/resume/pause/cancel goal,
inspect plan, inspect evidence, verify, export. Guided create form: objective,
acceptance criteria, constraints, allowed/prohibited paths, model, agent, tool
permissions, sandbox mode, network policy, step budget, runtime budget. Fast
path: `/goal Fix X` creates a draft goal and opens its plan.

`/resume` lists resumable goals/sessions/safe interrupted tool runs/paused
workflows with title, status, last activity, model, workspace, completed/pending
steps, last error. Never resume a side effect twice — use idempotency and
checkpoint state.

## /login interactive authentication

Provider list (only those actually supported; label unimplemented ones as
unavailable/experimental): Codex, OpenAI-compatible API, Anthropic, Google,
OpenRouter, Ollama, llama.cpp, Custom endpoint, Configured providers. Each
entry shows: name, type, auth method, auth state, endpoint, local?, credentials
stored?, device login supported?, API key required?. Keyboard nav + details
panel.

### Codex login flow

Menu: device login; use existing Codex CLI auth; import existing auth into an
isolated Silent Nexus profile; API key/supported token; inspect current S//N
Codex login; logout from S//N Codex profile; cancel. Only expose methods that
work with the installed Codex CLI — do not invent protocols.

Device login: start supported flow, show verification URL + device code, allow
copy, waiting state, cancellation, detect success, store only in isolated
storage, verify, display account without secrets. TUI stays responsive.

Existing login detection: detect Codex CLI installed + working auth. Never
modify user's Codex CLI auth files. On detection offer: use temporarily without
copying / copy into isolated profile / create new isolated login / cancel.

Isolation: dedicated profile dir (e.g. `~/.config/silent-nexus/auth/codex/`,
via platform abstractions, not hard-coded Linux paths). Must not overwrite
Codex config, change default profile, log user out, change env globally, or
expose raw credentials. Restrictive fs permissions; OS keyring where feasible;
documented fallback limits; profile removal. Set isolated config dir/env only
on the Codex child process (e.g. `CODEX_HOME`) — verify from the installed CLI
how it actually handles auth dirs; do not assume env vars. If clean isolation
is unsupported, implement a safe wrapper and document guarantees/limits.

Import requires explicit confirmation showing source/destination and the
promise that the original is unmodified. Redact sensitive paths. Never display
token contents.

## Provider credential storage

Unified secure credential service: OS keyring where available, encrypted or
restricted fallback, per-provider profiles, isolated credentials, status,
deletion, verification, profile names, multiple accounts where feasible.
Actions: `/login /logout /auth status /auth profiles /auth remove`. Secrets
never in logs, crash reports, model context, command history, diagnostics, or
status panels; Debug impls must not leak them.

## /model interactive provider and model menu

Not a plain text list. Provider list (only real support, mark unsupported):
Ollama, llama.cpp, Codex, OpenAI-compatible, OpenRouter, Anthropic, Google,
Custom endpoint, configured local/remote providers. Each row: terminal-safe
marker, name, local/remote, auth state, endpoint health, model count, active
profile, current selection, setup requirement. Example:

```
● Ollama          Local    Connected      6 models
○ Codex           Remote   Login required
```

Not color-only. Selecting a provider: inspect connection + auth, load models,
show capabilities, setup actions. Actions: select, connect, login, enter API
key, configure endpoint, refresh models, test connection, view config, set
default, remove config, back. Show "Device login available" / "API key
required" / "No authentication required" as appropriate. Custom endpoints:
URL, key, headers, discovery endpoint, manual model, context limit, streaming,
tool-call format, TLS validation, timeout — validate URLs, safe defaults.

### Ollama flow

Detect local endpoint, test connectivity, list installed models with sizes/
capabilities, select, refresh, configure other endpoint, tool-calling support,
context config, persist. If down: Retry / Configure endpoint / View startup
instructions / Back. Never install/start Ollama or download models without
explicit approval; pulls show id, size, disk availability, confirmation,
progress, cancellation.

### llama.cpp flow

Detect configured/common endpoints, test `/v1/models`, load models, inspect
tool-call/context/streaming support, custom server config, persist. Don't
require owning the llama.cpp process; process management stays optional and
permission-controlled.

### Custom endpoint flow

Guided form: profile name, protocol (OpenAI-compatible / Ollama-compatible /
llama.cpp-compatible / custom JSON adapter), base URL, auth type, key/token,
headers, model ID, model-list endpoint, chat endpoint, tool-call support,
structured output, streaming, context limit, output limit, timeout, TLS
verification. Validate before save. Test connection / Discover models / Save /
Save and select / Cancel. No API keys in plain config files.

### Model details

Show: name, provider, local/remote, context limit, output limit, tool calling,
structured output, reasoning, multimodal, est. memory, quantization, active
status, health, profile. Actions: select for session, set default, assign to
agent/goal, test model, view raw details. Model test = minimal safe prompt
reporting connection, first-token latency, total latency, streaming, structured
response, tool-call test.

### Status bar state

Always show active provider/model: `MODEL  Ollama / qwen3:4b`; if none,
`MODEL  Not configured` (opens /model); if auth missing, `MODEL  Codex / Login
required`. Never silently fall back to a paid provider — fallback only per
explicit user configuration.

### /login ↔ /model relationship

`/login` manages auth profiles; `/model` manages endpoints/models/selection.
`/model` offers "Login to provider" when auth missing; `/login` offers "Choose
model" after success.

## TUI visual redesign — cyberpunk Silent Protocol

Feel: futuristic, technical, premium, fast, dark, sharp, readable, cohesive,
distinctive. No generic gray dashboard, no visual noise. Brand: `SILENT//NEXUS`
(compact `S//N`), optional startup line `SILENT//NEXUS :: LOCAL INTELLIGENCE
ONLINE`. No big ASCII art.

Semantic theme tokens (no hard-coded styling): near-black background, graphite/
deep blue-black panels, neon cyan primary, ultraviolet secondary, acid green
success, amber warning, signal red failure, steel gray muted, cyan-violet glow
selection. Capability detection: truecolor / 256 / 16 / no-color. Never
color-only status.

Restrained cyberpunk details: thin neon separators, segmented status bars,
compact circuit-like borders, selected-panel glow, scan indicator, subtle
activity markers, agent pulse, streamed-token indicator, cybernetic panel
labels, high-contrast diffs, compact diagnostic glyphs. Avoid flashing, glitch
overload, unreadable symbols, gradient overuse, giant logos, fake loaders.

Responsive layout — wide: header (SESSION/MODEL/SANDBOX), conversation +
right ACTIVE CONTEXT panel (goal/plan/files/approvals/runtime), input line
("Type a message or / for commands"), footer (WORKSPACE/BRANCH/PROVIDER/
TOKENS/NETWORK/STATUS). Medium: right panel becomes tabs. Narrow: single
column. No overlap or overflow ever.

Polished real-data views for: chat, goals, sessions, login, providers, models,
agents, tools, approvals, diff, context, sandbox, MCP, memory, skills, logs,
configuration, help. No fake data.

## Modals and reusable components

Searchable selection menu, confirmation dialog, secure text input, form,
progress dialog, tab view, status badge, toast, error panel, help panel,
credential/model/provider/session/goal pickers. Secure inputs: mask secrets,
skip history/logs, safe paste, clear buffers where practical. Menu keys:
Up/Down or j/k, Enter, Esc back, Tab focus, Ctrl+K palette, ? contextual help.
Document real keybindings in /help.

## Responsiveness and async

Never block the event loop during device login, provider discovery, model list
loading, network/model tests, goal/tool execution, indexing, sandbox startup.
Background tasks with cancellation, progress, timeout, status updates, proper
channels, stale-result protection (an old completed request must not overwrite
a newer selection).

## Error handling

Actionable errors with reason + actions (Retry / alternative / view logs /
cancel), never exposing secrets. Distinguish: connection refused, DNS, TLS,
invalid credentials, expired login, unsupported endpoint, malformed response,
no models, timeout, rate limit, permission denied.

## Persistence

Persist: active provider/model, provider profiles, auth profile references,
theme, layout prefs, safe command history, recent commands, active goal/
session, panel state, model assignments, provider health cache, last selected
menu items. No secrets in state files. Versioned config migrations.

## Testing requirements

- Parser: slash detection, quoted args, aliases, malformed syntax, unknown
  suggestions, empty input, URLs, multiline, history, autocomplete.
- Shared services: TUI and CLI hit the same behavior for status, goal, resume,
  model, login, logout, sandbox, tools.
- Login: Codex unavailable / installed-but-logged-out / existing detected /
  isolated copy / cancelled import / device success / device timeout / logout
  removes only S//N profile / original Codex profile unchanged / no secrets in
  logs.
- Model: Ollama detected/unavailable, llama.cpp detected, API key required,
  device login available, custom endpoint success/failure, discovery, selection
  persistence, health refresh.
- TUI: modal nav, narrow/medium/wide terminals, palette, focus, async ops,
  cancellation, resize, no-color, keyboard-only.
Use mock providers and temp dirs; never test against real user credentials.

## Validation

```
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --release
```

Then manually run `snx` and exercise `/status /help /goal /goals /resume
/login /model /tools /sandbox /btw /exit` — each must execute real
functionality. Test Codex isolation with temp mock profiles first; confirm the
original Codex auth is untouched.

## No-placeholder requirement

No: non-functional menus, static provider lists, fake login success, fake
models/status, unexplained disabled commands, hard-coded auth state, mock UI
wired to production code, empty slash handlers, TODO command impls, duplicated
CLI/TUI business logic, plain-text secrets, hidden fallback to paid APIs. If a
provider can't be fully supported, show its real implementation status.

## Completion criteria

1. `snx` launches a functional full-screen TUI.
2. Existing commands work inside the TUI.
3. Autocomplete + command palette work.
4. `/status` shows real state.
5. `/goal` interactive creation/management.
6. `/resume` resumes real goals/sessions.
7. `/login` interactive provider menu.
8. Codex safest available device/existing-login flows.
9. S//N Codex credentials isolated from Codex CLI credentials.
10. `/model` interactive provider/model menu.
11. Ollama + llama.cpp discovery work.
12. Custom endpoints configurable and testable.
13. API-key requirements clearly shown.
14. Device-login availability clearly shown.
15. Active provider/model visible in TUI.
16. Polished original cyberpunk design.
17. Responsive during async operations.
18. No unrelated project modified.
19. Tests pass.
20. Release build succeeds.

## Required final report

Root causes; files/modules changed; shared command architecture; slash commands
implemented; palette behavior; /goal; /resume; /login provider list; Codex auth
methods; exact Codex isolation mechanism; confirmation existing Codex login
untouched; /model interface; Ollama; llama.cpp; custom endpoints; secure
credential storage; theme/layouts; responsive behavior; tests + exact results;
release-build result; known limitations; exact commands to launch/test.

Do not claim the interface is fixed until all required interactive flows work
from inside `snx`.
