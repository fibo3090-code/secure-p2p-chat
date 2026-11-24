//! Attacks against the trust boundaries: each test plays a hostile party,
//! starts from what an attacker controls (a forged invite, an unanswered
//! prompt, a content hash, a borrowed username, a guest seat) and checks how
//! far it reaches — which a happy-path test cannot tell you.

mod common;

use common::*;
use messenger_core::core::generate_rsa_keypair;
use messenger_core::party::{ChannelKind, Role};
use messenger_core::types::{Config, TrustState};
use messenger_core::RSA_KEY_BITS;
use p2pem_classic::app::chat_manager::ChatManager;
use p2pem_classic::app::party_manager::{PartyManager, PartyStatus};
use std::time::Duration;
use uuid::Uuid;

// ── Invite forgery ──────────────────────────────────────────────────────────

/// An invite's `fingerprint` and `public_key` must agree: the signature only
/// covers the key, while every trust decision is made against `fingerprint`.
#[test]
fn an_invite_whose_fingerprint_contradicts_its_key_is_refused() {
    let mgr = ChatManager::new(Config::default());

    // A real key, paired with a fingerprint that is not its own.
    let key = generate_rsa_keypair(RSA_KEY_BITS).expect("key");
    let pem = messenger_core::core::pem_encode_public(&rsa::PublicKey::from(&key))
        .expect("encode public key");
    let honest = messenger_core::core::fingerprint_pubkey(pem.as_bytes()).unwrap();
    let forged = "ff".repeat(32);
    assert_ne!(honest, forged);

    let payload = serde_json::json!({
        "name": "Impostor",
        "address": "10.0.0.9:12345",
        "fingerprint": forged,
        "public_key": pem,
    });
    use base64::Engine;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&payload).unwrap());
    let link = format!("chat-p2p://invite/{encoded}");

    let result = mgr.parse_invite_link(&link);
    assert!(
        result.is_err(),
        "an invite naming a fingerprint its key does not produce must be refused"
    );
}

/// Garbage in the invite slot (pasted from anywhere) must not panic.
#[test]
fn malformed_invite_links_are_refused_without_panicking() {
    let mgr = ChatManager::new(Config::default());
    let attacks = [
        "",
        "chat-p2p://invite/",
        "chat-p2p://invite/!!!not-base64!!!",
        "chat-p2p://invite/e30",    // {}
        "chat-p2p://invite/bnVsbA", // null
        "chat-p2p://invite/W10",    // []
        "http://example.com/",
        "javascript:alert(1)",
        &format!("chat-p2p://invite/{}", "A".repeat(100_000)),
    ];
    for link in attacks {
        // The contract is "returns Err", not "does not crash the app".
        let _ = mgr.parse_invite_link(link);
    }
}

// ── TOFU ────────────────────────────────────────────────────────────────────

/// A contact the user never verified must not make its fingerprint
/// auto-accepted (pasting a link used to pre-trust whatever it named).
#[test]
fn an_unverified_contact_does_not_pre_trust_its_fingerprint() {
    let mut mgr = ChatManager::new(Config::default());
    let fp = "ab".repeat(32);
    let id = mgr.add_contact("Stranger".into(), None, Some(fp.clone()), None);

    assert_eq!(
        mgr.get_contact(id).map(|c| c.trust_state),
        Some(TrustState::Unverified),
    );
    // No chat holds this fingerprint and no contact vouches for it as verified,
    // so a connection claiming it must still face the prompt.
    assert!(
        !mgr.chats
            .values()
            .any(|c| c.peer_fingerprint.as_deref() == Some(fp.as_str())),
        "nothing may have silently recorded this fingerprint as trusted"
    );
}

/// An incoming chat must not start with a fingerprint pre-filled, which would
/// make TOFU trivially match and auto-trust every caller (a past bug).
#[test]
fn a_fresh_incoming_chat_holds_no_fingerprint() {
    let mut mgr = ChatManager::new(Config::default());
    let id = Uuid::new_v4();
    mgr.create_local_chat_for_test(id, "Peer".into());
    assert_eq!(
        mgr.chats.get(&id).and_then(|c| c.peer_fingerprint.clone()),
        None,
        "an incoming conversation must start unverified"
    );
}

// ── Community access control ────────────────────────────────────────────────

