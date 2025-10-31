# 🗺️ Development Plan - Encrypted P2P Messenger

## 📊 Current Status (v1.0.2)

### ✅ What Works
- **Core Functionality**: RSA-2048-OAEP + AES-256-GCM encryption
- **Messaging**: Bidirectional text messaging (bug fixed in v1.0.2)
- **File Transfer**: Chunked file sending/receiving with progress
- **UI/UX**: Modern WhatsApp-like interface with avatars, timestamps, and copy functionality
- **Connection**: TCP-based P2P connections with fingerprint verification
- **Persistence**: JSON-based message history storage

### ⚠️ Known Limitations
1. **No forward secrecy** - session keys derived from long-term RSA keys
2. **No persistent identities** - keys regenerated each session
3. **LAN-only** - no NAT traversal for WAN connectivity
4. **Manual fingerprint verification** - no certificate authority
5. **Some unit tests fail** - mock infrastructure needs updates

---

## 🎯 Development Roadmap

### **Phase 1: Critical Security Enhancements** 🔒
*Estimated: 2-3 weeks*

#### 1.1 Forward Secrecy with X25519 ECDH
**Priority**: HIGH | **Impact**: Critical security improvement

**Changes Required**:
- Add `x25519-dalek` dependency to `Cargo.toml`
- Modify `src/core/crypto.rs`:
  - Add ephemeral key generation
  - Implement ECDH key agreement
  - Derive session keys from shared secret using HKDF
- Update `src/network/session.rs`:
  - Modify handshake to exchange ephemeral keys
  - Keep RSA for identity/authentication
  - Use derived keys for AES-GCM encryption
- Update protocol in `src/core/protocol.rs`:
  - Add `EPHEMERAL_KEY` message type

**Benefits**:
- Past messages remain secure even if long-term keys compromised
- Matches Signal/WhatsApp security model
- No backward compatibility issues (protocol version check)

**Testing**:
- Unit tests for key derivation
- Integration test for new handshake
- Verify old clients gracefully reject connection

#### 1.2 Persistent Identity Storage
**Priority**: HIGH | **Impact**: User experience + security

**Changes Required**:
- Add `argon2` and `zeroize` dependencies
- Create `src/identity/` module:
  - `keystore.rs`: Encrypted key storage
  - `passphrase.rs`: Argon2 key derivation
- Implement:
  - First-time key generation with passphrase
  - Encrypted key file in user data directory
  - Secure key loading with password prompt
  - Key rotation functionality
- Update UI in `src/main.rs`:
  - Add passphrase entry dialog
  - Add "Change Password" in settings
  - Add "Export Identity" feature

**Benefits**:
- Consistent identity across sessions
- Enables contact lists in future
- Better trust model

**Storage Format**:
```json
{
  "version": "1.0",
  "identity_id": "<uuid>",
  "encrypted_private_key": "<base64>",
  "public_key_pem": "<pem>",
  "salt": "<base64>",
  "created_at": "<timestamp>"
}
```

---

### **Phase 2: Connection Reliability** 🔌
*Estimated: 1-2 weeks*

#### 2.1 Heartbeat/Ping System
**Priority**: MEDIUM | **Impact**: Better connection stability

**Changes Required**:
- Modify `src/core/protocol.rs`:
  - Add `HEARTBEAT` and `PONG` message types
- Update `src/network/session.rs`:
  - Add periodic heartbeat (every 30 seconds)
  - Add timeout detection (90 seconds no response)
  - Emit `ConnectionTimeout` event
- Update `src/app/chat_manager.rs`:
  - Handle `ConnectionTimeout` event
  - Auto-reconnect logic (optional)

**Configuration** (in Settings):
- Heartbeat interval (default: 30s)
- Timeout threshold (default: 90s)
- Auto-reconnect enabled (default: true)

#### 2.2 Message Delivery Acknowledgments
**Priority**: MEDIUM | **Impact**: User confidence

**Changes Required**:
- Add message IDs to all messages (UUID)
- Add `src/core/protocol.rs`:
  - `ACK` message type with message ID
  - `READ_RECEIPT` message type
- Update `src/types.rs`:
  - Add `DeliveryStatus` enum (Sending, Sent, Delivered, Read)
  - Add `delivery_status` field to `Message`
- Update UI in `src/main.rs`:
  - Show checkmarks: ✓ (sent), ✓✓ (delivered), ✓✓ (blue, read)
  - Add "read receipts" toggle in settings

