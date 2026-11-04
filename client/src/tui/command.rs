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
    AcceptFile,
    DeclineFile,

    // --- identity ---
    Name(String),
    Backup(String),

    // --- chat management ---
    Rename(String),
    DeleteChat,
    ClearHistory,
    /// Move an unreadable history file aside and start a new, empty one.
    HistoryReset,
    /// Delete everything on this device (password-checked, then confirmed).
    Wipe {
        password: String,
    },

    // --- identity / security ---
    Identity,
    Verify(bool),
    Unlock(Option<String>),
    SetPassword(String),
    ChangePassword {
        current: String,
        new: String,
    },

    // --- party servers ---
    PartyConnect {
        address: String,
        username: String,
        password: Option<String>,
    },
    /// Accept the community server identity shown by the last `:party-connect`
    /// and finish joining. Separate command because the credentials are only
    /// sent once the user has confirmed the fingerprint out of band.
    PartyTrust,
    PartyPost(String),
    /// Send a direct message to a member (by username or `#index`) on the current
    /// Party server.
    PartyDm {
        target: String,
        text: String,
    },
    /// Create a channel on the current Party server.
    PartyCreateChannel(String),
    /// Share a local file into the current Party server's channel.
    PartySendFile(String),
    /// Download a shared file (matched by name or content-hash prefix) from the
    /// current Party server into the download directory.
    PartyDownload(String),
    /// Open the Communities pane.
    PartyPane,
    /// Refresh the shared-file listing and storage usage.
    PartyFiles,
    /// Delete a shared file by name or hash.
    PartyDeleteFile(String),
    /// Change a member's role: `<username> <guest|member|admin>`.
    PartyRole {
        username: String,
        role: String,
    },
    /// Change a channel's access: `<#channel> <public|private|locked|announce> [members…]`.
    PartyChannelAccess {
        channel: String,
        kind: String,
        members: Vec<String>,
    },
    /// Delete a channel and its history.
    PartyDeleteChannel(String),
    /// Show the server's activity log (admins only).
    PartyAudit,
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
    ("accept", ":accept", "Accept the incoming file offer"),
    ("decline", ":decline", "Decline the incoming file offer"),
    ("name", ":name <name>", "Change your display name"),
    ("backup", ":backup <path>", "Export an encrypted identity backup"),
    (
        "party-connect",
        ":party-connect <host[:port]> <username> [password]",
        "Join a Party server",
    ),
    (
        "party-trust",
        ":party-trust",
        "Accept the community server identity shown by :party-connect and join",
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
    (
        "party-create-channel",
        ":party-create-channel <name>",
        "Create a channel on the current Party server",
    ),
    (
        "party-send-file",
        ":party-send-file <path>",
        "Share a file in the current Party channel",
    ),
    (
        "party-download",
        ":party-download <name|hash>",
        "Save a shared Party file to your download folder",
    ),
    ("party-status", ":party-status", "Show joined Party servers"),
    (
        "party",
        ":party",
        "Open the Communities pane (channels, members, files)",
    ),
    (
        "party-files",
        ":party-files",
        "Refresh the shared-file list and your storage usage",
    ),
    (
        "party-delete-file",
        ":party-delete-file <name|hash>",
        "Delete a file you shared (or any, as an admin)",
    ),
    (
        "party-role",
        ":party-role <username> <guest|member|admin>",
        "Change a member's role (admins only)",
    ),
    (
        "party-channel-access",
        ":party-channel-access <#channel> <public|private|locked|announce> [members…]",
        "Change who can read and post in a channel (admins only)",
    ),
    (
        "party-delete-channel",
        ":party-delete-channel <#channel>",
        "Delete a channel and its history (admins only)",
    ),
    (
        "party-audit",
        ":party-audit",
        "Show the community's activity log (admins only)",
    ),
    ("rename", ":rename <title>", "Rename the selected chat"),
    ("delete", ":delete", "Delete the selected chat"),
    ("clear-history", ":clear-history", "Erase all chats and contacts"),
    (
        "history-reset",
        ":history-reset",
        "Keep an unreadable history file aside and start a new one",
    ),
    (
        "wipe",
        ":wipe <password>",
        "Delete your identity, history and communities from this device, then quit",
    ),
    ("identity", ":identity", "Show your name, fingerprint and safety grid"),
    ("verify", ":verify <accept|reject>", "Answer a pending fingerprint check"),
    ("unlock", ":unlock [password]", "Unlock a password-protected identity"),
    ("set-password", ":set-password <password>", "Set a password on a new identity"),
    (
        "change-password",
        ":change-password <current> <new>",
        "Change your password (needs the current one)",
    ),
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

/// Non-canonical command names accepted by the parser.
/// Tuple: (alias, canonical command name).
pub const COMMAND_ALIASES: &[(&str, &str)] = &[
    ("party-channel", "party-create-channel"),
    ("diag", "diagnostics"),
    ("?", "help"),
    ("q", "quit"),
    ("quit!", "quit"),
    ("q!", "quit"),
];

pub fn command_help(name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let wanted = name.trim().trim_start_matches(':').to_ascii_lowercase();
    let canonical = COMMAND_ALIASES
        .iter()
        .find_map(|(alias, target)| (*alias == wanted).then_some(*target))
        .unwrap_or(wanted.as_str());
    COMMANDS
        .iter()
        .copied()
        .find(|(name, _, _)| *name == canonical)
}

pub fn parse_bool(value: &str) -> std::result::Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "enable" | "enabled" => Ok(true),
        "off" | "false" | "0" | "disable" | "disabled" => Ok(false),
        other => Err(format!("Expected on/off, got '{}'", other)),
    }
}

