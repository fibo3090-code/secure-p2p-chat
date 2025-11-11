# 🐛 Bug Fixes Summary - Session 2025-11-02

## Overview

This document summarizes the three critical bugs that were fixed in this session, their root causes, and the solutions implemented.

---

## Bug #1: History Not Persisting After Installation ✅ FIXED

### Symptoms
- Conversations and contacts disappeared when app was closed and reopened
- Only occurred when app was installed with an installer (not when run from source)
- History file seemed to save but was not found on next run

### Root Cause
**Problem**: Using relative file path for history storage.

```rust
// OLD CODE (BROKEN):
let history_path = PathBuf::from("Downloads").join("history.json");
```

When the app was installed to `C:\Program Files\AppName\`, the working directory was the installation directory. The app would save to:
- `C:\Program Files\AppName\Downloads\history.json`

But this path:
1. Required admin permissions (Program Files is protected)
2. Changed depending on where the app was installed
3. Was not the standard location for user data

### Solution
**Use platform-specific user data directory** via the `directories` crate.

```rust
// NEW CODE (FIXED):
let history_path = if let Some(proj_dirs) = 
    directories::ProjectDirs::from("com", "chat-p2p", "EncryptedMessenger") {
    let data_dir = proj_dirs.data_dir();
    std::fs::create_dir_all(data_dir).ok();
    data_dir.join("history.json")
} else {
    // Fallback
    PathBuf::from("Downloads").join("history.json")
};
```

**Result**:
- Windows: `%APPDATA%\chat-p2p\history.json`
- Linux: `~/.local/share/chat-p2p/history.json`
- macOS: `~/Library/Application Support/chat-p2p/history.json`

### Files Changed
- `Cargo.toml` - Added `directories = "5"` dependency
- `src/main.rs:148-183` - Updated history path logic with proper cross-platform paths

### Testing
```bash
# Windows
echo %APPDATA%\chat-p2p\
# Should show: C:\Users\YourName\AppData\Roaming\chat-p2p\

# Test
cargo run
# Send some messages, add contacts
# Close app
cargo run
# Verify history loaded
```

---

## Bug #2: Invite Link Same for Everyone ✅ FIXED

### Symptoms
- Generated invite link was always `chat-p2p://invite/YOUR_INVITE_LINK_HERE` (same placeholder for all users)
- Pasting this link would fail or create invalid contacts
- No way to share identity with others

### Root Cause
**Problem**: No persistent identity system + placeholder implementation.

```rust
// OLD CODE (BROKEN):
if self.my_invite_link.is_none() {
    ui.label("🔄 Generating your invite link...");
    // TODO: Get actual fingerprint/pubkey from session
    self.my_invite_link = Some("chat-p2p://invite/YOUR_INVITE_LINK_HERE".to_string());
}
```

The app had:
- No persistent identity (RSA keys regenerated each session)
- No way to access user's public key and fingerprint
- Just a placeholder string for everyone

### Solution
**Implement persistent identity system** with RSA key pairs.

#### Step 1: Create Identity Module
Created `src/identity/mod.rs` with:
- `Identity` struct (name, RSA keys, fingerprint)
- `Identity::new()` - Generate RSA-2048 key pair
- `Identity::save()/load()` - Persist to JSON file
- `Identity::generate_invite_link()` - Create base64-encoded link

```rust
// NEW: Identity system
pub struct Identity {
    pub id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    private_key_pem: Option<String>,
    pub public_key_pem: String,
    pub fingerprint: String, // SHA-256 hash
}

impl Identity {
    pub fn generate_invite_link(&self) -> Result<String> {
        let payload = json!({
            "name": self.name,
            "fingerprint": self.fingerprint,
            "public_key": self.public_key_pem,
        });
        let json = serde_json::to_string(&payload)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        Ok(format!("chat-p2p://invite/{}", encoded))
    }
}
```

#### Step 2: Load Identity on Startup
```rust
// NEW CODE in src/main.rs:
let identity = encodeur_rsa_rust::identity::Identity::get_or_create(data_dir, "User")?;
tracing::info!("Using identity: {} (fingerprint: {}...)", identity.name, &identity.fingerprint[..16]);
```

