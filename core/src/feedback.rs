// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Transport-wide per-packet feedback (the idea of RTP's transport-cc).
//!
//! Every data packet's crypto counter doubles as its transport-wide sequence
//! number. The receiver reports, about once a frame, which counters arrived
//! and when (its own clock, microseconds). The sender joins that with its send
//! times to see one-way delay variation and loss for every packet.

use crate::wire::{Reader, Writer};
use std::collections::VecDeque;

/// Subtypes on the feedback channel.
pub mod sub {
    pub const TRANSPORT: u8 = 0;
    pub const NACK: u8 = 1;
    pub const FRAME_LOST: u8 = 2;
    pub const RECEIVER_REPORT: u8 = 3;
}

const MAX_RANGE: u64 = 4096;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransportFeedback {
    pub fb_seq: u16,
    /// First counter of the reported range.
    pub from: u64,
    /// For each counter in `from..from + received.len()`: its arrival time in
    /// the receiver's microseconds, or `None` if it hasn't arrived.
    pub received: Vec<Option<u64>>,
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}
fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

impl TransportFeedback {
    pub fn encode(&self, w: &mut Writer) {
        w.u8(sub::TRANSPORT).u16(self.fb_seq).u64(self.from).u16(self.received.len() as u16);
        let mut bitmap = vec![0u8; self.received.len().div_ceil(8)];
        for (i, r) in self.received.iter().enumerate() {
            if r.is_some() {
                bitmap[i / 8] |= 1 << (i % 8);
            }
        }
        w.bytes(&bitmap);
        let mut prev: Option<u64> = None;
        for t in self.received.iter().flatten() {
            match prev {
                None => w.varint(*t),
                Some(p) => w.varint(zigzag(*t as i64 - p as i64)),
            };
            prev = Some(*t);
        }
    }

    /// Decode from the bytes after the subtype byte.
    pub fn decode(r: &mut Reader) -> Option<Self> {
        let fb_seq = r.u16()?;
        let from = r.u64()?;
        let count = r.u16()? as usize;
        let bitmap = r.take(count.div_ceil(8))?.to_vec();
        let mut received = Vec::with_capacity(count);
        let mut prev: Option<u64> = None;
        for i in 0..count {
            if bitmap[i / 8] & (1 << (i % 8)) != 0 {
                let t = match prev {
                    None => r.varint()?,
                    Some(p) => (p as i64 + unzigzag(r.varint()?)) as u64,
                };
                prev = Some(t);
                received.push(Some(t));
            } else {
                received.push(None);
            }
        }
        Some(TransportFeedback { fb_seq, from, received })
    }
}

/// Receiver side: remembers arrivals until the next feedback goes out.
#[derive(Default)]
pub struct FeedbackRecorder {
    /// Next counter not yet covered by a feedback.
    from: Option<u64>,
    arrivals: Vec<(u64, u64)>,
    highest: Option<u64>,
    fb_seq: u16,
}

impl FeedbackRecorder {
    pub fn record(&mut self, counter: u64, arrival_us: u64) {
        if let Some(from) = self.from {
            if counter < from {
                return; // reordered behind a sent report; the sender counts it lost
            }
        }
        self.arrivals.push((counter, arrival_us));
        self.highest = Some(self.highest.map_or(counter, |h| h.max(counter)));
    }

    pub fn has_news(&self) -> bool {
        !self.arrivals.is_empty()
    }

    pub fn build(&mut self) -> Option<TransportFeedback> {
        let highest = self.highest?;
        if self.arrivals.is_empty() {
            return None;
        }
        let lowest = self.arrivals.iter().map(|a| a.0).min()?;
        let from = self.from.unwrap_or(lowest).max(highest.saturating_sub(MAX_RANGE - 1));
        let mut received = vec![None; (highest + 1 - from) as usize];
        for &(c, t) in &self.arrivals {
            if c >= from {
                received[(c - from) as usize] = Some(t);
            }
        }
        self.arrivals.clear();
        self.from = Some(highest + 1);
        self.fb_seq = self.fb_seq.wrapping_add(1);
        Some(TransportFeedback { fb_seq: self.fb_seq, from, received })
    }
}

/// One sent packet, as the congestion controller sees it.
#[derive(Clone, Copy, Debug)]
pub struct SentPacket {
    pub send_us: u64,
    pub size: usize,
    /// False for packets the congestion controller must not read: probe
    /// trains sent on another route.
    pub cc: bool,
    /// Padding of this controller probe.
    pub probe: Option<u32>,
}

/// Sender side: send time and size of each counter, until feedback covers it.
#[derive(Default)]
pub struct SentLog {
    first: u64,
    packets: VecDeque<Option<SentPacket>>,
}

impl SentLog {
    pub fn record(&mut self, counter: u64, send_us: u64, size: usize) {
        self.record_as(counter, send_us, size, true);
    }

    /// Record a packet; `cc: false` keeps it out of congestion control.
    pub fn record_as(&mut self, counter: u64, send_us: u64, size: usize, cc: bool) {
        self.push(counter, SentPacket { send_us, size, cc, probe: None });
    }

    /// Record a padding packet of the controller's probe `id`.
    pub fn record_probe(&mut self, counter: u64, send_us: u64, size: usize, id: u32) {
        self.push(counter, SentPacket { send_us, size, cc: true, probe: Some(id) });
    }

    fn push(&mut self, counter: u64, p: SentPacket) {
        if self.packets.is_empty() {
            self.first = counter;
        }
        while self.first + (self.packets.len() as u64) < counter {
            self.packets.push_back(None);
        }
        self.packets.push_back(Some(p));
        while self.packets.len() > 1 << 15 {
            self.packets.pop_front();
            self.first += 1;
        }
    }

    pub fn get(&self, counter: u64) -> Option<SentPacket> {
        if counter < self.first {
            return None;
        }
        *self.packets.get((counter - self.first) as usize)?
    }

    /// Forget everything before `counter`.
    pub fn forget_before(&mut self, counter: u64) {
        while self.first < counter && !self.packets.is_empty() {
            self.packets.pop_front();
            self.first += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_roundtrip_with_loss_and_reorder() {
        let mut rec = FeedbackRecorder::default();
        rec.record(100, 5_000);
        rec.record(102, 5_900);
        rec.record(101, 6_100); // arrived late
        let fb = rec.build().unwrap();
        assert_eq!(fb.from, 100);
        let mut w = Writer::default();
        fb.encode(&mut w);
        let mut r = Reader::new(&w.buf);
        assert_eq!(r.u8(), Some(sub::TRANSPORT));
        assert_eq!(TransportFeedback::decode(&mut r).unwrap(), fb);
        rec.record(105, 7_000);
        let fb2 = rec.build().unwrap();
        assert_eq!(fb2.from, 103);
        assert_eq!(fb2.received, vec![None, None, Some(7_000)]);
    }
}
