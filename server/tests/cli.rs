//! The community server binary's start-up rules, exercised on the real
//! binary: they live in `main`, so nothing in-process reaches them.
//!
//! - It refuses to start without a password unless `--open` says so: an
//!   operator who forgot the flag must not be serving an open community.
//! - `--owner` must be a whole fingerprint: a typo would otherwise configure
//!   an owner nobody can ever be.
//! - Started, it prints the fingerprint members verify and, while nobody owns
//!   the community, the one-time owner claim token.
//! - `--retain-days` prunes at start-up, not only an hour later: the
//!   retention task's first tick is immediate.

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

/// Stop a server binary the way an operator does: SIGTERM on Unix, asserting
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
    stop(&mut child);
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

/// Count the messages `party.db` holds for any channel.
fn stored_messages(dir: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(dir.join("party.db")).unwrap();
    conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap()
}

/// SRV-06 retention, wired end to end: history older than `--retain-days` is
/// gone as soon as the server starts (an operator who sets a limit expects it
/// to hold now, not after the first hourly tick), the newest message of the
/// channel is kept, and nothing younger is touched. The pruning rules are
/// unit-tested in `state`; this is the part only `main` does.
#[test]
fn retention_prunes_old_history_at_start_up() {
    use messenger_server::state::PartyState;
    let dir = tempfile::tempdir().unwrap();
    {
        let mut state = PartyState::load("Retained", None, dir.path()).unwrap();
        let alice = state.join("alice", None, None).unwrap();
        let channel = state.default_channel();
        for text in ["old one", "old two", "recent", "newest"] {
            state.post_message(alice, channel, text.into()).unwrap();
        }
    }
    // Backdate the first two by three days.
    let three_days_ago = messenger_core::util::current_timestamp_millis() as i64 - 3 * 86_400_000;
    let conn = rusqlite::Connection::open(dir.path().join("party.db")).unwrap();
    let changed = conn
        .execute(
            "UPDATE messages SET envelope = json_set(envelope, '$.timestamp', ?1)
              WHERE json_extract(envelope, '$.payload.Text') IN ('old one', 'old two')",
            [three_days_ago],
        )
        .unwrap();
    drop(conn);
    assert_eq!(changed, 2, "the fixture did not backdate two messages");
    assert_eq!(stored_messages(dir.path()), 4);

    let mut child = server()
        .args(["--port", "0", "--open", "--retain-days", "1", "--data-dir"])
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
    let mut pruned = None;
    while Instant::now() < deadline && pruned.is_none() {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            if line.contains("retention: pruned expired history") {
                pruned = Some(line);
            }
        }
    }
    stop(&mut child);
    let pruned = pruned.expect("no pruning at start-up");
    assert!(pruned.contains("messages=2"), "{pruned}");
    assert_eq!(
        stored_messages(dir.path()),
        2,
        "the recent history was not kept"
    );
}
