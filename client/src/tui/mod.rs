pub mod app;
pub mod command;
mod downloads;
pub mod input;
pub mod overlays;
pub mod ui;

use crate::logbuf::LogBuffer;
use crate::tui::app::{TuiApp, TuiCommand, TuiFocus, TuiMode};
use anyhow::Result;
use ratatui::prelude::*;
use ratatui_crossterm::crossterm::{
    cursor::Show,
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io;
use std::time::Duration;
use ui::ui;

#[derive(Debug, Clone)]
pub struct TuiLaunchConfig {
    pub host: bool,
    pub connect: Option<String>,
    pub host_relay: Option<String>,
    pub relay_connect: Option<String>,
    pub relay_token: Option<String>,
    pub port: u16,
}

/// Put the terminal back the way the shell expects it: cooked mode, main
/// screen, bracketed paste off, cursor visible. Best effort — every step runs
/// even if an earlier one fails, because a half-restored terminal is the
/// failure being prevented.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    );
}

/// Owns the terminal's raw/alternate-screen state for the life of the TUI and
/// restores it on drop — including the early returns. `TuiApp::new(logs)?` used
/// to return *after* raw mode and the alternate screen were entered, so a
/// startup error (an unreadable identity file is the realistic one) left the
/// shell in raw mode on a blank screen with the error invisible (audit TUI-06).
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let guard = TerminalGuard;
        // Bracketed paste makes a paste arrive as one `Event::Paste` instead of
        // keystrokes. Without it every pasted newline was an Enter: each line
        // was sent as a message, and a line starting with `:` ran as a command
        // (`:send ~/.ssh/id_rsa`) — audit TUI-05.
        //
        // Mouse capture is intentionally NOT enabled — we handle no mouse
        // events and capturing would block the terminal's native selection.
        execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Restore the terminal before the default panic message is printed, so a
/// panic is readable instead of being drawn into a raw alternate screen.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}

/// Main entry point for the TUI.
pub async fn run(logs: LogBuffer, launch: TuiLaunchConfig) -> Result<()> {
    install_panic_hook();
    let guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiApp::new(logs)?;
    app.prompt_auth_if_needed();
    start_launch(&mut app, launch).await;

    let res = run_app(&mut terminal, app, &mut CrosstermEvents).await;

    drop(guard);

    if let Err(err) = res {
        // The alternate screen is already torn down here, so stderr is the only
        // channel the user still sees; the tracing call keeps the structured
        // record for diagnostics exports.
        tracing::error!(error = ?err, "TUI exited with an error");
        eprintln!("The TUI exited with an error: {err:?}");
    }

    Ok(())
}

/// Run the launch options now, or — with the identity still locked — once it
/// is unlocked. Run before the unlock they all failed on the locked key and
/// were lost: every launch after the first, since the first sets a password
/// (TUI-10).
async fn start_launch(app: &mut TuiApp, launch: TuiLaunchConfig) {
    let any = launch.host
        || launch.connect.is_some()
        || launch.host_relay.is_some()
        || launch.relay_connect.is_some();
    if !any {
        return;
    }
    if app.identity_locked() {
        app.deferred_launch = Some(launch);
    } else {
        apply_launch_config(app, &launch).await;
    }
}

/// The deferred half of [`start_launch`]: called each turn of the loop.
async fn run_deferred_launch(app: &mut TuiApp) {
    if app.identity_locked() {
        return;
    }
    if let Some(launch) = app.deferred_launch.take() {
        apply_launch_config(app, &launch).await;
    }
}

