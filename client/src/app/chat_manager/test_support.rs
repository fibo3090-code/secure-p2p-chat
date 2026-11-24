//! Fixtures shared by the `ChatManager` unit tests.

use super::*;

/// A manager whose downloads/spools live under `dir`.
pub(super) fn manager_in(dir: &Path, auto_accept_files: bool) -> ChatManager {
    ChatManager::new(Config {
        download_dir: dir.join("downloads"),
        temp_dir: dir.join("temp"),
        auto_accept_files,
        ..Config::default()
    })
}

/// A default manager with one local chat.
pub(super) fn manager_with_chat(title: &str) -> (ChatManager, Uuid) {
    let mut mgr = ChatManager::default();
    let id = Uuid::new_v4();
    mgr.create_local_chat_for_test(id, title.to_string());
    (mgr, id)
}

/// A conversation with a live (in-memory) session; returns its control lane.
pub(super) fn chat_with_session(
    mgr: &mut ChatManager,
    title: &str,
) -> (Uuid, mpsc::UnboundedReceiver<ProtocolMessage>) {
    let id = Uuid::new_v4();
    mgr.create_local_chat_for_test(id, title.to_string());
    let (tx, rx) = mpsc::unbounded_channel();
    mgr.add_session_for_test(id, SessionHandle::for_test_control(tx));
    (id, rx)
}

/// A host-side incoming chat ("Peer") mapped to a fresh session id:
/// `(session_id, incoming_chat_id)`.
pub(super) fn incoming_chat(mgr: &mut ChatManager) -> (Uuid, Uuid) {
    let session_id = Uuid::new_v4();
    let incoming = Uuid::new_v4();
    mgr.create_local_chat_for_test(incoming, "Peer".into());
    mgr.chat_id_mapping.insert(incoming, session_id);
    (session_id, incoming)
}

/// Register a fingerprint-confirm channel for `session` and return its receiver.
pub(super) fn confirm_channel(
    mgr: &mut ChatManager,
    session: Uuid,
) -> mpsc::UnboundedReceiver<bool> {
    let (tx, rx) = mpsc::unbounded_channel();
    mgr.add_fingerprint_confirm_sender_for_test(session, tx);
    rx
}

/// A live session (handle + event receiver + confirm sender) for `session_id`;
/// returns the control lane so a test can assert on queued frames.
pub(super) fn wire_session_for_test(
    mgr: &mut ChatManager,
    session_id: Uuid,
) -> mpsc::UnboundedReceiver<ProtocolMessage> {
    let (app_tx, app_rx) = mpsc::unbounded_channel();
    mgr.add_session_for_test(session_id, SessionHandle::for_test_control(app_tx));
    let (_event_tx, event_rx) = mpsc::unbounded_channel::<SessionEvent>();
    mgr.session_events
        .insert(session_id, Arc::new(Mutex::new(event_rx)));
    let (confirm_tx, _confirm_rx) = mpsc::unbounded_channel();
    mgr.add_fingerprint_confirm_sender_for_test(session_id, confirm_tx);
    app_rx
}

pub(super) fn frame(mgr: &mut ChatManager, session: Uuid, msg: ProtocolMessage) {
    mgr.handle_session_event(session, SessionEvent::MessageReceived(msg));
}

pub(super) fn meta(filename: &str, size: u64, seq: u64) -> ProtocolMessage {
    ProtocolMessage::FileMeta {
        filename: filename.into(),
        size,
        seq,
    }
}

pub(super) fn chunk(bytes: &[u8], seq: u64) -> ProtocolMessage {
    ProtocolMessage::FileChunk {
        chunk: bytes.to_vec(),
        seq,
    }
}

/// Feed a whole incoming file (Meta, one chunk, End) at `seq`, `seq+1`, `seq+2`.
pub(super) fn feed_file(mgr: &mut ChatManager, chat: Uuid, name: &str, body: &[u8], seq: u64) {
    frame(mgr, chat, meta(name, body.len() as u64, seq));
    frame(mgr, chat, chunk(body, seq + 1));
    frame(mgr, chat, ProtocolMessage::FileEnd { seq: seq + 2 });
}

pub(super) fn text(seq: u64, body: &str) -> ProtocolMessage {
    ProtocolMessage::Text {
        text: body.to_string(),
        timestamp: 0,
        seq,
    }
}

pub(super) fn texts(mgr: &ChatManager, chat: Uuid) -> Vec<String> {
    mgr.get_chat(chat)
        .map(|c| {
            c.messages
                .iter()
                .filter_map(|m| match &m.content {
                    MessageContent::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Paths of the received-file messages in a chat.
pub(super) fn file_paths(mgr: &ChatManager, chat: Uuid) -> Vec<std::path::PathBuf> {
    mgr.get_chat(chat)
        .unwrap()
        .messages
        .iter()
        .filter_map(|m| match &m.content {
            MessageContent::File { path: Some(p), .. } => Some(p.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn has_file_message(mgr: &ChatManager, chat: Uuid) -> bool {
    mgr.get_chat(chat)
        .unwrap()
        .messages
        .iter()
        .any(|m| matches!(m.content, MessageContent::File { .. }))
}

/// Everything left in `dir/downloads` (empty if it does not exist).
pub(super) fn downloads_left(dir: &Path) -> Vec<std::ffi::OsString> {
    std::fs::read_dir(dir.join("downloads"))
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default()
}

pub(super) fn transfer_status(mgr: &ChatManager, id: Uuid) -> Option<TransferStatus> {
    mgr.active_transfers_snapshot()
        .into_iter()
        .find(|t| t.id == id)
        .map(|t| t.status)
}
