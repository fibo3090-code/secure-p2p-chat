//! Text messaging: chunked send/reassembly of large messages,
//! broadcast, typing indicators, and notification previews.

use super::*;

pub(super) struct IncomingTextMessage {
    pub(super) timestamp_millis: u64,
    pub(super) parts: Vec<Option<String>>,
    pub(super) updated_at: std::time::Instant,
}

impl ChatManager {
    pub(super) fn build_text_protocol_messages(
        send_seq: &mut u64,
        text: &str,
        timestamp: u64,
    ) -> Result<Vec<ProtocolMessage>> {
        if text.len() <= crate::MAX_TEXT_MESSAGE_BYTES {
            *send_seq += 1;
            return Ok(vec![ProtocolMessage::Text {
                text: text.to_string(),
                timestamp,
                seq: *send_seq,
            }]);
        }

        let chunks = Self::split_text_chunks(text, crate::TEXT_CHUNK_BYTES);
        if chunks.is_empty() {
            bail!("Message is empty");
        }

        let message_id = Uuid::new_v4();
        let total_chunks = u32::try_from(chunks.len())
            .map_err(|_| anyhow!("Message is too large to chunk safely"))?;
        // Symmetric with the receiver's decode cap: refuse to build a message
        // the peer would reject anyway, so the user gets a clear error instead
        // of a silently dropped message.
        if total_chunks > crate::MAX_TEXT_CHUNKS {
            bail!(
                "Message is too large to send (would need {} chunks, max {})",
                total_chunks,
                crate::MAX_TEXT_CHUNKS
            );
        }
        let mut messages = Vec::with_capacity(chunks.len());

        for (chunk_index, text_part) in chunks.into_iter().enumerate() {
            *send_seq += 1;
            messages.push(ProtocolMessage::TextChunk {
                message_id,
                chunk_index: chunk_index as u32,
                total_chunks,
                text_part,
                timestamp,
                seq: *send_seq,
            });
        }

        Ok(messages)
    }

    fn split_text_chunks(text: &str, max_bytes: usize) -> Vec<String> {
        debug_assert!(max_bytes > 0);

        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut current_bytes = 0usize;

        for ch in text.chars() {
            let ch_bytes = ch.len_utf8();
            if current_bytes > 0 && current_bytes + ch_bytes > max_bytes {
                chunks.push(std::mem::take(&mut current));
                current_bytes = 0;
            }
            current.push(ch);
            current_bytes += ch_bytes;
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        chunks
    }

    pub(super) fn cleanup_stale_incoming_text_messages(&mut self) {
        const INCOMING_TEXT_TIMEOUT: Duration = Duration::from_secs(120);

        let now = std::time::Instant::now();
        let stale_keys: Vec<(Uuid, Uuid)> = self
            .incoming_text_messages
            .iter()
            .filter_map(|(key, pending)| {
                if now.duration_since(pending.updated_at) >= INCOMING_TEXT_TIMEOUT {
                    Some(*key)
                } else {
                    None
                }
            })
            .collect();

        for (chat_id, _message_id) in stale_keys {
            self.incoming_text_messages.remove(&(chat_id, _message_id));
            self.add_toast(
                ToastLevel::Warning,
                "A large incoming message could not be completed and was discarded.".to_string(),
            );
        }
    }

    pub(super) fn register_incoming_text_chunk(
        &mut self,
        chat_id: Uuid,
        message_id: Uuid,
        chunk_index: u32,
        total_chunks: u32,
        text_part: String,
        timestamp_millis: u64,
    ) -> Result<Option<(String, chrono::DateTime<chrono::Utc>)>> {
        // Defense in depth: the wire decoder already caps `total_chunks`, but
        // guard here too so this buffer can never pre-allocate an unbounded
        // number of slots regardless of how it is reached.
        if total_chunks == 0 || total_chunks > crate::MAX_TEXT_CHUNKS {
            bail!("Invalid chunk count for large text message");
        }
        let key = (chat_id, message_id);
        // Bound the number of concurrent partial messages per chat so a peer
        // cannot open many reassembly buffers and sit on memory until timeout.
        if !self.incoming_text_messages.contains_key(&key) {
            let in_flight = self
                .incoming_text_messages
                .keys()
                .filter(|(c, _)| *c == chat_id)
                .count();
            if in_flight >= crate::MAX_CONCURRENT_PARTIAL_TEXT_PER_CHAT {
                bail!("Too many concurrent large messages in flight for this chat");
            }
        }
        let entry = self
            .incoming_text_messages
            .entry(key)
            .or_insert_with(|| IncomingTextMessage {
                timestamp_millis,
                parts: vec![None; total_chunks as usize],
                updated_at: std::time::Instant::now(),
            });

        if entry.parts.len() != total_chunks as usize {
            bail!("Chunk count mismatch for large text message");
        }

        let index = chunk_index as usize;
        if index >= entry.parts.len() {
            bail!("Chunk index out of bounds for large text message");
        }

        entry.timestamp_millis = timestamp_millis;
        entry.updated_at = std::time::Instant::now();
        entry.parts[index] = Some(text_part);

        if entry.parts.iter().all(Option::is_some) {
            let parts = self
                .incoming_text_messages
                .remove(&(chat_id, message_id))
                .ok_or_else(|| anyhow!("Large text message disappeared during reassembly"))?
                .parts;
            let text = parts
                .into_iter()
                .collect::<Option<Vec<String>>>()
                .ok_or_else(|| anyhow!("Large text message reassembly failed"))?
                .join("");
            let timestamp =
                chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_millis as i64)
                    .unwrap_or_else(chrono::Utc::now);
            return Ok(Some((text, timestamp)));
        }

        Ok(None)
    }