async fn apply_launch_config(app: &mut TuiApp, launch: &TuiLaunchConfig) {
    if launch.host {
        app.execute_command(TuiCommand::Host(Some(launch.port)))
            .await;
    }

    if let Some(target) = launch.connect.as_deref() {
        // `--port` is the default for a target that names none: `--connect
        // 1.2.3.4 --port 9000` dialled 12345 (TUI-10).
        match crate::util::parse_host_port(target.trim(), Some(launch.port)) {
            Ok((host, port)) if !target.trim().contains(char::is_whitespace) => {
                app.execute_command(TuiCommand::Connect { host, port })
                    .await
            }
            Ok(_) => app.chat_manager.add_toast(
                crate::types::ToastLevel::Error,
                format!("Invalid --connect: '{target}' is not one address"),
            ),
            Err(e) => app.chat_manager.add_toast(
                crate::types::ToastLevel::Error,
                format!("Invalid --connect: {}", e),
            ),
        }
    }

    if let Some(relay) = launch.host_relay.as_deref() {
        let cmd = match launch.relay_token.as_deref() {
            Some(token) => format!(":host-relay {} {}", relay, token),
            None => format!(":host-relay {}", relay),
        };
        match TuiApp::parse_command(&cmd) {
            Ok(cmd @ TuiCommand::HostRelay { .. }) => app.execute_command(cmd).await,
            Ok(_) => {}
            Err(e) => app.chat_manager.add_toast(
                crate::types::ToastLevel::Error,
                format!("Invalid relay host launch: {}", e),
            ),
        }
    }

    if let Some(relay) = launch.relay_connect.as_deref() {
        match launch.relay_token.as_deref() {
            Some(token) => {
                match TuiApp::parse_command(&format!(":connect-relay {} {}", relay, token)) {
                    Ok(cmd @ TuiCommand::ConnectRelay { .. }) => app.execute_command(cmd).await,
                    Ok(_) => {}
                    Err(e) => app.chat_manager.add_toast(
                        crate::types::ToastLevel::Error,
                        format!("Invalid relay connect launch: {}", e),
                    ),
                }
            }
            None => app.chat_manager.add_toast(
                crate::types::ToastLevel::Error,
                "--connect-relay requires --relay-token <token>".to_string(),
            ),
        }
    }
}

/// Place the hardware cursor in the input/command box using the editable field's
/// own cursor position, reusing the shared layout so it never drifts from the UI.
fn set_cursor_position<B: Backend>(terminal: &mut Terminal<B>, app: &TuiApp) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    let size: Rect = terminal.size()?.into();
    let input_area = ui::regions(size).input;

    let show_cursor =
        !app.overlay.is_open() && (app.mode == TuiMode::Command || app.focus == TuiFocus::Input);
    if !show_cursor {
        terminal.hide_cursor()?;
        return Ok(());
    }

    let (field, prefix) = if app.mode == TuiMode::Command {
        (&app.command_field, 1u16) // account for the leading ':'
    } else {
        (&app.input_field, 0u16)
    };
    let (row, col) = field.cursor_display();

    let max_inner_rows = input_area.height.saturating_sub(2);
    let max_inner_cols = input_area.width.saturating_sub(2);
    let y = input_area.y + 1 + row.min(max_inner_rows.saturating_sub(1));
    let x = input_area.x + 1 + (col + prefix).min(max_inner_cols.saturating_sub(1));

    terminal.show_cursor()?;
    terminal.set_cursor_position((x, y))?;
    Ok(())
}

/// Where terminal input comes from. The only part of the loop that needs a
/// real TTY is crossterm's global event queue, so it sits behind this and the
/// loop itself — key filtering, paste, quit-and-save, cursor placement — runs
/// in tests against a `TestBackend` and a script.
trait EventSource {
    /// The next event, or `None` if nothing arrived within `timeout`.
    fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>>;
}

struct CrosstermEvents;

impl EventSource for CrosstermEvents {
    fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
        Ok(if event::poll(timeout)? {
            Some(event::read()?)
        } else {
            None
        })
    }
}

/// Main application loop.
async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    mut app: TuiApp,
    events: &mut impl EventSource,
) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    loop {
        app.tick();
        run_deferred_launch(&mut app).await;

        if let Some(cmd) = app.take_pending_command() {
            app.execute_command(cmd).await;
        }

        terminal.draw(|f| ui(f, &mut app))?;
        set_cursor_position(terminal, &app)?;

        if app.should_quit {
            app.shutdown_save();
            return Ok(());
        }

        // Wait for input, then take everything else already waiting before
        // the next frame (TUI-14): one tick and one redraw per *key* made a
        // fast typist, a held key or a paste delivered as keys crawl. Capped so
        // a flood still repaints, and stopped at a submitted command, which
        // has to run before more input — the app holds only one.
        let mut next = events.next_event(Duration::from_millis(100))?;
        let mut handled = 0;
        while let Some(event) = next {
            // On Windows, crossterm reports both key-press and key-release events;
            // only act on presses so each keystroke registers once.
            match event {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key_event(key),
                // A paste is text, never keystrokes (see TerminalGuard).
                Event::Paste(text) => app.handle_paste(&text),
                _ => {}
            }
            if app.should_quit {
                app.shutdown_save();
                return Ok(());
            }
            handled += 1;
            if handled >= MAX_EVENTS_PER_FRAME || app.has_pending_command() {
                break;
            }
            next = events.next_event(Duration::ZERO)?;
        }
    }
}

