# Changelog

## [2.16.1] — 2026-08-05

One fix. A conversational turn no longer carries the approved profile card.

### Fixed

- **The profile card was overriding the persona's voice on conversation.**
  2.14.0 narrowed a conversational turn to drop the plan, the operational
  contract, the role charter, and the tool inventory — but kept the approved
  profile. That card is a record about *work*: how the operator wants results
  reported, which projects they run, what constraints apply. It was emitted as a
  system section in the same block as the persona, above the request.

  So a profile carrying `communication_style: "Prefers concise answers"` — a
  perfectly good instruction for a task report — reached a turn where a
  character was supposed to be speaking in its own voice, and the reply came
  back as a four-line summary card instead of prose. The preference was written
  about work and applied to something else.

  Work still receives the profile unchanged; a conversation does not. The
  decision lives on the existing `PromptShape` (`includes_profile`), so the loop
  and `/persona inspect-effective` cannot disagree about it.

  This section is description, not enforcement — policy, sandbox, workspace
  confinement, approval, redaction, and audit are applied in code and are
  untouched — so omitting it removes no check and grants nothing.

### Compatibility

PATCH. No schema change, no configuration change, no migration. Profiles, their
facts, and the review queue are all untouched, and the profile tools behave
exactly as before on the turns that carry them.

## [2.16.0] — 2026-08-05

Two corrections to how 2.15.0 delivers a persona. Both are small; the second
changes output on every turn.

### Changed

- **The persona layer now decides temperature outright.** A persona that names
  no `temperature` used to send none, leaving the per-model value in
  `models.toml` to apply. It now resolves to **1.0**, so a turn carrying a
  persona always carries a temperature the persona layer chose.

  The reasoning: a persona is supposed to read the same way wherever it runs,
  and inheriting sampling from whichever model happens to be pinned is exactly
  what made that untrue — the same character came out clipped on one model and
  florid on another. The cost is real and worth stating plainly: **the
  `temperature` key in `models.toml` no longer applies to turns that carry a
  persona**, which is every user turn. Set it on the persona instead.

  `max_output_tokens` is unchanged and deliberately asymmetric: unset still
  means *omit the parameter* and let the server choose its own ceiling. That is
  a real third state, not a zero, and a persona asking for zero output tokens is
  still refused.

- **The directive and the persona text join with a single space**, not a blank
  line — one continuous instruction rather than a heading followed by prose.
  The persona text itself is still passed byte for byte.

### Note

`/persona inspect-effective` now reports `persona_temperature` as the value that
would actually be sent, plus `persona_temperature_is_default` so the number can
be told apart from a persona that chose 1.0 itself. The field is no longer
nullable.

## [2.15.0] — 2026-08-05

Personas again. 2.14.0 delivered the persona as an instruction; this release
changes where it sits, how it is introduced, and what the model is allowed to
sample at while it is active.

### Changed

- **The persona now opens the system block instead of closing it.** 2.14.0 moved
  it last, on the reasoning that the position nearest generation is weighted
  most strongly for voice. Putting it first works better: the rest of the
  prompt then reads as instructions given *to* that character, rather than as a
  competing description of one that the persona has to argue with afterwards.
  Authority is untouched either way — `ActivePersona` is still rank 4 and still
  pinned. Only wire position changed.

- **The adoption directive is one sentence: `Your name is <name>.`** It replaces
  the paragraph 2.14.0 prefixed. The persona's own text already establishes who
  it is; the directive only has to remove the ambiguity a provider's default
  identity would otherwise fill, and a long preamble about what the identity
  does and does not grant is prompt the model has to reconcile before it reaches
  the character. It still grants nothing, and it still never alters the persona
  text.

### Added

- **Per-persona sampling.** A persona can carry `temperature` (0.0–2.0) and
  `max_output_tokens`, applied only on turns where that persona is active:

  ```
  snx persona create "Cartographer" --temperature 1.1 --max-output-tokens 2048 …
  ```

  A terse analytical persona and a florid roleplay one want genuinely different
  sampling, and running both at whatever the model config says makes the second
  read flat. Both fields are optional and independent; unset means "leave the
  model's own setting alone", which is not the same as zero. Values a provider
  would reject are refused when the persona is saved rather than mid-turn.

### Note

As in 2.14.0: a hosted backend applies its own identity and content policy
server-side, above anything sent from here. None of this overrides that, and
none of it is claimed to.

## [2.14.0] — 2026-08-04

### Fixed

- **A selected persona could be present in the prompt and still not govern the
  answer.** With a persona active and confirmed, the model answered as a coding
  assistant. The persona was never dropped — it was sent, once, at the right
  authority — but it arrived as one labelled block in the middle of a prompt
  otherwise made of safety rules, policy, a tool inventory, a role charter, and
  plan JSON, with nothing saying it was the identity to answer as. Three
  changes, none of which touch the persona text:

  - The persona section is now **emitted last**, after every other instruction
    and immediately before the conversation. Its authority is unchanged
    (`ActivePersona`, still pinned) — rank decides conflicts and shed order,
    position decides what the provider reads next to the request.
  - It is prefixed with a sentence naming the persona as the identity to answer
    as, in the first person, including when asked what it is. The sentence
    grants no capability and relaxes nothing above it.
  - Turns that need no tools no longer carry the tool machine. A simple request
    with no goal, pending task, or tracked plan omits the plan, contract,
    charter, and tool inventory, and keeps safety, policy, project
    instructions, profile, memory, and the persona.

- **`/persona inspect-effective` reported its own design instead of the
  request.** `behavioral persona count` and `duplicate persona sections` were
  hardcoded, so the one tool for diagnosing a delivery problem reported
  everything healthy by construction. It now shows the exact section body that
  would be sent, whether the adoption directive is present, the shape of the
  next turn — computed by the same function the loop calls — and a plain
  statement of what a hosted provider can still override.

### Added

- `[persona]` config: `adoption_directive` and `conversational_turns`, both on
  by default.

### Note

A hosted backend applies its own identity and content policy server-side, above
anything Silent Nexus sends. None of this overrides that, and none of it is
claimed to: a hosted model may still answer in its own voice or decline. Running
a local model is what removes the other party from the decision.

## [2.13.2] — 2026-08-04

### Fixed

- **A persona refused for carrying a credential was stored anyway.** Creating a
  persona whose text contained an API key printed `refusing to persist persona
  containing a likely secret` — and then wrote it. The key was readable
  immediately afterwards through `persona list` and `persona inspect`.

  Persona creation spans two stores, and only the second one scanned for
  credentials. The first inserted the row unchecked; the second refused; nothing
  removed the row. So the refusal was reported after the credential was already
  on disk, and the message described an enforcement that had not happened.

  The scan moved to `nexus_core::secret`, where **every** durable write can
  reach it, and now runs in the persona store itself — before the first write,
  across the name, description, and instructions. Creation and editing also undo
  their first write if the second fails for any other reason, so the two stores
  can no longer disagree about whether a persona exists.

  Ordinary prose is unaffected: the 2.13.1 token-boundary rule still applies, so
  `asterisk-wrapped action lines` remains storable.

## [2.13.1] — 2026-08-04

### Fixed

- **A persona could be refused as a secret for containing an ordinary English
  word.** The credential scan tested `contains("sk-")`, which matches inside
  `asterisk-wrapped`, `risk-averse`, `task-specific`, and `desk-bound` — so a
  persona that described its own prose format was rejected with "refusing to
  persist persona containing a likely secret". A key prefix now only counts when
  it starts a token *and* is followed by enough key-shaped characters to be a
  key; the same applies to `bearer `, which used to fire on "the bearer of the
  message". Real credentials are still refused, including at the start of a
  payload.

- **The persona manager could create but not edit or delete.** It now offers
  both as chooser submenus, so the fast path — Enter on a row selects that
  persona — keeps working.

- **Choosing the built-in `Nexus` row failed with `not found`.** Selecting it is
  the same thing as clearing the selection, but its id was passed through to the
  harness, which looked for a stored row that does not exist. Every path that
  consumes a chosen persona id now recognises the built-in identity.

### Added

- **`/sessions` can delete a session.** A `Delete session…` chooser in the menu
  and `snx session delete <id>`, both confirmed first. The attached session is
  listed but disabled — deleting the session a turn is writing to would leave it
  writing to a row that no longer exists.

- `snx persona use` as an alias of `snx persona select`, matching `/persona use`.

## [2.13.0] — 2026-08-04

### Changed

- **The selected persona is now the sole behavioral identity of a turn.**
  Previously a persona was an *additional* instruction: the flagship role's
  charter opened with "You are NEXUS…" and stayed in the prompt no matter what
  the operator had selected, so a custom persona arrived beside a built-in one
  and had to argue with it. There is now exactly one behavioral persona in every
  request — the selected one, or the built-in `Nexus` identity when none is
  selected. Never both, never zero.

  The resolution happens in one place (`BehavioralPersona::resolve`), so the
  invariant holds by construction rather than by each call site remembering it.
  The persona layer is pinned above the operational agent contract and every
  task section, and it is delivered through the strongest channel the provider
  actually has.

- **Agent charters carry obligations, not identity.** The Nexus charter kept its
  responsibilities — carry the objective, prove it with evidence, keep
  self-improvement approval-gated — and gave up its "You are NEXUS" preamble and
  its restatement of the safety bounds. Identity and manner belong to the persona
  layer; the bounds were already pinned above both and enforced in code, so
  repeating them made a custom persona look as if it were dropping protections it
  never had the power to drop.

### Added

- **`/persona` opens a real manager, and `PERSONA FORGE` is a real editor.** The
  old manager's "Create persona…" pasted `/persona create name instructions`
  into the message composer for the operator to finish typing. It now opens a
  multi-step editor with multiline entry, paste, cursor movement, undo/redo, a
  live character and token count, a structured section view, and a raw view
  showing the exact text that will be stored and sent. Switching between the two
  views never loses a character, and a first `Esc` on unsaved work asks before a
  second one discards it.

- **`/persona inspect-effective` (`PERSONA MATRIX`).** Reports which persona is
  being sent, its revision, how many behavioral personas the request contains,
  whether the built-in identity is included, whether the persona travels as a
  system instruction or as a user message, which channel the provider offers,
  and what that channel costs. It shows the three prompt layers separately. It
  never shows credentials, provider-internal instructions, or anything the
  application cannot see.

- **Persona metadata and derivation.** Content profile, category, tags, base
  persona, inheritance mode, persistence policy, compatibility notes,
  recommendations, and revision. Deriving a persona defaults to a snapshot — an
  independent copy that a later edit to the source cannot reach — with `extend`
  available for a live link. Content profile is a label: it is never consulted
  when the prompt is built, so it cannot add, remove, or reword a character of
  persona text.

- **Provider instruction channels are reported honestly.** Each adapter declares
  whether it offers a true system role, a dedicated instructions field, a prefix
  fallback, or nothing. The Claude CLI bridge serializes the conversation into
  one prompt and now says so, rather than implying system-level authority it
  does not have.

