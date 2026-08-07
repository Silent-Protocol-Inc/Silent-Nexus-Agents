# Silent Nexus Harness Design

This directory is the governing design layer for the Silent Nexus CLI harness.
It describes the product's identity, non-negotiable safety properties, and the
architecture that lets the CLI and full-screen TUI remain one instrument.

It is not a replacement for implementation documentation:

- [`HARNESS_CONSTITUTION.md`](HARNESS_CONSTITUTION.md) governs how changes are
  made and what floors may not be lowered.
- [`HARNESS_GENOME.md`](HARNESS_GENOME.md) defines what makes a harness belong
  to Silent Nexus.
- [`HARNESS_ARCHITECTURE.md`](HARNESS_ARCHITECTURE.md) owns the system model,
  boundaries, state, and authority relationships.
- [`../architecture.md`](../architecture.md) remains the implementation-level
  per-turn pipeline.
- [`../cli-reference.md`](../cli-reference.md) remains the command contract.

## Reading order

1. Constitution — before changing policy, approval, storage, or operator trust.
2. Genome — before changing interaction, presentation, or command semantics.
3. Architecture — before changing crate boundaries, event flow, or state.
4. Existing subsystem documents — for the concrete implementation contract.

## The product in one sentence

Silent Nexus is an operator-controlled, evidence-producing boundary around an
untrusted model: it turns intent into bounded action without hiding who decided,
what was allowed, what happened, or what remains unproven.

## Status

Version 0.1 · 2026-08-08 · Initial governing design layer, derived from the
current 1.x implementation and repository policy.

