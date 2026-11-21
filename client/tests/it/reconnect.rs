//! Reconnecting to a peer you have already verified: a verified conversation,
//! a restart of the dialing side, and a re-dial by address — the way both UIs
//! reconnect.
//!
//! What must hold afterwards, on **both** sides:
//!
//! - **One conversation per peer.** The re-dial starts a fresh chat (nothing at
//!   dial time says who will answer); once the peer's verified identity
//!   arrives it is folded back into the conversation that holds the history.
//!   The dialing side used to leave a second, empty "host:port" thread.
//! - **No second safety-code prompt** for a peer both sides already verified.
//! - **Messages flow both ways in the old conversation**, which also proves
//!   the replay counters were reset for the new session.

use crate::common::*;
use p2pem_classic::app::ChatManager;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redialing_a_verified_peer_continues_the_same_conversation() {
    let host_dir = tempfile::tempdir().expect("host dir");
    let client_dir = tempfile::tempdir().expect("client dir");
    let client_history = client_dir.path().join("history.json.enc");
    let history_key = [7u8; 32];
    let mut host = manager_in(host_dir.path());
    host.set_history_key([9u8; 32]);
    let mut client = manager_in(client_dir.path());
    client.set_history_key(history_key);
    let host_key = rsa_key();
    let client_key = rsa_key();

    // --- First contact. ---
    let port = host_on_any_port(&mut host, host_key.clone()).await;
    let (host_chat, client_chat) =
        dial_and_verify(&mut host, &mut client, port, client_key.clone()).await;
    client
        .send_message(client_chat, "before".to_string())
        .expect("send before");
    pump_pair(&mut host, &mut client, "first message", |h, _| {
        texts(h, host_chat) == ["before"]
    })
    .await;

    // --- The dialing side restarts. ---
    client.save_history(&client_history).expect("save");
    drop(client);
    // Dropping the manager must close its sessions, or the host would not
    // notice until its idle timeout.
    pump_pair(
        &mut host,
        &mut ChatManager::default(),
        "host sees the drop",
        |h, _| !h.is_connected(&host_chat),
    )
    .await;
    let mut client = manager_in(client_dir.path());
    client.set_history_key(history_key);
    client
        .load_history_auto(&client_history, &history_key)
        .expect("load");
    assert_eq!(conversations(&client), vec![client_chat]);

    // The first peer consumed the listener; re-open it on the same port, the
    // way both shells' auto-rehost does.
    assert!(host.check_rehost_needed());
    host.start_host(port, host_key).await.expect("rehost");
    assert_eq!(host.hosting_port, Some(port), "a rehost keeps its port");

    // --- Re-dial by address, exactly as the UIs do (no existing chat id). ---
    let redial = client
        .connect_to_host("127.0.0.1", port, None, client_key)
        .await
        .expect("redial");
    assert_ne!(redial, client_chat, "a dial by address starts a fresh chat");
    pump_pair(&mut host, &mut client, "second session", |h, c| {
        h.is_connected(&host_chat) && c.is_connected(&client_chat)
    })
    .await;

    assert!(
        host.pending_fingerprint().is_none() && client.pending_fingerprint().is_none(),
        "a peer both sides already verified must not be prompted again"
    );
    assert_eq!(
        conversations(&client),
        vec![client_chat],
        "the re-dial left a second conversation beside the one with the history"
    );
    assert_eq!(conversations(&host), vec![host_chat]);
    assert_eq!(client.resolve_display_chat_id(redial), client_chat);

    client
        .send_message(client_chat, "after".to_string())
        .expect("client sends after reconnect");
    host.send_message(host_chat, "reply".to_string())
        .expect("host sends after reconnect");
    // Each side lists its own message first, so compare without order.
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    pump_pair(
        &mut host,
        &mut client,
        "messages after reconnect",
        |h, c| {
            sorted(texts(h, host_chat)) == ["after", "before", "reply"]
                && sorted(texts(c, client_chat)) == ["after", "before", "reply"]
        },
    )
    .await;
}