- **`snx persona` covers the same ground as the TUI** — `list`, `create`,
  `duplicate`, `derive`, `edit`, `select`, `disable`, `status`, `inspect`,
  `inspect-effective`, `test`, `export`, `import`, `delete` — through the same
  services, so neither surface owns its own notion of what a persona is.

- **A persona segment in the status bar.** Shown only when a custom persona is
  active; the built-in identity is the default and needs no announcement.

### Fixed

- **A persona could previously be dropped entirely.** A selection that resolved
  to empty text left the turn with no behavioral identity at all, which is as
  wrong as having two. Resolution now falls back to the built-in identity, and
  the layer is pinned so budget pressure cannot shed it either.

- **Persona validation no longer has anything to say about content.** Personas
  are rejected only for technical reasons: empty, oversized, malformed encoding,
  terminal control sequences, invalid inheritance, or an embedded credential
  under the existing secret-handling policy. Mature, explicit, profane, romantic,
  or otherwise unconventional text is stored and transmitted exactly as written.
  The provider or model may still refuse to answer; that refusal is reported as
  the provider's, and Silent Nexus does not reroute around it.

  Personas remain behavioral only. Persona text cannot grant tools, shell,
  network, credentials, sandbox scope, approval bypass, budgets, or any other
  runtime capability, and a persona that asks for them receives none.

## [2.12.3] — 2026-08-02

### Fixed

- **Plan mode had no keyboard way out.** `/plan exit` worked, but it had to be
  typed into the very composer whose behavior the operator was trying to leave —
  and nothing said so below Desktop width, so on a small terminal the only exit
  was a command nobody had been shown. **`Esc` on an empty composer now leaves
  plan mode**, and the way out is named in the composer title, in the toast that
  announces the mode, and in `?`.

  Esc is honored only when the composer is empty, so a half-typed instruction is
  never traded away for a mode change. While there is text in it, the title
  offers `/plan exit` instead of promising a key that would do nothing.

- **The composer title told the operator the wrong thing about Enter while
  planning.** On layouts where the context rail is gone, the title showed
  `→ context · Enter send` — and it won exactly when the composer was empty,
  which is the moment someone is deciding what to do. In plan mode Enter does
  not send an instruction, so the hint answered the right question with the
  wrong answer and plan mode named neither itself nor its exit. Plan mode now
  outranks the context hint.

## [2.12.2] — 2026-08-02

### Fixed

- **Full access could not reach the files it had asked about.** The permission
  ladder made the safety class — privilege escalation, denied commands, terminal
  Git side effects, and reads of locked paths — a single question per session.
  It worked for commands, but not for reads: `.env`, `.git`, keystores, and
  `credentials.*` are refused by the workspace guard *by path*, before policy is
  consulted at all. So the operator was asked, answered, and the agent still
  reported `No .env file exists in the workspace` — which is worse than a
  refusal, because it is not true.

  The guard now shares the session's answer, and the file tools consult that
  one decision in every place they used to make it themselves — the per-file
  read check *and* the three listing paths (`fs.list`, `fs.search`,
  `fs.tree`), which silently skipped restricted entries and are why the model
  concluded the file was absent. Approving full access's one question lets that
  session read those paths; leaving the session locks them again, since the
  answer is process state and is never written down.

  Redaction is a separate layer and is unchanged: a secret-shaped value inside
  an approved file is still masked before it reaches the model or the audit log,
  so reading `.env` shows its shape and its ordinary keys, not its credentials.

  **Workspace confinement is untouched and has no mode.** The unlock lifts only
  "this path is one you told me to keep away from". It does not move the
  boundary: absolute paths outside the workspace, `..` traversal, and symlinks
  pointing out of the tree are refused exactly as before, with a test that pins
  each of them against an unlocked guard.

## [2.12.1] — 2026-08-02

### Changed

- **The permission modes now decide what a refusal means.** They are a ladder of
  how much the operator wants to be interrupted, and the layers below did not
  know which rung they were on — so a read-only agent role, a narrowed scope, or
  a denied read format refused outright even in the mode chosen precisely so the
  operator could decide. Now: **auto-edit** turns a configured refusal into a
  question, and **full access** permits it and records it. Every action still
  goes through the audit log, so "without asking" never means "without a
  record".

  Under full access this covers raw shell and host terminal actions too: they
  used to force a prominent one-time prompt whatever the mode, and now run
  unprompted while the operator is present to have chosen the mode.

  Four refusals are not preferences and no mode absorbs them silently:
  privilege escalation, the denied-command list, terminal Git side effects, and
  reads of locked paths (`.git`, `.env`, keystores). Under full access these are
  **asked once**, and the answer stands for the rest of the session. It is
  process state and is never written down, so leaving — exit, disconnect,
  crash — asks again. In every other mode they still refuse.

  Unattended and background runs cannot answer a prompt, so a stored setting is
  not allowed to answer for them: full access never becomes "background agents
  may run host commands", and `automatic/background terminal execution requires
  strong container isolation` still holds.

### Fixed

- **Approving a terminal action did nothing.** A host terminal action raises a
  prominent one-time prompt that states the action is not isolated; answering it
  recorded `✓ Approved once` and the turn then failed anyway with
  `action denied by policy: host execution cannot prove restricted-file
  masking…`. Consent was asked for, given, and then refused, with no way to tell
  in advance that answering would not help. The masking check now honours that
  approval. The token behind it is granted only by an attended approval of that
  one action and is still spent on it, so the next action asks again and nothing
  unattended can reach it.
- **Full access was stricter than default mode.** Full access sets
  `commands = "allow"`, so nothing asked — and with nothing asked there was no
  approval to point at, so the masking check and the host backend both refused.
  Choosing the most permissive mode left no way to run a command at all. Full
  access is now read as the same consent given once: a structured
  `program + argv` invocation runs and is audited. Raw shell (`terminal.run`)
  still raises the one-time prompt every time, because an arbitrary command line
  is the case worth reading before it runs whatever the mode.

  This applies only while someone is there to have chosen it. Unattended and
  background runs cannot answer a prompt, and a stored setting is not allowed to
  answer on their behalf — so full access never becomes "background agents may
  run host commands", and `automatic/background terminal execution requires
  strong container isolation` still holds.
- **Full access could not be turned on at all.** `App::bootstrap` reset the
  seven policy decisions back to the defaults on every start whenever they
  matched the full-access preset. `/permissions` wrote the choice to the managed
  overrides and the file kept saying `full-access`, but every new process
  quietly disagreed — `snx config show` reported the defaults over a file that
  said otherwise, and the status bar read `default`. The setting now persists,
  as choosing it asks for. It still never makes destructive or external side
  effects automatic, and terminal execution on a host backend still requires an
  attended session.
- **The approval prompt now names the exposure it is asking about.** It cited
  the host-process fallback rule; it now says plainly that nothing can hide
  restricted files (`.git`, `.env`, keystores) from a host command, so approving
  runs it with your own read access. Approving is the decision, so it should be
  made against the real terms.

## [2.12.0] — 2026-08-02

### Added

- **`[sandbox].allow_unmasked_host_reads`** (default `false`), for running host
  commands in a workspace whose restricted files cannot be masked.

  Restricted paths are masked by bind-mounting `/dev/null` over them, so only
  the container backend can mask anything. Without one, a host command inherits
  your own read access and terminal actions are refused — which, because `.git`
  is restricted, means refused in **every** Git repository. That default stands
  and nothing changes for anyone who leaves this unset.

  Setting it accepts the exposure in exchange for a working terminal without a
  container. It is a genuine widening, not a formality: a host command can then
  read paths `fs.read_file` refuses one at a time. Actions are still approved
  individually, and the approval card still states that the action is not
  isolated, so the consent is informed at the moment it is given rather than
  buried in a config file. The refusal message now lists this alongside the
  per-file tools and the container sandbox, so all three ways forward are
  visible at the point of failure.

  ```sh
  snx config set sandbox.allow_unmasked_host_reads true --workspace
  ```

## [2.11.1] — 2026-08-02

### Fixed

- **The restricted-file refusal counted a repository's insides and never said
  so.** Host commands are refused when restricted paths cannot be masked, which
  is deliberate: masking works by bind-mounting `/dev/null` over each path, only
  the container backend can do it, and without one a host command inherits your
  own read access and could read `.git`, `.env`, or a keystore. What was wrong
  is what the refusal *said*. The scan walks into `.git` on purpose and every
  path under it is restricted, so each object, ref, and hook sample was counted
  separately — a fresh `git init` holding one file reported **24 restricted
  files**. Masks apply to directories, so those children never contributed any
  enforcement; they only made a routine refusal read as a discovery about your
  workspace. A path already covered by a directory mask is no longer counted,
  and that same repository now reports 2. The message also now says the thing
  that actually explains it: `.git` is restricted, so this applies in **every**
  Git repository, not just unusual ones.

## [2.11.0] — 2026-08-02

### Added

- **`nexus` is now a real flagship agent — the default Recursive
  Self-Improvement (RSI) agent.** A fresh install already selected `nexus` as
  its default agent, but the role was a hollow shell: same capabilities as the
  orchestrator, yet no behavioral contract, so it told the model nothing about
  how to work. `nexus` now carries a proper charter — a generalist that plans,
  implements, verifies, and delegates, and that improves over time by letting
  the harness record *approval-gated* self-improvement proposals. The charter
  sits strictly below the immutable safety rules and cannot relax workspace
  confinement, approval, or evidence requirements.
- **SNX can act on what you tell it about yourself.** Telling SNX "my name is
  Sans" stored a fact in the canonical profile store — and then the agent
  answered *"I don't have a profile-card management tool in this session."* That
  answer was accurate: the storage layer was complete, but nothing above it was
  reachable from a turn. There is now a `profile` tool category — `get_active`,
  `list`, `create`, `select`, `update`, `add_fact`, `remove_fact`, `merge`,
  `get_candidates`, `review_candidate` — granted to every role for reading and
  to working roles for writing. Roles that work from external material
  (researcher, and the read-only audit roles) can read the card and never write
  it, so nothing SNX finds on the internet becomes something you said.
  Capability gating is enforced in the agent loop, not merely declared.
  Alongside it, a deterministic pre-turn pass captures durable statements —
  preferred name, occupation, timezone, language, working preferences, stack —
  from named, individually tested wordings. There is **no extra model call and
  no per-message classification**: an ambiguous phrasing captures nothing and
  the agent uses `profile.add_fact` deliberately instead, so every automatic
  capture traces to a pattern with a test beside it. Repeating yourself is a
  no-op rather than a duplicate row; a changed value supersedes the old one
  without discarding it. Credentials are never turned into a profile fact, and
  sensitive categories are held as candidates you approve in `/profile` — which
  now shows what is waiting and says plainly that it is not in use yet. New
  `[profile]` block: `auto_capture`, `capture_preferences`,
  `require_review_for_sensitive`, all on by default.
- **Recursive Self-Improvement is a first-class, visible default.** The
  post-turn analysis that mines finished turns for reusable workflows, repeated
  failures, and stated preferences (recording each as an approval-gated proposal
  reviewed with `snx profile`) has been running for releases, but silently and
  with no way to turn it off. It now lives under a `[self_improvement]` config
  block (`enabled`, `surface_pending`, both on by default), and `snx status` and
  the TUI startup surface the pending-proposal count so the review queue is no
  longer invisible.
- **Flagship identity in branding.** `snx about` and `snx status` now show the
  flagship agent line — `nexus · Recursive Self-Improvement (RSI)`. The product
  name stays **NEXUS by Silent Protocol**; this names the agent, not a rebrand.
- **Governed self-improvement: Nexus RSI + WARP.** Self-improvement is no longer
  just a proposal queue. Candidates are now typed (target, scope, risk tier,
  success metrics, affected components) and judged by **WARP** — Watch, Assess,
  Replay, Promote — an independent validation layer in its own crate that can
  reject a candidate but never author one. WARP runs deterministic checks in an
  isolated worktree or overlay, replays sanitized historical fixtures (including
  holdouts), runs an adversarial suite, scans the candidate's diff for the
  mechanical forms of reward hacking, and asks independent evaluators that never
  see the author's reasoning. An objective failure is a hard veto no model
  verdict can average away.
- **A governance layer the pipeline cannot edit.** `nexus-core::governance` holds
  the ruleset as compile-time constants with no setter and no config key; RSI and
  WARP depend on that crate, so they cannot reach up and rewrite what constrains
  them. A candidate touching governance, audit, policy, permissions, or the
  validation layer is tier 4 and auto-rejected. Risk classification only moves
  *up*: the effective tier is the maximum of declared and computed.
- **Promotion fails closed.** The promotion gate rejects when WARP is
  unavailable, a stage is missing, or a verdict is inconclusive. Tier 1 may
  auto-promote after every stage passes; tier 2 needs a shadow run; tier 3 needs
  a human signature that is not the author's. `/permissions full access` removes
  prompts, not governance — `allow_tier_3_auto_promotion = true` is recorded and
  ignored.
- **Shadow, canary, and recorded rollback.** A shadow run gives the candidate
  real inputs and the world nothing: only read-only tool calls execute, and an
  effect that escapes containment is a hard veto. Canary rollout climbs
  5→15→30→50→100% with deterministic per-session assignment; a success or error
  breach rolls back, and one security violation rolls back at any sample size.
  Every promotion is recorded with its author and a way back — the ledger
  refuses to record one that has neither a rollback command nor a checkpoint.
- **`/rsi` and `snx rsi`.** Status, candidates, candidate detail, observations,
  outcomes, promotions, rollbacks, and the governance ruleset. The candidate list
  shows the declared tier next to WARP's classified one, so a candidate that
  undersold its blast radius is visible. `/status` shows how many candidates wait
  on a human. Full documentation in [`docs/rsi.md`](docs/rsi.md).
- **What this does not do**, stated plainly: the stages are implemented and
  tested individually, but the loop is not yet wired to carry a candidate through
  every stage on its own; code-plane changes are never hot-swapped into a running
  process (they ship as a human-approved release); and open-ended self-directed
  modification stays disabled.
- **A presentation architecture: boot, status, timeline, debug.** A turn used to
  be either raw tool rows (`fs.read`, `terminal.exec`, argument JSON) or silence
  until the final answer, and startup was three unrelated system lines pushed
  from two different files. What you see is now organized into four strict
  layers with one rule between them: *boot, status, and timeline may render only
  what the translation layer emitted; debug renders the untranslated truth.*
  That is structural — the product layers consume a `Presented` value that has
  no field for a tool name, an argument blob, or raw output, so a leak is a
  compile error, and a test asserts it over every tool in the real registry.
  `snx run` renders at the debug layer throughout, as it always has: it is a log
  with no `/view` to turn detail back on. It shares the intent card and the
  milestones with the TUI, so the two surfaces cannot drift.
  Full documentation in [`docs/presentation.md`](docs/presentation.md).
- **One welcome panel instead of startup logs.** Opening a session used to print
  four `✓ DONE  NOTICE` cards — session restored, memory linked, what's new,
  ready — because startup facts were pushed through the ordinary timeline
  renderer, which files them as completed `Notice` events. Startup is not work
  the agent did, so it is no longer an event at all: `BootSnapshot` gathers the
  facts once and a dedicated panel above the timeline renders them, with
  identity, workspace, model, agent, access, session, memory, one changelog
  headline, and two or three tips chosen from live state. Every section is
  omitted when it has nothing real to say, so a fresh workspace shows identity,
  metadata, and a tip — never `Session: none`. Labels are `SESSION // RESTORED`,
  not run outcomes. The panel collapses to one line
  (`◢ NEXUS · implementer · Ollama / qwen · main · restored`) when the first turn
  starts, so it is never a permanent tax on transcript height, and it sheds rows
  in a defined order on a short terminal rather than being clipped. The timeline
  now starts empty. No progress bar: startup is not measurable in advance.
