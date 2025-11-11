# 📋 Audit & Changes Documentation

**Project**: Encrypted P2P Messenger
**Version**: 1.2.0 → 1.3.0+
**Date**: 2025-11-02
**Purpose**: Comprehensive audit, UX improvements, and GitHub preparation

---

## 📊 Executive Summary

This document consolidates all findings from the codebase audit and provides a roadmap for UX improvements, documentation reorganization, and GitHub readiness.

### Key Findings
- ✅ **Cryptography**: Industry-standard (RSA-2048, AES-256-GCM, X25519 forward secrecy)
- ✅ **Architecture**: Well-structured, modular code
- ⚠️ **Contact UX**: Requires manual fingerprint/UUID entry (not user-friendly)
- ⚠️ **Group Chat**: Complex workflow, no moderation features
- ⚠️ **Documentation**: Needs reorganization for GitHub visibility
- ⚠️ **Rename Button**: Hidden on right side of screen

### Priority Actions
1. 🔴 **P0 (Critical)**: Create README.md ✅ DONE
2. 🔴 **P0 (Critical)**: Create SECURITY.md ✅ DONE
3. 🔴 **P0 (Critical)**: Update .gitignore ✅ DONE
4. 🟡 **P1 (High)**: Improve contact selection UX
5. 🟡 **P1 (High)**: Make rename button accessible

---

## 🗺️ Architecture Mapping

### File Structure & Responsibilities

| File/Module | Lines of Code | Responsibility | Key Functions |
|-------------|---------------|----------------|---------------|
| `src/main.rs` | ~2,436 | GUI, UI logic, event handling | `render_sidebar`, `render_chat`, emoji picker |
| `src/types.rs` | ~148 | Data structures | `Chat`, `Contact`, `Message`, `Config` |
| `src/app/chat_manager.rs` | ~735 | Business logic, sessions | `add_contact`, `create_group_chat`, `send_message` |
| `src/core/crypto.rs` | ~450 | Cryptography | RSA, AES-GCM, X25519 ECDH, fingerprints |
| `src/core/protocol.rs` | ~300 | Message types | `ProtocolMessage` enum, parsing |
| `src/network/session.rs` | ~600 | Network sessions | `run_host_session`, `run_client_session` |
| `src/transfer/` | ~400 | File transfers | `send_file`, `IncomingFile` |

**Total Codebase**: ~5,069 lines of Rust

### UI Component Locations

| Feature | File | Line Range | Notes |
|---------|------|------------|-------|
| **Contact Add Dialog** | `src/main.rs` | 997-1108 | Manual fingerprint entry (needs improvement) |
| **Group Creation** | `src/main.rs` | 1109-1200 | Basic selection UI (needs wizard) |
| **Rename Dialog** | `src/main.rs` | 1201-1250 | Modal dialog (should be inline) |
| **Chat Header** | `src/main.rs` | 393-463 | Rename button on line 455 |
| **Sidebar** | `src/main.rs` | 183-306 | Chat list with avatars |

### Data Flow

```
User Action (GUI)
    ↓
ChatManager (business logic)
    ↓
SessionHandle (network communication)
    ↓
TcpStream (encrypted with AES-256-GCM)
    ↓
Peer Application
```

---

## 🔍 Keyword Search Results

### Occurrences by Category

| Keyword | Count | Files Affected | Context |
|---------|-------|----------------|---------|
| `contact` | 135 | 5 files | Contact management, UI, persistence |
| `group` | 21 | 3 files | Group chat creation, messaging |
| `conversation` | 90 | 6 files | Chat management, renaming |
| `rename` | 18 | 2 files | Conversation renaming feature |
| `fingerprint` | 87 | 8 files | Cryptographic identity verification |
| `id` / `uuid` | 243 | 11 files | Internal identifiers (not exposed to user) |
| `member` | 5 | 2 files | Group participants |
| `admin` / `moderator` | 0 | 0 files | ⚠️ NOT IMPLEMENTED |

### Critical Findings

