// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The audio receive side: an adaptive jitter buffer around
//! 20 ms, Opus in-band FEC for a single lost packet, concealment otherwise,
//! and capture times on the local clock for A/V sync.
//!
//! Call `pop` once per 10 ms of playout (the audio device's callback). It
//! says what to decode for that 10 ms: a packet, the *next* packet with FEC
//! (`opus_decode(.., decode_fec = 1)` rebuilds the lost one from it), or
//! concealment (`opus_decode` with no data). The target delay is the 95th
//! percentile of packet delay variation over the last 2 s plus one frame,
//! at least 20 ms. The buffer grows by concealing without advancing, and
//! shrinks by skipping a frame, at most once per 200 ms.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// 10 ms at 48 kHz.
pub const FRAME_TS: u32 = 480;
const TS_PER_MS: f64 = 48.0;
const MIN_TARGET_MS: f64 = 20.0;
const MAX_TARGET_MS: f64 = 150.0;
const JITTER_WINDOW: Duration = Duration::from_secs(2);
const ADJUST_GAP: Duration = Duration::from_millis(200);
const SHRINK_AFTER: Duration = Duration::from_millis(500);
const MAX_PACKETS: usize = 64;
/// After this many concealed frames with nothing buffered, playout stops and
/// buffers again (the sender went quiet).
const IDLE_CONCEALS: u32 = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioPlay {
    /// Decode this Opus packet.
    Packet(Vec<u8>),
    /// This frame was lost: decode this packet (the next one) with FEC on.
    Fec(Vec<u8>),
    /// Nothing usable: decode with no data (packet loss concealment).
    Conceal,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioOut {
    /// The 48 kHz timestamp of the 10 ms this covers.
    pub timestamp: u32,
    pub play: AudioPlay,
    /// When the sender captured it, on our clock (µs since our session start),
    /// for A/V sync against `ReceivedFrame::send_local_us`.
    pub capture_local_us: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JitterStats {
    pub target_ms: f64,
    pub depth_ms: f64,
    pub played: u64,
    pub fec: u64,
    pub concealed: u64,
    pub late: u64,
    pub dropped: u64,
}

struct Slot {
    data: Vec<u8>,
    capture_local_us: Option<i64>,
    arrival: Instant,
}

pub struct JitterBuffer {
    epoch: Instant,
    frame: u64,
    slots: BTreeMap<u64, Slot>,
    /// Extended timestamp of the next frame to play; None while buffering.
    playout: Option<u64>,
    highest: Option<(u32, u64)>,
    /// (arrival, transit in ms) for the delay-variation window.
    transit: VecDeque<(Instant, f64)>,
    target_ms: f64,
    over_since: Option<Instant>,
    last_adjust: Option<Instant>,
    idle: u32,
    stats: JitterStats,
}

impl JitterBuffer {
    pub fn new(now: Instant) -> Self {
        JitterBuffer {
            epoch: now,
            frame: FRAME_TS as u64,
            slots: BTreeMap::new(),
            playout: None,
            highest: None,
            transit: VecDeque::new(),
            target_ms: MIN_TARGET_MS,
            over_since: None,
            last_adjust: None,
            idle: 0,
            stats: JitterStats { target_ms: MIN_TARGET_MS, ..Default::default() },
        }
    }

    fn extend(&mut self, ts: u32) -> u64 {
        match self.highest {
            None => {
                let ext = (1u64 << 32) + ts as u64; // room to go back
                self.highest = Some((ts, ext));
                ext
            }
            Some((hts, hext)) => {
                let ext = (hext as i64 + ts.wrapping_sub(hts) as i32 as i64) as u64;
                if ext > hext {
                    self.highest = Some((ts, ext));
                }
                ext
            }
        }
    }

    fn ms(&self, ts: u64) -> f64 {
        ts as f64 / TS_PER_MS
    }

    /// A received audio packet.
    pub fn push(&mut self, now: Instant, timestamp: u32, data: Vec<u8>, capture_local_us: Option<i64>) {
        let ext = self.extend(timestamp);
        let arrival_ms = now.saturating_duration_since(self.epoch).as_secs_f64() * 1e3;
        self.transit.push_back((now, arrival_ms - self.ms(ext)));
        while self.transit.front().is_some_and(|(t, _)| now.saturating_duration_since(*t) > JITTER_WINDOW) {
            self.transit.pop_front();
        }
        self.update_target();
        if self.playout.is_some_and(|p| ext < p) {
            self.stats.late += 1;
            return;
        }
        self.slots.insert(ext, Slot { data, capture_local_us, arrival: now });
        while self.slots.len() > MAX_PACKETS {
            self.slots.pop_first();
            self.stats.dropped += 1;
        }
        self.idle = 0;
    }

    fn update_target(&mut self) {
        let min = self.transit.iter().map(|t| t.1).fold(f64::INFINITY, f64::min);
        let mut d: Vec<f64> = self.transit.iter().map(|t| t.1 - min).collect();
        d.sort_by(f64::total_cmp);
        let p95 = d.get(((d.len().max(1) - 1) as f64 * 0.95).round() as usize).copied().unwrap_or(0.0);
        self.target_ms = (p95 + self.frame as f64 / TS_PER_MS).clamp(MIN_TARGET_MS, MAX_TARGET_MS);
        self.stats.target_ms = self.target_ms;
    }

    /// Buffered audio from the playout point on, in ms.
    fn depth_ms(&self, from: u64) -> f64 {
        self.slots.last_key_value().map_or(0.0, |(&k, _)| self.ms((k + self.frame).saturating_sub(from)))
    }

    fn may_adjust(&self, now: Instant) -> bool {
        self.last_adjust.is_none_or(|t| now.saturating_duration_since(t) >= ADJUST_GAP)
    }

    /// What to play for the next 10 ms. None while buffering (play silence).
    pub fn pop(&mut self, now: Instant) -> Option<AudioOut> {
        let p = match self.playout {
            Some(p) => p,
            None => {
                let (&first, slot) = self.slots.first_key_value()?;
                let waited = now.saturating_duration_since(slot.arrival).as_secs_f64() * 1e3;
                if self.depth_ms(first) < self.target_ms && waited < self.target_ms {
                    return None;
                }
                self.playout = Some(first);
                first
            }
        };
        while self.slots.first_key_value().is_some_and(|(&k, _)| k < p) {
            self.slots.pop_first();
        }
        let depth = self.depth_ms(p);
        self.stats.depth_ms = depth;
        let frame_ms = self.frame as f64 / TS_PER_MS;
        let ts = p as u32;

        if depth > self.target_ms + 1.5 * frame_ms {
            let since = *self.over_since.get_or_insert(now);
            if now.saturating_duration_since(since) >= SHRINK_AFTER && self.may_adjust(now) && self.slots.contains_key(&(p + self.frame)) {
                self.slots.remove(&p);
                self.stats.dropped += 1;
                self.last_adjust = Some(now);
                self.playout = Some(p + self.frame);
                return self.pop(now);
            }
        } else {
            self.over_since = None;
        }

        if let Some(s) = self.slots.get(&p) {
            if depth < self.target_ms - frame_ms && self.may_adjust(now) {
                // Grow: conceal once without advancing.
                self.last_adjust = Some(now);
                self.stats.concealed += 1;
                return Some(AudioOut { timestamp: ts, play: AudioPlay::Conceal, capture_local_us: None });
            }
            let _ = s;
            let s = self.slots.remove(&p).unwrap();
            self.playout = Some(p + self.frame);
            self.stats.played += 1;
            return Some(AudioOut { timestamp: ts, play: AudioPlay::Packet(s.data), capture_local_us: s.capture_local_us });
        }
        if self.slots.keys().next().is_some() {
            // Lost (or still reordered past its slot): FEC from the next packet if we have it.
            self.playout = Some(p + self.frame);
            if let Some(next) = self.slots.get(&(p + self.frame)) {
                self.stats.fec += 1;
                let capture = next.capture_local_us.map(|c| c - (frame_ms * 1e3) as i64);
                return Some(AudioOut { timestamp: ts, play: AudioPlay::Fec(next.data.clone()), capture_local_us: capture });
            }
            self.stats.concealed += 1;
            return Some(AudioOut { timestamp: ts, play: AudioPlay::Conceal, capture_local_us: None });
        }
        // Underrun: nothing at or after the playout point. Conceal without
        // advancing, so the delay grows to what the network needs.
        self.idle += 1;
        self.stats.concealed += 1;
        if self.idle >= IDLE_CONCEALS {
            self.playout = None;
            self.idle = 0;
        }
        Some(AudioOut { timestamp: ts, play: AudioPlay::Conceal, capture_local_us: None })
    }

    pub fn stats(&self) -> JitterStats {
        self.stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plays_in_order_uses_fec_for_one_loss_and_stretches_on_underrun() {
        let t0 = Instant::now();
        let mut jb = JitterBuffer::new(t0);
        let ms = Duration::from_millis;
        // Packets every 10 ms; packet 5 is lost; 8..10 arrive 40 ms late.
        let mut played = Vec::new();
        for tick in 0..40u32 {
            let now = t0 + ms(10 * tick as u64);
            if tick < 8 && tick != 5 {
                jb.push(now, tick * FRAME_TS, vec![tick as u8], Some(tick as i64 * 10_000));
            }
            if tick >= 12 && tick < 15 {
                let n = tick - 4;
                jb.push(now, n * FRAME_TS, vec![n as u8], None);
            }
            if let Some(o) = jb.pop(now) {
                played.push(o.play);
            }
        }
        assert_eq!(played[0], AudioPlay::Packet(vec![0]), "starts at the first packet once 20 ms are buffered");
        let fec = played.iter().position(|p| *p == AudioPlay::Fec(vec![6])).expect("frame 5 rebuilt from 6's FEC");
        assert_eq!(played[fec - 1], AudioPlay::Packet(vec![4]));
        assert_eq!(played[fec + 1], AudioPlay::Packet(vec![6]));
        // The late packets still play, after concealment, in order.
        let late: Vec<_> = played.iter().filter(|p| matches!(p, AudioPlay::Packet(d) if d[0] >= 8)).collect();
        assert_eq!(late, vec![&AudioPlay::Packet(vec![8]), &AudioPlay::Packet(vec![9]), &AudioPlay::Packet(vec![10])]);
        assert!(jb.stats().target_ms > MIN_TARGET_MS, "the delay spike raised the target: {:?}", jb.stats());
    }
}
