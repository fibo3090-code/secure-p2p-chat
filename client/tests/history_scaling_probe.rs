//! A measurement, not an assertion: how does saving cost scale with history size?
//!
//! `save_encrypted` serialises the whole history to JSON, encrypts it as one
//! blob and rewrites the file. So the cost of saving one message is proportional
//! to every message that came before it, and the cost of a conversation's life
//! is quadratic. This prints the numbers so the decision about whether that
//! matters is made from data rather than from the shape of the code.
//!
//! Ignored by default — it is a probe, not a test. **Run it in release mode:**
//!
//! ```text
//! cargo test --release -p p2pem-classic --test history_scaling_probe -- --ignored --nocapture
//! ```
//!
//! `--release` is not a nicety here, it is the difference between the numbers
//! meaning something and being noise. The two things this loop spends its time
//! in are `serde_json` and ChaCha20-Poly1305, and both are among the most
//! inlining-sensitive code in the dependency tree: a debug build leaves every
//! serializer call and every quarter-round as a real function call with
//! overflow checks around it, which is comfortably an order of magnitude slower
//! and — worse for a *scaling* probe — slower by a different factor at each
//! size. The point of this file is that a product decision gets made from data.
//! Debug-build data would answer a question nobody asked.
//!
//! There is no timing assertion anywhere in here, so it cannot be flaky; the
//! only way it can mislead is by being read as if it described the shipped
//! build. Hence this paragraph.

use messenger_core::types::{Chat, ChatKind, Message, MessageContent, Transport};
use p2pem_classic::app::persistence::HistoryFile;
use std::time::Instant;
use uuid::Uuid;

fn chat_with(messages: usize) -> Chat {
    let mut chat = Chat {
        id: Uuid::new_v4(),
        title: "Probe".to_string(),
        kind: ChatKind::Dm,
        transport: Transport::Direct,
        peer_fingerprint: Some("FE".repeat(32)),
        participants: Vec::new(),
        messages: Vec::with_capacity(messages),
        created_at: chrono::Utc::now(),
        send_seq: 0,
        recv_seq: 0,
        peer_typing: false,
        typing_since: None,
        is_host_placeholder: false,
        read_count: 0,
        title_is_custom: false,
    };
    for i in 0..messages {
        chat.messages.push(Message {
            id: Uuid::new_v4(),
            from_me: i % 2 == 0,
            content: MessageContent::Text {
                // Roughly a realistic message length.
                text: format!("message {i}: {}", "lorem ipsum dolor sit amet ".repeat(4)),
            },
            timestamp: chrono::Utc::now(),
            delivered: false,
        });
    }
    chat
}

#[test]
#[ignore = "measurement probe, not an assertion"]
fn how_does_saving_scale_with_history_size() {
    let dir = tempfile::tempdir().expect("dir");
    let key = [7u8; 32];

    println!();
    // A number is only as good as the build that produced it, so the table says
    // which build it is rather than leaving the reader to remember.
    if cfg!(debug_assertions) {
        println!("  ⚠ DEBUG BUILD — these timings are not the shipped ones.");
        println!("    serde_json and ChaCha20-Poly1305 are both heavily inlined in release;");
        println!("    re-run with --release before making a decision from this table.");
    } else {
        println!("  release build");
    }
    println!();
    println!("  messages |  file size |  one save | cost of reaching it");
    println!("  ---------+------------+-----------+--------------------");

    for &n in &[100usize, 1_000, 5_000, 20_000, 50_000] {
        let history = HistoryFile::new(vec![chat_with(n)]);

        let path = dir.path().join(format!("history-{n}.enc"));
        // Warm: one save so the file exists.
        history.save_encrypted(&path, &key).expect("save");

        let start = Instant::now();
        const REPS: u32 = 5;
        for _ in 0..REPS {
            history.save_encrypted(&path, &key).expect("save");
        }
        let per_save = start.elapsed() / REPS;
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        // Every message written cost a full rewrite, so reaching n messages cost
        // roughly n/2 * per_save in total — the quadratic term.
        let cumulative = per_save.mul_f64(n as f64 / 2.0);

        println!(
            "  {:>8} | {:>7} KB | {:>7.1?} | {:>8.1?}",
            n,
            size / 1024,
            per_save,
            cumulative
        );
    }
    println!();
    println!("  'cost of reaching it' is the total time spent saving over the");
    println!("  life of a conversation that grew one message at a time.");
    println!();
}
