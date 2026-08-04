# Personas

A persona is the behavioral identity a turn runs under: who the assistant is,
how it speaks, and how it carries itself. It is not a label, a colour, a style
preset, or a prompt prefix.

**Exactly one behavioral persona reaches the model on every turn** — the persona
you selected, or the built-in `Nexus` identity when you have not selected one.
Never both, never zero.

## What a persona controls

Identity, name, characterization, role, personality, temperament, tone, manner,
vocabulary, communication style, response format and length, narrative
perspective, emotional expression, relationship framing, roleplay behavior,
creativity, expertise framing, reasoning style, humor, profanity preferences,
romantic behavior, mature themes, conversational boundaries, and content
preferences.

## What a persona can never control

Filesystem permissions, shell access, network access, credentials, environment
secrets, provider authentication, sandbox scope, destructive tool access,
approval requirements, role capabilities, execution budgets, privileges, system
policy, audit settings.

All of those are decided in runtime code before a single token is generated,
which is why persona text asking for them changes nothing. A persona may change
how a reviewer *talks*; it cannot give the reviewer an implementer's tools.

## The prompt layers

Authority, strongest first. This is the order that decides which layer wins a
conflict and what gets shed when the context budget runs short:

```
immutable safety rules              ← enforced in code; nothing below relaxes them
provider protocol requirements
enforced policy and sandbox scope
approved active profile
ACTIVE BEHAVIORAL PERSONA           ← exactly one, always
OPERATIONAL AGENT CONTRACT          ← what the role owes, never who it is
goal · approved plan · current task · memory · session context
the user's request
```

The persona layer is pinned, so budget pressure sheds optional context —
memories, observations, older history — and never the assistant's identity.

**Wire order is a different question, and the persona goes first.** It opens the
system block; every other instruction follows it. Authority and position are
separate concerns — rank decides who wins a conflict and what gets shed, position
decides what the model reads as the frame for everything after. A correctly
ranked persona buried in the middle of a long operational prompt is read as
background prose, and models answered as the assistant the surrounding text
described. Putting it first makes the rest of the prompt read as instructions
given *to* that character rather than as a competing description of one. Rank is
unchanged.

The section opens with one short sentence naming the persona — `Your name is
<name>.` — and then the stored text, byte for byte. It is short on purpose: a
persona's own text already establishes who it is, so the directive only has to
remove the ambiguity a provider's default identity would otherwise fill. It
grants nothing; everything above the persona is enforced in code before a token
is generated.

A persona may also carry its own sampling — see below.

`/persona inspect-effective` prints the exact section body that would be sent,
whether the directive is present, the sampling in force, and what shape the next
turn takes.

## Per-persona sampling

Voice is not only wording. A terse analytical persona and a florid roleplay one
want genuinely different sampling, and running both at whatever the model config
happens to say makes the second read flat and drift back toward assistant-speak.

A persona can therefore set `temperature` (0.0–2.0) and `max_output_tokens`,
which override the model's values for turns where that persona is active and for
nothing else:

```
snx persona create "Cartographer" --temperature 1.1 --max-output-tokens 2048 …
```

Both are optional and independent. An unset field is not a zero — it leaves the
operator's own model configuration alone. Values a provider would reject are
refused when the persona is saved, not halfway through the first turn that uses
it.

## Conversational turns

A turn that needs no tools does not carry the tool machine. When the request
classifies as simple and there is no active goal, pending task, or tracked plan,
the prompt omits the approved plan, the operational contract, the role charter,
and the tool inventory.

It keeps the safety rules, the policy and sandbox scope, project instructions,
the profile, memory, and the persona.

This is presentation, not enforcement. Policy, sandbox, workspace confinement,
approval, redaction, and audit all live outside the model, so a conversational
turn cannot do anything a full turn could not. Anything that looks like work
gets the full prompt, and if a narrowed turn does need a tool it says so and the
next turn is classified again with everything attached.

Turn it off with `conversational_turns = false` under `[persona]`.

## Managing personas

`/persona` opens the manager. `Create persona…` opens **PERSONA FORGE**: a real
multiline editor with paste, cursor movement, undo/redo, a live character and
token count, and a raw view showing the exact text that will be stored and sent.
Nothing writes a half-finished command into the message composer.

The forge has two editing modes, toggled with `^R`:

- **Structured** — optional sections (Identity, Role, Personality, Tone,
  Relationship framing, Communication style, Response behavior, Content
  preferences, Roleplay behavior, Formatting, Examples, Custom instructions).
  Only sections you filled in appear in the composed prompt.
- **Raw** — the prompt itself. Switching between the two never loses text.

