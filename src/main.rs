#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
use clap::Parser;

use egui_tracing::tracing::EventCollector;
use encodeur_rsa_rust::*;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

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

    /// Enable GUI mode (default)
    #[arg(long, default_value_t = true)]
    gui: bool,
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
    let args = Args::parse();
    tracing::debug!(?args, "Parsed CLI arguments");

    if args.gui || (!args.host && args.connect.is_none()) {
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
    } else if args.host {
        // CLI host mode
        tracing::warn!("CLI host mode is not implemented. Please use the GUI.");
        println!("CLI host mode is not implemented. Please use the GUI.");
        std::process::exit(1);
    } else if let Some(addr) = args.connect {
        // CLI client mode
        tracing::warn!(
            "CLI connect mode is not implemented. Please use the GUI. Got address: {}",
            addr
        );
        println!("CLI connect mode is not implemented. Please use the GUI.");
        std::process::exit(1);
    }

    Ok(())
}
