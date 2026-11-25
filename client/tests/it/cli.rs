//! The terminal client binary, run for real: its non-interactive modes, and
//! (Linux, `in_a_terminal`) the TUI itself started in a pty. The TUI's
//! behaviour is otherwise covered through `TuiApp` in the unit tests and
//! `tui.rs` here.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn client() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_p2pem-classic"));
    cmd.env("RUST_LOG", "info");
    cmd
}

/// Stop a long-running binary the way an operator does: SIGTERM on Unix, asserting
/// it shuts down cleanly (a killed process also never writes its coverage
/// profile, which is why `main.rs` used to read as untested). Windows has no
/// SIGTERM to send a console child, so there it is killed.
fn stop(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let sent = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .expect("run kill");
        assert!(sent.success(), "could not signal the child");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().expect("wait for the child") {
                assert!(
                    status.success(),
                    "SIGTERM did not shut it down cleanly: {status}"
                );
                return;
            }
            assert!(Instant::now() < deadline, "SIGTERM did not stop it");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
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
    // A token on the command line (meaningless to a relay, but it is what a
    // user mistypes) must not be echoed into the log.
    let secret = "relay-token-that-must-not-be-logged";
    let mut child = client()
        .args([
            "--relay-server",
            "--port",
            &port.to_string(),
            "--relay-token",
            secret,
        ])
        .env("RUST_LOG", "debug")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
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
    stop(&mut child);
    let output = child.wait_with_output().expect("relay output");
    assert!(
        answer.to_lowercase().contains("unknown relay token"),
        "not a relay answering: {answer}"
    );
    // TUI-24: a daemon that logs nowhere cannot be operated. The relay writes
    // to stderr (the TUI cannot — it owns the screen).
    let log = String::from_utf8_lossy(&output.stderr);
    assert!(
        log.contains(&format!("Relay server listening on port {port}")),
        "the relay logged nothing useful: {log:?}"
    );
    assert!(!log.contains(secret), "the relay token was logged: {log}");
}

/// The TUI started for real, in a real terminal: `script` (util-linux) gives
/// the binary a pty, so `main` → `tui::run` → raw mode, the alternate screen
/// and the event loop all run as a user's would, over a throwaway profile
/// (`HOME`/`XDG_*` point at a temp dir, never the developer's).
#[cfg(target_os = "linux")]
mod in_a_terminal {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};

    struct Tui {
        child: std::process::Child,
        stdin: std::process::ChildStdin,
        screen: Arc<Mutex<Vec<u8>>>,
    }

    impl Tui {
        fn start(home: &std::path::Path) -> Tui {
            let bin = env!("CARGO_BIN_EXE_p2pem-classic");
            let mut child = Command::new("script")
                .args([
                    "-qfec",
                    &format!("stty cols 160 rows 40; exec '{bin}'"),
                    "/dev/null",
                ])
                .env("HOME", home)
                .env("XDG_DATA_HOME", home.join("data"))
                .env("XDG_CONFIG_HOME", home.join("config"))
                .env("XDG_CACHE_HOME", home.join("cache"))
                .env("TERM", "xterm-256color")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("util-linux `script` is needed to give the TUI a terminal");
            let stdin = child.stdin.take().unwrap();
            let mut stdout = child.stdout.take().unwrap();
            let screen = Arc::new(Mutex::new(Vec::new()));
            let sink = screen.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = stdout.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    sink.lock().unwrap().extend_from_slice(&buf[..n]);
                }
            });
            Tui {
                child,
                stdin,
                screen,
            }
        }

        /// The terminal output so far, escape sequences and box drawing
        /// removed and whitespace collapsed: what a person would read.
        fn text(&self) -> String {
            let raw = String::from_utf8_lossy(&self.screen.lock().unwrap()).into_owned();
            let mut out = String::new();
            let mut chars = raw.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\u{1b}' {
                    if chars.peek() == Some(&'[') {
                        chars.next();
                        for c in chars.by_ref() {
                            if c.is_ascii_alphabetic() || c == '~' {
                                break;
                            }
                        }
                    }
                    out.push(' ');
                } else if ('\u{2500}'..='\u{257f}').contains(&c) || c.is_whitespace() {
                    out.push(' ');
                } else {
                    out.push(c);
                }
            }
            out.split_whitespace().collect::<Vec<_>>().join(" ")
        }

        fn raw_contains(&self, needle: &[u8]) -> bool {
            self.screen
                .lock()
                .unwrap()
                .windows(needle.len())
                .any(|w| w == needle)
        }

        fn wait_for(&self, what: &str) {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !self.text().contains(what) {
                assert!(
                    Instant::now() < deadline,
                    "never showed {what:?}; the screen read: {}",
                    self.text()
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        fn send(&mut self, bytes: &[u8]) {
            self.stdin.write_all(bytes).unwrap();
            self.stdin.flush().unwrap();
        }

        /// Ctrl+C asks, a second Ctrl+C confirms; then the process must end
        /// on its own, successfully.
        fn quit(mut self) -> Self {
            self.send(b"\x03");
            self.wait_for("Quit?");
            self.send(b"\x03");
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if let Some(status) = self.child.try_wait().unwrap() {
                    assert!(status.success(), "the TUI exited with {status}");
                    return self;
                }
                assert!(Instant::now() < deadline, "Ctrl+C twice did not quit");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// First launch: the set-password prompt, in a terminal put back the way
    /// it was found. Quitting there — before any identity exists on disk —
    /// must leave nothing behind that the next launch cannot read. It used to
    /// write `history.json.enc` under the never-saved identity's key, so the
    /// second launch (a new identity) opened a fresh install on "History
    /// unreadable — nothing is being saved".
    #[test]
    fn quitting_at_the_first_run_prompt_leaves_a_clean_profile() {
        let home = tempfile::tempdir().unwrap();
        let profile = home.path().join("data").join("encryptedmessenger");

        let first = Tui::start(home.path()).quit_after("Set a password");
        // Raw mode's screen and paste mode on, and both off again on exit.
        for seq in [
            &b"\x1b[?1049h"[..],
            b"\x1b[?1049l",
            b"\x1b[?2004h",
            b"\x1b[?2004l",
        ] {
            assert!(
                first.raw_contains(seq),
                "terminal sequence {:?} missing",
                String::from_utf8_lossy(seq)
            );
        }
        assert!(
            !profile.join("history.json.enc").exists(),
            "a history was written under a key no file keeps"
        );
        assert!(!profile.join("identity.json").exists());

        let second = Tui::start(home.path()).quit_after("Set a password");
        assert!(
            !second.text().contains("History unreadable"),
            "the second launch: {}",
            second.text()
        );
    }

    impl Tui {
        fn quit_after(self, what: &str) -> Self {
            self.wait_for(what);
            self.quit()
        }
    }
}
