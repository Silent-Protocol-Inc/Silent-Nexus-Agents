# Upgrade from 0.2 to 1.0

1. Stop foreground/background `snx` processes.
2. Preserve the existing `.nexus` directory and create an external backup.
3. Install the verified 1.0.0 binary atomically.
4. Update any floating `sandbox.container_image` to the documented digest.
5. Run `snx doctor --deep`; opening the workspace applies append-only migration
   `0005_production_hardening`.
6. Run `snx maintenance check`.

Migration 0005 adds checksums, timeline FTS, indexes, and backfills existing
timeline rows. Migrations 0001-0004 are not rewritten. Existing timeline and
redacted JSONL export fields remain compatible.

Behavioral security changes:

- automatic/background terminal execution now requires strong container
  isolation;
- process fallback requires one-time attended approval for every model terminal
  action;
- raw shell/interpreters/wrappers/unproved commands cannot use session grants or
  auto-edit;
- generic terminal privilege escalation and Git commit/push/remote/alias or
  unrecognized operations are denied;
- generic model filesystem tools cannot access `.nexus`, `.git`, or credential
  paths;
- container images must be digest pinned and are not pulled automatically.

State trees are repaired to private permissions during bootstrap. Symlinks in
private state are rejected. New artifact paths are relative; verified legacy
absolute paths inside the state root remain supported.

To roll back the binary, reinstall the previous verified executable. To roll
back state/schema, restore the complete pre-upgrade database and artifacts
backup; do not manually remove migration records.
