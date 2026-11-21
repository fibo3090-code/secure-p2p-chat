//! Communities (Party servers): a thin bridge over `PartyManager` (shared with
//! the terminal client), plus the saved-communities file (parties.json) with
//! fingerprint pinning. Params are single words (Tauri arg-naming footgun).
use crate::*;
use messenger_core::party::{ChannelKind, FilePermissions, Role, UploadTarget};

#[derive(Serialize)]
pub(crate) struct PartyMemberDto {
    id: String,
    username: String,
    online: bool,
    is_me: bool,
    /// Messages in my DM thread with this member, for unread badges.
    dm_messages: usize,
    role: &'static str,
}

#[derive(Serialize)]
pub(crate) struct PartyChannelDto {
    id: String,
    name: String,
    /// Message count, for unread badges.
    messages: usize,
    kind: &'static str,
    /// A private channel's member ids; empty for every other kind.
    members: Vec<String>,
    /// Whether this client may post here, so the composer can say why not.
    can_post: bool,
}

#[derive(Serialize)]
pub(crate) struct PartyFileDto {
    hash: String,
    name: String,
    size: u64,
    mime: String,
    uploader: String,
    uploader_name: String,
    location: String,
    location_name: String,
    is_dm: bool,
    shared_at: u64,
    can_delete: bool,
    can_view: bool,
    can_download: bool,
    can_share: bool,
}

#[derive(Serialize)]
pub(crate) struct PartyQuotaDto {
    used: u64,
    limit: Option<u64>,
    server_used: u64,
    server_limit: u64,
}

#[derive(Serialize)]
pub(crate) struct PartyAuditDto {
    at: u64,
    actor_name: String,
    action: String,
    detail: String,
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::Guest => "guest",
        Role::Member => "member",
        Role::Admin => "admin",
        Role::Owner => "owner",
    }
}

fn parse_role(s: &str) -> Result<Role, String> {
    match s {
        "guest" => Ok(Role::Guest),
        "member" => Ok(Role::Member),
        "admin" => Ok(Role::Admin),
        // Ownership is never assigned, so "owner" is deliberately unparseable.
        other => Err(format!("unknown role: {other}")),
    }
}

fn kind_str(k: ChannelKind) -> &'static str {
    match k {
        ChannelKind::Public => "public",
        ChannelKind::Private => "private",
        ChannelKind::Locked => "locked",
        ChannelKind::Announce => "announce",
    }
}

fn parse_kind(s: &str) -> Result<ChannelKind, String> {
    match s {
        "public" => Ok(ChannelKind::Public),
        "private" => Ok(ChannelKind::Private),
        "locked" => Ok(ChannelKind::Locked),
        "announce" => Ok(ChannelKind::Announce),
        other => Err(format!("unknown channel kind: {other}")),
    }
}

#[derive(Serialize)]
pub(crate) struct PartyServerDto {
    id: String,
    name: String,
    address: String,
    /// The username this client joined with (for rejoin).
    username: String,
    fingerprint: String,
    status: &'static str,
    status_detail: Option<String>,
    member_id: Option<String>,
    channels: Vec<PartyChannelDto>,
    members: Vec<PartyMemberDto>,
    last_error: Option<String>,
    /// So the UI offers admin controls only to admins (the server enforces).
    my_role: &'static str,
    /// Most recent successful governance action, shown once then cleared.
    last_notice: Option<String>,
    files: Vec<PartyFileDto>,
    quota: Option<PartyQuotaDto>,
    audit: Vec<PartyAuditDto>,
}

