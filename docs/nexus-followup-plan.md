# NEXUS follow-up plan

The following work is intentionally excluded from v1.2.0 so the critical tool
access and scoped-approval fixes can ship without unrelated UI risk.

## Input and provider behavior

- Make multiline input reliable in mobile terminals, including Termius.
- Discover and apply hosted-provider token limits automatically.

## Configuration

- Complete the interactive `/config` experience.
- Add correct per-language configuration-file handling.
- Introduce a default unrestricted `nexus` agent through a separately reviewed
  permission and safety design.

## Branding

- Finish the exact five-row `NEXUS` wordmark.
- Make the `◤◢` identity responsive across terminal widths.
- Resolve the current branding compile gaps and complete its tests.

The six-file branding snapshot is preserved locally on
`wip/branding-followup-2026-07-18` and is not part of v1.2.0.
