//! Conversations: listing, history, sending text and files, transfers,
//! rename/delete.
use crate::*;

#[derive(Serialize)]
pub(crate) struct ConvSummary {
    id: String,
    title: String,
    last: Option<String>,
    /// Whether the newest message is the user's own, so the list can prefix the
    /// preview with "You:" — without it a row cannot say whose turn it is.
    last_from_me: bool,
    connected: bool,
    placeholder: bool,
    kind: &'static str,
    transport: &'static str,
    /// The peer's fingerprint is confirmed (TOFU-verified).
    verified: bool,
    messages: usize,
    /// Unseen peer messages, from the read mark persisted in the history, so
    /// anything that arrived while the app was closed is still badged.
    unread: usize,
    /// RFC 3339 timestamp of the newest message.
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
                // A preview line, not a 24 MiB message (PERF-02).
                MessageContent::Text { text } => preview_of(text),
                MessageContent::File { filename, .. } => format!("📎 {}", filename),
            });
            out.push(ConvSummary {
                id: id.to_string(),
                title: chat.title.clone(),
                last,
                last_from_me: chat.messages.last().is_some_and(|m| m.from_me),
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

/// Mark a conversation read up to its newest message (persisted in history).
#[tauri::command]
pub(crate) async fn mark_read(id: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let chat_id = parse_id(&id)?;
    state.manager.lock().await.mark_chat_read(chat_id);
    // No save: this fires often, and the poll loop writes the new read mark
    // within `HISTORY_SAVE_MIN_INTERVAL`.
    Ok(())
}

/// Report window focus and the open conversation, so notifications never fire
/// for the thread on screen (`ChatManager` owns no window handle).
#[tauri::command]
pub(crate) async fn set_presence(
    focused: bool,
    chat: Option<String>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    // An unparseable id means "no conversation open", never an error.
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
    /// "incoming" or "outgoing".
    direction: &'static str,
    /// Still cancellable (pending or in progress).
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

/// The active file transfers (both directions), for live progress.
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
    let transfer_id = parse_id(&id)?;
    state.manager.lock().await.cancel_transfer(transfer_id);
    Ok(())
}

/// Characters of the last message shown as a conversation's preview.
pub(crate) const PREVIEW_CHARS: usize = 200;

/// The start of `text`, cut on a character boundary, for the sidebar.
pub(crate) fn preview_of(text: &str) -> String {
    match text.char_indices().nth(PREVIEW_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_string(),
    }
}

/// Bytes of one message's text sent to the chat pane. A ~24 MiB peer text
/// shipped whole stalled every refresh of the webview (PERF-02); the history
/// keeps the full text.
pub(crate) const MAX_DISPLAY_TEXT_BYTES: usize = 256 * 1024;

/// `text` cut to [`MAX_DISPLAY_TEXT_BYTES`] on a character boundary, with a
/// note saying how much was left out.
pub(crate) fn display_text(text: &str) -> std::borrow::Cow<'_, str> {
    if text.len() <= MAX_DISPLAY_TEXT_BYTES {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut cut = MAX_DISPLAY_TEXT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let rest_kib = (text.len() - cut).div_ceil(1024);
    std::borrow::Cow::Owned(format!(
        "{}\n\n[… {rest_kib} KiB more not shown — the full message is kept in your history]",
        &text[..cut]
    ))
}

/// The conversation (with messages) as JSON for the chat pane; only oversized
/// texts are cut (see [`MAX_DISPLAY_TEXT_BYTES`]).
#[tauri::command]
pub(crate) async fn get_conversation(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<serde_json::Value, String> {
    ensure_ready(&state)?;
    let uuid = parse_id(&id)?;
    let mgr = state.manager.lock().await;
    let chat = mgr
        .get_chat(uuid)
        .ok_or_else(|| "No such conversation".to_string())?;
    let mut value = serde_json::to_value(chat).map_err(|e| e.to_string())?;
    if let Some(messages) = value.get_mut("messages").and_then(|m| m.as_array_mut()) {
        for m in messages {
            // `MessageContent` is internally tagged: {"type":"text","text":…}.
            if m.pointer("/content/type").and_then(|t| t.as_str()) != Some("text") {
                continue;
            }
            let Some(text) = m.pointer_mut("/content/text") else {
                continue;
            };
            if let Some(s) = text.as_str() {
                if s.len() > MAX_DISPLAY_TEXT_BYTES {
                    *text = serde_json::Value::String(display_text(s).into_owned());
                }
            }
        }
    }
    Ok(value)
}

#[tauri::command]
pub(crate) async fn send_message(
    id: String,
    text: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = parse_id(&id)?;
    state
        .manager
        .lock()
        .await
        .send_message(uuid, text)
        .map_err(|e| e.to_string())?;
    // No save: per-message saves are quadratic. The poll loop writes within
    // `HISTORY_SAVE_MIN_INTERVAL` and window close flushes.
    Ok(())
}

/// Pick a file and send it over the conversation; cancelling is a no-op.
#[tauri::command]
pub(crate) async fn send_file<R: tauri::Runtime>(
    id: String,
    window: tauri::WebviewWindow<R>,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = parse_id(&id)?;
    let picked = crate::native_file_dialog(
        window,
        crate::Dialog::Open {
            title: "Send a file",
        },
    )
    .await?;
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
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(())
}

/// Accept an incoming file offer (a transfer in the "awaiting" state).
#[tauri::command]
pub(crate) async fn accept_transfer(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = parse_id(&id)?;
    state
        .manager
        .lock()
        .await
        .accept_incoming_file(uuid)
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(())
}

/// Decline an incoming file offer: the spool is deleted, the rest discarded.
#[tauri::command]
pub(crate) async fn decline_transfer(
    id: String,
    state: tauri::State<'_, Bridge>,
) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = parse_id(&id)?;
    state
        .manager
        .lock()
        .await
        .reject_incoming_file(uuid)
        .map_err(|e| e.to_string())
}

/// Resolve a file message's on-disk path from history, plus whether the user
/// sent it. The webview passes only (chat id, message id), never a raw path,
/// so the bridge cannot open or read arbitrary files.
async fn file_message_path(
    state: &tauri::State<'_, Bridge>,
    id: &str,
    msg: &str,
) -> Result<(PathBuf, bool), String> {
    let chat_id = parse_id(id)?;
    let msg_id = parse_id(msg)?;
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
        MessageContent::File { path: Some(p), .. } => Ok((p.clone(), m.from_me)),
        MessageContent::File { path: None, .. } => {
            Err("This file's location was not recorded".to_string())
        }
        _ => Err("Not a file message".to_string()),
    }
}

/// Open a file with the OS default app, or reveal it in the file manager.
/// In the bridge tests the request is recorded (`tests::opened`) instead. Same
/// gate as the `tests` module itself, which is off on Windows (see
/// Cargo.toml): a plain `cfg(test)` here named a module that does not exist
/// there, and the Windows test build did not compile.
fn open_path_os(path: &Path, reveal: bool) -> std::io::Result<()> {
    #[cfg(all(test, not(windows)))]
    {
        crate::tests::opened::record(path, reveal);
        Ok(())
    }
    #[cfg(not(all(test, not(windows))))]
    launch_with_os(path, reveal)
}

#[cfg_attr(all(test, not(windows)), allow(dead_code))]
fn launch_with_os(path: &Path, reveal: bool) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        if reveal {
            // `/select,` must reach explorer as one quoted raw token.
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

/// Extensions whose "open" action is really "run this code".
///
/// Checked on every platform rather than per-OS: a `.desktop` file is inert on
/// Windows and a `.exe` is inert on Linux, but warning about the wrong one costs
/// a dialog while missing the right one costs the machine. The list covers the
/// launchers that need no interpreter of their own — the classic double
/// extension (`holiday.jpg.exe`) lands here because only the final one counts.
const RISKY_EXTENSIONS: &[&str] = &[
    // Windows executables, installers and script hosts
    "exe",
    "com",
    "scr",
    "pif",
    "bat",
    "cmd",
    "msi",
    "msp",
    "msc",
    "cpl",
    "hta",
    "vbs",
    "vbe",
    "jse",
    "wsf",
    "wsh",
    "ps1",
    "psm1",
    "reg",
    "lnk",
    "url",
    "scf",
    "inf",
    "chm",
    "application",
    "gadget", // Cross-platform runtimes / scripts
    "jar",
    "js",
    "py",
    "pl",
    "rb",
    "php", // Unix shells and launchers
    "sh",
    "bash",
    "zsh",
    "command",
    "desktop",
    "run",
    "appimage",
    "app",
    "pkg",
    "deb",
    "rpm",
    // Windows launchers, installers, add-ins and scriptlets (TAURI-06)
    "appref-ms",
    "settingcontent-ms",
    "library-ms",
    "search-ms",
    "diagcab",
    "appinstaller",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "xll",
    "wsc",
    "sct",
    "vb",
    "ws",
    "ps1xml",
    "psc1",
    "jnlp",
    "website",
    "mde",
    "cab",
    // Disk images: mounting one presents its contents without the download's
    // protections
    "iso",
    "img",
    "vhd",
    "vhdx",
    "dmg",
    // Macro-enabled Office documents
    "docm",
    "dotm",
    "xlsm",
    "xltm",
    "xlam",
    "pptm",
    "potm",
    "ppam",
    // Opens in a browser with a `file://` origin, script and all
    "html",
    "htm",
    "xhtml",
    "mht",
    "mhtml",
    "svg",
    // macOS launchers and scripts
    "terminal",
    "tool",
    "scpt",
    "applescript",
    "workflow",
    "webloc",
    "inetloc",
    "fileloc",
    "mpkg",
];

/// Received content that opens on a click without asking: types whose default
/// handler displays or plays them. Everything else — documents, archives,
/// anything unnamed — gets a confirmation first, because the list of things
/// that run code when opened is open-ended and a deny-list is always missing
/// one (TAURI-06). Mirrored in `desktop/src/lib/bridge.js`, checked by
/// `bridge.test.js`.
const INERT_EXTENSIONS: &[&str] = &[
    // Raster images (SVG is not one: it can carry script)
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "heic", "heif", "avif",
    // Documents that only display
    "pdf", "txt", "md", "csv", "log", // Audio
    "mp3", "wav", "ogg", "oga", "flac", "m4a", "aac", "opus", // Video
    "mp4", "m4v", "mov", "mkv", "webm", "avi",
];

/// Why opening `path` needs the user's say-so, or `None` when it is content
/// known to be inert.
///
/// `kind` names the extension (empty when there is none), or "executable" for
/// a Unix file with an exec bit, so the warning can say what it is about.
/// `runs` is true when the OS would *execute* it, rather than merely hand it
/// to an application that might.
#[derive(Debug, PartialEq)]
struct OpenRisk {
    kind: String,
    runs: bool,
}

fn execution_risk(path: &Path) -> Option<OpenRisk> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if RISKY_EXTENSIONS.contains(&ext.as_str()) {
        return Some(OpenRisk {
            kind: ext,
            runs: true,
        });
    }
    // On Unix the executable bit decides, extension or not.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                return Some(OpenRisk {
                    kind: "executable".to_string(),
                    runs: true,
                });
            }
        }
    }
    if INERT_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    // Not known to run, not known to be safe. With no extension at all,
    // `xdg-open` sniffs the content — a PE file can reach Wine.
    Some(OpenRisk {
        kind: ext,
        runs: false,
    })
}

