// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Limits on a host's LAN listener, which anyone on the network can reach:
//! how many connections may be open at once (in total and from one address),
//! and how often pairing may be tried (`PairHello`), from one address and in
//! total. An IPv6 address counts by its /64, as one host can hold many.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Connections open at once, in total and from one address.
pub const MAX_CONNS: usize = 32;
pub const MAX_CONNS_PER_ADDR: usize = 4;

/// Pairing tries from one address: 3 at once, then one every 10 seconds.
pub const PAIR_BURST_PER_ADDR: u32 = 3;
pub const PAIR_EVERY_PER_ADDR: Duration = Duration::from_secs(10);
/// Pairing tries from everyone together: 10 at once, then one every 2 seconds.
pub const PAIR_BURST: u32 = 10;
pub const PAIR_EVERY: Duration = Duration::from_secs(2);

/// The address a limit counts by.
fn source(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
        v4 => v4,
    }
}

#[derive(Default)]
struct Open {
    total: usize,
    by_addr: HashMap<IpAddr, usize>,
}

/// The connections open now. Cheap to clone; clones share the count.
#[derive(Clone, Default)]
pub struct Slots(Arc<Mutex<Open>>);

/// One open connection's place; dropping it frees the place.
pub struct Slot {
    slots: Slots,
    addr: IpAddr,
}

impl Slots {
    /// A place for a new connection from `ip`, or `None` when it would pass either cap.
    pub fn take(&self, ip: IpAddr) -> Option<Slot> {
        let addr = source(ip);
        let mut o = self.0.lock().unwrap();
        let n = o.by_addr.get(&addr).copied().unwrap_or(0);
        if o.total >= MAX_CONNS || n >= MAX_CONNS_PER_ADDR {
            return None;
        }
        o.total += 1;
        o.by_addr.insert(addr, n + 1);
        Some(Slot { slots: self.clone(), addr })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut o = self.slots.0.lock().unwrap();
        o.total -= 1;
        if let Some(n) = o.by_addr.get_mut(&self.addr) {
            *n -= 1;
            if *n == 0 {
                o.by_addr.remove(&self.addr);
            }
        }
    }
}

/// A token bucket: `burst` tokens, one more every `every`.
#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn full(burst: u32, now: Instant) -> Bucket {
        Bucket { tokens: burst as f64, at: now }
    }

    fn refill(&mut self, burst: u32, every: Duration, now: Instant) {
        let gained = now.saturating_duration_since(self.at).as_secs_f64() / every.as_secs_f64();
        self.tokens = (self.tokens + gained).min(burst as f64);
        self.at = now;
    }
}

/// How often pairing may be tried on the LAN.
pub struct PairRate {
    all: Bucket,
    by_addr: HashMap<IpAddr, Bucket>,
}

impl PairRate {
    pub fn new(now: Instant) -> PairRate {
        PairRate { all: Bucket::full(PAIR_BURST, now), by_addr: HashMap::new() }
    }

    /// May `ip` try pairing now? Takes a token from its bucket and the shared one when so.
    pub fn allow(&mut self, ip: IpAddr, now: Instant) -> bool {
        let addr = source(ip);
        self.all.refill(PAIR_BURST, PAIR_EVERY, now);
        let mine = self.by_addr.entry(addr).or_insert_with(|| Bucket::full(PAIR_BURST_PER_ADDR, now));
        mine.refill(PAIR_BURST_PER_ADDR, PAIR_EVERY_PER_ADDR, now);
        let ok = mine.tokens >= 1.0 && self.all.tokens >= 1.0;
        if ok {
            mine.tokens -= 1.0;
            self.all.tokens -= 1.0;
        }
        // Forget addresses whose bucket has filled up again: they are as new.
        if self.by_addr.len() > 256 {
            self.by_addr.retain(|_, b| {
                b.refill(PAIR_BURST_PER_ADDR, PAIR_EVERY_PER_ADDR, now);
                b.tokens < PAIR_BURST_PER_ADDR as f64
            });
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn connections_are_capped_per_address_and_freed_on_drop() {
        let slots = Slots::default();
        let held: Vec<Slot> = (0..MAX_CONNS_PER_ADDR).map(|_| slots.take(ip("192.0.2.9")).unwrap()).collect();
        assert!(slots.take(ip("192.0.2.9")).is_none(), "one more from the same address");
        assert!(slots.take(ip("192.0.2.10")).is_some(), "another address still gets in");
        drop(held);
        assert!(slots.take(ip("192.0.2.9")).is_some());
    }

    #[test]
    fn pairing_tries_are_rate_limited_per_address_and_in_total() {
        let t0 = Instant::now();
        let mut r = PairRate::new(t0);
        for _ in 0..PAIR_BURST_PER_ADDR {
            assert!(r.allow(ip("198.51.100.2"), t0));
        }
        assert!(!r.allow(ip("198.51.100.2"), t0), "the fourth try at once is refused");
        assert!(r.allow(ip("198.51.100.2"), t0 + PAIR_EVERY_PER_ADDR), "one more after the refill");
        // Many addresses together hit the shared limit.
        let mut ok = 0;
        for i in 0..50 {
            ok += r.allow(ip(&format!("203.0.113.{i}")), t0 + PAIR_EVERY_PER_ADDR) as u32;
        }
        assert!(ok < PAIR_BURST, "{ok} tries got through from 50 addresses");
    }
}