#### 1. Contact Management (lines 997-1108 in main.rs)
```rust
// Current implementation - PROBLEMATIC
new_contact_name: String,
new_contact_fingerprint: String,  // ⚠️ User must enter 64-char hex!
new_contact_pubkey: String,        // ⚠️ User must paste PEM key!
```

**Problem**: Users expected to manually enter:
- 64-character hexadecimal fingerprint
- Multi-line PEM-encoded RSA public key

**Impact**: Extremely poor UX, high error rate

#### 2. Group Chat (lines 83-107 in chat_manager.rs)
```rust
pub fn create_group_chat(&mut self, participants: Vec<Uuid>, title: Option<String>) -> Uuid {
    // Takes UUIDs directly - no user-friendly selection
}
```

**Problem**: No UI wizard for group creation, no role system

#### 3. Rename Button (line 455 in main.rs)
```rust
// Hidden on far right of chat header
if ui.button("✏️ Rename").on_hover_text("Rename conversation").clicked() {
```

**Problem**: Not discoverable, requires scrolling on small screens

---

## 🎨 UX Improvements Specification

### Issue #1: Contact Selection by Name (not ID)

**Current Flow**:
1. User clicks "Add Contact"
2. Modal requires: name, fingerprint (64 hex chars), public key (PEM)
3. User must manually copy/paste from another app

**Proposed Flow**:
1. User clicks "Add Contact"
2. **Three tabs**:
   - **Search**: Find contacts on LAN (mDNS discovery)
   - **QR Code**: Scan or upload QR code image
   - **Invite Link**: Paste `chat-p2p://invite/...` URL
3. Fingerprint automatically validated
4. One-click add

**Implementation**:
- Add `mdns` crate for LAN discovery
- Add `qrcode` + `rqrr` for QR code support
- Add invitation link generator (base64-encoded contact info)

**Files to Modify**:
- `Cargo.toml` (dependencies)
- `src/app/chat_manager.rs` (+150 LOC)
- `src/main.rs` (UI tabs: +200 LOC)

**Acceptance Criteria**:
- [ ] User can add contact by scanning QR code
- [ ] User can add contact from LAN discovery
- [ ] User can add contact from invite link
- [ ] No manual fingerprint entry required

---

### Issue #2: Inline Conversation Rename

**Current**: Button on far right → modal dialog → type → save

**Proposed**:
- **Option A** (recommended): Inline editable title in chat header
  - Click "✏️" → title becomes text field → save/cancel buttons appear
- **Option B**: Right-click conversation in sidebar → "Rename" context menu
- **Option C**: Keyboard shortcut (F2 or Ctrl+R)

**Implementation** (Option A - Inline Edit):

```rust
// In render_chat header section (line ~427)
if self.rename_mode == Some(chat_id) {
    // Show editable text field
    let response = ui.text_edit_singleline(&mut self.rename_input);
    if ui.button("✅ Save").clicked() || response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        manager.rename_chat(chat_id, self.rename_input.clone());
        self.rename_mode = None;
    }
    if ui.button("❌ Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        self.rename_mode = None;
    }
} else {
    ui.heading(&chat.title);
    if ui.button("✏️").on_hover_text("Rename").clicked() {
        self.rename_mode = Some(chat_id);
        self.rename_input = chat.title.clone();
    }
}
```

**Files to Modify**:
- `src/main.rs` (lines 427-442, ~40 LOC change)

**Acceptance Criteria**:
- [ ] Click pencil icon → title becomes editable
- [ ] Enter key saves, Escape key cancels
- [ ] Works on desktop and mobile
- [ ] No modal dialog required

---

### Issue #3: Group Chat Wizard

**Current**: Direct API call with UUIDs

**Proposed**: Step-by-step wizard modal

**Steps**:
1. **Name & Photo**: Enter group name, optional avatar
2. **Select Members**: Search contacts by name, checkboxes
3. **Invite Link** (optional): Generate shareable link
4. **Permissions**: Choose default role (Member/Moderator)
5. **Create**: Confirm and create group

