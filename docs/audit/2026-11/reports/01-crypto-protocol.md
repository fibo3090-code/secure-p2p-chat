# Cryptography, protocol and identity security

## Scope covered (list every file read)

Read in full:
- `core/src/core/crypto.rs`, `core/src/core/protocol.rs`, `core/src/core/framing.rs`, `core/src/core/mod.rs`
- `core/src/network/session.rs`
- `core/src/identity/mod.rs` (all non-test code, plus the list of tests), `core/src/identity/invite.rs`
- `core/src/lib.rs`, `core/src/util.rs`
- `client/src/app/persistence.rs`
- `SECURITY.md`, `THREAT_MODEL.md`, `docs/protocol.md`

Read in part, to follow a call path out of scope:
- `client/src/app/chat_manager/events.rs` (TOFU decision, `fingerprint_known_trusted`)
- `client/src/app/chat_manager/connect.rs` (`spawn_session`, `connect_to_contact`)
- `client/src/app/chat_manager/contacts.rs` (`import_contact`, `promote_contact_verified`)
- `core/src/party/mod.rs` (`FrameSeq`, `send_framed`, `recv_framed`)
- `server/src/state/mod.rs` (`join`), `server/src/dispatch.rs`, `server/src/connection.rs` (where the fingerprint is used)
- `core/src/types.rs` (Debug derives, `SessionEvent`)
- Dependency sources: `pem-rfc7468-0.7.0` (`grammar.rs` `strip_preamble`, `decoder.rs`), `spki-0.7.3` (`from_public_key_pem`), `der-0.7.10` (`Document::from_pem`), `aes-gcm-0.11.1` (`zeroize` feature)
- The workspace and `core` `Cargo.toml` files, and `Cargo.lock` for crate versions

Nothing was built or run. Every finding comes from reading the code.

## Summary

The primitives are sound and used correctly in most places: X25519 with a contributory check, HKDF with a transcript salt, AES-256-GCM with role-separated counter nonces, Argon2id with capped parameters, strict UTF-8 and bincode decoding, and constant-time password comparison. The serious problems are in how identity is computed and bound, not in the primitives.

1. **Critical: fingerprint spoofing (CRYPTO-01).** The fingerprint that TOFU, contact trust and Party membership rely on is computed by a hand-written PEM scan (`fingerprint_pubkey`). The key that signatures are checked against comes from a different, RFC 7468 parser that accepts a preamble. A PEM string can carry a victim's key in the "preamble" and the attacker's key in the parsed block. The result is a victim's fingerprint backed by the attacker's signature. Anyone who has a victim's public key (it is in every invite link) can:
   - impersonate a verified peer with no prompt;
   - pass a Party server pin;
   - join a community as its Owner;
   - mint a "signed" invite naming the victim's fingerprint, which redirects an existing verified contact's address to the attacker.
2. **High: the SAS can be ground (CRYPTO-02).** There is no commitment, so a MITM (for example a relay operator) picks its last ephemeral key after seeing the other side's. The documented claim of "one guess per connection" is false. The six digits alone take about 10^6 trials, which is trivial.
3. **Medium: no direction binding (CRYPTO-03).** Both directions use one key and one AAD, and decrypt never checks the role bit. An on-path party can reflect a peer's own frames back to it as the other peer's, both during the handshake and mid-session.
4. **Medium: smaller trust gaps.**
   - Invite fingerprints are never compared with the handshake fingerprint (CRYPTO-04).
   - The connection password is sent to a host whose identity has not been verified yet (CRYPTO-05).
   - Two rekeys inside one round trip can drop the old key and kill the session (CRYPTO-06, suspected).
5. **Docs.** Several claims are wrong: SAS coverage, the Ed25519 proof format, "password never exposed", and invite expiry in `THREAT_MODEL.md`.

## Findings