/// Outcome of [`open_file`]: opened, or held back because the file would
/// execute and the user has not confirmed.
#[derive(Serialize)]
pub(crate) struct OpenOutcome {
    opened: bool,
    /// The extension that held the open back (empty when the file has none,
    /// "executable" for an exec bit), when `opened` is false.
    blocked: Option<String>,
    /// The OS would execute it, as opposed to it being a type not known to be
    /// inert. The two get different warnings: a .docx is not a program.
    runs: bool,
    /// The file's name, for the confirmation prompt.
    filename: Option<String>,
}

/// Open a file with the default app (`reveal: false`) or show it in the file
/// manager (`reveal: true`). A file RECEIVED from a peer that would execute is
/// refused unless `confirm` is set; revealing launches nothing, and files the
/// user sent are not gated.
#[tauri::command]
pub(crate) async fn open_file(
    id: String,
    msg: String,
    reveal: bool,
    confirm: Option<bool>,
    state: tauri::State<'_, Bridge>,
) -> Result<OpenOutcome, String> {
    ensure_ready(&state)?;
    let (path, from_me) = file_message_path(&state, &id, &msg).await?;
    if !path.exists() {
        return Err(format!("File no longer exists on disk: {}", path.display()));
    }
    if !reveal && !from_me && !confirm.unwrap_or(false) {
        if let Some(risk) = execution_risk(&path) {
            return Ok(OpenOutcome {
                opened: false,
                blocked: Some(risk.kind),
                runs: risk.runs,
                filename: path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .or_else(|| Some(path.display().to_string())),
            });
        }
    }
    open_path_os(&path, reveal).map_err(|e| e.to_string())?;
    Ok(OpenOutcome {
        opened: true,
        blocked: None,
        runs: false,
        filename: None,
    })
}