**Implementation**:

```rust
// In App struct
wizard_step: Option<GroupWizardStep>,
wizard_group_name: String,
wizard_selected: Vec<Uuid>,
wizard_invite_link: Option<String>,

enum GroupWizardStep {
    NameAndPhoto,
    SelectMembers,
    InviteLink,
    Permissions,
    Confirm,
}
```

**Files to Modify**:
- `src/main.rs` (+300 LOC for wizard UI)
- `src/app/chat_manager.rs` (+50 LOC for link generation)

**Acceptance Criteria**:
- [ ] Wizard has 4-5 clear steps
- [ ] Can search contacts by name
- [ ] Can generate invite link
- [ ] Can set default permissions
- [ ] Preview before creating

---

### Issue #4: Role-Based Moderation

**Current**: No roles, all participants equal

**Proposed**: Four-tier role system

| Role | Can Do | Who Can Assign |
|------|--------|----------------|
| **Owner** | Everything, delete group, change owner | N/A (creator) |
| **Admin** | Add/remove members, change roles, delete messages | Owner |
| **Moderator** | Mute users, delete messages, pin messages | Owner, Admin |
| **Member** | Send messages, add reactions | Anyone |

**Implementation**:

```rust
// In types.rs
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum GroupRole {
    Owner,
    Admin,
    Moderator,
    Member,
}

// In Chat struct
pub struct Chat {
    pub participants: Vec<(Uuid, GroupRole)>,  // Changed from Vec<Uuid>
    pub owner_id: Uuid,
    // ...
}

// In protocol.rs
pub enum ProtocolMessage {
    // ...
    GroupMemberUpdate {
        member_id: Uuid,
        new_role: GroupRole
    },
    GroupMemberRemove {
        member_id: Uuid,
        reason: String
    },
    GroupMessageDelete {
        message_id: Uuid,
        by_moderator: Uuid
    },
}
```

**UI Changes**:
- Right-click member → context menu (Promote, Mute, Kick)
- Role badges in member list
- Permission checks before actions

**Files to Modify**:
- `src/types.rs` (+30 LOC)
- `src/core/protocol.rs` (+50 LOC)
- `src/app/chat_manager.rs` (+150 LOC)
- `src/main.rs` (+200 LOC for UI)

**Acceptance Criteria**:
- [ ] Group creator becomes Owner automatically
- [ ] Owner can assign Admin/Moderator roles
- [ ] Admins can add/remove members
- [ ] Moderators can delete messages and mute users
- [ ] Role badges visible in member list

---

## 📚 Documentation Reorganization

### Current Structure (GOOD)
```
docs/
├── DOCS_INDEX.md ✅
├── Community/
├── Technical/
└── Planning/
```

### Missing Critical Files (FIXED)
- ❌ `README.md` at root → ✅ **CREATED**
- ❌ `SECURITY.md` at root → ✅ **CREATED**
- ⚠️ `CONTRIBUTING.md` in docs/ → should be at root
- ⚠️ `CHANGELOG.md` in docs/ → should be at root

### Proposed Actions

#### 1. Move Files to Root (GitHub Standard)
```bash
# Move community files to root
mv docs/Community/CONTRIBUTING.md ./
mv docs/Community/CODE_OF_CONDUCT.md ./

# Move changelog to root
mv docs/Planning/CHANGELOG.md ./

# Keep LICENSE where it is or move to root
```

#### 2. Create User-Facing Docs
```
docs/
├── INDEX.md (rename from DOCS_INDEX.md)
├── user-guide/
│   ├── installation.md
│   ├── quick-start.md
│   ├── features.md
│   └── troubleshooting.md
├── developer/
│   ├── architecture.md (CLAUDE.md)
│   ├── protocol-spec.md
│   ├── forward-secrecy.md
│   └── testing-guide.md
└── project/
    ├── roadmap.md (DEVELOPMENT_PLAN.md)
    └── history.md
```

#### 3. Update Cross-References
All `.md` files need updated links after reorganization.