#[derive(Serialize)]
pub(crate) struct PartyMessageDto {
    sender_name: String,
    from_me: bool,
    kind: &'static str,
    text: String,
    size: Option<u64>,
    /// Content hash of a file message, to request its download.
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
    // Fail closed: `Members` arrives after `Joined`/`Channels`, and defaulting to
    // `Role::default()` (Member) would briefly give a guest a composer.
    let my_role = conn
        .member_id
        .and_then(|me| conn.members.iter().find(|m| m.id == me))
        .map(|m| m.role)
        .unwrap_or(Role::Guest);
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
            role: role_str(m.role),
        })
        .collect();
    let channels = conn
        .channels
        .iter()
        .map(|c| PartyChannelDto {
            id: c.id.to_string(),
            name: c.name.clone(),
            messages: conn.messages.get(&c.id).map_or(0, |v| v.len()),
            kind: kind_str(c.kind),
            members: c.members.iter().map(|m| m.to_string()).collect(),
            // Mirrors the server's rule.
            can_post: conn
                .member_id
                .is_some_and(|me| c.kind.may_post(my_role, c.members.contains(&me)).is_ok()),
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
        my_role: role_str(my_role),
        last_notice: conn.last_notice.clone(),
        files: conn
            .files
            .iter()
            .map(|f| PartyFileDto {
                hash: f.hash.clone(),
                name: f.name.clone(),
                size: f.size,
                mime: f.mime.clone(),
                uploader: f.uploader.to_string(),
                uploader_name: f.uploader_name.clone(),
                location: f.location.to_string(),
                location_name: f.location_name.clone(),
                is_dm: f.is_dm,
                shared_at: f.shared_at,
                can_delete: f.can_delete,
                can_view: f.perms.view,
                can_download: f.perms.download,
                can_share: f.perms.share,
            })
            .collect(),
        quota: conn.quota.map(|q| PartyQuotaDto {
            used: q.used,
            limit: q.limit,
            server_used: q.server_used,
            server_limit: q.server_limit,
        }),
        audit: conn
            .audit
            .iter()
            .map(|a| PartyAuditDto {
                at: a.at,
                actor_name: a.actor_name.clone(),
                action: a.action.clone(),
                detail: a.detail.clone(),
            })
            .collect(),
    }
}

/// A stored envelope as a display DTO, with the sender's username resolved.
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

/// Run one `PartyManager` call under its lock, as a command result.
async fn with_party<T, E: std::fmt::Display>(
    state: &Bridge,
    f: impl FnOnce(&mut PartyManager) -> Result<T, E>,
) -> Result<T, String> {
    f(&mut *state.party.lock().await).map_err(|e| e.to_string())
}

/// What `party_join` did: `"joined"` (credentials sent, join in flight) or
/// `"verify"` (nothing sent; the user must confirm the server's identity).
#[derive(Serialize)]
pub(crate) struct PartyJoinDto {
    status: &'static str,
    server: Option<String>,
    fingerprint: String,
    sas: Option<String>,
}

/// Connect to a community server and join. Two-step, because the `Join` frame
/// carries the username and password: with no pin, the first call only learns
/// the server's fingerprint for the user to verify; a second call with
/// `fingerprint` set joins only if the server presents exactly that key
/// (audit TAURI-01 / UI-01).
#[tauri::command]
pub(crate) async fn party_join(
    address: String,
    username: String,
    password: String,
    fingerprint: Option<String>,
    state: tauri::State<'_, Bridge>,
) -> Result<PartyJoinDto, String> {
    ensure_ready(&state)?;
    let address = address.trim().to_string();
    let username = username.trim().to_string();
    let pk = lock_or_recover(&state.identity)
        .private_key()
        .map_err(|e| e.to_string())?;
    let password = Some(password).filter(|p| !p.trim().is_empty());

    // A saved pin always wins over the step-one fingerprint; it is compared
    // inside `connect_and_join` before the `Join` frame is written.
    let saved = load_saved_parties(&state.parties_path)?;
    let pinned = saved
        .into_iter()
        .find(|p| p.address.eq_ignore_ascii_case(&address) && !p.fingerprint.is_empty())
        .map(|p| p.fingerprint)
        .or_else(|| fingerprint.filter(|f| !f.trim().is_empty()));

    let mut party = state.party.lock().await;
    let outcome = party
        .connect_and_join(&address, &username, password, &pk, pinned.as_deref())
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

    // Rejoin replaces any older entry for the same address.
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

    // The poll loop saves (and pins) the entry once the server accepts us.
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
    // Stable order so the server list doesn't reshuffle each poll.
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(out)
}