- **A live status line.** While a turn runs, one transient row above the input
  says what the agent is doing — `◇ Tracing intent · 24 seconds · high effort` —
  with a terse verb, elapsed time, and the *reported* effort, omitted rather than
  guessed when the provider did not report one. It degrades to verb-only on
  narrow terminals, holds a verb for a dwell window so a fast tool sequence
  cannot strobe, and disappears when idle. It renders in every narration and
  thinking mode, including `off`: liveness is not verbosity. Being a render-time
  projection with no store write, it cannot append to the record it sits above.
- **Intent and milestones (`/narrate`, `snx narrate`).** A task turn opens with a
  2–5 step intent card and then reports milestones as they happen. The steps come
  from a deterministic skeleton built from the same task class and work estimate
  the work breakdown uses; a model may improve the *wording* only, and the gate
  accepts a rewording only if it is 1:1 — same count, same order, verb still
  compatible with the step, no identifier-shaped token — otherwise the skeleton
  stands and the plan records `refined: false` rather than implying model
  authorship. The plan is an intention: no step is ever ticked off, and a
  milestone is constructible only from a completed fact. Greetings and one-step
  lookups get no intent and no milestones. New `[narration]` block: `mode`
  (`off|compact|auto|verbose`, default `auto`), `refine_wording`, `max_steps`.
- **One design language, and one icon per action state.**
  `nexus-core::brand::design` owns the icons, motion timings, separators, casing,
  and elapsed formatting; nothing else picks a glyph, so a reskin is a second
  `Skin` constructor rather than an edit to every renderer. Icons are keyed to
  what the agent is *doing* (`◇` tracing intent, `⌕` scanning, `▸` applying,
  `◎` checking, `◌` waiting, `◆` composing) rather than to a tool family, with a
  full ASCII fallback. Reduced motion collapses an animation to its final frame
  instead of swapping in a different design, and animation never encodes
  progress, because nothing here measures its own.

### Changed

- **No emoji on any product surface.** They are double-width, font-dependent, and
  render as boxes on several supported mobile clients. `[tui.activity].tool_icons
  = "emoji"` still parses and now resolves to geometric, so no configuration
  breaks; the tool-family glyph ladder survives as a debug-layer concern, since
  tool rows no longer appear above it. A terminal that cannot draw Unicode still
  overrides any preference, exactly as before.
- **Raw tool rows fold while narration is active** — into the milestone that
  describes them. `/view detailed|debug` reveals them whatever narration says,
  and `/narrate off` folds nothing, restoring the previous timeline. `/status`
  now prints all three axes together (`auto thinking · verbose narration ·
  default view`) because they are easy to confuse. There is deliberately no
  `debug` narration mode: raw-payload visibility belongs to `/view` and is not
  duplicated.

- **Recording a memory takes effect immediately.** `memory.add` stored every
  agent-recorded fact as a candidate that a human had to approve before it could
  be read back, so an agent that wrote down what it had just established could
  not use it, and the review queue filled with facts nobody disputed. Recording
  now applies directly. The properties that make it safe are unchanged: secrets
  are still refused, the store is still separate from the workspace, writes are
  still budgeted per turn, and everything recorded is still visible and
  deletable in `/memory`. Set `[memory].require_approval = true` to put the
  queue back.

### Fixed

- **Answers render as terminal documents, not Markdown source.** A model answer
  arrives as Markdown, and the timeline handed it to a plain word-wrapper — so
  `## Review summary`, `**Suggested fix:**`, and backticked commands reached the
  operator as literal source. There was no parser to ignore; there was no
  parser. Answers are now parsed with CommonMark (`pulldown-cmark`, tables, task
  lists, and strikethrough on, **HTML off**) into a width-independent document
  and projected to styled rows: ruled headings, real bold and italic, inline
  code without backticks, nested lists with hanging indents and per-depth
  bullets, framed code blocks with their language label, quoted bars, thematic
  rules, and responsive tables that degrade to key/value records rather than
  being crushed. Severity headings (`Critical`, `High`, `Medium`, `Low`,
  `Informational`) take a theme accent, and the word stays, so meaning is never
  carried by colour alone. The stored answer remains the canonical source —
  export and copy are unaffected — and the parse is memoised, so a resize
  re-renders rather than re-parses. Streaming is safe: an unclosed fence renders
  as provisional code, a list still filling stays a list, and an unmatched
  emphasis opener is dropped at the tip so `**` never flickers into view — using
  CommonMark's left-flanking rule, so `2 * 3` keeps its asterisk.
- **`/view` is a selector, not a flag.** Every other presentation control opens
  a menu with the current value marked; `/view` printed a report and expected
  you to retype the command with the value you wanted. Bare `/view` now opens
  the picker. Typed arguments still work as the scripting path.
- **Card headers stop stamping run outcomes on things that are not runs.** The
  operator's own message was headed `✓ DONE  USER MESSAGE` — telling them their
  typing had succeeded — and every one-sentence notice cost two rows, a status
  word above the sentence. A user message now reads `❯ You` in the composer's
  own prompt character, a short notice becomes its own single row, and an intent
  card says `Intent · 3 steps` rather than `RUNNING`, because a plan is an
  intention and is never running or done. Long and multi-line notices keep a
  separate body so they can still wrap.
- **A refused tool call no longer wedges the session permanently.** When policy
  refused an action, the turn returned immediately — after the assistant message
  carrying the `function_call` had already been persisted. The stored
  conversation was left holding a call with no `function_call_output`, and
  providers that speak the Responses API validate that pairing, so *every*
  subsequent turn came back `HTTP 400 … No tool output found for function call
  call_…`. The session was dead and no amount of retrying could clear it. Every
  early exit now records the tool result first and ends the turn after, so the
  refusal reaches the model as well as the operator. The same fix covers the
  repeated-malformed-arguments, no-progress, and failure-budget exits.
- **A second checkout no longer greets you as a stranger.** Profile cards are
  global but the active one is per-workspace, and a workspace that had never
  chosen fell back to the anonymous `default` card — so a new clone of the same
  project started as nobody and filed everything you told it onto the
  placeholder. A workspace with no explicit choice now inherits your most
  recently used card. An explicit choice is still never overwritten.
