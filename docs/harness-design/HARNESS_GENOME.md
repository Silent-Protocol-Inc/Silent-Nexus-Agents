# Silent Nexus Harness Genome

**Status:** Governing · **Version:** 0.1 · **Owner:** Silent Protocol

The Genome defines the product identity. It is not a colour palette, terminal
theme, or list of commands. A future CLI or TUI belongs to Silent Nexus when it
preserves these loci while changing its expression.

## The seven loci

### H-1 · The operator remains the authority

The harness may advise, classify, plan, and explain; it may not turn model
output into permission. **Breach test:** an action can execute because the model
said it was safe, without an independent policy and consent path.

### H-2 · Every action has a visible boundary

An action exposes its capability, target, risk, policy result, approval scope,
isolation, and outcome. **Breach test:** the operator sees “running” but cannot
tell what will be touched or why it was allowed.

### H-3 · The timeline is the product's memory of work

Conversation is not enough. Intent, routing, activity, approval, execution,
diff, validation, interruption, and final result form a durable event story.
**Breach test:** a resumed session cannot reconstruct what happened or which
claims remain unverified.

### H-4 · The CLI and TUI are two views of one control plane

Commands, permissions, state transitions, and output semantics are shared.
The TUI adds focus, menus, and timeline presentation; it does not create a
second implementation of the product. **Breach test:** a command is safe or
functional in one surface but silently different in the other.

### H-5 · Safety is legible, not merely present

Sandbox strength, network state, redaction, policy decisions, and limitations
are communicated in operator language. **Breach test:** a weak host guardrail
is presented as containment, or a fallback is presented as equivalent to the
preferred path.

### H-6 · Continuity survives interruption

Provider limits, terminal resize, restart, denial, timeout, compaction, and
partial execution preserve useful state and offer a bounded recovery path.
**Breach test:** interruption forces the operator to guess what already ran.

### H-7 · Every conclusion carries its evidence status

The harness separates fact, observation, inference, proposal, and unknown, and
shows the evidence source where it matters. **Breach test:** a model-generated
summary is indistinguishable from an independently verified result.

## Character

The harness should feel calm, exact, inspectable, and quietly opinionated. It is
not a chatbot shell, a magic automation layer, or a dashboard that hides risk.
Its visual hierarchy follows operational urgency: current task, current action,
operator decision, result, then background detail.

## Inherited design grammar

These principles are deliberately borrowed from the website design system's
method, not its visual language:

- **The system makes an argument.** A harness surface must say what decision or
  understanding it supports; generic “activity” is not a sufficient purpose.
- **Identity is structural before chromatic.** The product should remain
  recognisable with colour disabled because its timeline, evidence order,
  command vocabulary, and approval grammar carry the identity.
- **Meaning before ornament.** Rules, badges, icons, numbering, spinners, and
  motion must encode a true state or relationship. Nothing exists only to make
  the terminal feel busy.
- **Restraint is a resource.** Spend visual emphasis on the one thing requiring
  operator attention. A screen with five urgent indicators has made none clear.
- **Consistency transfers knowledge.** Reuse command grammar, state words,
  approval choices, and evidence labels so an operator can move between CLI,
  inline mode, and TUI without relearning authority.
- **Adaptation follows the subject.** A repository review, provider picker,
  approval, and recovery flow need different compositions because their tasks
  differ; a universal card layout is not a virtue.

## Adaptation rule

New commands, panels, providers, and personas may change wording and density,
but they must answer four questions: what is happening, who authorised it, what
can it touch, and what evidence proves the result. Identity is carried by these
relationships, not by a fixed cyberpunk look.

## Honest weaknesses

- The product is information-dense; narrow terminals can make the safety story
  harder to scan.
- Human approval is a deliberate bottleneck and cannot be designed away.
- Provider differences mean “model available” never guarantees equal behavior.
- Timeline richness can become noise; default presentation must remain concise.
- A specification cannot prove keyboard, screen-reader, or real operator
  comprehension without testing the built binary with people.
