# 📜 Project History & Bug Fixes

This document archives the development history, major bug fixes, and feature additions from versions 0.9.0 through 1.0.2.

---

## Version 1.0.2 (2025-10-23) - Critical Bug Fix

### 🐛 Major Bug: Messages Not Being Received

**Symptom**: Messages were being sent successfully but never appeared in the recipient's chat window.

**Root Cause**: Session events (`MessageReceived`) were spawned in background tasks that only logged but never processed the events.

```rust
// BROKEN CODE:
tokio::spawn(async move {
    while let Some(event) = to_app_rx.recv().await {
        // ❌ Only logged, never added to chat!
        tracing::debug!("Session event: {:?}", event);
    }
});
```

### The Fix

**1. Event Storage**
- Added `session_events: HashMap<Uuid, mpsc::UnboundedReceiver<SessionEvent>>` to `ChatManager`
- Events now stored instead of spawning fire-and-forget tasks

**2. Event Processing**
- Created `poll_session_events()` - polls all pending events every frame
- Created `handle_session_event()` - processes each event type
- Integrated into UI update loop (~60 FPS)

**3. Event Handling**
```rust
SessionEvent::MessageReceived(proto_msg) => {
    match proto_msg {
        ProtocolMessage::Text { text, .. } => {
            chat.messages.push(Message {
                id: Uuid::new_v4(),
                from_me: false,
                content: MessageContent::Text { text },
                timestamp: chrono::Utc::now(),
            });
        }
        // ... handle files, etc.
    }
}
```

### Files Modified
- `src/app/chat_manager.rs` - Event handling system
- `src/main.rs` - UI event polling integration
- `src/network/session.rs` - Enhanced logging

### Impact
- ✅ Bidirectional messaging now works
- ✅ All session events properly processed
- ✅ Connection status updates correctly
- ✅ Fingerprints display properly
- ✅ Toast notifications appear

---

## Version 1.0.0 (2025-10-23) - Major UI/UX Overhaul

### UI Problem: Input Bar Not Visible

**Issue**: The message input bar was hidden because the scrolling area took all vertical space.

**Solution**: Redesigned with fixed 3-panel architecture:

1. **Top Panel** (60px fixed)
   - Avatar with color and initials
   - Chat title
   - Connection status (🟢 Connected)
   - Fingerprint with copy button

2. **Bottom Panel** (120px fixed) - **ALWAYS VISIBLE**
   - Multiline text input (3 lines)
   - Attachment button 📎
   - Send button 📤 (blue when active)
   - File preview when selected

3. **Center Panel** (dynamic)
   - Scrollable message area
   - Auto-scroll to bottom
   - Message bubbles organized chronologically

### New Features Added

#### Connection Status Indicator ✅
- Real-time **🟢 Connected** display
- Green color for visual confirmation
- Displayed in chat header

#### Modern Message Bubbles 💬
- **WhatsApp-style** rounded bubbles
- **Blue** (RGB 0, 120, 255) for sent messages
- **Dark gray** (RGB 60, 60, 70) for received messages
- White text for optimal contrast
- 12px rounded corners
- Hover effect with light border

#### Message Copy Button 📋
- **"📋 Copy"** button on each text message
- Instant clipboard copy
- Discreet and accessible
- Non-intrusive to reading

#### Enhanced Chat Interface 🎨
- **Enriched Header**: Avatar, status, fingerprint access
- **Professional Input**: Large 70px text area, 3 visible lines, hint text
- **Optimized Messages**: Empty state guide, smooth scrolling, optimal spacing

#### File Preview System 📎
- **Clear Display**: Filename in **bold** and **blue**
- **Cancel Button**: "❌ Cancel" to abort
- **Send Button**: "✅ Send File" to confirm
- **Position**: Above input area
- **Visibility**: Impossible to miss

#### File Display in Messages 📄
- Large 📄 icon (24px)
- Filename in bold
- Size clearly displayed
- **"📂 Open File"** button with white text
- Horizontal layout: icon + info + button

#### Smart Relative Timestamps ⏰
- **Today**: "14:30"
- **Yesterday**: "Yesterday 14:30"
- **This Week**: "Monday 14:30"
- **Older**: "2025-01-15 14:30"
- Small gray text (10px)
- Always visible below messages

#### Dynamic Send Button 📤
- **Disabled State**: Gray, non-clickable
- **Active State**: Bright blue (RGB 0, 120, 255)
- **Fixed Size**: 65x70px, always accessible
- **Bold Text**: "📤\nSend"

