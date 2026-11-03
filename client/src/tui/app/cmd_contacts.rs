//! Contact and invite commands.
//!
//! Bodies of `TuiApp::execute_command`, one method per command, so the
//! dispatcher reads as a table.

use super::*;

impl TuiApp {
    pub(super) fn cmd_contact_add(
        &mut self,
        name: String,
        address: String,
        fingerprint: Option<String>,
    ) {
        if let Err(e) = ChatManager::parse_address(&address) {
            self.toast(ToastLevel::Error, format!("Bad address: {}", e));
            return;
        }
        self.chat_manager
            .add_contact(name.clone(), Some(address), fingerprint, None);
        self.toast(ToastLevel::Success, format!("Contact '{}' added", name));
        self.save_history_now();
    }

    pub(super) async fn cmd_contact_connect(&mut self, target: String) {
        if !self.require_unlocked() {
            return;
        }
        self.sync_contact_ids();
        let Some(contact_id) = self.resolve_contact(&target) else {
            self.toast(ToastLevel::Error, format!("No contact '{}'", target));
            return;
        };
        let Some(privkey) = self.private_key() else {
            return;
        };
        match self
            .chat_manager
            .connect_to_contact(contact_id, None, &privkey)
            .await
        {
            Ok(chat_id) => {
                self.toast(ToastLevel::Info, "Connecting to contact…".to_string());
                self.overlay = TuiOverlay::None;
                self.select_chat(chat_id);
            }
            Err(e) => self.toast(ToastLevel::Error, format!("Connect failed: {}", e)),
        }
    }

    pub(super) fn cmd_contact_remove(&mut self, target: String) {
        self.sync_contact_ids();
        match self.resolve_contact(&target) {
            Some(id) => {
                self.chat_manager.remove_contact(id);
                self.sync_contact_ids();
                self.toast(ToastLevel::Info, "Contact removed".to_string());
                self.save_history_now();
            }
            None => self.toast(ToastLevel::Error, format!("No contact '{}'", target)),
        }
    }

    pub(super) fn cmd_contact_rename(&mut self, target: String, new_name: String) {
        self.sync_contact_ids();
        match self.resolve_contact(&target) {
            Some(id) => {
                if let Some(c) = self.chat_manager.contacts.get_mut(&id) {
                    c.name = new_name.chars().take(50).collect();
                }
                self.toast(ToastLevel::Success, "Contact renamed".to_string());
                self.save_history_now();
            }
            None => self.toast(ToastLevel::Error, format!("No contact '{}'", target)),
        }
    }

    pub(super) fn cmd_invite(&mut self, addr: Option<String>) {
        // Explicit address wins; otherwise embed every reachable
        // candidate in priority order (UPnP external first, LAN next)
        // so peers can try them in turn.
        let addrs: Vec<String> = match addr {
            Some(a) => vec![a],
            None => {
                let mut v = Vec::new();
                if let Some(ext) = self.chat_manager.external_address.clone() {
                    v.push(ext);
                }
                if let Some(ip) = crate::util::primary_local_ipv4() {
                    let lan = format!("{}:{}", ip, self.chat_manager.config.listen_port);
                    if !v.contains(&lan) {
                        v.push(lan);
                    }
                }
                v
            }
        };
        match self
            .identity
            .generate_signed_invite_link_with_addresses(addrs, None, None)
        {
            Ok(link) => {
                Self::copy_to_clipboard(&link);
                self.overlay = TuiOverlay::Invite { link };
                self.toast(ToastLevel::Success, "Invite link copied".to_string());
            }
            Err(e) => self.toast(ToastLevel::Error, format!("Invite failed: {}", e)),
        }
    }

    pub(super) fn cmd_import(&mut self, link: String) {
        match self.chat_manager.parse_invite_link(&link) {
            Ok(contact) => {
                let name = contact.name.clone();
                if let Err(e) = self.chat_manager.import_contact(contact) {
                    self.toast(ToastLevel::Error, e.to_string());
                    return;
                }
                self.sync_contact_ids();
                self.toast(
                    ToastLevel::Success,
                    format!("Imported '{}'. Use :contact-connect {}", name, name),
                );
                self.save_history_now();
            }
            Err(e) => self.toast(ToastLevel::Error, format!("Import failed: {}", e)),
        }
    }
}
