# Data management

## Locations

Per-workspace durable data is under:

```text
<workspace>/.nexus/config.toml
<workspace>/.nexus/state/nexus.db
<workspace>/.nexus/state/artifacts/
<workspace>/.nexus/state/logs/
```

User configuration and auth profiles use the platform config directory. On
Linux this is normally `~/.config/silent-nexus`, subject to `XDG_CONFIG_HOME`.

Generic model filesystem tools cannot access `.nexus`; state is managed only by
trusted harness code and explicit maintenance commands.

## Retention

Silent Nexus 1.0 does not automatically delete transcripts, tasks, plans,
goals, memories, or artifacts. Memory TTL pruning occurs only through the
documented memory workflow. Removing an installed binary does not remove data.

## Integrity

SQLite uses WAL, foreign keys, busy timeout, bounded retries, and migration
checksums. Artifact reads validate state-root confinement, no-follow regular
files, recorded size, and SHA-256. Private trees are `0700`/`0600`.

Run `snx maintenance check` to inspect database integrity, journal state,
storage size, migration checksums, permissions, and all recorded artifacts.

## Backup

```sh
snx maintenance backup /absolute/new/backup-directory
```

The destination must not exist. The command uses SQLite's online backup API,
copies artifacts without following symlinks, writes a SHA-256 manifest, and
renames the completed snapshot atomically.

Backups contain `nexus.db`, `artifacts/`, and `manifest.json`. Store them with
the same confidentiality as the original state.

## Restore

1. Stop all foreground/background Silent Nexus processes.
2. Preserve the current state directory separately.
3. Verify every file against `manifest.json`.
4. Restore `nexus.db` and `artifacts/` as one consistent set into a private
   state directory.
5. Run the same or newer verified `snx` binary.
6. Run `snx maintenance check` and `snx doctor --deep`.

Do not combine a database from one snapshot with artifacts from another.

## Optimization

`snx maintenance optimize` runs `PRAGMA optimize` and a WAL checkpoint.
`--vacuum` additionally rewrites the database and is refused while work is
active. Neither mode deletes logical records.
