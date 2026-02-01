# Documentation TODO

This file tracks planned improvements for the project's documentation.

- [ ] **Consolidate Roadmaps:** Merge `ROADMAP.md` and `SECURITY_ROADMAP.md` into a single, unified roadmap to create a single source of truth for the project's direction.

- [ ] **Add Glossary:** Create a `docs/GLOSSARY.md` file to define technical and cryptographic terms used throughout the documentation (e.g., "forward secrecy," "AEAD," "TOFU").

- [ ] **Add Diagrams:**
    - [ ] Create a sequence diagram for the handshake protocol in `docs/04_protocol.md`.
    - [ ] Create a diagram illustrating the file transfer process (chunking, sending, receiving) in `DEVELOPER_GUIDE.md`.
    - [ ] Create a diagram for the re-keying mechanism in `docs/04_protocol.md`.

- [ ] **Synchronize Version Numbers:** Create a script or a pre-commit hook to automatically synchronize the version number across all documentation files (`README.md`, `SECURITY.md`, `THREAT_MODEL.md`, etc.) during the release process.

- [ ] **Add Summaries to Long Documents:**
    - [ ] Add a "tl;dr" or "Executive Summary" section at the beginning of `THREAT_MODEL.md`.
    - [ ] Add a "tl;dr" or "Security Overview" section at the beginning of `SECURITY.md`.

- [ ] **Consolidate `README.md` and `docs/GETTING_STARTED.md`:** Streamline the `README.md` to be a more concise entry point and direct users to the `docs/GETTING_STARTED.md` for more detailed instructions.
