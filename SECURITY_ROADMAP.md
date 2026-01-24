# Security & Quality Roadmap

This document prioritizes the security review findings and tracks implementation progress toward v2.0.0.

**Status**: 10 GitHub issues total; 4 CRITICAL complete (80%)
**Last Updated**: 2026-01-24
**Progress**: Issues #1–4 ✅ COMPLETE; Issues #5–7 📋 PENDING DECISIONS; Issues #8–10 🗓️ PLANNED

---

## 📊 Summary by Priority & Status

| Priority | Count | Est. Hours | Status | Target |
|----------|-------|-----------|--------|--------|
| **CRITICAL** | 4 | 6–7 | ✅ **COMPLETE** | v1.7.2 ✓ |
| **HIGH** | 3 | 12–14 | 📋 **PENDING** | v1.8.0 |
| **MEDIUM** | 3 | 15–20 | 🗓️ **PLANNED** | v1.9.0+ |
| **TOTAL** | 10 | 33–41 | **75% TRACKED** | v2.0.0 |

---

## ✅ CRITICAL ISSUES — COMPLETE (v1.7.2)

All critical security issues have been resolved. See [CHANGELOG.md](CHANGELOG.md) for details.

### [#1] ✅ Add AAD (Additional Authenticated Data) to AES-GCM
- **Status**: ✅ **COMPLETE**
- **Est**: 4 hours
- **Impact**: Strengthens AEAD authenticity guarantees by binding optional context
- **Files Modified**: `src/core/crypto.rs`, `src/network/session.rs`, `src/transfer/sender.rs`
- **Testing**: 15 crypto tests pass; 3 new AAD-specific tests added
- **Commit**: [5d0de9c](https://github.com/fibo3090-code/secure-p2p-chat/commit/5d0de9c)
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/1

### [#2] ✅ Add Payload Size Validation Before Deserialization
- **Status**: ✅ **COMPLETE** (Pre-existing + test added)
- **Est**: 45 min
- **Impact**: Prevents memory exhaustion DoS via chunked reading validation
- **Files**: `src/core/framing.rs` (already implemented; test added)
- **Testing**: `test_framing_recv_reject_oversized_header()` validates rejection
- **Commit**: [5d0de9c](https://github.com/fibo3090-code/secure-p2p-chat/commit/5d0de9c)
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/2

### [#3] ✅ Set Up GitHub Actions CI
- **Status**: ✅ **COMPLETE**
- **Est**: 1 hour
- **Impact**: Continuous security & quality checks on all PRs
- **Files**: `.github/workflows/security.yml`, `deny.toml`
- **Components**: 6 parallel jobs (rustfmt, clippy, test, audit, build-release, cross-platform)
- **Commits**: [3df99ec](https://github.com/fibo3090-code/secure-p2p-chat/commit/3df99ec), [940ec33](https://github.com/fibo3090-code/secure-p2p-chat/commit/940ec33)
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/3

### [#4] ✅ Create THREAT_MODEL.md and Expand SECURITY.md
- **Status**: ✅ **COMPLETE**
- **Est**: 2 hours
- **Impact**: Transparency into threat scenarios, mitigations, and responsible disclosure
- **Files Created**: `THREAT_MODEL.md` (500+ lines)
- **Files Updated**: `SECURITY.md` (expanded with responsible disclosure section)
- **Contents**:
  - 6 threat actor profiles with capabilities
  - 8 detailed attack scenarios with mitigation analysis
  - Assets protection across 6 attack surfaces
  - Known limitations and future improvements
  - Responsible disclosure policy with SLAs
  - Security roadmap through v2.0.0
- **Commit**: [1fdd0ed](https://github.com/fibo3090-code/secure-p2p-chat/commit/1fdd0ed)
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/4

---

## 📋 HIGH PRIORITY ISSUES — PENDING DECISIONS (v1.8.0)

These issues require architectural decisions or significant implementation effort.

### [#5] 🔴 Harden/Modernize Signature Scheme
- **Planned for**: v1.8.0 (next release)
- **Est**: 3–10 hours (depends on decision)
- **Impact**: Replace aging RSA-2048 with modern Ed25519
- **Decision Needed**: 
  - **Option A (Recommended)**: Full Ed25519 migration (8–10h)
    - Smaller keys (32 bytes vs. 2048 bits)
    - Faster signature operations
    - Better side-channel resistance
    - Dual-mode handshake for backward compatibility
  - **Option B (Interim)**: RSA-PSS hardening (3–4h)
    - Use PSS padding for identity signatures
    - Doesn't address RSA-2048 weakness (112-bit security)
    - Still requires Ed25519 migration later
- **Files Affected**: `src/identity/mod.rs`, `src/core/crypto.rs`, `src/network/session.rs`
- **Documentation**: Update `docs/04_protocol.md`, `SECURITY.md`, `THREAT_MODEL.md`
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/5

### [#6] 🔴 Implement Replay Protection & Session Key Rotation
- **Planned for**: v1.8.0 (parallel track)
- **Est**: 4–5 hours
- **Impact**: Long-term key exposure reduction; improves forward secrecy
- **Implementation**:
  - Automatic session key re-negotiation (every hour or configurable)
  - Explicit replay attack prevention (sequence numbering enhanced)
  - Periodic re-handshake to derive fresh session keys
- **Files Affected**: `src/network/session.rs`, `src/app/chat_manager.rs`
- **Testing**: Add replay detection tests, key rotation stress tests
- **Documentation**: Update `SECURITY.md` and `docs/04_protocol.md`
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/6

### [#7] 🔴 Harden Invite Links & Token-Based Sharing
- **Planned for**: v1.8.0 (low-risk addition)
- **Est**: 1–4 hours (depends on decision)
- **Impact**: Prevents tampering with invite links; improves TOFU security
- **Decision Needed**:
  - **Option A (Recommended)**: Signed invites with Ed25519 (3–4h)
    - Sign invite link with identity key
    - Receiver validates signature
    - Prevents tampering with IP, port, or fingerprint
  - **Option B (Simpler)**: Checksum-only approach (1h)
    - Add BLAKE3 checksum to invite
    - Lower security than signatures; catches accidental corruption
- **Files Affected**: `src/identity/mod.rs`, `src/app/chat_manager.rs`, `src/types.rs`
- **Interactions**: Depends on #5 decision (use Ed25519 for signing)
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/7

---

## 🗓️ MEDIUM PRIORITY ISSUES — PLANNED (v1.9.0+)

These issues improve code quality, testing, and maintainability.

### [#8] 🟡 Add Unit Tests, Integration Tests & Fuzzing
- **Planned for**: v1.9.0
- **Est**: 9–12 hours
- **Impact**: Catches regressions early; ensures crypto correctness
- **Scope**:
  - Expand unit test coverage (target: 85%+)
  - Add integration tests for multi-peer scenarios
  - Add fuzz tests for protocol parsing
  - Add security-specific tests (timing attacks, key derivation, signature validation)
- **Files**: `tests/**/*.rs`, `src/**/*` (test modules)
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/8

### [#9] 🟡 Add CONTRIBUTING.md, CODE_OF_CONDUCT.md, Signed Releases
- **Planned for**: v1.9.0
- **Est**: 2–2.5 hours
- **Impact**: Community guidelines; verifiable releases
- **Scope**:
  - Write CONTRIBUTING.md (dev workflow, PR guidelines)
  - Add CODE_OF_CONDUCT.md (community standards)
  - Enable GPG signing for releases (GitHub tag signing)
  - Create release checklists
- **Files**: CONTRIBUTING.md, CODE_OF_CONDUCT.md, release process docs
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/9

### [#10] 🟡 Implement Typed Errors & Structured Logging
- **Planned for**: v1.9.0+ (ongoing improvement)
- **Est**: 4–7 hours
- **Impact**: Better error messages; searchable logs for debugging
- **Scope**:
  - Replace string errors with `thiserror` types
  - Add structured logging (JSON) via `tracing`
  - Improve error context propagation
- **Files**: Throughout codebase, especially `src/network/`, `src/core/`
- **Link**: https://github.com/fibo3090-code/secure-p2p-chat/issues/10

---

## 🔮 Future Work (v2.0.0+)

Beyond the 10 tracked issues, planned major initiatives:

1. **Professional Security Audit** (4–6 weeks)
   - Engage independent cryptographic audit firm
   - Review handshake, key derivation, protocol implementation
   - Publish findings and remediation plan

2. **Session Key Rotation Policy** (already in #6)
   - Implement automatic key re-negotiation

3. **Encrypted Identity Keystore** (v1.8.0 or later)
   - Use Argon2 + AES-256 to encrypt private keys at rest
   - Currently stored plaintext on disk (HIGH RISK)

4. **Post-Quantum Cryptography (PQC)** (v2.0.0)
   - Hybrid classical/PQC handshake (CRYSTALS-Kyber, CRYSTALS-Dilithium)
   - Prepare for quantum threats

5. **Hardware Signing Support** (v2.0.0+)
   - FIDO2 / security key integration
   - Isolate identity key to secure enclave

6. **Onion Routing (Tor Integration)** (v2.0.0+)
   - Hide IP addresses from peers
   - Defend against traffic analysis

---

## Decision Matrix: What to Work On Next

**Recommend starting with Issue #5** (Signature Scheme Hardening):

1. **Make decision**: Ed25519 (Option A) or RSA-PSS (Option B)?
2. **Assign**: Designer + cryptographer to pair
3. **Estimate**: 8–10 hours for full implementation + testing
4. **Parallel track #6**: Key rotation (can start simultaneously)

Once #5 is decided, #7 can leverage its results (Ed25519 for signing invite links).

---

## Progress Tracking

**Last Update**: January 24, 2026

- ✅ Issues #1–4: Complete (80% of critical work)
- 📋 Issues #5–7: Ready for assignment (awaiting decisions)
- 🗓️ Issues #8–10: Backlog (can start after #1–7)

**Next Review**: When #5 decision is finalized and implementation starts

---

## Related Documentation

- **[SECURITY.md](SECURITY.md)** — Security policy, threat model, responsible disclosure
- **[THREAT_MODEL.md](THREAT_MODEL.md)** — Detailed threat analysis and attack scenarios
- **[CHANGELOG.md](CHANGELOG.md)** — Release notes including v1.7.2 complete items
- **[CONTRIBUTING.md](CONTRIBUTING.md)** — Development guidelines
- **[docs/04_protocol.md](docs/04_protocol.md)** — Protocol v3 technical specification
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
