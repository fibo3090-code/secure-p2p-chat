# Security & Quality Roadmap

This document prioritizes the security review findings and tracks implementation progress.

**Status**: 10 GitHub issues created across 3 priority levels  
**Last Updated**: 2026-01-24  
**Reviewer**: Independent security audit (external)

---

## 📊 Summary by Priority

| Priority | Count | Est. Hours | Status |
|----------|-------|-----------|--------|
| **CRITICAL** | 4 | 6–7 | 🔴 Not started |
| **HIGH** | 3 | 12–14 | 🔴 Not started |
| **MEDIUM** | 3 | 15–20 | 🔴 Not started |
| **TOTAL** | 10 | 33–41 | 🔴 Not started |

---

## 🔴 CRITICAL (Fix This Week)

Must complete before next release or external sharing.

### [#1] Add AAD (Additional Authenticated Data) to AES-GCM
- **Est**: 4 hours
- **Impact**: Strengthens AEAD authenticity guarantees
- **Files**: `src/core/crypto.rs`, `src/network/session.rs`, `docs/04_protocol.md`
- **Owner**: [Assign]
- **Status**: 🔴 Not started
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/1

### [#2] Add Payload Size Validation Before Deserialization
- **Est**: 45 min
- **Impact**: Prevents memory exhaustion DoS
- **Files**: `src/core/framing.rs`
- **Owner**: [Assign]
- **Status**: 🔴 Not started
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/2

### [#3] Set Up GitHub Actions CI
- **Est**: 1 hour
- **Impact**: Continuous security & quality checks
- **Files**: `.github/workflows/security.yml`, `deny.toml`
- **Owner**: [Assign]
- **Status**: 🔴 Not started
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/3

### [#4] Create THREAT_MODEL.md and Expand SECURITY.md
- **Est**: 2 hours
- **Impact**: Transparency, user understanding, responsible disclosure process
- **Files**: `THREAT_MODEL.md`, `SECURITY.md`, `README.md`
- **Owner**: [Assign]
- **Status**: 🔴 Not started
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/4

**Subtotal CRITICAL**: ~7.75 hours

---

## 🟡 HIGH (Weeks 1–2)

Important security hardening; plan for next release cycle.

### [#5] Harden or Migrate Signature Scheme
- **Est**: 3–10 hours (Option B: RSA-PSS=3–4h, Option A: Ed25519=8–10h)
- **Impact**: Modernize crypto, improve signature security
- **Files**: `src/identity/mod.rs`, `src/core/crypto.rs`, `src/network/session.rs`
- **Owner**: [Assign]
- **Status**: 🔴 Not started
- **Decision**: Choose Option A (Ed25519) or Option B (RSA-PSS)
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/5

### [#6] Implement Explicit Replay Protection & Rekey Policy
- **Est**: 4–5 hours
- **Impact**: Prevents nonce exhaustion on long sessions; explicit ordering policy
- **Files**: `src/network/session.rs`, `src/lib.rs`, `docs/04_protocol.md`
- **Owner**: [Assign]
- **Status**: 🔴 Not started
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/6

### [#7] Harden Invite Links (URL-safe Base64 + Signing)
- **Est**: 1–4 hours (Option B: Checksum=1h, Option A: Signed=3–4h)
- **Impact**: Prevents tampering with invite metadata
- **Files**: `src/app/chat_manager.rs`, `src/core/crypto.rs`, `docs/04_protocol.md`
- **Owner**: [Assign]
- **Status**: 🔴 Not started
- **Decision**: Choose Option A (Signed) or Option B (Checksum)
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/7

**Subtotal HIGH**: ~12–19 hours

---

## 🟠 MEDIUM (Months 1–2)

Quality, testing, and process improvements.

### [#8] Add Comprehensive Unit/Integration Tests & Fuzzing
- **Est**: 9–12 hours
- **Impact**: Catch regressions, fuzz for edge cases
- **Files**: `tests/`, `fuzz_targets/`, `Cargo.toml`
- **Owner**: [Assign]
- **Status**: 🔴 Not started
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/8

### [#9] Add CONTRIBUTING.md, CODE_OF_CONDUCT.md, Signed Releases
- **Est**: 2–2.5 hours
- **Impact**: Community standards, contributor onboarding
- **Files**: `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `CHANGELOG.md`, `.github/workflows/release.yml`
- **Owner**: [Assign]
- **Status**: 🔴 Not started
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/9

### [#10] Switch to Typed Errors & Structured Logging
- **Est**: 4–7 hours
- **Impact**: Better error handling, debuggability, secret redaction
- **Files**: `src/**/errors.rs`, `src/main.rs`, module updates
- **Owner**: [Assign]
- **Status**: 🔴 Not started
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/10

**Subtotal MEDIUM**: ~15–21.5 hours

---

## 🎯 Execution Strategy

### Week 1: CRITICAL Issues
1. Start with [#3] CI setup (fastest ROI — runs on all future PRs)
2. Parallel: [#2] Payload size validation (quick win)
3. Parallel: [#1] AAD implementation (most complex, start early)
4. Finalize: [#4] Threat model documentation

**Exit criteria**: All CRITICAL issues merged and CI is passing

### Weeks 2–3: HIGH Issues
1. [#5] Signature scheme (pick Ed25519 or RSA-PSS and commit)
2. [#6] Replay protection & rekey policy
3. [#7] Harden invite links

**Exit criteria**: Protocol hardening complete

### Weeks 4+: MEDIUM Issues
1. [#8] Tests & fuzzing (high effort but valuable)
2. [#9] Community docs (quick, unlocks contributions)
3. [#10] Error/logging refactor (ongoing, not blocking)

---

## 📋 Acceptance Criteria Checklist

Before marking issue as done:
- [ ] Code changes merged to main
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes (no warnings)
- [ ] `cargo fmt` passes (code formatted)
- [ ] Documentation updated
- [ ] PR reviewed by at least one other maintainer
- [ ] No regressions detected in manual testing

---

## 🔗 Related Documents

- [THREAT_MODEL.md](THREAT_MODEL.md) — what we protect, attacker models, limitations
- [SECURITY.md](SECURITY.md) — vulnerability disclosure, best practices
- [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md) — architecture, development workflow
- [docs/04_protocol.md](docs/04_protocol.md) — protocol specification

---

## 📞 Questions / Blocking Issues

If you encounter blockers or need clarification on any issue, comment on the GitHub issue or email: [security@example.com]

---

**Generated**: 2026-01-24  
**Next Review**: After CRITICAL issues are complete
