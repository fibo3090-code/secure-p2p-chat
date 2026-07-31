//! Conversations: listing, history, sending text and files, transfers,
//! rename/delete.
use crate::*;

#[derive(Serialize)]
pub(crate) struct ConvSummary {
    id: String,
    title: String,
    last: Option<String>,
    connected: bool,
    placeholder: bool,
    kind: &'static str,
    transport: &'static str,
    /// True once the peer's fingerprint has been confirmed (TOFU-verified). The
    /// UI must not claim "verified" for conversations that are still pending.
    verified: bool,
    /// Total message count.
    messages: usize,
    /// Messages from the peer the user has not seen yet. Computed from the read
    /// mark persisted in the encrypted history, **not** from what happened to be
    /// on screen this session — so anything that arrived while the app was
    /// closed is still badged on the next launch.
    unread: usize,
    /// RFC 3339 timestamp of the newest message, for "last activity" display.
    last_at: Option<String>,
}

fn kind_str(k: messenger_core::types::ChatKind) -> &'static str {
    use messenger_core::types::ChatKind::*;
    match k {
        Dm => "dm",
        Group => "group",
        Channel => "channel",
    }
}

fn transport_str(t: messenger_core::types::Transport) -> &'static str {
    use messenger_core::types::Transport::*;
    match t {
        Direct => "direct",
        Relay => "relay",
        Server => "server",
    }
}

#[tauri::command]
pub(crate) async fn list_conversations(
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<ConvSummary>, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    let mut out = Vec::new();
    for id in mgr.chat_ids() {
        if let Some(chat) = mgr.get_chat(id) {
            let last = chat.messages.last().map(|m| match &m.content {
                MessageContent::Text { text } => text.clone(),
                MessageContent::File { filename, .. } => format!("📎 {}", filename),
            });
            out.push(ConvSummary {
                id: id.to_string(),
                title: chat.title.clone(),
                last,
                connected: mgr.is_connected(&id),
                placeholder: chat.is_host_placeholder,
                kind: kind_str(chat.kind),
                transport: transport_str(chat.transport),
                verified: chat.peer_fingerprint.is_some(),
                messages: chat.messages.len(),
                unread: chat.unread_count(),
                last_at: chat.messages.last().map(|m| m.timestamp.to_rfc3339()),
            });
        }
    }
    Ok(out)
}

/// Mark a conversation as read up to its newest message. The read mark lives in
/// the encrypted history, so the badge stays cleared across restarts.
#[tauri::command]
pub(crate) async fn mark_read(id: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let chat_id = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    {
        let mut mgr = state.manager.lock().await;
        mgr.mark_chat_read(chat_id);
    }
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}

/// Report what the user can see: whether the window has focus, and which
/// conversation is open. `ChatManager` owns no window handle, so the shell has
/// to push this down — without it "notify when a message arrives in the
/// background" fires for the thread the user is actively reading.
#[tauri::command]
pub(crate) async fn set_presence(
    focused: bool,
    chat: Option<String>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    // An unparseable id means "no conversation open" rather than an error — the
    // shell should never be able to break presence tracking with a bad string.
    let active = chat.as_deref().and_then(|c| Uuid::parse_str(c).ok());
    state.manager.lock().await.set_ui_presence(focused, active);
    Ok(())
}

/// A live file transfer, for progress display in the chat pane.
#[derive(Serialize)]
pub(crate) struct TransferDto {
    id: String,
    chat_id: String,
    filename: String,
    size: u64,
    received: u64,
    status: &'static str,
    /// "incoming" or "outgoing", so the UI can label the direction.
    direction: &'static str,
    /// Whether the transfer is still cancellable (pending or in progress).
    cancellable: bool,
    /// Failure reason when `status == "failed"`.
    error: Option<String>,
}

fn transfer_status_parts(s: &TransferStatus) -> (&'static str, Option<String>) {
    match s {
        TransferStatus::Pending => ("pending", None),
        TransferStatus::AwaitingAcceptance => ("awaiting", None),
        TransferStatus::InProgress => ("active", None),
        TransferStatus::Completed => ("done", None),
        TransferStatus::Failed(e) => ("failed", Some(e.clone())),
        TransferStatus::Cancelled => ("cancelled", None),
    }
}

/// The active file transfers (both directions), polled by the frontend on the
/// same `state-updated` cadence as the conversation list, so large sends and
/// receives show live progress instead of nothing.
#[tauri::command]
pub(crate) async fn list_transfers(
    state: tauri::State<'_, Bridge>,
) -> Result<Vec<TransferDto>, String> {
    ensure_ready(&state)?;
    let mgr = state.manager.lock().await;
    Ok(mgr
        .active_transfers_snapshot()
        .into_iter()
        .map(|t| {
            let (status, error) = transfer_status_parts(&t.status);
            let cancellable = matches!(
                t.status,
                TransferStatus::Pending | TransferStatus::InProgress
            );
            let direction = match t.direction {
                TransferDirection::Incoming => "incoming",
                TransferDirection::Outgoing => "outgoing",
            };
            TransferDto {
                id: t.id.to_string(),
                chat_id: t.chat_id.to_string(),
                filename: t.filename,
                size: t.size,
                received: t.received,
                status,
                direction,
                cancellable,
                error,
            }
        })
        .collect())
}

/// Cancel an in-flight file transfer (either direction) by id.
#[tauri::command]
pub(crate) async fn cancel_transfer(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let transfer_id = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state.manager.lock().await.cancel_transfer(transfer_id);
    Ok(())
}