- **Saying your name again does not add another row.** Repeating any fact was
  supposed to be a no-op, and was — except along the identity path, which wrote
  straight to the table without consulting the deduplication every other write
  goes through. So the one sentence people actually repeat, "my name is …", was
  the one that accumulated: found on a live card as three identical
  `identity.name` rows. There is now a single way in, and a changed value still
  supersedes the old one rather than sitting beside it. Existing duplicates are
  left alone rather than migrated — they are harmless and yours to remove.
- **ACTIVE CONTEXT no longer buries the timeline when a phone keyboard opens.**
  On a narrow terminal the context panel was drawn as a full-body wipe painted
  last, so when a software keyboard cut the viewport height the panel covered
  the transcript, the composer, and any open approval prompt — and the work
  itself became unreachable. The layout now states its priority explicitly:
  the timeline and the composer are never what yields. `classify()` decides
  from height as well as width and guarantees the timeline a floor of rows, the
  panel is bounded to the body rect and drawn *before* modals rather than over
  them, and when there is no room for it at all it is not drawn and the status
  bar says `CTX hidden` instead of hiding silently. A panel the layout collapsed
  comes back when the keyboard closes; a panel you closed yourself stays closed.
  Resize is reconciled rather than ignored, so composer text, cursor, and scroll
  survive the trip. The reason this shipped at all: every wide terminal size in
  the test suite was also tall, and every short one also narrow — the
  wide-and-short quadrant a phone in landscape lands in had no coverage
  anywhere. It does now.
- **An agent that cannot change your profile says so instead of claiming it
  did.** Withholding the write tools from the researcher and the read-only audit
  roles stopped the write but told the model nothing, and silence read as
  permission: asked to record an occupation, the researcher answered *"I have
  recorded that"* having stored nothing. The profile section now states whether
  the role may write and that it must not claim otherwise. Refusing a write and
  reporting the refusal are separate guarantees; only the first was in place.
- **The status bar no longer offers to close a panel that is not on screen.**
  The layout decided the context panel's placement from the terminal size, but
  the welcome panel then took rows out of the body — so on a short, wide
  terminal the composer read `context · Esc close` with nothing to close. The
  geometry is now the single answer to whether the panel is showing, and the
  composer and status bar both read it.
- **The flagship agent describes itself.** `/agent` listed `nexus` as a bare row
  with no subtitle while all twenty-one other roles had one, because
  `output_contract()` — which is the *model's* contract, not a menu subtitle —
  was an empty string for it. The two are now separate: `description()` is what
  the picker renders, and it falls through to the contract for every other role,
  so no other row changes by a character.
- **The restricted-file refusal says what to do instead.** A workspace holding
  files classified as no-read — a password manager, a keystore, a repo with
  `.env` — cannot run host commands, because nothing can prove those files are
  masked from the process. The message stated the rule and stopped; it now names
  how many files blocked it and points at the two ways forward: the per-file
  tools, which are checked individually, or the container sandbox.
- **An approval card states its decision.** Answering a prompt flipped the card
  to `✓ DONE` but left the summary reading "awaiting approval", so a resolved
  request looked like a pending one. It now reads `Approved once · run: cargo
  test`, or `Awaiting your approval · …` while it is genuinely waiting.
- **A changelog headline no longer gets cut mid-word.** "What's new" read only
  the first physical line of a markdown bold span, so a wrapped headline showed
  as "…is now a real flagship agent — the def" + "ault Recursive". The span is
  rejoined and the cap falls on a word boundary.
- **The restored-session line shows a date, not a timestamp.** It printed the
  raw `2026-07-23T18:25:53.941Z`; it now reads `23 Jul 2026`.
- **Two places that leaked a tool name into operator-facing text.** A failed tool
  was narrated as `"<tool> failed: …"` and a passing validation as
  `"<tool> passed."`, both on surfaces that are supposed to be user-level. Both
  now route through the single translation layer, which replaces two partial
  implementations that disagreed with each other.
- **Startup text is emitted from one place.** The pending-proposals line — which
  still pointed at the superseded `snx profile` — moved into the boot memory
  stage and points at `/rsi`, and the two hardcoded system rows moved into the
  welcome stage.

## [2.10.2] — 2026-07-25

### Fixed

- **Typing on the timeline no longer eats letters.** A left click (or scroll)
  moves keyboard focus onto the timeline, where `j`/`k`/`d`/`n`/`N`/`y` were
  vi-style shortcuts — while every *other* letter fell through and typed into
  the composer. So after an accidental click, a word like "plan" silently lost
  its "n" and the input looked broken for one specific key. Any printable key
  pressed on the timeline now returns focus to the composer and inserts the
  character; the timeline is driven with the arrow keys, `Enter`, and `Esc`.
  `n`/`N` still jump between search matches while a search is active — a modal
  context where you are navigating results, not composing.

## [2.10.1] — 2026-07-25

### Fixed

- **`/plan <objective>` no longer answers with a session error.** Typing
  `/plan build a login form` before sending anything replied
  *"no active session — send a message first"* — the operator had just given the
  objective the error told them to give. Free text after `/plan` (anything that
  is not a stored-plan subcommand) now enters plan mode, exactly as bare `/plan`
  does, and its toast explains the next step. The `create`, `edit`, `approve`,
  `run`, `pause`, `resume`, `verify`, `history`, and `export` subcommands still
  act on a session's stored plan and still require one.
- **`snx config budgets` shows the weighted-spend guard.** The 2.10.0
  `limits.local_runaway_guard`, `limits.context_compaction`, and `limits.retry`
  settings were readable only in `config show`, though the budgets view is where
  an operator looks to tune spend — and its own footer points at
  `/config set limits.<field>`. They now appear under a *runaway guard &
  compaction* heading.
- **`snx config set tui.<field>` works.** The settable-path allowlist omitted
  `tui`, so `snx config set tui.activity.tool_icons "emoji"` was refused as an
  unsafe path even though the key is documented and its `limits.*` siblings are
  CLI-settable. `tui` holds only display preferences and no secrets; invalid
  values are still rejected by validation (`tool_icons` must be
  `geometric|emoji|ascii`).
- **"1 step", not "1 steps".** The plan panel header and the turn-completion
  line pluralized unconditionally.

## [2.10.0] — 2026-07-24

### Added

- **A real plan review.** `/plan` now opens a **PLAN AUTHORIZATION** pop-up
  showing the plan itself and four answers: *Approve*, *Approve with note*,
  *Request changes*, *Decline*. Previously the decision was routed through the
  generic tool-permission modal, which showed `tool: plan.approve` and no plan —
  you could only approve blind or deny, and denying left the run wedged at a
  blocked "Plan approval" stage. Keys: `↑↓`/`j`/`k` select, `Enter` confirm, `A`
  approve, `N` note, `R` changes, `D` decline, `PgUp`/`PgDn` scroll, `Esc` to
  decide later. *Approve with note* carries your instruction into the execution
  that follows; *Request changes* sends the plan back to be revised and reopens
  the review for the new revision only.
- **A pinned execution tracker** above the composer. While a turn runs, the
  step list stays put — `AGENT planner · EXECUTION 2/5` with `◇` pending,
  `◆` active, `✓` complete, `×` failed, `!` blocked, `–` skipped, `?` awaiting
  approval, each paired with a word where there is room. It updates in place
  rather than appending, is unaffected by scrolling the timeline, shows a window
  around the active step for long plans, and clears when the turn ends.
- **`memory.add`** — the tool an agent needs to actually record what it found.
  2.9.0 granted the memory category to every role, but no tool carried that
  category, so the grant resolved to an empty set and a reviewer told to note
  its findings still answered "no memory tool is available in this session".
  Recording is offered to every role, including read-only ones: it appends to a
  separate, budgeted (`limits.max_memory_writes`), secret-refusing store rather
  than mutating the workspace. Agent-authored entries are candidates — visible
  in `/memory` at once, retrieved into later turns after `snx memory approve`.
- **Prompt caching, and the numbers to see it.** Every turn re-sends the whole
  conversation, and until now every token of that prefix was billed at full rate
  on every call. Anthropic requests carry two `cache_control` breakpoints — one
  after the system prompt, which covers the tool schemas rendered before it, and
  one at the end of the conversation, so this turn's write is the next turn's
  read. Codex and OpenAI requests carry a `prompt_cache_key` derived from the
  conversation's opening turn, keeping repeat calls on the machine holding the
  warm copy. Usage is now reported as three numbers rather than one — uncached
  input, cache reads, cache writes — and `snx run` prints a `cache` line when
  the provider reports one. A measured three-call turn on `gpt-5.6-luna` read
  3584 of its 5881 input tokens from cache.
  - Per-model `prompt_cache = false` sends exactly what earlier versions sent;
    `prompt_cache_ttl = "1h"` buys a longer window at a higher write cost
    (Anthropic only). Both default to on/5m for metered providers.
  - Ollama and llama.cpp report nothing and are left alone: they have no caching
    API, and `keep_alive` already governs their reuse. `claude-plan` reports
    cache usage but cannot set it — the `claude` CLI owns its own request body.
  - Context usage and the aggregate token budget are unchanged: both read the
    full prompt size, cached or not. Caching changes what a turn costs, not when
    one gets stopped.
- The `/btw` aside pop-up scrolls (`↑`/`↓`, `PgUp`/`PgDn`). An answer longer
  than the pop-up was unreadable: there were no scroll keys, and the
  follow-the-bottom offset was computed from unwrapped lines, so it stopped
  short and cut off the newest text.
- **Live activity in the timeline.** A running turn showed its tool calls and
  nothing between them; the operator watched `fs.read_file` land with no idea
  why. Each turn now opens **activity segments** — `◢ REVIEWER ACTIVITY ·
  STEP 2/5` with a one-line operational summary and the tools it grouped
  (`✓ ⌗ repo.structure`, `✓ ⎇ repo.git_status`). A new segment opens when
  intent materially changes — a new plan step, a new tool family, a failure, a
  validation, a compaction, a retry — and updates in place while streaming, so
  partial chunks never duplicate a line. The text is the model's own public
  prose where it offers any, and otherwise a factual line the harness composes
  from state it already holds (active role and step, tool intent, the last
  result and the paths it touched). No second model call, no extra tokens, and
  **no private reasoning** — a silent model still narrates, where before it
  emitted the literal `[structured tool action omitted]`.
- **Per-tool glyphs**, cyberpunk-native, in a three-tier ladder: geometric by
  default (`⌗ ⌕ ⏵ ⎇ ⌸`), opt-in emoji (`[tui.activity] tool_icons = "emoji"`),
  and ASCII (`[r] [s] [x] [g]`) under `SNX_ASCII`, `TERM=dumb`, or a `C`/`POSIX`
  locale. Width is measured with `unicode_width` in every tier, so truncation is
  honest on a 40-column phone terminal.
