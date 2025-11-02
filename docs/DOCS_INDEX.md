# 📚 Documentation Index

**Project**: Encrypted P2P Messenger  
**Version**: 1.2.0  
**Last Updated**: 2025-10-31

---

## 🚀 Quick Start

**New users start here**:
1. **[README.md](README.md)** - Installation, usage guide, and quick start
2. **[CHANGELOG.md](CHANGELOG.md)** - Latest changes and version history

---

---

## ✨ New Features (v1.2.1)

- Contacts: Manage known peers with display names and fingerprints. Contacts are persisted in history files and can be added/removed from the UI.
- Group chats: Create group conversations by selecting multiple contacts. Group chats maintain a participants list and a group title.
- Rename conversations: Rename any chat or group to a friendly title; the name is saved in chat history and persisted across runs.

These features improve usability for multi-peer scenarios and make organizing conversations easier.

---

## 📖 Documentation Structure

### For Users

| Document | Purpose | When to Read |
|----------|---------|--------------|
| **[README.md](README.md)** | Complete user guide | First time setup |
| **[CHANGELOG.md](CHANGELOG.md)** | Version history & changes | When upgrading |
| **[HISTORY.md](HISTORY.md)** | Past bug fixes (v0.9-v1.0.2) | Troubleshooting old issues |

### For Developers

| Document | Purpose | When to Read |
|----------|---------|--------------|
| **[CLAUDE.md](CLAUDE.md)** | Architecture deep-dive | Understanding codebase |
| **[IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md)** | Component status | Checking what's implemented |
| **[FORWARD_SECRECY.md](FORWARD_SECRECY.md)** | v1.1.0 security details | Understanding crypto |
| **[DEVELOPMENT_PLAN.md](DEVELOPMENT_PLAN.md)** | Roadmap & future plans | Contributing features |
| **[PROTOCOL_SPEC.md](PROTOCOL_SPEC.md)** | Protocol specification (FR) | Protocol implementation |

### For Contributors

| Document | Purpose | When to Read |
|----------|---------|--------------|
| **[CONTRIBUTING.md](CONTRIBUTING.md)** | Contribution guidelines | Before submitting PR |
| **[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)** | Community standards | Always |

---

## 📋 Document Descriptions

### README.md
**Complete User Guide**
- Installation instructions
- Quick start for host/client modes
- Feature overview with screenshots
- Security model explanation
- Keyboard shortcuts
- Troubleshooting common issues
- What's new in v1.1.0

**Key Sections**:
- ✨ Features
- 🚀 Quick Start
- 🔐 Security - CRITICAL!
- ⚙️ Configuration
- 🐛 Troubleshooting
- 🎨 What's New

### CHANGELOG.md
**Version History**
- v1.2.0 (2025-10-31) - Enhanced UX Release (emoji, drag-drop, typing, notifications)
- v1.1.0 (2025-10-31) - Forward secrecy implementation
- v1.0.2 (2025-10-23) - Message receiving bug fix
- v1.0.0 (2025-10-23) - Major UI/UX overhaul
- v0.9.0 - Initial implementation

**Includes**:
- Breaking changes
- New features
- Bug fixes
- Security improvements
- Migration guides

### FORWARD_SECRECY.md
**Technical Documentation for v1.1.0**
- What is forward secrecy?
- X25519 ECDH implementation
- HKDF-SHA256 key derivation
- Protocol v2 specification
- Security analysis
- Performance benchmarks
- Testing guide
- FAQ section

**400+ lines** of comprehensive technical details

### DEVELOPMENT_PLAN.md
**Roadmap for Future Features**
- ✅ Phase 1: Security enhancements (v1.1.0 - Forward Secrecy DONE)
- Phase 2: Connection reliability (heartbeat, acknowledgments)
- ✅ Phase 3: User experience (v1.2.0 - ALL DONE)
- ✅ Phase 4.1: Emoji picker (v1.2.0 - DONE)
- Phase 4.2+: Message search, image previews
- Phase 5: Testing & quality
- Phase 6: Advanced features

