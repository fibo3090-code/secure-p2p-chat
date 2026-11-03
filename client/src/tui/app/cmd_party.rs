//! Community (Party server) commands.
//!
//! Bodies of `TuiApp::execute_command`, one method per command, so the
//! dispatcher reads as a table.

use super::*;

impl TuiApp {
    pub(super) async fn cmd_party_connect(
        &mut self,
        address: String,
        username: String,
        password: Option<String>,
    ) {
        let Some(privkey) = self.unlocked_private_key() else {
            return;
        };
        match self
            .party_manager
            // No expected fingerprint yet: this stops before the credentials
            // are sent and returns the server's fingerprint for the user to
            // check. `:party-trust` then passes back exactly that fingerprint,
            // so the connection that carries the password must present it.
            .connect_and_join(&address, &username, password.clone(), &privkey, None)
            .await
        {
            Ok(PartyJoinOutcome::Joining { server_id, .. }) => {
                self.current_party = Some(server_id);
                let fp = self
                    .party_manager
                    .server(server_id)
                    .map(|s| s.server_fingerprint.clone())
                    .unwrap_or_default();
                let short = &fp[..fp.len().min(16)];
                self.toast(
                    ToastLevel::Info,
                    format!("Joining Party {address} as {username} (server fp {short}…)"),
                );
            }
            Ok(PartyJoinOutcome::NeedsVerification { fingerprint, .. }) => {
                // The full fingerprint, grouped for reading aloud: it is what
                // the community server prints when it starts, so it is the
                // one thing the operator can actually compare.
                let grouped = fingerprint
                    .as_bytes()
                    .chunks(8)
                    .map(|c| String::from_utf8_lossy(c).into_owned())
                    .collect::<Vec<_>>()
                    .join(" ");
                self.toast(
                    ToastLevel::Warning,
                    format!(
                        "Verify {address} before joining. Its fingerprint is {grouped}. \
                         Check it matches the one the operator's server printed at \
                         startup, then run :party-trust. Nothing has been sent yet."
                    ),
                );
                self.pending_party_join = Some(PendingPartyJoin {
                    address,
                    username,
                    password,
                    fingerprint,
                });
            }
            Err(e) => self.toast(ToastLevel::Error, format!("Party connect failed: {e}")),
        }
    }

    pub(super) async fn cmd_party_trust(&mut self) {
        if !self.require_unlocked() {
            return;
        }
        let Some(pending) = self.pending_party_join.take() else {
            self.toast(
                ToastLevel::Error,
                "No community server is waiting to be verified.".to_string(),
            );
            return;
        };
        let Some(privkey) = self.private_key() else {
            return;
        };
        match self
            .party_manager
            .connect_and_join(
                &pending.address,
                &pending.username,
                pending.password,
                &privkey,
                // Exactly the fingerprint the user was shown: the connection
                // that carries the credentials must present it, or nothing is
                // sent.
                Some(&pending.fingerprint),
            )
            .await
        {
            Ok(PartyJoinOutcome::Joining { server_id, .. }) => {
                self.current_party = Some(server_id);
                self.toast(
                    ToastLevel::Info,
                    format!("Joining Party {} as {}", pending.address, pending.username),
                );
            }
            // With an expected fingerprint the manager never asks again; if it
            // ever did, joining without a verified key is the wrong answer.
            Ok(PartyJoinOutcome::NeedsVerification { .. }) => self.toast(
                ToastLevel::Error,
                "The server asked to be verified again; nothing was sent.".to_string(),
            ),
            Err(e) => self.toast(ToastLevel::Error, format!("Party connect failed: {e}")),
        }
    }

    pub(super) fn cmd_party_post(&mut self, text: String) {
        let Some(server_id) = self.current_party else {
            self.toast(
                ToastLevel::Error,
                "No Party server joined. Use :party-connect".to_string(),
            );
            return;
        };
        let channel = self
            .party_manager
            .server(server_id)
            .and_then(|s| s.channels.first().map(|c| c.id));
        match channel {
            Some(ch) => {
                if let Err(e) = self.party_manager.post(server_id, ch, text) {
                    self.toast(ToastLevel::Error, format!("Party post failed: {e}"));
                }
            }
            None => self.toast(
                ToastLevel::Warning,
                "No channel available yet; try again in a moment".to_string(),
            ),
        }
    }

    pub(super) fn cmd_party_dm(&mut self, target: String, text: String) {
        let Some(server_id) = self.current_party else {
            self.toast(
                ToastLevel::Error,
                "No Party server joined. Use :party-connect".to_string(),
            );
            return;
        };
        // Resolve the target to a member id: `#N` (1-based) or a username.
        let member = self.party_manager.server(server_id).and_then(|s| {
            if let Some(idx) = target
                .strip_prefix('#')
                .and_then(|n| n.parse::<usize>().ok())
            {
                s.members.get(idx.wrapping_sub(1)).map(|m| m.id)
            } else {
                s.members
                    .iter()
                    .find(|m| m.username.eq_ignore_ascii_case(&target))
                    .map(|m| m.id)
            }
        });
        match member {
            Some(mid) => {
                if let Err(e) = self.party_manager.send_dm(server_id, mid, text) {
                    self.toast(ToastLevel::Error, format!("DM failed: {e}"));
                }
            }
            None => self.toast(
                ToastLevel::Error,
                format!("No member '{target}' on this server (try :party-status)"),
            ),
        }
    }