/// The arguments after a command word, with usage errors drawn from
/// [`COMMANDS`] — so `:help` and the error for a malformed command can never
/// disagree, which is what happened when each arm spelled its own.
struct Args<'a> {
    cmd: &'a str,
    words: std::str::SplitWhitespace<'a>,
    /// Everything after the command word, spaces preserved (trimmed).
    rest: &'a str,
}

impl<'a> Args<'a> {
    fn usage(&self) -> String {
        match command_help(self.cmd) {
            Some((_, usage, _)) => format!("Usage: {usage}"),
            None => format!("Usage: :help {}", self.cmd),
        }
    }

    /// The next word, if any.
    fn next(&mut self) -> Option<&'a str> {
        self.words.next()
    }

    /// The next word, or the command's usage as the error.
    fn word(&mut self) -> Result<&'a str, String> {
        self.words.next().ok_or_else(|| self.usage())
    }

    fn opt_string(&mut self) -> Option<String> {
        self.words.next().map(str::to_string)
    }

    /// The whole remainder of the line (it may contain spaces), required.
    fn text(&self) -> Result<String, String> {
        non_empty(self.rest, || self.usage())
    }

    /// The remainder after `first` (a word already taken with [`Self::word`]),
    /// required — for `<target> <free text>` commands.
    fn text_after(&self, first: &str) -> Result<String, String> {
        non_empty(self.rest[first.len()..].trim(), || self.usage())
    }
}

fn non_empty(s: &str, usage: impl FnOnce() -> String) -> Result<String, String> {
    if s.is_empty() {
        Err(usage())
    } else {
        Ok(s.to_string())
    }
}