    pub(super) fn preview_text_for_notification(text: &str) -> String {
        const MAX_PREVIEW_CHARS: usize = 50;
        let truncated: String = text.chars().take(MAX_PREVIEW_CHARS).collect();
        if text.chars().count() > MAX_PREVIEW_CHARS {
            format!("{}...", truncated)
        } else {
            truncated
        }
    }

    /// Send a text message over the chat's active session.
    pub fn send_message(&mut self, chat_id: Uuid, text: String) -> Result<()> {
        tracing::debug!(
            "send_message called for chat_id={}, len(text)={} chars",
            chat_id,
            text.len()
        );

        // Resolve chat_id through mapping (in case this is an incoming connection with a different chat_id)
        let actual_session_chat_id = self
            .chat_id_mapping
            .get(&chat_id)
            .copied()
            .unwrap_or(chat_id);

        let has_session = self.chats.contains_key(&chat_id)
            && self.sessions.contains_key(&actual_session_chat_id);

        // One-to-one chat path
        if !has_session {
            // An established conversation (fingerprint confirmed) without a
            // session means the peer dropped — telling the user "connecting"
            // there is a lie that hides why their message went nowhere.
            let was_established = self
                .chats
                .get(&chat_id)
                .is_some_and(|c| c.peer_fingerprint.is_some());
            tracing::warn!(
                "No active session for 1:1 chat {} (mapped to {}); established={}",
                chat_id,
                actual_session_chat_id,
                was_established,
            );
            if was_established {
                self.add_toast(
                    ToastLevel::Error,
                    "Not delivered: the peer is disconnected. Reconnect (or ask them to) and try again.".to_string(),
                );
            } else {
                self.add_toast(
                    ToastLevel::Info,
                    "Connecting... please wait before sending messages".to_string(),
                );
            }
            return Ok(()); // Do not error; just inform the user and skip sending
        }

        let session = self
            .sessions
            .get(&actual_session_chat_id)
            .ok_or_else(|| anyhow::anyhow!("Session should exist but was not found"))?;

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found for sending message"))?;

        let timestamp = crate::util::current_timestamp_millis();
        let messages = Self::build_text_protocol_messages(&mut chat.send_seq, &text, timestamp)?;

        for msg in messages {
            if let Err(e) = session.from_app_tx.send(msg) {
                tracing::error!("Failed to send message to chat {}: {}", chat_id, e);
                return Err(e.into());
            }
        }

        // Add to local history
        chat.messages.push(Message {
            id: Uuid::new_v4(),
            from_me: true,
            content: MessageContent::Text { text },
            timestamp: chrono::Utc::now(),
        });

        Ok(())
    }

    /// Send typing start indicator
    pub fn send_typing_start(&mut self, chat_id: Uuid) -> Result<()> {
        if !self.config.enable_typing_indicators {
            return Ok(());
        }

        let actual_id = self.chat_id_mapping.get(&chat_id).unwrap_or(&chat_id);
        let session = self
            .sessions
            .get(actual_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found"))?;
        chat.send_seq += 1;
        session
            .from_app_tx
            .send(ProtocolMessage::TypingStart { seq: chat.send_seq })?;
        Ok(())
    }

    /// Send typing stop indicator
    pub fn send_typing_stop(&mut self, chat_id: Uuid) -> Result<()> {
        if !self.config.enable_typing_indicators {
            return Ok(());
        }

        let actual_id = self.chat_id_mapping.get(&chat_id).unwrap_or(&chat_id);
        let session = self
            .sessions
            .get(actual_id)
            .ok_or_else(|| anyhow::anyhow!("Session not found"))?;

        let chat = self
            .chats
            .get_mut(&chat_id)
            .ok_or_else(|| anyhow::anyhow!("Chat not found"))?;
        chat.send_seq += 1;
        session
            .from_app_tx
            .send(ProtocolMessage::TypingStop { seq: chat.send_seq })?;
        Ok(())
    }
}