/// Messages in a channel (seeded on join; live posts arrive via the poll loop).
#[tauri::command]
pub(crate) async fn party_history(
    server: String,
    channel: String,
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<PartyMessageDto>, String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let cid = parse_id(&channel)?;
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

#[tauri::command]
pub(crate) async fn party_post(
    server: String,
    channel: String,
    text: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let cid = parse_id(&channel)?;
    with_party(&state, |p| p.post(sid, cid, text)).await
}

#[tauri::command]
pub(crate) async fn party_create_channel(
    server: String,
    name: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    with_party(&state, |p| p.create_channel(sid, name)).await
}

#[tauri::command]
pub(crate) async fn party_send_dm(
    server: String,
    to: String,
    text: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let to = parse_id(&to)?;
    with_party(&state, |p| p.send_dm(sid, to, text)).await
}

/// The DM thread with a member. Also requests a fresh fetch, which lands on a
/// later poll.
#[tauri::command]
pub(crate) async fn party_dm_history(
    server: String,
    peer: String,
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<PartyMessageDto>, String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let peer = parse_id(&peer)?;
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

/// Clear a server's last surfaced error once the UI has shown it.
#[tauri::command]
pub(crate) async fn party_clear_error(
    server: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    state.party.lock().await.clear_server_error(sid);
    Ok(())
}

/// A joined community, persisted (no password) for one-click rejoin; the
/// pinned fingerprint refuses a different identity at the same address.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SavedParty {
    pub(crate) address: String,
    pub(crate) username: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) fingerprint: String,
}

/// Load the saved communities. Missing is an empty list (first run); an
/// unreadable file is an error, because silently dropping the pins would turn
/// the next join back into an unverified first contact.
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

/// Save the saved communities atomically (like `identity.json`): a torn file
/// is indistinguishable from one with no pins.
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

/// Insert or update the entry for `address` (case-insensitive). A load failure
/// aborts: rewriting from an empty list would destroy every other pin.
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