/// Input handled between two frames at most; see `run_app`.
const MAX_EVENTS_PER_FRAME: usize = 512;

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui_crossterm::crossterm::event::{KeyCode, KeyEvent, KeyEventState, KeyModifiers};
    use std::collections::VecDeque;

    /// A scripted terminal. When the script runs out the loop is ended with an
    /// error, so a test that forgets to quit fails instead of hanging.
    /// The third field counts *blocking* polls — the one wait per frame — so
    /// a test can see how many frames an input took.
    struct Script(VecDeque<Event>, VecDeque<Box<dyn FnOnce()>>, usize);

    impl Script {
        fn new(events: impl IntoIterator<Item = Event>) -> Self {
            Script(events.into_iter().collect(), VecDeque::new(), 0)
        }

        /// Run `f` (between frames, as an idle poll) before the first event.
        fn first(mut self, f: impl FnOnce() + 'static) -> Self {
            self.1.push_back(Box::new(f));
            self
        }
    }

    impl EventSource for Script {
        fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>> {
            if timeout.is_zero() {
                // "Anything else already waiting?" — a script with nothing
                // left simply has nothing waiting.
                return Ok(self.0.pop_front());
            }
            self.2 += 1;
            if let Some(f) = self.1.pop_front() {
                f();
                return Ok(None);
            }
            self.0
                .pop_front()
                .map(Some)
                .ok_or_else(|| anyhow::anyhow!("script ran out"))
        }
    }

    fn press(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    fn release(c: char) -> Event {
        Event::Key(KeyEvent {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        })
    }

    fn enter() -> Event {
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    }

    fn app_in(dir: &tempfile::TempDir) -> TuiApp {
        TuiApp::new_in(Some(dir.path().to_path_buf()), LogBuffer::new()).expect("app")
    }

    fn screen(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `:q` then `y`, through the real loop: the history is written on the way
    /// out. Quitting is the one save a user counts on, and the loop is where
    /// it happens.
    #[tokio::test]
    async fn quitting_through_the_loop_saves_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        let chat = uuid::Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(chat, "Kept".into());
        let history = dir.path().join("history.json.enc");
        assert!(!history.exists());
        let key = app
            .chat_manager
            .history_key
            .expect("an unlocked app has a key");

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut script = Script::new([press(':'), press('q'), enter(), press('y')]);
        run_app(&mut terminal, app, &mut script)
            .await
            .expect("the loop ended by quitting, not by running out of input");
        assert!(script.0.is_empty());

        // Read it back with the key the app was using. (A fresh identity has
        // no password and is never written, so a relaunch here would be a new
        // identity — that rule is tested where it lives.)
        let mut reader = crate::app::ChatManager::default();
        assert!(reader.load_history_auto(&history, &key).expect("readable"));
        assert!(
            reader.chats.contains_key(&chat),
            "the conversation did not survive the quit"
        );
    }

    /// TUI-14: the loop handled one event per frame, so a burst of input —
    /// a fast typist, a key held down, a paste the terminal delivers as keys —
    /// cost a full tick and a full redraw *per key*. Everything already
    /// waiting is handled before the next frame.
    #[tokio::test]
    async fn a_burst_of_keys_is_handled_in_one_frame() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        app.chat_manager
            .create_local_chat_for_test(uuid::Uuid::new_v4(), "Kept".into());
        let key = app.chat_manager.history_key.expect("key");
        assert!(app.chat_manager.config.enable_notifications);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        // Two commands typed in one burst. Draining must stop at the first
        // Enter: the app holds one pending command, and a second submitted in
        // the same frame would silently replace it.
        let mut events = vec![press(':')];
        events.extend("set notifications off".chars().map(press));
        events.push(enter());
        events.extend([press(':'), press('q'), press('!'), enter()]);
        let keys = events.len();
        let mut script = Script::new(events);
        let history = dir.path().join("history.json.enc");
        run_app(&mut terminal, app, &mut script)
            .await
            .expect("the loop ended by quitting");
        assert!(script.0.is_empty());
        assert!(script.2 <= 3, "{keys} keys took {} frames", script.2);
        // The first command ran: its setting is in the saved history.
        let mut reader = crate::app::ChatManager::default();
        assert!(reader.load_history_auto(&history, &key).expect("readable"));
        assert!(
            !reader.config.enable_notifications,
            "the first command of the burst was lost"
        );
    }

    /// `:q!` skips the confirmation, and with it the confirmation's own save:
    /// the loop's save on the way out is the only one it gets.
    #[tokio::test]
    async fn force_quitting_still_saves_the_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        let chat = uuid::Uuid::new_v4();
        app.chat_manager
            .create_local_chat_for_test(chat, "Kept".into());
        let key = app.chat_manager.history_key.expect("key");
        let history = dir.path().join("history.json.enc");
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        // The first frame autosaves in the background. Let that land and take
        // it away, so the file can only come back from the save on the way out.
        let autosaved = history.clone();
        let mut script =
            Script::new([press(':'), press('q'), press('!'), enter()]).first(move || {
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                while !autosaved.exists() {
                    assert!(std::time::Instant::now() < deadline, "no autosave");
                    std::thread::sleep(Duration::from_millis(10));
                }
                // Written via temp + rename, so present means complete.
                std::fs::remove_file(&autosaved).unwrap();
            });
        run_app(&mut terminal, app, &mut script)
            .await
            .expect("quit");

        let mut reader = crate::app::ChatManager::default();
        assert!(
            reader.load_history_auto(&history, &key).expect("readable"),
            "nothing was saved on the way out"
        );
        assert!(reader.chats.contains_key(&chat));
    }

    #[tokio::test]
    async fn declining_the_quit_prompt_keeps_running() {
        let dir = tempfile::tempdir().unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut script = Script::new([press(':'), press('q'), enter(), press('n')]);
        let err = run_app(&mut terminal, app_in(&dir), &mut script)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("script ran out"), "{err}");
    }

    /// Windows reports a key's release as well as its press. Acting on both
    /// typed every character twice.
    #[tokio::test]
    async fn a_key_release_is_not_a_second_keystroke() {
        let dir = tempfile::tempdir().unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut script = Script::new([
            press(':'),
            release(':'),
            press('a'),
            release('a'),
            press('b'),
            release('b'),
        ]);
        let _ = run_app(&mut terminal, app_in(&dir), &mut script).await;
        let shown = screen(&terminal);
        assert!(shown.contains(":ab"), "{shown}");
        assert!(!shown.contains("aa") && !shown.contains("::"), "{shown}");

        // The hardware cursor follows the edit: just after ":ab", inside the box.
        let input = ui::regions(Rect::new(0, 0, 100, 30)).input;
        assert!(terminal.backend().cursor_visible());
        assert_eq!(
            terminal.backend().cursor_position(),
            Position::new(input.x + 1 + 3, input.y + 1)
        );
    }

    /// A paste arrives as one event and is inserted as text. Delivered as
    /// keystrokes, each newline was an Enter: every line was sent, and a line
    /// starting with ':' ran as a command.
    #[tokio::test]
    async fn a_pasted_command_is_text_not_a_command() {
        let dir = tempfile::tempdir().unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut script = Script::new([Event::Paste(":quit!\n".into())]);
        let err = run_app(&mut terminal, app_in(&dir), &mut script)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("script ran out"),
            "a pasted ':quit!' quit the app: {err}"
        );
    }

    #[tokio::test]
    async fn the_cursor_is_hidden_while_an_overlay_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        // `:q` opens the quit confirmation; the script ends there.
        let mut script = Script::new([press(':'), press('q'), enter()]);
        let _ = run_app(&mut terminal, app_in(&dir), &mut script).await;
        assert!(!terminal.backend().cursor_visible());
    }

    fn launch() -> TuiLaunchConfig {
        TuiLaunchConfig {
            host: false,
            connect: None,
            host_relay: None,
            relay_connect: None,
            relay_token: None,
            port: 0,
        }
    }

    fn toasts(app: &TuiApp) -> Vec<String> {
        app.chat_manager
            .toasts
            .iter()
            .map(|t| t.message.clone())
            .collect()
    }

    #[tokio::test]
    async fn launch_flags_host_and_report_what_they_could_not_do() {
        let dir = tempfile::tempdir().unwrap();

        // --host --port 0: listening, on the port the OS gave it.
        let mut app = app_in(&dir);
        apply_launch_config(
            &mut app,
            &TuiLaunchConfig {
                host: true,
                ..launch()
            },
        )
        .await;
        assert!(
            app.chat_manager.hosting_port.is_some_and(|p| p != 0),
            "{:?}",
            toasts(&app)
        );

        // A malformed target is said, not silently ignored.
        let mut app = app_in(&dir);
        apply_launch_config(
            &mut app,
            &TuiLaunchConfig {
                connect: Some("not a host:port:at all".into()),
                ..launch()
            },
        )
        .await;
        assert!(
            toasts(&app).iter().any(|t| t.contains("Invalid --connect")),
            "{:?}",
            toasts(&app)
        );

        // --connect-relay needs a token; without one nothing is dialled.
        let mut app = app_in(&dir);
        apply_launch_config(
            &mut app,
            &TuiLaunchConfig {
                relay_connect: Some("127.0.0.1:1".into()),
                ..launch()
            },
        )
        .await;
        assert!(
            toasts(&app)
                .iter()
                .any(|t| t.contains("requires --relay-token")),
            "{:?}",
            toasts(&app)
        );
        assert!(app.chat_manager.chats.is_empty());
    }

    /// TUI-10 (a): after the first run the identity is locked at launch, and
    /// the flags ran before the unlock prompt was answered — failing on the
    /// locked key and lost. They wait for the unlock now.
    #[tokio::test]
    async fn launch_flags_wait_for_the_unlock() {
        let dir = tempfile::tempdir().unwrap();
        let password = format!("pw-{}", uuid::Uuid::new_v4());
        let mut identity =
            messenger_core::identity::Identity::new_with_plaintext("T".into()).unwrap();
        identity.encrypt(&password).unwrap();
        identity.save(&dir.path().join("identity.json")).unwrap();
        let mut app = app_in(&dir);
        assert!(app.identity_locked());

        start_launch(
            &mut app,
            TuiLaunchConfig {
                host: true,
                ..launch()
            },
        )
        .await;
        assert!(
            app.chat_manager.hosting_port.is_none(),
            "hosted on a locked identity"
        );
        run_deferred_launch(&mut app).await;
        assert!(
            app.chat_manager.hosting_port.is_none(),
            "ran before the unlock"
        );

        app.execute_command(crate::tui::app::TuiCommand::Unlock(Some(password)))
            .await;
        assert!(!app.identity_locked());
        run_deferred_launch(&mut app).await;
        assert!(
            app.chat_manager.hosting_port.is_some(),
            "{:?}",
            toasts(&app)
        );
        assert!(app.deferred_launch.is_none(), "it runs once");
    }

    /// TUI-10 (b): `--port` is the port `--connect` uses when the target names
    /// none; it used to dial 12345 whatever `--port` said.
    #[tokio::test]
    async fn connect_uses_the_port_flag_when_the_target_names_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_in(&dir);
        apply_launch_config(
            &mut app,
            &TuiLaunchConfig {
                connect: Some("127.0.0.1".into()),
                port: 9,
                ..launch()
            },
        )
        .await;
        assert!(
            toasts(&app).iter().any(|t| t.contains("127.0.0.1:9")),
            "{:?}",
            toasts(&app)
        );
    }

    #[tokio::test]
    async fn launch_flags_reach_a_relay() {
        use messenger_core::network::{generate_relay_token, serve_relay, RELAY_WAIT_TIMEOUT};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay = listener.local_addr().unwrap().to_string();
        tokio::spawn(serve_relay(listener, RELAY_WAIT_TIMEOUT));
        let token = generate_relay_token();

        let host_dir = tempfile::tempdir().unwrap();
        let mut host = app_in(&host_dir);
        apply_launch_config(
            &mut host,
            &TuiLaunchConfig {
                host_relay: Some(relay.clone()),
                relay_token: Some(token.clone()),
                ..launch()
            },
        )
        .await;
        let client_dir = tempfile::tempdir().unwrap();
        let mut client = app_in(&client_dir);
        apply_launch_config(
            &mut client,
            &TuiLaunchConfig {
                relay_connect: Some(relay),
                relay_token: Some(token),
                ..launch()
            },
        )
        .await;

        // Both reach the safety-code prompt: the flags dialled, through the
        // relay, with the token given.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            host.tick();
            client.tick();
            if host.chat_manager.pending_fingerprint().is_some()
                && client.chat_manager.pending_fingerprint().is_some()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "host {:?} / client {:?}",
                toasts(&host),
                toasts(&client)
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
