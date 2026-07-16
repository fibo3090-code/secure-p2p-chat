#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
use clap::Parser;

use egui_tracing::tracing::EventCollector;
use p2pem_classic::*;
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

    /// Run a self-hosted relay server on --port
    #[arg(long)]
    relay_server: bool,

    /// Host a session through a relay endpoint (format: HOST:PORT)
    #[arg(long)]
    host_relay: Option<String>,

    /// Connect to a peer through a relay endpoint (format: HOST:PORT)
    #[arg(long)]
    connect_relay: Option<String>,

    /// Relay rendezvous token used with --host-relay or --connect-relay
    #[arg(long)]
    relay_token: Option<String>,

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
    support::install_panic_hook();

    // Initialize logging
    let event_collector = EventCollector::new();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,p2pem_classic=debug".into()),
        )
        .with(event_collector.clone())
        .init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Application starting");
    let mut args = Args::parse();
    tracing::debug!(?args, "Parsed CLI arguments");

    if args.relay_server {
        tracing::info!("Starting relay server mode");
        network::run_relay_server(args.port).await?;
        return Ok(());
    }

    // Default to GUI if no mode is specified
    if !args.gui && !args.tui {
        args.gui = true;
    }

    // On Windows, if we are in TUI mode or explicitly requested console features,
    // we need to attach to the parent console or allocate a new one.
    #[cfg(target_os = "windows")]
    if args.tui || args.host || args.connect.is_some() {
        use std::io::IsTerminal;
        if !std::io::stdout().is_terminal() {
            // Try to attach to parent console (e.g. if launched from cmd/powershell)
            if unsafe {
                windows_sys::Win32::System::Console::AttachConsole(
                    windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
                )
            } == 0
            {
                // If attaching failed (e.g. launched from Explorer), allocate a new console
                unsafe { windows_sys::Win32::System::Console::AllocConsole() };
            }
        }
    }

    if args.tui {
        // Launch TUI
        tracing::info!("Starting TUI mode");
        let launch = tui::TuiLaunchConfig {
            host: args.host,
            connect: args.connect.clone(),
            host_relay: args.host_relay.clone(),
            relay_connect: args.connect_relay.clone(),
            relay_token: args.relay_token.clone(),
            port: args.port,
        };
        if let Err(e) = tui::run(event_collector.clone(), launch).await {
            tracing::error!(error = %e, "TUI application exited with an error");
            std::process::exit(1);
        }
        tracing::info!("TUI application exited");
    } else if args.gui {
        // Launch GUI
        tracing::info!("Starting GUI mode");

        let icon = image::load_from_memory(include_bytes!("../assets/icon.png"))
            .expect("bundled app icon must decode")
            .into_rgba8();
        let (icon_width, icon_height) = icon.dimensions();
        let native_options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1200.0, 800.0])
                .with_min_inner_size([800.0, 600.0])
                .with_icon(egui::IconData {
                    rgba: icon.into_raw(),
                    width: icon_width,
                    height: icon_height,
                }),
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
