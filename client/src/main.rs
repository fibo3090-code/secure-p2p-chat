//! P2PEM terminal client.
//!
//! This binary is the ratatui terminal UI plus the relay-server mode. The
//! desktop app is a separate product (`p2pem-desktop`, Tauri + React) built
//! from `desktop/`; the egui GUI that used to live behind `--gui` was retired
//! once that reached parity, so there is exactly one desktop app to install.
use clap::Parser;

use p2pem_classic::logbuf::LogBuffer;
use p2pem_classic::*;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "P2PEM — encrypted peer-to-peer messaging (terminal client)"
)]
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

    /// Retired: the graphical app is now a separate download (see --help)
    #[arg(long, hide = true)]
    gui: bool,

    /// Run the terminal UI (the default for this binary)
    #[arg(long)]
    tui: bool,
}

/// Printed when someone passes the retired `--gui` flag, or launches a shortcut
/// that still carries it. Failing with directions beats silently starting a
/// different interface than the one that was asked for.
const GUI_RETIRED_NOTICE: &str = "\
`--gui` has been retired: this binary is the terminal client.

The graphical app is P2PEM Desktop, a separate download:
  https://github.com/fibo3090-code/secure-p2p-chat/releases/latest

It uses its own identity and history, so installing it does not affect this one.
To keep using the terminal interface, run without `--gui`.";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    support::install_panic_hook();

    // Retained in memory for the log overlay (Ctrl+L) and diagnostics bundles,
    // in addition to whatever the environment's RUST_LOG selects.
    let logs = LogBuffer::new();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,p2pem_classic=debug".into()),
        )
        .with(logs.clone())
        .init();

    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Application starting");
    let args = Args::parse();
    tracing::debug!(?args, "Parsed CLI arguments");

    if args.gui {
        eprintln!("{GUI_RETIRED_NOTICE}");
        std::process::exit(2);
    }

    if args.relay_server {
        tracing::info!("Starting relay server mode");
        network::run_relay_server(args.port).await?;
        return Ok(());
    }

    // No console juggling on Windows any more: this is a console-subsystem
    // binary now that the GUI is gone, so Windows attaches (or creates) a
    // console for it. The old build declared `windows_subsystem = "windows"`
    // for the egui window and had to `AttachConsole`/`AllocConsole` by hand
    // whenever the terminal UI was requested.
    tracing::info!("Starting TUI mode");
    let launch = tui::TuiLaunchConfig {
        host: args.host,
        connect: args.connect.clone(),
        host_relay: args.host_relay.clone(),
        relay_connect: args.connect_relay.clone(),
        relay_token: args.relay_token.clone(),
        port: args.port,
    };
    if let Err(e) = tui::run(logs, launch).await {
        tracing::error!(error = %e, "TUI application exited with an error");
        std::process::exit(1);
    }
    tracing::info!("TUI application exited");

    Ok(())
}
