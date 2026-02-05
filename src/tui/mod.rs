pub mod app;
pub mod ui;

use crate::tui::app::TuiApp;
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use egui_tracing::tracing::EventCollector;
use ratatui::prelude::*;
use std::io;
use std::time::Duration;
use ui::ui;

/// Main entry point for the TUI.
pub async fn run(event_collector: EventCollector) -> Result<()> {
    // Setup terminal
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    enable_raw_mode()?;

    // Create app and run the main loop
    let app = TuiApp::new(event_collector)?;
    let res = run_app(&mut terminal, app).await;

    // Restore terminal
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

/// Main application loop.
async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut app: TuiApp,
) -> Result<()> {
    loop {
        app.chat_manager.poll_session_events();
        app.chat_ids = app.chat_manager.chats.keys().copied().collect();
        if app.chat_list_state.selected().is_none() && !app.chat_ids.is_empty() {
            app.chat_list_state.select(Some(0));
        }

        terminal.draw(|f| {
            ui(f, &mut app);
        })?;

        // Set cursor position
        let input_chunk = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
            .split(terminal.size()?)[1];
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)].as_ref())
            .split(input_chunk);

        terminal.set_cursor(
            main_chunks[1].x + app.input_text.len() as u16 + 1,
            main_chunks[1].y + 1,
        )?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.copy_logs();
                    }
                    KeyCode::Down => app.next_chat(),
                    KeyCode::Up => app.previous_chat(),
                    KeyCode::PageDown => app.scroll_down(),
                    KeyCode::PageUp => app.scroll_up(),
                    KeyCode::Enter => app.send_message(),
                    KeyCode::Char(c) => app.input_text.push(c),
                    KeyCode::Backspace => {
                        app.input_text.pop();
                    }
                    _ => {}
                }
            }
        }
    }
}
