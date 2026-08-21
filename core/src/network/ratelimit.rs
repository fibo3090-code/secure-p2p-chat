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

/// How many source addresses one limiter will track at once.
///
/// The per-address history is pruned by [`WINDOW`], which bounds the table for
/// *honest* traffic but not for an attacker: a scan from a fresh address every
/// few milliseconds — trivial from an IPv6 /64, and the normal shape of an
/// internet-wide sweep — adds an entry per packet and only ever drops them
/// `WINDOW` later. That is an unbounded allocation driven by a remote party, so
/// the table is capped as well as aged.
///
/// At the cap, the addresses whose most recent connection is oldest are evicted
/// first. Evicting the *stalest* is what keeps the limit meaningful under the
/// attack that forces it: whoever is connecting hardest has the freshest entry
/// and so is the last to be forgotten. The size is generous next to any real
/// deployment (a busy server sees far fewer than this in a 30-second window) and
/// costs well under a megabyte.
pub const MAX_TRACKED_ADDRESSES: usize = 4096;

/// One address's recent history.
///
/// `last_seen` advances on *every* check, including the ones that are refused,
/// while `attempts` only records the connections that were allowed. The two
/// differ precisely for an address that is over its limit and still trying —
/// which is why eviction orders on `last_seen`: an attacker who keeps knocking
/// must not be able to age their own entry out of the table.
#[derive(Debug)]
struct Entry {
    attempts: Vec<Instant>,
    last_seen: Instant,
}

