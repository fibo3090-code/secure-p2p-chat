# Network layer (sessions, relay, hole punch, discovery, NAT, rate limiting)

## Scope covered (list every file read)

Read in full:
- `core/src/network/session.rs` (2211 lines: handshake, password gate, TOFU wait, `Transport`, `run_message_loop`, `write_while_reading`, plus the unit tests)
- `core/src/network/relay.rs` (server, client registration, punch coordination, tests)
- `core/src/network/punch.rs`
- `core/src/network/discovery.rs`
- `core/src/network/ratelimit.rs`
- `core/src/network/nat.rs` (UPnP / NAT-PMP)
- `core/src/network/mod.rs`
- `core/src/core/framing.rs`
- `client/src/app/chat_manager/connect.rs`
- `core/tests/session_e2e.rs` (parts that matter here: idle, rekey and simultaneous-traffic tests), `core/tests/relay_e2e.rs`, `core/tests/host_listener.rs`, `core/tests/adversarial_parsing.rs` (test list)

Read the parts these depend on:
- `core/src/core/crypto.rs` (`AesCipher`: `new_with_role`, `encrypt`, `decrypt`, `NonceRole::nonce_bit`)
- `core/src/core/protocol.rs` (`impl Debug for ProtocolMessage`)
- `client/src/app/chat_manager/mod.rs` (`check_rehost_needed`, `stop_hosting`, `teardown_session`, `register_session_task`, `FILE_LANE_CAPACITY`)
- `client/src/app/chat_manager/events.rs` (`NewConnection` / `Disconnected` handling, `accept_incoming_connection`, `is_hole_punched`)
- `client/src/tui/app/mod.rs` (`auto_rehost`, `REHOST_DEBOUNCE`)
- `desktop/src-tauri/src/lib.rs` (`REHOST_CHECK_INTERVAL`, `auto_rehost`, log filter)
- `desktop/src-tauri/src/commands/connect.rs` (`start_host`, `list_discovered_peers`, `set_locked`, relay commands)
- `client/src/main.rs` (`--relay-server` entry), `core/src/lib.rs` constants

No cargo builds or tests were run. Every finding comes from reading the code.

## Summary

The network layer is far more hardened than usual. Reads are cancel-safe, the two outbound lanes apply backpressure, writes keep reading so crossing transfers cannot deadlock, every handshake step has a timeout, relay hello and pending slots are bounded, the rate limiter is capped and keys IPv6 per /64, and mDNS input is capped. Most past bugs listed in CLAUDE.md are fixed and have tests. The main remaining risks:

1. **Frame reflection (NET-01, High).** Both directions use one AES key and one AAD, and `decrypt` never checks the nonce's role bit. An on-path attacker can echo a peer's own frames back to it, and they are accepted as the other side's. A bridging relay is in that position by design. Echoed frames include `Text`, `Ack` and `Rekey`, so the attacker can inject the user's own words as the peer's, forge delivery receipts, or desync the keys.
2. **One-shot host listener as a DoS primitive (NET-02, High).** The host drops its listener on the first `accept()`. It rebinds only after the handshake fails or completes, which can take up to about 75 s. One TCP connection that sends nothing blocks the host for 15 s, and repeating it keeps the host unreachable indefinitely. The per-IP rate limiter cannot help with a one-shot listener.
3. **Rekey can desync under bidirectional bulk traffic (NET-03, Medium, SUSPECTED).** Only one previous key is kept, and the rekey counter also counts frames received under the old key. Two host rekeys can therefore happen before the client switches keys, and the session dies with "Decryption failed".
4. **Nagle plus a split header/payload write (NET-04, Medium).** No session socket sets `TCP_NODELAY`, and `send_packet` writes the 4-byte header and the payload separately. Each small message can wait out the peer's delayed ACK: about 40 ms on Linux, up to 200 ms on Windows.
5. **Relay server availability.** A host that disconnects while waiting keeps its token slot for 5 minutes (NET-06). There is no per-IP cap on pending slots, so about 6 IPv4 addresses can fill the global table of 1024 (NET-07). The control channel is cleartext, so the token and LAN IPs are visible on path (NET-09).
6. **Smaller issues.** The punch hello tag can be reflected by any stranger (NET-08). The relay path silently ignores the connection password (NET-11). Contacts never fall back to the relay (NET-12). If the two peers confirm the SAS more than 5 minutes apart, the session dies (NET-10).

