//! TUI command language.
//!
//! Every action in the TUI is reachable by typing a `:`-prefixed command, so the
//! whole app can be driven from the keyboard (or by an automated client). The
//! same `TuiCommand` values are produced both by the command line and by overlay
//! shortcuts, and they all funnel through `TuiApp::execute_command`.

/// A parsed command ready for execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiCommand {
    // --- connections ---
    Host(Option<u16>),
    Connect {
        host: String,
        port: u16,
    },
    HostRelay {
        relay: String,
        token: Option<String>,
    },
    ConnectRelay {
        relay: String,
        token: String,
    },
    Disconnect,
    StopHost,
    /// Set (or clear, when `None`) the P2P connection password used for hosting and
    /// connecting.
    ConnectionPassword(Option<String>),
    /// Lock (`true`) or unlock (`false`) the conversation against new connections.
    Lock(bool),

    // --- contacts ---
    Contacts,
    ContactAdd {
        name: String,
        address: String,
        fingerprint: Option<String>,
    },
    ContactConnect(String),
    ContactRemove(String),
    ContactRename {
        target: String,
        new_name: String,
    },

    // --- invites ---
    Invite(Option<String>),
    InviteRelay(String),
    Import(String),

    // --- files ---
    Send(String),
    Transfers,

    // --- chat management ---
    Rename(String),
    DeleteChat,
    ClearHistory,

    // --- identity / security ---
    Identity,
    Verify(bool),
    Unlock(Option<String>),
    SetPassword(String),

    // --- party servers ---
    PartyConnect {
        address: String,
        username: String,
        password: Option<String>,
    },
    PartyPost(String),
    /// Send a direct message to a member (by username or `#index`) on the current
    /// Party server.
    PartyDm {
        target: String,
        text: String,
    },
    PartyStatus,

    // --- settings ---
    Settings,
    Set {
        key: String,
        value: String,
    },

    // --- app ---
    Diagnostics,
    Logs,
    Help(Option<String>),
    Quit,
    ForceQuit,
}

/// Registry of commands used to render `:help` and validate names.
/// Tuple: (name, usage, description).
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("host", ":host [port]", "Start listening for an incoming peer"),
    ("connect", ":connect <host[:port]>", "Connect to a hosting peer"),
    (
        "host-relay",
        ":host-relay <relay[:port]> [token]",
        "Host through a relay; copies an invite link",
    ),
    (
        "connect-relay",
        ":connect-relay <relay[:port]> <token>",
        "Connect to a peer through a relay",
    ),
    ("disconnect", ":disconnect", "Disconnect / remove the selected chat"),
    ("stop-host", ":stop-host", "Stop listening (tear down the host)"),
    (
        "connection-password",
        ":connection-password [password]",
        "Set/clear the P2P connection password (host requires it; client supplies it)",
    ),
    (
        "lock",
        ":lock <on|off>",
        "Lock the conversation against new connections (on) or unlock (off)",
    ),
    ("contacts", ":contacts", "Open the contacts list"),
    (
        "contact-add",
        ":contact-add <name> <host:port> [fingerprint]",
        "Save a new contact",
    ),
    (
        "contact-connect",
        ":contact-connect <name|#>",
        "Connect to a saved contact",
    ),
    ("contact-remove", ":contact-remove <name|#>", "Delete a saved contact"),
    (
        "contact-rename",
        ":contact-rename <name|#> <new name>",
        "Rename a saved contact",
    ),
    (
        "invite",
        ":invite [host:port]",
        "Generate a signed invite link (copied to clipboard)",
    ),
    (
        "invite-relay",
        ":invite-relay <relay[:port]>",
        "Host via relay and generate an invite link",
    ),
    ("import", ":import <invite-link>", "Import an invite link as a contact"),
    ("send", ":send <path>", "Send a file to the selected chat"),
    ("transfers", ":transfers", "Show active file transfers"),
    (
        "party-connect",
        ":party-connect <host[:port]> <username> [password]",
        "Join a Party server",
    ),
    (
        "party-post",
        ":party-post <message>",
        "Post a message to the current Party channel",
    ),
    (
        "party-dm",
        ":party-dm <username|#> <message>",
        "Direct-message a member on the current Party server",
    ),
    ("party-status", ":party-status", "Show joined Party servers"),
    ("rename", ":rename <title>", "Rename the selected chat"),
    ("delete", ":delete", "Delete the selected chat"),
    ("clear-history", ":clear-history", "Erase all chats and contacts"),
    ("identity", ":identity", "Show your name, fingerprint and safety grid"),
    ("verify", ":verify <accept|reject>", "Answer a pending fingerprint check"),
    ("unlock", ":unlock [password]", "Unlock a password-protected identity"),
    ("set-password", ":set-password <password>", "Set or change your password"),
    ("settings", ":settings", "Open the settings panel"),
    (
        "set",
        ":set <key> <value>",
        "Change a setting (download-dir, notifications, typing, mdns, auto-accept, theme, auto-host, listen-port)",
    ),
    ("diagnostics", ":diagnostics", "Export a diagnostics bundle"),
    ("logs", ":logs", "Copy the event log to the clipboard"),
    ("help", ":help [command]", "Show this help"),
    ("quit", ":quit", "Save and exit (alias :q)"),
];