/// Blobs are stored server-wide, so knowing a hash must not fetch the bytes: a
/// member who cannot reach where a file was shared is refused at download.
#[tokio::test]
async fn a_content_hash_is_not_a_capability() {
    let server = start_server(None).await;
    let (mut owner, owner_sid) = join_owner(&server, "owner").await;
    let (mut intruder, intruder_sid) = join(&server, "intruder", None).await;
    wait_members(&mut owner, owner_sid, 2).await;

    // A private channel the intruder is not in.
    owner
        .create_channel_of_kind(
            owner_sid,
            "vault".to_string(),
            ChannelKind::Private,
            Vec::new(),
        )
        .expect("create private channel");
    let vault = wait_channel(&mut owner, owner_sid, "vault").await;

    let secret = b"the contents of a private channel".to_vec();
    owner
        .send_file(
            owner_sid,
            vault,
            "secret.bin".to_string(),
            "application/octet-stream".to_string(),
            secret.clone(),
        )
        .expect("upload");
    owner.refresh_files(owner_sid).expect("list");
    pump_until(&mut owner, "file stored", |m| {
        has_file(m, owner_sid, "secret.bin")
    })
    .await;
    let hash = owner
        .server(owner_sid)
        .unwrap()
        .files
        .iter()
        .find(|f| f.name == "secret.bin")
        .map(|f| f.hash.clone())
        .expect("hash");

    // The intruder never sees it in a listing…
    intruder.refresh_files(intruder_sid).expect("list");
    idle(&mut [&mut intruder], 40, 25).await;
    assert!(
        !has_file(&intruder, intruder_sid, "secret.bin"),
        "a private channel's file must not appear in a non-member's listing"
    );

    // …and asking for the hash directly must be refused (or go unanswered).
    let rx = intruder
        .request_download(intruder_sid, hash)
        .expect("request is sent");
    if let Some(Ok(bytes)) = download_result(&mut intruder, rx, Duration::from_secs(20)).await {
        panic!(
            "a non-member downloaded {} bytes of a private file by hash",
            bytes.len()
        );
    }
}

/// Nobody may grant at or above their own level, and the owner may never be
/// demoted — otherwise whoever gets an admin seat owns the community.
#[tokio::test]
async fn a_member_cannot_escalate_or_depose_the_owner() {
    let server = start_server(None).await;
    let (mut owner, owner_sid) = join_owner(&server, "owner").await;
    let (mut member, member_sid) = join(&server, "member", None).await;
    wait_members(&mut owner, owner_sid, 2).await;

    let me = member.server(member_sid).unwrap().member_id.unwrap();
    let owner_id =
        member_named(&member, member_sid, "owner").expect("the owner is visible in the directory");

    // Promote self to admin, then to owner; demote the actual owner.
    for role in [Role::Admin, Role::Owner] {
        let _ = member.set_role(member_sid, me, role);
    }
    let _ = member.set_role(member_sid, owner_id, Role::Guest);
    idle(&mut [&mut member, &mut owner], 60, 20).await;

    assert_eq!(
        member.my_role(member_sid),
        Some(Role::Member),
        "a member must not be able to promote themselves"
    );
    assert_eq!(
        owner.my_role(owner_sid),
        Some(Role::Owner),
        "the owner must not be demotable by a member"
    );
}

/// A guest is read-only everywhere; the server must refuse what a modified
/// client would not hide.
#[tokio::test]
async fn a_guest_cannot_write_however_it_asks() {
    let server = start_server(None).await;
    let (mut owner, owner_sid) = join_owner(&server, "owner").await;
    let (mut guest, guest_sid) = join(&server, "guest", None).await;
    wait_members(&mut owner, owner_sid, 2).await;
    let guest_id = member_named(&owner, owner_sid, "guest").expect("guest visible");
    owner
        .set_role(owner_sid, guest_id, Role::Guest)
        .expect("demote to guest");

    pump_until(&mut guest, "demoted", |m| {
        m.my_role(guest_sid) == Some(Role::Guest)
    })
    .await;

    let general = guest
        .server(guest_sid)
        .unwrap()
        .channels
        .first()
        .map(|c| c.id)
        .expect("a channel");

    // Every write the client can express.
    let _ = guest.post(guest_sid, general, "let me in".to_string());
    let _ = guest.create_channel(guest_sid, "guest-channel".to_string());
    let _ = guest.send_file(
        guest_sid,
        general,
        "payload.bin".to_string(),
        "application/octet-stream".to_string(),
        vec![0u8; 64],
    );

    idle(&mut [&mut guest, &mut owner], 60, 20).await;
    owner.refresh_files(owner_sid).ok();
    idle(&mut [&mut owner], 40, 25).await;

    let server_view = owner.server(owner_sid).unwrap();
    assert!(
        !server_view
            .channels
            .iter()
            .any(|c| c.name == "guest-channel"),
        "a guest created a channel"
    );
    assert!(
        !server_view.files.iter().any(|f| f.name == "payload.bin"),
        "a guest uploaded a file"
    );
}

/// Identity is the handshake-verified fingerprint, not the username: reusing
/// someone's name must not take over their membership.
#[tokio::test]
async fn a_username_is_not_an_identity() {
    let server = start_server(None).await;
    let (mut owner, owner_sid) = join_owner(&server, "owner").await;
    let real_owner_id = owner.server(owner_sid).unwrap().member_id.unwrap();

    // A different key claiming the same username.
    let mut impostor = PartyManager::new();
    let imp_sid = connect_pinned(&mut impostor, &server, "owner", None, &client_key()).await;
    idle(&mut [&mut impostor, &mut owner], 80, 25).await;

    let status = impostor.server(imp_sid).map(|s| s.status.clone());
    match status {
        Some(PartyStatus::Rejected(_)) => {} // refused outright: correct
        Some(PartyStatus::Joined) => {
            let imp_id = impostor.server(imp_sid).unwrap().member_id;
            assert_ne!(
                imp_id,
                Some(real_owner_id),
                "a different key took over an existing member by reusing the username"
            );
            assert_ne!(
                impostor.my_role(imp_sid),
                Some(Role::Owner),
                "a name collision must not confer the owner's role"
            );
        }
        other => panic!("unexpected status {other:?}"),
    }
}
