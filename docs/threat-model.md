# Threat model

Silent Nexus is built for the case where **the model itself may be adversarial**
— jailbroken, prompt-injected via web/MCP content, or simply wrong — and the
operator is a legitimate user running it on their own machine against their own
workspace. Safety is a property of the harness, not the model.

## Assets

- The operator's filesystem outside the workspace (SSH keys, cloud creds, dotfiles).
- The workspace itself (integrity of code and data).
- Secrets in environment variables and configuration.
- The operator's network position (internal services, cloud metadata).
- The operator's terminal (control-sequence injection).
- Local compute (denial via runaway processes / output).

## Adversaries and defenses

### 1. A malicious or jailbroken model
The model can emit any bytes. It cannot:
- call an unknown tool (registry lookup fails closed);
- pass invalid arguments (JSON Schema validation rejects them);
- touch a path outside the workspace (`WorkspaceGuard` canonicalizes and checks
  every path, including symlink-swap on write);
- exceed its role's capabilities (role→category gate);
- perform a destructive/external action without at least an approval prompt
  (policy cannot be configured to auto-allow these);
- escape output/time limits (sandbox-enforced);
- exfiltrate a secret through tool output (redaction runs before display/persist).

Verified by the adversarial scenarios in `nexus-agent/tests/loop_scenarios.rs`.

### 2. Prompt injection via web or MCP content
Fetched web pages and MCP tool results are **data, never instructions**. Web
content is wrapped in an explicit untrusted-content banner and never elevated to
system/priority context. The context manager is forbidden from promoting
retrieved content into the load-bearing safety segments.

### 3. SSRF / DNS rebinding / metadata theft
The web tools (`nexus-tools::net_guard`) block, by default: `file://` and other
unsafe schemes, credential-bearing URLs, loopback (unless explicitly enabled),
RFC-1918 and other private ranges, and cloud metadata endpoints
(169.254.169.254 and friends). The destination host is resolved once and the
connection is pinned to the vetted IP, defeating DNS-rebinding. Redirects are
re-validated hop by hop.

### 4. Secret disclosure
Sensitive environment variables are never forwarded into sandboxes and never
logged. The `Redactor` masks PEM keys, bearer tokens, `sk-`/`AKIA`/`gh_`/`xox`
patterns, and credential-bearing URLs, plus the values of any registered secret
env vars, before text is shown or stored. Memory storage **refuses** content
that redaction would alter.

### 5. Terminal injection
All model- and web-derived text passes through `sanitize_terminal`, which strips
CSI/OSC/DCS sequences and drops carriage returns, so hostile output cannot
rewrite your scrollback, retitle your window, or spoof a prompt.

### 6. Resource exhaustion
The sandbox applies wall-clock timeouts, CPU/address-space/process-count rlimits
(process backend), and a hard output cap that terminates the process group when
exceeded.

### 7. Persistence tampering
The SQLite store is created 0600. Skills are declarative and validated to reject
embedded payloads (`#!/`, `eval(`, `base64,`, …); agent-proposed skills are
stored disabled and never auto-enabled. MCP servers are untrusted by default and
require an explicit human action to register or trust.

## Explicit non-goals / residual risk

- **The restricted-process backend is not a container.** It confines paths and
  applies rlimits and (when available) a network namespace, but the host
  filesystem and kernel attack surface remain visible. `snx sandbox status`
  states this. For hostile code, use the container backend.
- **`path-validation-only` (`sandbox.backend = "none"`) is not isolation.** It
  runs commands as ordinary local processes with only workspace path checks.
- **Silent Nexus does not defend against a compromised host or a malicious
  operator.** It protects the operator from the model, not the machine from the
  operator.
- **A model with an approved command capability can still do damage within what
  you approved.** Approval is a real decision; read the prompt.
- **Token/lot estimates are approximate** (no per-model tokenizer); they
  deliberately over-estimate to avoid context overflow.
