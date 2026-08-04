//! Contacts and invite links.
use crate::*;

// ── Contacts + invite links ─────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct ContactDto {
    id: String,
    name: String,
    fingerprint: Option<String>,
    address: Option<String>,
    trust: &'static str,
    /// Whether this contact can be dialled at all. A contact imported from a
    /// relay invite has no direct address but *is* reachable through its relay
    /// token — the UI used to grey out Connect for exactly those, because it
    /// only knew about `address`.
    reachable: bool,
    /// True when the only way to reach them is the relay, so the UI can say
    /// "via relay" instead of showing an empty address line.
    relay_only: bool,
    /// Deleting this contact would lift a block, letting them connect again.
    /// The confirmation dialog has to be able to say so.
    blocked: bool,
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
    }
}

#[tauri::command]
pub(crate) async fn list_contacts(
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<ContactDto>, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    Ok(mgr.contacts.values().map(contact_dto).collect())
}

/// The current user's signed invite link. Embeds every reachable candidate
/// address in priority order — the UPnP external mapping first (reachable
/// from outside the LAN), then the local IP + configured listen port — so
/// peers try them in turn.
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
    let id = crate::lock_identity(&state.identity);
    id.generate_signed_invite_link_with_addresses(addresses, None, None)
        .map_err(|e| e.to_string())
}

/// Parse an invite link and store it as a contact.
///
/// The result carries `signed`: a v1 link has no signature at all, so anyone can
/// mint one naming anybody. That does not make the contact dangerous by itself
/// — an imported contact starts `Unverified` and still has to pass the safety
/// code on first connection — but the UI says so rather than importing both
/// kinds of link with the same silent success.
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
    // Contacts are not covered by the poll loop's change signature, so an
    // import that is not saved here survives only until the next crash.
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(ImportedContactDto {
        contact: dto,
        signed,
    })
}

/// Delete a saved contact (its conversations and history are kept).
///
/// This also clears the peer's stored fingerprint, so the confirmation dialog's
/// promise — that they will have to be verified again — is actually true.
#[tauri::command]
pub(crate) async fn remove_contact(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state.manager.lock().await.remove_contact(uuid);
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(())
}

/// Block a contact: live sessions with it are dropped and future connection
/// attempts from its fingerprint are refused automatically.
#[tauri::command]
pub(crate) async fn block_contact(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
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
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .unblock_contact(uuid)
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(())
}

/// Dial a stored contact.
///
/// Goes through `ChatManager::connect_to_contact` rather than dialling the
/// saved address directly. That method is what keeps a contact in ONE
/// conversation: it reuses the chat already mapped to the contact (or matched
/// by fingerprint), so reconnecting continues the thread instead of opening
/// another "Peer ab12cd34" and fragmenting the history. Dialling `connect_to_host`
/// here also skipped the blocked-contact check, every candidate address after
/// the first, the relay fallback, and the contact↔chat association.
#[tauri::command]
pub(crate) async fn connect_contact(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let pk = {
        let id = crate::lock_identity(&state.identity);
        id.private_key().map_err(|e| e.to_string())?
    };
    state
        .manager
        .lock()
        .await
        .connect_to_contact(uuid, None, &pk)
        .await
        .map(|_chat_id| ())
        .map_err(|e| e.to_string())
}
