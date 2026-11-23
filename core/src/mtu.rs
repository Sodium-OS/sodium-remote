// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Path MTU probing (in the spirit of RFC 8899, DPLPMTUD). Every path starts
//! at 1200-byte datagrams, which fit any sane path including TURN and VPN
//! tunnels. Padded probes then try larger sizes; an acknowledged probe raises
//! the size, three silent ones end the search. Probing restarts on a path
//! change and every ten minutes.

use std::time::{Duration, Instant};

pub const BASE_MTU: usize = 1200;
/// UDP payload sizes to try: 1280 (IPv6 minimum), tunnels (WireGuard,
/// a corporate VPN client at 1422), then Ethernet's 1472 over IPv4.
const STEPS: [usize; 5] = [1280, 1350, 1400, 1440, 1472];
const TRIES: u32 = 3;
const RESEARCH: Duration = Duration::from_secs(600);

pub struct MtuProber {
    pub current: usize,
    step: usize,
    tries: u32,
    probe_id: u16,
    inflight: Option<(usize, u16, Instant)>,
    next_at: Option<Instant>,
}

impl MtuProber {
    pub fn new(now: Instant) -> Self {
        MtuProber { current: BASE_MTU, step: 0, tries: 0, probe_id: 0, inflight: None, next_at: Some(now) }
    }

    /// Start over (a new path).
    pub fn reset(&mut self, now: Instant) {
        *self = MtuProber { probe_id: self.probe_id, ..MtuProber::new(now) };
    }

    /// A probe to send now: (datagram size, probe id).
    pub fn poll_probe(&mut self, now: Instant, rtt: Duration) -> Option<(usize, u16)> {
        let timeout = (rtt * 2).max(Duration::from_millis(200));
        if let Some((_, _, sent)) = self.inflight {
            if now < sent + timeout {
                return None;
            }
            self.inflight = None;
            self.tries += 1;
            if self.tries >= TRIES {
                self.step = STEPS.len(); // search done at the last good size
                self.next_at = Some(now + RESEARCH);
            }
        }
        if self.step >= STEPS.len() {
            if self.next_at.is_some_and(|t| now >= t) {
                self.step = STEPS.iter().position(|&s| s > self.current).unwrap_or(STEPS.len());
                self.tries = 0;
            }
            if self.step >= STEPS.len() {
                return None;
            }
        }
        if self.next_at.is_some_and(|t| now < t) {
            return None;
        }
        self.probe_id = self.probe_id.wrapping_add(1);
        let size = STEPS[self.step];
        self.inflight = Some((size, self.probe_id, now));
        Some((size, self.probe_id))
    }

    pub fn on_ack(&mut self, size: usize, id: u16) {
        if let Some((s, i, _)) = self.inflight {
            if s == size && i == id {
                self.current = self.current.max(size);
                self.inflight = None;
                self.tries = 0;
                self.step += 1;
                if self.step >= STEPS.len() {
                    self.next_at = None;
                }
            }
        }
    }

    pub fn poll_timeout(&self, rtt: Duration) -> Option<Instant> {
        match self.inflight {
            Some((_, _, sent)) => Some(sent + (rtt * 2).max(Duration::from_millis(200))),
            None if self.step < STEPS.len() => self.next_at,
            None => self.next_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn climbs_until_probes_stop_coming_back() {
        let mut now = Instant::now();
        let rtt = Duration::from_millis(10);
        let mut p = MtuProber::new(now);
        assert_eq!(p.current, 1200);
        // The path carries up to 1400-byte datagrams (like a VPN).
        for _ in 0..20 {
            if let Some((size, id)) = p.poll_probe(now, rtt) {
                if size <= 1400 {
                    p.on_ack(size, id);
                }
            }
            now += Duration::from_millis(250);
        }
        assert_eq!(p.current, 1400);
        assert_eq!(p.poll_probe(now, rtt), None, "the search is over");
    }
}