**Includes**:
- Technical specifications
- Estimated timelines
- Priority matrix
- Success metrics
- v1.2.0 achievement summary

### HISTORY.md
**Past Development History (v0.9 - v1.0.2)**
- v1.0.2 message receiving bug fix
- v1.0.0 UI visibility fixes
- v0.9.0 initial features
- Common issues & solutions
- Testing guides
- Performance notes

**Consolidates**:
- Old bug fix documentation
- Feature evolution
- Historical context

### CLAUDE.md
**Architecture Deep-Dive**
- System design
- Code organization
- Module responsibilities
- Design patterns
- Best practices
- Development workflows

**For developers** understanding the codebase

### IMPLEMENTATION_STATUS.md
**Component Status Report**
- ✅ Completed components
- Build status
- Test status
- Security features
- Performance metrics
- Dependencies
- Known limitations

**Updated for v1.2.0** with:
- Forward secrecy (v1.1.0)
- Emoji picker, drag-drop, typing indicators, notifications (v1.2.0)

### PROTOCOL_SPEC.md
**Protocol Specification (French)**
- Complete protocol specification
- Wire format details
- Handshake sequences
- Message types
- Security model
- Original design document

**77,000+ bytes** of protocol documentation

### CONTRIBUTING.md
**Contribution Guidelines**
- How to contribute
- Code style
- Pull request process
- Testing requirements
- Documentation standards

### CODE_OF_CONDUCT.md
**Community Standards**
- Expected behavior
- Unacceptable behavior
- Enforcement
- Contact information

---

## 🗂️ Documentation Organization

```
Project Root/
├── README.md ..................... START HERE (User Guide)
├── CHANGELOG.md .................. Version History
├── DOCS_INDEX.md ................. This file
│
├── Technical/
│   ├── FORWARD_SECRECY.md ........ v1.1.0 Security Details
│   ├── CLAUDE.md ................. Architecture Deep-Dive
│   ├── IMPLEMENTATION_STATUS.md .. Component Status
│   └── PROTOCOL_SPEC.md ...... Protocol Spec (FR)
│
├── Planning/
│   ├── DEVELOPMENT_PLAN.md ....... Roadmap
│   └── HISTORY.md ................ Past Changes (v0.9-v1.0.2)
│
└── Community/
    ├── CONTRIBUTING.md ........... Contribution Guide
    └── CODE_OF_CONDUCT.md ........ Community Rules
```

---

## 🎯 Quick Reference by Task

### "I want to install and use the app"
→ **[README.md](README.md)** - Complete user guide

### "I want to understand the latest security features"
→ **[FORWARD_SECRECY.md](FORWARD_SECRECY.md)** - v1.1.0 technical details

### "I want to contribute code"
→ **[CONTRIBUTING.md](CONTRIBUTING.md)** + **[CLAUDE.md](CLAUDE.md)**

### "I want to know what changed"
→ **[CHANGELOG.md](CHANGELOG.md)** - Version history

### "I want to understand the architecture"
→ **[CLAUDE.md](CLAUDE.md)** - Architecture deep-dive

