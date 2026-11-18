//! The client's integration suite, as **one** test binary.
//!
//! Every file directly under `tests/` is its own crate, linked against the
//! whole dependency tree — this directory used to hold 21 of them, which meant
//! 21 links of tokio, ratatui, rusqlite and the server on every change, and a
//! harness copied into each one because test crates cannot share a module
//! without each compiling it again. One binary links once and shares
//! `common`.
//!
//! Isolation is not lost: CI runs `cargo nextest`, which starts a process per
//! test regardless of how tests are grouped into binaries.

mod common;

mod contacts;
mod file_transfer;
mod manager;
mod party;
mod persistence;
mod probes;
mod reconnect;
mod tui;
