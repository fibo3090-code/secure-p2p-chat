//! Hostile input from community members, against the server's own state rules.
//!
//! The UIs validate what a polite member types; a member is free to send the
//! server anything. Each test sends something no UI would, and asserts the
//! server refuses it or stays consistent.

use messenger_core::party::{MessagePayload, PartyRequest, PartyResponse};
use uuid::Uuid;

use crate::dispatch::{handle_request, ConnState};
use crate::state::PartyState;

fn server_with(names: &[&str]) -> (PartyState, Vec<Uuid>) {
    let mut state = PartyState::new("Adversarial", None);
    let ids = names
        .iter()
        .map(|n| state.join(n, None, Some(format!("fp-{n}"))).expect("join"))
        .collect();
    (state, ids)
}

#[test]
fn blank_messages_are_refused_in_channels_and_dms() {
    let (mut state, ids) = server_with(&["alice", "bob"]);
    let general = state.default_channel();
    for blank in ["", "   ", "\n\t \n"] {
        assert!(
            state
                .post_message(ids[0], general, blank.to_string())
                .is_err(),
            "a blank channel message {blank:?} was stored"
        );
        assert!(
            state.post_dm(ids[0], ids[1], blank.to_string()).is_err(),
            "a blank DM {blank:?} was stored"
        );
    }
    assert!(state.history_since(ids[0], general, 0).is_empty());
}

#[test]
fn usernames_that_hide_or_rewrite_themselves_are_refused() {
    let mut state = PartyState::new("Adversarial", None);
    state.join("admin", None, Some("fp-real".into())).unwrap();
    for (i, spoof) in [
        "ad\u{200B}min",    // zero-width space: renders as "admin"
        "\u{202E}nimda",    // right-to-left override: renders as "admin"
        "admin\u{2060}",    // word joiner
        "\u{FEFF}admin",    // byte-order mark
        "bob\nadmin",       // a line break inside a name
        "\u{200B}\u{200B}", // nothing visible at all
        "tab\there",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            state
                .join(spoof, None, Some(format!("fp-spoof-{i}")))
                .is_err(),
            "{spoof:?} was accepted as a username"
        );
    }
    // Ordinary names in any script are fine.
    for (i, ok) in ["Zoë", "李雷", "José María", "ميلا"].into_iter().enumerate() {
        assert!(
            state.join(ok, None, Some(format!("fp-ok-{i}"))).is_ok(),
            "{ok:?} was refused"
        );
    }
}

#[test]
fn a_username_cannot_be_taken_again_by_case_or_padding() {
    let mut state = PartyState::new("Adversarial", None);
    state.join("alice", None, Some("fp-a".into())).unwrap();
    for (i, again) in ["Alice", "ALICE", "  alice  "].into_iter().enumerate() {
        assert!(
            state.join(again, None, Some(format!("fp-{i}"))).is_err(),
            "{again:?} took alice's name"
        );
    }
}

#[test]
fn channel_names_that_hide_or_rewrite_themselves_are_refused() {
    let (mut state, _) = server_with(&["alice"]);
    for spoof in [
        "gen\u{200B}eral",
        "\u{202E}lareneg",
        "two\nlines",
        "\u{2066}x\u{2069}",
    ] {
        assert!(
            state.create_channel(spoof).is_err(),
            "{spoof:?} was accepted as a channel name"
        );
    }
    assert!(state.create_channel("random").is_ok());
    assert!(
        state.create_channel("RANDOM").is_err(),
        "a channel name was duplicated by case"
    );
}

#[test]
fn posts_to_nowhere_are_refused() {
    let (mut state, ids) = server_with(&["alice"]);
    let general = state.default_channel();
    assert!(state
        .post_message(ids[0], Uuid::new_v4(), "hi".into())
        .is_err());
    assert!(state.post_dm(ids[0], Uuid::new_v4(), "hi".into()).is_err());
    assert!(state
        .post_message(Uuid::new_v4(), general, "hi".into())
        .is_err());
    assert!(state.post_dm(Uuid::new_v4(), ids[0], "hi".into()).is_err());
}

#[test]
fn username_length_counts_characters_not_bytes() {
    let mut state = PartyState::new("Adversarial", None);
    assert!(state.join(&"é".repeat(32), None, Some("a".into())).is_ok());
    assert!(state.join(&"x".repeat(33), None, Some("b".into())).is_err());
}

#[test]
fn every_request_before_joining_is_refused() {
    let mut state = PartyState::new("Adversarial", None);
    let general = state.default_channel();
    let mut conn = ConnState::default();
    let requests = vec![
        PartyRequest::ListMembers,
        PartyRequest::ListChannels,
        PartyRequest::PostMessage {
            channel: general,
            text: "sneaky".into(),
        },
        PartyRequest::FetchHistory {
            channel: general,
            since_seq: 0,
        },
        PartyRequest::CreateChannel {
            name: "mine".into(),
        },
        PartyRequest::DownloadFile { hash: "00".into() },
        PartyRequest::FetchAuditLog { limit: 10 },
    ];
    for req in requests {
        let out = handle_request(&mut state, &mut conn, req);
        assert!(
            matches!(out.replies.as_slice(), [PartyResponse::Error(_)])
                && out.broadcast.is_empty()
                && out.directed.is_empty()
                && out.deferred.is_none(),
            "a request before joining did something: {:?}",
            out.replies
        );
    }
    assert!(state.channels().iter().all(|c| c.name != "mine"));
    assert!(state.history_since(Uuid::nil(), general, 0).is_empty());
    let _ = MessagePayload::Text(String::new());
}
