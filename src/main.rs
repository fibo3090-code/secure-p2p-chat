#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
use clap::Parser;

use egui_tracing::tracing::EventCollector;
use encodeur_rsa_rust::*;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod tui;

#[derive(Debug, Parser)]
#[command(author, version, about = "P2P Encrypted Messaging Application")]
struct Args {
    /// Start as host (server mode)
    #[arg(short = 'H', long)]
    host: bool,

    /// Connect to host (format: IP:PORT or IP)
    #[arg(short, long)]
    connect: Option<String>,

    /// Port to use (default: 12345)
    #[arg(short, long, default_value_t = PORT_DEFAULT)]
    port: u16,

    /// Enable GUI mode (default unless --tui is used)
    #[arg(long)]
    gui: bool,

    /// Enable Terminal UI mode
    #[arg(long)]
    tui: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    let event_collector = EventCollector::new();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,encodeur_rsa_rust=debug".into()),
        )
        .with(event_collector.clone())
        .init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Application starting");
    let mut args = Args::parse();
    tracing::debug!(?args, "Parsed CLI arguments");

    // Default to GUI if no mode is specified
    if !args.gui && !args.tui {
        args.gui = true;
    }

    if args.tui {
        // Launch TUI
        tracing::info!("Starting TUI mode");
        if let Err(e) = tui::run().await {
            tracing::error!(error = %e, "TUI application exited with an error");
            std::process::exit(1);
        }
        tracing::info!("TUI application exited");
    } else if args.gui {
        // Launch GUI
        tracing::info!("Starting GUI mode");

        let native_options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1200.0, 800.0])
                .with_min_inner_size([800.0, 600.0]),
            ..Default::default()
        };

        tracing::debug!("Creating native window and launching eframe");
        let run_result = eframe::run_native(
            "Encrypted P2P Messenger",
            native_options,
            Box::new(|cc| match gui::App::new(cc, event_collector.clone()) {
                Ok(app) => Ok(Box::new(app)),
                Err(e) => {
                    tracing::error!(error = %e, "Failed to create app state");
                    Err(e.into())
                }
            }),
        );
        if let Err(e) = run_result {
            tracing::error!(error = %e, "Failed to start GUI application");
            std::process::exit(1);
        }
        tracing::info!("GUI application exited");
    }

    Ok(())
}
