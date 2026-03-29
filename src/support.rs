use anyhow::{Context, Result};
use chrono::Utc;
use egui_tracing::tracing::EventCollector;
use serde::Serialize;
use std::backtrace::Backtrace;
use std::path::{Path, PathBuf};

use crate::types::Config;

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsConfig {
    pub download_dir: String,
    pub temp_dir: String,
    pub auto_accept_files: bool,
    pub enable_notifications: bool,
    pub enable_typing_indicators: bool,
    pub show_log_terminal: bool,
    pub theme: String,
    pub font_size: u8,
    pub auto_connect: bool,
    pub notification_sound: String,
    pub auto_host_on_startup: bool,
    pub listen_port: u16,
    pub auto_trust_on_first_use: bool,
    pub enable_mdns: bool,
}

impl From<&Config> for DiagnosticsConfig {
    fn from(config: &Config) -> Self {
        Self {
            download_dir: config.download_dir.display().to_string(),
            temp_dir: config.temp_dir.display().to_string(),
            auto_accept_files: config.auto_accept_files,
            enable_notifications: config.enable_notifications,
            enable_typing_indicators: config.enable_typing_indicators,
            show_log_terminal: config.show_log_terminal,
            theme: format!("{:?}", config.theme),
            font_size: config.font_size,
            auto_connect: config.auto_connect,
            notification_sound: format!("{:?}", config.notification_sound),
            auto_host_on_startup: config.auto_host_on_startup,
            listen_port: config.listen_port,
            auto_trust_on_first_use: config.auto_trust_on_first_use,
            enable_mdns: config.enable_mdns,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsReport {
    pub generated_at_utc: String,
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub history_path: String,
    pub identity_path: String,
    pub history_exists: bool,
    pub identity_exists: bool,
    pub identity_locked: bool,
    pub identity_name: String,
    pub identity_fingerprint_prefix: String,
    pub chats: usize,
    pub contacts: usize,
    pub sessions: usize,
    pub active_toasts: usize,
    pub discovered_peers: usize,
    pub config: DiagnosticsConfig,
}

pub fn format_event_logs(event_collector: &EventCollector) -> String {
    let mut log_text = String::new();
    for event in event_collector.events() {
        let level = event.level.as_str();
        let target = event.target.as_str();
        let msg = event
            .fields
            .get("message")
            .map(|s| s.as_str())
            .unwrap_or("");
        let timestamp = event.time.to_rfc3339();
        log_text.push_str(&format!("[{}] {} [{}] {}\n", timestamp, level, target, msg));
    }
    log_text
}

pub fn default_diagnostics_dir() -> PathBuf {
    if let Some(dirs) = directories::ProjectDirs::from("com", "chat-p2p", "EncryptedMessenger") {
        return dirs.data_dir().join("diagnostics");
    }
    PathBuf::from("diagnostics")
}

pub fn export_diagnostics_bundle(
    base_dir: &Path,
    report: &DiagnosticsReport,
    logs: &str,
) -> Result<PathBuf> {
    std::fs::create_dir_all(base_dir).with_context(|| {
        format!(
            "failed to create diagnostics directory {}",
            base_dir.display()
        )
    })?;

    let stamp = Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let bundle_dir = base_dir.join(format!("bundle-{}", stamp));
    std::fs::create_dir_all(&bundle_dir)
        .with_context(|| format!("failed to create bundle directory {}", bundle_dir.display()))?;

    let report_path = bundle_dir.join("diagnostics.json");
    let logs_path = bundle_dir.join("logs.txt");
    let readme_path = bundle_dir.join("README.txt");

    let report_json = serde_json::to_string_pretty(report)?;
    std::fs::write(&report_path, report_json)
        .with_context(|| format!("failed to write {}", report_path.display()))?;
    std::fs::write(&logs_path, logs)
        .with_context(|| format!("failed to write {}", logs_path.display()))?;
    std::fs::write(
        &readme_path,
        "Diagnostics bundle for support.\nShare diagnostics.json and logs.txt when reporting a bug.\nPrivate keys are not included.\n",
    )
    .with_context(|| format!("failed to write {}", readme_path.display()))?;

    Ok(bundle_dir)
}

pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            *s
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.as_str()
        } else {
            "non-string panic payload"
        };

        let location = info
            .location()
            .map(|loc| format!("{}:{}", loc.file(), loc.line()))
            .unwrap_or_else(|| "unknown".to_string());

        let crash_dir = default_diagnostics_dir().join("crashes");
        let _ = std::fs::create_dir_all(&crash_dir);
        let crash_path =
            crash_dir.join(format!("panic-{}.log", Utc::now().format("%Y%m%d-%H%M%S")));
        let report = format!(
            "application: Encrypted P2P Messenger\nversion: {}\nos: {}\narch: {}\ntimestamp_utc: {}\nlocation: {}\npanic: {}\n\nbacktrace:\n{}\n",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            Utc::now().to_rfc3339(),
            location,
            payload,
            Backtrace::force_capture()
        );
        let _ = std::fs::write(&crash_path, report);

        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_bundle_writes_expected_files() {
        let dir = tempfile::tempdir().unwrap();
        let report = DiagnosticsReport {
            generated_at_utc: Utc::now().to_rfc3339(),
            app_version: "test".to_string(),
            os: "windows".to_string(),
            arch: "x86_64".to_string(),
            history_path: "history.json.enc".to_string(),
            identity_path: "identity.json".to_string(),
            history_exists: false,
            identity_exists: false,
            identity_locked: true,
            identity_name: "User".to_string(),
            identity_fingerprint_prefix: "abcd1234".to_string(),
            chats: 1,
            contacts: 2,
            sessions: 0,
            active_toasts: 0,
            discovered_peers: 0,
            config: DiagnosticsConfig {
                download_dir: "Downloads".to_string(),
                temp_dir: "temp".to_string(),
                auto_accept_files: false,
                enable_notifications: true,
                enable_typing_indicators: true,
                show_log_terminal: false,
                theme: "Dark".to_string(),
                font_size: 14,
                auto_connect: false,
                notification_sound: "Default".to_string(),
                auto_host_on_startup: false,
                listen_port: crate::PORT_DEFAULT,
                auto_trust_on_first_use: false,
                enable_mdns: false,
            },
        };

        let bundle_dir = export_diagnostics_bundle(dir.path(), &report, "hello logs").unwrap();
        assert!(bundle_dir.join("diagnostics.json").exists());
        assert!(bundle_dir.join("logs.txt").exists());
        assert!(bundle_dir.join("README.txt").exists());
    }
}
