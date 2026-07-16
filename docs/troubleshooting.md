# Troubleshooting

## Start with evidence

Run:

```sh
snx --version
snx about
snx doctor --deep
snx sandbox status
snx maintenance check
snx logs
```

Use `--json` for automation. Do not include credentials or unredacted provider
payloads in reports.

## Container unavailable

`auto` selects a container only when Docker/Podman responds and the exact pinned
image is already local. Pull the documented digest explicitly, then rerun
`snx sandbox test`. If `auto` falls back to process execution, automatic and
background terminal actions remain denied; attended actions require one-time
unsafe-host approval.

## Permission or symlink error

Bootstrap repairs private directories to `0700` and files to `0600`. Symlinks
inside private state/auth trees are rejected. Inspect the reported path; replace
unexpected symlinks only after confirming their origin and preserving a backup.

## Migration checksum mismatch

Do not edit `schema_migrations` or `migration_checksums`. A mismatch means
shipped migration history differs from the binary. Preserve the database,
confirm the binary/archive SHA-256, and restore a backup or reinstall the
correct binary.

## Database busy

Silent Nexus applies a busy timeout and bounded retries for foreground/worker
contention. If contention persists, stop other `snx` workers, rerun
`maintenance check`, then `maintenance optimize`. Never delete WAL/SHM files
while a process has the database open.

## Artifact integrity failure

An artifact read validates path confinement, regular-file/no-follow status,
size, and SHA-256. Treat a failure as corruption or tampering. Restore the
artifact and database together from the same backup.

## Output cap or timeout

The process group/container is killed immediately when the shared stdout/stderr
budget is crossed. Reduce command verbosity or use a typed tool that stores
full output as an artifact; do not raise limits without reviewing denial-of-
service risk.

## Older config rejects the container image

1.0 requires `name@sha256:<64 hex>` for `sandbox.container_image`. Replace
floating tags with the documented pinned digest and review the image before use.