**Protocol**:
```
1. Send TEXT message with UUID
2. Receiver sends ACK with UUID
3. Sender updates status to "Delivered"
4. When message viewed, send READ_RECEIPT
5. Sender updates status to "Read"
```

---

### **Phase 3: User Experience Enhancements** 🎨
*Estimated: 2 weeks*

#### 3.1 Drag & Drop File Support
**Priority**: MEDIUM | **Impact**: Convenience

**Changes Required**:
- Update `src/main.rs`:
  - Implement `dropped_files` handler
  - Extract file paths from drop event
  - Show file preview with confirmation
- Handle multiple files (queue or reject)

**UI Flow**:
1. User drags file over chat window
2. Show drop zone indicator
3. On drop: Show file preview
4. Confirm to send

#### 3.2 Typing Indicators
**Priority**: MEDIUM | **Impact**: User engagement

**Changes Required**:
- Add `src/core/protocol.rs`:
  - `TYPING_START` and `TYPING_STOP` messages
- Update `src/network/session.rs`:
  - Send typing notification when user types
  - Debounce (send after 500ms of no typing)
  - Auto-stop after 5 seconds
- Update UI:
  - Show "typing..." indicator in chat header
  - Animate with dots

**Rate Limiting**:
- Max 1 typing notification per 2 seconds
- Prevents spam

#### 3.3 Desktop Notifications
**Priority**: MEDIUM | **Impact**: User awareness

**Changes Required**:
- Add `notify-rust` dependency (cross-platform)
- Create `src/notifications.rs`:
  - `show_message_notification()`
  - `show_file_notification()`
  - Request notification permissions
- Update `src/main.rs`:
  - Trigger notifications when window not focused
  - Add "Enable Notifications" toggle in settings
  - Platform-specific icon

**Notification Content**:
```
Title: "New message from [Peer Name]"
Body: "[First 50 chars of message]..."
Icon: App icon
Action: Focus window + show chat
```

---

### **Phase 4: Enhanced Features** ⭐
*Estimated: 2-3 weeks*

#### 4.1 Emoji Picker
**Priority**: LOW | **Impact**: Modern UX

**Implementation**:
- Use `egui-extras` or custom picker
- Common emoji categories
- Search functionality
- Recently used emojis

#### 4.2 Message Search
**Priority**: MEDIUM | **Impact**: Usability

**Changes Required**:
- Add search bar in chat panel
- Implement full-text search in message history
- Highlight matches
- Jump to message functionality
- Search across all chats option

**Algorithm**:
- Simple substring match for MVP
- Case-insensitive
- Support regex in future

#### 4.3 Image Preview Inline
**Priority**: MEDIUM | **Impact**: Visual richness

**Changes Required**:
- Detect image files (jpg, png, gif, webp)
- Load image data
- Display thumbnail in message bubble
- Click to open full-size
- Lazy loading for performance

**Supported Formats**:
- JPG, PNG, GIF (via `image` crate)
- WebP support
- Max preview size: 300x300px
- Cache thumbnails

---

### **Phase 5: Testing & Quality** 🧪
*Estimated: 1-2 weeks*

#### 5.1 Fix Unit Test Failures
**Priority**: HIGH | **Impact**: Code quality

**Issues**:
- Transfer tests use `DuplexStream` instead of `TcpStream`
- Mock infrastructure needs abstraction

**Solution**:
- Create trait `AsyncStream` for read/write
- Implement for both `TcpStream` and `DuplexStream`
- Update test helpers

#### 5.2 Integration Tests
**Priority**: HIGH | **Impact**: Reliability

**Test Scenarios**:
1. **Full Handshake Test**:
   - Start host, connect client
   - Verify encryption established
   - Verify fingerprints match
2. **Message Flow Test**:
   - Send/receive text messages
   - Verify delivery
   - Check persistence
3. **File Transfer Test**:
   - Send small file (1 KB)
   - Send large file (10 MB)
   - Verify integrity (checksum)
4. **Connection Drop Test**:
   - Simulate network failure
   - Verify cleanup
   - Test reconnection
5. **Stress Test**:
   - 1000 rapid messages
   - Multiple large files
   - Memory leak detection

**Test Framework**:
```rust
#[tokio::test]
async fn test_full_conversation() {
    // Setup host and client
    // Exchange messages
    // Verify received
    // Cleanup
}
```

