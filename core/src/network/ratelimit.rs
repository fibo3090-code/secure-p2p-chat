//! Per-IP connection rate limiting for the accept loops that face the internet:
//!
//! - the **community server**, where `dispatch::MAX_JOIN_ATTEMPTS` caps password
//!   guesses per connection and this caps reconnects, together bounding guesses per
//!   minute per address;
//! - the **relay server**, where every accepted socket parks a task holding a
//!   rendezvous slot until it times out.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Connections one address may open within [`WINDOW`], for the community server.
pub const MAX_CONNECTIONS: usize = 10;
/// The sliding window those connections are counted over.
pub const WINDOW: Duration = Duration::from_secs(30);

/// How many source addresses one limiter tracks at once.
///
/// Ageing alone does not bound the table against a scan from fresh addresses (trivial
/// from an IPv6 /64), so it is capped too. At the cap the addresses seen least recently
/// are evicted first: whoever is connecting hardest has the freshest entry and is the
/// last to be forgotten. Generous next to any real deployment, under a megabyte.
pub const MAX_TRACKED_ADDRESSES: usize = 4096;

/// One address's recent history. `last_seen` advances on every check, refused ones
/// included, while `attempts` records only allowed connections; eviction orders on
/// `last_seen` so an address that keeps knocking cannot age itself out.
#[derive(Debug)]
struct Entry {
    attempts: Vec<Instant>,
    last_seen: Instant,
}

/// The key a source address is counted under: IPv4 per address, **IPv6 per /64** (the
/// smallest block routinely delegated, so per-address counting is bypassed by
/// incrementing a number; not wider, or one ISP's customers would share a bucket).
///
/// Public so the community server's per-address concurrency cap groups addresses
/// exactly the way this rate limit does.
pub fn limiter_key(ip: IpAddr) -> IpAddr {
    match canonical_ip(ip) {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => {
            let mut octets = v6.octets();
            // Keep the routing prefix, zero the interface identifier.
            octets[8..].fill(0);
            IpAddr::from(octets)
        }
    }
}

/// The well-known NAT64 prefix (RFC 6052 §2.1), `64:ff9b::/96`.
const NAT64_WELL_KNOWN_PREFIX: [u8; 12] = [0x00, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0];

