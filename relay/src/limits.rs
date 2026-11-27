// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Token-bucket rate limits, keyed by client address or device.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Instant;

use crate::proto::DeviceId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// New WebSocket connections per address.
    Conn,
    /// New device registrations per address.
    Register,
    /// Requests per device.
    Msg,
    /// Pairing sessions opened per host.
    PairOpen,
    /// Pairing join attempts per device.
    PairJoin,
    /// Pairing join attempts per address.
    PairJoinIp,
    /// Looking for open pairing windows, per address: 30 a minute, since homes and offices share one.
    PairDiscover,
    /// TURN credential requests per device.
    Turn,
}

impl Kind {
    /// (burst, tokens per second)
    fn rule(self) -> (f64, f64) {
        match self {
            Kind::Conn => (30.0, 30.0 / 60.0),
            Kind::Register => (5.0, 5.0 / 3600.0),
            Kind::Msg => (100.0, 20.0),
            Kind::PairOpen => (10.0, 10.0 / 600.0),
            Kind::PairJoin => (5.0, 5.0 / 60.0),
            Kind::PairJoinIp => (10.0, 10.0 / 600.0),
            Kind::PairDiscover => (30.0, 30.0 / 60.0),
            Kind::Turn => (10.0, 10.0 / 60.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Ip(IpAddr),
    Device(DeviceId),
}

impl Key {
    /// IPv6 addresses are limited per /64, IPv4-mapped ones as IPv4.
    pub fn ip(ip: IpAddr) -> Self {
        let ip = match ip {
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => IpAddr::V4(v4),
                None => {
                    let mut seg = v6.segments();
                    seg[4..].fill(0);
                    IpAddr::V6(seg.into())
                }
            },
            v4 => v4,
        };
        Key::Ip(ip)
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Default)]
pub struct Limits {
    buckets: HashMap<(Kind, Key), Bucket>,
}

impl Limits {
    fn refill(&mut self, kind: Kind, key: Key) -> &mut Bucket {
        let (burst, rate) = kind.rule();
        let now = Instant::now();
        let b = self
            .buckets
            .entry((kind, key))
            .or_insert(Bucket { tokens: burst, last: now });
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(burst);
        b.last = now;
        b
    }

    /// Takes one token; false when the bucket is empty.
    pub fn take(&mut self, kind: Kind, key: Key) -> bool {
        let b = self.refill(kind, key);
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Gives one token back (an unregistered device frees its registration).
    pub fn refund(&mut self, kind: Kind, key: Key) {
        let burst = kind.rule().0;
        let b = self.refill(kind, key);
        b.tokens = (b.tokens + 1.0).min(burst);
    }

    /// Forgets buckets that have refilled completely.
    pub fn gc(&mut self) {
        let now = Instant::now();
        self.buckets.retain(|(kind, _), b| {
            let (burst, rate) = kind.rule();
            b.tokens + now.duration_since(b.last).as_secs_f64() * rate < burst
        });
    }
}
