//! Connection commands: hosting, connecting (direct and relayed), the connection password and the hosting lock.
//!
//! Bodies of `TuiApp::execute_command`, one method per command, so the
//! dispatcher reads as a table.

use super::*;

impl TuiApp {
    pub(super) async fn cmd_host(&mut self, port_override: Option<u16>) {
        let port = port_override.unwrap_or(self.chat_manager.config.listen_port);
        let Some(privkey) = self.unlocked_private_key() else {
            return;
        };
        match self.chat_manager.start_host(port, privkey).await {
            Ok(_) => self.toast(ToastLevel::Success, format!("Hosting on :{}", port)),
            Err(e) => self.toast(ToastLevel::Error, format!("Failed to host: {}", e)),
        }
    }

    pub(super) async fn cmd_connect(&mut self, host: String, port: u16) {
        let Some(privkey) = self.unlocked_private_key() else {
            return;
        };
        match self
            .chat_manager
            .connect_to_host(&host, port, None, privkey)
            .await
        {
            Ok(chat_id) => {
                self.toast(
                    ToastLevel::Info,
                    format!("Connecting to {}:{}…", host, port),
                );
                self.select_chat(chat_id);
            }
            Err(e) => self.toast(ToastLevel::Error, format!("Connect failed: {}", e)),
        }
    }

    pub(super) async fn cmd_host_relay(&mut self, relay: String, token: Option<String>) {
        let Some(privkey) = self.unlocked_private_key() else {
            return;
        };
        match self
            .chat_manager
            .start_host_via_relay(&relay, token, privkey)
            .await
        {
            Ok((chat_id, relay_token)) => {
                if let Ok(link) = self.identity.generate_signed_invite_link_with_route(
                    None,
                    Some(relay.clone()),
                    Some(relay_token.clone()),
                ) {
                    Self::copy_to_clipboard(&link);
                    self.overlay = TuiOverlay::Invite { link };
                }
                self.toast(
                    ToastLevel::Success,
                    format!("Relay host ready (token {})", relay_token),
                );
                self.select_chat(chat_id);
            }
            Err(e) => self.toast(ToastLevel::Error, format!("Relay host failed: {}", e)),
        }
    }

    pub(super) async fn cmd_connect_relay(&mut self, relay: String, token: String) {
        let Some(privkey) = self.unlocked_private_key() else {
            return;
        };
        match self
            .chat_manager
            .connect_via_relay(&relay, &token, None, privkey)
            .await
        {
            Ok(chat_id) => {
                self.toast(ToastLevel::Info, format!("Connecting via relay {}…", relay));
                self.select_chat(chat_id);
            }
            Err(e) => self.toast(ToastLevel::Error, format!("Relay connect failed: {}", e)),
        }
    }

    pub(super) fn cmd_connection_password(&mut self, pw: Option<String>) {
        let set = pw.as_ref().map(|p| !p.is_empty()).unwrap_or(false);
        self.chat_manager.set_connection_password(pw);
        if set {
            self.toast(
                ToastLevel::Success,
                "Connection password set (applies to next host/connect)".to_string(),
            );
        } else {
            self.toast(ToastLevel::Info, "Connection password cleared".to_string());
        }
    }

    pub(super) fn cmd_lock(&mut self, on: bool) {
        self.chat_manager.set_conversation_locked(on);
        if on {
            self.chat_manager.stop_hosting();
            self.toast(
                ToastLevel::Warning,
                "Conversation locked — no new connections accepted".to_string(),
            );
        } else {
            self.toast(ToastLevel::Info, "Conversation unlocked".to_string());
        }
    }
}