#### Step 3: Use Real Identity in UI
```rust
// NEW CODE (FIXED):
if self.my_invite_link.is_none() {
    match self.identity.generate_invite_link() {
        Ok(link) => self.my_invite_link = Some(link),
        Err(e) => { /* show error */ }
    }
}
```

### Files Created
- `src/identity/mod.rs` - Complete identity management module (370 lines)

### Files Changed
- `src/lib.rs` - Added identity module export
- `Cargo.toml` - Added `argon2`, `zeroize` dependencies (for future password protection)
- `src/main.rs:97` - Added `identity: Identity` field to App struct
- `src/main.rs:157-163` - Load or create identity on startup
- `src/main.rs:1285-1295` - Generate real invite link from identity

### Invite Link Format
```
chat-p2p://invite/eyJuYW1lIjoiQWxpY2UiLCJmaW5nZXJwcmludCI6ImExYjJjM2Q0ZTVmNi4uLiIsInB1YmxpY19rZXkiOiItLS0tLUJFR0lOIFBVQkxJQyBLRVktLS0tLVxuLi4uIn0=

Decodes to:
{
  "name": "Alice",
  "fingerprint": "a1b2c3d4e5f6...",
  "public_key": "-----\nBEGIN PUBLIC KEY-----\n..."
}
```

### Testing
```bash
# Generate invite link
cargo run
# Go to Contacts → Add Contact → Tab 2 (Share My Link)
# Click "Copy" button

# Paste in another instance
# Go to Contacts → Add Contact → Tab 1 (Invite Link)
# Paste link, click "Add from Link"
# Verify contact added with correct name and fingerprint
```

---

## Bug #3: Group Chat Messages Disappearing ✅ FIXED

### Symptoms
- Messages sent to group chats would not appear in history
- No error message shown to user
- Group chat appeared to work but messages vanished

### Root Cause
**Problem**: Messages only added to history if send succeeded, and added multiple times.

```rust
// OLD CODE (BROKEN):
for contact_id in participants {
    if let Some(one_chat_id) = self.contact_to_chat.get(&contact_id) {
        if let Some(session) = self.sessions.get(one_chat_id) {
            let _ = session.from_app_tx.send(msg.clone());

            // BUG: Message added INSIDE loop, AFTER send
            if let Some(gchat) = self.chats.get_mut(&group_chat_id) {
                gchat.messages.push(Message { /* ... */ });
            }
        }
    }
}
```

**Problems**:
1. Message only added to history if at least one participant had an active session
2. If all participants offline → message never added to history
3. Message added multiple times (once per successful send) - duplicate messages!

### Solution
**Add message to history ONCE before trying to send, show warning for offline participants.**

```rust
// NEW CODE (FIXED):
pub fn send_group_message(&mut self, group_chat_id: Uuid, text: String) -> Result<usize> {
    // 1. Add message to history ONCE (before send loop)
    if let Some(gchat) = self.chats.get_mut(&group_chat_id) {
        gchat.messages.push(Message {
            id: Uuid::new_v4(),
            from_me: true,
            content: MessageContent::Text { text: text.clone() },
            timestamp: chrono::Utc::now(),
        });
    }

    // 2. Try to send to all participants
    let mut sent_count = 0;
    let mut offline_contacts = Vec::new();

    for contact_id in participants {
        if let Some(contact) = self.contacts.get(&contact_id) {
            if let Some(one_chat_id) = self.contact_to_chat.get(&contact_id) {
                if let Some(session) = self.sessions.get(one_chat_id) {
                    if session.from_app_tx.send(msg.clone()).is_ok() {
                        sent_count += 1;
                    }
                } else {
                    offline_contacts.push(contact.name.clone());
                }
            } else {
                offline_contacts.push(contact.name.clone());
            }
        }
    }

    // 3. Show warning toast for offline participants
    if !offline_contacts.is_empty() {
        let message = if sent_count == 0 {
            format!("⚠ Message sent locally but all recipients are offline: {}", offline_contacts.join(", "))
        } else {
            format!("⚠ Sent to {} recipient(s), but offline: {}", sent_count, offline_contacts.join(", "))
        };
        self.add_toast(ToastLevel::Warning, message);
    }

    Ok(sent_count)
}
```