/// Largest image inlined as a preview; bigger ones get the plain file card.
const PREVIEW_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Inline preview of an image file message as a `data:` URL, or `None`. SVG is
/// excluded: it can carry scripts, and the file may come from a peer.
#[tauri::command]
pub(crate) async fn file_preview(
    id: String,
    msg: String,
    state: tauri::State<'_, Bridge>,
) -> Result<Option<String>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    ensure_ready(&state)?;
    let (path, _from_me) = file_message_path(&state, &id, &msg).await?;
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

/// Open an http(s) link from a message in the system browser. Only web
/// schemes: message text comes from the peer.
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
    let uuid = parse_id(&id)?;
    state
        .manager
        .lock()
        .await
        .rename_chat(uuid, title)
        .map_err(|e| e.to_string())?;
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(())
}

#[tauri::command]
pub(crate) async fn delete_chat(id: String, state: tauri::State<'_, Bridge>) -> Result<(), String> {
    ensure_ready(&state)?;
    let uuid = parse_id(&id)?;
    state.manager.lock().await.delete_chat(uuid);
    persist_history(&state.manager, &state.history_path, &state.saved_sig).await;
    Ok(())
}

#[cfg(test)]
mod size_tests {
    use super::*;

    /// PERF-02: preview and chat pane text are bounded, cut on char boundaries.
    #[test]
    fn previews_and_displayed_texts_are_bounded() {
        let huge = "ü".repeat(MAX_DISPLAY_TEXT_BYTES); // twice the cap in bytes
        let preview = preview_of(&huge);
        assert_eq!(preview.chars().count(), PREVIEW_CHARS + 1); // + the ellipsis
        assert_eq!(preview_of("hi"), "hi");

        let shown = display_text(&huge);
        assert!(shown.len() < MAX_DISPLAY_TEXT_BYTES + 128);
        assert!(shown.contains("KiB more not shown"));
        assert!(matches!(
            display_text("hi"),
            std::borrow::Cow::Borrowed("hi")
        ));
    }
}