**Acceptance Criteria**:
- [ ] README.md at root (user-facing)
- [ ] SECURITY.md at root (security policy)
- [ ] CONTRIBUTING.md at root (standard)
- [ ] CHANGELOG.md at root (standard)
- [ ] All internal links updated

---

## 🔧 Ready-to-Apply Patches

### Patch 1: Update .gitignore ✅ COMPLETED

```diff
--- a/.gitignore
+++ b/.gitignore
@@ -88,6 +88,10 @@ keystore.json
 *.db
 *.sqlite

+# Application runtime directories (may contain sensitive user data)
+Downloads/
+temp/
+
 # Private keys and certificates (CRITICAL - never commit these!)
 private_key.pem
 id_rsa
```

**Status**: ✅ Applied

---

### Patch 2: Inline Rename Button (READY TO APPLY)

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -95,6 +95,7 @@ pub struct App {
     input_text: String,
     // ... other fields ...
+    rename_mode: Option<Uuid>,
+    rename_input: String,
     show_rename_dialog: bool,
     rename_chat_id: Option<Uuid>,
-    rename_input: String,
@@ -425,10 +426,28 @@ impl App {
                 // Title and status
                 ui.vertical(|ui| {
-                    ui.heading(&chat.title);
+                    // Inline rename mode
+                    if self.rename_mode == Some(chat_id) {
+                        ui.horizontal(|ui| {
+                            let response = ui.text_edit_singleline(&mut self.rename_input);
+                            if ui.button("✅").clicked() || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
+                                if let Ok(mut mgr) = self.chat_manager.try_lock() {
+                                    let _ = mgr.rename_chat(chat_id, self.rename_input.clone());
+                                }
+                                self.rename_mode = None;
+                            }
+                            if ui.button("❌").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
+                                self.rename_mode = None;
+                            }
+                        });
+                    } else {
+                        ui.horizontal(|ui| {
+                            ui.heading(&chat.title);
+                            if ui.small_button("✏️").on_hover_text("Rename").clicked() {
+                                self.rename_mode = Some(chat_id);
+                                self.rename_input = chat.title.clone();
+                            }
+                        });
+                    }
-                    // Show typing indicator or connection status
                     if chat.peer_typing {
```

**To Apply**:
```bash
cd "chat-p2p"
git apply inline-rename.patch
cargo fmt
cargo build --release
```

**Acceptance Test**:
1. Open chat
2. Click ✏️ button next to title
3. Type new name
4. Press Enter or click ✅
5. Verify name updated in sidebar

---

### Patch 3: Contact Search UI (SPECIFICATION)

**New Dependencies** (add to `Cargo.toml`):
```toml
[dependencies]
# ... existing ...
mdns = "3.0"          # LAN discovery
qrcode = "0.14"       # QR code generation
rqrr = "0.7"          # QR code scanning
base64 = "0.21"       # Invite links
```

**New Functions** (`src/app/chat_manager.rs`):
```rust
impl ChatManager {
    /// Discover peers on LAN using mDNS
    pub fn discover_lan_peers(&self) -> Vec<DiscoveredPeer> {
        // Implementation: mDNS service discovery
        // Look for "_chat-p2p._tcp.local" services
    }

    /// Generate invite link for this user
    pub fn generate_invite_link(&self) -> String {
        let payload = InviteLinkPayload {
            name: "User",  // from config
            fingerprint: self.get_own_fingerprint(),
            public_key: self.get_own_pubkey_pem(),
        };
        let encoded = base64::encode(serde_json::to_string(&payload).unwrap());
        format!("chat-p2p://invite/{}", encoded)
    }

    /// Parse invite link
    pub fn parse_invite_link(&self, link: &str) -> Result<Contact> {
        // Base64 decode → JSON parse → validate → return Contact
    }

    /// Generate QR code for invite
    pub fn generate_invite_qr(&self) -> Vec<u8> {
        let link = self.generate_invite_link();
        qrcode::QrCode::new(link).unwrap().render::<image::Luma<u8>>().build()
    }
}
```

**UI Changes** (`src/main.rs`):
```rust
// In render_add_contact_dialog
egui::Window::new("Add Contact").show(ctx, |ui| {
    ui.horizontal(|ui| {
        if ui.selectable_label(self.contact_tab == 0, "🔍 Search").clicked() {
            self.contact_tab = 0;
        }
        if ui.selectable_label(self.contact_tab == 1, "📷 QR Code").clicked() {
            self.contact_tab = 1;
        }
        if ui.selectable_label(self.contact_tab == 2, "🔗 Link").clicked() {
            self.contact_tab = 2;
        }
    });

    ui.separator();

    match self.contact_tab {
        0 => self.render_contact_search(ui),  // LAN discovery
        1 => self.render_qr_scanner(ui),       // QR upload/scan
        2 => self.render_invite_link(ui),      // Paste link
        _ => {}
    }
});
```

**Acceptance Criteria**:
- [ ] Three tabs visible: Search, QR Code, Link
- [ ] Search tab shows LAN peers (if any)
- [ ] QR tab can upload image or show own QR
- [ ] Link tab accepts paste and validates
- [ ] No manual fingerprint entry anywhere

---

## 📋 GitHub Preparation Checklist

### Files Created ✅
- [x] `README.md` at root (comprehensive, user-facing)
- [x] `SECURITY.md` at root (threat model, reporting)
- [x] `AUDIT_CHANGES.md` (this document)

### Files Updated ✅
- [x] `.gitignore` (added `Downloads/`, `temp/`)

### Files to Move (Next Step)
- [ ] `docs/Community/CONTRIBUTING.md` → `./CONTRIBUTING.md`
- [ ] `docs/Community/CODE_OF_CONDUCT.md` → `./CODE_OF_CONDUCT.md`
- [ ] `docs/Planning/CHANGELOG.md` → `./CHANGELOG.md`

### Files to Create
- [ ] `docs/user-guide/installation.md`
- [ ] `docs/user-guide/quick-start.md`
- [ ] `docs/user-guide/features.md`
- [ ] `docs/user-guide/troubleshooting.md`

### Security Audit
- [x] No secrets in `.gitignore` (validated)
- [x] No hardcoded API keys (none found)
- [x] No `.env` files committed (excluded)
- [x] History.json excluded (plaintext sensitive data)
- [x] Private keys excluded (*.pem, *.key, etc.)

### GitHub Actions (Future)
- [ ] CI/CD pipeline (`rust.yml`)
- [ ] Security audit workflow (`security.yml`)
- [ ] Auto-release on tag (`release.yml`)

---

## 🎯 Priority Todo List

### Sprint 1: GitHub Readiness (WEEK 1)
**Status**: ✅ 70% Complete

1. ✅ Create `README.md` (2h) - DONE
2. ✅ Create `SECURITY.md` (2h) - DONE
3. ✅ Update `.gitignore` (30min) - DONE
4. ⏳ Move documentation files (1h)
5. ⏳ Update internal links (1h)

**Deliverable**: Project ready for public GitHub repository

---

### Sprint 2: Contact UX Overhaul (WEEK 2-3)
**Priority**: 🔴 P0 (Critical UX Issue)
**Estimated Effort**: 2-3 days

**Tasks**:
1. Add `mdns`, `qrcode`, `rqrr` dependencies (30min)
2. Implement LAN discovery (`chat_manager.rs`, 2h)
3. Implement invite link generator (1h)
4. Implement QR code generator/parser (2h)
5. Create 3-tab contact UI (`main.rs`, 4h)
6. Testing and refinement (4h)

**Acceptance Criteria**:
- [ ] User never enters raw fingerprint/UUID
- [ ] LAN discovery works on same network
- [ ] QR code upload works
- [ ] Invite links work

**Deliverable**: Intuitive contact addition (no technical knowledge required)

---

### Sprint 3: Inline Rename (WEEK 3)
**Priority**: 🟡 P1 (High - Accessibility)
**Estimated Effort**: 4-6 hours

**Tasks**:
1. Apply "Patch 2: Inline Rename" (30min)
2. Add keyboard shortcuts (F2, Escape) (1h)
3. Test on desktop and mobile (2h)
4. Update documentation (1h)

**Acceptance Criteria**:
- [ ] One-click rename (no modal)
- [ ] Works with Enter/Escape keys
- [ ] Mobile-friendly

**Deliverable**: Accessible rename button

---

### Sprint 4: Group Chat Wizard (WEEK 4-5)
**Priority**: 🟡 P1 (High - Feature Completeness)
**Estimated Effort**: 2-3 days

**Tasks**:
1. Design wizard UI mockup (2h)
2. Implement 4-step wizard (`main.rs`, 6h)
3. Add member search by name (2h)
4. Add invite link generation (1h)
5. Testing and polish (4h)

**Acceptance Criteria**:
- [ ] Step-by-step group creation
- [ ] Search members by name
- [ ] Generate invite link
- [ ] Set default permissions

**Deliverable**: User-friendly group creation

---

### Sprint 5: Role-Based Moderation (WEEK 6-7)
**Priority**: 🟢 P2 (Medium - Advanced Feature)
**Estimated Effort**: 3-4 days

**Tasks**:
1. Add `GroupRole` enum (`types.rs`, 1h)
2. Update protocol messages (`protocol.rs`, 2h)
3. Implement permission checks (`chat_manager.rs`, 4h)
4. Add moderation UI (context menus, 6h)
5. Testing and edge cases (4h)

**Acceptance Criteria**:
- [ ] Owner/Admin/Moderator/Member roles
- [ ] Context menu with moderation actions
- [ ] Permission checks enforced
- [ ] Role badges visible

**Deliverable**: Complete group moderation system

---

## 📊 Version Planning

### v1.3.0 (Target: Q1 2026) - "Usability Release"
**Focus**: Fix critical UX issues

**Features**:
- ✅ README, SECURITY, .gitignore improvements
- 🔄 Contact search by name (not ID)
- 🔄 Inline rename
- 🔄 Group creation wizard
- 🔄 Persistent identities (Argon2 encryption)
- 🔄 Message delivery ACKs

**Breaking Changes**: None

---

### v2.0.0 (Target: Q2-Q3 2026) - "Professional Release"
**Focus**: Enterprise-ready features

**Features**:
- Role-based moderation
- NAT traversal (STUN/TURN)
- Message search
- Image previews
- Voice messages
- Disappearing messages

**Breaking Changes**: Protocol v3 (backward compatible)

---

### v3.0.0 (Target: Q4 2026+) - "Next Generation"
**Focus**: Advanced features

**Features**:
- Post-quantum cryptography
- Mobile apps (iOS/Android)
- Multi-device sync
- Voice/video calls
- Plugin system

**Breaking Changes**: Protocol v4

---

## 📞 Support & Questions

**For this audit**:
- Document author: Claude Code (AI Assistant)
- Review date: 2025-11-02
- Version: 1.0

**For project questions**:
- GitHub Issues: Technical questions
- GitHub Discussions: General questions
- Email: project@[domain].com

---

**Last Updated**: 2025-11-02
**Next Review**: After Sprint 2 completion
**Status**: 🟢 Active Development

---

## ✅ Quick Start Checklist (for Developers)

**To begin implementation**:
1. [ ] Read this document fully
2. [ ] Review [SECURITY.md](SECURITY.md) for crypto constraints
3. [ ] Read [docs/Technical/CLAUDE.md](docs/Technical/CLAUDE.md) for architecture
4. [ ] Check [docs/Planning/DEVELOPMENT_PLAN.md](docs/Planning/DEVELOPMENT_PLAN.md) for roadmap
5. [ ] Start with Sprint 1 tasks (documentation)
6. [ ] Move to Sprint 2 (contact UX) - highest user impact

**Priority order**: Sprint 1 → Sprint 2 → Sprint 3 → Sprint 4 → Sprint 5

---

**End of Audit Document**
