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

/// Per-connection session state. A connection must `Join` before any other
/// request is honoured; once joined it carries the member's id. `peer_fingerprint`
/// is the identity verified during the v3 handshake, bound to the member on join.
#[derive(Debug, Default)]
pub struct ConnState {
    member: Option<Uuid>,
    peer_fingerprint: Option<String>,
}

impl ConnState {
    /// Create connection state carrying the handshake-verified peer fingerprint.
    pub fn with_fingerprint(fingerprint: String) -> Self {
        Self {
            member: None,
            peer_fingerprint: Some(fingerprint),
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
pub fn handle_request(
    state: &mut PartyState,
    conn: &mut ConnState,
    req: PartyRequest,
) -> Vec<PartyResponse> {
    match req {
        PartyRequest::Join { username, password } => {
            if conn.member.is_some() {
                return vec![PartyResponse::Error(
                    "already joined on this connection".to_string(),
                )];
            }
            match state.join(
                &username,
                password.as_deref(),
                conn.peer_fingerprint.clone(),
            ) {
                Ok(id) => {
                    conn.member = Some(id);
                    vec![PartyResponse::Joined {
                        member_id: id,
                        server_name: state.name().to_string(),
                        tier: state.tier(),
                    }]
                }
                Err(e) => vec![PartyResponse::JoinRejected {
                    reason: e.reason().to_string(),
                }],
            }
        }

        // Everything past this point requires a joined member.
        other => {
            let Some(member) = conn.member else {
                return vec![PartyResponse::Error("join required".to_string())];
            };
            match other {
                PartyRequest::Join { .. } => unreachable!("handled above"),
                PartyRequest::ListMembers => vec![PartyResponse::Members(state.members())],
                PartyRequest::ListChannels => vec![PartyResponse::Channels(state.channels())],
                PartyRequest::PostMessage { channel, text } => {
                    match state.post_message(member, channel, text) {
                        Ok(env) => vec![PartyResponse::MessagePosted {
                            channel: env.channel,
                            seq: env.seq,
                        }],
                        Err(e) => vec![PartyResponse::Error(e)],
                    }
                }
                PartyRequest::FetchHistory { channel, since_seq } => {
                    vec![PartyResponse::History(
                        state.history_since(channel, since_seq),
                    )]
                }
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
            let resp = handle_request(&mut state, &mut conn, req);
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
        match &posted[..] {
            [PartyResponse::MessagePosted { channel: c, seq }] => {
                assert_eq!(*c, channel);
                assert_eq!(*seq, 1);
            }
            other => panic!("expected MessagePosted, got {other:?}"),
        }

        let history = handle_request(
            &mut state,
            &mut conn,
            PartyRequest::FetchHistory {
                channel,
                since_seq: 0,
            },
        );
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

        let members = handle_request(&mut state, &mut conn, PartyRequest::ListMembers);
        assert!(matches!(&members[..], [PartyResponse::Members(m)] if m.len() == 1));

        let channels = handle_request(&mut state, &mut conn, PartyRequest::ListChannels);
        assert!(matches!(&channels[..], [PartyResponse::Channels(c)] if c.len() == 1));
    }
}
