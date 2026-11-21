//! The terminal client binary's non-interactive modes, on the real binary.
//! (The TUI itself needs a terminal; its behaviour is covered through
//! `TuiApp` in the unit tests and `tui.rs` here.)

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn client() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_p2pem-classic"));
    cmd.env("RUST_LOG", "info");
    cmd
}

/// The retired `--gui` flag fails with directions to the desktop app rather
/// than silently starting a different interface than the one asked for.
#[test]
fn the_retired_gui_flag_points_to_the_desktop_app() {
    let out = client()
        .arg("--gui")
        .output()
        .expect("run the client binary");
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("`--gui` has been retired"), "{text}");
    assert!(text.contains("releases/latest"), "{text}");
}

/// `--relay-server --port N` serves a real rendezvous: asked for a token
/// nobody hosts, it answers "unknown" — which only a live relay can.
#[tokio::test]
async fn relay_server_mode_serves_a_rendezvous() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = client()
        .args(["--relay-server", "--port", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the relay");
    let relay = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(30);
    let answer = loop {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (_out, out_rx) = tokio::sync::mpsc::unbounded_channel();
        let (_file, file_rx) = tokio::sync::mpsc::channel(1);
        let (_confirm, confirm_rx) = tokio::sync::mpsc::unbounded_channel();
        let result = messenger_core::network::run_client_session_via_relay(
            &relay,
            &messenger_core::network::generate_relay_token(),
            crate::common::rsa_key(),
            tx,
            out_rx,
            file_rx,
            confirm_rx,
            uuid::Uuid::new_v4(),
            None,
        )
        .await;
        let err = result
            .expect_err("an unhosted token must not pair")
            .to_string();
        if !err.to_lowercase().contains("connection refused") || Instant::now() > deadline {
            break err;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        answer.to_lowercase().contains("unknown relay token"),
        "not a relay answering: {answer}"
    );
}
