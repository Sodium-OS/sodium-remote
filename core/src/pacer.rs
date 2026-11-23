// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The pacer: each video frame is spread over about half a frame interval, at
//! 1.5x or more of the target rate, so a big frame never bursts into Wi-Fi or
//! switch buffers. Small, urgent traffic (input, cursor,
//! feedback, control, audio) never waits here.
//!
//! A congestion-control probe's padding is paced the same way (`push_padding`),
//! and a real frame cuts it: the padding still queued is dropped, so a probe
//! never holds a frame beyond the bucket.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const PACING_FACTOR: f64 = 1.5;
/// Bytes that may leave back to back before pacing applies: two packets, or
/// 2 ms at the pacing rate (timers in the driver tick at 1 ms).
const BURST_PACKETS_BYTES: f64 = 2.0 * 1200.0;
const BURST_TIME_S: f64 = 0.002;

pub struct Pacer {
    /// Packets, and whether each is a probe's padding.
    queue: VecDeque<(Vec<u8>, bool)>,
    queued_bytes: usize,
    rate_bps: f64,
    /// Leaky-bucket budget in bytes (may go negative: debt).
    budget: f64,
    last: Option<Instant>,
}

impl Pacer {
    pub fn new() -> Self {
        Pacer { queue: VecDeque::new(), queued_bytes: 0, rate_bps: 1e6, budget: BURST_PACKETS_BYTES, last: None }
    }

    /// Queue a frame's packets and set the rate so it drains in `spread`
    /// (half a frame interval), never slower than 1.5x the target.
    pub fn push_frame(&mut self, now: Instant, packets: Vec<Vec<u8>>, target_bps: u64, spread: Duration) {
        self.push(now, packets, false, target_bps, spread);
    }

    /// A probe's padding, paced as a frame of the same size.
    pub fn push_padding(&mut self, now: Instant, packets: Vec<Vec<u8>>, target_bps: u64, spread: Duration) {
        self.push(now, packets, true, target_bps, spread);
    }

    fn push(&mut self, now: Instant, packets: Vec<Vec<u8>>, padding: bool, target_bps: u64, spread: Duration) {
        self.refill(now);
        for p in packets {
            self.queued_bytes += p.len();
            self.queue.push_back((p, padding));
        }
        let drain = self.queued_bytes as f64 * 8.0 / spread.as_secs_f64().max(1e-4);
        self.rate_bps = (target_bps as f64 * PACING_FACTOR).max(drain);
    }

    /// Drop the padding still queued (a frame is coming); returns how many
    /// packets went.
    pub fn cut_padding(&mut self) -> usize {
        let before = self.queue.len();
        self.queue.retain(|(_, padding)| !padding);
        self.queued_bytes = self.queue.iter().map(|(p, _)| p.len()).sum();
        before - self.queue.len()
    }

    /// Retransmissions go to the front of the line.
    pub fn push_urgent(&mut self, now: Instant, packet: Vec<u8>) {
        self.refill(now);
        self.queued_bytes += packet.len();
        self.queue.push_front((packet, false));
    }

    fn refill(&mut self, now: Instant) {
        if let Some(last) = self.last {
            if now <= last {
                return; // an earlier arrival time handed in late: time never runs back here
            }
            let dt = now.saturating_duration_since(last).as_secs_f64();
            let cap = BURST_PACKETS_BYTES.max(self.rate_bps / 8.0 * BURST_TIME_S);
            self.budget = (self.budget + dt * self.rate_bps / 8.0).min(cap);
        }
        self.last = Some(now);
    }

    pub fn poll(&mut self, now: Instant) -> Option<Vec<u8>> {
        self.refill(now);
        if self.budget < 0.0 {
            return None;
        }
        let (p, _) = self.queue.pop_front()?;
        self.queued_bytes -= p.len();
        self.budget -= p.len() as f64;
        Some(p)
    }

    /// When the next queued packet may leave.
    pub fn poll_timeout(&self) -> Option<Instant> {
        if self.queue.is_empty() {
            return None;
        }
        let last = self.last?;
        if self.budget >= 0.0 {
            return Some(last);
        }
        Some(last + Duration::from_secs_f64(-self.budget * 8.0 / self.rate_bps))
    }

    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes
    }

    pub fn rate_bps(&self) -> f64 {
        self.rate_bps
    }
}

impl Default for Pacer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_drains_in_about_half_a_frame_interval() {
        let mut p = Pacer::new();
        let t0 = Instant::now();
        // 100 KB frame at a 10 Mbps target: 1.5x would take 53 ms, so the
        // half-interval rule (8.3 ms) sets the rate.
        let packets: Vec<Vec<u8>> = (0..100).map(|_| vec![0u8; 1000]).collect();
        let spread = Duration::from_micros(8333);
        p.push_frame(t0, packets, 10_000_000, spread);
        let mut now = t0;
        let mut sent = 0;
        while sent < 100 {
            while p.poll(now).is_some() {
                sent += 1;
            }
            if let Some(t) = p.poll_timeout() {
                now = t.max(now + Duration::from_micros(1));
            }
        }
        let took = now - t0;
        assert!(took <= spread + Duration::from_millis(1), "took {took:?}");
        assert!(took >= spread - Duration::from_millis(1), "took {took:?} (bursted)");
    }
}
