use crate::logbuf::LogBuffer;
use anyhow::{Context, Result};
use chrono::Utc;
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
    pub auto_host_on_startup: bool,
    pub listen_port: u16,
    pub auto_trust_on_first_use: bool,
    pub enable_mdns: bool,
    pub relay_server: Option<String>,
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
            auto_host_on_startup: config.auto_host_on_startup,
            listen_port: config.listen_port,
            auto_trust_on_first_use: config.auto_trust_on_first_use,
            enable_mdns: config.enable_mdns,
            relay_server: config.relay_server.clone(),
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

/// Render the retained log events as plain text for the clipboard and the
/// diagnostics bundle.
/// The event log as it may leave the machine — copied to the clipboard or
/// put in a diagnostics bundle: [`format_event_logs`] scrubbed by
/// [`redact_for_sharing`]. One function for both, because log lines now carry
/// their structured fields (fingerprints, addresses — QUAL-06) and a copy is
/// usually pasted into a public issue, exactly like the bundle.
pub fn shareable_event_logs(logs: &LogBuffer) -> String {
    redact_for_sharing(&format_event_logs(logs))
}

pub fn format_event_logs(logs: &LogBuffer) -> String {
    logs.events()
        .iter()
        .map(|e| {
            format!(
                "[{}] {} [{}] {}\n",
                e.time.to_rfc3339(),
                e.level,
                e.target,
                e.message
            )
        })
        .collect()
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

    // Owner-only, like everything else the app writes about its user, and the
    // logs scrubbed: this bundle is made to be attached to a public issue, and
    // it carried live relay tokens, peer addresses and full fingerprints in
    // world-readable files (audit PRIV-04).
    let write = |path: &Path, body: &[u8]| {
        messenger_core::util::write_file_atomic(path, body)
            .with_context(|| format!("failed to write {}", path.display()))
    };
    let report_json = serde_json::to_string_pretty(report)?;
    write(&report_path, report_json.as_bytes())?;
    write(&logs_path, redact_for_sharing(logs).as_bytes())?;
    write(
        &readme_path,
        b"Diagnostics bundle for support.\n\
          Share diagnostics.json and logs.txt when reporting a bug.\n\
          Private keys and message text are not included. Relay tokens, IP addresses\n\
          and long fingerprints in the logs are replaced with <hex> / <ip>, but file\n\
          names and contact names may remain: read logs.txt before sharing it.\n",
    )?;

    Ok(bundle_dir)
}

/// Scrub what must not leave the machine from text meant to be shared: runs of
/// 32+ hex digits (relay tokens, fingerprints, hashes) become `<hex>` and IPv4
/// addresses `<ip>`. Best effort by shape — a short fingerprint prefix, which
/// identifies nothing alone, is kept, and names are not recognised.
pub fn redact_for_sharing(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let core = word.trim_end_matches('.');
        let tail = &word[core.len()..];
        let is_hex = core.len() >= 32 && core.chars().all(|c| c.is_ascii_hexdigit());
        let parts: Vec<&str> = core.split('.').collect();
        let is_ipv4 = parts.len() == 4
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u16>().is_ok_and(|n| n <= 255));
        if is_hex {
            out.push_str("<hex>");
            out.push_str(tail);
        } else if is_ipv4 {
            out.push_str("<ip>");
            out.push_str(tail);
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

pub fn install_panic_hook() {
    install_panic_hook_in(default_diagnostics_dir());
}

/// [`install_panic_hook`] writing under `diagnostics_dir/crashes`. The
/// desktop app passes its own data folder: the default is the terminal
/// client's, and the desktop shipped with no hook at all, so a crash in a
/// `panic = "abort"` release left no trace anywhere (QUAL-07).
pub fn install_panic_hook_in(diagnostics_dir: PathBuf) {
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

        let crash_dir = diagnostics_dir.join("crashes");
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

    /// A panic leaves a report where it was told to, with the message and
    /// where it happened.
    #[test]
    fn a_panic_leaves_a_crash_report_in_the_given_folder() {
        let dir = tempfile::tempdir().unwrap();
        install_panic_hook_in(dir.path().to_path_buf());
        let caught = std::panic::catch_unwind(|| panic!("the crash under test"));
        // Back to the default hook before asserting, so a failure below is
        // reported normally.
        let _ = std::panic::take_hook();
        assert!(caught.is_err());
        let reports: Vec<_> = std::fs::read_dir(dir.path().join("crashes"))
            .expect("no crashes folder")
            .flatten()
            .collect();
        assert_eq!(reports.len(), 1);
        let report = std::fs::read_to_string(reports[0].path()).unwrap();
        assert!(report.contains("panic: the crash under test"), "{report}");
        assert!(report.contains("support.rs"), "no location: {report}");
    }

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
            config: DiagnosticsConfig::from(&Config::default()),
        };

        let token = "0123456789abcdef0123456789abcdef";
        let fp = "ab".repeat(32);
        let logs = format!("relay token {token} from 192.168.1.20:12345 peer {fp}.");
        let bundle_dir = export_diagnostics_bundle(dir.path(), &report, &logs).unwrap();
        assert!(bundle_dir.join("diagnostics.json").exists());
        assert!(bundle_dir.join("README.txt").exists());

        // PRIV-04: the bundle is meant to be attached to a public issue, and
        // its logs carried live relay tokens, peer addresses and full
        // fingerprints, in files any local user could read.
        let written = std::fs::read_to_string(bundle_dir.join("logs.txt")).unwrap();
        for leaked in [token, "192.168.1.20", fp.as_str()] {
            assert!(
                !written.contains(leaked),
                "{leaked} left in the shared logs: {written}"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for f in ["diagnostics.json", "logs.txt", "README.txt"] {
                let mode = std::fs::metadata(bundle_dir.join(f))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o600, "{f} is {:o}", mode & 0o777);
            }
        }
    }

    #[test]
    fn redaction_removes_secrets_and_keeps_the_rest() {
        let out = redact_for_sharing(
            "v1.2.3 dialled 10.0.0.5:9000 with 0123456789ABCDEF0123456789ABCDEF; port 12345 ok",
        );
        assert_eq!(out, "v1.2.3 dialled <ip>:9000 with <hex>; port 12345 ok");
        // Not addresses: too few parts, or an octet out of range.
        assert_eq!(
            redact_for_sharing("1.2.3 and 1.2.3.999"),
            "1.2.3 and 1.2.3.999"
        );
        // Short hex (an 8-char fingerprint prefix) is kept: it identifies nothing alone.
        assert_eq!(redact_for_sharing("fp abcd1234"), "fp abcd1234");
    }

    /// What leaves the machine is scrubbed, fields included.
    #[test]
    fn the_shareable_log_keeps_the_error_and_drops_identifiers() {
        use tracing_subscriber::layer::SubscriberExt;
        let logs = LogBuffer::new();
        let fp = "ab".repeat(32);
        tracing::subscriber::with_default(
            tracing_subscriber::registry().with(logs.clone()),
            || {
                tracing::warn!(peer = %fp, addr = "203.0.113.7:12345", error = "handshake timed out", "dial failed");
            },
        );
        let out = shareable_event_logs(&logs);
        assert!(
            out.contains("dial failed") && out.contains("handshake timed out"),
            "{out}"
        );
        assert!(!out.contains(&fp) && !out.contains("203.0.113.7"), "{out}");
    }
}
