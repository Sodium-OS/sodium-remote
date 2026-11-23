// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Clock sync and RTT from ping/pong exchanges (NTP's four timestamps).
//!
//! The offset is taken from the lowest-RTT sample of the last eight, which is
//! the one least skewed by queueing. Both sides ping, so both know the RTT
//! (the receiver needs it to decide whether NACK is worth it).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

const FAST_PINGS: u32 = 10;
const FAST_INTERVAL: Duration = Duration::from_millis(100);
const SLOW_INTERVAL: Duration = Duration::from_millis(500);

pub struct ClockSync {
    samples: VecDeque<(i64, i64)>,
    /// Smoothed RTT.
    pub srtt: Option<Duration>,
    /// Lowest recent RTT.
    pub min_rtt: Option<Duration>,
    /// Peer clock minus our clock, in microseconds.
    pub offset_us: Option<i64>,
    pings_sent: u32,
    next_ping: Option<Instant>,
}

impl Default for ClockSync {
    fn default() -> Self {
        ClockSync { samples: VecDeque::new(), srtt: None, min_rtt: None, offset_us: None, pings_sent: 0, next_ping: None }
    }
}

impl ClockSync {
    /// Is a ping due? Returns the time to stamp in it.
    pub fn poll_ping(&mut self, now: Instant) -> bool {
        if self.next_ping.is_some_and(|t| now < t) {
            return false;
        }
        self.pings_sent += 1;
        let gap = if self.pings_sent < FAST_PINGS { FAST_INTERVAL } else { SLOW_INTERVAL };
        self.next_ping = Some(now + gap);
        true
    }

    pub fn poll_timeout(&self) -> Option<Instant> {
        self.next_ping
    }

    /// t0: our send, t1: peer receive, t2: peer send, t3: our receive.
    pub fn on_pong(&mut self, t0: u64, t1: u64, t2: u64, t3: u64) {
        let (t0, t1, t2, t3) = (t0 as i64, t1 as i64, t2 as i64, t3 as i64);
        let rtt = ((t3 - t0) - (t2 - t1)).max(0);
        let offset = ((t1 - t0) + (t2 - t3)) / 2;
        self.samples.push_back((rtt, offset));
        if self.samples.len() > 8 {
            self.samples.pop_front();
        }
        let best = self.samples.iter().min_by_key(|s| s.0).copied().unwrap();
        self.offset_us = Some(best.1);
        self.min_rtt = Some(Duration::from_micros(best.0 as u64));
        let r = Duration::from_micros(rtt as u64);
        self.srtt = Some(match self.srtt {
            None => r,
            Some(s) => s.mul_f64(0.875) + r.mul_f64(0.125),
        });
    }

    /// The RTT to plan with: smoothed, or a cautious guess before any sample.
    pub fn rtt(&self) -> Duration {
        self.srtt.unwrap_or(Duration::from_millis(100))
    }

    /// Convert a peer timestamp to our clock.
    pub fn to_local(&self, peer_us: u64) -> Option<i64> {
        Some(peer_us as i64 - self.offset_us?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_and_rtt_from_an_asymmetric_queue() {
        let mut c = ClockSync::default();
        // Peer clock is 1 s ahead; path 5 ms each way; one sample has 20 ms of queue.
        let off = 1_000_000u64;
        c.on_pong(0, 5_000 + off + 20_000, 5_100 + off + 20_000, 30_100);
        c.on_pong(100_000, 105_000 + off, 105_100 + off, 110_100);
        assert_eq!(c.offset_us, Some(1_000_000));
        assert_eq!(c.min_rtt, Some(Duration::from_millis(10)));
    }
}
