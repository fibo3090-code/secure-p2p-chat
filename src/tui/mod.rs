pub mod app;
pub mod ui;

use crate::tui::app::{TuiApp, TuiCommand, TuiFocus, TuiMode};
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use egui_tracing::tracing::EventCollector;
use ratatui::prelude::*;
use std::io;
use std::time::Duration;
use ui::ui;

#[derive(Debug, Clone)]
pub struct TuiLaunchConfig {
    pub host: bool,
    pub connect: Option<String>,
    pub port: u16,
}

/// Main entry point for the TUI.
pub async fn run(event_collector: EventCollector, launch: TuiLaunchConfig) -> Result<()> {
    enable_raw_mode()?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiApp::new(event_collector)?;
    apply_launch_config(&mut app, &launch).await;

    let res = run_app(&mut terminal, app).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err)
    }

    Ok(())
}

async fn apply_launch_config(app: &mut TuiApp, launch: &TuiLaunchConfig) {
    if launch.host {
        app.execute_command(TuiCommand::Host(Some(launch.port)))
            .await;
    }

    if let Some(target) = launch.connect.as_deref() {
        let cmd = format!(":connect {}", target);
        match TuiApp::parse_command(&cmd) {
            Ok(connect_cmd @ TuiCommand::Connect { .. }) => {
                app.execute_command(connect_cmd).await;
            }
            Ok(_) => {}
            Err(e) => {
                app.chat_manager.add_toast(
                    crate::types::ToastLevel::Error,
                    format!("Invalid --connect: {}", e),
                );
            }
        }
    }
}

fn set_cursor_position(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &TuiApp,
) -> Result<()> {
    let size: Rect = terminal.size()?.into();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)].as_ref())
        .split(size);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
        .split(vertical[0]);

    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(5)].as_ref())
        .split(columns[1]);

    let input_area = right_chunks[1];

    let show_cursor = app.focus == TuiFocus::Input || app.mode == TuiMode::Command;
    if !show_cursor {
        terminal.hide_cursor()?;
        return Ok(());
    }

    terminal.show_cursor()?;

    let text = if app.mode == TuiMode::Command {
        app.command_buffer.as_str()
    } else {
        app.input_text.as_str()
    };
    let lines: Vec<&str> = text.split('\n').collect();
    let last_line = lines.last().copied().unwrap_or("");
    let row = lines.len().saturating_sub(1) as u16;
    let max_inner_rows = input_area.height.saturating_sub(2);
    let y = input_area.y + 1 + row.min(max_inner_rows.saturating_sub(1));

    let col = last_line.chars().count() as u16;
    let max_inner_cols = input_area.width.saturating_sub(2);
    let x = input_area.x + 1 + col.min(max_inner_cols.saturating_sub(1));

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

        terminal.draw(|f| {
            ui(f, &mut app);
        })?;

        set_cursor_position(terminal, &app)?;

        if app.should_quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => app.handle_key_event(key),
                Event::Resize(_, _) => {}
                _ => {}
            }

            if app.should_quit {
                return Ok(());
            }
        }
    }
}