---

### **Phase 6: Advanced Features** 🚀
*Estimated: 3-4 weeks*

#### 6.1 Message Grouping by Date
**Priority**: LOW | **Impact**: Organization

**UI Changes**:
- Date separators (e.g., "Today", "Yesterday", "March 15, 2025")
- Group messages within same day
- Smooth scrolling between dates

#### 6.2 Export Conversations
**Priority**: LOW | **Impact**: Data portability

**Export Formats**:
1. **JSON**: Raw data export
2. **HTML**: Styled, readable format
3. **PDF**: Professional document (via `printpdf`)
4. **TXT**: Plain text backup

**UI**:
- Settings → Export Chat
- Select chat and format
- Include/exclude file attachments option

#### 6.3 Theme Toggle
**Priority**: LOW | **Impact**: Personalization

**Implementation**:
- Dark mode (current)
- Light mode
- Auto (follow system)
- Custom accent colors

**Themes**:
```rust
struct Theme {
    background: Color32,
    text: Color32,
    sent_bubble: Color32,
    received_bubble: Color32,
    accent: Color32,
}
```

---

## 🔮 Future Considerations (v2.0+)

### Group Chats
- Multi-party encryption (complex!)
- Member management
- Protocol redesign required

### NAT Traversal
- STUN/TURN servers
- Hole punching
- ICE candidate exchange
- Requires centralized signaling server

### Mobile Apps
- Android (Kotlin + Rust FFI)
- iOS (Swift + Rust FFI)
- Shared core logic
- Platform-specific UI

### Voice/Video Calls
- WebRTC integration
- Encrypted voice data
- P2P negotiation
- Major feature addition

---

## 📈 Priority Matrix

### Must Have (v1.1)
1. ✅ Fix unit tests
2. 🔒 Forward secrecy
3. 🔑 Persistent identities
4. 💓 Heartbeat system

### Should Have (v1.2)
5. ✓✓ Delivery acknowledgments
6. 📁 Drag & drop files
7. ⌨️ Typing indicators
8. 🔔 Desktop notifications

### Nice to Have (v1.3)
9. 😊 Emoji picker
10. 🔍 Message search
11. 🖼️ Image previews
12. 🎨 Theme toggle

### Future (v2.0+)
13. 👥 Group chats
14. 🌐 NAT traversal
15. 📱 Mobile apps
16. 📞 Voice/video calls

---

## 🛠️ Development Workflow

### For Each Feature:
1. **Design**: Write technical spec
2. **Prototype**: Implement in feature branch
3. **Test**: Unit + integration tests
4. **Review**: Code review checklist
5. **Document**: Update README and CHANGELOG
6. **Deploy**: Merge to main

### Code Quality Checklist:
- [ ] All tests pass
- [ ] No clippy warnings
- [ ] Code formatted (`cargo fmt`)
- [ ] Documentation updated
- [ ] CHANGELOG updated
- [ ] No performance regression
- [ ] Security review (if crypto changes)

---

## 📊 Estimated Timeline

- **Phase 1** (Security): 2-3 weeks
- **Phase 2** (Reliability): 1-2 weeks
- **Phase 3** (UX): 2 weeks
- **Phase 4** (Features): 2-3 weeks
- **Phase 5** (Testing): 1-2 weeks
- **Phase 6** (Advanced): 3-4 weeks

**Total**: ~11-16 weeks for complete roadmap

**MVP for v1.1** (Phases 1-2): ~4 weeks

---

## 🎯 Success Metrics

### User Metrics
- Time to first message < 2 minutes
- Zero crashes during normal use
- File transfers succeed > 99% of time
- User satisfaction rating > 4.5/5

### Technical Metrics
- Test coverage > 80%
- Build time < 2 minutes
- Memory usage < 100 MB baseline
- File transfer speed > 10 MB/s on LAN

### Security Metrics
- Zero critical vulnerabilities
- Forward secrecy implemented
- Encryption verified by security audit
- Passphrase strength enforced

---

## 📝 Notes

- This plan is flexible - priorities may shift based on user feedback
- Each phase can be developed independently
- Backward compatibility maintained where possible
- Security changes may require protocol version bump
- Regular releases preferred over big bang releases

---

**Last Updated**: 2025-10-31
**Current Version**: 1.0.2
**Target Version**: 2.0.0
**Status**: 🟢 Active Development
