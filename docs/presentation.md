# Presentation: boot, status, timeline, debug

NEXUS presents itself in four layers with one design language. This document is
the contract between them.

## The layers

| Layer | Answers | Sources | Never shows | Switch |
| --- | --- | --- | --- | --- |
| **Boot** | "What am I coming back to?" | completed startup facts | raw logs, internal timings, a stage that did not run | always (skippable) |
| **Status** | "Is it alive, and on what?" | phase, elapsed, reported effort, active intent step | tool names, commands, paths, percentages, ETAs | while a turn runs |
| **Timeline** | "What happened?" | translated milestones, results, diffs, approvals, errors | tool names, argument JSON, raw output | `/narrate` |
| **Debug** | "What *actually* happened?" | everything, unmodified | *nothing is hidden here* | `/view debug` |

The rule that keeps them apart:

> Boot, Status, and Timeline may render only what the translation layer emitted.
> Debug renders the untranslated truth.

That is structural, not a convention. The three product layers consume a
`Presented` value (`nexus-agent/src/narration/translate.rs`), and `Presented`
has no field for a tool name, an argument blob, or raw output — a leak is a
compile error. Tests assert the other half: for every tool in the real registry,
no translation writes its name into the text.

The same fact reads differently per layer. Status says *Running checks*; the
timeline later says *Tests passed (14s)*; debug shows
`terminal.exec cargo test -j2 … exit 0`.

### Where each surface sits

The TUI carries all four layers and switches between them. **`snx run` renders
at the debug layer** and always has: it is a non-interactive runner whose output
is a log, it has no `/view` to turn detail back on, and a CI job that cannot see
which command failed is worse off than a noisy one. It shares the intent card
and the milestones with the TUI — that is what keeps the two surfaces from
drifting apart — and prints the raw tool rows underneath them.

## The three axes

Three verbosity-adjacent controls, each owning exactly one question:

| Control | Question | Values |
| --- | --- | --- |
| `/thinking` | how much *optional deliberation* the harness does | `off` `on` `auto` |
| `/narrate` | whether the agent *says what it is doing* | `off` `compact` `auto` `verbose` |
| `/view` | which *stored events render* | `default` `detailed` `debug` |

> **Narration folds; `/view` reveals.**

While narration is active, raw tool rows fold into the milestone that describes
them. `/view detailed|debug` brings them back whatever narration says, and
`/narrate off` folds nothing — it restores the pre-narration timeline exactly.

There is deliberately no `debug` narration mode: raw-payload visibility belongs
to `/view` and is not duplicated. `/status` prints all three together
(`auto thinking · verbose narration · default view`) because they are easy to
confuse. None of them changes what runs, what is checked, or what needs
approval.

### What each narration mode shows

| | intent | milestones | wording refined | tool rows | status line |
| --- | --- | --- | --- | --- | --- |
| `off` | — | — | no | shown | **yes** |
| `compact` | yes | failures, approvals, check results | no | folded | yes |
| `auto` *(default)* | yes | meaningful milestones | yes | folded | yes |
| `verbose` | yes | every observed action | yes | folded | yes + step |

## Boot — the wake flow

```
        ▓▒░ NEXUS ░▒▓
  ◈  Session restored · fix the tier check · feat/nexus-rsi-flagship · 2026-07-30T…
  ◆  Memory linked · 14 facts · 3 awaiting review · 2 improvement candidates — /rsi
  ◇  What's new · Governed self-improvement
  ✓  Ready · 3 uncommitted changes here — /diff to review, /commit to record
```

Every stage is omitted when it has nothing real to say — a fresh workspace shows
only `Ready`. "What's new" is read from the changelog compiled into the binary,
so it cannot claim a feature this build lacks, and it appears once per version.
There is no progress bar, because startup is not measurable in advance.

## Status line

```
  ◇ Tracing intent · 24 seconds · high effort     ← wide
  ⌕ Scanning the workspace · 3 seconds            ← no effort reported → omitted
  ◎ Running checks · 1m04s                        ← narrow: short elapsed
  ▸ Applying changes                              ← mobile: verb only
  ◌ Waiting on your approval · 8 seconds          ← blocked, warning color
                                                  ← idle: the row does not exist
```

It renders whenever a turn is running, in *every* narration and thinking mode:
liveness feedback is not verbosity. It is a render-time projection that performs
no store write, so it cannot append to the record it sits above.

Effort appears only when the provider reported one. The step counter needs both
`verbose` and a real intent plan. A verb holds for a dwell window so a fast tool
sequence cannot strobe through four words in a second.

## Timeline — intent and milestones

A task turn opens with 2–5 steps:

```
◈ INTENT
  1. Read the failing test and its module
  2. Apply the fix
  3. Run the suite and report
```

The steps come from a **deterministic skeleton** built from the same task class
and work estimate the work breakdown uses. A model may improve the *wording*;
the gate accepts only a 1:1 rewording — same count, same order, each step still
opening with a verb compatible with what that step is, and no identifier-shaped
token. Anything else keeps the skeleton and records `refined: false` rather than
implying model authorship.

The refinement is one small completion (`temperature 0`, 256 tokens, 8-second
leash) on task-shaped turns in `auto` and `verbose`, and nothing at all
otherwise. Every way it can go wrong — no provider, an error, a timeout, an
unparseable answer, a rejected rewording — lands on the same place: the
skeleton, unchanged. `refine_wording = false` removes the call entirely and
keeps the deterministic half.

The plan is an **intention**. No step is ever ticked off; progress comes from
milestones, and a milestone is constructible only from a completed fact.

Greetings and one-step lookups get no intent and no milestones at all.

## The design language

`nexus-core/src/brand/design.rs` owns the icons, motion timings, separators, and
casing. Nothing else picks a glyph. Reskinning means writing a second `Skin`
constructor, not editing every renderer.

| Action state | Icon | ASCII |
| --- | --- | --- |
| Tracing intent | `◇` | `?` |
| Shaping the approach | `◈` | `*` |
| Scanning the workspace | `⌕` | `/` |
| Applying changes | `▸` | `>` |
| Running checks | `◎` | `=` |
| Waiting (on you / on provider) | `◌` | `.` |
| Composing the answer | `◆` | `+` |
| Done | `✓` | `v` |
| Failed | `✕` | `x` |
| Needs approval | `△` | `!` |

Icons name an **action state**, not a tool family: the operator cares that a
change is being applied, not which function was called. Tool-family marks still
exist for the debug layer.

**No emoji.** They are double-width, depend on an installed font, and render as
boxes on several supported mobile clients. A legacy
`[tui.activity].tool_icons = "emoji"` still loads and resolves to geometric.

**Motion is cosmetic.** Nothing here measures its own progress, so no animation
implies it, and reduced motion collapses an animation to its final frame rather
than swapping in a different design.

## Configuration

```toml
[narration]
mode = "auto"           # off | compact | auto | verbose
refine_wording = true   # allow one bounded model pass over the intent wording
max_steps = 5           # clamped to 2..=5
```

An explicit `/narrate` choice is stored in UI state and outranks this from then
on, exactly as `/thinking` and `/view` do for their own axes.

## Truthfulness rules

1. Nothing is presented before it happened — enforced by the types.
2. The skeleton is the source of truth; refinement changes wording only.
3. Intent is intention; no step is marked done from the plan alone.
4. Unknown is omitted, not invented — no effort when none was reported, no boot
   line for a stage that did not run, no ETA or percentage anywhere.
5. No tool names, paths, arguments, or output above the debug layer.
6. Never private reasoning.
7. Silence over filler.
8. Degradation is recorded, not hidden.
9. Animation never represents progress.