    pub(super) fn cmd_party_create_channel(&mut self, name: String) {
        let Some(server_id) = self.current_party else {
            self.toast(
                ToastLevel::Error,
                "No Party server joined. Use :party-connect".to_string(),
            );
            return;
        };
        match self.party_manager.create_channel(server_id, name.clone()) {
            Ok(()) => self.toast(ToastLevel::Info, format!("Creating channel '{name}'…")),
            Err(e) => self.toast(ToastLevel::Error, format!("Create channel failed: {e}")),
        }
    }

    pub(super) fn cmd_party_send_file(&mut self, path: String) {
        let Some(server_id) = self.current_party else {
            self.toast(
                ToastLevel::Error,
                "No Party server joined. Use :party-connect".to_string(),
            );
            return;
        };
        let Some(channel) = self
            .party_manager
            .server(server_id)
            .and_then(|s| s.channels.first().map(|c| c.id))
        else {
            self.toast(ToastLevel::Error, "Party has no channels yet".to_string());
            return;
        };
        let p = std::path::Path::new(&path);
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let mime = crate::util::guess_mime(p).to_string();
        match std::fs::read(p) {
            Ok(data) => {
                match self
                    .party_manager
                    .send_file(server_id, channel, name.clone(), mime, data)
                {
                    Ok(()) => self.toast(ToastLevel::Info, format!("Sharing '{name}'…")),
                    Err(e) => self.toast(ToastLevel::Error, format!("Share failed: {e}")),
                }
            }
            Err(e) => self.toast(ToastLevel::Error, format!("Could not read {path}: {e}")),
        }
    }

    pub(super) fn cmd_party_download(&mut self, query: String) {
        let Some(server_id) = self.current_party else {
            self.toast(
                ToastLevel::Error,
                "No Party server joined. Use :party-connect".to_string(),
            );
            return;
        };
        // Match a shared file by name substring or content-hash prefix
        // across everything this client has seen on the server.
        let q = query.to_ascii_lowercase();
        let meta = self.party_manager.server(server_id).and_then(|s| {
            s.messages
                .values()
                .flatten()
                .find_map(|env| match &env.payload {
                    messenger_core::party::MessagePayload::File(f)
                        if f.name.to_ascii_lowercase().contains(&q) || f.hash.starts_with(&q) =>
                    {
                        Some(f.clone())
                    }
                    _ => None,
                })
        });
        match meta {
            Some(f) => match self
                .party_manager
                .request_download(server_id, f.hash.clone())
            {
                Ok(rx) => {
                    self.pending_party_downloads.push((rx, f.name.clone()));
                    self.toast(ToastLevel::Info, format!("Downloading '{}'…", f.name));
                }
                Err(e) => self.toast(ToastLevel::Error, format!("Download failed: {e}")),
            },
            None => self.toast(
                ToastLevel::Error,
                format!("No shared file matches '{query}'"),
            ),
        }
    }

    pub(super) fn cmd_party_status(&mut self) {
        let ids = self.party_manager.server_ids();
        if ids.is_empty() {
            self.toast(ToastLevel::Info, "No Party servers joined".to_string());
        }
        for id in ids {
            if let Some(s) = self.party_manager.server(id) {
                let name = if s.server_name.is_empty() {
                    "…"
                } else {
                    &s.server_name
                };
                self.toast(
                    ToastLevel::Info,
                    format!(
                        "Party {} [{}] {:?} — {} channels, {} members",
                        s.address,
                        name,
                        s.status,
                        s.channels.len(),
                        s.members.len()
                    ),
                );
            }
        }
    }

    pub(super) fn cmd_party_pane(&mut self) {
        // Ask for the file listing as the pane opens, so it is populated
        // by the time the first redraw lands rather than after a manual
        // refresh nobody would think to run.
        if let Some(id) = self.current_party {
            let _ = self.party_manager.refresh_files(id);
        }
        self.overlay = TuiOverlay::Party;
    }

    pub(super) fn cmd_party_files(&mut self) {
        if let Some(id) = self.require_party() {
            match self.party_manager.refresh_files(id) {
                Ok(()) => self.toast(
                    ToastLevel::Info,
                    "Refreshing shared files — see :party".to_string(),
                ),
                Err(e) => self.toast(ToastLevel::Error, e.to_string()),
            }
        }
    }

