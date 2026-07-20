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
    /// Total message count; the frontend derives unread badges from it (count
    /// beyond what was on screen when the conversation was last open).
    messages: usize,
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
            });
        }
    }
    Ok(out)
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
            TransferDto {
                id: t.id.to_string(),
                chat_id: t.chat_id.to_string(),
                filename: t.filename,
                size: t.size,
                received: t.received,
                status,
                error,
            }
        })
        .collect())
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