### "I have a bug or issue"
→ **[README.md#Troubleshooting](README.md)** + **[HISTORY.md](HISTORY.md)**

### "I want to see future plans"
→ **[DEVELOPMENT_PLAN.md](DEVELOPMENT_PLAN.md)** - Roadmap

### "I want to implement the protocol"
→ **[PROTOCOL_SPEC.md](PROTOCOL_SPEC.md)** - Protocol spec

---

## 📊 Documentation Statistics

| Metric | Count |
|--------|-------|
| **Total Documents** | 10 MD files |
| **User Guides** | 3 (README, CHANGELOG, HISTORY) |
| **Technical Docs** | 4 (FORWARD_SECRECY, CLAUDE, STATUS, protocol) |
| **Planning Docs** | 1 (DEVELOPMENT_PLAN) |
| **Community Docs** | 2 (CONTRIBUTING, CODE_OF_CONDUCT) |
| **Total Lines** | ~3000+ lines |
| **Coverage** | Installation → Architecture → Future |

---

## 🔄 Documentation Lifecycle

### When Documents Are Updated

| Document | Update Frequency | Trigger |
|----------|------------------|---------|
| README.md | Major versions | New features, breaking changes |
| CHANGELOG.md | Every release | Any version change |
| FORWARD_SECRECY.md | Stable | Major security changes only |
| DEVELOPMENT_PLAN.md | Quarterly | New feature planning |
| IMPLEMENTATION_STATUS.md | Major versions | Component status changes |
| HISTORY.md | Stable | Historical reference only |
| CLAUDE.md | As needed | Architecture changes |
| CONTRIBUTING.md | Rare | Process changes |
| CODE_OF_CONDUCT.md | Rare | Policy updates |
| PROTOCOL_SPEC.md | Stable | Protocol changes only |

---

## ✅ Documentation Quality Checklist

- [x] All documents have clear purpose
- [x] No duplicate information
- [x] Clear cross-references
- [x] Updated for v1.1.0
- [x] Consistent formatting
- [x] No broken links
- [x] Searchable content
- [x] Version information included

---

## 🌐 External Resources

### Related Links
- **Rust Documentation**: https://doc.rust-lang.org/
- **RustCrypto**: https://github.com/RustCrypto
- **egui Framework**: https://github.com/emilk/egui
- **X25519 Spec**: RFC 7748
- **HKDF Spec**: RFC 5869

### Similar Projects
- **Signal Protocol**: https://signal.org/docs/
- **WhatsApp Security**: https://www.whatsapp.com/security/
- **TLS 1.3**: RFC 8446

---

## 📝 Notes

### Recent Changes
**v1.2.0** (2025-10-31):
- ✅ Updated CHANGELOG.md with comprehensive v1.2.0 release notes
- ✅ Enhanced HISTORY.md with v1.1.0 cleanup details
- ✅ Renamed `project overvieuw.md` → `PROTOCOL_SPEC.md` (fixed typo)
- ✅ Updated all documentation references

**v1.1.0** (2025-10-31):
- ✅ Created HISTORY.md (consolidates old bug fixes)
- ✅ Created FORWARD_SECRECY.md (v1.1.0 technical)
- ✅ Created DOCS_INDEX.md (navigation guide)
- ✅ Updated README.md (v1.1.0 features)
- ✅ Updated CHANGELOG.md (v1.1.0 entry)
- ✅ Updated IMPLEMENTATION_STATUS.md (v1.1.0 status)

### File Consolidation History
**v1.1.0 Cleanup**: 5 redundant files merged into HISTORY.md and FORWARD_SECRECY.md
- BUGFIX_MESSAGES.md → HISTORY.md
- FIX_SUMMARY.md → HISTORY.md
- NEW_FEATURES.md → HISTORY.md
- TEST_MESSAGING.md → HISTORY.md
- IMPLEMENTATION_SUMMARY.md → FORWARD_SECRECY.md

**v1.2.0 Cleanup**: 3 redundant files merged into CHANGELOG.md and HISTORY.md
- CLEANUP_SUMMARY.md → HISTORY.md (v1.1.0 cleanup section)
- COMPLETION_SUMMARY.md → CHANGELOG.md (v1.2.0 detailed stats)
- RELEASE_NOTES_v1.2.0.md → CHANGELOG.md (v1.2.0 release notes)

**Total**: 8 redundant files consolidated with **zero data loss** - all information preserved

---

## 🆘 Help

**Can't find what you're looking for?**

1. Search this index for keywords
2. Check README.md first (covers 80% of user needs)
3. Check CHANGELOG.md for version-specific info
4. Check HISTORY.md for historical issues
5. Open an issue on GitHub

**For technical details**: CLAUDE.md or FORWARD_SECRECY.md  
**For contributing**: CONTRIBUTING.md  
**For roadmap**: DEVELOPMENT_PLAN.md

---

**Last Updated**: 2025-10-31  
**Maintained By**: Project Team  
**Status**: ✅ Up to date with v1.2.0