pub fn parse_bool(value: &str) -> std::result::Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" | "enable" | "enabled" => Ok(true),
        "off" | "false" | "no" | "0" | "disable" | "disabled" => Ok(false),
        other => Err(format!("Expected on/off, got '{}'", other)),
    }
}

/// Split a `host[:port]` token, defaulting the port when omitted.
fn split_host_port(target: &str) -> std::result::Result<(String, u16), String> {
    if let Some((h, p)) = target.rsplit_once(':') {
        // Guard against bare IPv6 without brackets being mis-split.
        let port = p
            .parse::<u16>()
            .map_err(|_| format!("Invalid port in '{}'", target))?;
        if h.trim().is_empty() {
            return Err("Host cannot be empty".to_string());
        }
        Ok((h.to_string(), port))
    } else {
        Ok((target.to_string(), crate::PORT_DEFAULT))
    }
}

/// Parse a raw command-line string (with or without the leading `:`).
pub fn parse_command(raw: &str) -> std::result::Result<TuiCommand, String> {
    let input = raw.trim().trim_start_matches(':').trim();
    if input.is_empty() {
        return Err("Empty command. Try :help".to_string());
    }

    let mut parts = input.split_whitespace();
    let cmd = parts.next().unwrap_or_default();
    // Remainder of the line after the command word (preserves spaces).
    let rest = input[cmd.len()..].trim().to_string();

    match cmd {
        "host" => {
            let port = parts
                .next()
                .map(|p| {
                    p.parse::<u16>()
                        .map_err(|_| "Invalid port. Example: :host 9000".to_string())
                })
                .transpose()?;
            Ok(TuiCommand::Host(port))
        }
        "connect" => {
            let target = parts
                .next()
                .ok_or_else(|| "Usage: :connect <host[:port]>".to_string())?;
            let (host, port) = split_host_port(target)?;
            Ok(TuiCommand::Connect { host, port })
        }
        "host-relay" => {
            let relay = parts
                .next()
                .ok_or_else(|| "Usage: :host-relay <relay[:port]> [token]".to_string())?;
            let token = parts.next().map(str::to_string);
            Ok(TuiCommand::HostRelay {
                relay: relay.to_string(),
                token,
            })
        }
        "connect-relay" => {
            let relay = parts
                .next()
                .ok_or_else(|| "Usage: :connect-relay <relay[:port]> <token>".to_string())?;
            let token = parts
                .next()
                .ok_or_else(|| "Usage: :connect-relay <relay[:port]> <token>".to_string())?;
            Ok(TuiCommand::ConnectRelay {
                relay: relay.to_string(),
                token: token.to_string(),
            })
        }
        "disconnect" => Ok(TuiCommand::Disconnect),
        "stop-host" => Ok(TuiCommand::StopHost),
        "connection-password" => Ok(TuiCommand::ConnectionPassword(
            parts.next().map(str::to_string),
        )),
        "lock" => {
            let arg = parts
                .next()
                .ok_or_else(|| "Usage: :lock <on|off>".to_string())?;
            Ok(TuiCommand::Lock(parse_bool(arg)?))
        }

        "contacts" => Ok(TuiCommand::Contacts),
        "contact-add" => {
            let name = parts.next().ok_or_else(|| {
                "Usage: :contact-add <name> <host:port> [fingerprint]".to_string()
            })?;
            let address = parts.next().ok_or_else(|| {
                "Usage: :contact-add <name> <host:port> [fingerprint]".to_string()
            })?;
            let fingerprint = parts.next().map(str::to_string);
            Ok(TuiCommand::ContactAdd {
                name: name.to_string(),
                address: address.to_string(),
                fingerprint,
            })
        }
        "contact-connect" => {
            let target = parts
                .next()
                .ok_or_else(|| "Usage: :contact-connect <name|#>".to_string())?;
            Ok(TuiCommand::ContactConnect(target.to_string()))
        }
        "contact-remove" => {
            let target = parts
                .next()
                .ok_or_else(|| "Usage: :contact-remove <name|#>".to_string())?;
            Ok(TuiCommand::ContactRemove(target.to_string()))
        }
        "contact-rename" => {
            let target = parts
                .next()
                .ok_or_else(|| "Usage: :contact-rename <name|#> <new name>".to_string())?;
            let new_name = rest[target.len()..].trim().to_string();
            if new_name.is_empty() {
                return Err("Usage: :contact-rename <name|#> <new name>".to_string());
            }
            Ok(TuiCommand::ContactRename {
                target: target.to_string(),
                new_name,
            })
        }

        "invite" => {
            let addr = parts.next().map(str::to_string);
            Ok(TuiCommand::Invite(addr))
        }
        "invite-relay" => {
            let relay = parts
                .next()
                .ok_or_else(|| "Usage: :invite-relay <relay[:port]>".to_string())?;
            Ok(TuiCommand::InviteRelay(relay.to_string()))
        }
        "import" => {
            if rest.is_empty() {
                return Err("Usage: :import <invite-link>".to_string());
            }
            Ok(TuiCommand::Import(rest))
        }

        "send" => {
            if rest.is_empty() {
                return Err("Usage: :send <path>".to_string());
            }
            Ok(TuiCommand::Send(rest))
        }
        "transfers" => Ok(TuiCommand::Transfers),

        "party-connect" => {
            let address = parts.next().ok_or_else(|| {
                "Usage: :party-connect <host[:port]> <username> [password]".to_string()
            })?;
            let username = parts.next().ok_or_else(|| {
                "Usage: :party-connect <host[:port]> <username> [password]".to_string()
            })?;
            let password = parts.next().map(str::to_string);
            Ok(TuiCommand::PartyConnect {
                address: address.to_string(),
                username: username.to_string(),
                password,
            })
        }
        "party-post" => {
            if rest.is_empty() {
                return Err("Usage: :party-post <message>".to_string());
            }
            Ok(TuiCommand::PartyPost(rest))
        }
        "party-dm" => {
            let target = parts
                .next()
                .ok_or_else(|| "Usage: :party-dm <username|#> <message>".to_string())?;
            let text = rest[target.len()..].trim().to_string();
            if text.is_empty() {
                return Err("Usage: :party-dm <username|#> <message>".to_string());
            }
            Ok(TuiCommand::PartyDm {
                target: target.to_string(),
                text,
            })
        }
        "party-status" => Ok(TuiCommand::PartyStatus),

        "rename" => {
            if rest.is_empty() {
                return Err("Usage: :rename <new title>".to_string());
            }
            Ok(TuiCommand::Rename(rest))
        }
        "delete" => Ok(TuiCommand::DeleteChat),
        "clear-history" => Ok(TuiCommand::ClearHistory),

        "identity" => Ok(TuiCommand::Identity),
        "verify" => {
            let arg = parts
                .next()
                .ok_or_else(|| "Usage: :verify <accept|reject>".to_string())?;
            match arg.to_ascii_lowercase().as_str() {
                "accept" | "yes" | "y" | "ok" => Ok(TuiCommand::Verify(true)),
                "reject" | "no" | "n" | "deny" => Ok(TuiCommand::Verify(false)),
                other => Err(format!("Expected accept or reject, got '{}'", other)),
            }
        }
        "unlock" => Ok(TuiCommand::Unlock(parts.next().map(str::to_string))),
        "set-password" => {
            let pw = parts
                .next()
                .ok_or_else(|| "Usage: :set-password <password>".to_string())?;
            Ok(TuiCommand::SetPassword(pw.to_string()))
        }

        "settings" => Ok(TuiCommand::Settings),
        "set" => {
            let key = parts
                .next()
                .ok_or_else(|| "Usage: :set <key> <value>".to_string())?;
            let value = rest[key.len()..].trim().to_string();
            if value.is_empty() {
                return Err("Usage: :set <key> <value>".to_string());
            }
            Ok(TuiCommand::Set {
                key: key.to_ascii_lowercase(),
                value,
            })
        }

        "diagnostics" | "diag" => Ok(TuiCommand::Diagnostics),
        "logs" => Ok(TuiCommand::Logs),
        "help" | "?" => Ok(TuiCommand::Help(parts.next().map(str::to_string))),
        "quit" | "q" => Ok(TuiCommand::Quit),
        "quit!" | "q!" => Ok(TuiCommand::ForceQuit),
        _ => Err(format!("Unknown command: {}. Try :help", cmd)),
    }
}