/// The key a source address is counted under.
///
/// IPv4 is counted per address. **IPv6 is counted per /64**, which is the
/// smallest block anyone is routinely delegated — a single residential or cloud
/// allocation hands its holder 2^64 addresses, so counting per-address means the
/// limit can be stepped around by incrementing a number. Every honest client
/// shares a /64 with the rest of its own LAN, which is the same blast radius
/// IPv4 NAT already gives us, so this costs legitimate users nothing new.
///
/// It deliberately does not go wider than /64: /48 would put unrelated customers
/// of one ISP into a single bucket, which turns a rate limit into collateral
/// damage.
///
/// ⚠️ **Canonicalise before matching.** An IPv4-mapped address (`::ffff:a.b.c.d`)
/// is an `IpAddr::V6`, and its four address bytes live in `octets[12..]` — inside
/// the range the /64 mask zeroes. So did the `0xffff` marker at `octets[10..12]`.
/// Masking one without unwrapping it first mapped *every* IPv4 client to the same
/// key, `::`, and one address could then spend the whole bucket and lock out
/// every other IPv4 client on the server. That was latent only because
/// `relay.rs` binds `0.0.0.0`, which hands us `IpAddr::V4` directly and never
/// takes this branch — the moment anything binds `[::]` (a dual-stack listener,
/// which is the entire point of the /64 work) it becomes a one-line denial of
/// service. `to_ipv4_mapped` is the canonicalisation, and it has to happen
/// before the mask, not after.
fn limiter_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => ip,
        // A dual-stack listener reports IPv4 peers as `::ffff:a.b.c.d`. That is
        // an IPv4 client and must be counted per address, exactly as it would be
        // on an IPv4-only listener.
        IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some() => {
            IpAddr::V4(v6.to_ipv4_mapped().expect("just checked"))
        }
        IpAddr::V6(v6) => {
            let mut octets = v6.octets();
            // Keep the routing prefix, zero the interface identifier.
            octets[8..].fill(0);
            IpAddr::from(octets)
        }
    }
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

    /// A limiter with a caller-chosen cap and window. The relay and the party
    /// server have different connection profiles, so they pick their own.
    pub fn with_limits(max: usize, window: Duration) -> Self {
        Self {
            attempts: HashMap::new(),
            max,
            window,
            max_tracked: MAX_TRACKED_ADDRESSES,
        }
    }

    /// Override how many addresses may be tracked at once. Exposed so the
    /// eviction path is testable without allocating [`MAX_TRACKED_ADDRESSES`]
    /// entries; production uses the default.
    pub fn with_max_tracked(mut self, max_tracked: usize) -> Self {
        self.max_tracked = max_tracked.max(1);
        self
    }

    /// Record a connection from `ip` and report whether it should be refused.
    ///
    /// `now` is passed in so the behaviour is testable without sleeping.
    pub fn check_at(&mut self, ip: IpAddr, now: Instant) -> bool {
        // Count IPv6 by /64 rather than by address; see `limiter_key`.
        let ip = limiter_key(ip);
        // Drop addresses whose whole history has aged out, so a long-running
        // server does not accumulate an entry per address it has ever seen.
        let window = self.window;
        self.attempts.retain(|_, e| {
            e.attempts.iter().any(|t| now.duration_since(*t) < window)
                || now.duration_since(e.last_seen) < window
        });

        // Age-based pruning alone leaves the table at the mercy of whoever is
        // rotating source addresses, so enforce the hard ceiling too. Only a
        // *new* address can grow the table, so this is the one place to check.
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

    /// Drop the addresses whose most recent connection is oldest, until there is
    /// room for one more.
    ///
    /// Evicting an entry forgives that address its recent connections, so the
    /// order matters: the freshest entries are the ones actively being limited,
    /// and dropping those is exactly what an attacker rotating addresses would
    /// want. A batch is taken rather than a single entry so a sustained scan
    /// pays for the sort once every `max_tracked / 16` new addresses instead of
    /// on every one of them.
    fn evict_stalest(&mut self) {
        let target = self.max_tracked.saturating_sub(1);
        let batch = (self.max_tracked / 16).max(1);
        let want_removed = self.attempts.len().saturating_sub(target).max(batch);

        let mut by_recency: Vec<(IpAddr, Instant)> = self
            .attempts
            .iter()
            .map(|(ip, entry)| (*ip, entry.last_seen))
            .collect();
        // Oldest last-seen first: those are the addresses closest to ageing out
        // on their own.
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

    fn ip6(last: u16) -> IpAddr {
        IpAddr::from([0x2001, 0xdb8, 0, 0, 0, 0, 0, last])
    }

    /// A scan from a fresh address every few milliseconds used to add a table
    /// entry per source and only drop it a whole window later, which is an
    /// allocation a remote party controls. The table has a hard ceiling now.
    #[test]
    fn the_address_table_is_capped() {
        let mut rl = RateLimiter::with_limits(MAX_CONNECTIONS, WINDOW).with_max_tracked(64);
        let now = Instant::now();
        for i in 0..10_000u16 {
            // Every connection from a brand-new address, all inside one window
            // so nothing ages out on its own.
            rl.check_at(ip6(i), now + Duration::from_millis(u64::from(i)));
        }
        assert!(
            rl.tracked_addresses() <= 64,
            "the table must not grow past its cap, saw {}",
            rl.tracked_addresses()
        );
    }

    /// Eviction must forget the addresses that have gone quiet, never the one
    /// currently flooding — otherwise the scan that fills the table is also the
    /// way to clear your own limit.
    #[test]
    fn eviction_keeps_the_freshest_addresses() {
        let mut rl = RateLimiter::with_limits(3, WINDOW).with_max_tracked(4);
        let now = Instant::now();

        // The flooder reaches its cap and is being refused.
        for _ in 0..3 {
            rl.check_at(ip(1), now);
        }
        assert!(rl.check_at(ip(1), now), "the flooder is limited");

        // Now a rotation of fresh addresses pushes the table over its ceiling
        // many times over, while the flooder keeps knocking alongside it. Each
        // refused knock still counts as contact, so the flooder is never the
        // stalest entry and never evicted.
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

    /// A /64 is the smallest block anyone is routinely delegated, so counting
    /// IPv6 per address means the limit is bypassed by incrementing a number.
    #[test]
    fn ipv6_is_limited_by_prefix_not_by_address() {
        let mut rl = RateLimiter::new();
        let now = Instant::now();

        // Every connection from a different address inside one /64.
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

    /// …but a genuinely different network is still its own bucket. Widening
    /// beyond /64 would make one ISP customer's flood everyone else's problem.
    #[test]
    fn separate_ipv6_prefixes_are_independent() {
        let mut rl = RateLimiter::new();
        let now = Instant::now();
        let other = IpAddr::from([0x2001, 0xdb8, 0, 1, 0, 0, 0, 1]);

        for _ in 0..MAX_CONNECTIONS {
            rl.check_at(ip6(1), now);
        }
        assert!(rl.check_at(ip6(2), now), "the flooding /64 is limited");
        assert!(
            !rl.check_at(other, now),
            "a different /64 must not inherit someone else's limit"
        );
    }

    /// A dual-stack listener (`[::]`) reports an IPv4 peer as `::ffff:a.b.c.d`,
    /// and the /64 mask zeroes everything from octet 8 — which is where both the
    /// address *and* the `0xffff` marker live. Without canonicalising first,
    /// every IPv4 client in the world hashed to the same key, `::`, and one
    /// address could lock out all the others.
    #[test]
    fn ipv4_mapped_addresses_are_not_collapsed_into_one_bucket() {
        let mapped = |a, b, c, d| IpAddr::V6(std::net::Ipv4Addr::new(a, b, c, d).to_ipv6_mapped());

        assert_ne!(
            limiter_key(mapped(1, 2, 3, 4)),
            limiter_key(mapped(203, 0, 113, 9)),
            "two different IPv4 clients must not share a bucket"
        );
        assert_eq!(
            limiter_key(mapped(203, 0, 113, 9)),
            limiter_key(IpAddr::from([203, 0, 113, 9])),
            "the same client must count the same whether the listener is v4 or dual-stack"
        );

        let mut rl = RateLimiter::new();
        let now = Instant::now();
        for _ in 0..MAX_CONNECTIONS {
            rl.check_at(mapped(1, 2, 3, 4), now);
        }
        assert!(
            rl.check_at(mapped(1, 2, 3, 4), now),
            "the flooder is limited"
        );
        assert!(
            !rl.check_at(mapped(203, 0, 113, 9), now),
            "one IPv4 address must not be able to lock out every other IPv4 client"
        );
    }

    /// The v4-mapped unwrap must not swallow real IPv6. `::ffff:0:0/96` is the
    /// mapped range; `64:ff9b::/96` (NAT64) and ordinary global addresses are not.
    #[test]
    fn real_ipv6_is_still_counted_by_prefix() {
        let nat64 = IpAddr::from([0x0064, 0xff9b, 0, 0, 0, 0, 0xc000, 0x0221]);
        assert_eq!(
            limiter_key(nat64),
            IpAddr::from([0x0064, 0xff9b, 0, 0, 0, 0, 0, 0]),
            "a NAT64 address is IPv6 and is masked to its /64"
        );
        assert_eq!(
            limiter_key(ip6(7)),
            IpAddr::from([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0]),
            "a global IPv6 address keeps its prefix and loses its interface id"
        );
    }

    #[test]
    fn ipv4_is_still_counted_per_address() {
        let mut rl = RateLimiter::new();
        let now = Instant::now();
        for _ in 0..MAX_CONNECTIONS {
            rl.check_at(ip(1), now);
        }
        assert!(rl.check_at(ip(1), now));
        assert!(
            !rl.check_at(ip(2), now),
            "IPv4 neighbours must stay independent"
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
