//! The community server binary's start-up rules, exercised on the real
//! binary: they live in `main`, so nothing in-process reaches them.
//!
//! - It refuses to start without a password unless `--open` says so: an
//!   operator who forgot the flag must not be serving an open community.
//! - `--owner` must be a whole fingerprint: a typo would otherwise configure
//!   an owner nobody can ever be.
//! - Started, it prints the fingerprint members verify and, while nobody owns
//!   the community, the one-time owner claim token.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn server() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_server"));
    cmd.env_remove("PARTY_PASSWORD")
        .env_remove("PARTY_OPEN")
        .env_remove("PARTY_OWNER")
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1");
    cmd
}

fn run_to_exit(cmd: &mut Command) -> (bool, String) {
    let out = cmd.output().expect("run the server binary");
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.success(), text)
}

#[test]
fn it_refuses_to_start_without_a_password_unless_told_it_is_open() {
    let dir = tempfile::tempdir().unwrap();
    let (ok, text) = run_to_exit(server().args(["--port", "0", "--data-dir"]).arg(dir.path()));
    assert!(!ok, "an unprotected server started");
    assert!(
        text.contains("refusing to start without a password"),
        "{text}"
    );
    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "a refused start wrote state"
    );
}

#[test]
fn an_owner_that_is_not_a_whole_fingerprint_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    for bad in ["abc", &"g".repeat(64), &"a".repeat(63)] {
        let (ok, text) = run_to_exit(
            server()
                .args(["--port", "0", "--open", "--owner", bad, "--data-dir"])
                .arg(dir.path()),
        );
        assert!(!ok, "--owner {bad:?} was accepted");
        assert!(
            text.contains("--owner must be a 64-character hex fingerprint"),
            "{text}"
        );
    }
}

#[test]
fn an_open_server_starts_and_prints_its_fingerprint_and_claim_token() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = server()
        .args(["--port", "0", "--open", "--data-dir"])
        .arg(dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the server");
    let out = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let (mut fingerprint, mut claim, mut open_warning) = (false, false, false);
    while Instant::now() < deadline && !(fingerprint && claim && open_warning) {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            fingerprint |= line.contains("fingerprint=");
            claim |= line.contains("owner_claim_token=");
            open_warning |= line.contains("this server is OPEN");
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        fingerprint,
        "the fingerprint members verify was not printed"
    );
    assert!(claim, "an unowned community printed no claim token");
    assert!(open_warning, "an open server did not say so");
    assert!(
        dir.path().join("server_identity.pem").exists(),
        "the identity members pin was not persisted"
    );
}
