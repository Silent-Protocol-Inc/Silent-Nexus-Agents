## Outcome

Describe the operator-visible result.

## Safety and compatibility

- [ ] Workspace, policy, approval, sandbox, redaction, and audit boundaries remain intact.
- [ ] No secret or credential material was added.
- [ ] Migrations are append-only and config/export compatibility was considered.
- [ ] Documentation reflects the actual isolation and behavior.

## Validation

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`
- [ ] Relevant adversarial/security checks

List exact commands and results:
