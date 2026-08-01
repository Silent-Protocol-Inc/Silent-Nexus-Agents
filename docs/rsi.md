# Governed self-improvement: Nexus RSI and WARP

Nexus RSI is the engine that notices what went wrong and proposes a fix. **WARP**
— Watch, Assess, Replay, Promote — is the independent layer that decides whether
that fix is real. They are separate crates on purpose.

The single rule the design exists to enforce:

> Nexus may propose. It must never define success, make the change, judge the
> result, authorise it, and promote it through one unchallenged path.

## What is built, and what is not

This release ships the engine, the validation layer, and the surface. It does
**not** yet run the loop end to end without you: candidates are recorded and
classified, and each WARP stage is implemented and tested, but wiring a live
candidate through every stage automatically is the next step. Read this document
as "what the pieces guarantee", not "what happens by itself tonight".

| Built | Where |
| --- | --- |
| Observation → typed candidates | `nexus-rsi` (`ObservationCollector`, `ImprovementPlanner`) |
| Multi-dimensional outcome scoring | `nexus-rsi::OutcomeEvaluator`, table `rsi_outcomes` |
| Memory curation (candidate/unverified) | `nexus-rsi::MemoryCurator` |
| Isolation, deterministic checks | `nexus-warp` (`IsolationProvider`, `DeterministicValidator`) |
| Replay, adversarial suite, holdouts | `nexus-warp::{replay, adversarial}` |
| Independent evaluators | `nexus-warp::evaluators` |
| Reward-hacking detection | `nexus-warp::integrity` |
| Risk tiers, promotion gate, governance | `nexus-warp::{risk, promotion}`, `nexus-core::governance` |
| Shadow, canary, health, rollback | `nexus-warp::{shadow, canary, rollback}` |
| `/rsi` and `snx rsi` | `nexus-app::rsi`, `nexus-tui::menus::rsi_menu` |
| Autonomous end-to-end orchestration | **not yet** |
| Level-5 open-ended self-modification | **disabled by design** |

## The two planes

**Data plane** — prompts, routing and retrieval policy, memory, skills, agent
instructions. The running process can apply these itself once WARP clears them,
subject to the tier gates below.

**Code plane** — Rust harness source. RSI can generate a patch and WARP validates
it in a git worktree (`cargo build`, per-crate tests, `clippy -D warnings`,
schema-no-drift), but a running process cannot hot-swap compiled Rust and does
not pretend to. Code-plane "promotion" means a validated branch for human review
that ships through the normal release gate. Code-plane candidates are always
tier 3, and may never touch governance, audit, policy, or validation.

## Risk tiers

| Tier | Examples | Gate |
| --- | --- | --- |
| 0 Observation | telemetry, lessons, reports | auto |
| 1 Low | memory dedup, cache tuning, presentation | auto, **only** after every WARP stage passes |
| 2 Moderate | tool routing, planning strategy, retry, context assembly | shadow run required |
| 3 High | code plane, evaluation policy, network/credential/deploy/MCP | human approval, from someone other than the author |
| 4 Prohibited | disable validation, weaken security, tamper with audit, self-expand permissions, edit governance | auto-reject |

Classification only moves **up**. The effective tier is the maximum of what the
candidate declared and what `RiskClassifier` computes, so labelling yourself
`low` buys nothing. `/rsi candidates` renders that as `low → high` when the
classifier raised it — a candidate that undersold its blast radius is visible.

## Governance

`crates/nexus-core/src/governance.rs` holds the ruleset as compile-time
constants. There is no setter and no config key. `nexus-rsi` and `nexus-warp`
depend on `nexus-core`, so the pipeline cannot import its way up and edit the
rules that constrain it; changing governance requires a human writing Rust and
shipping a release. See the current ruleset with `snx rsi governance`.

Three layers, weakest last:

1. **Dependency direction** — the pipeline cannot reach the rules.
2. **Protected components** — a candidate whose blast radius touches governance,
   audit, policy/permissions, or the validation layer is tier 4 and auto-rejected.