/// Reduce an address to the one that identifies its source. IPv4-mapped
/// (`::ffff:a.b.c.d`) and NAT64 addresses carry the IPv4 address in octets 8..16, which
/// a /64 mask would wipe, putting every IPv4 client into one bucket.
fn canonical_ip(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(v6) = ip else {
        return ip;
    };
    if let Some(v4) = v6.to_ipv4_mapped() {
        return IpAddr::V4(v4);
    }
    let octets = v6.octets();
    if octets[..12] == NAT64_WELL_KNOWN_PREFIX {
        return IpAddr::V4(std::net::Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    }
    IpAddr::V6(v6)
}

/// Sliding-window counter of recent connections per source address.
#[derive(Debug)]
pub struct RateLimiter {
    attempts: HashMap<IpAddr, Entry>,
    max: usize,
    window: Duration,
    max_tracked: usize,
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

    /// A limiter with a caller-chosen cap and window.
    pub fn with_limits(max: usize, window: Duration) -> Self {
        Self {
            attempts: HashMap::new(),
            max,
            window,
            max_tracked: MAX_TRACKED_ADDRESSES,
        }
    }

    /// Override how many addresses may be tracked at once (so eviction is testable).
    pub fn with_max_tracked(mut self, max_tracked: usize) -> Self {
        self.max_tracked = max_tracked.max(1);
        self
    }

    /// Record a connection from `ip` at `now` and report whether it should be refused.
    pub fn check_at(&mut self, ip: IpAddr, now: Instant) -> bool {
        let ip = limiter_key(ip);
        // Drop addresses whose whole history has aged out.
        let window = self.window;
        self.attempts.retain(|_, e| {
            e.attempts.iter().any(|t| now.duration_since(*t) < window)
                || now.duration_since(e.last_seen) < window
        });

        // Only a new address can grow the table, so enforce the ceiling here.
        if !self.attempts.contains_key(&ip) && self.attempts.len() >= self.max_tracked {
            self.evict_stalest();
        }

        let max = self.max;
        let entry = self.attempts.entry(ip).or_insert_with(|| Entry {
            attempts: Vec::new(),
            last_seen: now,
        });
        // Every knock counts as contact, answered or not.
        entry.last_seen = entry.last_seen.max(now);
        entry.attempts.retain(|t| now.duration_since(*t) < window);
        if entry.attempts.len() >= max {
            return true;
        }
        entry.attempts.push(now);
        false
    }

    /// [`Self::check_at`] against the current clock.
    pub fn check(&mut self, ip: IpAddr) -> bool {
        self.check_at(ip, Instant::now())
    }

    /// Drop the addresses seen least recently until there is room for one more. A batch
    /// is taken so a sustained scan pays for the sort once per `max_tracked / 16` new
    /// addresses rather than on every one.
    fn evict_stalest(&mut self) {
        let target = self.max_tracked.saturating_sub(1);
        let batch = (self.max_tracked / 16).max(1);
        let want_removed = self.attempts.len().saturating_sub(target).max(batch);

        let mut by_recency: Vec<(IpAddr, Instant)> = self
            .attempts
            .iter()
            .map(|(ip, entry)| (*ip, entry.last_seen))
            .collect();
        by_recency.sort_unstable_by_key(|(_, last)| *last);

        let removed = by_recency.len().min(want_removed);
        for (ip, _) in by_recency.into_iter().take(removed) {
            self.attempts.remove(&ip);
        }
        tracing::debug!(
            evicted = removed,
            tracked = self.attempts.len(),
            "rate limiter table hit its address cap; forgot the stalest entries"
        );
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

    fn ip6(last: u16) -> IpAddr {
        IpAddr::from([0x2001, 0xdb8, 0, 0, 0, 0, 0, last])
    }

    fn parse(s: &str) -> IpAddr {
        s.parse().expect("valid address")
    }

    /// Spend `a`'s whole budget on a default limiter; returns it with `a` refused.
    fn flooded(a: IpAddr, now: Instant) -> RateLimiter {
        let mut rl = RateLimiter::new();
        for i in 0..MAX_CONNECTIONS {
            assert!(!rl.check_at(a, now), "connection {i} must be allowed");
        }
        assert!(
            rl.check_at(a, now),
            "the connection past the cap is refused"
        );
        rl
    }

    /// Flooding `a` must not limit `b`.
    fn assert_independent(a: IpAddr, b: IpAddr) {
        let now = Instant::now();
        let mut rl = flooded(a, now);
        assert!(!rl.check_at(b, now), "{b} must not inherit {a}'s count");
    }

    #[test]
    fn allows_up_to_the_cap_then_refuses() {
        flooded(ip(1), Instant::now());
    }

    /// A rate limit, not a ban: once the window has passed the address is welcome.
    #[test]
    fn the_window_slides() {
        let now = Instant::now();
        let mut rl = flooded(ip(1), now);
        assert!(!rl.check_at(ip(1), now + WINDOW + Duration::from_secs(1)));
    }

    #[test]
    fn addresses_are_limited_independently() {
        assert_independent(ip(1), ip(2));
    }

    /// A long-running server must not hold an entry per address it has ever seen.
    #[test]
    fn stale_addresses_are_forgotten() {
        let mut rl = RateLimiter::new();
        let now = Instant::now();
        rl.check_at(ip(1), now);
        rl.check_at(ip(2), now);
        assert_eq!(rl.tracked_addresses(), 2);

        rl.check_at(ip(3), now + WINDOW + Duration::from_secs(1));
        assert_eq!(
            rl.tracked_addresses(),
            1,
            "addresses with no connections in the window are dropped"
        );
    }

    /// A scan from a fresh address per packet, all inside one window, must not grow the
    /// table past its cap.
    #[test]
    fn the_address_table_is_capped() {
        let mut rl = RateLimiter::with_limits(MAX_CONNECTIONS, WINDOW).with_max_tracked(64);
        let now = Instant::now();
        for i in 0..10_000u16 {
            rl.check_at(ip6(i), now + Duration::from_millis(u64::from(i)));
        }
        assert!(
            rl.tracked_addresses() <= 64,
            "the table must not grow past its cap, saw {}",
            rl.tracked_addresses()
        );
    }

    /// Eviction forgets quiet addresses, never the one flooding, or filling the table
    /// would be the way to clear your own limit.
    #[test]
    fn eviction_keeps_the_freshest_addresses() {
        let mut rl = RateLimiter::with_limits(3, WINDOW).with_max_tracked(4);
        let now = Instant::now();
        for _ in 0..3 {
            rl.check_at(ip(1), now);
        }
        assert!(rl.check_at(ip(1), now), "the flooder is limited");

        // Fresh addresses overflow the table while the flooder keeps knocking.
        for i in 0..40u16 {
            let t = now + Duration::from_millis(u64::from(i) + 1);
            rl.check_at(ip6(i), t);
            rl.check_at(ip(1), t);
        }

        assert!(rl.tracked_addresses() <= 4);
        assert!(
            rl.check_at(ip(1), now + Duration::from_millis(41)),
            "the address with the most recent connections must still be limited"
        );
    }

    #[test]
    fn ipv6_is_limited_by_prefix_not_by_address() {
        let mut rl = RateLimiter::new();
        let now = Instant::now();
        for i in 0..MAX_CONNECTIONS {
            assert!(
                !rl.check_at(ip6(i as u16), now),
                "connection {i} is within the cap"
            );
        }
        assert!(
            rl.check_at(ip6(9999), now),
            "a fresh address in the same /64 must not reset the limit"
        );
        assert_eq!(
            rl.tracked_addresses(),
            1,
            "the whole /64 is one entry, not one per address"
        );
    }

    #[test]
    fn separate_ipv6_prefixes_are_independent() {
        assert_independent(ip6(1), IpAddr::from([0x2001, 0xdb8, 0, 1, 0, 0, 0, 1]));
    }

    /// IPv4-mapped addresses count per IPv4 address, and share the plain form's bucket
    /// so a dual-stack listener does not hand out two budgets for one address.
    #[test]
    fn ipv4_mapped_addresses_are_counted_per_ipv4_address() {
        assert_independent(parse("::ffff:203.0.113.1"), parse("::ffff:203.0.113.2"));
        let now = Instant::now();
        let mut rl = flooded(parse("::ffff:203.0.113.1"), now);
        assert!(
            rl.check_at(parse("203.0.113.1"), now),
            "the mapped and plain forms of one address must share a bucket"
        );
    }

    #[test]
    fn nat64_addresses_are_counted_per_embedded_ipv4_address() {
        assert_independent(parse("64:ff9b::203.0.113.1"), parse("64:ff9b::203.0.113.2"));
    }

    /// NAT64 is a 96-bit prefix: sharing only its first 32 bits is not NAT64.
    #[test]
    fn a_longer_prefix_match_is_not_treated_as_nat64() {
        assert_independent(parse("64:ff9b:1::1"), parse("64:ff9b:2::1"));
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
