//! Contacts and invite links.
use crate::*;

#[derive(Serialize)]
pub(crate) struct ContactDto {
    id: String,
    name: String,
    fingerprint: Option<String>,
    address: Option<String>,
    trust: &'static str,
    /// Dialable at all: a relay-invite contact has no address but a token.
    reachable: bool,
    /// The relay is the only route, so the UI can say "via relay".
    relay_only: bool,
    /// Deleting this contact would lift a block; the dialog must say so.
    blocked: bool,
    /// The conversation this contact is already associated with, if any, so a
    /// conversation that went offline can offer "Reconnect" and a contact row
    /// can open its existing thread instead of starting another.
    chat_id: Option<String>,
}

/// An imported invite plus whether the link that produced it was signed.
#[derive(Serialize)]
pub(crate) struct ImportedContactDto {
    contact: ContactDto,
    signed: bool,
}

fn trust_str(t: &messenger_core::types::TrustState) -> &'static str {
    use messenger_core::types::TrustState::*;
    match t {
        Unverified => "unverified",
        Verified => "verified",
        Trusted => "trusted",
        Blocked => "blocked",
    }
}

fn contact_dto(c: &messenger_core::types::Contact) -> ContactDto {
    let has_relay = c.relay_server.is_some() && c.relay_token.is_some();
    let has_direct = c.address.is_some() || !c.addresses.is_empty();
    ContactDto {
        id: c.id.to_string(),
        name: c.name.clone(),
        fingerprint: c.fingerprint.clone(),
        address: c.address.clone(),
        trust: trust_str(&c.trust_state),
        reachable: has_direct || has_relay,
        relay_only: has_relay && !has_direct,
        blocked: c.trust_state == messenger_core::types::TrustState::Blocked,
        chat_id: None,
    }
}

#[tauri::command]
pub(crate) async fn list_contacts(
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<ContactDto>, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    Ok(mgr
        .contacts
        .values()
        .map(|c| ContactDto {
            chat_id: mgr.contact_to_chat.get(&c.id).map(|id| id.to_string()),
            ..contact_dto(c)
        })
        .collect())
}

/// The user's signed invite link, with every candidate address in priority
/// order: the UPnP external mapping first, then LAN IP + listen port.
#[tauri::command]
pub(crate) async fn my_invite_link(state: tauri::State<'_, Bridge>) -> Result<String, String> {
    ensure_ready(&state)?;
    let addresses = {
        let mgr = state.manager.lock().await;
        let port = mgr.config.listen_port;
        let mut addrs = Vec::new();
        if let Some(ext) = mgr.external_address.clone() {
            addrs.push(ext);
        }
        if let Some(ip) = messenger_core::util::primary_local_ipv4() {
            let lan = messenger_core::util::format_host_port(&ip, port);
            if !addrs.contains(&lan) {
                addrs.push(lan);
            }
        }
        addrs
    };
    let id = lock_or_recover(&state.identity);
    id.generate_signed_invite_link_with_addresses(addresses, None, None)
        .map_err(|e| e.to_string())
}

/// Parse an invite link and store it as a contact. `signed` is false for a v1
/// link, which anyone can mint; the contact still starts `Unverified`, but the
/// UI warns.
#[tauri::command]
pub(crate) async fn import_invite(
    link: String,
    state: tauri::State<'_, Bridge>,
) -> Result<ImportedContactDto, String> {
    ensure_ready(&state)?;
    let signed = ChatManager::invite_link_is_signed(&link);
    let mut mgr = state.manager.lock().await;
    let contact = mgr.parse_invite_link(&link).map_err(|e| e.to_string())?;
    let id = mgr.import_contact(contact).map_err(|e| e.to_string())?;
    let dto = contact_dto(mgr.get_contact(id).expect("just inserted"));
    drop(mgr);
    // Contact mutations save immediately.
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(ImportedContactDto {
        contact: dto,
        signed,
    })
}

/// Delete a saved contact (conversations are kept). Also clears the peer's
/// stored fingerprint, so they must be verified again.
#[tauri::command]
pub(crate) async fn remove_contact(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = parse_id(&id)?;
    state.manager.lock().await.remove_contact(uuid);
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(())
}

/// Block a contact: live sessions drop and its fingerprint is refused.
#[tauri::command]
pub(crate) async fn block_contact(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = parse_id(&id)?;
    state
        .manager
        .lock()
        .await
        .block_contact(uuid)
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(())
}

/// Undo a block; trust returns to Verified when the fingerprint was already
/// confirmed in a conversation, otherwise Unverified.
#[tauri::command]
pub(crate) async fn unblock_contact(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = parse_id(&id)?;
    state
        .manager
        .lock()
        .await
        .unblock_contact(uuid)
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(())
}

/// Dial a stored contact via `connect_to_contact`, which keeps the contact in
/// ONE conversation and applies the blocked check, every candidate address and
/// the relay fallback. Never dial `connect_to_host` directly here.
#[tauri::command]
pub(crate) async fn connect_contact(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = parse_id(&id)?;
    let pk = lock_or_recover(&state.identity)
        .private_key()
        .map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .connect_to_contact(uuid, None, &pk)
        .await
        .map(|_chat_id| ())
        .map_err(|e| e.to_string())
}
