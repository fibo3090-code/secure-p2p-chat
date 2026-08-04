//! Per-IP connection rate limiting for accept loops.
//!
//! Used by both listeners that face the open internet:
//!
//! - the **community server**, where `dispatch::MAX_JOIN_ATTEMPTS` caps password
//!   guesses *within* one connection and so pushes an attacker into reconnecting
//!   between guesses. This is the other half: it caps how often they may
//!   reconnect. Together they put a hard ceiling on guesses per minute per
//!   address, on top of the RSA handshake each attempt already pays for.
//! - the **relay server**, where every accepted socket parks a task holding a
//!   rendezvous slot until it times out, so an unlimited connection rate is an
//!   unlimited memory and task cost.
//!
//! It also blunts plain connection floods — an accepted socket spawns a task
//! that immediately starts work that is not cheap.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Connections one address may open within [`WINDOW`], for the community server.
pub const MAX_CONNECTIONS: usize = 10;
/// The sliding window those connections are counted over.
pub const WINDOW: Duration = Duration::from_secs(30);

/// Sliding-window counter of recent connections per source address.
#[derive(Debug)]
pub struct RateLimiter {
    attempts: HashMap<IpAddr, Vec<Instant>>,
    max: usize,
    window: Duration,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::with_limits(MAX_CONNECTIONS, WINDOW)
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// A limiter with a caller-chosen cap and window. The relay and the party
    /// server have different connection profiles, so they pick their own.
    pub fn with_limits(max: usize, window: Duration) -> Self {
        Self {
            attempts: HashMap::new(),
            max,
            window,
        }
    }

    /// Record a connection from `ip` and report whether it should be refused.
    ///
    /// `now` is passed in so the behaviour is testable without sleeping.
    pub fn check_at(&mut self, ip: IpAddr, now: Instant) -> bool {
        // Drop addresses whose whole history has aged out, so a long-running
        // server does not accumulate an entry per address it has ever seen.
        let window = self.window;
        self.attempts
            .retain(|_, seen| seen.iter().any(|t| now.duration_since(*t) < window));

        let max = self.max;
        let seen = self.attempts.entry(ip).or_default();
        seen.retain(|t| now.duration_since(*t) < window);
        if seen.len() >= max {
            return true;
        }
        seen.push(now);
        false
    }

    /// [`Self::check_at`] against the current clock.
    pub fn check(&mut self, ip: IpAddr) -> bool {
        self.check_at(ip, Instant::now())
    }

    /// Addresses currently being tracked. Exposed for tests and diagnostics.
    pub fn tracked_addresses(&self) -> usize {
        self.attempts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn allows_up_to_the_cap_then_refuses() {
        let mut rl = RateLimiter::new();
        let now = Instant::now();
        for i in 0..MAX_CONNECTIONS {
            assert!(!rl.check_at(ip(1), now), "connection {i} must be allowed");
        }
        assert!(
            rl.check_at(ip(1), now),
            "the connection past the cap must be refused"
        );
    }

    #[test]
    fn the_window_slides() {
        let mut rl = RateLimiter::new();
        let now = Instant::now();
        for _ in 0..MAX_CONNECTIONS {
            rl.check_at(ip(1), now);
        }
        assert!(rl.check_at(ip(1), now));
        // Once the window has passed, the same address is welcome again —
        // this is a rate limit, not a ban.
        let later = now + WINDOW + Duration::from_secs(1);
        assert!(!rl.check_at(ip(1), later));
    }

    #[test]
    fn addresses_are_limited_independently() {
        let mut rl = RateLimiter::new();
        let now = Instant::now();
        for _ in 0..MAX_CONNECTIONS {
            rl.check_at(ip(1), now);
        }
        assert!(rl.check_at(ip(1), now), "the flooding address is limited");
        assert!(
            !rl.check_at(ip(2), now),
            "an unrelated address must not be caught in someone else's limit"
        );
    }

    /// A server that ran for a month must not hold an entry per address it has
    /// ever seen.
    #[test]
    fn stale_addresses_are_forgotten() {
        let mut rl = RateLimiter::new();
        let now = Instant::now();
        rl.check_at(ip(1), now);
        rl.check_at(ip(2), now);
        assert_eq!(rl.tracked_addresses(), 2);

        let later = now + WINDOW + Duration::from_secs(1);
        rl.check_at(ip(3), later);
        assert_eq!(
            rl.tracked_addresses(),
            1,
            "addresses with no connections in the window are dropped"
        );
    }

    #[test]
    fn custom_limits_are_honoured() {
        let mut rl = RateLimiter::with_limits(2, Duration::from_secs(5));
        let now = Instant::now();
        assert!(!rl.check_at(ip(1), now));
        assert!(!rl.check_at(ip(1), now));
        assert!(rl.check_at(ip(1), now), "third connection is over the cap");
    }
}
