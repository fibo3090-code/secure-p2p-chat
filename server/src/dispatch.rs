//! Party request dispatcher (Phase 1, slice 1b).
//!
//! The pure protocol-handling layer: given the per-connection session state and an
//! incoming [`PartyRequest`], it applies the request to the shared [`PartyState`]
//! and produces the [`PartyResponse`]s to send back. It enforces the "must join
//! before acting" rule and is completely network-free, so it is unit-tested
//! directly. The runtime (next slice) owns the transport and fan-out: it decrypts
//! a request off the v3 tunnel, calls [`handle_request`], encrypts the replies, and
//! separately broadcasts newly posted messages to other online members.

use messenger_core::party::{
    ChannelKind, Envelope, PartyRequest, PartyResponse, UploadTarget, PARTY_CHUNK_BYTES,
};
use uuid::Uuid;

use crate::state::PartyState;

/// The outcome of handling one request:
/// - `replies` go back to the requesting connection;
/// - `broadcast` responses are pushed to all *other* connected members (a posted
///   channel message, or a refreshed channel list);
/// - `directed` responses are delivered to a specific member's connections (DMs).
///
/// The runtime fans `broadcast`/`directed` out via the connection hub.
#[derive(Debug, Default)]
pub struct Dispatch {
    pub replies: Vec<PartyResponse>,
    pub broadcast: Vec<PartyResponse>,
    pub directed: Vec<(Uuid, PartyResponse)>,
}

impl Dispatch {
    fn reply(resp: PartyResponse) -> Self {
        Self {
            replies: vec![resp],
            broadcast: Vec::new(),
            directed: Vec::new(),
        }
    }
}

/// How many rejected `Join` attempts a single connection may make before it is
/// refused outright.
///
/// Password guessing is the attack this bounds. Without a limit one TCP
/// connection can carry an unlimited stream of `Join` frames, so an attacker
/// pays for one RSA handshake and then guesses as fast as the link allows. With
/// it they must redo the full v3 handshake every three guesses — and the accept
/// loop's per-IP limiter bounds how often they may do *that*.
pub const MAX_JOIN_ATTEMPTS: u32 = 3;

/// Per-connection session state. A connection must `Join` before any other
/// request is honoured; once joined it carries the member's id. `peer_fingerprint`
/// is the identity verified during the v3 handshake, bound to the member on join.
#[derive(Debug, Default)]
pub struct ConnState {
    member: Option<Uuid>,
    peer_fingerprint: Option<String>,
    /// Rejected join attempts on this connection (see [`MAX_JOIN_ATTEMPTS`]).
    join_failures: u32,
    /// Chunked uploads this connection started and has not finished. Tracked
    /// here so they can be discarded when the connection ends, and so the
    /// per-connection concurrency cap has something to count.
    uploads: Vec<Uuid>,
}

impl ConnState {
    /// Create connection state carrying the handshake-verified peer fingerprint.
    pub fn with_fingerprint(fingerprint: String) -> Self {
        Self {
            member: None,
            peer_fingerprint: Some(fingerprint),
            join_failures: 0,
            uploads: Vec::new(),
        }
    }

    /// Uploads this connection left in flight, for cleanup on disconnect.
    pub fn open_uploads(&self) -> &[Uuid] {
        &self.uploads
    }

    /// The joined member's id, if this connection has completed `Join`.
    pub fn member(&self) -> Option<Uuid> {
        self.member
    }
}

/// Where a stored DM envelope has to be delivered: to the recipient, and to the
/// sender's *other* connections.
///
/// The runtime excludes the originating connection when it fans `directed` out,
/// so the sender's own client is not sent a duplicate of the message it already
/// appended optimistically — but its other devices, which know nothing about the
/// send, are. A self-DM collapses to a single entry so it is not delivered twice.
fn dm_delivery(from: Uuid, to: Uuid, env: Envelope) -> Vec<(Uuid, PartyResponse)> {
    if from == to {
        return vec![(to, PartyResponse::Message(env))];
    }
    vec![
        (to, PartyResponse::Message(env.clone())),
        (from, PartyResponse::Message(env)),
    ]
}