Keys: `Tab` field · `←→` choose · `Enter` newline in the editor · `PgUp`/`PgDn`
step · `^S` save · `^Z`/`^Y` undo/redo · `Esc` cancel (twice when there are
unsaved changes).

Every action is also available non-interactively:

```
snx persona list
snx persona create <name> <instructions...> [--content-profile …] [--activate]
snx persona duplicate <source> <new-name>      # independent copy
snx persona derive <source> <new-name> <text>  # live link to the base
snx persona edit <id> <instructions...>
snx persona select <id>   # alias: use
snx persona disable
snx persona status
snx persona inspect <id>
snx persona inspect-effective [--json]
snx persona test [--model <name>] [prompt...]
snx persona export <id> [--path file] | snx persona import <file|-> [--activate]
snx persona delete <id>
```

Both surfaces call the same services, so neither can drift.

## Derivation

`snapshot` (the default) copies the resolved text once and cuts the link:
editing the copy cannot touch the source, and deleting the source cannot break
the copy. `extend` keeps a live reference — the base is resolved at prompt time
and the derived text follows it. Self-inheritance, cycles, missing parents, and
excessive depth are refused.

## Content profiles

`General`, `Mature`, `Adults-only`, `Custom`. This is **metadata**: it labels a
persona for the manager, the status bar, and future session defaults. It is
never consulted when the prompt is built, so it cannot add a hidden content
rule, soften text you wrote, or change a single character of your persona.

An adults-only persona asks once for an acknowledgment that it is intended for
adult participants and adult fictional characters. Only the fact and its
timestamp are stored — no identity data is requested and no verification is
performed. The status bar marks it with a bare `+`, which says the persona is
classified without disclosing anything it contains.

## What validation does and does not do

A persona is refused only for technical reasons: empty, larger than the
configured maximum, malformed encoding, terminal control sequences, invalid
inheritance, an unreadable import, or an embedded credential under the existing
secret-handling policy.

It is never refused, rewritten, summarized, softened, or euphemised for being
mature, adult, sexually explicit, romantic, profane, unconventional,
controversial, roleplay-oriented, or violent in fiction. There is no keyword
list. What you write is what is stored, and what is stored is what is sent.

**The provider is a separate matter, and it is the limit of what any of this can
do.** A hosted backend applies its own instructions and content policy on the
server, above anything sent from here. It may answer in its own voice, ignore
the persona, or decline — and delivering the persona better does not change
that. A ChatGPT-backend model answering "I'm ChatGPT, an AI assistant" is the
provider's policy speaking, not a persona that failed to arrive;
`/persona inspect-effective` will show the persona present and correct in the
same session.

That refusal is reported as the provider's answer. Silent Nexus does not rewrite
your persona to pre-empt it and does not reroute to another provider to get
around it. Running a local model is how you remove the other party from the
decision, and it is your call to make.

## Providers

How a persona is delivered depends on the adapter, and the inspector reports it
honestly:

| Channel | Adapters | Authority |
|---|---|---|
| system role | Ollama, OpenAI-compatible, local endpoints | full |
| dedicated instructions field | Anthropic, Codex Responses | full |
| prefix fallback | Claude CLI bridge | **weaker** — disclosed |
| unsupported | — | persona cannot be delivered |

A weak channel is stated as a limitation rather than presented as equivalent.

Note for local models served through Ollama: when a request supplies a system
message, Ollama does **not** also apply the model's `Modelfile` `SYSTEM` block.
Silent Nexus always sends one, so the selected persona is the only application
instruction the model receives.

## Agents, subagents, and goals

Persona and agent are separate. The agent decides task responsibility, tools,
workflow, permissions, budget, and read/write capability; the persona decides
voice and manner. Switching roles replaces the operational contract and leaves
the persona alone.

A delegated subagent shares the parent's session and active context, so it
**inherits the active persona** and changes only its operational contract. It
gains no permissions from persona content.

A goal records the persona active when it was created, so resuming it restores
the identity it ran under.

## Persistence

The selected persona survives ordinary turns, application restart, `/resume`,
model switching, provider switching, agent switching, context compaction, and
goal continuation. Selecting is recorded as a persona id and revision, so a
later edit is visible as a new revision rather than a silent substitution.

Persona definitions are stored apart from conversation history, profile facts,
long-term memory, provider credentials, and agent definitions. **Fictional
persona content is never extracted as a fact about you** — a persona's biography
is the character's, not the operator's.

## Exports

`snx persona export` writes a portable document: name, description, system
prompt, content profile, category, tags, inheritance metadata, revision,
compatibility notes, recommendations, and schema version. It never contains
credentials, authentication data, chat history, profile information, long-term
memory, or runtime policy. Imported text is preserved exactly; an imported
mature or adults-only persona is not refused for its classification.

---

Apache-2.0 © Silent Protocol.