/// Valid keys for `:set`, exposed for validation and help.
pub fn settings_keys() -> &'static [&'static str] {
    &[
        "download-dir",
        "notifications",
        "typing",
        "mdns",
        "auto-accept",
        "theme",
        "auto-host",
        "listen-port",
    ]
}

pub use parse_bool as parse_setting_bool;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connection_commands() {
        assert_eq!(
            parse_command(":host 7777").unwrap(),
            TuiCommand::Host(Some(7777))
        );
        assert_eq!(parse_command(":host").unwrap(), TuiCommand::Host(None));
        assert_eq!(
            parse_command(":connect 10.0.0.1").unwrap(),
            TuiCommand::Connect {
                host: "10.0.0.1".into(),
                port: crate::PORT_DEFAULT
            }
        );
        assert_eq!(
            parse_command(":connect-relay relay:23456 tok").unwrap(),
            TuiCommand::ConnectRelay {
                relay: "relay:23456".into(),
                token: "tok".into()
            }
        );
    }

    #[test]
    fn parses_connection_password_and_lock() {
        assert_eq!(
            parse_command(":connection-password hunter2").unwrap(),
            TuiCommand::ConnectionPassword(Some("hunter2".into()))
        );
        assert_eq!(
            parse_command(":connection-password").unwrap(),
            TuiCommand::ConnectionPassword(None)
        );
        assert_eq!(parse_command(":lock on").unwrap(), TuiCommand::Lock(true));
        assert_eq!(parse_command(":lock off").unwrap(), TuiCommand::Lock(false));
        assert!(parse_command(":lock").is_err());
    }

    #[test]
    fn parses_party_commands() {
        assert_eq!(
            parse_command(":party-connect 10.0.0.5:9000 alice s3cret").unwrap(),
            TuiCommand::PartyConnect {
                address: "10.0.0.5:9000".into(),
                username: "alice".into(),
                password: Some("s3cret".into()),
            }
        );
        assert_eq!(
            parse_command(":party-connect 10.0.0.5 bob").unwrap(),
            TuiCommand::PartyConnect {
                address: "10.0.0.5".into(),
                username: "bob".into(),
                password: None,
            }
        );
        assert_eq!(
            parse_command(":party-post hello everyone").unwrap(),
            TuiCommand::PartyPost("hello everyone".into())
        );
        assert_eq!(
            parse_command(":party-dm alice hi there").unwrap(),
            TuiCommand::PartyDm {
                target: "alice".into(),
                text: "hi there".into()
            }
        );
        assert!(parse_command(":party-dm alice").is_err());
        assert_eq!(
            parse_command(":party-status").unwrap(),
            TuiCommand::PartyStatus
        );
        assert!(parse_command(":party-connect 10.0.0.5").is_err());
        assert!(parse_command(":party-post").is_err());
    }

    #[test]
    fn parses_contact_commands() {
        assert_eq!(
            parse_command(":contact-add Alice 1.2.3.4:9000 ab12").unwrap(),
            TuiCommand::ContactAdd {
                name: "Alice".into(),
                address: "1.2.3.4:9000".into(),
                fingerprint: Some("ab12".into())
            }
        );
        assert_eq!(
            parse_command(":contact-rename 2 New Name").unwrap(),
            TuiCommand::ContactRename {
                target: "2".into(),
                new_name: "New Name".into()
            }
        );
    }

    #[test]
    fn parses_set_and_verify() {
        assert_eq!(
            parse_command(":set notifications off").unwrap(),
            TuiCommand::Set {
                key: "notifications".into(),
                value: "off".into()
            }
        );
        assert_eq!(
            parse_command(":verify accept").unwrap(),
            TuiCommand::Verify(true)
        );
        assert_eq!(
            parse_command(":verify reject").unwrap(),
            TuiCommand::Verify(false)
        );
        assert!(parse_command(":verify maybe").is_err());
    }

    #[test]
    fn import_and_send_preserve_remainder() {
        assert_eq!(
            parse_command(":import chat-p2p://invite/v2/abc").unwrap(),
            TuiCommand::Import("chat-p2p://invite/v2/abc".into())
        );
        assert_eq!(
            parse_command(":send C:/path with space/file.txt").unwrap(),
            TuiCommand::Send("C:/path with space/file.txt".into())
        );
    }

    #[test]
    fn rejects_unknown_and_empty() {
        assert!(parse_command(":bogus").is_err());
        assert!(parse_command(":").is_err());
        assert!(parse_command(":rename").is_err());
    }

    #[test]
    fn parses_quit_aliases() {
        assert_eq!(parse_command(":q").unwrap(), TuiCommand::Quit);
        assert_eq!(parse_command(":quit!").unwrap(), TuiCommand::ForceQuit);
    }

    #[test]
    fn setting_bool_accepts_common_forms() {
        assert!(parse_setting_bool("on").unwrap());
        assert!(!parse_setting_bool("FALSE").unwrap());
        assert!(parse_setting_bool("maybe").is_err());
    }
}
