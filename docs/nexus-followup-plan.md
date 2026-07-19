# NEXUS follow-up plan

The following work remains after v1.3.0 and is intentionally isolated from the
completed input, model-limit, provider-refresh, and file-format permission work.

## Timeline reasoning UX

- Add a selectable `Thinking…` row under the main execution timeline. `Ctrl+E`
  expands safe summaries and operational status only; hidden chain-of-thought
  is never displayed or persisted.

## Configuration

- Complete the interactive `/config` experience.
- Introduce a default unrestricted `nexus` agent through a separately reviewed
  permission and safety design.

## Branding

- Finish the exact five-row `NEXUS` wordmark.
- Make the `◤◢` identity responsive across terminal widths.
- Resolve the current branding compile gaps and complete its tests.

The six-file branding snapshot is preserved locally on
`wip/branding-followup-2026-07-18` and is not part of v1.3.0.
