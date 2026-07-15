//! Communities (Party servers): join/leave, channels, DMs, file sharing,
//! and the saved-communities file (parties.json) with fingerprint pinning.
use crate::*;

// ── Communities (Party servers) ─────────────────────────────────────────────
//
// A thin bridge over `PartyManager`, mirroring the egui Party tab. Command
// params are single words (`server`, `channel`, …) to avoid the Tauri 2
// arg-naming footgun where a snake_case param silently no-ops.

#[derive(Serialize)]
pub(crate) struct PartyMemberDto {
    id: String,
    username: String,
    online: bool,
    is_me: bool,
    /// Message count in my DM thread with this member (0 when not joined yet).
    /// The frontend derives DM unread badges from it.
    dm_messages: usize,
}

#[derive(Serialize)]
pub(crate) struct PartyChannelDto {
    id: String,
    name: String,
    /// Message count in this channel; the frontend derives unread badges from it.
    messages: usize,
}

#[derive(Serialize)]
pub(crate) struct PartyServerDto {
    id: String,
    name: String,
    address: String,
    /// The username this client joined with (for rejoin flows).
    username: String,
    fingerprint: String,
    status: &'static str,
    status_detail: Option<String>,
    member_id: Option<String>,
    channels: Vec<PartyChannelDto>,
    members: Vec<PartyMemberDto>,
    last_error: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct PartyMessageDto {
    sender_name: String,
    from_me: bool,
    kind: &'static str,
    text: String,
    size: Option<u64>,
    /// Content hash of a file message, used to request its download. `None` for text.
    hash: Option<String>,
    timestamp: u64,
}

fn party_status_parts(status: &PartyStatus) -> (&'static str, Option<String>) {
    match status {
        PartyStatus::Connecting => ("connecting", None),
        PartyStatus::Joined => ("joined", None),
        PartyStatus::Rejected(reason) => ("rejected", Some(reason.clone())),
        PartyStatus::Disconnected => ("disconnected", None),
    }
}

/// Serialize one connection's directory (channels + members), resolving "you".
fn server_dto(
    id: Uuid,
    conn: &p2pem_classic::app::party_manager::PartyServerConn,
) -> PartyServerDto {
    let (status, status_detail) = party_status_parts(&conn.status);
    let members = conn
        .members
        .iter()
        .map(|m| PartyMemberDto {
            id: m.id.to_string(),
            username: m.username.clone(),
            online: m.online,
            is_me: conn.member_id == Some(m.id),
            dm_messages: conn
                .member_id
                .map(|me| {
                    let thread = messenger_core::party::dm_thread_id(me, m.id);
                    conn.messages.get(&thread).map_or(0, |v| v.len())
                })
                .unwrap_or(0),
        })
        .collect();
    let channels = conn
        .channels
        .iter()
        .map(|c| PartyChannelDto {
            id: c.id.to_string(),
            name: c.name.clone(),
            messages: conn.messages.get(&c.id).map_or(0, |v| v.len()),
        })
        .collect();
    PartyServerDto {
        id: id.to_string(),
        name: conn.server_name.clone(),
        address: conn.address.clone(),
        username: conn.username.clone(),
        fingerprint: conn.server_fingerprint.clone(),
        status,
        status_detail,
        member_id: conn.member_id.map(|m| m.to_string()),
        channels,
        members,
        last_error: conn.last_error.clone(),
    }
}

/// Turn a stored envelope into a display DTO, resolving the sender's username
/// from the member directory and flagging the local user's own messages.
fn message_dto(
    env: &messenger_core::party::Envelope,
    conn: &p2pem_classic::app::party_manager::PartyServerConn,
) -> PartyMessageDto {
    use messenger_core::party::MessagePayload;
    let sender_name = conn
        .members
        .iter()
        .find(|m| m.id == env.sender)
        .map(|m| m.username.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let (kind, text, size, hash) = match &env.payload {
        MessagePayload::Text(t) => ("text", t.clone(), None, None),
        MessagePayload::File(f) => ("file", f.name.clone(), Some(f.size), Some(f.hash.clone())),
    };
    PartyMessageDto {
        sender_name,
        from_me: conn.member_id == Some(env.sender),
        kind,
        text,
        size,
        hash,
        timestamp: env.timestamp,
    }
}

/// Connect to a community server, verify (TOFU) its fingerprint out of band, and
/// join with a username. Returns the local server id.
#[tauri::command]
pub(crate) async fn party_join(
    address: String,
    username: String,
    password: String,
    state: tauri::State<'_, Bridge>,
) -> Result<String, String> {
    ensure_ready(&state)?;
    let address = address.trim().to_string();
    let username = username.trim().to_string();
    let pk = {
        let id = state.identity.lock().unwrap();
        id.private_key().map_err(|e| e.to_string())?
    };
    let password = Some(password).filter(|p| !p.trim().is_empty());
    let mut party = state.party.lock().await;
    let sid = party
        .connect_and_join(&address, &username, password, &pk)
        .await
        .map_err(|e| e.to_string())?;

    // TOFU pinning: if we've joined this address before, its identity must not
    // have changed. A mismatch is either a redeployed server or an active MITM —
    // refuse and tell the user, never silently trust the new key.
    let fingerprint = party
        .server(sid)
        .map(|c| c.server_fingerprint.clone())
        .unwrap_or_default();
    let saved = load_saved_parties(&state.parties_path);
    if let Some(pinned) = saved
        .iter()
        .find(|p| p.address.eq_ignore_ascii_case(&address) && !p.fingerprint.is_empty())
    {
        if pinned.fingerprint != fingerprint {
            party.remove_server(sid);
            return Err(format!(
                "SECURITY: this server's identity changed since you last joined \
                 (expected {}…, got {}…). If the operator redeployed the server this \
                 may be expected — leave the saved community and rejoin to trust the \
                 new identity. Otherwise, do not proceed.",
                &pinned.fingerprint[..16.min(pinned.fingerprint.len())],
                &fingerprint[..16.min(fingerprint.len())]
            ));
        }
    }
    // Rejoin replaces: drop any older entry for the same address (e.g. a
    // disconnected or join-rejected zombie) so the list never shows duplicates.
    let stale: Vec<Uuid> = party
        .server_ids()
        .into_iter()
        .filter(|id| {
            *id != sid
                && party
                    .server(*id)
                    .is_some_and(|c| c.address.eq_ignore_ascii_case(&address))
        })
        .collect();
    for id in stale {
        party.remove_server(id);
    }
    drop(party);

    upsert_saved_party(
        &state.parties_path,
        SavedParty {
            address,
            username,
            name: String::new(),
            fingerprint,
        },
    );
    Ok(sid.to_string())
}

/// The joined community servers with their channels and member directories.
#[tauri::command]
pub(crate) async fn party_list(
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<PartyServerDto>, String> {
    ensure_ready(&state)?;
    let party = state.party.lock().await;
    let mut out: Vec<PartyServerDto> = party
        .server_ids()
        .into_iter()
        .filter_map(|id| party.server(id).map(|conn| server_dto(id, conn)))
        .collect();
    // Stable order so the UI's server list doesn't reshuffle each poll.
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(out)
}

/// Messages in a channel (already seeded from durable history on join; live
/// posts arrive via the poll loop).
#[tauri::command]
pub(crate) async fn party_history(
    server: String,
    channel: String,
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<PartyMessageDto>, String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let cid = Uuid::parse_str(&channel).map_err(|e| e.to_string())?;
    let party = state.party.lock().await;
    let conn = party
        .server(sid)
        .ok_or_else(|| "unknown server".to_string())?;
    let msgs = conn
        .messages
        .get(&cid)
        .map(|v| v.iter().map(|e| message_dto(e, conn)).collect())
        .unwrap_or_default();
    Ok(msgs)
}

/// Post a text message to a channel.
#[tauri::command]
pub(crate) async fn party_post(
    server: String,
    channel: String,
    text: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let cid = Uuid::parse_str(&channel).map_err(|e| e.to_string())?;
    state
        .party
        .lock()
        .await
        .post(sid, cid, text)
        .map_err(|e| e.to_string())
}

/// Create a new channel on a community server.
#[tauri::command]
pub(crate) async fn party_create_channel(
    server: String,
    name: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    state
        .party
        .lock()
        .await
        .create_channel(sid, name)
        .map_err(|e| e.to_string())
}

/// Send a direct message to another member of a community server.
#[tauri::command]
pub(crate) async fn party_send_dm(
    server: String,
    to: String,
    text: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let to = Uuid::parse_str(&to).map_err(|e| e.to_string())?;
    state
        .party
        .lock()
        .await
        .send_dm(sid, to, text)
        .map_err(|e| e.to_string())
}

/// The DM thread with a member. Requests a fresh fetch (offline catch-up); the
/// authoritative history lands on a later poll.
#[tauri::command]
pub(crate) async fn party_dm_history(
    server: String,
    peer: String,
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<PartyMessageDto>, String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let peer = Uuid::parse_str(&peer).map_err(|e| e.to_string())?;
    let party = state.party.lock().await;
    let _ = party.fetch_dm_history(sid, peer);
    let conn = party
        .server(sid)
        .ok_or_else(|| "unknown server".to_string())?;
    Ok(party
        .dm_messages(sid, peer)
        .iter()
        .map(|e| message_dto(e, conn))
        .collect())
}

/// Clear a server's last surfaced error (e.g. a rejected post) after the UI has
/// shown it.
#[tauri::command]
pub(crate) async fn party_clear_error(
    server: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    state.party.lock().await.clear_server_error(sid);
    Ok(())
}

/// A community this client has joined, persisted (plaintext, no secrets — the
/// password is never stored) so it survives restarts: the UI offers one-click
/// rejoin, and the pinned server fingerprint turns the first join's TOFU into a
/// real trust anchor — a different identity at the same address is refused.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SavedParty {
    pub(crate) address: String,
    pub(crate) username: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) fingerprint: String,
}

/// Load the saved-communities list; missing or unreadable files are an empty list
/// (never an error — this is convenience state, not critical data).
fn load_saved_parties(path: &Path) -> Vec<SavedParty> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Best-effort save of the saved-communities list; errors are logged, not fatal.
fn save_saved_parties(path: &Path, list: &[SavedParty]) {
    match serde_json::to_string_pretty(list) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                tracing::warn!("saving communities list failed: {e}");
            }
        }
        Err(e) => tracing::warn!("serializing communities list failed: {e}"),
    }
}

