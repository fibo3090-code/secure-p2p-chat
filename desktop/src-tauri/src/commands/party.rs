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

/// What `party_join` did. `status: "joined"` means the credentials went out and
/// the join is in flight; `status: "verify"` means nothing was sent yet and the
/// user has to confirm the server's identity first.
#[derive(Serialize)]
pub(crate) struct PartyJoinDto {
    status: &'static str,
    server: Option<String>,
    fingerprint: String,
    sas: Option<String>,
}

/// Connect to a community server and join with a username.
///
/// Two-step on purpose. The `Join` frame carries the community username *and
/// password*, so it must not be written to a server whose identity the user has
/// not accepted. If this address has a pin, it is compared before the frame goes
/// out. If it has none — a first join — the connection is made far enough to
/// learn the server's fingerprint and SAS, then dropped, and the UI is asked to
/// show them. Calling again with `trust: true` completes it. Previously the
/// first join handed the credentials to whatever key answered and pinned it
/// afterwards, which is trust-on-first-use without the trust step.
#[tauri::command]
pub(crate) async fn party_join(
    address: String,
    username: String,
    password: String,
    trust: Option<bool>,
    state: tauri::State<'_, Bridge>,
) -> Result<PartyJoinDto, String> {
    ensure_ready(&state)?;
    let address = address.trim().to_string();
    let username = username.trim().to_string();
    let pk = {
        let id = crate::lock_identity(&state.identity);
        id.private_key().map_err(|e| e.to_string())?
    };
    let password = Some(password).filter(|p| !p.trim().is_empty());

    // TOFU pinning: if we've joined this address before, its identity must not
    // have changed. Look the pin up BEFORE connecting and hand it down, so the
    // comparison happens inside `connect_and_join` while the tunnel is up but
    // before the `Join` frame carrying the username and password is written.
    let saved = load_saved_parties(&state.parties_path)?;
    let pinned = saved
        .into_iter()
        .find(|p| p.address.eq_ignore_ascii_case(&address) && !p.fingerprint.is_empty())
        .map(|p| p.fingerprint);

    let mut party = state.party.lock().await;
    let outcome = party
        .connect_and_join(
            &address,
            &username,
            password,
            &pk,
            pinned.as_deref(),
            trust.unwrap_or(false),
        )
        .await
        .map_err(|e| e.to_string())?;

    let (sid, fingerprint) = match outcome {
        PartyJoinOutcome::NeedsVerification { fingerprint, sas } => {
            return Ok(PartyJoinDto {
                status: "verify",
                server: None,
                fingerprint,
                sas: Some(sas),
            });
        }
        PartyJoinOutcome::Joining {
            server_id,
            fingerprint,
        } => (server_id, fingerprint),
    };

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

    // The saved entry (and with it the pin) is written by the poll loop once the
    // server actually accepts us. Saving here would pin a community the user
    // never got into — a typo'd address or a wrong password would leave a
    // permanent entry behind.
    Ok(PartyJoinDto {
        status: "joined",
        server: Some(sid.to_string()),
        fingerprint,
        sas: None,
    })
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
    let mut party = state.party.lock().await;
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

/// Load the saved-communities list.
///
/// A missing file is an empty list — that is a first run. An *unreadable* one is
/// an error, deliberately. This file holds the pinned fingerprint of every
/// community the user has joined, and that pin is the only thing standing
/// between them and a server that swapped its key. Swallowing a parse failure
/// silently discarded every pin and turned the next join back into an
/// unverified first contact, which is exactly the situation the pin exists to
/// prevent — a corrupt file must stop the join, not quietly downgrade it.
fn load_saved_parties(path: &Path) -> Result<Vec<SavedParty>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(format!(
                "Could not read your saved communities ({}): {e}. \
                 Not joining, because the fingerprints that protect those \
                 connections are stored there.",
                path.display()
            ))
        }
    };
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&raw).map_err(|e| {
        format!(
            "Your saved communities file ({}) is damaged: {e}. \
             It holds the fingerprints that protect those connections, so \
             joining is refused until it is repaired or removed.",
            path.display()
        )
    })
}

/// Save the saved-communities list.
///
/// Written through `write_file_atomic` — temp file, fsync, rename, 0600 from
/// creation — for the same reason `identity.json` is: a half-written file after
/// a crash is indistinguishable from a file with no pins in it.
fn save_saved_parties(path: &Path, list: &[SavedParty]) {
    match serde_json::to_string_pretty(list) {
        Ok(json) => {
            if let Err(e) = messenger_core::util::write_file_atomic(path, json.as_bytes()) {
                tracing::warn!("saving communities list failed: {e}");
            }
        }
        Err(e) => tracing::warn!("serializing communities list failed: {e}"),
    }
}