### CRYPTO-01 — The fingerprint and the verified key come from two different PEM parsers, so a preamble spoofs any identity
- Severity: Critical
- Category: security
- Status: VERIFIED (traced through this repo's code and the exact locked dependency sources; no proof-of-concept test was run)
- Location:
  - `core/src/core/crypto.rs:75-95` (`fingerprint_pubkey`)
  - `core/src/core/crypto.rs:49-51` (`pem_decode_public`)
  - `core/src/network/session.rs:327-358` (`verify_identity_proof` verifies against `pem_decode_public(&proof.public_key_pem)`)
  - `core/src/network/session.rs:552-553` (`established` computes `peer_fingerprint = fingerprint_pubkey(peer.public_key_pem)`)
  - `core/src/identity/invite.rs:98-111, 139-154, 193` (invite signature checked against `pem_decode_public`, binding checked with `fingerprint_pubkey`)
  - `core/src/identity/mod.rs:234-253` (duplicate of the same scan)
  - Dependency: `pem-rfc7468-0.7.0/src/grammar.rs:61-81` (`strip_preamble`), called from `decoder.rs:186-188`, reached through `spki-0.7.3 traits.rs:38-41` and `der-0.7.10 document.rs:82-84`
- Problem:
  - `fingerprint_pubkey` hashes the base64 body between the *first* `-----BEGIN PUBLIC KEY-----` substring and the *first* `-----END PUBLIC KEY-----` substring found anywhere in the string. If base64 decoding fails, it falls back to hashing the raw bytes.
  - `pem_decode_public` (`RsaPublicKey::from_public_key_pem` → `pem_rfc7468::decode_vec`) follows RFC 7468 and **skips a preamble**: any bytes without a NUL, up to the first `\n` that is followed by `-----BEGIN `. A preamble that does not start exactly at `-----BEGIN ` can therefore hold a complete PEM block of its own.
  - The two functions read different keys out of a string like this:

    ```text
    "x-----BEGIN PUBLIC KEY-----\n<VICTIM b64>\n-----END PUBLIC KEY-----\n-----BEGIN PUBLIC KEY-----\n<ATTACKER b64>\n-----END PUBLIC KEY-----\n"
    ```

    - `fingerprint_pubkey` finds the victim block (offset 1), base64-decodes it, and returns **the victim's fingerprint**.
    - `pem_decode_public` treats everything up to `\n-----BEGIN` as preamble and returns **the attacker's key**.
- Impact / failure scenario: the victim's public key is in every invite link (`SignedInvitePayload.public_key`), and a host sends its identity proof to anyone who connects. With it, an attacker can:
  1. **Impersonate a verified P2P peer silently.** The attacker sends an `IdentityProof` with the crafted PEM, signed with the attacker's own key. `verify_identity_proof` succeeds under the attacker's key, `peer_fingerprint` comes out as the victim's, and `handle_tofu_verification` accepts it without a prompt: `fingerprint_known_trusted` at `client/src/app/chat_manager/events.rs:152-160`, or the stored fingerprint matches at `:92`. Any on-path attacker or relay operator gets a complete MITM. So does anyone the user can be induced to dial.
  2. **Bypass a Party server pin.** The client compares `tunnel.peer_fingerprint` (`core/src/party/mod.rs:860`) with the pin, so a fake server presenting the real server's fingerprint receives the user's community credentials.
  3. **Take over a Party membership, including the Owner seat.** `server/src/state/mod.rs:474-482` re-admits a member purely on `fingerprint`, and `server/src/connection.rs:135` takes that fingerprint from the tunnel. Anyone with the Owner's public key (from an invite they shared) who knows the community password, or where there is none, becomes the Owner.
  4. **Forge a "signed" invite for someone else.** The attacker builds an invite whose `public_key` is the crafted PEM, signs it with the attacker's key, and sets `fingerprint` to the victim's. `rsa_verify_pss` passes (attacker key) and `check_fingerprint_binding` passes (victim fingerprint). `import_contact` (`client/src/app/chat_manager/contacts.rs:77-102`) then *refreshes* the existing, verified victim contact with the attacker's `address`, `addresses` and relay details, keeping its `Verified` state. The next connection to "the victim" goes to the attacker, who passes TOFU through path 1. The documented defence "an invite's fingerprint and public_key must agree" is defeated.
- Recommendation:
  - Derive the fingerprint from the **parsed** key, never from text:

    ```rust
    let key = pem_decode_public(pem)?;
    hex(sha256(key.to_public_key_der()?))
    ```

    Remove the raw-bytes fallback (return `Result<String>`). For honest PEMs the output is unchanged, because SPKI DER is canonical.
  - Additionally require `pem == pem_encode_public(&key)` (exact canonical form) for `IdentityProof.public_key_pem` and for invite `public_key`. Reject anything else, with no preamble, no extra blocks and no CRLF variants.
  - Make `Identity::calculate_fingerprint` call the same function.
  - Add regression tests:
    - the crafted string above must be rejected by `verify_identity_proof`/`established`, by `decode_signed` and by `decode_unsigned`;
    - a full handshake presenting it must fail;
    - Party join with it must not map to an existing member.
  - Add the `fingerprint_pubkey(pem) == fingerprint(parse(pem))` property to `core/tests/fuzz_parsers.rs`.
  - Treat this as a security advisory: it affects every released version that has this code.
- Effort: S (fix) / M (tests, advisory, release)

### CRYPTO-02 — The SAS has no commitment, so a MITM can grind its last ephemeral key until both SAS values match
- Severity: High
- Category: security
- Status: VERIFIED (protocol structure); the cost figures below are estimates
- Location:
  - `core/src/core/crypto.rs:493-522` (`derive_sas`, and its doc claim at `:501-502` that the attacker "gets one guess per connection attempt")
  - `core/src/network/session.rs:574-664` (wire order: host sends its ephemeral first, the client chooses its own after seeing it)
  - `core/src/network/session.rs:457-488` (transcript = versions + ephemerals only)
  - `core/src/network/session.rs:854, 910` (SAS = `derive_sas(transport_aad)`)
- Problem:
  - The SAS is a deterministic function of `host_version‖client_version‖E_host‖E_client`, with no hash commitment (unlike ZRTP and similar designs).
  - A MITM between Alice (client) and Bob (host) plays host to Alice and sends `E_M2`. Alice replies with `E_A`, which fixes `SAS_A`.
  - The MITM then connects to Bob as a client. After receiving `E_B`, it is free to try candidate `E_M1` values until `derive_sas(E_B, E_M1) == SAS_A`. It has up to `HANDSHAKE_TIMEOUT_SECS` (15 s) before Bob's `recv_peer_ephemeral` gives up, and holding Alice's side open is easy.
  - Each trial costs one incremental curve-point addition plus about 8 SHA-256 compressions (salt, HKDF extract/expand).
- Impact / failure scenario:
  - SAS is the primary MITM check: the UI leads with it and the fingerprint is demoted to "advanced".
  - Matching only the six digits (~2^20) takes well under a second on one core. Users comparing "the numbers" are fully defeated.
  - Matching the full ~2^35 within 15 s needs roughly 2×10^9 trials per second, which is GPU-class but feasible for a determined relay operator or network adversary.
  - Once the codes match, both users accept the MITM's fingerprints.
- Recommendation:
  - Add a commitment so neither side can choose its ephemeral key after seeing the other's. For example:
    1. The client first sends `H(E_C ‖ client_version)`.
    2. The host sends `E_H`.
    3. The client reveals `E_C`.
    4. The host checks the hash before deriving anything.
  - Include both identity public keys (DER) in the SAS input; the documentation already claims this.
  - Consider a longer code (for example 4 emoji + 6 digits). The UI should ask users to compare all of it.
  - Fix the docs (see CRYPTO-16).
  - This is a wire change: bump `PROTOCOL_VERSION` to 4 and keep v3 behind a "legacy, weaker SAS" warning.
- Effort: M

### CRYPTO-03 — Both directions share one key and one AAD, and decrypt never checks the role bit, so frames can be reflected
- Severity: Medium
- Category: security
- Status: VERIFIED
- Location:
  - `core/src/network/session.rs:29, 475-485` (one key and one `transport_aad` for both directions)
  - `core/src/core/crypto.rs:524-544, 586` (role bit set on encrypt), `:673-702` (`decrypt` ignores it)
  - `core/src/network/session.rs:1180-1217` (receive path; replay check only against the peer's last seq)
  - `core/src/network/session.rs:316-358` (the identity signature covers only the signer's own ephemeral key: no role, no peer ephemeral, no transcript)
  - The Party framing has the same root cause: `core/src/party/mod.rs:732-775`
- Problem:
  - `NonceRole` keeps the two directions' nonces disjoint, which prevents nonce reuse. But no direction is authenticated: a ciphertext produced by A decrypts at A.
  - **Mid-session:** if A's outgoing seq is higher than B's (common, since every file chunk consumes a seq), an on-path party (for example a bridging relay) can inject A's own earlier frame into A's inbound stream. A accepts it as B's `Text`, `FileMeta` or `Ack`. A's `last_valid_seq` then jumps, and B's genuine frames are silently dropped as "replays" (`Received::Continue`, log only) until B's counter catches up.
  - **During the handshake:** reflecting every host packet back to the host completes a handshake with "itself". The ephemeral key is accepted (`h·hG` is contributory), the reflected proof verifies against the reflected ephemeral, and the peer fingerprint is the host's own, which the TOFU code does not reject. With no connection password, the user gets a prompt for a "peer" that echoes everything.
  - The password-gate frames also share the transport key and AAD with message-loop frames, so there is no domain separation.
- Impact / failure scenario:
  - Messages appear to come from the peer when they were actually the user's own.
  - The peer's real messages are silently lost, while the peer sees them as sent (no ✓ receipt).
  - The integrity guarantee of the authenticated channel is broken, which matters to the stated "active network attacker" threat model.
  - The attacker cannot create new plaintext, so the impact is limited.
- Recommendation:
  - Now, compatible with existing peers: in `Transport::receive` (and `recv_identity_proof` / `recv_framed`), reject any frame whose `nonce[0] & 0x80` does not equal the **peer's** role bit. Both roles already set it.
  - Reject a peer ephemeral key equal to our own, and a peer fingerprint equal to our own.
  - Next protocol version:
    - derive two directional keys (HKDF `info = "…/h2c"` and `"…/c2h"`);
    - put the sender role in the AAD;
    - give the password gate its own AAD label;
    - have identity proofs sign `role ‖ transcript_hash` instead of only their own ephemeral key.
  - Add a test that reflects a frame back into its sender and expects it to be refused.
- Effort: S (role-bit check) / M (directional keys)

### CRYPTO-04 — An invite's fingerprint is never compared with the fingerprint presented on first connection
- Severity: Medium
- Category: security
- Status: VERIFIED
- Location:
  - `client/src/app/chat_manager/connect.rs:431-516` (`connect_to_contact` dials but never seeds or checks an expected fingerprint)
  - `client/src/app/chat_manager/events.rs:91-138` (a `None` stored fingerprint leads to a plain first-use prompt)
  - `client/src/app/chat_manager/contacts.rs:278-306` (`promote_contact_verified` silently does nothing on a mismatch)
- Problem: a contact imported from a signed invite carries a fingerprint the user got out of band. When the user dials that contact, the new chat starts with `peer_fingerprint: None`, so whatever key answers is shown as an ordinary first-use prompt. There is no warning that it differs from the invite.
- Impact / failure scenario: a MITM (relay operator, DNS/LAN attacker) presents its own key when the user connects to an invited contact. The user sees the normal "verify the safety code" prompt instead of "this is not the key in the invite you imported". Combined with CRYPTO-02, the out-of-band invite does nothing to stop the attack.
- Recommendation:
  - When a session is spawned for a contact, record the contact's fingerprint as the *expected* fingerprint for that session. Do not auto-trust it: an invite is still not verification.
  - In `handle_tofu_verification`, treat a mismatch as a key-change warning, the same as the `Some(known)` branch.
  - Add a test: import an invite for key K1, have K2 answer, and assert that a mismatch warning is shown.
- Effort: S

### CRYPTO-05 — The connection password is sent to an unverified host
- Severity: Medium
- Category: security
- Status: VERIFIED
- Location:
  - `core/src/network/session.rs:716-742` (`client_password_gate`)
  - `core/src/network/session.rs:893-911` (runs before `verify_then_run` / TOFU)
  - `docs/protocol.md:95-107`
- Problem:
  - The client sends the plaintext connection password inside the tunnel as soon as the handshake ends, before the user has compared the SAS or fingerprint and before any expected-fingerprint check.
  - The tunnel is authenticated only to *some* key.
  - This is the same flaw `CLAUDE.md` describes as fixed for Party joins ("no credential moves before step two").
- Impact / failure scenario: anyone who can impersonate the host collects the password from every connecting client and then uses it against the real host. That includes a malicious relay, a spoofed address in an invite, or a LAN attacker. The protocol doc's statement "the password is never exposed" is false.
- Recommendation:
  - Move the gate after the client's TOFU decision, or have the client answer only after its user (or a stored or expected fingerprint) has accepted the host.
  - Alternatively, replace the plaintext with a challenge-response that is useless to a fake host: `HMAC(password, transcript_hash ‖ "client")` checked by the host. A fake host could still run an offline dictionary attack against it, so prefer reordering, or use a PAKE.
  - Store the password in `Zeroizing<String>` (`client/src/app/chat_manager/mod.rs:190`).
- Effort: M

### CRYPTO-06 — A second rekey can overwrite the retained old key while old-key frames are still in flight
- Severity: Medium
- Category: bug
- Status: SUSPECTED (depends on timing)
- Location:
  - `core/src/network/session.rs:973-1009` (`initiate_rekey` sets `prev_cipher = Some(current)` unconditionally)
  - `core/src/network/session.rs:1089-1093` (`rekey_due`)
  - `core/src/network/session.rs:1185-1205` (a frame that fails under both keys ends the session)
  - `core/src/network/session.rs:1240-1241` (received app frames also count toward the rekey threshold)
- Problem:
  - Only one previous key is kept, and the host may start a new rekey before the client has proven it switched (`prev_cipher` still `Some`).
  - `messages_since_rekey` counts frames in both directions, including old-key (K0) frames still arriving from the client. With crossing traffic (both sides streaming files), the host can reach 100 messages and send `Rekey2` within one round trip of `Rekey1`.
  - At that point `prev_cipher` becomes K1 and K0 is discarded. Any K0 frame still in flight then fails under both K2 and K1, and the session is torn down with "Decryption failed!".
- Impact / failure scenario: on high bandwidth-delay links with transfers crossing in both directions, sessions drop at random during a rekey and the transfer fails. Existing tests cover only a single rekey (`old_key_in_flight_frames_survive_a_rekey`, `messages_keep_flowing_across_a_rekey`).
- Recommendation:
  - Make `rekey_due()` also require `self.prev_cipher.is_none()`, so a new rekey waits until a frame under the current key has arrived. The peer's keep-alive guarantees that happens within 120 s.
  - Add an integration test that runs two bidirectional streams with `REKEY_MESSAGE_COUNT` lowered (test hook) and asserts that the session survives several rekeys.
- Effort: S

### CRYPTO-07 — Signature-scheme negotiation is not covered by the transcript or signatures
- Severity: Low
- Category: security
- Status: VERIFIED
- Location:
  - `core/src/network/session.rs:412-442` (plaintext `SupportedSignatureSchemes`)
  - `core/src/network/session.rs:466-473` (salt covers only versions and ephemerals)
  - `core/src/network/session.rs:281-284` (the signed digest omits the scheme)
- Problem: an on-path attacker can rewrite the offers so both sides fall back to RSA-PSS without being detected.
- Impact / failure scenario: no direct break today, because the Ed25519 path also depends on an RSA-PSS binding. It does mean the planned retirement of RSA can be silently downgraded, and it contradicts THREAT_MODEL's "downgrade or negotiation confusion" control.
- Recommendation: hash both scheme offers into the transcript salt, or into the signed proof digest, in the next protocol version. Treat `version` the same way: `check_peer_version` currently accepts any version ≥ 3 with no negotiated minimum, and `IdentityProof.version` is never checked.
- Effort: S (as part of the v4 bump)

### CRYPTO-08 — A malformed `nonce` in `identity.json` panics during unlock
- Severity: Low
- Category: bug
- Status: VERIFIED
- Location: `core/src/identity/mod.rs:362-363` (`cipher.decrypt(nonce.into(), ciphertext)` where `nonce: &[u8]` comes from the file)
- Problem: `From<&[u8]> for &GenericArray<u8, U12>` calls `GenericArray::from_slice`, which asserts the length. A nonce that is not 12 bytes panics instead of returning an error.
- Impact / failure scenario: a truncated or edited `identity.json` crashes unlock (TUI), or panics inside a Tauri command and may poison the bridge mutex. The intended "error" auth state is never reached.
- Recommendation: check `nonce.len() == 12` and `salt.len() >= 8` when loading or decrypting, and return an error otherwise. Use `Nonce::try_from` or a length check, never `.into()` on an unchecked slice.
- Effort: S

### CRYPTO-09 — The history file has no AAD, no anti-rollback, and random nonces under one fixed key
- Severity: Low
- Category: security
- Status: VERIFIED
- Location:
  - `client/src/app/persistence.rs:134-189`
  - `core/src/identity/mod.rs:715-722` (`history_key = SHA-256(PKCS#8 DER)`, no label)
- Problem:
  - Every save encrypts the whole history with the same key and a random 96-bit nonce, with no AAD.
  - Nothing binds the file to a version or counter, so an older `history.json.enc` (for example from a backup or synced folder) decrypts and loads normally. That restores deleted chats, a revoked `peer_fingerprint` (contact deletion is meant to revoke trust), a lifted block, and old `Config` values.
  - The random-nonce birthday bound (~2^48 saves) is not a practical concern.
- Impact / failure scenario: anyone who can write to the data directory can silently undo trust revocations and blocks. The threat model lists file tampering as a local attacker capability.
- Recommendation:
  - Store a monotonically increasing generation counter in the file and as AAD.
  - Keep the last seen generation inside `identity.json`, which is written atomically and is under the password. Warn on rollback.
  - Add AAD such as `"p2pem-history-v1"`.
  - Consider XChaCha20-Poly1305 for nonce margin. The key derivation is frozen, so leave it as it is.
- Effort: M

### CRYPTO-10 — Gaps in key-material hygiene
- Severity: Low
- Category: best-practice
- Status: VERIFIED (except where marked)
- Location:
  - The workspace `Cargo.toml` (line 39: `aes-gcm = "0.11.0"`) does not enable the `zeroize` feature of `aes-gcm-0.11.1` (`Cargo.toml:60-63`). The AES round keys and GHASH key inside `Aes256Gcm` are not wiped on drop, and `AesCipher: Clone` duplicates them (`prev_cipher`).
  - `core/src/core/crypto.rs:607-609`: `get_current_key` returns a non-zeroized copy (used at `session.rs:996, 1227`).
  - `core/src/core/crypto.rs:422-426, 446-450`: intermediate key arrays are not zeroized.
  - `core/src/identity/mod.rs:715-722`: `history_key` returns a plain `[u8; 32]`.
  - `client/src/app/chat_manager/mod.rs:175`: `history_key: Option<[u8; 32]>` is held for the life of the process, and `history_snapshot` copies it out.
  - `client/src/app/chat_manager/mod.rs:190`: `connection_password: Option<String>`.
  - `core/src/core/crypto.rs:276-282`: `generate_ed25519_keypair` leaves its seed on the stack.
- Problem: copies of secrets outlive their use. The THREAT_MODEL claims "zeroization in sensitive paths".
- Impact / failure scenario: a memory disclosure (crash dump, swap) recovers session, history or password material.
- Recommendation:
  - Enable `aes-gcm/zeroize` (and `chacha20poly1305/zeroize`, if present for that version).
  - Return `Zeroizing<[u8; 32]>` from `get_current_key`, `rekey_session_key`, `derive_session_key` and `history_key`.
  - Store `history_key` and `connection_password` in `Zeroizing`.
  - Consider dropping `Clone` on `AesCipher`.
- Effort: S

### CRYPTO-11 — Packets before authentication may be up to 8 MiB
- Severity: Low
- Category: security
- Status: VERIFIED
- Location: `core/src/network/session.rs:66-76, 380-408, 425`, using `core/src/core/framing.rs:37-74`
- Problem: the version packet (4 bytes), the ephemeral key (32 bytes) and the scheme list (≤10 bytes) are all read with `recv_packet`, which accepts any length up to `MAX_PACKET_SIZE` (8 MiB) before the size is checked.
- Impact / failure scenario: every unauthenticated connection can make the receiver allocate and read 8 MiB per handshake step, each within its 15 s timeout. This is small for a single P2P host, which accepts one peer at a time. It is more relevant to the Party server and relay, which reuse `host_handshake`.
- Recommendation: add `recv_packet_bounded(stream, max)` and use small limits for handshake frames (for example 64 bytes for version, ephemeral and schemes; 16 KiB for the identity proof).
- Effort: S

### CRYPTO-12 — `has_hidden_characters` and `is_forbidden_char` miss several invisible and direction-changing characters
- Severity: Low
- Category: security
- Status: VERIFIED
- Location: `core/src/util.rs:234-259`
- Problem: the following are not covered:
  - U+061C ARABIC LETTER MARK (a bidi mark);
  - U+2028/U+2029 (line and paragraph separators; these are not `is_control`);
  - the Hangul fillers U+115F, U+1160, U+3164 and U+FFA0 (render blank);
  - U+034F COMBINING GRAPHEME JOINER;
  - variation selectors U+FE00–FE0F;
  - tag characters U+E0000–E007F.
- Impact / failure scenario: a community username or channel name made of Hangul fillers looks blank, or impersonates "admin" with an invisible suffix. A filename containing U+2028 breaks the layout of a file card.
- Recommendation: extend both predicates, ideally using Unicode `Default_Ignorable_Code_Point` plus `Bidi_Control`, and add these code points to the property tests.
- Effort: S

### CRYPTO-13 — The Windows reserved-name list is incomplete
- Severity: Low
- Category: security
- Status: SUSPECTED (depends on Windows behaviour)
- Location: `core/src/util.rs:220-224, 299-311`
- Problem:
  - These are missing: `COM0`, `LPT0`, the superscript forms `COM¹ COM² COM³ LPT¹ LPT² LPT³`, and `CONIN$`, `CONOUT$`, which Microsoft documents as reserved.
  - The stem is taken with `split('.')`, so `"CON .txt"` yields the stem `"con "`. Win32 device-name detection is believed to ignore trailing spaces before the dot.
- Impact / failure scenario: a received file named, for example, `CONIN$.txt` or `COM¹.txt` fails to rename or is treated as a device. The result is a confusing I/O failure, not code execution.
- Recommendation: add the missing names, compare the stem after trimming trailing spaces and dots, and extend the property tests.
- Effort: S

### CRYPTO-14 — Peer identity keys have no minimum strength
- Severity: Low
- Category: security
- Status: VERIFIED
- Location:
  - `core/src/core/crypto.rs:49-51, 134-148`
  - `core/src/network/session.rs:328`
  - `core/src/core/crypto.rs:290-299` (`verify`, not `verify_strict`)
- Problem:
  - Any RSA public key the `rsa` crate accepts is taken as an identity, including 512- or 1024-bit keys. The crate caps the maximum size, not the minimum.
  - Ed25519 verification uses the permissive `verify`, which accepts small-order or malleable signatures.
- Impact / failure scenario: a contact using a weak key can later be impersonated by anyone who factors it, and the user is never told the key is weak. The Ed25519 weakness is mitigated because the subkey is bound by RSA.
- Recommendation: reject RSA identity keys under 2048 bits, both in the handshake and in invites. Use `verify_strict` for Ed25519.
- Effort: S

### CRYPTO-15 — The Ed25519 proof path adds work and no security while the identity is still RSA
- Severity: Info
- Category: performance
- Status: VERIFIED
- Location:
  - `core/src/network/session.rs:291-301` (an RSA-PSS binding is signed again on every handshake)
  - `core/src/network/session.rs:344-356` (the verifier does an RSA verify plus an Ed25519 verify)
- Problem: when both sides negotiate Ed25519 (the default), each handshake costs one RSA sign, one Ed25519 sign, one RSA verify and one Ed25519 verify per side. That is strictly more work than RSA-PSS alone, and security still rests on RSA-PSS.
- Impact / failure scenario: handshakes are slightly slower. The "Ed25519 identity proofs" control in the threat model gives no practical gain.
- Recommendation: compute the binding once per identity and cache it (it is deterministic in what it covers). Document that the real benefit only arrives with independent Ed25519 identity keys.
- Effort: S

### CRYPTO-16 — Security docs disagree with the code
- Severity: Low
- Category: docs
- Status: VERIFIED
- Location and problem:
  - `docs/protocol.md:82-86` says the SAS transcript "includes both ephemeral keys and both identity keys". Identity keys are **not** included (`session.rs:466-473`). `crypto.rs:497-502` repeats this and also claims "one guess per connection attempt" (see CRYPTO-02).
  - `docs/protocol.md:369-379` describes `ed25519_public` / `ed25519_binding` as extra proof fields that "default to absent". In the code they are packed in bincode inside `signature` (`protocol.rs:180-200`, `session.rs:296-300`).
  - `docs/protocol.md:105-106` says "the password is never exposed". It is sent to a host that has not been verified yet (CRYPTO-05).
  - `docs/protocol.md:358-360` says the scheme offers are exchanged "before the tunnel is established", which is true, but omits that they are unauthenticated (CRYPTO-07).
  - `docs/protocol.md:454-455` says Party uploads are inline only and chunked transfer is not implemented. `CLAUDE.md` documents chunked 100 MiB uploads.
  - `THREAT_MODEL.md:192` lists "no invite … expiry enforcement". `SECURITY.md:144-146` and `invite.rs:157-172` enforce 30-day expiry.
  - `THREAT_MODEL.md:36` lists "Ed25519 identity proofs" as a control, while `SECURITY.md:201` disclaims Ed25519 identity keys. Both need to state that the identity is RSA and Ed25519 is a bound subkey.
  - `SECURITY.md:182-184` says `bincode` is "used for the relay control protocol". It also decodes `IdentityProof`, `Ed25519Proof` and every Party frame.
  - `core/src/core/crypto.rs:393` says the HKDF info example is "p2p-messenger-v2". The real value is `b"p2p-messenger-v2-forward-secrecy"`.
  - The `NonceRole::Host` doc says "The connection initiator's listener side", which contradicts itself.
- Impact / failure scenario: reviewers and users reason from wrong guarantees. The SAS claims are the most damaging.
- Recommendation: correct each item. After fixing CRYPTO-01 to 03, document direction binding and the SAS commitment.
- Effort: S

### CRYPTO-17 — Security-critical paths with no tests
- Severity: Info
- Category: maintainability
- Status: VERIFIED (searched `core/tests`, `client/tests` and the in-file tests)
- Location: `core/tests/*`, `client/tests/key_rotation_tests.rs`, `core/src/network/session.rs` tests
- Problem: none of the following is tested:
  - PEM canonicalisation, or agreement between the fingerprint and the parsed key (CRYPTO-01);
  - reflected frames, during the handshake or mid-session (CRYPTO-03);
  - a peer reusing our own ephemeral key or fingerprint;
  - several rekeys with traffic in both directions (CRYPTO-06);
  - an invite-fingerprint mismatch on first connect (CRYPTO-04);
  - an `identity.json` with a wrongly sized nonce or salt (CRYPTO-08);
  - history rollback (CRYPTO-09).

  The `key_rotation_tests.rs` tests exercise the KDF, not the message loop.
- Impact / failure scenario: the bugs above went unnoticed.
- Recommendation: add the tests listed in each finding. Add a proptest that `fingerprint_pubkey(s)`, when it succeeds, equals the fingerprint of `pem_decode_public(s)`.
- Effort: M

### CRYPTO-18 — Minor protocol and encoding nits
- Severity: Info
- Category: maintainability
- Status: VERIFIED
- Location:
  - `core/src/core/protocol.rs:377-381`: `SupportedSignatureSchemes` encodes `schemes.len() as u8`, which silently truncates, while the decoder caps at 8. An encoder given more than 8 produces frames the decoder rejects.
  - `core/src/network/session.rs:1124, 1210` and `client/src/app/chat_manager/events.rs:173` log `FileMeta.filename`, peer addresses, fingerprints and the SAS at `debug` level. This is metadata in logs. Content is redacted, which is good.
  - `core/src/core/crypto.rs:337-354`: `ed25519_private_to_hex` / `ed25519_private_from_hex` are public helpers for exporting secret keys with no caller in production. Remove them to reduce misuse.
- Recommendation: assert or bound the scheme count in the encoder. Keep peer metadata out of logs below `trace`. Delete the unused secret-hex helpers.
- Effort: S

## Things done well
- X25519 refuses all-zero keys at parse time and non-contributory secrets at agreement. HKDF is salted with the transcript hash, and AADs carry labels (`identity-proof`, `transport`).
- AES-GCM nonces are `role-bit ‖ 31-bit random ‖ 64-bit counter`, with a counter that never wraps (it panics instead) and a fresh cipher on every rekey. No AEAD nonce reuse was found.
- Replay protection is strictly monotonic over one sequence space shared by all lanes and by `Rekey`, and it is enforced before frames reach the app.
- Decoding is strict: UTF-8 is refused rather than repaired, length prefixes are bounded before slicing, `decode_exact` rejects trailing bytes, `total_chunks` is capped, and `recv_packet` reads in chunks.
- Argon2id (64 MiB, t=3, p=4) uses stored parameters with upper bounds and is never retried with other configurations. A password floor is enforced in the core. Saving an unencrypted identity is refused. Writes are atomic and created with mode 0600. A damaged identity file is never replaced.
- Invites are signed, bind the timestamp, and enforce expiry and clock skew. One struct defines the payload for both minting and reading.
- The connection-password compare is constant-time and independent of length. `ProtocolMessage`'s `Debug` output redacts content.
- RSA encryption and decryption were removed, which takes Marvin (RUSTSEC-2023-0071) off the attack surface.
- The SAS derivation is frozen by a known-answer test, and the history-key derivation by another.

## Open questions
1. Have any shipped releases computed fingerprints with `fingerprint_pubkey` while using `pem-rfc7468` ≥ 0.7, which strips preambles? If so, CRYPTO-01 needs a GHSA and coordinated disclosure. Every existing pin and verified contact should be considered suspect if the user may have connected through an untrusted relay or network.
2. Does the Party server have its own fingerprint→member mapping anywhere else (for example the audit log or role checks)? All of them inherit CRYPTO-01. A Party-focused auditor should confirm.
3. Are relay operators meant to be fully untrusted? If so, CRYPTO-02 and CRYPTO-03 are directly reachable by them and should be prioritised with CRYPTO-01.
4. Is there a plan for protocol v4? CRYPTO-02, CRYPTO-03 (directional keys) and CRYPTO-07 all need a wire change and should ship together.