/// Parse a raw command-line string (with or without the leading `:`).
pub fn parse_command(raw: &str) -> std::result::Result<TuiCommand, String> {
    use TuiCommand as C;

    let input = raw.trim().trim_start_matches(':').trim();
    if input.is_empty() {
        return Err("Empty command. Try :help".to_string());
    }
    let mut words = input.split_whitespace();
    let cmd = words.next().unwrap_or_default();
    let mut a = Args {
        cmd,
        words,
        rest: input[cmd.len()..].trim(),
    };

    Ok(match cmd {
        "host" => C::Host(
            a.next()
                .map(|p| {
                    p.parse::<u16>()
                        .map_err(|_| "Invalid port. Example: :host 9000".to_string())
                })
                .transpose()?,
        ),
        "connect" => {
            let target = a.word()?;
            // The same parser every other connect path uses.
            let (host, port) = crate::util::parse_host_port(target, Some(crate::PORT_DEFAULT))
                .map_err(|e| format!("{e} in '{target}'"))?;
            C::Connect { host, port }
        }
        "host-relay" => C::HostRelay {
            relay: a.word()?.to_string(),
            token: a.opt_string(),
        },
        "connect-relay" => C::ConnectRelay {
            relay: a.word()?.to_string(),
            token: a.word()?.to_string(),
        },
        "disconnect" => C::Disconnect,
        "stop-host" => C::StopHost,
        "connection-password" => C::ConnectionPassword(a.opt_string()),
        "lock" => C::Lock(parse_bool(a.word()?)?),

        "contacts" => C::Contacts,
        "contact-add" => C::ContactAdd {
            name: a.word()?.to_string(),
            address: a.word()?.to_string(),
            fingerprint: a.opt_string(),
        },
        "contact-connect" => C::ContactConnect(a.word()?.to_string()),
        "contact-remove" => C::ContactRemove(a.word()?.to_string()),
        "contact-rename" => {
            let target = a.word()?;
            C::ContactRename {
                target: target.to_string(),
                new_name: a.text_after(target)?,
            }
        }

        "invite" => C::Invite(a.opt_string()),
        "invite-relay" => C::InviteRelay(a.word()?.to_string()),
        "import" => C::Import(a.text()?),

        "send" => C::Send(a.text()?),
        "transfers" => C::Transfers,
        "accept" => C::AcceptFile,
        "decline" => C::DeclineFile,
        "name" => C::Name(a.text()?),
        "backup" => C::Backup(a.text()?),

        "party-connect" => C::PartyConnect {
            address: a.word()?.to_string(),
            username: a.word()?.to_string(),
            password: a.opt_string(),
        },
        "party-trust" => C::PartyTrust,
        "party-post" => C::PartyPost(a.text()?),
        "party-dm" => {
            let target = a.word()?;
            C::PartyDm {
                target: target.to_string(),
                text: a.text_after(target)?,
            }
        }
        "party-create-channel" | "party-channel" => C::PartyCreateChannel(a.text()?),
        "party-send-file" => C::PartySendFile(a.text()?),
        "party-download" => C::PartyDownload(a.text()?),
        "party-status" => C::PartyStatus,
        "party" => C::PartyPane,
        "party-files" => C::PartyFiles,
        "party-audit" => C::PartyAudit,
        "party-delete-file" => C::PartyDeleteFile(a.text()?),
        "party-delete-channel" => {
            C::PartyDeleteChannel(a.text()?.trim_start_matches('#').to_string())
        }
        "party-role" => C::PartyRole {
            username: a.word()?.to_string(),
            role: a.word()?.to_ascii_lowercase(),
        },
        "party-channel-access" => C::PartyChannelAccess {
            channel: a.word()?.trim_start_matches('#').to_string(),
            kind: a.word()?.to_ascii_lowercase(),
            members: a.words.map(str::to_string).collect(),
        },

        "rename" => C::Rename(a.text()?),
        "delete" => C::DeleteChat,
        "clear-history" => C::ClearHistory,
        "history-reset" => C::HistoryReset,
        "wipe" => C::Wipe {
            password: a.text()?,
        },

        "identity" => C::Identity,
        "verify" => match a.word()?.to_ascii_lowercase().as_str() {
            "accept" | "y" | "ok" => C::Verify(true),
            "reject" | "n" | "deny" => C::Verify(false),
            other => return Err(format!("Expected accept or reject, got '{}'", other)),
        },
        "unlock" => C::Unlock(a.opt_string()),
        "set-password" => C::SetPassword(a.word()?.to_string()),
        "change-password" => C::ChangePassword {
            current: a.word()?.to_string(),
            new: a.word()?.to_string(),
        },

        "settings" => C::Settings,
        "set" => {
            let key = a.word()?;
            C::Set {
                key: key.to_ascii_lowercase(),
                value: a.text_after(key)?,
            }
        }

        "diagnostics" | "diag" => C::Diagnostics,
        "logs" => C::Logs,
        "help" | "?" => C::Help(a.opt_string()),
        "quit" | "q" => C::Quit,
        "quit!" | "q!" => C::ForceQuit,
        _ => return Err(format!("Unknown command: {}. Try :help", cmd)),
    })
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
    fn parses_party_file_commands() {
        assert_eq!(
            parse_command(":party-send-file C:\\pics\\cat photo.png").unwrap(),
            TuiCommand::PartySendFile("C:\\pics\\cat photo.png".into()),
            "the whole rest (spaces included) is the path"
        );
        assert!(parse_command(":party-send-file").is_err());

        assert_eq!(
            parse_command(":party-download cat photo.png").unwrap(),
            TuiCommand::PartyDownload("cat photo.png".into())
        );
        assert!(parse_command(":party-download").is_err());
    }

    #[test]
    fn parses_connection_password_and_lock() {
        let pw = format!("pw-{}", uuid::Uuid::new_v4());
        assert_eq!(
            parse_command(&format!(":connection-password {pw}")).unwrap(),
            TuiCommand::ConnectionPassword(Some(pw))
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
        let party_pw = format!("pw-{}", uuid::Uuid::new_v4());
        assert_eq!(
            parse_command(&format!(":party-connect 10.0.0.5:9000 alice {party_pw}")).unwrap(),
            TuiCommand::PartyConnect {
                address: "10.0.0.5:9000".into(),
                username: "alice".into(),
                password: Some(party_pw),
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
            parse_command(":party-create-channel random").unwrap(),
            TuiCommand::PartyCreateChannel("random".into())
        );
        assert_eq!(
            parse_command(":party-channel random").unwrap(),
            TuiCommand::PartyCreateChannel("random".into())
        );
        assert!(parse_command(":party-create-channel").is_err());
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
    fn parses_help_topics_and_command_aliases() {
        assert_eq!(
            parse_command(":help connect").unwrap(),
            TuiCommand::Help(Some("connect".into()))
        );
        assert_eq!(
            command_help("party-channel").unwrap().0,
            "party-create-channel"
        );
        assert_eq!(command_help(":q").unwrap().0, "quit");
        assert!(command_help("missing").is_none());
    }

    #[test]
    fn parses_ipv6_targets_without_losing_host_bits() {
        assert_eq!(
            parse_command(":connect ::1").unwrap(),
            TuiCommand::Connect {
                host: "::1".into(),
                port: crate::PORT_DEFAULT,
            }
        );
        // The brackets are address syntax, not part of the host: a socket
        // cannot resolve "[::1]", only "::1".
        assert_eq!(
            parse_command(":connect [::1]:9000").unwrap(),
            TuiCommand::Connect {
                host: "::1".into(),
                port: 9000,
            }
        );
    }

    #[test]
    fn setting_bool_accepts_common_forms() {
        assert!(parse_setting_bool("on").unwrap());
        assert!(!parse_setting_bool("FALSE").unwrap());
        assert!(parse_setting_bool("maybe").is_err());
    }

    /// A malformed command answers with the same usage line `:help` shows.
    #[test]
    fn usage_errors_come_from_the_command_table() {
        for raw in [
            ":connect",
            ":contact-add bob",
            ":party-dm alice",
            ":set theme",
            ":party-channel",
            ":party-channel-access #ops",
        ] {
            let err = parse_command(raw).unwrap_err();
            let (_, usage, _) = command_help(raw.split_whitespace().next().unwrap()).unwrap();
            assert_eq!(err, format!("Usage: {usage}"), "for {raw}");
        }
    }
}
