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
    ContactDto {
        id: c.id.to_string(),
        name: c.name.clone(),
        fingerprint: c.fingerprint.clone(),
        address: c.address.clone(),
        trust: trust_str(&c.trust_state),
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

/// The current user's signed invite link (address derived from the local IP +
/// the configured listen port, when resolvable).
#[tauri::command]
pub(crate) async fn my_invite_link(state: tauri::State<'_, Bridge>) -> Result<String, String> {
    ensure_ready(&state)?;
    let address = {
        let mgr = state.manager.lock().await;
        let port = mgr.config.listen_port;
        messenger_core::util::primary_local_ipv4()
            .map(|ip| messenger_core::util::format_host_port(&ip, port))
    };
    let id = state.identity.lock().unwrap();
    id.generate_signed_invite_link(address)
        .map_err(|e| e.to_string())
}

/// Parse an invite link and store it as a contact.
#[tauri::command]
pub(crate) async fn import_invite(
    link: String,
    state: tauri::State<'_, Bridge>,
) -> Result<ContactDto, String> {
    ensure_ready(&state)?;
    let contact = {
        let mgr = state.manager.lock().await;
        mgr.parse_invite_link(&link).map_err(|e| e.to_string())?
    };
    let mut mgr = state.manager.lock().await;
    let id = mgr.import_contact(contact);
    Ok(contact_dto(mgr.get_contact(id).expect("just inserted")))
}

/// Dial a stored contact by its saved address.
#[tauri::command]
pub(crate) async fn connect_contact(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let address = {
        let mgr = state.manager.lock().await;
        mgr.get_contact(uuid).and_then(|c| c.address.clone())
    };
    let address = address.ok_or_else(|| "Contact has no saved address".to_string())?;
    let (host, port) =
        messenger_core::util::parse_host_port(&address, Some(messenger_core::PORT_DEFAULT))
            .map_err(|e| e.to_string())?;
    let pk = {
        let id = state.identity.lock().unwrap();
        id.private_key().map_err(|e| e.to_string())?
    };
    state
        .manager
        .lock()
        .await
        .connect_to_host(&host, port, None, pk)
        .await
        .map(|_chat_id| ())
        .map_err(|e| e.to_string())
}