## Findings

### NET-01 — Transport frames can be reflected back to their sender and are accepted as the peer's
- Severity: High
- Category: security
- Status: VERIFIED (traced through code; no PoC run)
- Location: `core/src/network/session.rs:475-485` (one `aes_key` for both roles), `core/src/network/session.rs:483-484` (one `transport_aad` for both directions), `core/src/network/session.rs:1185-1194` (`Transport::receive` tries current then previous key, with no direction check), `core/src/core/crypto.rs:673-701` (`decrypt` ignores the nonce role bit), `core/src/core/crypto.rs:538-543,586` (the role bit exists only to separate nonces), `core/src/network/session.rs:943-963` (each side checks seqs only against the peer's counter)
- Problem: Host and client derive one AES-256-GCM key and use the same AAD (`labeled_aad(b"transport", salt)`) in both directions. The role is encoded only in the high bit of the nonce, which prevents nonce collisions but is not authenticated as a direction. `AesCipher::decrypt` takes the nonce from the payload and never checks that its role bit is the *peer's*. `validate_message_sequence` compares an incoming seq only with `last_valid_seq` from the peer's numbering, and our own `sent_seq` is an independent counter. A frame we sent with seq N therefore decrypts on our own side and passes the replay check whenever N exceeds the peer's latest seq. That is the normal case for the more talkative side.
- Impact (concrete): Any active on-path attacker can echo frames back to their sender. The bridging relay always is one ("relay never holds key material" is the stated trust model), as are hostile Wi-Fi and ISP paths.
  - Echo the user's `Text "yes"` back to them. It appears in the conversation as a message from the peer: an integrity break of E2E messaging.
  - Echo the user's own `Ack { acked_seq }` back. The user's sent message with that wire seq is marked delivered when it never was.
  - Echo the host's `Rekey` (encrypted under the old key, decryptable through `prev_cipher`). The host derives yet another key and the session tears down. This is a targeted, silent kill.
  - Side effect: the peer's genuine frames whose seq is at or below the echoed one are then dropped as replays (`session.rs:1214-1217`), so the attacker also gets silent message suppression.
- Recommendation: Make direction part of the cryptography. The best fix is to derive two keys with HKDF (`info` = `"...host->client"` / `"...client->host"`), or at least two AADs, and rekey both. A minimal, backward-compatible stopgap: in `Transport::receive`, reject any frame whose nonce byte 0 high bit equals our own role bit before trying to decrypt. Add a test that feeds a frame from a role-`Host` cipher into a role-`Host` `Transport::receive` and expects rejection. The Party framing (`send_framed`/`recv_framed`) uses `host_handshake` too and should be checked for the same property (see Open questions).
- Effort: S (role-bit check) / M (direction-separated keys with a version bump)

### NET-02 — The one-shot host listener lets one TCP connection make the host unreachable, repeatedly
- Severity: High
- Category: security (availability)
- Status: VERIFIED
- Location: `core/src/network/session.rs:122-137` (accept, `drop(listener)`, *then* rate-limit check), `core/src/network/session.rs:66-76,586-614` (each handshake step has a 15 s budget), `core/src/network/session.rs:840-849` (password gate, another 15 s), `client/src/app/chat_manager/events.rs:214-233,387-397` (placeholder consumed only on `NewConnection`), `client/src/app/chat_manager/events.rs:297-326` (rehost only after `Disconnected`), `desktop/src-tauri/src/lib.rs:579`, `client/src/tui/app/mod.rs:64` (1.5 s rehost cadence)
- Problem: `run_host_session` accepts exactly one peer and drops the listener at once. The placeholder chat stays until the session emits `NewConnection` (after the full handshake and password gate) or `Disconnected` (after failure). Auto-rehost waits for the placeholder to disappear. Between `accept()` and then, nothing is listening on the port. A connection that sends nothing holds that state for `HANDSHAKE_TIMEOUT_SECS` (15 s). One that sends each handshake packet just inside its own 15 s budget holds it for about 75 s, plus the 1.5 s rehost tick. The rate limiter is checked after the listener is gone, so it can only end the session early. It never protects the port, and an attacker under 5 connections per 10 s never triggers it.
- Impact: `while :; do nc host 12345 </dev/null; done` (one connection at a time, reconnecting as soon as the port reappears) keeps the host refusing nearly all legitimate peers, for as long as the attacker likes. UPnP (`connect.rs:212-214`) exposes the port to the whole internet, and the listener binds `0.0.0.0` (`session.rs:93-97`).
- Recommendation: Keep an accept loop alive for the life of the hosting state. Spawn a per-connection handshake task (with a concurrency cap, for example 4 in-flight handshakes, plus the existing per-IP limiter applied *before* spawning). Promote a connection to the conversation only after `NewConnection`. Also apply one overall handshake deadline (for example 20 s total) instead of 15 s per packet. The "one peer per session" model can stay at the session level; only the listener must not be one-shot.
- Effort: M

### NET-03 — Two host rekeys can happen before the client switches, after which old-key frames are undecryptable
- Severity: Medium
- Category: bug
- Status: SUSPECTED (logic traced; needs a high-bandwidth crossing-transfer test)
- Location: `core/src/network/session.rs:1089-1093` (`rekey_due`), `core/src/network/session.rs:1003` (`prev_cipher = Some(cipher.clone())` replaces the older retained key), `core/src/network/session.rs:1185-1194` (only current + one previous key are tried), `core/src/network/session.rs:1240-1241` (frames *received* under the old key still increment `messages_since_rekey`), `core/src/network/session.rs:1198-1204` (any failure ends the session)
- Problem: After the host sends `Rekey1` (key0→key1), the client keeps sending under key0 until it *processes* `Rekey1`. That frame sits behind every host frame already queued in the socket buffers and in the client's `inbox`, and the client processes it only between its own writes (`write_while_reading`). Meanwhile the host counts each key0 frame it receives (decrypted through `prev_cipher`) toward `messages_since_rekey`. With crossing file transfers and deep TCP buffers, the count can reach 100 before the client switches: Linux autotuning reaches multi-MB windows, and 100 frames is only 6.4 MB of 64 KiB chunks. The host then sends `Rekey2`, `prev_cipher` becomes key1, and the next in-flight key0 frame fails under both keys. The result is "Decryption failed!" and `Received::Stop`.
- Impact: Large simultaneous two-way transfers on fast links can tear down the session nondeterministically. `simultaneous_traffic_in_both_directions_keeps_framing_intact` (60 frames each way through a 64 KiB duplex) cannot reach this state.
- Recommendation: Do not initiate a new rekey while `prev_cipher.is_some()` (the peer has not yet proven it switched). Optionally, count only frames *sent* under the current key toward the threshold. Add a test with two crossing 20 MB transfers over a duplex sized to several MB.
- Effort: S

### NET-04 — No `TCP_NODELAY` and a two-write frame cause delayed-ACK stalls on every small message
- Severity: Medium
- Category: performance
- Status: VERIFIED (code); latency figure from standard TCP behaviour
- Location: `core/src/core/framing.rs:21-26` (header `write_all`, then payload `write_all`, unbuffered); no `set_nodelay` on direct sockets (`session.rs:93-97,169`), bridged relay sockets (`relay.rs:319`, `punch.rs:314-330`) or relay server sockets (`relay.rs:170`). Only punched sockets set it (`punch.rs:357`).
- Problem: With Nagle enabled, the 4-byte header goes out immediately. The payload write then waits for that segment's ACK, and the receiver holds the ACK on its delayed-ACK timer (about 40 ms on Linux, up to 200 ms on Windows). This is the classic write-write-read pattern. Typing indicators, texts, ACKs and pings each pay up to one delayed-ACK interval. Two `write` syscalls per frame also double the syscall count.
- Recommendation: Call `set_nodelay(true)` on every session `TcpStream` (accept, `connect_first_reachable`, `connect_reusable`, relay server accepted sockets). Write the frame in one call: build `header||payload` into one buffer, or use `write_vectored` / a `BufWriter` flushed per frame.
- Effort: S

### NET-05 — Unauthenticated peers can create unlimited TOFU prompts, 30-minute sessions and junk conversations
- Severity: Medium
- Category: security (DoS / UX abuse)
- Status: VERIFIED
- Location: `core/src/network/session.rs:745,757-781` (30-minute confirmation wait per session), `client/src/app/chat_manager/events.rs:214-233` and `341-398` (each `NewConnection` creates a `Chat` "Peer xxxx" and queues a prompt), `core/src/network/session.rs:32-33` (5 connections per 10 s per IP)
- Problem: Any party can complete the v3 handshake with a fresh throwaway RSA key (there is no password unless the user set one). Each such connection consumes the placeholder, triggers a rehost 1.5 s later, creates a persisted conversation, queues a fingerprint prompt, and parks a task plus socket for up to 30 minutes. The rate limit allows about 30 per minute per IPv4 address, or per /64.
- Impact: Hundreds of queued prompts and "Peer …" conversations from one machine in a few minutes. Conversations left by a timed-out prompt are not deleted (only `reject_fingerprint` deletes them), so they persist in history.
- Recommendation: Cap concurrent unverified sessions (for example 3), dropping new ones beyond the cap. Do not create or persist the `Chat` until the user accepts. Delete never-accepted conversations on `Disconnected`. Consider a shorter confirmation timeout for first contacts.
- Effort: M

### NET-06 — A relay host that disconnects while waiting keeps its token slot for the full 5 minutes
- Severity: Medium
- Category: bug (availability)
- Status: VERIFIED
- Location: `core/src/network/relay.rs:554-588` (`host_flow` awaits only `rendezvous_rx` and never watches the host socket)
- Problem: During the wait the server does not read the host's control stream, so a host that closes (app restart, network change, laptop sleep) goes unnoticed. The slot holds the token until `RELAY_WAIT_TIMEOUT`. A reconnecting host with the same token (contacts store `relay_token` and `start_host_via_relay(token: Some(..))` reuses it) gets "Relay token already in use" for up to 5 minutes. A joiner that arrives is paired with the dead socket and fails.
- Recommendation: In `host_flow`, `select!` the rendezvous with a read on `stream`. EOF or an error releases the slot (`release_dead_slot`). Alternatively let a re-registration of the same token replace a slot whose socket is closed.
- Effort: S

### NET-07 — No per-address cap on pending rendezvous slots; a few IPs can fill the global table
- Severity: Medium
- Category: security (availability of the relay server)
- Status: VERIFIED
- Location: `core/src/network/relay.rs:46` (`MAX_PENDING_RENDEZVOUS = 1024`), `relay.rs:50-51` (20 connections per 30 s per IP), `relay.rs:35` (slots live 300 s), `relay.rs:516-524`
- Problem: Each allowed connection can register a fresh random token and hold it for 300 s. One IPv4 address can therefore hold about 20 × (300/30) = 200 slots, and 6 addresses fill the table. After that every legitimate host gets "Relay is at capacity" for as long as the attacker keeps going.
- Recommendation: Track pending slots per `limiter_key(ip)` and cap them (for example 4 per address). Consider a shorter default wait with client-side re-registration.
- Effort: S

### NET-08 — The punch hello tag is sent before the peer proves anything, so any stranger can reflect it and claim the punched socket
- Severity: Medium
- Category: security
- Status: VERIFIED
- Location: `core/src/network/punch.rs:77-83` (static tag = `SHA256(token)[..16]`), `punch.rs:356-382` (`validate` writes our hello first, then checks the peer's tag), `punch.rs:151-177` (listener on `0.0.0.0:<control port>` accepts anyone during the window), `punch.rs:216-244` (host picks the first validated socket)
- Problem: `validate` sends `MAGIC || role || tag` before reading anything. A third party that connects to the host's punch port during the 10 s window receives the tag, then answers with the opposite role byte and the same tag. The socket validates, the host sends `SELECT`, the stranger sends `ACK`, and the host returns it as `RelayStream::Direct`. That port is the relay-observed public endpoint: reachable through endpoint-independent-filtering NATs, and visible to the relay or any on-path observer of the cleartext control channel (NET-09). The real joiner then times out, so its punch fails. The host has already dropped its control connection, so the server's bridge fallback also fails.
- Impact: Punch hijack or denial of the rendezvous. Identity is still protected by the v3 handshake and TOFU/SAS. A first-contact user who skips the SAS comparison would accept the attacker.
- Recommendation: Turn the hello into challenge-response. Each side sends a random 16-byte nonce, then `HMAC-SHA256(token, role || own_nonce || peer_nonce)`, and verifies the peer's MAC before anything is selected. The token is 128-bit and otherwise secret, so this makes the tag real rendezvous authentication.
- Effort: S

### NET-09 — The relay control channel is cleartext: token, LAN IPs and pairing are visible and spoofable on path
- Severity: Medium
- Category: security / privacy
- Status: VERIFIED
- Location: `core/src/network/relay.rs:735-744` (plain bincode over `TcpStream`), `relay.rs:391-421` (`local_addrs` from `primary_local_ipv4`), `relay.rs:328`
- Problem: `HostV2/JoinV2 { token, local_addrs }` and `PunchStart { peer_public, peer_locals }` travel unencrypted. A passive observer learns the 128-bit token, which is the only thing that pairs peers and keys the punch tag, plus each user's private LAN address. An active one can squat the token or rewrite `PunchStart` to point a client's punch at arbitrary addresses.
- Impact: Token theft enables squatting, punch hijack (NET-08) and pairing denial. TOFU/SAS still prevents silent impersonation. LAN IP disclosure is a privacy leak.
- Recommendation: Run the control protocol over TLS (or a Noise NK channel to a pinned relay key). At minimum, document that relay tokens are bearer secrets visible on path, and only send `local_addrs` after the server has paired a peer.
- Effort: M

### NET-10 — Confirming the SAS more than 5 minutes apart kills the session
- Severity: Medium
- Category: bug / ux
- Status: VERIFIED
- Location: `core/src/network/session.rs:745` (30-minute confirmation timeout), `session.rs:800-815` (the message loop and its keep-alives start only after *local* confirmation), `session.rs:1018,1356` (300 s idle timer starts at message-loop entry)
- Problem: Side A accepts first and enters the message loop with a 300 s idle timer. Side B is still in `await_user_confirmation`, which neither reads nor sends pings. If B takes longer than 5 minutes (people comparing a SAS by phone or chat), A times out with "Receive idle timeout (300s)". When B then accepts, its session immediately sees EOF. Meanwhile A was shown `Ready` and could "send" messages that B never processes.
- Recommendation: Run a confirmation-phase keep-alive, or exchange an encrypted "confirmed" frame and start the idle timer only after both sides have confirmed. Alternatively, suspend A's idle timer until the first frame from B arrives. Align the timeouts.
- Effort: S/M

### NET-11 — The connection password is silently ignored for relay-hosted sessions
- Severity: Low
- Category: security (expectation mismatch)
- Status: VERIFIED
- Location: `core/src/network/relay.rs:216,245` (`connection_password` hard-coded `None`), `client/src/app/chat_manager/connect.rs:267-297,363-384` (relay paths never read `self.connection_password`)
- Problem: A user who set a connection password and then hosts via relay has no password gate. Anyone with the relay token reaches the TOFU prompt. Nothing tells the user.
- Recommendation: Pass `self.connection_password.clone()` through both relay entry points. The gate runs inside the tunnel, so it is transport-agnostic. If that is not wanted, refuse or warn in the UI.
- Effort: S

### NET-12 — `connect_to_contact` never falls back to the relay when direct addresses fail
- Severity: Low
- Category: bug / ux / docs
- Status: VERIFIED
- Location: `client/src/app/chat_manager/connect.rs:425-430` (doc: "direct addresses first, then the relay"), `connect.rs:497-513` (relay used only when `candidates.is_empty()`)
- Problem: A contact with both addresses and a relay token is dialled only directly. When every candidate fails (the usual case behind NAT once the host's LAN or UPnP address is stale), the session fails with "Connection failed" and the relay is never tried. The doc comment and CLAUDE.md both claim a relay fallback.
- Recommendation: Run `connect_first_reachable` first. On failure inside the spawned task, call `run_client_session_via_relay` if `relay_server`/`relay_token` exist. This must happen within the same task and session id, so the manager's state is unchanged.
- Effort: S

### NET-13 — A UPnP / NAT-PMP mapping that fails the routable check is created and then leaked
- Severity: Low
- Category: bug
- Status: VERIFIED
- Location: `core/src/network/nat.rs:105-121` (`add_port` before `check_routable`), `nat.rs:145-153` (same for NAT-PMP), `nat.rs:76-91` (after a UPnP "failure" NAT-PMP maps too), `client/src/app/chat_manager/connect.rs:237-242` (`Err` gives no protocol, so no unmap and no renewal)
- Problem: Behind double NAT or CGNAT, the inner router gets a port forward to the host for `LEASE_SECS` (1 h) that nothing cleans up. The code reports failure, and both protocols may leave mappings.
- Recommendation: Query the external IP first and only then `add_port`, or `unmap_port` on the error path. Optionally retry with lease 0 when the router answers `OnlyPermanentLeasesSupported`.
- Effort: S

### NET-14 — Client-side relay I/O has no timeouts
- Severity: Low
- Category: bug / resilience
- Status: VERIFIED
- Location: `core/src/network/punch.rs:314-330` (`connect_reusable`: unbounded `socket.connect`, waits for the OS default of about 2 min per address), `core/src/network/relay.rs:333-343` (`recv_relay_message` loop with no deadline)
- Problem: An unreachable, blackholed or misbehaving relay leaves a joiner "Connecting via relay …" until the OS connect timeout, or forever after `Waiting` if the server stalls. The direct path bounds each attempt with `CONNECT_ATTEMPT_TIMEOUT_SECS`; the relay path does not.
- Recommendation: Wrap `connect_reusable` in `CONNECT_ATTEMPT_TIMEOUT_SECS`. Bound the joiner's wait for `PunchStart`/`Paired` (for example `PUNCH_REPORT_TIMEOUT` + margin), and the host's at `RELAY_WAIT_TIMEOUT` + margin.
- Effort: S

### NET-15 — Punch outcome is decided per client, not by the server, so split verdicts break both peers
- Severity: Low
- Category: bug
- Status: SUSPECTED
- Location: `core/src/network/relay.rs:366-385` (a client returns `Direct` and drops its control stream on local success), `relay.rs:592-604` (the server bridges unless *both* report success), `core/src/network/punch.rs:237-239` (host ACK timeout of 3 s), `punch.rs:257-290` (unbiased `select!` between a waiter returning and the deadline)
- Problem: The two sides can disagree. Example: over an RTT above 3 s, the joiner ACKs and succeeds while the host's ACK read times out. Or the joiner's deadline and a waiter's completion race in an unbiased `select!`. The successful side has already left with a direct socket and closed its control connection. The server then "bridges" the other side to a closed socket. Both handshakes fail where a bridged session would have worked.
- Recommendation: Make the server's verdict authoritative. Clients report and then wait for `Paired` or a new `PunchConfirmed` before using the direct socket. Use `biased;` with the completion branch first in `select_as_joiner`.
- Effort: M

### NET-16 — Peer-supplied punch candidates include loopback and link-local; the host dials them
- Severity: Low
- Category: security (SSRF-lite / local probing)
- Status: VERIFIED
- Location: `core/src/network/punch.rs:87-105` (filters only port 0, unspecified and multicast), `core/src/network/relay.rs:495-498` (server caps only count and length)
- Problem: A joiner (or a relay or on-path attacker, see NET-09) can list `127.0.0.1:<port>` or other LAN addresses. For 10 s the host repeatedly opens TCP connections to them from the punch port and writes a 25-byte hello: a small local port probe and protocol-confusion vector against loopback services.
- Recommendation: Drop loopback, link-local and broadcast. Accept only RFC1918 / ULA addresses as "LAN" candidates, and at most one per address family.
- Effort: S

### NET-17 — Punch listener spawns detached, unbounded validation tasks
- Severity: Low
- Category: security (resource) / maintainability
- Status: VERIFIED
- Location: `core/src/network/punch.rs:155-169` (`tokio::spawn` per accepted socket, outside the `JoinSet`)
- Problem: Anyone who can reach the punch port during the window can open many connections. Each spawns a task holding a socket for `VALIDATE_TIMEOUT` (3 s), and `workers.abort_all()` does not cancel them. The backlog is 16, but accepts are unbounded.
- Recommendation: Spawn the validations into the same `JoinSet` (or a semaphore-bounded set) so they are aborted with the attempt.
- Effort: S

### NET-18 — mDNS advertises the user's display name and machine hostname; peer names are unsanitised
- Severity: Low
- Category: security (privacy) / ux
- Status: VERIFIED
- Location: `desktop/src-tauri/src/commands/connect.rs:16-43` (`register(&id.name, port)`), `core/src/network/discovery.rs:150-176` (instance = display name, host = `hostname::get()`), `discovery.rs:204` (remote `hostname` shown verbatim), `desktop/src-tauri/src/commands/connect.rs:40` (advertises the *requested* `port`, not the bound one)
- Problem: The fingerprint was removed from the TXT record for privacy, but the instance name (the identity's display name) and the hostname (often "Alices-MacBook-Pro") link the person to the machine just as well. Remote names are attacker-controlled and are passed to the UI without stripping control or bidi characters. Only after `stop_hosting` through `set_locked` is the service unregistered. Hosting stopped by `MAX_CONSECUTIVE_REHOST_FAILURES` keeps advertising a dead port.
- Recommendation: Advertise a random per-install instance name and a generic host label. Sanitise remote names with the same rules as filenames or usernames. Unregister on every `stop_hosting` path. Register `mgr.hosting_port`.
- Effort: S

### NET-19 — Desktop defaults to `messenger_core=debug`, logging every frame
- Severity: Low
- Category: performance / privacy
- Status: VERIFIED
- Location: `desktop/src-tauri/src/lib.rs:332-334`, `core/src/network/session.rs:1124,1210` (`debug!` per sent and received frame), `core/src/core/protocol.rs:120-130` (`FileMeta` filename is logged; text is redacted)
- Problem: The shipped app formats and writes a log line for every 64 KiB chunk, including filenames and peer addresses, to stdout by default.
- Recommendation: Default to `info` in release builds (`cfg!(debug_assertions)`), and move per-frame logs to `trace`.
- Effort: S

### NET-20 — Relay control frames accept trailing bytes, unlike every other bincode frame
- Severity: Info
- Category: best-practice
- Status: VERIFIED
- Location: `core/src/network/relay.rs:741-744` (`bincode::deserialize`, not `util::decode_exact`)
- Problem: This is the inconsistency CLAUDE.md calls a bug class ("Bincode frames reject trailing bytes"). It has no direct exploit here because these frames are pre-auth and untrusted anyway.
- Recommendation: Use `crate::util::decode_exact`.
- Effort: S

### NET-21 — Per-frame copies and the bridge's small buffers
- Severity: Info
- Category: performance
- Status: VERIFIED
- Location: `core/src/core/framing.rs:60-70` (zero-filled `resize` per 64 KiB chunk), `core/src/network/session.rs:1126-1129` (`to_plain_bytes` → `encrypt` → a second `Vec`), `core/src/network/relay.rs:609` (`copy_bidirectional` uses tokio's default 8 KiB buffers)
- Problem: Every file chunk is copied about 4 times and allocated about 3 times per direction. The bridged relay moves each 64 KiB frame in 8+ syscalls per side.
- Recommendation: Use `copy_bidirectional_with_sizes(…, 64 KiB, 64 KiB)`. Encrypt in place (`encrypt_in_place` / `BytesMut`). Read the body with `read_buf` into reserved capacity instead of `resize`.
- Effort: S/M

### NET-22 — Test gaps for these paths
- Severity: Info
- Category: maintainability
- Status: VERIFIED
- Location: `core/tests/session_e2e.rs:537-572`, `core/tests/host_listener.rs`, `core/src/network/punch.rs:384-492`, `core/src/network/relay.rs:746-1128`
- Missing tests:
  - A reflection test (NET-01).
  - A crossing bulk-transfer test large enough for two host rekeys (NET-03).
  - A test that the host stays connectable while a silent connection sits in the handshake (NET-02).
  - A relay test where the host disconnects while waiting and re-registers the same token (NET-06).
  - A per-IP pending-slot cap test (NET-07).
  - A punch test with a third socket replaying the tag (NET-08).
  - A test with split punch verdicts (NET-15).
  - A test that the password applies over relay (NET-11).
  - A test that a contact with stale direct addresses reaches the relay (NET-12).
  - A test that peers confirming more than `RECV_IDLE_TIMEOUT` apart still connect (NET-10; the env hooks already make this cheap).

## Things done well
- The message loop holds the read future across `select!` iterations (`session.rs:1332-1350`). `write_while_reading` keeps reading during writes, which removes the crossing-write deadlock, with a cap on held frames (`MAX_FRAMES_HELD_DURING_WRITE`).
- Bulk file data uses a bounded lane and control traffic an unbounded one, with one shared monotonic wire seq. The idle timer measures silence from the last *received* frame, and keep-alives double as rekey triggers.
- Each `recv_packet` grows its buffer in chunks after validating the length, and every handshake step has a timeout. The password compare hashes first, so it is length-independent and constant-time.
- The relay has a hello timeout, a capped pending table with sweep-before-full, slot release keyed to the owner (`release_dead_slot`), TCP keepalive on bridged sockets, a log-and-continue accept loop, and a legacy-server fallback with tests.
- The rate limiter ages and caps entries, evicts by `last_seen`, keys IPv6 per /64, and canonicalises IPv4-mapped and NAT64 addresses, with good tests.
- mDNS caps peers and addresses, retains before the cap check, and removes by exact fullname. The TXT record carries no identity.
- Session tasks are tracked and aborted, binding happens before any state exists, and placeholders are cleaned up on every failure path (`spawn_session` emits `Error` + `Disconnected`).
- The UPnP renewal task cancels and unmaps properly, even mid-remap.

## Open questions
- Do the Party server's `send_framed`/`recv_framed` share NET-01? They build on `host_handshake`'s single key and transport AAD. If the per-direction `FrameSeq` counters are independent, a reflected frame may pass there too. That module belongs to the server auditor.
- Did older v3 peers always set the nonce role bit? `AesCipher::new` defaults to Host. If any shipped version built both ends with the default role, the cheap fix for NET-01 (reject our own role bit) would break compatibility with it, and direction-separated keys behind a version bump would be needed instead.
- `accept_incoming_connection` (`events.rs:356-398`) falls back to the *client-offered* chat id without checking whether that chat is live. A returning peer offering the id of a conversation that currently has a live session would remap `chat_id_mapping` away from that session. TOFU should catch a fingerprint mismatch, but the session-hijack angle is worth a look by the chat-manager auditor.
- Is relay hosting meant to expire silently after 5 minutes (`RELAY_WAIT_TIMEOUT`, and `check_rehost_needed` excludes relay)? Contacts' stored relay tokens only work while the host is actively waiting, which users may not expect.
