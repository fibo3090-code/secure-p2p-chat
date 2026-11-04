//! Per-member budget for writes: posts, DMs, uploads, shares and governance
//! actions.
//!
//! Only channel creation used to be limited. Any member could post 64 KiB
//! messages and DMs as fast as their link allowed, each one stored durably,
//! committed under the global state lock and fanned out to every connection —
//! enough for a single member to slow the whole community to a crawl and grow
//! the database without bound. This is a token bucket per member, with two
//! budgets: one for the number of writes, one for the bytes of text they carry.
//!
//! In memory only, like the channel-creation limiter: it is a burst limiter,
//! not a quota, and a restart forgiving it costs nothing.

use std::collections::HashMap;
use std::time::Instant;

use uuid::Uuid;

/// The shape of a member's write budget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WriteRateLimit {
    /// Writes a member may make in a burst.
    pub burst: f64,
    /// Writes per second the budget refills by.
    pub per_second: f64,
    /// Bytes of message text a member may send in a burst.
    pub byte_burst: f64,
    /// Bytes of message text per second the budget refills by.
    pub bytes_per_second: f64,
}

/// Generous for people, tight for scripts: a burst of 30 writes and 3 a
/// second sustained; 1 MiB of text in a burst (16 maximum-size messages) and
/// 128 KiB a second sustained. File bytes are not counted here — storage
/// quotas already bound those.
pub const DEFAULT_WRITE_RATE: WriteRateLimit = WriteRateLimit {
    burst: 30.0,
    per_second: 3.0,
    byte_burst: 1024.0 * 1024.0,
    bytes_per_second: 128.0 * 1024.0,
};

/// What a refused write is told.
pub const WRITE_RATE_REFUSAL: &str = "you are sending too fast — wait a moment and try again";

#[derive(Debug)]
struct Bucket {
    writes: f64,
    bytes: f64,
    at: Instant,
}

/// Every member's bucket. Bounded by the membership, which is itself capped.
#[derive(Debug)]
pub(super) struct WriteBuckets {
    limit: Option<WriteRateLimit>,
    buckets: HashMap<Uuid, Bucket>,
}

impl Default for WriteBuckets {
    fn default() -> Self {
        Self {
            limit: Some(DEFAULT_WRITE_RATE),
            buckets: HashMap::new(),
        }
    }
}

impl WriteBuckets {
    pub(super) fn set_limit(&mut self, limit: Option<WriteRateLimit>) {
        self.limit = limit;
        self.buckets.clear();
    }

    /// Spend one write carrying `text_bytes` from `member`'s budget, or refuse
    /// without spending anything.
    pub(super) fn check(
        &mut self,
        member: Uuid,
        text_bytes: usize,
        now: Instant,
    ) -> Result<(), String> {
        let Some(limit) = self.limit else {
            return Ok(());
        };
        let bucket = self.buckets.entry(member).or_insert(Bucket {
            writes: limit.burst,
            bytes: limit.byte_burst,
            at: now,
        });
        let elapsed = now.saturating_duration_since(bucket.at).as_secs_f64();
        bucket.writes = (bucket.writes + elapsed * limit.per_second).min(limit.burst);
        bucket.bytes = (bucket.bytes + elapsed * limit.bytes_per_second).min(limit.byte_burst);
        bucket.at = now;
        // A cost above the whole burst could never be paid; charge the burst.
        let cost = (text_bytes as f64).min(limit.byte_burst);
        if bucket.writes < 1.0 || bucket.bytes < cost {
            return Err(WRITE_RATE_REFUSAL.to_string());
        }
        bucket.writes -= 1.0;
        bucket.bytes -= cost;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_burst_is_allowed_then_refused_then_refilled() {
        let mut b = WriteBuckets::default();
        let m = Uuid::new_v4();
        let t0 = Instant::now();
        for _ in 0..DEFAULT_WRITE_RATE.burst as usize {
            b.check(m, 10, t0).unwrap();
        }
        assert!(b.check(m, 10, t0).is_err(), "the burst is spent");
        // Another member is unaffected.
        b.check(Uuid::new_v4(), 10, t0).unwrap();
        // One second refills `per_second` writes.
        let t1 = t0 + Duration::from_secs(1);
        for _ in 0..DEFAULT_WRITE_RATE.per_second as usize {
            b.check(m, 10, t1).unwrap();
        }
        assert!(b.check(m, 10, t1).is_err());
    }

    #[test]
    fn text_bytes_have_their_own_budget() {
        let mut b = WriteBuckets::default();
        let m = Uuid::new_v4();
        let t0 = Instant::now();
        let big = 64 * 1024;
        let fits = (DEFAULT_WRITE_RATE.byte_burst as usize) / big;
        for _ in 0..fits {
            b.check(m, big, t0).unwrap();
        }
        assert!(b.check(m, big, t0).is_err(), "the byte burst is spent");
        // A write carrying no text costs no bytes, so it is still allowed on
        // the remaining write budget.
        b.check(m, 0, t0).unwrap();
    }

    #[test]
    fn no_limit_means_no_limit() {
        let mut b = WriteBuckets::default();
        b.set_limit(None);
        let m = Uuid::new_v4();
        let t0 = Instant::now();
        for _ in 0..10_000 {
            b.check(m, 64 * 1024, t0).unwrap();
        }
    }
}
