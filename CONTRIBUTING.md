# Contributing

Thank you for contributing. This document covers the contribution process: branching, commits, local checks, and pull requests. For the technical side — toolchain, build and run commands, code map, and the release process — see [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md).

Before changing code, read the doc that matches the kind of change you are making; the canonical index is [docs/README.md](docs/README.md).

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
cargo test --workspace --doc           # nextest does not run doctests
```

If you changed anything under `desktop/`, also run the frontend gates — they are
all enforced by CI:

```bash
cd desktop
npm run lint      # a deliberately narrow rule set: bugs, not style
npm test          # vitest: pure modules in node, React components in jsdom
npm run build     # and commit the rebuilt desktop/dist/, which is tracked
npm run smoke     # Playwright: the only gate that actually starts the app
```

If you touch packaging or release files, validate the relevant scripts manually.

## Bug Reports

Use the GitHub issue forms for bug reports and feature requests so reports include reproduction, impact, and environment details up front.

Check the docs first so the report is based on current behavior, not an outdated assumption.

Security issues must **not** be reported in public issues — see the [responsible disclosure process](SECURITY.md#responsible-disclosure) instead.

## Pull Request Checklist

- [ ] Scope is focused and reviewable.
- [ ] Tests added or updated, or rationale provided.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes.
- [ ] `cargo nextest run --workspace` (or `cargo test --workspace`) passes.
- [ ] `cargo test --workspace --doc` passes.
- [ ] If `desktop/` changed: `npm run lint`, `npm test`, `npm run build` and
      `npm run smoke` pass, and the rebuilt `desktop/dist/` is committed.
- [ ] Relevant docs updated.
- [ ] `CHANGELOG.md` updated for user-visible changes.

## Documentation Rules

- Do not duplicate large explanations across multiple files.
- Prefer updating the canonical doc and linking to it.
- If protocol behavior changes, update `docs/protocol.md`.
- If architecture or module ownership changes, update `docs/architecture.md`.
- If security guarantees or limits change, update `SECURITY.md` and `THREAT_MODEL.md`.
- If UX changes materially, update `docs/USER_GUIDE.md` or `DESIGN_NOTES.md`.

Releases are cut by maintainers; the release checklist lives in [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md#release-checklist).
