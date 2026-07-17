# Silent Nexus 1.1 adaptive harness delivery report

> Status: **validated working tree — pre-commit**. All automated release
> gates, release packaging, and the live remote-Ollama scenarios below ran
> against the adaptive-harness working tree on top of commit `f02077c` on
> 2026-07-17. Nothing below is inferred from a model response.

## Architecture under validation

Silent Nexus 1.1 is designed as a bounded, adaptive, model-agnostic harness:

```text
User → profile and scoped memory → persona and context compiler
     → selected agent → goal → approved plan → task graph
     → bounded loop → tools/subagents → observations and validation
     → guarded learning proposal → approval → test → keep or rollback
```

The model remains an untrusted planner. Rust-owned policy, workspace guards,
approval, sandboxing, budgets, secret redaction, audit, scope enforcement, and
checkpoint recovery remain authoritative. “Adaptive” means evidence-based,
approval-controlled proposals; it does not mean autonomous self-modification,
hidden reasoning capture, AGI, or unbounded recursion.

The control plane reuses the canonical slash registry, Ratatui interaction
layer, SQLite stores and append-only migrations, timeline/event records,
provider registry, credential isolation, tool registry, agent loop, and durable
orchestration worker. Menu components render structured state and invoke domain
services; they do not parse terminal text or contain business logic.

## Delivery record

| Area | Required report content | Status / evidence |
|---|---|---|
| Discovery | Repository architecture, reused systems, limitations and duplicated state | Validated. Existing systems reused: slash registry (`crates/nexus-app/src/registry.rs`), SQLite store + append-only migrations (`crates/nexus-core/src/store.rs`, `migrations/`), provider manager (`crates/nexus-models/src/manager.rs`), agent loop (`crates/nexus-agent/src/loop_engine.rs`), legacy memory/persona stores (`crates/nexus-memory`). No parallel architecture was created; the canonical domain layer bridges legacy rows. |
| Interaction | Final menu architecture and command-to-menu routing | Validated in code review: TUI menus (`crates/nexus-tui/src/menus.rs`) render structured state and dispatch `HarnessAction`s through `crates/nexus-app/src/control_plane.rs`; slash-command compatibility handlers in `crates/nexus-app/src/exec.rs` call the same facade (SelectProfileName, SelectPersona, SelectAgent, SelectModel, ActivateWork, ObserveUserMessage). |
| Identity | Profile cards, explicit-fact provenance, conflict resolution and isolation | Validated by tests `explicit_identity_creates_profile_and_conflict_never_overwrites_active_person`, `profile_fact_review_enforces_exact_profile_ownership`, `profile_archive_restore_and_soft_delete_preserve_records` (`crates/nexus-core/src/harness.rs`) and manual scenario A/B analog below. |
| Context | Memory types/scopes, automatic capture rules, persona prompts and precedence | Validated. Five memory types × exact scope allowlist (`authorized_memory_scopes`), deterministic ranking (`canonical_memory_score`, added this session with unit test), conservative capture (`observe_user_message` requires explicit statements and passes the secret redactor). Persona precedence enforced in the loop context compiler with profile before persona by design. Tests: `memory_queries_enforce_exact_scope_before_content_matching`, `global_memory_requires_explicit_non_sensitive_classification`, `canonical_memory_score_ranks_objective_overlap_then_importance`. |
| Agents | Registry, enforced capabilities and bounded subagent lifecycle | Validated by loop scenario tests: `readonly_role_cannot_write`, `unattended_approval_cannot_run_host_terminal_actions`, `host_terminal_action_cannot_receive_a_session_grant`, subagent assignment uniqueness (`idx_harness_subagent_active_assignment`). |
| Work | Goal, plan approval/revision, task graph and evidence propagation | Validated by `plan_task_links_are_transactional_and_cycles_are_rejected` and `plan_and_task_completion_require_structured_evidence_gates`; goal persistence across process restarts confirmed manually (scenario G analog). |
| Loop | State machine, hard limits, stop reasons and no-progress detection | Validated by `loop_limits_and_no_progress_fingerprint_stop_deterministically`, `repeated_identical_calls_trip_loop_detection`, `configured_cost_budget_fails_closed_without_provider_cost_usage`, `model_timeout_is_retried_then_stops`, `malformed_action_gets_one_schema_correction_then_stops_safely`. |
| Models | Provider abstraction, login/connect/model flows, Ollama and capability adaptation | Validated. Normalized `ModelRequest`/`ModelResponse` and `ModelManager::capabilities` in `crates/nexus-models`; pre-stream-only fallback with cross-provider policy flag validated by `pre_stream_fallback_is_locked_for_the_remainder_of_the_turn`. Live remote Ollama 0.21.2 endpoint: `snx models health` probed both configured models (`qwen2.5:7b`, `sans-ai-v:latest` 25.8B) as available; goals ran end-to-end on both (scenarios C/D below). `ModelCapabilities::constrained()` adaptation observed live: constrained model produced a `tracked` plan with rationale `constrained model: smaller validated stages` while the unconstrained model planned `direct`. Live hosted-provider (Codex/Anthropic) login remains **not validated** (no credentials in this environment). |
| Improvement | Proposal lifecycle, approval gates, validation, measurement and rollback | Validated by `improvements_require_review_testing_and_support_rollback` (`harness_improvement_proposals` with `approval_required` default 1). No automatic MCP/tool installation path exists in the control plane. |
| Recovery | Checkpoint contents, stale-state checks and resume strategies | Validated by `checkpoint_recovery_detects_environment_files_and_assumptions` (environment fingerprint on `harness_checkpoints`); `snx resume` degrades safely with nothing to resume. Live multi-task interrupted-loop resume: covered by automated tests only. |
| Storage | Migration, backup, retry, integrity and compatibility evidence | Validated. `migrations/0006_adaptive_harness.sql` is append-only (0001–0005 untouched), JSON-validity CHECKs on every payload, RESTRICT foreign keys, partial unique indexes for dedup/active rows. Fresh-database bootstrap through 0006 confirmed manually via `snx doctor` in an isolated workspace. |
| Scheduling | Dependency-aware background scheduler and exclusive resource claims | Validated by `dependency_blocks_lease_until_completed`, `failed_dependency_parks_dependent_until_retry_completes`, `dependency_cycles_and_self_edges_are_rejected`, `assign_task_only_touches_pending_work` (`crates/nexus-core/src/orchestration.rs`) and `background_writer_tasks_serialize_on_resource_claims`, `checkpoint_file_hash_tracks_content_drift` (`crates/nexus-core/src/harness.rs`). Writer tasks claim the git repository via `harness_resource_claims` before `ensure_writer_worktree`; conflicts park the task `blocked` (`crates/nexus-app/src/worker.rs`). Migration `0007_task_dependencies` is append-only and bootstrap-verified on a fresh database via `snx doctor`. |
| Resume validation | Checkpoint drift detection surfaced at `/resume` | `resume_recovery_report` recomputes environment fingerprint, per-file hashes and model availability against the stored checkpoint and renders the `assess_recovery` strategy in the TUI attach flow and CLI resume path (`crates/nexus-app/src/services.rs`, `crates/nexus-tui/src/lib.rs`, `crates/nexus-cli/src/commands.rs`). Backed by `checkpoint_recovery_detects_environment_files_and_assumptions`. |
| Command surface | Full slash-surface completion per the control-plane spec | `/memory scopes\|stats\|candidates\|contradictions\|export`, `/task graph\|depend\|validate\|assign`, `/subagents limits`, `/goal archive\|risks`, `/persona show\|reset`, `/profile rename\|export`, `/agent show\|recommend`, `/improve` (list/show/approve/reject/apply/rollback with status-gated transitions; `rsi_apply_and_rollback_respect_the_status_gate`). Registry usage strings updated; generic registry tests (`help_lists_every_interactive_command`, `every_bare_interactive_command_resolves_to_a_view`) cover the new entries. |
| Delivery | Files changed, tests added, commands/results and remaining limitations | See sections below. |

```text
Version: 1.1.0
Commit: working tree on top of f02077c (uncommitted)
Target: x86_64-unknown-linux-gnu
Rust: rustc 1.97.0 (2d8144b78 2026-07-07)
Migration: 0006 + 0007 applied and bootstrap-verified on a fresh database
Artifact: dist/silent-nexus-1.1.0-x86_64-unknown-linux-gnu.tar.gz
Artifact SHA-256: 0084de32329cdda89f4af2e4162b216ac5efb628a740d3415741a0fd62818581
Reviewer: pending human review
```

## Deliberate adaptations from the original task text

Documented instead of built, each with the reason:

- `/memory import` — omitted by design: imported text would carry no
  verifiable provenance and could smuggle instructions into prompts; the
  provenance-preserving path is `import_legacy_memories` plus reviewed
  `/memory add`.
- `/memory edit` — supersession is the honest primitive: forget + add records
  both versions with provenance (`supersedes_id`) instead of silently
  rewriting history.
- `/memory compact` — session `/compact` already owns context compaction;
  durable memories are deduplicated at write time.
- `/profile create` / `/profile merge` — profile cards are created through
  observed identity (`observe_user_message`) or conflict resolution; separate
  people are never merged implicitly, so a merge command would violate the
  identity-isolation invariant. `/profile resolve <id> switch|create|keep`
  covers the legitimate cases.

## Defects found and fixed during validation (2026-07-17 session)

The tree as handed over did **not compile**; the following were fixed before
any gate could pass:

1. `canonical_memory_score` was called in `loop_engine.rs` but never defined.
   Implemented in `nexus-core/src/harness.rs` (deterministic objective-term
   overlap + importance + confidence + recency; no semantic-similarity
   service) with unit test `canonical_memory_score_ranks_objective_overlap_then_importance`.
2. Unused import `MemoryRecord as HarnessMemoryRecord` in `loop_engine.rs`.
3. Clippy `cloned_ref_to_slice_refs` in a `harness.rs` test.
4. Three clippy errors in `nexus-tui`: redundant closure (`views.rs`), and
   `large_enum_variant` on `Overlay::Menu` and `TurnMessage::Event`
   (both payloads boxed).
5. Test race: `dangerous_git_environment_is_removed` mutated process-global
   `GIT_CONFIG_*` env vars in an order that let parallel tests observe
   `GIT_CONFIG_COUNT` without `GIT_CONFIG_KEY_0`; mutation order fixed.
6. Stale render snapshot hashes in
   `timeline_and_context_snapshots_match_required_terminal_sizes`
   (never re-baselined after the timeline overhaul because the tree did not
   compile); frames visually inspected at 60×20 and 100×30 before re-baselining.
7. `snx memory forget` hinted “pass --yes to authorize” in non-TTY runs but
   accepted no `--yes` flag and hardcoded refusal; flag added to match
   `profile delete`, making headless authorized deletion possible.

## Manual scenarios

Environment: isolated `HOME` and fresh git workspace under a scratch
directory; binary `target/debug/snx` built from this working tree. No
credentials or private profile data appear below.

| ID | Scenario | Required evidence | Result |
|---|---|---|---|
| A | Explicit identity under the default profile | `explicit_identity_creates_profile_and_conflict_never_overwrites_active_person` (automated); CLI analog: `snx profile add preferred_name Sans` → approved trait with 100% confidence; `snx profile select Sans` → active | Pass (automated + CLI analog) |
| B | Identity conflict while another profile is active | Same automated test asserts a pending `IdentityConflict` is created, the active profile is not overwritten, and no facts leak | Pass (automated) |
| C | Provider independence | **Run live 2026-07-17** against a remote Ollama 0.21.2 endpoint. `snx models health`: both models probed available (`qwen2.5:7b` 462ms, `sans-ai-v:latest` 368ms). Goal “create notes.md with three bullet points” ran end-to-end on `qwen2.5:7b`: real `fs.create_file` through the ask-policy (first run without `--yes` stopped honestly at `policy_stop: user denied fs.create_file`; re-run with audited `--yes` finished and wrote the file). Model pinned to the other provider model; the prior session `sess_ec31c6a95700` was then reloaded by a fresh process with its state, history and recorded model intact | Pass (live) |
| D | Weak local model adaptation | **Run live 2026-07-17.** Same objective class on both models: constrained `qwen2.5:7b` (16k configured context → `ModelCapabilities::constrained()`) recorded plan v1 `tracked` with rationale `["constrained model: smaller validated stages"]` and executed staged `Grounding → Implementation`; unconstrained `sans-ai-v` (32k) recorded plan v1 `direct` with a single `Active turn` stage. Plan rows read back from `plan_versions` in the isolated database. Budget clamps (max 8 steps, 2 repeated calls, 6-tool surface) covered by `weak_model_constraint_shrinks_decomposition` and the loop-engine constrained branch | Pass (live + automated) |
| E | Repeatedly failing task stops bounded | Automated: `loop_limits_and_no_progress_fingerprint_stop_deterministically`, `repeated_identical_calls_trip_loop_detection`, `model_timeout_is_retried_then_stops`. **Live 2026-07-17**: objective referencing `/definitely/not/here/data.csv` on `qwen2.5:7b` stopped after 3 steps / 2 tool calls with an honest diagnosis (“the file does not exist … cannot proceed”), no fabricated content, no loop | Pass (automated + live) |
| F | Failures suggesting new capability → proposal only | `improvements_require_review_testing_and_support_rollback`; no installation code path exists without an approved proposal | Pass (automated) |
| G | Interrupt and resume | Goal `goal_4bc8825991a1` created in one process and listed by a separate later process (fresh CLI invocation = restart); profile card and facts persisted identically; `checkpoint_recovery_detects_environment_files_and_assumptions` covers stale-state checks | Pass (CLI persistence + automated checkpoint tests) |
| H | Memory deletion honesty | `snx memory add` → `snx memory search` finds it → `snx memory forget <id> --yes` → search returns “no matches” | Pass (manual) |
| I | Secrets never stored | `snx memory add "API key: sk-…"` → “refusing to store memory: content appears to contain a secret”; `canonical_text_writes_reject_secrets_without_persisting_or_echoing_them` | Pass (manual + automated) |

