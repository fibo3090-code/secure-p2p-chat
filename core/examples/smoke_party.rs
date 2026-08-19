//! Throwaway harness: drive a real Party server over a real socket.
//!
//! Point it at a running `p2pem-server` and it performs the full v3 handshake,
//! joins, posts, and reads history back. Used to check a *released* server
//! against a locally-built client, which is the only way to exercise the
//! cross-version path — a released binary cannot be linked into a unit test.
//!
//! Usage: `cargo run -p messenger-core --example smoke_party -- 127.0.0.1:21345`

use messenger_core::core::generate_rsa_keypair;
use messenger_core::party::{PartyClient, PartyRequest, PartyResponse};
use messenger_core::RSA_KEY_BITS;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:21345".to_string());
    let username = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "smoke".to_string());

    let privkey = generate_rsa_keypair(RSA_KEY_BITS)?;
    let stream = tokio::net::TcpStream::connect(&addr).await?;
    let mut client = PartyClient::connect(stream, &privkey, uuid::Uuid::new_v4()).await?;
    println!("handshake ok");
    println!("  server fingerprint: {}", client.server_fingerprint());
    println!("  SAS:                {}", client.sas());

    client
        .send(&PartyRequest::Join {
            username: username.clone(),
            password: None,
        })
        .await?;
    let member_id = match client.recv().await? {
        PartyResponse::Joined {
            member_id,
            server_name,
            ..
        } => {
            println!("joined '{server_name}' as {username} ({member_id})");
            member_id
        }
        other => anyhow::bail!("join failed: {other:?}"),
    };

    client.send(&PartyRequest::ListChannels).await?;
    let channel = loop {
        match client.recv().await? {
            PartyResponse::Channels(list) => {
                println!(
                    "channels: {:?}",
                    list.iter().map(|c| &c.name).collect::<Vec<_>>()
                );
                break list.first().map(|c| c.id).expect("a default channel");
            }
            other => println!("  (push: {other:?})"),
        }
    };

    let text = format!("smoke test from {member_id}");
    client
        .send(&PartyRequest::PostMessage {
            channel,
            text: text.clone(),
        })
        .await?;
    loop {
        match client.recv().await? {
            PartyResponse::MessagePosted { seq, .. } => {
                println!("posted, seq {seq}");
                break;
            }
            PartyResponse::ActionFailed { reason, .. } => anyhow::bail!("post refused: {reason}"),
            other => println!("  (push: {other:?})"),
        }
    }

    client
        .send(&PartyRequest::FetchHistory {
            channel,
            since_seq: 0,
        })
        .await?;
    loop {
        match client.recv().await? {
            PartyResponse::History(items) => {
                println!("history: {} message(s)", items.len());
                anyhow::ensure!(
                    items.iter().any(|e| matches!(
                        &e.payload,
                        messenger_core::party::MessagePayload::Text(t) if *t == text
                    )),
                    "the posted message is missing from history"
                );
                break;
            }
            other => println!("  (push: {other:?})"),
        }
    }

    println!("SMOKE OK");
    Ok(())
}