/// Return the full conversation (with messages) as JSON for the chat pane.
#[tauri::command]
pub(crate) async fn get_conversation(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<serde_json::Value, String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let mgr = state.manager.lock().await;
    match mgr.get_chat(uuid) {
        Some(chat) => serde_json::to_value(chat).map_err(|e| e.to_string()),
        None => Err("No such conversation".to_string()),
    }
}

#[tauri::command]
pub(crate) async fn send_message(
    id: String,
    text: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .send_message(uuid, text)
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}

/// Pick a file with the native dialog and send it over the given conversation.
/// The picker runs on a blocking thread so it never stalls the async runtime;
/// a cancelled dialog is a successful no-op.
#[tauri::command]
pub(crate) async fn send_file(id: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let picked = tokio::task::spawn_blocking(|| rfd::FileDialog::new().pick_file())
        .await
        .map_err(|e| e.to_string())?;
    let Some(path) = picked else {
        return Ok(()); // user cancelled
    };
    state
        .manager
        .lock()
        .await
        .send_file(uuid, path)
        .await
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}

/// Accept an incoming file offer (a transfer in the "awaiting" state).
#[tauri::command]
pub(crate) async fn accept_transfer(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .accept_incoming_file(uuid)
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}

/// Decline an incoming file offer: the spooled data is deleted and the rest
/// of the stream is discarded.
#[tauri::command]
pub(crate) async fn decline_transfer(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .reject_incoming_file(uuid)
        .map_err(|e| e.to_string())
}

/// Resolve a file message's on-disk path from history. The webview only ever
/// passes (chat id, message id) — never a raw filesystem path — so the bridge
/// cannot be used to open or read arbitrary files.
async fn file_message_path(
    state: &tauri::State<'_, Bridge>,
    id: &str,
    msg: &str,
) -> Result<PathBuf, String> {
    let chat_id = Uuid::parse_str(id).map_err(|e| e.to_string())?;
    let msg_id = Uuid::parse_str(msg).map_err(|e| e.to_string())?;
    let mgr = state.manager.lock().await;
    let chat = mgr
        .get_chat(chat_id)
        .ok_or_else(|| "No such conversation".to_string())?;
    let m = chat
        .messages
        .iter()
        .find(|m| m.id == msg_id)
        .ok_or_else(|| "No such message".to_string())?;
    match &m.content {
        MessageContent::File { path: Some(p), .. } => Ok(p.clone()),
        MessageContent::File { path: None, .. } => {
            Err("This file's location was not recorded".to_string())
        }
        _ => Err("Not a file message".to_string()),
    }
}

/// Open a file with the OS default app, or reveal it in the file manager.
fn open_path_os(path: &Path, reveal: bool) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        if reveal {
            // The `/select,` argument must reach explorer as one raw token,
            // quoted, or paths with spaces get split.
            std::process::Command::new("explorer")
                .raw_arg(format!("/select,\"{}\"", path.display()))
                .spawn()?;
        } else {
            std::process::Command::new("explorer").arg(path).spawn()?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = std::process::Command::new("open");
        if reveal {
            cmd.arg("-R");
        }
        cmd.arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // xdg-open has no "reveal" mode; open the containing directory instead.
        let target = if reveal {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        std::process::Command::new("xdg-open").arg(target).spawn()?;
    }
    Ok(())
}

/// Open a sent/received file with the default app (`reveal: false`) or show it
/// in the file manager (`reveal: true`). A file card in the chat is no longer
/// a dead end — this is what its click and folder button call.
#[tauri::command]
pub(crate) async fn open_file(
    id: String,
    msg: String,
    reveal: bool,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let path = file_message_path(&state, &id, &msg).await?;
    if !path.exists() {
        return Err(format!("File no longer exists on disk: {}", path.display()));
    }
    open_path_os(&path, reveal).map_err(|e| e.to_string())
}

/// Cap on how large an image is inlined as a preview; larger ones fall back to
/// the plain file card (the webview would balloon on multi-MB data URLs).
const PREVIEW_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Inline preview for an image file message, as a `data:` URL, or `None` when
/// the file is not a previewable image (or too large). SVG is deliberately
/// excluded — it can carry scripts, and history files may come from a peer.
#[tauri::command]
pub(crate) async fn file_preview(
    id: String,
    msg: String,
    state: tauri::State<'_, Bridge>,
) -> Result<Option<String>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    ensure_ready(&state)?;
    let path = file_message_path(&state, &id, &msg).await?;
    let mime = messenger_core::util::guess_mime(&path);
    if !mime.starts_with("image/") || mime == "image/svg+xml" {
        return Ok(None);
    }
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| e.to_string())?;
    if meta.len() > PREVIEW_MAX_BYTES {
        return Ok(None);
    }
    let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
    Ok(Some(format!(
        "data:{};base64,{}",
        mime,
        STANDARD.encode(bytes)
    )))
}

/// Open an http(s) link from a message in the system browser. The scheme is
/// whitelisted here — message text comes from the peer, and anything except
/// plain web URLs (file:, smb:, custom app schemes, …) must not launch.
#[tauri::command]
pub(crate) async fn open_url(url: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("Only http(s) links can be opened".to_string());
    }
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", &url])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(&url).spawn();
    result.map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) async fn rename_chat(
    id: String,
    title: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state
        .manager
        .lock()
        .await
        .rename_chat(uuid, title)
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}

#[tauri::command]
pub(crate) async fn delete_chat(id: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    state.manager.lock().await.delete_chat(uuid);
    persist_history(&state.manager, &state.history_path).await;
    Ok(())
}
