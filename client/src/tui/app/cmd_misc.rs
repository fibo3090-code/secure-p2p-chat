//! File, identity and chat-management commands.
//!
//! Bodies of `TuiApp::execute_command`, one method per command, so the
//! dispatcher reads as a table.

use super::*;

impl TuiApp {
    pub(super) async fn cmd_send(&mut self, path: String) {
        let Some(chat_id) = self.selected_chat_id() else {
            self.toast(ToastLevel::Warning, "No chat selected".to_string());
            return;
        };
        match self
            .chat_manager
            .send_file(chat_id, PathBuf::from(path))
            .await
        {
            Ok(()) => {}
            Err(e) => self.toast(ToastLevel::Error, format!("Send file failed: {}", e)),
        }
    }

    pub(super) fn cmd_resolve_file_offer(&mut self, accept: bool) {
        // Prefer an offer in the selected chat, else take any pending one.
        let selected = self.selected_chat_id();
        let mut offers: Vec<_> = self
            .chat_manager
            .active_transfers_snapshot()
            .into_iter()
            .filter(|t| t.status == crate::types::TransferStatus::AwaitingAcceptance)
            .collect();
        offers.sort_by_key(|t| Some(t.chat_id) != selected);
        match offers.first() {
            Some(t) => {
                let result = if accept {
                    self.chat_manager.accept_incoming_file(t.id)
                } else {
                    self.chat_manager.reject_incoming_file(t.id)
                };
                if let Err(e) = result {
                    self.toast(ToastLevel::Error, format!("File offer: {}", e));
                }
            }
            None => self.toast(
                ToastLevel::Warning,
                "No incoming file is awaiting approval".to_string(),
            ),
        }
    }

    pub(super) fn cmd_name(&mut self, name: String) {
        match self.identity.set_name(&name) {
            Ok(()) => match self.identity.save(&self.identity_path) {
                Ok(()) => {
                    // The identity overlay renders this cached copy.
                    self.identity_name = self.identity.name.clone();
                    self.toast(
                        ToastLevel::Success,
                        format!("Display name changed to {}", self.identity.name),
                    )
                }
                Err(e) => self.toast(ToastLevel::Error, format!("Failed to save identity: {}", e)),
            },
            Err(e) => self.toast(ToastLevel::Error, format!("Invalid name: {}", e)),
        }
    }

    pub(super) fn cmd_backup(&mut self, dest: String) {
        let source = self.identity_path.clone();
        if !self.identity.is_encrypted() {
            // A legacy plaintext identity must never be copied around:
            // the backup would contain the raw private key. (The
            // startup password prompt can be dismissed with Esc, so
            // this state is reachable.)
            self.toast(
                ToastLevel::Error,
                "Set a password first (:password) — refusing to export an unencrypted identity"
                    .to_string(),
            );
        } else if !source.exists() {
            self.toast(
                ToastLevel::Error,
                "No identity file to back up yet".to_string(),
            );
        } else {
            match std::fs::copy(&source, &dest) {
                Ok(_) => self.toast(
                    ToastLevel::Success,
                    format!("Identity backup saved to {}", dest),
                ),
                Err(e) => self.toast(ToastLevel::Error, format!("Backup failed: {}", e)),
            }
        }
    }

    pub(super) fn cmd_rename(&mut self, title: String) {
        match self.selected_chat_id() {
            Some(id) => {
                if let Err(e) = self.chat_manager.rename_chat(id, title) {
                    self.toast(ToastLevel::Error, format!("Rename failed: {}", e));
                } else {
                    self.toast(ToastLevel::Success, "Chat renamed".to_string());
                    self.save_history_now();
                }
            }
            None => self.toast(ToastLevel::Warning, "No chat selected".to_string()),
        }
    }

    pub(super) fn cmd_unlock(&mut self, pw: Option<String>) {
        match pw {
            Some(pw) => self.try_unlock(&pw),
            None => {
                self.overlay = TuiOverlay::Password {
                    mode: PasswordMode::Unlock,
                }
            }
        }
    }

    /// `:wipe <password>` — check the password against the identity on disk,
    /// then ask once more (Ctrl+Y) before deleting everything.
    pub(super) fn cmd_wipe(&mut self, password: &str) {
        if !self.identity.is_encrypted() {
            self.toast(
                ToastLevel::Error,
                "Set a password first; :wipe checks it before deleting anything.".to_string(),
            );
            return;
        }
        let checked = Identity::load(&self.identity_path).and_then(|mut id| id.decrypt(password));
        match checked {
            Ok(()) => self.overlay = TuiOverlay::ConfirmWipe,
            Err(_) => self.toast(
                ToastLevel::Error,
                "Wrong password — nothing was deleted".to_string(),
            ),
        }
    }

    /// Delete everything and quit. `ChatManager::delete_all_data` stops
    /// hosting and sessions and refuses every later save, so the save on the
    /// way out cannot write a history file back.
    pub(super) fn wipe_now(&mut self) {
        for id in self.party_manager.server_ids() {
            self.party_manager.remove_server(id);
        }
        let data_dir = self
            .history_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let (history, identity) = (self.history_path.clone(), self.identity_path.clone());
        match self
            .chat_manager
            .delete_all_data(&data_dir, &history, &identity)
        {
            Ok(()) => self.should_quit = true,
            Err(e) => self.toast(
                ToastLevel::Error,
                format!("Deleting data failed part-way: {e}"),
            ),
        }
    }

    /// `:history-reset` — move an unreadable history aside (renamed, never
    /// deleted) so saving can resume.
    pub(super) fn cmd_history_reset(&mut self) {
        if self.chat_manager.history_load_error().is_none() {
            self.toast(
                ToastLevel::Info,
                "Your history loaded normally; there is nothing to reset.".to_string(),
            );
            return;
        }
        let path = self.history_path.clone();
        match self.chat_manager.set_aside_unreadable_history(&path) {
            Ok(moved) => {
                self.save_history_now();
                let note = moved
                    .map(|p| format!("Old history kept as {}", p.display()))
                    .unwrap_or_else(|| "Started a new history".to_string());
                self.toast(ToastLevel::Success, note);
            }
            Err(e) => self.toast(
                ToastLevel::Error,
                format!("Could not move the unreadable history aside: {e}"),
            ),
        }
    }
}