#[tauri::command]
pub(crate) async fn party_saved(
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<SavedParty>, String> {
    ensure_ready(&state)?;
    load_saved_parties(&state.parties_path)
}

/// Leave a community and forget it locally. The server keeps the membership,
/// so rejoining with the same identity resumes it.
#[tauri::command]
pub(crate) async fn party_leave(
    server: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
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

/// Create a channel of a given kind; `members` seeds a private channel. The
/// server enforces who may create the restricted kinds.
#[tauri::command]
pub(crate) async fn party_create_channel_kind(
    server: String,
    name: String,
    kind: String,
    members: Vec<String>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let kind = parse_kind(&kind)?;
    let members = parse_ids(&members)?;
    with_party(&state, |p| {
        p.create_channel_of_kind(sid, name, kind, members)
    })
    .await
}

#[tauri::command]
pub(crate) async fn party_delete_channel(
    server: String,
    channel: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let cid = parse_id(&channel)?;
    with_party(&state, |p| p.delete_channel(sid, cid)).await
}

#[tauri::command]
pub(crate) async fn party_set_channel_access(
    server: String,
    channel: String,
    kind: String,
    members: Vec<String>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let cid = parse_id(&channel)?;
    let kind = parse_kind(&kind)?;
    let members = parse_ids(&members)?;
    with_party(&state, |p| p.set_channel_access(sid, cid, kind, members)).await
}

#[tauri::command]
pub(crate) async fn party_set_role(
    server: String,
    member: String,
    role: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let mid = parse_id(&member)?;
    let role = parse_role(&role)?;
    with_party(&state, |p| p.set_role(sid, mid, role)).await
}

/// Request the Drive listing and quota; they land in a later `party-updated`.
#[tauri::command]
pub(crate) async fn party_refresh_files(
    server: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    with_party(&state, |p| p.refresh_files(sid)).await
}

/// Re-share a file into another channel or DM without re-uploading. `from` is
/// where the caller's right to share it comes from.
#[tauri::command]
pub(crate) async fn party_share_file(
    server: String,
    hash: String,
    from: String,
    channel: Option<String>,
    peer: Option<String>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let from = parse_id(&from)?;
    let target = match (channel, peer) {
        (Some(c), _) => UploadTarget::Channel(parse_id(&c)?),
        (None, Some(p)) => UploadTarget::Dm(parse_id(&p)?),
        (None, None) => return Err("no destination given".to_string()),
    };
    with_party(&state, |p| p.share_file(sid, hash, from, target)).await
}

/// The four rights as one single-word struct parameter (`perms`).
#[derive(Deserialize)]
pub(crate) struct PermsDto {
    view: bool,
    download: bool,
    delete: bool,
    share: bool,
}

/// Change what a shared file grants, by default (`member` omitted) or for one
/// member. The server refuses rights the caller does not hold.
#[tauri::command]
pub(crate) async fn party_set_file_permissions(
    server: String,
    hash: String,
    location: String,
    member: Option<String>,
    perms: PermsDto,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let loc = parse_id(&location)?;
    let member = member.as_deref().map(parse_id).transpose()?;
    let perms = FilePermissions {
        view: perms.view,
        download: perms.download,
        delete: perms.delete,
        share: perms.share,
    };
    with_party(&state, |p| {
        p.set_file_permissions(sid, hash, loc, member, perms)
    })
    .await
}

/// Delete one share of a file (its uploader, or an admin).
#[tauri::command]
pub(crate) async fn party_delete_file(
    server: String,
    hash: String,
    location: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let loc = parse_id(&location)?;
    with_party(&state, |p| p.delete_file(sid, hash, loc)).await
}

/// Request the audit log (admins only); lands in a later `party-updated`.
#[tauri::command]
pub(crate) async fn party_refresh_audit(
    server: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    with_party(&state, |p| p.refresh_audit(sid, 200)).await
}

#[tauri::command]
pub(crate) async fn party_clear_notice(
    server: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    state.party.lock().await.clear_notice(sid);
    Ok(())
}

fn parse_ids(ids: &[String]) -> Result<Vec<Uuid>, String> {
    ids.iter().map(|s| parse_id(s)).collect()
}

/// Pick a file and read it as `(name, mime, data)`; `Ok(None)` if cancelled.
/// The size is checked against `MAX_PARTY_FILE_BYTES` *before* reading, and
/// the read is async. Files past `MAX_INLINE_FILE_BYTES` are streamed in
/// chunks by `PartyManager`, so the server's ceiling is the only hard limit.
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
    let cap = messenger_core::party::MAX_PARTY_FILE_BYTES;
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

/// Pick a file and post it to a channel; cancelling is a no-op.
#[tauri::command]
pub(crate) async fn party_send_file<R: tauri::Runtime>(
    server: String,
    channel: String,
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let cid = parse_id(&channel)?;
    let Some((name, mime, data)) = pick_upload(window).await? else {
        return Ok(());
    };
    with_party(&state, |p| p.send_file(sid, cid, name, mime, data)).await
}

/// Pick a file and send it as a DM; cancelling is a no-op.
#[tauri::command]
pub(crate) async fn party_send_file_dm<R: tauri::Runtime>(
    server: String,
    to: String,
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    let peer = parse_id(&to)?;
    let Some((name, mime, data)) = pick_upload(window).await? else {
        return Ok(());
    };
    with_party(&state, |p| p.send_file_dm(sid, peer, name, mime, data)).await
}

/// How long to wait for a requested file, so a server that never answers gives
/// an error rather than a click that does nothing forever.
const PARTY_DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Download a community file by content hash and save it via the native
/// dialog, which `name` seeds. The server access-checks the read. A cancelled
/// dialog is a no-op.
#[tauri::command]
pub(crate) async fn party_download_file<R: tauri::Runtime>(
    server: String,
    hash: String,
    name: String,
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let sid = parse_id(&server)?;
    // The lock is released before awaiting, so the poll loop can complete us.
    let rx = with_party(&state, |p| p.request_download(sid, hash)).await?;
    // Outer error: connection torn down. Inner error: the server refused.
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
    // Another member chose this name: sanitize it (`../`, device names, bidi
    // overrides) before it pre-fills the save dialog.
    let suggested = messenger_core::util::sanitize_filename(&name);
    let picked = crate::native_file_dialog(window, move |d| {
        d.set_title("Save file")
            .set_file_name(suggested)
            .save_file()
    })
    .await?;
    let Some(path) = picked else {
        return Ok(());
    };
    tokio::fs::write(&path, &data)
        .await
        .map_err(|e| e.to_string())
}