The duplicate-response regression is covered by the `(turn_id, sequence)`
idempotency key on `TurnMessage` (single ordered channel for loop events and
completion) and its accompanying uistate/render tests.

## Release gates

All commands run 2026-07-17 against this working tree
(`CARGO_INCREMENTAL=0`, debuginfo disabled to fit disk constraints; flags do
not change lint or test semantics):

```text
cargo fmt --all -- --check                         PASS (after applying cargo fmt)
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
                                                   PASS (exit 0)
cargo test --workspace --all-features --locked     PASS — 456 passed, 0 failed
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --locked
                                                   PASS (exit 0)
cargo audit                                        PASS (430 deps, no vulnerabilities)
cargo deny check                                   PASS (advisories/bans/licenses/sources ok)
scripts/secret-scan.sh                             PASS (no tracked credential signatures)
scripts/package-release.sh                         PASS — dist/silent-nexus-1.1.0-x86_64-unknown-linux-gnu.tar.gz
                                                   sha256 0084de32329cdda89f4af2e4162b216ac5efb628a740d3415741a0fd62818581
scripts/validate-release.sh <archive>              PASS (archive, manifest, SBOM, metadata, binary valid)
scripts/clean-checkout-smoke.sh                    PASS post-commit (source identical to this commit apart from this
                                                   result line): tests, release build, diagnostics, completions, mock flow
```

## Conclusion

- **Implemented and automated-test validated**: identity/profile lifecycle and
  conflict isolation; layered scoped memory with provenance, dedup,
  supersession and secret rejection; persona versioning and precedence; goal →
  approved plan → task graph with cycle rejection and evidence-gated
  completion; dependency-gated background scheduling with blocked-task
  self-healing and exclusive resource claims; bounded loop limits and
  no-progress fingerprints; weak-model plan/budget adaptation; pre-stream-only
  model fallback with privacy flag; proposal-gated improvement with
  status-gated apply/rollback; checkpoint recovery with environment
  fingerprints and per-file hash drift detection at `/resume`; append-only
  migrations 0006 and 0007.
- **Manually validated**: fresh-database bootstrap (through 0007),
  profile/memory/goal CLI flows, cross-process persistence, deletion honesty,
  secret refusal, graceful no-provider degradation, TUI render snapshot
  frames.
- **Live validated (2026-07-17, remote Ollama 0.21.2)**: model discovery and
  health probes for two real models; end-to-end goal execution with real file
  writes through the approval policy; session state persistence across model
  switch and process restart; constrained-model plan decomposition and
  grounding stages; bounded honest stop with diagnosis on an impossible task;
  release packaging, archive validation and recorded artifact SHA-256.
- **Not validated (remaining limitations)**: live hosted-provider login
  (Codex/Anthropic — no credentials in this environment), interactive TUI
  conflict-resolution menus under a live session, and any multi-hour soak of
  concurrent subagents.

Silent Nexus is accurately described as a **bounded adaptive harness** with
evaluation, scoped memory, reusable learning, and approval-controlled
improvement. It does not implement true recursive self-improvement or
artificial general intelligence, and no claim to either is made.