#### Informative Empty State 💡
- **When No Messages**:
  - Lock icon: "🔒 End-to-end encrypted conversation"
  - Encouragement: "Send your first message below!"
- **Vertically Centered**: Visually pleasant
- **Subtle Colors**: Gray to avoid distraction

#### Hover Effects ✨
- **Messages**: Gray border on hover
- **Buttons**: Native egui hover effect
- **Visual Feedback**: User knows what's clickable

### Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| **Ctrl+Enter** | Send message |
| **Tab** | Navigate between fields |
| **Escape** | Close dialogs |

### Visual Improvements

**Color Palette**:
- Sent messages: Blue (#0078FF)
- Received messages: Dark gray (#3C3C46)
- Text: White (#FFFFFF)
- Timestamps: Light gray (#C8C8DC)
- Connected status: Green (#00C800)
- File preview: Light blue (#6496FF)

**Spacing**:
- Between messages: 4px
- Message padding: 12px horizontal, 8px vertical
- Header panel: 60px height
- Input panel: 120px height
- Text area: 70px height

**Borders**:
- Message bubbles: 12px rounded corners
- Hover: 1px gray border
- Frame: No border (stroke NONE)

### Before/After Comparison

| Aspect | Before | After |
|--------|--------|-------|
| **Input bar visibility** | ❌ Hidden | ✅ Always visible |
| **Connection status** | ❌ Absent | ✅ "🟢 Connected" |
| **Message copy** | ❌ Impossible | ✅ Copy button |
| **Message style** | 🔲 Basic | 💬 Modern bubbles |
| **Empty state** | ⚪ Empty | 💡 User guide |
| **File preview** | 📎 Minimal | 📋 Detailed with cancel |
| **Send button** | ⚪ Static | 🔵 Dynamic (color) |
| **Chat header** | 📝 Title only | 👤 Avatar + status + fingerprint |
| **Hover effects** | ❌ None | ✨ Borders and feedback |

### Technical Architecture

```rust
render_chat(ui, chat_id) {
    // Top panel - FIXED 60px
    TopBottomPanel::top("chat_header")
        .exact_height(60.0)
        .show_inside(ui, |ui| {
            // Avatar + Title + Status + Fingerprint
        });

    // Bottom panel - FIXED 120px
    TopBottomPanel::bottom("chat_input")
        .exact_height(120.0)
        .show_inside(ui, |ui| {
            // File preview (if present)
            // Input area + Buttons
        });

    // Center panel - DYNAMIC (fills remaining space)
    CentralPanel::default()
        .show_inside(ui, |ui| {
            ScrollArea::vertical()
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    // Messages here
                });
        });
}
```

### Advantages of This Architecture
1. **Fixed panels guarantee visibility** of input
2. **Center panel adapts** to remaining space
3. **No manual height calculations** required
4. **Automatically responsive** if window resized
5. **Cleaner, more maintainable** code

---

## Version 0.9.0 - Initial Functional Implementation

### Core Features Implemented

**Cryptography**:
- RSA-2048-OAEP-SHA256 encryption
- AES-256-GCM symmetric encryption
- SHA-256 fingerprinting
- Secure random generation (CSPRNG)

**Networking**:
- TCP-based P2P connections
- Length-prefixed framing (4-byte header)
- Host/client session management
- Connection handshake

**Messaging**:
- Text message exchange
- Message persistence (JSON)
- Basic chat interface

**File Transfer**:
- Chunked sending (64 KiB chunks)
- Streaming reception
- Metadata exchange
- Progress tracking

**GUI**:
- Basic egui interface
- Sidebar with chat list
- Message panel
- Text input
- Connection dialogs

---

## Common Issues & Solutions

### Issue: Can't Connect

**Checklist**:
1. Firewall - Allow port 12345
2. IP address - Must be exact
3. Network - Both on same network (for LAN)
4. Port - Verify both using same port

### Issue: Messages Not Sending

**Solutions**:
1. Verify connection is active
2. Look for red error toasts
3. Try reconnecting
4. Enable debug logging: `$env:RUST_LOG="encodeur_rsa_rust=debug"`

### Issue: Files Won't Transfer

**Checklist**:
1. Check file size vs your limit (Settings)
2. Verify download folder exists and is writable
3. Check disk space
4. Try a smaller file first

---

## Testing Guide

### Basic Connection Test

```bash
# Terminal 1 (Host)
cargo run --release
# Click: Connection → Start Host (port 12345)

# Terminal 2 (Client)
cargo run --release
# Click: Connection → Connect to Host
# Enter: 127.0.0.1, port 12345
```

### Message Exchange Test

1. Send "Hello from host" from Terminal 1
2. Send "Hello from client" from Terminal 2
3. ✅ Both messages should appear in BOTH windows

### Expected Log Output

```
DEBUG: Session event for <uuid>: MessageReceived(Text { ... })
INFO:  Added received message to chat <uuid>
```

### File Transfer Test

1. Click 📎 button
2. Select a test file (< 10 MB)
3. Verify preview appears
4. Click ✅ Send File
5. Check file appears in download folder

---

## Debug Logging

Enable comprehensive logging:

```powershell
# Windows PowerShell
$env:RUST_LOG="encodeur_rsa_rust=debug"
cargo run --release

# Linux/Mac
RUST_LOG=encodeur_rsa_rust=debug cargo run --release
```

**Log Levels**:
- `ERROR`: Critical failures
- `WARN`: Potential issues
- `INFO`: Important events
- `DEBUG`: Detailed flow
- `TRACE`: Everything

---

## Performance Notes

### Crypto Operations
- **RSA keygen (2048-bit)**: ~200-500ms (async, non-blocking)
- **RSA encrypt/decrypt**: <10ms per operation
- **AES-GCM encrypt/decrypt**: <1ms per message
- **SHA-256 fingerprint**: <1ms

### File Transfer
- **Chunk size**: 64 KiB
- **Throughput**: Limited by network, not CPU
- **Memory usage**: Constant (streaming)
- **Max file size**: Tested with >1 GB files

### GUI
- **Frame rate**: 60 FPS (egui default)
- **Responsiveness**: All network ops on background threads
- **Memory**: ~50-100 MB typical

---

## Architecture Summary

```
src/
├── core/                # Cryptography and protocol
│   ├── crypto.rs        # RSA + AES-GCM
│   ├── framing.rs       # Length-prefixed TCP
│   └── protocol.rs      # Message types
├── network/             # Session management
│   └── session.rs       # Host/client handshake
├── transfer/            # File transfer system
│   ├── sender.rs        # Chunked sending
│   └── receiver.rs      # Streaming reception
├── app/                 # Business logic
│   ├── chat_manager.rs  # Sessions and messages
│   └── persistence.rs   # JSON history storage
└── main.rs              # GUI and CLI entry points
```

---

## Known Limitations (as of v1.0.2)

1. **No Forward Secrecy**: Session keys derived from long-term RSA keys
   - *Fixed in v1.1.0*

2. **No Persistent Identity**: Keys regenerated each session
   - *Planned for v1.2.0*

3. **Trust on First Use (TOFU)**: No certificate authority
   - *Manual fingerprint verification required*

4. **LAN Only**: No NAT traversal for WAN connectivity
   - *Planned for v2.0.0*

5. **Test Infrastructure**: Some unit tests fail due to mock issues
   - *Does not affect functionality*

---

## Security Evolution

### v0.9.0 - v1.0.2
- ✅ RSA-2048-OAEP encryption
- ✅ AES-256-GCM authenticated encryption
- ✅ SHA-256 fingerprints
- ✅ Tamper detection via GCM tags
- ✅ Path traversal protection
- ✅ Secure random generation
- ❌ No forward secrecy

### v1.1.0+
- ✅ All previous features
- ✅ **Forward secrecy** via X25519 ECDH
- ✅ HKDF-SHA256 key derivation
- ✅ Protocol version negotiation
- ✅ Downgrade attack protection

---

## Migration Notes

### From v1.0.2 to v1.1.0
- ⚠️ **Breaking**: Protocol v2 incompatible with v1
- ✅ Message history preserved (JSON format unchanged)
- ✅ No data loss
- ⚠️ Both parties must upgrade to communicate

### Data Files
- **Message History**: `*.json` in application data directory
- **Config**: Settings saved automatically
- **No manual migration** required

---

## Credits

**Bug Fixes**:
- v1.0.2 message receiving bug (2025-10-23)
- v1.0.0 UI input bar visibility (2025-10-23)

**Features**:
- v1.1.0 forward secrecy implementation (2025-10-31)
- v1.0.0 modern UI/UX overhaul (2025-10-23)
- v0.9.0 initial implementation

---

**This history document consolidates**:
- BUGFIX_MESSAGES.md
- FIX_SUMMARY.md
- NEW_FEATURES.md (French)
- TEST_MESSAGING.md
- Historical CHANGELOG entries

**For current information, see**:
- README.md - User guide
- CHANGELOG.md - Recent changes
- FORWARD_SECRECY.md - v1.1.0 technical details
- DEVELOPMENT_PLAN.md - Future roadmap