**Additional Fix**: Updated `send_message()` to auto-detect group chats:

```rust
// NEW: send_message() detects group chats automatically
pub fn send_message(&mut self, chat_id: Uuid, text: String) -> Result<()> {
    // Check if this is a group chat (has participants but no direct session)
    let is_group_chat = if let Some(chat) = self.chats.get(&chat_id) {
        chat.participants.len() > 0 && !self.sessions.contains_key(&chat_id)
    } else {
        false
    };

    if is_group_chat {
        // Automatically route to group message handler
        self.send_group_message(chat_id, text)?;
        return Ok(());
    }

    // Otherwise, normal 1-on-1 message
    // ...
}
```

### Files Changed
- `src/app/chat_manager.rs:109-167` - Fixed `send_group_message()` with proper history handling
- `src/app/chat_manager.rs:259-298` - Updated `send_message()` to auto-detect group chats

### User Experience Improvements
**Before**:
- User sends message to group
- Message disappears (if all offline)
- No feedback

**After**:
- User sends message to group
- Message appears in history immediately
- Toast notification shows: "⚠ Message sent locally but all recipients are offline: Alice, Bob"
- Clear feedback about what happened

### Testing
```bash
# Test 1: All participants offline
cargo run
# Create group with 2-3 contacts (don't connect to them)
# Send message to group
# ✅ Message should appear in history
# ✅ Should show warning toast about offline participants

# Test 2: Some participants online
cargo run
# Create group with Alice, Bob, Charlie
# Connect to Alice (establish session)
# Send message to group
# ✅ Message appears in history
# ✅ Message sent to Alice
# ✅ Warning toast: "Sent to 1 recipient(s), but offline: Bob, Charlie"

# Test 3: Message persistence
cargo run
# Send message to group (all offline)
# Close app
cargo run
# ✅ Message should still be in history
```

---

## Impact Summary

### Before Fixes:
- ❌ History lost after app reinstallation
- ❌ Can't share identity with others (invite link broken)
- ❌ Group chat messages disappear
- ❌ Poor user experience (no error messages)

### After Fixes:
- ✅ History persists correctly in user data directory
- ✅ Can share real invite links with name, fingerprint, public key
- ✅ Group chat messages always saved to history
- ✅ Clear warnings when participants are offline
- ✅ Professional user experience

### Technical Improvements:
- ✅ Added persistent identity system (foundation for future features)
- ✅ Cross-platform file paths (Windows/Linux/macOS)
- ✅ Better error handling and user feedback
- ✅ Cleaner code architecture

---

## Verification Checklist

### Test All Fixes:
- [ ] Install app with installer → Close → Reopen → History persists
- [ ] Generate invite link → Copy → Paste in another instance → Contact added
- [ ] Create group → Send message (all offline) → Message appears in history
- [ ] Create group → Connect to one participant → Send → See "offline" warning
- [ ] Restart app → Group messages still visible

### Regression Testing:
- [ ] Can still send 1-on-1 messages
- [ ] Can still connect as host/client
- [ ] Can still send files
- [ ] Can still add contacts manually
- [ ] Can still rename conversations

---

## Future Improvements

### Related to These Fixes:

1. **Identity System Enhancements**:
   - Add password protection (encrypt private key with Argon2)
   - Add first-time setup wizard for user name
   - Add profile picture support
   - Add identity export/import for multi-device

2. **Group Chat Enhancements**:
   - Add proper group chat protocol (true multi-party)
   - Add group member roles (admin, moderator, member)
   - Add ability to remove members
   - Add group invite links

3. **History Enhancements**:
   - Add automatic backup
   - Add import/export functionality
   - Add cloud sync option (end-to-end encrypted)
   - Add search across history

**See**: `FEATURES_ROADMAP.md` for complete list of 48+ future features.

---

*Last Updated: 2025-11-02*
*Session: v1.3.0-dev*
*Bugs Fixed: 3/3 ✅*
