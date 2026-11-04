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
    apply_launch_config(&mut app, &launch).await;

    let res = run_app(&mut terminal, app).await;

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

async fn apply_launch_config(app: &mut TuiApp, launch: &TuiLaunchConfig) {
    if launch.host {
        app.execute_command(TuiCommand::Host(Some(launch.port)))
            .await;
    }

    if let Some(target) = launch.connect.as_deref() {
        match TuiApp::parse_command(&format!(":connect {}", target)) {
            Ok(cmd @ TuiCommand::Connect { .. }) => app.execute_command(cmd).await,
            Ok(_) => {}
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
fn set_cursor_position(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &TuiApp,
) -> Result<()> {
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

/// Main application loop.
async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut app: TuiApp,
) -> Result<()> {
    loop {
        app.tick();

        if let Some(cmd) = app.take_pending_command() {
            app.execute_command(cmd).await;
        }

        terminal.draw(|f| ui(f, &mut app))?;
        set_cursor_position(terminal, &app)?;

        if app.should_quit {
            app.shutdown_save();
            return Ok(());
        }

        if event::poll(Duration::from_millis(100))? {
            // On Windows, crossterm reports both key-press and key-release events;
            // only act on presses so each keystroke registers once.
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key_event(key),
                // A paste is text, never keystrokes (see TerminalGuard).
                Event::Paste(text) => app.handle_paste(&text),
                _ => {}
            }
            if app.should_quit {
                app.shutdown_save();
                return Ok(());
            }
        }
    }
}