/// How a stored channel message reaches everyone else.
///
/// A channel every joined member may read can simply be broadcast. A `Private`
/// one cannot: `broadcast_except` pushes to every registered connection, so a
/// member who is not in the channel — and who is correctly not shown it in
/// `ListChannels` — would still receive every live message posted to it. Those
/// are delivered by member instead, to exactly the people allowed to read them.
/// The runtime excludes the originating connection either way, so the poster is
/// not sent a copy of what it already appended.
fn channel_fanout(state: &PartyState, env: Envelope) -> Dispatch {
    let ack = PartyResponse::MessagePosted {
        channel: env.channel,
        seq: env.seq,
    };
    if state.channel_is_open_to_all(env.channel) {
        return Dispatch {
            replies: vec![ack],
            broadcast: vec![PartyResponse::Message(env)],
            directed: Vec::new(),
        };
    }
    let readers = state.members_who_can_read(env.channel);
    Dispatch {
        replies: vec![ack],
        broadcast: Vec::new(),
        directed: readers
            .into_iter()
            .map(|m| (m, PartyResponse::Message(env.clone())))
            .collect(),
    }
}

/// The thread a failed upload belongs to, so `ActionFailed` can be correlated
/// with the message the client is already showing.
fn upload_thread(member: Uuid, target: UploadTarget) -> Uuid {
    match target {
        UploadTarget::Channel(id) => id,
        UploadTarget::Dm(to) => messenger_core::party::dm_thread_id(member, to),
    }
}

/// Reply with the requester's own filtered channel list, and nudge everyone else
/// to re-fetch theirs.
///
/// The list cannot simply be broadcast any more: `channels_for` filters private
/// channels per member, while the hub sends one identical frame to every
/// connection. Broadcasting one member's view would either leak the private
/// channels to everyone or hide them from the people who are in them.
fn channel_list_refresh(state: &PartyState, member: Uuid) -> Dispatch {
    Dispatch {
        replies: vec![PartyResponse::Channels(state.channels_for(member))],
        broadcast: vec![PartyResponse::DirectoryChanged],
        directed: Vec::new(),
    }
}