/// Insert or update the entry for `address` (matched case-insensitively).
///
/// A load failure aborts the write: rewriting the file from an empty list would
/// destroy every other community's pin.
pub(crate) fn upsert_saved_party(path: &Path, entry: SavedParty) {
    let mut list = match load_saved_parties(path) {
        Ok(list) => list,
        Err(e) => {
            tracing::error!("{e}");
            return;
        }
    };
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
    load_saved_parties(&state.parties_path)
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
        let mut list = load_saved_parties(&state.parties_path)?;
        list.retain(|p| !p.address.eq_ignore_ascii_case(&addr));
        save_saved_parties(&state.parties_path, &list);
    }
    Ok(())
}

/// Open the native file picker (parented to the app window) and read the chosen
/// file's bytes, returning `(name, mime, data)`. `Ok(None)` if the user
/// cancelled.
///
/// A community upload is sent inline, so the whole file is held in memory by
/// definition — which is exactly why the size is checked against
/// [`messenger_core::party::MAX_INLINE_FILE_BYTES`] *before* reading rather than
/// after. Reading first meant picking a 4 GB file allocated 4 GB only to be
/// rejected. The read is async: the previous blocking `std::fs::read` ran on the
/// async runtime and stalled every other task on that worker for its duration.
async fn pick_upload<R: tauri::Runtime>(
    window: tauri::WebviewWindow<R>,
) -> Result<Option<(String, String, Vec<u8>)>, String> {
    let picked =
        crate::native_file_dialog(window, |d| d.set_title("Share a file").pick_file()).await?;
    let Some(path) = picked else {
        return Ok(None);
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let mime = messenger_core::util::guess_mime(&path).to_string();
    let size = tokio::fs::metadata(&path)
        .await
        .map_err(|e| format!("Could not read {}: {e}", path.display()))?
        .len();
    let cap = messenger_core::party::MAX_INLINE_FILE_BYTES as u64;
    if size > cap {
        return Err(format!(
            "{} is {} — community files are limited to {}.",
            name,
            messenger_core::util::format_size(size),
            messenger_core::util::format_size(cap)
        ));
    }
    let data = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    Ok(Some((name, mime, data)))
}

/// Pick a file and post it to a community channel. The picker runs on a blocking
/// thread, parented to the app window; a cancelled dialog is a successful no-op.
#[tauri::command]
pub(crate) async fn party_send_file<R: tauri::Runtime>(
    server: String,
    channel: String,
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let cid = Uuid::parse_str(&channel).map_err(|e| e.to_string())?;
    let Some((name, mime, data)) = pick_upload(window).await? else {
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
pub(crate) async fn party_send_file_dm<R: tauri::Runtime>(
    server: String,
    to: String,
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = Uuid::parse_str(&server).map_err(|e| e.to_string())?;
    let peer = Uuid::parse_str(&to).map_err(|e| e.to_string())?;
    let Some((name, mime, data)) = pick_upload(window).await? else {
        return Ok(());
    };
    state
        .party
        .lock()
        .await
        .send_file_dm(sid, peer, name, mime, data)
        .map_err(|e| e.to_string())
}

/// How long to wait for a community server to deliver a requested file before
/// giving up. Generous enough for a large blob on a slow link, short enough that
/// a server which simply never answers produces an error instead of a click that
/// does nothing forever.
const PARTY_DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Download a community file by content hash and save it via the native dialog.
/// `name` seeds the save dialog. The server only returns bytes the caller is
/// allowed to see (access-checked there). A cancelled save dialog is a no-op.
#[tauri::command]
pub(crate) async fn party_download_file<R: tauri::Runtime>(
    server: String,
    hash: String,
    name: String,
    window: tauri::WebviewWindow<R>,
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
    // Bounded wait. A bare `rx.await` hangs forever if the server accepts the
    // request and then never answers (or the response is lost): the click does
    // nothing, with no error and no way to tell it apart from a slow link.
    // Outer error: the sender was dropped (connection torn down). Inner error:
    // the server refused (file gone / not permitted).
    let data = match tokio::time::timeout(PARTY_DOWNLOAD_TIMEOUT, rx).await {
        Ok(Ok(Ok(data))) => data,
        Ok(Ok(Err(reason))) => return Err(reason),
        Ok(Err(_)) => return Err("download failed: the connection closed".to_string()),
        Err(_) => {
            return Err(format!(
                "download timed out after {}s — the community server did not send the file",
                PARTY_DOWNLOAD_TIMEOUT.as_secs()
            ))
        }
    };
    let suggested = name;
    let picked = crate::native_file_dialog(window, move |d| {
        d.set_title("Save file")
            .set_file_name(suggested)
            .save_file()
    })
    .await?;
    let Some(path) = picked else {
        return Ok(()); // user cancelled the save dialog
    };
    // Async: this runs on the shared runtime, and a blocking write of a
    // multi-megabyte blob stalls every other task on that worker.
    tokio::fs::write(&path, &data)
        .await
        .map_err(|e| e.to_string())
}
