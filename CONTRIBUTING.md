# Contributing

This repository is intentionally documented in layers. Before changing code, read the doc that matches the kind of change you are making instead of guessing from stale comments.

## Read First

- `README.md`: project overview, quick start, current feature set.
- `docs/README.md`: canonical documentation index.
- `docs/USER_GUIDE.md`: user-facing behavior and troubleshooting.
- `docs/03_architecture.md`: current module boundaries and state flow.
- `docs/04_protocol.md`: current wire protocol and handshake behavior.
- `SECURITY.md`: shipped protections, limits, and disclosure policy.
- `docs/05_platform_spec.md`: platform plan, roadmap, and backlog.

## Contribution Rules

1. Branch from `main` with a focused name such as `fix/ipv6-parse` or `docs/protocol-cleanup`.
2. Use Conventional Commits where practical: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`.
3. Keep changes scoped. Avoid mixing protocol work, UI refactors, and unrelated cleanup in one PR.
4. Add or update tests for any non-trivial behavior change.
5. Update docs in the same PR when behavior, architecture, or security posture changes.

## Local Workflow

Run these before opening a PR:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace          # or: cargo test --workspace
```

If you changed the Tauri desktop crate (`desktop/`), also run
`cargo check -p p2pem-desktop` and `cd desktop && npm run build` (it has no
automated tests). If you touch packaging or release files, validate the relevant
scripts manually.

## Bug Reports

Use the GitHub issue forms for bug reports and feature requests so reports include reproduction, impact, and environment details up front.

Check the docs first so the report is based on current behavior, not an outdated assumption.

## Pull Request Checklist

- [ ] Scope is focused and reviewable.
- [ ] Tests added or updated, or rationale provided.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes.
- [ ] `cargo nextest run --workspace` (or `cargo test --workspace`) passes.
- [ ] If `desktop/` changed: `cargo check -p p2pem-desktop` and `npm run build` pass.
- [ ] Relevant docs updated.
- [ ] `CHANGELOG.md` updated for user-visible changes.

## Documentation Rules

- Do not duplicate large explanations across multiple files.
- Prefer updating the canonical doc and linking to it.
- If protocol behavior changes, update `docs/04_protocol.md`.
- If architecture or module ownership changes, update `docs/03_architecture.md`.
- If security guarantees or limits change, update `SECURITY.md` and `THREAT_MODEL.md`.
- If UX changes materially, update `docs/USER_GUIDE.md` or `DESIGN_NOTES.md`.

## Release Notes

For releases:

1. Move relevant `CHANGELOG.md` entries out of `Unreleased`.
2. Bump the version in `Cargo.toml`.
3. Re-run quality checks.
4. Verify packaging scripts if release artifacts are being produced.