/// Apply `req` to `state` for the connection `conn`, returning the responses to
/// send back to that client. Newly posted messages are stored in `state`; the
/// runtime is responsible for broadcasting them to other connections.
pub fn handle_request(state: &mut PartyState, conn: &mut ConnState, req: PartyRequest) -> Dispatch {
    match req {
        PartyRequest::Join { username, password } => {
            if conn.member.is_some() {
                return Dispatch::reply(PartyResponse::Error(
                    "already joined on this connection".to_string(),
                ));
            }
            // Rate limit: a connection gets a small number of rejected attempts
            // and is then done. Otherwise one handshake buys unlimited password
            // guesses down the same socket.
            if conn.join_failures >= MAX_JOIN_ATTEMPTS {
                return Dispatch::reply(PartyResponse::JoinRejected {
                    reason: "too many failed attempts on this connection — reconnect to try again"
                        .to_string(),
                });
            }
            match state.join(
                &username,
                password.as_deref(),
                conn.peer_fingerprint.clone(),
            ) {
                Ok(id) => {
                    conn.member = Some(id);
                    Dispatch {
                        replies: vec![PartyResponse::Joined {
                            member_id: id,
                            server_name: state.name().to_string(),
                            tier: state.tier(),
                        }],
                        // Everyone else's directory just went stale: either a new
                        // member appeared or a returning one came back online.
                        // Nothing used to announce this, so every client's member
                        // list was frozen at the moment *it* joined — new members
                        // never showed up and the online dots never changed.
                        broadcast: vec![PartyResponse::Members(state.members())],
                        directed: Vec::new(),
                    }
                }
                Err(e) => {
                    conn.join_failures = conn.join_failures.saturating_add(1);
                    Dispatch::reply(PartyResponse::JoinRejected {
                        reason: e.reason().to_string(),
                    })
                }
            }
        }

        // Everything past this point requires a joined member.
        other => {
            let Some(member) = conn.member else {
                return Dispatch::reply(PartyResponse::Error("join required".to_string()));
            };
            match other {
                PartyRequest::Join { .. } => unreachable!("handled above"),
                PartyRequest::ListMembers => {
                    Dispatch::reply(PartyResponse::Members(state.members()))
                }
                // Filtered: a private channel does not advertise its existence to
                // somebody who is not in it.
                PartyRequest::ListChannels => {
                    Dispatch::reply(PartyResponse::Channels(state.channels_for(member)))
                }
                PartyRequest::PostMessage { channel, text } => {
                    match state.post_message(member, channel, text) {
                        // Ack the poster; deliver to everyone else who may read
                        // the channel (see `channel_fanout`).
                        Ok(env) => channel_fanout(state, env),
                        // `ActionFailed`, not `Error`: the client already put
                        // this message on screen and must be able to tell that
                        // *this* send was refused so it can take it back.
                        Err(e) => {
                            Dispatch::reply(PartyResponse::ActionFailed { channel, reason: e })
                        }
                    }
                }
                PartyRequest::FetchHistory { channel, since_seq } => Dispatch::reply(
                    PartyResponse::History(state.history_since(member, channel, since_seq)),
                ),
                PartyRequest::CreateChannel { name } => {
                    match state.create_channel_of_kind(member, &name, ChannelKind::Public, vec![]) {
                        Ok(_) => channel_list_refresh(state, member),
                        Err(e) => Dispatch::reply(PartyResponse::Error(e)),
                    }
                }
                PartyRequest::SendDm { to, text } => match state.post_dm(member, to, text) {
                    // Ack the sender (who appends locally); deliver to the recipient
                    // and to the sender's *other* devices.
                    Ok(env) => Dispatch {
                        replies: vec![PartyResponse::MessagePosted {
                            channel: env.channel,
                            seq: env.seq,
                        }],
                        broadcast: Vec::new(),
                        directed: dm_delivery(member, to, env),
                    },
                    Err(e) => Dispatch::reply(PartyResponse::ActionFailed {
                        channel: messenger_core::party::dm_thread_id(member, to),
                        reason: e,
                    }),
                },
                PartyRequest::FetchDmHistory { with, since_seq } => {
                    let thread = messenger_core::party::dm_thread_id(member, with);
                    Dispatch::reply(PartyResponse::History(state.dm_history(thread, since_seq)))
                }
                PartyRequest::PostFile {
                    channel,
                    name,
                    mime,
                    data,
                } => match state.post_file(member, channel, name, mime, data) {
                    // Like PostMessage, including the private-channel rule.
                    Ok(env) => channel_fanout(state, env),
                    Err(e) => Dispatch::reply(PartyResponse::ActionFailed { channel, reason: e }),
                },
                PartyRequest::SendFileDm {
                    to,
                    name,
                    mime,
                    data,
                } => match state.post_file_dm(member, to, name, mime, data) {
                    // Like SendDm: ack the sender, deliver to the recipient and to
                    // the sender's other devices.
                    Ok(env) => Dispatch {
                        replies: vec![PartyResponse::MessagePosted {
                            channel: env.channel,
                            seq: env.seq,
                        }],
                        broadcast: Vec::new(),
                        directed: dm_delivery(member, to, env),
                    },
                    Err(e) => Dispatch::reply(PartyResponse::ActionFailed {
                        channel: messenger_core::party::dm_thread_id(member, to),
                        reason: e,
                    }),
                },
                PartyRequest::DownloadFile { hash } => match state.blob_bytes_for(member, &hash) {
                    Some(data) => Dispatch::reply(PartyResponse::FileData { hash, data }),
                    // Same reply for unknown and access-denied, so the endpoint
                    // never reveals a file the member isn't allowed to see.
                    None => Dispatch::reply(PartyResponse::Error("unknown file".to_string())),
                },

                // --- Governance and file management ---------------------------
                PartyRequest::CreateChannelOfKind {
                    name,
                    kind,
                    members,
                } => match state.create_channel_of_kind(member, &name, kind, members) {
                    // A new channel changes what everyone may see, and private
                    // channels are filtered per member, so each connection needs
                    // its *own* view rather than one shared list.
                    Ok(_) => channel_list_refresh(state, member),
                    Err(e) => Dispatch::reply(PartyResponse::Error(e)),
                },
                PartyRequest::DeleteChannel { channel } => {
                    match state.delete_channel(member, channel) {
                        Ok(msg) => {
                            let mut out = channel_list_refresh(state, member);
                            out.replies.push(PartyResponse::Ok(msg));
                            out
                        }
                        Err(e) => Dispatch::reply(PartyResponse::Error(e)),
                    }
                }
                PartyRequest::SetChannelAccess {
                    channel,
                    kind,
                    members,
                } => match state.set_channel_access(member, channel, kind, members) {
                    Ok(msg) => {
                        let mut out = channel_list_refresh(state, member);
                        out.replies.push(PartyResponse::Ok(msg));
                        out
                    }
                    Err(e) => Dispatch::reply(PartyResponse::Error(e)),
                },
                PartyRequest::SetRole {
                    member: target,
                    role,
                } => match state.set_role(member, target, role) {
                    Ok(msg) => Dispatch {
                        // A role change alters what the member may *do* and what
                        // they may *see*: `channels_for` filters by role, so a
                        // demoted admin would keep listing private channels
                        // until something else happened to refresh them. The
                        // server still refuses the reads, so that is stale UI
                        // rather than a leak — but it is still wrong on screen.
                        replies: vec![
                            PartyResponse::Members(state.members()),
                            PartyResponse::Channels(state.channels_for(member)),
                            PartyResponse::Ok(msg),
                        ],
                        broadcast: vec![
                            PartyResponse::Members(state.members()),
                            PartyResponse::DirectoryChanged,
                        ],
                        directed: Vec::new(),
                    },
                    Err(e) => Dispatch::reply(PartyResponse::Error(e)),
                },
                PartyRequest::ListFiles => {
                    Dispatch::reply(PartyResponse::Files(state.files_for(member)))
                }
                PartyRequest::DeleteFile { hash, channel } => {
                    match state.delete_file(member, &hash, channel) {
                        Ok(msg) => Dispatch::reply(PartyResponse::Ok(msg)),
                        Err(e) => Dispatch::reply(PartyResponse::Error(e)),
                    }
                }
                PartyRequest::FetchAuditLog { limit } => {
                    match state.audit_log(member, limit.clamp(1, 1000) as usize) {
                        Ok(entries) => Dispatch::reply(PartyResponse::AuditLog(entries)),
                        Err(e) => Dispatch::reply(PartyResponse::Error(e)),
                    }
                }
                PartyRequest::FetchQuota => {
                    Dispatch::reply(PartyResponse::Quota(state.quota_for(member)))
                }

                // --- Chunked upload / download ------------------------------
                PartyRequest::StartUpload {
                    name,
                    mime,
                    size,
                    target,
                } => {
                    match state.start_upload(member, name, mime, size, target, conn.uploads.len()) {
                        Ok(upload) => {
                            conn.uploads.push(upload);
                            Dispatch::reply(PartyResponse::UploadReady {
                                upload,
                                chunk_size: PARTY_CHUNK_BYTES as u32,
                            })
                        }
                        // `ActionFailed`, not `Error`: the client shows the file
                        // optimistically the moment it starts sending, so a
                        // refusal has to be attributable to that message.
                        Err(reason) => Dispatch::reply(PartyResponse::ActionFailed {
                            channel: upload_thread(member, target),
                            reason,
                        }),
                    }
                }
                PartyRequest::UploadChunk { upload, data } => {
                    match state.upload_chunk(member, upload, &data) {
                        Ok(()) => Dispatch::default(),
                        Err(reason) => {
                            conn.uploads.retain(|u| *u != upload);
                            Dispatch::reply(PartyResponse::Error(reason))
                        }
                    }
                }
                PartyRequest::FinishUpload { upload } => {
                    conn.uploads.retain(|u| *u != upload);
                    match state.finish_upload(member, upload) {
                        Ok((env, UploadTarget::Channel(_))) => channel_fanout(state, env),
                        Ok((env, UploadTarget::Dm(to))) => Dispatch {
                            replies: vec![PartyResponse::MessagePosted {
                                channel: env.channel,
                                seq: env.seq,
                            }],
                            broadcast: Vec::new(),
                            directed: dm_delivery(member, to, env),
                        },
                        Err(reason) => Dispatch::reply(PartyResponse::ActionFailed {
                            channel: Uuid::nil(),
                            reason,
                        }),
                    }
                }
                PartyRequest::CancelUpload { upload } => {
                    conn.uploads.retain(|u| *u != upload);
                    state.cancel_upload(member, upload);
                    Dispatch::default()
                }
                PartyRequest::DownloadChunk { hash, offset } => {
                    match state.blob_chunk_for(member, &hash, offset) {
                        Some((data, total)) => Dispatch::reply(PartyResponse::FileChunk {
                            hash,
                            offset,
                            total,
                            data,
                        }),
                        None => Dispatch::reply(PartyResponse::Error("unknown file".to_string())),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use messenger_core::party::MessagePayload;

    /// Join and return the assigned member id.
    fn joined_id(state: &mut PartyState, conn: &mut ConnState, name: &str) -> Uuid {
        match &join(state, conn, name, None)[..] {
            [PartyResponse::Joined { member_id, .. }] => *member_id,
            other => panic!("expected Joined, got {other:?}"),
        }
    }

    fn join(
        state: &mut PartyState,
        conn: &mut ConnState,
        name: &str,
        pw: Option<&str>,
    ) -> Vec<PartyResponse> {
        handle_request(
            state,
            conn,
            PartyRequest::Join {
                username: name.to_string(),
                password: pw.map(str::to_string),
            },
        )
        .replies
    }

    #[test]
    fn join_succeeds_and_sets_member() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        let resp = join(&mut state, &mut conn, "alice", None);
        match &resp[..] {
            [PartyResponse::Joined {
                member_id,
                server_name,
                ..
            }] => {
                assert_eq!(server_name, "Srv");
                assert_eq!(conn.member(), Some(*member_id));
            }
            other => panic!("expected Joined, got {other:?}"),
        }
    }

    /// One handshake must not buy unlimited password guesses: after a few
    /// rejections the connection is finished, forcing an attacker back through
    /// the RSA handshake (and the accept loop's per-IP limiter).
    #[test]
    fn repeated_wrong_passwords_exhaust_the_connection() {
        let mut state = PartyState::new("Srv", Some("pw".to_string()));
        let mut conn = ConnState::default();
        for _ in 0..MAX_JOIN_ATTEMPTS {
            let resp = join(&mut state, &mut conn, "alice", Some("nope"));
            assert!(matches!(resp[..], [PartyResponse::JoinRejected { .. }]));
        }
        // Past the cap the correct password is refused too — the connection
        // itself is spent, which is what makes reconnecting the only way on.
        let resp = join(&mut state, &mut conn, "alice", Some("pw"));
        match &resp[..] {
            [PartyResponse::JoinRejected { reason }] => {
                assert!(reason.contains("too many failed attempts"), "got: {reason}");
            }
            other => panic!("expected a rate-limited rejection, got {other:?}"),
        }
        assert_eq!(conn.member(), None);
    }

    /// Everyone already connected has to be told the directory changed, or their
    /// member list stays frozen at the moment they themselves joined — new
    /// members never appear and the online dots never move.
    #[test]
    fn joining_broadcasts_the_refreshed_directory() {
        let mut state = PartyState::new("Srv", None);
        let mut alice = ConnState::default();
        handle_request(
            &mut state,
            &mut alice,
            PartyRequest::Join {
                username: "alice".to_string(),
                password: None,
            },
        );

        let mut bob = ConnState::default();
        let out = handle_request(
            &mut state,
            &mut bob,
            PartyRequest::Join {
                username: "bob".to_string(),
                password: None,
            },
        );

        assert!(matches!(out.replies[..], [PartyResponse::Joined { .. }]));
        match &out.broadcast[..] {
            [PartyResponse::Members(members)] => {
                let names: Vec<&str> = members.iter().map(|m| m.username.as_str()).collect();
                assert_eq!(names, vec!["alice", "bob"]);
                assert!(members.iter().all(|m| m.online));
            }
            other => panic!("expected a Members broadcast, got {other:?}"),
        }
    }

    /// A DM must be delivered to the recipient *and* echoed to the sender's other
    /// devices. The runtime excludes the originating connection, so the echo
    /// cannot duplicate the message on the client that sent it.
    #[test]
    fn a_dm_is_directed_to_both_participants() {
        let mut state = PartyState::new("Srv", None);
        let mut alice_conn = ConnState::default();
        let alice = match &join(&mut state, &mut alice_conn, "alice", None)[..] {
            [PartyResponse::Joined { member_id, .. }] => *member_id,
            other => panic!("expected Joined, got {other:?}"),
        };
        let mut bob_conn = ConnState::default();
        let bob = match &join(&mut state, &mut bob_conn, "bob", None)[..] {
            [PartyResponse::Joined { member_id, .. }] => *member_id,
            other => panic!("expected Joined, got {other:?}"),
        };

        let out = handle_request(
            &mut state,
            &mut alice_conn,
            PartyRequest::SendDm {
                to: bob,
                text: "hello".to_string(),
            },
        );

        assert!(matches!(
            out.replies[..],
            [PartyResponse::MessagePosted { .. }]
        ));
        let targets: Vec<Uuid> = out.directed.iter().map(|(m, _)| *m).collect();
        assert_eq!(
            targets,
            vec![bob, alice],
            "the recipient and the sender's other devices both need this DM"
        );
        assert!(out
            .directed
            .iter()
            .all(|(_, r)| matches!(r, PartyResponse::Message(_))));
    }

    /// A private channel's messages must not be broadcast.
    ///
    /// `broadcast_except` pushes to every registered connection, so fanning a
    /// private channel out that way handed its live traffic to members who are
    /// correctly not shown the channel at all — they would not see it in
    /// `ListChannels` and could not fetch its history, but every message posted
    /// to it arrived on their socket anyway.
    #[test]
    fn a_private_channel_is_delivered_only_to_its_members() {
        let mut state = PartyState::new("Srv", None);
        let mut owner_conn = ConnState::default();
        let owner = joined_id(&mut state, &mut owner_conn, "owner");
        let mut bob_conn = ConnState::default();
        let bob = joined_id(&mut state, &mut bob_conn, "bob");
        let mut carol_conn = ConnState::default();
        let carol = joined_id(&mut state, &mut carol_conn, "carol");

        let private = state
            .create_channel_of_kind(owner, "secret", ChannelKind::Private, vec![bob])
            .unwrap()
            .id;

        let out = handle_request(
            &mut state,
            &mut owner_conn,
            PartyRequest::PostMessage {
                channel: private,
                text: "members only".to_string(),
            },
        );

        assert!(
            out.broadcast.is_empty(),
            "a private channel must not be broadcast to every connection"
        );
        let targets: Vec<Uuid> = out.directed.iter().map(|(m, _)| *m).collect();
        assert!(targets.contains(&owner), "the poster's other devices");
        assert!(targets.contains(&bob), "a channel member");
        assert!(
            !targets.contains(&carol),
            "carol is not in the channel and must not receive its messages"
        );
    }

    /// A public channel still takes the cheap path.
    #[test]
    fn a_public_channel_is_still_broadcast() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        let _owner = joined_id(&mut state, &mut conn, "owner");
        let channel = state.default_channel();
        let out = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::PostMessage {
                channel,
                text: "hello all".to_string(),
            },
        );
        assert!(matches!(&out.broadcast[..], [PartyResponse::Message(_)]));
        assert!(out.directed.is_empty());
    }

    /// A DM addressed to yourself must not be delivered twice.
    #[test]
    fn a_self_dm_is_delivered_once() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        let me = match &join(&mut state, &mut conn, "alice", None)[..] {
            [PartyResponse::Joined { member_id, .. }] => *member_id,
            other => panic!("expected Joined, got {other:?}"),
        };
        let out = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::SendDm {
                to: me,
                text: "note to self".to_string(),
            },
        );
        assert_eq!(out.directed.len(), 1);
        assert_eq!(out.directed[0].0, me);
    }

    /// The cap counts *failures*, so an honest user who mistypes once and then
    /// gets it right is not punished.
    #[test]
    fn a_successful_join_is_unaffected_by_an_earlier_mistake() {
        let mut state = PartyState::new("Srv", Some("pw".to_string()));
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", Some("nope"));
        let resp = join(&mut state, &mut conn, "alice", Some("pw"));
        assert!(matches!(resp[..], [PartyResponse::Joined { .. }]));
        assert!(conn.member().is_some());
    }

    #[test]
    fn wrong_password_is_rejected_and_leaves_connection_unjoined() {
        let mut state = PartyState::new("Srv", Some("pw".to_string()));
        let mut conn = ConnState::default();
        let resp = join(&mut state, &mut conn, "alice", Some("nope"));
        assert!(matches!(resp[..], [PartyResponse::JoinRejected { .. }]));
        assert_eq!(conn.member(), None);
    }

    #[test]
    fn double_join_on_one_connection_errors() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", None);
        let resp = join(&mut state, &mut conn, "alice2", None);
        assert!(matches!(resp[..], [PartyResponse::Error(_)]));
    }

    #[test]
    fn actions_before_join_require_join() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        for req in [
            PartyRequest::ListMembers,
            PartyRequest::ListChannels,
            PartyRequest::PostMessage {
                channel: Uuid::new_v4(),
                text: "hi".to_string(),
            },
            PartyRequest::FetchHistory {
                channel: Uuid::new_v4(),
                since_seq: 0,
            },
        ] {
            let resp = handle_request(&mut state, &mut conn, req).replies;
            assert!(
                matches!(resp[..], [PartyResponse::Error(ref m)] if m == "join required"),
                "expected join-required error, got {resp:?}"
            );
        }
    }

    #[test]
    fn post_then_fetch_history_round_trips_through_dispatch() {
        let mut state = PartyState::new("Srv", None);
        let channel = state.default_channel();
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", None);

        let posted = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::PostMessage {
                channel,
                text: "first message".to_string(),
            },
        );
        match &posted.replies[..] {
            [PartyResponse::MessagePosted { channel: c, seq }] => {
                assert_eq!(*c, channel);
                assert_eq!(*seq, 1);
            }
            other => panic!("expected MessagePosted, got {other:?}"),
        }
        // The post is also queued for broadcast to other members.
        assert!(
            matches!(&posted.broadcast[..], [PartyResponse::Message(env)] if env.seq == 1),
            "expected one broadcast Message with seq 1, got {:?}",
            posted.broadcast
        );

        let history = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::FetchHistory {
                channel,
                since_seq: 0,
            },
        )
        .replies;
        match &history[..] {
            [PartyResponse::History(items)] => {
                assert_eq!(items.len(), 1);
                assert_eq!(
                    items[0].payload,
                    MessagePayload::Text("first message".to_string())
                );
            }
            other => panic!("expected History, got {other:?}"),
        }
    }

    #[test]
    fn list_members_and_channels_after_join() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", None);

        let members = handle_request(&mut state, &mut conn, PartyRequest::ListMembers).replies;
        assert!(matches!(&members[..], [PartyResponse::Members(m)] if m.len() == 1));

        let channels = handle_request(&mut state, &mut conn, PartyRequest::ListChannels).replies;
        assert!(matches!(&channels[..], [PartyResponse::Channels(c)] if c.len() == 1));
    }

    #[test]
    fn create_channel_replies_and_broadcasts_the_new_list() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", None);

        let out = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::CreateChannel {
                name: "random".to_string(),
            },
        );
        // The creator gets their own filtered list. Everyone else is nudged to
        // re-fetch theirs rather than being handed this one: the list is per
        // member now, so broadcasting one member's view would either leak
        // private channels or hide them from the people who are in them.
        assert!(matches!(&out.replies[..], [PartyResponse::Channels(c)] if c.len() == 2));
        assert!(matches!(
            &out.broadcast[..],
            [PartyResponse::DirectoryChanged]
        ));
    }

    #[test]
    fn create_channel_before_join_requires_join() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        let out = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::CreateChannel {
                name: "x".to_string(),
            },
        );
        assert!(matches!(&out.replies[..], [PartyResponse::Error(m)] if m == "join required"));
    }

    #[test]
    fn post_file_acks_the_poster_and_broadcasts_a_file_message() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", None);
        let channel = state.default_channel();

        let out = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::PostFile {
                channel,
                name: "pic.png".to_string(),
                mime: "image/png".to_string(),
                data: b"\x89PNG data".to_vec(),
            },
        );
        assert!(matches!(
            &out.replies[..],
            [PartyResponse::MessagePosted { .. }]
        ));
        match &out.broadcast[..] {
            [PartyResponse::Message(env)] => {
                assert!(matches!(&env.payload, MessagePayload::File(f) if f.name == "pic.png"));
            }
            other => panic!("expected a broadcast File message, got {other:?}"),
        }
    }

    #[test]
    fn download_returns_bytes_for_known_and_errors_for_unknown() {
        let mut state = PartyState::new("Srv", None);
        let mut conn = ConnState::default();
        join(&mut state, &mut conn, "alice", None);
        let channel = state.default_channel();

        let post = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::PostFile {
                channel,
                name: "f.bin".to_string(),
                mime: "application/octet-stream".to_string(),
                data: b"payload".to_vec(),
            },
        );
        let hash = match &post.broadcast[..] {
            [PartyResponse::Message(env)] => match &env.payload {
                MessagePayload::File(f) => f.hash.clone(),
                other => panic!("expected File payload, got {other:?}"),
            },
            other => panic!("expected broadcast, got {other:?}"),
        };

        let ok = handle_request(&mut state, &mut conn, PartyRequest::DownloadFile { hash });
        assert!(
            matches!(&ok.replies[..], [PartyResponse::FileData { data, .. }] if data == b"payload")
        );

        let missing = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::DownloadFile {
                hash: "deadbeef".to_string(),
            },
        );
        assert!(matches!(&missing.replies[..], [PartyResponse::Error(_)]));
    }
}