    pub(super) fn cmd_party_audit(&mut self) {
        if let Some(id) = self.require_party() {
            match self.party_manager.refresh_audit(id, 50) {
                Ok(()) => self.toast(
                    ToastLevel::Info,
                    "Requested the activity log (admins only)".to_string(),
                ),
                Err(e) => self.toast(ToastLevel::Error, e.to_string()),
            }
        }
    }

    pub(super) fn cmd_party_delete_file(&mut self, query: String) {
        if let Some(id) = self.require_party() {
            // Match against the Drive listing, which is the only place
            // that knows *where* a file was shared — a hash alone is not
            // enough, because one blob can be shared in several places.
            let q = query.to_ascii_lowercase();
            let found = self.party_manager.server(id).and_then(|s| {
                s.files
                    .iter()
                    .find(|f| f.name.to_ascii_lowercase().contains(&q) || f.hash.starts_with(&q))
                    .map(|f| (f.hash.clone(), f.location, f.name.clone()))
            });
            match found {
                Some((hash, location, name)) => {
                    match self.party_manager.delete_file(id, hash, location) {
                        Ok(()) => self.toast(ToastLevel::Info, format!("Deleting {name}…")),
                        Err(e) => self.toast(ToastLevel::Error, e.to_string()),
                    }
                }
                None => self.toast(
                    ToastLevel::Error,
                    format!("No shared file matches '{query}'. Run :party-files first."),
                ),
            }
        }
    }

    pub(super) fn cmd_party_role(&mut self, username: String, role: String) {
        if let Some(id) = self.require_party() {
            let parsed = match role.as_str() {
                "guest" => Some(messenger_core::party::Role::Guest),
                "member" => Some(messenger_core::party::Role::Member),
                "admin" => Some(messenger_core::party::Role::Admin),
                _ => None,
            };
            let Some(parsed) = parsed else {
                self.toast(
                    ToastLevel::Error,
                    format!("Unknown role '{role}' — use guest, member or admin"),
                );
                return;
            };
            let target = self.party_manager.server(id).and_then(|s| {
                s.members
                    .iter()
                    .find(|m| m.username.eq_ignore_ascii_case(&username))
                    .map(|m| m.id)
            });
            match target {
                Some(member) => match self.party_manager.set_role(id, member, parsed) {
                    Ok(()) => self.toast(
                        ToastLevel::Info,
                        format!("Setting {username} to {}…", parsed.label()),
                    ),
                    Err(e) => self.toast(ToastLevel::Error, e.to_string()),
                },
                None => self.toast(ToastLevel::Error, format!("No member called '{username}'")),
            }
        }
    }

    pub(super) fn cmd_party_channel_access(
        &mut self,
        channel: String,
        kind: String,
        members: Vec<String>,
    ) {
        if let Some(id) = self.require_party() {
            let parsed_kind = match kind.as_str() {
                "public" => Some(messenger_core::party::ChannelKind::Public),
                "private" => Some(messenger_core::party::ChannelKind::Private),
                "locked" => Some(messenger_core::party::ChannelKind::Locked),
                "announce" => Some(messenger_core::party::ChannelKind::Announce),
                _ => None,
            };
            let Some(parsed_kind) = parsed_kind else {
                self.toast(
                    ToastLevel::Error,
                    format!("Unknown kind '{kind}' — use public, private, locked or announce"),
                );
                return;
            };
            let resolved = self.party_manager.server(id).map(|s| {
                let chan = s
                    .channels
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(&channel))
                    .map(|c| c.id);
                let ids: Vec<uuid::Uuid> = members
                    .iter()
                    .filter_map(|name| {
                        s.members
                            .iter()
                            .find(|m| m.username.eq_ignore_ascii_case(name))
                            .map(|m| m.id)
                    })
                    .collect();
                (chan, ids)
            });
            match resolved {
                Some((Some(chan), ids)) => {
                    match self
                        .party_manager
                        .set_channel_access(id, chan, parsed_kind, ids)
                    {
                        Ok(()) => self.toast(
                            ToastLevel::Info,
                            format!("Setting #{channel} to {}…", parsed_kind.label()),
                        ),
                        Err(e) => self.toast(ToastLevel::Error, e.to_string()),
                    }
                }
                _ => self.toast(ToastLevel::Error, format!("No channel called '#{channel}'")),
            }
        }
    }

    pub(super) fn cmd_party_delete_channel(&mut self, channel: String) {
        if let Some(id) = self.require_party() {
            let chan = self.party_manager.server(id).and_then(|s| {
                s.channels
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(&channel))
                    .map(|c| c.id)
            });
            match chan {
                Some(chan) => match self.party_manager.delete_channel(id, chan) {
                    Ok(()) => self.toast(ToastLevel::Info, format!("Deleting #{channel}…")),
                    Err(e) => self.toast(ToastLevel::Error, e.to_string()),
                },
                None => self.toast(ToastLevel::Error, format!("No channel called '#{channel}'")),
            }
        }
    }
}
