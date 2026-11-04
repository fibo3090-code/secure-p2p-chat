//! One thread's message history — a channel's or a DM thread's — with only its
//! newest envelopes held in memory.
//!
//! Every message ever posted used to live in a `Vec` for the life of the
//! process, and every one was deserialized at startup. Server memory therefore
//! grew with the community's entire history, restarts got slower with it, and
//! one member posting maximum-size messages could walk the server toward an
//! OOM kill. The database already holds every envelope, so memory only needs
//! the tail that live catch-up actually asks for; a request for anything older
//! is answered from SQLite (see `PartyState::history_page`).
//!
//! A server without a database (`PartyState::new`, used by tests and
//! in-process callers) has nowhere to page from, so it keeps everything.

use std::collections::VecDeque;

use messenger_core::party::Envelope;

/// How many of a thread's newest envelopes stay in memory on a disk-backed
/// server. A little over one full page, so the common request — "what did I
/// miss since I was last here?" — never touches the disk.
pub(super) const RESIDENT_HISTORY: usize = 256;

/// A thread's history: the newest envelopes, ascending by seq, plus the
/// highest sequence ever assigned.
#[derive(Debug, Default)]
pub(super) struct History {
    resident: VecDeque<Envelope>,
    /// The highest seq assigned in this thread, resident or not. The next
    /// envelope gets `last_seq + 1`; this is what keeps numbering stable once
    /// older envelopes are no longer in memory to be counted.
    last_seq: u64,
    /// Every envelope with `seq >= complete_from` is resident. 1 until the
    /// first eviction, so a thread that never outgrew the cap is served
    /// entirely from memory.
    complete_from: u64,
}

impl History {
    pub(super) fn new() -> Self {
        Self {
            resident: VecDeque::new(),
            last_seq: 0,
            complete_from: 1,
        }
    }

    /// Rebuild a thread from the newest `tail` envelopes in the database
    /// (ascending) and the highest seq stored. `complete` is whether `tail`
    /// is the whole thread.
    pub(super) fn from_tail(tail: Vec<Envelope>, last_seq: u64, complete: bool) -> Self {
        let complete_from = if complete {
            1
        } else {
            tail.first()
                .map(|e| e.seq)
                .unwrap_or(last_seq.saturating_add(1))
        };
        let last_seq = last_seq.max(tail.last().map(|e| e.seq).unwrap_or(0));
        Self {
            resident: tail.into(),
            last_seq,
            complete_from,
        }
    }

    /// The sequence number the next envelope in this thread gets.
    pub(super) fn next_seq(&self) -> u64 {
        self.last_seq + 1
    }

    /// Append `env` (which must carry [`Self::next_seq`]), evicting the
    /// oldest resident envelopes beyond `cap`. `None` keeps everything.
    pub(super) fn push(&mut self, env: Envelope, cap: Option<usize>) {
        debug_assert_eq!(env.seq, self.next_seq());
        self.last_seq = env.seq;
        self.resident.push_back(env);
        if let Some(cap) = cap {
            while self.resident.len() > cap {
                if let Some(evicted) = self.resident.pop_front() {
                    self.complete_from = evicted.seq + 1;
                }
            }
        }
    }

    /// Whether everything after `since_seq` is resident, so a page can be
    /// served from memory.
    pub(super) fn covers(&self, since_seq: u64) -> bool {
        since_seq.saturating_add(1) >= self.complete_from
    }

    /// Resident envelopes strictly after `since_seq`, ascending.
    ///
    /// A binary search, not a scan from the first message: `history_since`
    /// used to walk the whole channel to find its starting point, so catching
    /// up on a long channel cost O(n) per page and O(n²) overall.
    pub(super) fn after(&self, since_seq: u64) -> impl Iterator<Item = &Envelope> {
        let start = self.resident.partition_point(|e| e.seq <= since_seq);
        self.resident.range(start..)
    }

    /// Drop resident envelopes older than `cutoff_ms` (retention), always
    /// keeping the newest one: it carries `last_seq`, and a thread whose every
    /// envelope vanished would number its next message from 1 again on the
    /// next restart — under seqs clients already hold. Returns how many went.
    ///
    /// `complete_from` needs no change: what was popped no longer exists
    /// anywhere (retention deletes the same rows from the database), so
    /// "everything from `complete_from` up is resident" still holds.
    pub(super) fn prune_before(&mut self, cutoff_ms: u64) -> usize {
        let mut pruned = 0;
        while self.resident.len() > 1
            && self
                .resident
                .front()
                .is_some_and(|e| e.timestamp < cutoff_ms)
        {
            self.resident.pop_front();
            pruned += 1;
        }
        pruned
    }

    /// Envelopes currently held in memory (diagnostics and tests).
    pub(super) fn resident_len(&self) -> usize {
        self.resident.len()
    }
}
