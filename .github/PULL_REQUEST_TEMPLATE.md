## Summary

- What does this change do?
- Why is this the right scope?

## User Impact

- What changes for users, operators, or contributors?
- Are there packaging, CI, or release implications?

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo nextest run --workspace` (or `cargo test --workspace`)
- [ ] If `desktop/` changed: `cargo check -p p2pem-desktop` and `npm run build`
- [ ] Manual smoke test performed when relevant

## Docs And Release Notes

- [ ] Docs and in-app help updated if behavior changed
- [ ] `CHANGELOG.md` updated for user-visible changes
- [ ] Security and protocol docs updated if guarantees or wire behavior changed
