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

/// Put the terminal back the way the shell expects it. Best effort: every step
/// runs even if an earlier one fails.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    );
}

/// Owns the raw/alternate-screen state and restores it on drop, including on
/// early returns: a startup error must not leave the shell in raw mode on a
/// blank screen (audit TUI-06).
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let guard = TerminalGuard;
        // Bracketed paste delivers a paste as one `Event::Paste`, so a pasted
        // newline is not Enter and a pasted `:` line is not a command (audit
        // TUI-05). No mouse capture: it would block native text selection.
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
        // The screen is restored, so stderr reaches the user; tracing keeps
        // the record for diagnostics exports.
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
        launch_command(app, format!(":connect {}", target), "--connect").await;
    }
    if let Some(relay) = launch.host_relay.as_deref() {
        let cmd = match launch.relay_token.as_deref() {
            Some(token) => format!(":host-relay {} {}", relay, token),
            None => format!(":host-relay {}", relay),
        };
        launch_command(app, cmd, "relay host launch").await;
    }
    if let Some(relay) = launch.relay_connect.as_deref() {
        match launch.relay_token.as_deref() {
            Some(token) => {
                let cmd = format!(":connect-relay {} {}", relay, token);
                launch_command(app, cmd, "relay connect launch").await;
            }
            None => launch_error(app, "--connect-relay requires --relay-token <token>".into()),
        }
    }
}

/// Run a command built from launch flags (its command word is fixed, so it
/// parses to that command or to an error).
async fn launch_command(app: &mut TuiApp, raw: String, what: &str) {
    match TuiApp::parse_command(&raw) {
        Ok(cmd) => app.execute_command(cmd).await,
        Err(e) => launch_error(app, format!("Invalid {}: {}", what, e)),
    }
}

fn launch_error(app: &mut TuiApp, msg: String) {
    app.chat_manager
        .add_toast(crate::types::ToastLevel::Error, msg);
}

/// Place the hardware cursor in the input/command box, using the shared layout
/// so it never drifts from the UI.
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
            // Windows also reports key releases; act on presses only.
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