- **429 is a first-class signal.** A provider rate or quota limit collapsed into
  a generic `Provider` error string in every adapter, and no adapter read
  `Retry-After`. The Codex, OpenAI-compatible, and Anthropic adapters now parse
  the status and the `Retry-After` / `x-ratelimit-reset-*` /
  `anthropic-ratelimit-*-reset` headers into a typed `ProviderLimit` — a short
  reset drives a bounded retry, a long one pauses the turn with its state
  preserved and the reset time shown. Missing metadata is reported as
  *unavailable*, never estimated. Ollama and llama.cpp inherit no cloud quota
  assumptions.

### Changed

- **Ordinary prompts are no longer plan-gated.** A large enough request was
  classified as "planned" work and stopped at an approval for a plan you never
  asked to review. Only an explicit `/plan` is gated now; an ordinary turn plans
  its steps, shows them in the tracker, and starts. Every individual action is
  still policy-checked, sandboxed, approval-gated and audited exactly as before.
- Plan review text names whichever agent is actually running — planner,
  reviewer, orchestrator, a configured custom agent — and falls back to a
  neutral "Agent" rather than assuming one product name.
- An exhausted memory-write budget refuses the write instead of ending the run.
  Hitting `limits.max_memory_writes_per_turn` used to stop the turn outright, so
  an agent that tried to note one thing too many threw away a finished answer.
  The refusal is reported to the model, which finishes and says what it could
  not record; a model that keeps retrying still terminates through the ordinary
  repeated-error path. (Reachable for the first time now that `memory.add`
  exists.)
- `snx run --yes` tells the model its escalations are pre-authorized. The flag
  installed an auto-approving handler but left the prompt reading
  `destructive=ask`, so the model stopped and asked a human who — in a
  non-interactive run — was not there to answer. Every action is still
  policy-checked, sandboxed, and audited; only the standing authorization is now
  stated.