3. **Intent screening** — a keyword screen for the classic bypass phrasings. This
   is defence in depth: it is evadable by rewording, and the safety argument does
   not rest on it.

Permission mode and governance are **different axes**. `/permissions full access`
removes prompts. It does not grant a tier-3 bypass, an automatic MCP install, or
the removal of a validation stage. Setting `allow_tier_3_auto_promotion = true`
is recorded in the decision rationale and ignored.

## The promotion gate

`PromotionGate` is the only door to `Promoted`, and it fails closed:

- WARP unavailable, a missing stage report, or an `Inconclusive` verdict rejects.
  Silence is not consent.
- Hard vetoes — security failure, permission expansion, audit tampering,
  validation bypass, critical regression, secret exposure — are recorded
  individually and never averaged against gains elsewhere. There is no score to
  trade against.
- Required stages: `deterministic`, `replay`, `adversarial`, `integrity`,
  `evaluators`. All must be present and `Passed`.
- Tier 3 needs a human signature, and the signer may not be the author.

Evaluators see requirements, the candidate delta, objective results, and
baseline/candidate outputs. They do **not** see the author's reasoning or
confidence — `EvaluatorInput` has no field for it, so the exclusion is structural
rather than a convention. If an objective stage already failed, the pool returns
`Rejected` however the evaluators voted.

## Shadow and canary

A shadow run gives the candidate real inputs and gives the world nothing. Tool
calls are classified by `RiskLevel`; only `Read` executes, and everything else is
recorded as intent and dropped. A runner that reports having actually performed
an intercepted effect is a **containment breach** and a hard veto.

Canary rollout climbs 5 → 15 → 30 → 50 → 100%. Assignment is deterministic in
`(candidate_id, session_id)`, so a session keeps its arm and the comparison
measures the candidate rather than reassignment noise. Below `min_observations`
the monitor answers `Insufficient`, which holds the rollout — a quiet window is
not a healthy window. A success-rate or error-rate breach rolls back; slower or
chattier holds; one security violation rolls back at any sample size.

Every promotion is recorded with its author and a way back. `PromotionLedger`
refuses to record a promotion that has neither a rollback command nor a
checkpoint, so a candidate cannot reach the promoted state by leaving the
rollback plan blank.

## Reward hacking

Every metric is a target something could optimise the wrong way. The integrity
stage reads the candidate's diff and vetoes the mechanical shortcuts: removed
assertions, added `#[ignore]`, deleted tests, edits to a holdout fixture or to
the validation layer. It is a diff scan — it does not catch a sufficiently clever
rewrite, which is why holdout fixtures, multi-dimensional metrics with no single
reward, and unaveragable vetoes exist alongside it.

Replay fixtures can be marked `holdout`. A replay over a fully visible corpus
reports that fact rather than letting a clean run imply more than it shows.

## Privacy

Observations are redacted through the workspace redactor before storage, and
replay fixtures redact the objective at construction, so replay cannot leak what
observation protected. Fixtures are built from stored summaries, never raw
transcripts.

## Commands

| Command | Shows |
| --- | --- |
| `/rsi`, `snx rsi status` | observation state, candidate queue, last promotion |
| `snx rsi candidates` | every candidate, declared and classified tier |
| `snx rsi show <id>` | evidence, success metrics, WARP's classification |
| `snx rsi observations` | redacted harness events behind the candidates |
| `snx rsi outcomes` | multi-dimensional scores for finished tasks |
| `snx rsi promotions` / `rollbacks` | what was promoted, by whom, and any way back taken |
| `snx rsi governance` | the compile-time ruleset and protected components |

`/status` shows the candidate count and how many wait on a human. `/improve`
remains the review path for the older proposal store.

Turn post-turn analysis off with:

```toml
[self_improvement]
enabled = false
```

## Upgrading

A candidate row written before this release loads with the conservative
defaults — code plane, tier 3 — so upgrading never turns old rows into
auto-promotable ones. Payloads written by 2.11.0 still decode under an older
binary. Memory verification is derived from provenance at read time, so legacy
memory is treated as unverified without rewriting any of your data.
