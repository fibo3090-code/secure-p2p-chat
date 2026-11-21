//! One thread's message history — a channel's or a DM thread's — with only its
//! newest envelopes held in memory. SQLite holds all of them and answers older
//! requests (`PartyState::history_page`), so server memory and startup time do
//! not grow with history. A database-less state keeps everything.

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
    /// The highest seq assigned in this thread, resident or not; the next
    /// envelope gets `last_seq + 1`.
    last_seq: u64,
    /// Every envelope with `seq >= complete_from` is resident (1 until the
    /// first eviction).
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

    /// Resident envelopes strictly after `since_seq`, ascending (binary search).
    pub(super) fn after(&self, since_seq: u64) -> impl Iterator<Item = &Envelope> {
        let start = self.resident.partition_point(|e| e.seq <= since_seq);
        self.resident.range(start..)
    }

    /// Drop resident envelopes older than `cutoff_ms` (retention), always
    /// keeping the newest one so numbering survives a restart. Returns how many
    /// went. `complete_from` still holds: retention deletes the same rows from
    /// the database.
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

    /// Envelopes currently held in memory (tests).
    #[cfg(test)]
    pub(super) fn resident_len(&self) -> usize {
        self.resident.len()
    }
}