- **A budget that measures risk, not work.** A productive long turn died at
  `aggregate token budget 250000 exhausted` — a monotonic sum of input plus
  output that reached the ceiling identically whether the agent read twelve
  files across a large repo or re-read one file twelve times, and that (since
  2.10.0's caching) counted cache reads at full weight though they bill at a
  tenth. The budget is now **weighted spend** — `uncached_input +
  cache_read/10 + cache_write*5/4 + output` — paired with a **progress guard**
  that accumulates pressure on repetition and relieves it on genuine discovery
  (a completed plan step, a changed file, a new validation outcome). At the
  ceiling with progress still being made, the turn **compacts its history mid-run
  and continues** under the same run identity and tracker, rather than stopping;
  it pauses (resumably) only when there is nothing left to compact or progress
  has stalled. The context gauge and the true prompt size are unchanged — this
  changes the budget's unit, not what the operator sees as context used.
- **Terminal outcomes are typed and agree everywhere.** The stop reason was a
  free-form string read by two print statements, which let a red failure be
  followed by a green `DONE` while tasks were still pending. A typed
  `RunOutcome` — `Completed`, `CompletedWithWarnings`, `Paused`, `Cancelled`,
  `Declined`, `Failed`, `StoppedByGuard`, `WaitingForProvider` — is now the one
  value the timeline, the pinned tracker, the side panel, and the status bar all
  derive from, so a paused turn never reads as complete and a local guard is
  never labelled a provider limit.

### Fixed

- `snx config reset <path>` no longer reports success for a path it did not
  drop. A key inside a real section but misspelled (`limits.max_memory_writes`
  for `…_per_turn`) answered "configuration inherited" while the override stayed
  in force. It now says what it did, and names the scope it looked in.
- `snx sandbox test` and `snx test` print the exit code, not `Some(0)`. A
  process killed by a signal now reads as such instead of `None`.
- `snx memory approve <id>` said "approved" without approving. It flipped only
  the legacy row and left the canonical record a candidate, so `/memory` and
  `snx memory list` kept reporting the memory as unapproved no matter how often
  it was approved. It now goes through the same path as the TUI, which promotes
  both.
- A custom agent that narrows its `tool_categories` keeps memory. The 2.9.0
  change covered built-in roles only, so a narrowed definition silently lost the
  one category no role is meant to be without.
- A `/btw` reply is delivered to the aside pop-up wherever it sits in the
  overlay stack. A command already in flight when the pop-up opened could land a
  report on top of it; the reply was then dropped and the pop-up wedged
  "thinking", refusing every later question.
- Re-selecting a configured Codex model no longer resets it when the plan cache
  is cold or no longer lists that model. The 2.9.0 effort re-pick routed the
  selection through the save path, which rewrote the entry with no effort and
  default limits; an already-configured entry is now pinned as it stands.

## [2.9.0] — 2026-07-24

### Added

- `/btw` now opens an **aside pop-up** you type into, instead of answering
  inline in the transcript. Ask the agent a question or hand it context there
  and it is answered by the read-only sidecar **concurrently** — the main turn's
  inference keeps running and nothing from the aside joins the conversation
  history. Whatever you say is still recorded as session-scoped side context (so
  later turns are informed without re-paying for it), exactly as before; only
  the surface changed. `/btw <note>` opens the pop-up and asks in one step;
  `/btw --list` / `--clear` still work from the command line.

### Changed

- Every agent role can now record memory. A read-only role such as **reviewer**
  previously had no memory tool at all, so instructing a review subagent to note
  its findings to `/memory` silently did nothing. Memory is a curated, budgeted
  side store rather than a workspace mutation, so granting it does not raise a
  role's write or risk ceiling.
- The reasoning-effort picker (low / medium / high / …) now reappears when you
  re-select an **already-configured** Codex model. It was only offered the first
  time a model was added; selecting a saved model afterward pinned it silently at
  its original effort with no way to change it. Re-picking updates the existing
  entry in place.

### Fixed

- The operator's name is now detected far more widely so a profile card is
  created automatically. Detection was limited to `my name is …` / `call me …`
  at the very start of a message; it now recognizes self-introductions
  (`I'm …`, `I am …`, `this is …`) **anywhere** in a message, while a
  name-shape check keeps ordinary sentences like "I am tired" from matching.

## [2.8.0] — 2026-07-22

### Changed

- `/btw` now keeps what you tell it. It was a sidecar with two endings and no
  middle: by default the answer was rendered and **discarded**, so telling snx
  "the staging base url is in `.env.local`" left the next turn none the wiser;
  with `--add` it was spliced into the transcript as a full message and re-sent
  on every subsequent turn — the exact per-turn cost the command exists to
  avoid. Both are gone. Whatever you say is recorded as **session-scoped side
  context**, compiled into each turn's prompt as its own section rather than as
  a message, so it informs later turns without ever joining the history the
  model re-reads. Questions are still answered by the read-only sidecar, and the
  answer is retained the same way.

  Notes live and die with the session — `/memory add` remains the deliberate
  path for anything durable, so an aside cannot quietly become permanent project
  memory. `/btw --list` shows what the session is carrying and `/btw --clear`
  drops it; invisible injected context would be worse than none.

- The compaction summary budget scales with the context window instead of being
  a flat 1024 tokens. The old ceiling always bound above a ~4k budget, so a 4k
  model and a 200k model got the same recap — on a large window, an entire
  session compressed into half a percent of the prompt. It is now `budget / 8`,
  floored at 256 and capped by the model's own `max_output_tokens`, which is the
  only ceiling that is physically real: the summary arrives as a completion, so
  asking for more than the model can emit buys nothing. Nothing enforced that
  before. A 32k window goes from 1024 to ~3.5k.

### Fixed

- A context section dropped for budget is now logged. `ContextCompiler` has
  always recorded these in `omissions`, and the loop only ever logged
  `conflicts`, so an eviction was silent. This matters most for the session
  summary: the messages it stands for are already marked `compacted` and are no
  longer returned by `messages()`, so dropping it removes that history from the
  prompt with nothing said anywhere.

- **Correction to 2.7.0's stated rationale.** 2.7.0's changelog, the "Long
  sessions" section of `docs/architecture.md`, its commit message, and its
  release notes all described the session summary as *pinned context the
  compiler is not allowed to trim*, and claimed an oversized one fails the turn
  outright. That is not what the code does: the summary is pushed as
  `ContextSection::optional`, which is droppable and already capped by
  `layer_cap`. The `budget exhausted: pinned context requires 2193 tokens`
  error quoted as evidence came from the baseline pinned sections not fitting
  the deliberately tiny 3000-token window used for that test, and had nothing to
  do with the summary. The 2.7.0 *fix* was still correct — the fold really did
  grow context — but the reason given for it was not, and the real failure mode
  is worse: silent eviction rather than a loud error. Both documents now say so.

- The doc comment for `fallback_compaction_summary` had been orphaned onto
  `truncate_to_tokens` when the latter was inserted in 2.7.0.

## [2.7.0] — 2026-07-22

### Changed

- Self-hosted models are configured with a **32768**-token context window, up
  from 8192. The 8192 default shipped in 2.5.0 to stop a model from timing out
  and did not: that model was running out of memory and failed at every window
  size, while every capable model on the same server lost three quarters of its
  context for nothing. Existing `limit_mode = "auto"` entries still sitting at
  exactly 8192 are lifted on load; anything else is left alone.
- An `auto` entry's window is now a pure function of its ceiling and the
  configured default — `min(context_ceiling, limits.self_hosted_context_window)`
  on every refresh, up or down. Previously the number a refresh produced could
  not be predicted from configuration alone.

- A session that outgrows its context window now continues in place with a
  **model-written** summary of what was folded away. The machinery for this
  already existed and was half-wired: `ContextCompiler` compacted history when
  it overflowed, but passed no summarizer, so the model was handed
  `deterministic_summary` — the first 200 characters of one user message and a
  sorted list of tool names — and everything else was discarded. Nothing was
  shown to the operator either: `TimelineKind::Compaction` was defined and never
  emitted by anything.

  The fold now happens in the loop before the prompt is compiled, so it can make
  a model call and persist the result. A span is summarized once instead of
  being re-derived every turn, repeated folds append rather than overwrite, and
  the system contract, the session objective, and the six most recent messages
  are never folded. Folded rows stay on disk — the transcript and audit trail
  are untouched; they are only withheld from the model, which now sees the
  summary instead of both. When the summarizing call fails the turn still
  proceeds on the mechanical outline, and the timeline card and toast say that
  is what happened rather than implying an equivalent summary.

  The summary is bounded by a share of the prompt budget rather than a fixed
  size, and a fold whose summary would not be smaller than the messages it
  replaces is skipped instead of spending a model call to lose history.

  *(Corrected in 2.8.0: this entry originally said the summary is pinned context
  the compiler may not trim, and that an unbounded one fails the turn outright.
  That was wrong — the section is droppable, and the failure it actually causes
  is silent eviction. See the 2.8.0 entry.)*

### Added

- `limits.self_hosted_context_window` (default `32768`) — one setting that moves
  the window for every `ollama` / `llamacpp` model, editable in
  `/config budgets` and printed by `snx config budgets`. To pin a single model
  instead, set its `context_window` **and** `limit_mode = "manual"`, which takes
  it out of discovery's hands; `docs/providers.md` documents both.
- `snx config set <path> <value>` and `snx config reset <path>` — the managed
  override editing that `/config set` has always done in the TUI, now reachable
  from the command line. Global by default; `--workspace` writes the
  per-checkout override instead. Without these the setting above could only be
  changed from inside the TUI or by editing TOML by hand.

### Fixed

- `snx setup` wrote `context_window = 8192` for self-hosted starter entries,
  which are unmetered. They now start at the self-hosted default.
- Setting a model's `context_window`, `context_ceiling`, or `max_output_tokens`
  by hand left the matching `*_limit_source` at whatever the last catalog
  refresh wrote, so `snx model show` and `/model` reported the operator's own
  number as having come from provider metadata. Both commands now move the
  provenance with the value, and resetting the override restores it.

## [2.6.0] — 2026-07-22

### Added

- Plan mode. `/plan` with no arguments now enters a mode instead of printing
  "no durable plan": the turn runs under a policy scope that refuses every
  write, command, network call, and delegation, the agent reads the repository,
  and it calls `plan.submit` with an ordered list of steps naming the real files
  each one touches and how it is verified. The operator approves or declines
  that plan. On approval the mode ends and the same turn continues into
  execution with the full tool surface restored; on decline the draft stays
  stored and the mode stays on so the next message refines it. `/plan exit`
  leaves without approving. The status bar and the input frame both say the mode
  is on, at every terminal width.
- `plan.submit` tool (category `Goal`, risk `Read`) and
  `WorkBreakdown::from_stages`, so a plan can be authored rather than generated.

### Changed

- A plan produced in plan mode is written by the model from what it read in the
  workspace. `WorkBreakdown::generate` — the fixed
  Grounding / Implementation / Validation template — still backs `/plan create`
  and ordinary turns, but plan mode no longer uses it. The template described
  the shape of work; it never described *this* work.
- `ui-state` v8 adds `plan_mode`. The migration forces it to `false`: a crash
  while planning must not restore an operator into a mode that then refuses
  their next edit.

## [2.5.0] — 2026-07-22

### Added

- `/config budgets` opens a real editor for the `[limits]` block. Choosing
  "Budgets…" used to type a `/config set workspace limits.max_steps_per_turn 24`
  line into the prompt and leave the operator to edit a string and guess the
  path; every other structured config surface opens a view. The form shows each
  budget's effective value in a labelled group, validates on submit, and writes
  only the fields that were actually changed, so opening it never pins defaults
  as overrides. `snx config budgets` prints the same block non-interactively.
- `limits.self_hosted_max_tokens_per_turn` (default 5000000). The aggregate
  token ceiling exists to bound spend, which is not what limits a server the
  operator runs, so turns routed to `ollama` or `llamacpp` are bounded by this
  instead. Keyed on the provider kind rather than on whether the endpoint is
  loopback — a self-hosted server is routinely reached over the network.
- `models.<name>.context_ceiling`, `first_token_timeout_secs`, and `keep_alive`.

### Fixed

- Discovered Ollama models were configured with the architecture maximum from
  `/api/show` — 262144 tokens for several common models — and that number was
  sent as `num_ctx` on every request, so the server had to allocate a KV cache
  that large before it could emit a single token. Discovery now records the
  reported maximum as `context_ceiling` and configures a runtime window the
  operator raises deliberately. Existing discovery-owned entries are migrated;
  a hand-written `context_window` is left exactly as written.
- A provider timeout was a single deadline on time-to-first-byte, which for a
  self-hosted server includes model load and prefill. Loading a model is not a
  stalled stream: there is now a separate first-token allowance (600s for
  `ollama`/`llamacpp`), while `timeout_secs` measures what it is named for —
  silence between chunks. The new `no first token after Ns` error names the knob
  to turn instead of reporting a generic timeout, and is deliberately not
  retried, since waiting out the allowance again does not fix it.
- `llamacpp` set a client-wide response deadline, which reqwest applies to the
  whole exchange. A healthy answer still arriving token by token was cut off the
  moment it outlived `timeout_secs`.
- Ollama requests now send `keep_alive`, so a model stays resident between turns
  instead of being unloaded and cold-loaded again on the next one.
- Discovered self-hosted models were saved with the general 1024-token
  completion default, which truncates mid-answer for no saving.
- Models deleted from a reachable server stayed in the configured list forever.
  Reconciliation was running and its failures were being discarded by `let _ =`,
  so neither the pruning nor an explanation ever reached the operator.
- Forms did not scroll. The overlay was sized to its field count and clipped to
  the terminal, so a form longer than the window hid its tail and the key hints
  scrolled away with it. The viewport now follows focus and the hints stay put.

## [2.4.2] — 2026-07-20

### Fixed

- Restored the timeline notices a new install depends on. 2.3.0 tiered notices by
  severity, so everything except warnings and errors folded out of the default
  view — taking the first-run guidance with it. A fresh install opened onto a
  completely blank timeline with no sign of `/setup`, `/help`, or the palette.
- Notices are now essential at every severity. A notice is the harness addressing
  the operator directly (onboarding, command results, turn summaries), which is
  categorically different from the agent's internal process events. Reasoning
  summaries, routing, policy, and context packing remain tiered as before, so the
  concise timeline still does its job.


## [2.4.1] — 2026-07-20

### Fixed

- Restored the animated activity indicator in the timeline. 2.4.0 gated the
  entire live component on the deliberation mode, so with the default `auto` it
  disappeared for any turn that did not warrant a reasoning preview — taking the
  "NEXUS …" heading, the data-stream sweep, and the elapsed counter with it, and
  leaving no sign that the agent was working.
- The gate now applies to the reasoning preview alone. The indicator renders
  whenever a turn is running, in every mode including `off`, which was always
  meant to hide planning and reasoning summaries rather than activity, tool
  execution, and progress.

## [2.4.0] — 2026-07-20

### Added

- `/thinking off|on|auto` (and `snx thinking …`) replaces the on/off reasoning
  toggle with a real deliberation mode. `off` is quiet and fast, `on` always
  shows what the agent is doing and prefers grounded staged execution, and
  `auto` — the new default — decides per request. `/thinking status` reports the
  effective settings; the bare command opens a three-mode selector that previews
  what `auto` would decide for whatever is currently typed in the input box.
- Thinking phases (understanding, planning, searching, executing, waiting,
  verifying, finalizing) derived from structured runtime state. Phases are a
  pure projection recomputed each frame, so a transition updates the one live
  component instead of appending a timeline entry.
- A summarization engine that turns execution state into concise, action-oriented
  lines — "Inspecting workspace files.", "Preparing repository comparison.",
  "Recovering after failed request." — capped at three rendered lines. It reports
  what the harness is doing rather than paraphrasing model prose, and prefers a
  provider's own summary where one exists.
- The deterministic `auto` decision reuses the existing task classifier, so the
  same request always resolves the same way. Greetings and simple factual asks
  stay quiet; coding, research, planning, verification, and anything with
  structural signals (writes, multiple files, network reach) show the component.
- `[thinking]` configuration: `mode`, `deep_planning`,
  `summarize_provider_reasoning`, and `minimum_duration_ms` (an anti-flicker
  floor so sub-second turns never flash the component). Presentation settings
  stay in `[tui.activity]` — no key is defined in two places.
- A `THINK fast|deep|auto` status-bar segment, and the resolved decision with its
  reason in the Ctrl+E activity detail.
- First-run onboarding for a configured workspace that has never been opened
  interactively, and a single contextual next step for returning operators
  (active goal, resumable session, or uncommitted changes). When there is
  nothing to point at, the timeline stays quiet rather than showing filler.

### Changed

- Deliberation mode influences *optional* deliberation only. `off` skips
  grounding and staged planning for work carrying no risk flags and lowers retry
  tolerance; `on` prefers grounded, verified execution. Safety ceilings
  (`max_steps`, tool/model-call, failure, token, cost, and duration budgets) are
  never widened, and no mode can take destructive, multi-file, migration, or
  external work out of an approved plan.
- UI state migrated to version 7. An explicit 2.3.0 `thinking_enabled = false`
  is preserved as `off`; the default `true` becomes `auto`.
- `/thinking toggle` is kept as a documented alias and now cycles off → on → auto.

### Fixed

- `/view` and `/thinking` are independent controls again. Each previously
  overwrote the other: changing timeline verbosity silently changed reasoning
  visibility, and setting thinking forced a verbosity. Both directions are cut.

## [2.3.0] — 2026-07-20

- Redesigned the inference, live-activity, tool-execution, and progress timeline
  so raw internal events no longer flood the transcript. Timeline events now carry
  a visibility tier (essential, collapsed detail, diagnostic-only) and the default
  view shows only what the operator acted on or needs to act on.
- Added `/view default|detailed|debug` (alias `/activity`, `d` on the timeline) to
  choose verbosity. The choice persists per workspace and is independent of the
  content-type filter, so `/transcript` still applies inside every mode.
- Added a live NEXUS activity component: one status row with an elapsed counter,
  up to `reasoning_preview_lines` of preview drawn from structured runtime state,
  and a pointer to the full detail. It collapses to a single row on narrow
  terminals and to a static marker under reduced motion.
- The component is labelled honestly: it reads NEXUS ACTIVITY when the preview is
  derived from the harness's own state, and only claims a reasoning state when the
  provider actually supplied a reasoning channel. Nothing is fabricated or
  relabelled as chain-of-thought.
- Added a Ctrl+E activity detail overlay with Activity/Reasoning/Tools/Policy/
  Provider/Raw tabs, scrolling, in-panel search, and a copy mode. Only tabs with
  content are built; the Raw tab exists only in debug view. Opening it leaves the
  timeline scroll position and the input contents untouched.
- Coalesced repeated events into single cards: retries against one provider update
  one card through to exhaustion, and a plan stage moving pending → running →
  completed stays one row. Set `[tui.activity].coalesce_events = false` to restore
  a card per event.
- Redesigned per-component rendering. Cards now read as sentences — `● Running
  cargo test`, `✓ fs.read_file · read 412 lines  340ms`, `✕ shell.run · exit 127`,
  `✓ Updated render.rs  +42 −7`, `△ Provider limit` — instead of a generic status
  and type row. Every state keeps a text label so it survives no-color terminals.
- Narrative event titles replaced machine-formatted summaries (for example
  "Proposing a 3-stage plan" rather than "planned work · 3 stage(s) · plan v1").
- Added the `[tui.activity]` configuration block: `mode`, `reasoning_preview_lines`,
  `show_diagnostics`, `show_token_usage`, `animation`, `animation_speed`,
  `reduced_motion`, and `coalesce_events`. All values are defaulted and validated.
- The animation now runs on a single adaptive clock: ~8fps only while a turn is
  running and motion is not reduced, and a slow idle tick otherwise, so an idle
  harness stops waking up to animate nothing.

## [2.2.0] — 2026-07-19

- Made the TUI fully responsive on mobile and narrow terminals (Termius, Blink,
  small SSH windows). A centralized breakpoint system (width and height classes)
  now drives every chrome section instead of scattered width checks.
- The top header stacks into up to three rows and abbreviates by width instead of
  clipping; the workspace path compacts to the project name and sandbox values use
  short forms (e.g. `path-only`), with full values still available.
- The bottom status bar is now a priority-ranked, width-aware segment packer that
  wraps across rows and never clips important information; overflow is surfaced with
  a `+N ⋯ Ctrl+S` hint.
- The input box grows with wrapped/multiline text (up to 3–4 rows, then scrolls) and
  is always visible with the cursor kept in view.
- Added a Ctrl+S full-status overlay (and the existing `/status`) exposing every full
  value — model, sandbox, network, workspace, git, tokens, approvals, and more.
- Timeline title, help hints, and the sidebar now adapt to the available space; a
  controlled "terminal too small" message replaces corrupted rendering below the floor.

## [2.1.0] — 2026-07-19

- Redesigned the canonical startup lockup: a five-row stepped identity mark and a
  scanline-gradient `NEXUS` wordmark that sit side by side on desktop/tablet widths
  and stack vertically on narrow terminals, with a compact fallback below that.
- Replaced the boot sequence with a calm, sub-400ms staged fade-in (identity mark →
  wordmark → supporting copy); still skippable and reduced-motion aware.
- Restored the animated "NEXUS PROCESSING" data-stream indicator in the timeline
  while the agent is working (regressed in 2.0.0); quiet under reduced motion.
- Refined the palette for contrast: brighter, calmer azure; more vibrant identity
  magenta; softer neutral gray. The single canonical lockup continues to drive the
  boot screen, `about`/`version`, welcome/home, login, and the installer banner.

## [2.0.0] — 2026-07-19

- Replaced the removed `models` command with the provider-grouped, read-only
  `catalog` command across the TUI, CLI, help, completions, and documentation.
- Added exact-provenance reasoning profiles, generic Ollama thinking discovery,
  capability-aware effort controls, and provider-call activity cards.
- Added the unrestricted general-purpose `nexus` role as the fresh-config
  default while retaining all policy, approval, sandbox, redaction, and audit
  enforcement.
- Completed responsive NEXUS terminal branding and version-derived release
  workflow paths with tag/version mismatch rejection.

All notable changes to NEXUS are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project adheres to
Semantic Versioning.

## [1.3.0] — 2026-07-19

Feature and reliability release for mobile input, provider-discovered model
metadata, Ollama streaming, and format-aware read controls.

### Added

- Termius-compatible bracketed paste with multiline Unicode input and explicit
  newline key combinations.
- Provider-wide `/model` refresh. Selectable models come only from successful
  endpoint inventories; every discovered Ollama model is enriched through
  `/api/show` without model-name rules.
- Backward-compatible automatic model limits with effective context/output
  provenance and conservative exact-ID fallback behavior.
- Schema-v1 `[policy.read_formats]` controls, searchable TUI configuration,
  normalized format-aware grants, traversal skipping, index purging, and
  sandbox masks.

### Fixed

- Ollama thinking defaults, `done_reason`, streamed error objects, header/idle
  timeouts, empty model tests, and retry safety after visible output.
- Full Access is attended-session-only and resets on bootstrap, new sessions,
  compacted sessions, and resumed sessions.

### Security

- Sensitive environment files, credentials, private keys, `.git`, and `.nexus`
  remain hard-denied even during Full Access. Weak host execution fails closed
  when restricted-file masking cannot be proven.

## [1.2.0] — 2026-07-18

Critical reliability release restoring the complete effective agent tool
surface and replacing overly broad approval behavior with revocable,
workspace-scoped grants.

### Fixed

- Removed the alphabetical six-tool truncation and combined agent/task tool
  categories so filesystem inspection, repository operations, diagnostics,
  and eligible typed terminal actions remain available.
- Codex and other providers with native tool calls are no longer classified as
  constrained solely because structured JSON output is unavailable.
- `/tools` now keeps restricted tools visible and explains their effective
  availability under the active permission mode.
- Approval prompts default to `Approve once` and distinguish one-time,
  session-only, persistent workspace, and deny decisions.
- Command grants use executable/subcommand scopes for understood command
  families and retain the complete argv for unknown structures.

### Security

- Added append-only migration `0008_workspace_approval_grants.sql` for
  revocable workspace grants; configuration schema version remains `1`.
- Raw shell, interpreters, wrappers, unproved commands, destructive/external
  actions, and unsafe host execution remain one-time-only.
- Approval grants and revocations are recorded in the audit trail.

### Deferred

- Branding, mobile input, provider token-limit discovery, and expanded
  configuration work were deferred here and subsequently delivered (mobile input
  and token-limit discovery in 1.3.0; branding and configuration in 2.0.0).

## [1.1.2] — 2026-07-18

Patch release. Makes file changes visible in the timeline: file-mutating tool
calls now render a diff card with the file path and highlighted `+`/`-` lines.
No schema changes, no new commands.

### Fixed

- Creating a file (`fs.create_file`) now shows a timeline diff card with the
  file path as a header and every new line highlighted as an added (`+`) line.
  Previously the timeline only showed a one-line "wrote …" summary with no diff
  and no path, because a diff card was emitted solely when the tool name
  contained "diff" or the output was a git patch.
- `fs.patch_file` shows the replaced text as removed (`-`) and the new text as
  added (`+`); `fs.delete` shows the removed file's contents as `-` lines; and
  `fs.move` shows the destination path. Each card carries insertion/deletion
  counts.
- The timeline diff card now renders the file path and colorizes `+`/`-`/`@@`
  lines in every surface (TUI transcript and CLI run output). Previously the
  `TimelineKind::Diff` body had no renderer and only appeared as raw JSON when a
  card was expanded, so git-diff cards also showed no path or colors.

### Changed

- Structured tool diffs travel in tool-output metadata (never in the
  model-facing content), so richer diff cards add no model-context cost.

## [1.1.1] — 2026-07-18

Patch release from a post-1.1.0 stability audit (eight-angle diff review with
per-finding verification). Fixes ten confirmed correctness, privacy, and
convention issues; no schema changes, no new commands.

### Fixed

- `/memory show <id>` no longer returns the content of a forgotten memory.
  `forget` soft-deletes (status `deleted`) so the legacy-import dedup can still
  see the row, but the by-id lookup now rejects deleted rows instead of
  rendering their full payload — a privacy regression versus 1.0's hard delete.
- `/plan pause` and `/plan resume` no longer clobber non-runnable task states.
  Only `Draft`/`Pending`/`Ready`/`Running` tasks pause, and only `Paused` tasks
  resume, so pause/resume can no longer resurrect a `Failed` task or bypass a
  `Waiting`-on-approval gate.
- `/improve apply|rollback` on a skill proposal now takes the atomic status
  transition before toggling the skill, and restores the prior status if the
  skill toggle fails, so concurrent apply/rollback can no longer leave the
  skill's enabled state decoupled from the recorded proposal status.
- `/memory approve` and `/memory reject` are now blocked while a turn is active,
  matching the other memory mutations, so they cannot race the running turn's
  own memory writes. Read-only memory subcommands still run mid-turn.
- `/profile` operations (report, review, delete-fact, rename, export) resolve
  the canonical profile on demand when a background turn established the session
  context before a profile was set, instead of failing with "no active
  profile". Resolution does not rewrite the turn context, so prompt composition
  is unchanged.
- `/resume` distinguishes provider availability from model availability: a
  configured model whose provider credential is missing/revoked now recommends
  re-authentication or a model/provider switch instead of reporting the
  environment as an exact match. (Still a synchronous credential check, not a
  live reachability probe.)
- Non-interactive `/connect` now reports local runtimes and configured
  endpoints (matching the interactive menu) instead of the hosted-auth catalog
  that belongs to `/login`.
- A dependency-parked background task (`blocked`) is no longer mislabeled as
  `waiting_approval` in session snapshots and continuation checkpoints; it
  re-queues itself once its dependency clears.
- Dependency-block detection shares a single sentinel constant between the
  writer and the auto-requeue matcher, and `retry_task` now accepts `blocked`
  tasks so an operator has a manual escape hatch if a task is ever stranded.
- `/memory export` and `/profile export` write via
  `nexus_core::atomic::atomic_write_private` (O_NOFOLLOW, same-directory atomic
  replace, `0600`) per the AGENTS.md write discipline, instead of a bare
  `std::fs::write` that followed symlinks and left default permissions.

## [1.1.0] — 2026-07-17

Silent Nexus 1.1 is the adaptive-harness release line. Automated gates
(fmt, clippy, tests, docs, audit, deny, secret scan), release packaging and
archive validation, and live remote-Ollama scenarios recorded evidence on
2026-07-17; see `docs/adaptive-harness-delivery-report.md`.

### Added (completion session, 2026-07-17)

- Background scheduler honors task dependencies: `background_task_dependencies`
  (migration `0007_task_dependencies`) gates leasing, parks dependents of
  failed/cancelled prerequisites as `blocked` with a diagnostic error, and
  self-heals them back to `queued` once every dependency completes. Cycles and
  self-edges are rejected transactionally.
- Writer background tasks claim the git repository through
  `harness_resource_claims` before creating a worktree; conflicting writers
  are parked `blocked` instead of racing, and claims release on drop or lease
  expiry.
- `/resume` validates the latest checkpoint before reattaching: environment
  fingerprint, per-file content hashes, model availability, and stale
  assumptions are re-checked via `assess_recovery`, and the recovery report is
  rendered in both the TUI attach flow and the CLI resume path.
- Weak-model adaptation: `ModelCapabilities::constrained()` (small context,
  no native tool calls, or no structured output) shrinks the planned
  decomposition before the plan is recorded
  (`WorkEstimate::constrained_for_weak_model`), truncates the tool surface,
  and clamps per-turn step/repetition budgets.
- `/agent show <role>` (capability card) and `/agent recommend <objective>`
  (deterministic classifier-based suggestion; never auto-switches).
- Command-surface completion: `/memory scopes|stats|candidates|contradictions|export`,
  `/task graph|depend|validate|assign`, `/subagents limits`,
  `/goal archive|risks`, `/persona show|reset`, `/profile rename|export`, and
  a top-level `/improve` command (list/show/approve/reject/apply/rollback)
  with status-gated apply/rollback over RSI proposals.

### Fixed (validation session, 2026-07-17)

- `snx memory forget` now accepts `--yes` so non-interactive runs can
  authorize deletion, matching its own hint and `snx profile delete`.
- Canonical memory retrieval ranks records with a deterministic
  objective-overlap score (`canonical_memory_score`); the previous tree
  referenced the function without defining it and did not compile.
- Removed a parallel-test race on process-global `GIT_CONFIG_*` variables and
  re-baselined the timeline render snapshots after visual inspection.

### Release scope (validated 2026-07-17)

- One bounded, persisted harness context linking profiles, scoped memory,
  system-prompt personas, agents, goals, plans, task graphs, subagents,
  provider/model selection, evaluation, checkpoints, and improvement
  proposals.
- Menu-first slash-command control surfaces backed by canonical domain
  services rather than display-only actions.
- Provider-neutral model request/response/reference contracts and normalized
  capability, privacy, locality, cost, latency, and fallback metadata.
- Duplicate prompt/answer and first-line rendering corrections, including
  turn-scoped terminal-event idempotency.
- Append-only `0006_adaptive_harness` and `0007_task_dependencies` migrations
  while configuration schema version remains `1` throughout the compatible
  1.x line.

## [1.0.0] — 2026-07-17

First production-certified Silent Nexus release for
`x86_64-unknown-linux-gnu`.

### Added

- Structured command analysis across shell chains, wrappers, interpreters, and
  substitutions, with hard denials for privilege escalation and generic
  terminal Git mutation bypasses.
- Explicit isolation strength and filesystem-access metadata. Container actions
  run as the invoking UID/GID with per-action read-only/write mounts,
  sensitive-path masks, network-off defaults, dropped capabilities, resource
  limits, and a digest-pinned image.
- Append-only `0005_production_hardening` migration with migration checksums,
  timeline FTS, status indexes, and backward-compatible FTS backfill.
- `snx maintenance check`, `backup`, and `optimize`, plus `snx doctor --deep`.
- Atomic private writes, permission repair, zeroized secret buffers, verified
  artifact reads, bounded/sanitized Git subprocesses, SQLite busy handling, and
  one shared stdout/stderr kill budget.
- Deterministic Linux release packaging with man page, shell completions, SPDX
  SBOM, internal/external SHA-256 manifests, CI/security/release workflows, and
  user/system installer modes.

### Changed

- Version and embedded release metadata are now `1.0.0`; the pinned Rust/MSRV
  is `1.97.0`, locked builds are required, and internal crates are
  non-publishable.
- Automatic model terminal execution requires strong container isolation.
  Host-process fallback is prominently reported as approval-only and is denied
  for unattended/background work.
- Generic model filesystem access now excludes `.nexus`, `.git`, common
  credential paths, private keys, and credential stores while preserving
  documented public examples such as `.env.example`.
- Transcript filtering pages until the requested match count is reached, while
  durable search uses SQLite FTS and loads the matching event's surrounding
  page. TUI rendering caches wrapped layouts and renders the visible range.

### Security

- Raw shell, interpreters, wrappers, substitutions, unrecognized commands, and
  unsafe host execution cannot receive session grants or auto-edit approval.
- Generic terminal `git commit`, `git push`, `git remote`, Git aliases,
  unrecognized Git subcommands, and privilege escalation are hard denied.
- Output-cap breaches terminate process groups or containers immediately,
  independent of command timeout.
- State/auth/log trees are repaired to private permissions; symlink and
  artifact-tampering attacks are rejected.
- Sensitive-path discovery, filesystem listings, and model-facing Git
  status/diff fail closed so denied credential paths cannot leak through
  metadata or repository output.

### Compatibility

- Config remains version `1`; migrations are append-only; existing timeline and
  redacted JSONL export fields remain compatible throughout 1.x except where a
  necessary security break is documented.
- Silent Nexus 1.0 does not automatically delete transcripts, tasks, plans,
  goals, memories, or artifacts.

## [0.2.0] — 2026-07-17

Cyberdeck transcript and agent-harness upgrade.

### Added

- Durable, typed execution timelines with lifecycle spans, redacted payloads,
  stable streamed cards, lazy artifacts, legacy-session projection, wrapped
  pagination, filtering, search, Markdown/JSONL export, and inline mode.
- Truthful active-work snapshots, request context manifests, complexity-aware
  work breakdowns with runtime promotion, versioned plans/stages/evidence,
  durable tasks, and agent-run state.
- Compact/expanded/raw transcript details, transcript filters, context
  inspection, focus/drawer controls, continuation checkpoints, provider
  presets, additional/custom agents, and eight accessible cyberdeck themes.
- Consent-gated official Claude CLI plan provider, native Anthropic Messages
  provider, and Gemini/Groq/Mistral/xAI/DeepSeek compatible presets.
- On-demand workspace worker with SQLite leases, stale-run recovery, three
  readers/one writer, and persistent external `snx/task/<id>` Git worktrees.
- Advanced `/plan`, `/task`, `/subagents`, `/continue`, `/details`,
  `/transcript`, `/context`, and `/export` command families.

### Fixed

- Cancellation closes running assistant/tool cards instead of leaving phantom
  activity in resumed transcripts.
- Continuation children clone the current plan/stage/evidence state and share
  rollover-root write idempotency, preventing completed parent writes from
  replaying under a new session id.
- Subagent cancel/retry updates the linked task, delegation is limited to
  audited orchestrators, root fan-out is capped at eight, and late worker
  completions cannot overwrite a newer pause/cancel state.
- Writer worktrees derive the true Git top-level and remain outside the source
  checkout even when NEXUS was invoked from a nested directory.
- Provider reset timestamps retain their original case, and context category
  token counts remain explicitly estimated even after a provider reports the
  request total.

## [0.1.1] — 2026-07-16

Correctness hotfix installed before the 0.2 orchestration redesign.

### Fixed

- Normalize every exposed and historical Codex tool name to the provider wire
  contract, including deterministic collision handling and reverse mapping.
- Validate the complete serialized Codex request locally before any HTTP
  request is sent.
- Surface deterministic HTTP 4xx failures without retrying an unchanged
  request.
- Treat non-empty prose as a completed compatibility turn and retry malformed
  action JSON once with a concise schema correction.
- Retain the post-0.1.0 authentication, credential, goal/session,
  configuration, logout, staged-file, instruction-file, atomic-state, and
  destructive-memory correctness fixes documented below.

## [0.1.0] — 2026-07-11

Initial release: a complete, real, production-grade agentic CLI harness.

### Added

- **Controlled agent loop** (`nexus-agent`): deterministic classification,
  minimal tool selection, schema-validated actions, policy/approval, sandboxed
  execution, independent verification, and bounded recovery — with a
  compatibility protocol for models lacking native tool-calling.
- **Safety core** (`nexus-core`): workspace confinement with symlink-swap
  protection, secret redaction, terminal sanitization, risk levels, layered
  config, SQLite store (WAL, 0600), audit events, content-addressed artifacts.
- **Policy engine** (`nexus-policy`): layered allow/allow_session/ask/deny with
  builtin hard-denials; destructive/external can never auto-allow.
- **Model providers** (`nexus-models`): llama.cpp, Ollama, generic
  OpenAI-compatible, custom HTTP, and mock; task routing with fallback.
- **Sandbox** (`nexus-sandbox`): container, restricted-process, and mock
  backends, each reporting honest isolation. No model downloads.
- **Typed tools** (`nexus-tools`): filesystem, repo/git, terminal (+PTY),
  SSRF-guarded web, and diagnostics.
- **Durable goals** (`nexus-goals`): evidence-verified, crash-recoverable.
- **Guarded memory** (`nexus-memory`): secret-refusing, approval-gated, FTS5.
- **Context management** (`nexus-context`): bounded packing and safe compaction.
- **Code index** (`nexus-index`): heuristic symbol extraction for grounding.
- **Skills** (`nexus-skills`): versioned, payload-free, human-enabled.
- **MCP** (`nexus-mcp`): stdio client (untrusted-by-default) and curated
  read-only server.
- **CLI** (`snx`) and full-screen NEXUS TUI with no-color mode.
- Documentation, config schema, examples, and shell completions.

### Added (post-initial)

- **Interactive agent upgrade**: `/init`, `/title`, `/summary`, `/persona`,
  `/profile`, `/thinking`, `/branch`, `/commit`, and `/connector` now share one
  canonical command registry across CLI and TUI surfaces.
- **Durable continuity**: provider token/tool/runtime usage, exit timestamps,
  persona/profile selection, exact session approval grants, summary artifacts,
  and parent/child rollover links are stored through append-only migrations.
- **Personas, profile review, and RSI proposals**: project/global persona
  inheritance, explicit low-risk workflow learning, sensitive/conflicting
  trait review, improved bounded memory ranking, and disabled-by-default
  declarative skill proposals.
- **Local Git milestone**: status, diff, stage, unstage, restore, branches, log,
  and selected-file-only commits with diff preview and confirmation.
- **Connector catalog and custom endpoints**: Codex MCP/Agent Skill discovery
  imports disabled/untrusted without credentials; remote Ollama and
  OpenAI-compatible endpoints accept host/port or URL, TLS choices, connection
  tests, and model discovery.
- **Session handoffs**: `/summary` saves and copies a structured handoff, linked
  rollovers start with only the approved summary, and `/exit`/`/logout` restore
  the terminal before printing `snx resume <session-id>`.
- **Semantic themes**: `cyberpunk` and `edgerunner` palettes cover true-color,
  256-color, ANSI, and no-color terminals.
- **`openai` provider** for GPT: defaults `base_url` to `https://api.openai.com/v1`,
  requires an API key, uses native tool-calling.
- **Codex "Sign in with ChatGPT" auth** (`auth = "codex"`): reuse an OpenAI Codex
  CLI OAuth session (`~/.codex/auth.json`) instead of an API key. New `snx auth`
  command (`status`/`login`/`logout`); `login` offers device-code, API-key, and
  same-device browser flows via the trusted `codex` CLI. The reused token is
  redaction-registered and sent as `Authorization: Bearer` (+ `chatgpt-account-id`
  for OAuth sessions).

### Fixed (post-initial)

- Compatibility-mode planner/reviewer/researcher/documentation prose now ends a
  non-tool turn normally. Only explicit action JSON can invoke a tool; one
  concise schema correction is issued for malformed action payloads.
- Startup remains available with missing hosted credentials so `/connect` can
  repair them. Existing Codex CLI credentials require explicit consent, setup
  preserves hand-written configuration, and logout drops runtime secrets by
  terminating/reloading the active application context.
- Active goals and budgets attach to new sessions; staged-file restore,
  empty/unreadable instruction selection, atomic UI-state writes, effort
  persistence, session switching, tool counts, and destructive memory
  confirmation now follow their durable source of truth.
- Agent loop retry counter no longer reports an out-of-range attempt (e.g.
  `4/3`) before stopping at the retry budget.
- Codex Responses history replay now normalizes dotted harness tool names even
  when those tools are not exposed on the current turn, preventing
  `input[n].name` HTTP 400 failures. Deterministic provider 4xx errors now
  surface immediately instead of consuming the retry budget.
- TUI: honest header now shows workspace basename so model/agent/sandbox status
  stays visible; footer scroll hint corrected to `PgUp/PgDn`.

### Changed (post-initial)

- Replaced the SILENT-dominant startup banner with one canonical, responsive
  NEXUS lockup shared by boot, `/about`, `/version`, `/welcome`, provider login,
  CLI banners, and the installer.
- Startup now reveals icon, wordmark, attribution, and tagline in 360 ms, is
  immediately skippable, and falls back cleanly for reduced motion, CI,
  redirected output, limited color, short terminals, and ASCII-only terminals.

### Security

- SSRF, private-range, cloud-metadata, and DNS-rebinding protection in web tools.
- Secrets never forwarded to sandboxes or logs; memory refuses secret content.
- Web and MCP content treated as untrusted data, never instructions.