/// Insert or update the entry for `address` (matched case-insensitively).
pub(crate) fn upsert_saved_party(path: &Path, entry: SavedParty) {
    let mut list = load_saved_parties(path);
    match list
        .iter_mut()
        .find(|p| p.address.eq_ignore_ascii_case(&entry.address))
    {
        Some(existing) => {
            existing.username = entry.username;
            if !entry.name.is_empty() {
                existing.name = entry.name;
            }
            if !entry.fingerprint.is_empty() {
                existing.fingerprint = entry.fingerprint;
            }
        }
        None => list.push(entry),
    }
    save_saved_parties(path, &list);
}

/// The saved communities, for the join screen's one-click rejoin list.
#[tauri::command]
pub(crate) async fn party_saved(
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<SavedParty>, String> {
    ensure_ready(&state)?;
    Ok(load_saved_parties(&state.parties_path))
}

/// Leave a community: drop the connection, forget its local state, and remove it
/// from the saved list. The server keeps the membership, so rejoining with the
/// same identity later resumes it.
#[tauri::command]
pub(crate) async fn party_leave(
    server: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let mut party = state.party.lock().await;
    let address = party.server(sid).map(|c| c.address.clone());
    if !party.remove_server(sid) {
        return Err("unknown server".to_string());
    }
    drop(party);
    if let Some(addr) = address {
        let mut list = load_saved_parties(&state.parties_path);
        list.retain(|p| !p.address.eq_ignore_ascii_case(&addr));
        save_saved_parties(&state.parties_path, &list);
    }
    Ok(())
}

/// Open the native file picker (on a blocking thread) and read the chosen file's
/// bytes, returning `(name, mime, data)`. `Ok(None)` if the user cancelled.
async fn pick_upload() -> Result<Option<(String, String, Vec<u8>)>, String> {
    let picked = tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_file())
        .await
        .map_err(|e| e.to_string())?;
    let Some(path) = picked else {
        return Ok(None);
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let mime = messenger_core::util::guess_mime(&path).to_string();
    let data = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(Some((name, mime, data)))
}

/// Pick a file and post it to a community channel. The picker runs on a blocking
/// thread; a cancelled dialog is a successful no-op.
#[tauri::command]
pub(crate) async fn party_send_file(
    server: String,
    channel: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let cid = Uuid::parse_str(&channel).map_err(|e| e.to_string())?;
    let Some((name, mime, data)) = pick_upload().await? else {
        return Ok(());
    };
    state
        .party
        .lock()
        .await
        .send_file(sid, cid, name, mime, data)
        .map_err(|e| e.to_string())
}

/// Pick a file and send it as a direct message to another community member.
#[tauri::command]
pub(crate) async fn party_send_file_dm(
    server: String,
    to: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let peer = Uuid::parse_str(&to).map_err(|e| e.to_string())?;
    let Some((name, mime, data)) = pick_upload().await? else {
        return Ok(());
    };
    state
        .party
        .lock()
        .await
        .send_file_dm(sid, peer, name, mime, data)
        .map_err(|e| e.to_string())
}

/// Download a community file by content hash and save it via the native dialog.
/// `name` seeds the save dialog. The server only returns bytes the caller is
/// allowed to see (access-checked there). A cancelled save dialog is a no-op.
#[tauri::command]
pub(crate) async fn party_download_file(
    server: String,
    hash: String,
    name: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    // Register the download and drop the party lock BEFORE awaiting, so the poll
    // loop can lock the manager, drain the FileData response, and complete us.
    let rx = state
        .party
        .lock()
        .await
        .request_download(sid, hash)
        .map_err(|e| e.to_string())?;
    // Outer error: the sender was dropped (connection torn down). Inner error: the
    // server refused (file gone / not permitted). Either way, surface a message
    // rather than hang.
    let data = match rx.await {
        Ok(Ok(data)) => data,
        Ok(Err(reason)) => return Err(reason),
        Err(_) => return Err("download failed: the connection closed".to_string()),
    };
    let suggested = name;
    let picked = tokio::task::spawn_blocking(move || {
        rfd::FileDialog::new().set_file_name(suggested).save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(path) = picked else {
        return Ok(()); // user cancelled the save dialog
    };
    std::fs::write(&path, &data).map_err(|e| e.to_string())
}
