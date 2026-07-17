# Silent Nexus 1.x compatibility contract

Throughout the 1.x line:

- documented CLI command names, option meanings, and exit behavior remain
  compatible unless a necessary security break is announced;
- config schema remains version `1`; newly introduced fields are optional with
  secure defaults;
- database migrations are append-only and upgrade every earlier shipped
  migration level;
- existing timeline fields and redacted JSONL export fields remain present and
  retain meaning; new fields may be added;
- artifact records with verified legacy absolute paths inside the state root
  remain readable, while new records use relative paths;
- no release silently deletes transcripts, tasks, plans, goals, memories, or
  artifacts.

Security boundaries take precedence over convenience. A bypass discovered in
an existing command, config, or approval flow may be closed even if that
removes unsafe behavior. Such changes must be documented in the changelog and
upgrade guide.

Linux `x86_64-unknown-linux-gnu` is the certified 1.0.0 baseline and the 1.1.0
release-tooling target. The 1.1.0 release is certified only after its delivery
report records passing gates. Other targets are experimental; source
compatibility does not imply release certification.

Rust/MSRV remains exactly `1.97.0` for 1.1.0. Later 1.x releases may raise it
only with release notes and CI/toolchain changes.