/// NET-12: a contact can carry both a direct address and a relay route. When
/// the address has gone stale (the peer changed networks), the dial must fall
/// through to the relay — into the same conversation — instead of failing
/// direct-only and never trying the route that would have worked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_contact_whose_address_went_stale_is_reached_through_its_relay() {
    use messenger_core::network::{
        generate_relay_token, serve_relay, NO_HOLEPUNCH_ENV, RELAY_WAIT_TIMEOUT,
    };
    use messenger_core::types::Transport;

    // Bridged, so the route that won is unambiguous. Restored on the way out.
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            std::env::remove_var(NO_HOLEPUNCH_ENV);
        }
    }
    let _restore = Restore;
    std::env::set_var(NO_HOLEPUNCH_ENV, "1");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let relay = listener.local_addr().expect("addr").to_string();
    tokio::spawn(serve_relay(listener, RELAY_WAIT_TIMEOUT));

    let host_dir = tempfile::tempdir().unwrap();
    let client_dir = tempfile::tempdir().unwrap();
    let mut host = manager_in(host_dir.path());
    let mut client = manager_in(client_dir.path());
    let token = generate_relay_token();
    host.start_host_via_relay(&relay, Some(token.clone()), rsa_key())
        .await
        .expect("host on the relay");

    // A port nothing listens on any more: the contact's stale address.
    let dead = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let contact = client.add_contact("Moved".into(), Some(dead.to_string()), None, None);
    {
        let c = client.contacts.get_mut(&contact).unwrap();
        c.relay_server = Some(relay.clone());
        c.relay_token = Some(token);
    }
    let chat = client
        .connect_to_contact(contact, None, &rsa_key())
        .await
        .expect("dial the contact");

    pump_pair(&mut host, &mut client, "both prompts", |h, c| {
        h.pending_fingerprint().is_some() && c.pending_fingerprint().is_some()
    })
    .await;
    let prompt = host.pending_fingerprint().unwrap().session_id;
    host.confirm_fingerprint(prompt, true).unwrap();
    let prompt = client.pending_fingerprint().unwrap().session_id;
    client.confirm_fingerprint(prompt, true).unwrap();
    pump_pair(
        &mut host,
        &mut client,
        "connected through the relay",
        |_, c| c.is_connected(&chat),
    )
    .await;
    assert_eq!(
        client.chats[&chat].transport,
        Transport::Relay,
        "the conversation must say which route it is on"
    );
    assert_eq!(client.contact_to_chat.get(&contact), Some(&chat));
}

/// NET-11: a connection password set on the host applies when it hosts
/// through a relay too. It used to be dropped on that path, so anyone holding
/// the relay token reached the safety-code prompt, and nothing told the user
/// their password was not being asked for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_password_is_enforced_over_the_relay() {
    use messenger_core::network::{generate_relay_token, serve_relay, RELAY_WAIT_TIMEOUT};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let relay = listener.local_addr().expect("addr").to_string();
    tokio::spawn(serve_relay(listener, RELAY_WAIT_TIMEOUT));
    let password = format!("pw-{}", uuid::Uuid::new_v4());

    for (supplied, admitted) in [(None, false), (Some(password.clone()), true)] {
        let host_dir = tempfile::tempdir().unwrap();
        let client_dir = tempfile::tempdir().unwrap();
        let mut host = manager_in(host_dir.path());
        let mut client = manager_in(client_dir.path());
        host.set_connection_password(Some(password.clone()));
        client.set_connection_password(supplied.clone());
        let token = generate_relay_token();
        host.start_host_via_relay(&relay, Some(token.clone()), rsa_key())
            .await
            .expect("host on the relay");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        client
            .connect_via_relay(&relay, &token, None, rsa_key())
            .await
            .expect("dial through the relay");

        if admitted {
            pump_pair(&mut host, &mut client, "both prompts", |h, c| {
                h.pending_fingerprint().is_some() && c.pending_fingerprint().is_some()
            })
            .await;
        } else {
            pump_pair(&mut host, &mut client, "refused", |h, _| {
                h.toasts
                    .iter()
                    .any(|t| t.message.to_lowercase().contains("password"))
            })
            .await;
            assert!(
                host.pending_fingerprint().is_none(),
                "a peer without the password reached the safety-code prompt"
            );
        }
    }
}
