//! Party request dispatcher (Phase 1, slice 1b).
//!
//! The pure protocol-handling layer: given the per-connection session state and an
//! incoming [`PartyRequest`], it applies the request to the shared [`PartyState`]
//! and produces the [`PartyResponse`]s to send back. It enforces the "must join
//! before acting" rule and is completely network-free, so it is unit-tested
//! directly. The runtime (next slice) owns the transport and fan-out: it decrypts
//! a request off the v3 tunnel, calls [`handle_request`], encrypts the replies, and
//! separately broadcasts newly posted messages to other online members.

use messenger_core::party::{PartyRequest, PartyResponse};
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
}

impl ConnState {
    /// Create connection state carrying the handshake-verified peer fingerprint.
    pub fn with_fingerprint(fingerprint: String) -> Self {
        Self {
            member: None,
            peer_fingerprint: Some(fingerprint),
            join_failures: 0,
        }
    }

    /// The joined member's id, if this connection has completed `Join`.
    pub fn member(&self) -> Option<Uuid> {
        self.member
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
                    Dispatch::reply(PartyResponse::Joined {
                        member_id: id,
                        server_name: state.name().to_string(),
                        tier: state.tier(),
                    })
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
                PartyRequest::ListChannels => {
                    Dispatch::reply(PartyResponse::Channels(state.channels()))
                }
                PartyRequest::PostMessage { channel, text } => {
                    match state.post_message(member, channel, text) {
                        // Ack the poster, and broadcast the stored message to others.
                        Ok(env) => Dispatch {
                            replies: vec![PartyResponse::MessagePosted {
                                channel: env.channel,
                                seq: env.seq,
                            }],
                            broadcast: vec![PartyResponse::Message(env)],
                            directed: Vec::new(),
                        },
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
                PartyRequest::CreateChannel { name } => match state.create_channel(&name) {
                    // Refresh everyone's channel list (reply to creator + broadcast).
                    Ok(_) => Dispatch {
                        replies: vec![PartyResponse::Channels(state.channels())],
                        broadcast: vec![PartyResponse::Channels(state.channels())],
                        directed: Vec::new(),
                    },
                    Err(e) => Dispatch::reply(PartyResponse::Error(e)),
                },
                PartyRequest::SendDm { to, text } => match state.post_dm(member, to, text) {
                    // Ack the sender (who appends locally); deliver to the recipient.
                    Ok(env) => Dispatch {
                        replies: vec![PartyResponse::MessagePosted {
                            channel: env.channel,
                            seq: env.seq,
                        }],
                        broadcast: Vec::new(),
                        directed: vec![(to, PartyResponse::Message(env))],
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
                    // Like PostMessage: ack the poster, broadcast the file message.
                    Ok(env) => Dispatch {
                        replies: vec![PartyResponse::MessagePosted {
                            channel: env.channel,
                            seq: env.seq,
                        }],
                        broadcast: vec![PartyResponse::Message(env)],
                        directed: Vec::new(),
                    },
                    Err(e) => Dispatch::reply(PartyResponse::ActionFailed { channel, reason: e }),
                },
                PartyRequest::SendFileDm {
                    to,
                    name,
                    mime,
                    data,
                } => match state.post_file_dm(member, to, name, mime, data) {
                    // Like SendDm: ack the sender, deliver to the recipient.
                    Ok(env) => Dispatch {
                        replies: vec![PartyResponse::MessagePosted {
                            channel: env.channel,
                            seq: env.seq,
                        }],
                        broadcast: Vec::new(),
                        directed: vec![(to, PartyResponse::Message(env))],
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
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use messenger_core::party::MessagePayload;

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
        // Creator gets the refreshed list, and others get it broadcast.
        assert!(matches!(&out.replies[..], [PartyResponse::Channels(c)] if c.len() == 2));
        assert!(matches!(&out.broadcast[..], [PartyResponse::Channels(c)] if c.len() == 2));
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
