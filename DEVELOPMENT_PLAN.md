# Development Plan

This file is the active roadmap and backlog for the project. It is not a changelog and it is not a design specification. Completed work belongs in [CHANGELOG.md](CHANGELOG.md); detailed design belongs in the dedicated docs.

## Current Priorities

### 1. Productization

- Reduce UI-thread blocking and other responsiveness issues
- Tighten persistence, migration, and recovery behavior
- Improve diagnostics and failure visibility for users

### 2. Security and Privacy

- Maintain accurate security documentation as implementation changes
- Keep dependency risk under review
- Continue hardening mDNS behavior and discovery privacy
- Prepare future work for stronger invite lifecycle controls and better identity handling

### 3. Connectivity

- Add a practical story for internet connectivity
- Evaluate NAT traversal options
- Evaluate relay-assisted or overlay-assisted connection modes

## Backlog

### High priority

- NAT traversal / internet-grade connectivity
- File transfer progress and cancellation improvements in the GUI
- Better diagnostic export for support and bug reporting
- Stronger mDNS registration/removal behavior

### Medium priority

- Accessibility pass over GUI interactions and color usage
- Settings IA cleanup and tabbed organization
- Better contact management UX and trust-state workflows
- More protocol diagrams in docs
- Better crash recovery and persistence repair guidance

### Low priority

- Clipboard hygiene for copied sensitive values
- More polished onboarding and learning materials
- Additional automation around version/doc synchronization

## Known Large-Feature Gaps

These are intentionally tracked as roadmap items rather than described as “done” anywhere else:

- relay support
- onion routing / anonymity layer
- post-quantum migration
- hardware-backed identity support
- invite expiration and revocation

## Maintenance Rules

- Do not move incomplete work into the changelog.
- Do not describe roadmap items as shipped features in README, SECURITY, or protocol docs.
- When a roadmap item becomes real behavior, update the canonical doc in the same PR.
