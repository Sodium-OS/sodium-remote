// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The control channel's small ARQ: numbered segments, cumulative + selective
//! ACKs, retransmit on timeout, in-order delivery, and fragmentation so a
//! message may be larger than one datagram (clipboard, file chunks). A fixed
//! window of segments in flight is the flow control.

use crate::wire::{Channel, Reader, Writer, seq32_newer};
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

pub mod sub {
    pub const DATA: u8 = 0;
    pub const ACK: u8 = 1;
}

const FLAG_MORE: u8 = 1;
const WINDOW: usize = 256;
/// The largest message the receiver reassembles. The largest real one is a
/// 256 × 256 cursor image (256 KiB); clipboard data comes in 64 KiB chunks.
pub const MAX_MESSAGE: usize = 1 << 20;
const DATA_HEADER: usize = 1 + 1 + 4 + 1 + 1;

struct Segment {
    seq: u32,
    kind: u8,
    flags: u8,
    data: Vec<u8>,
    sent_at: Option<Instant>,
    tries: u32,
}

#[derive(Default)]
pub struct ReliableSender {
    next_seq: u32,
    pending: VecDeque<Segment>,
    inflight: BTreeMap<u32, Segment>,
}

impl ReliableSender {
    /// Queue a message of `kind`, cut to fit `max_payload`-byte datagrams.
    pub fn send(&mut self, kind: u8, data: &[u8], max_payload: usize) {
        let room = max_payload.saturating_sub(DATA_HEADER).max(1);
        let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[][..]] } else { data.chunks(room).collect() };
        let last = chunks.len() - 1;
        for (i, c) in chunks.into_iter().enumerate() {
            self.pending.push_back(Segment {
                seq: self.next_seq,
                kind,
                flags: if i < last { FLAG_MORE } else { 0 },
                data: c.to_vec(),
                sent_at: None,
                tries: 0,
            });
            self.next_seq = self.next_seq.wrapping_add(1);
        }
    }

    fn encode(s: &Segment) -> Vec<u8> {
        let mut w = Writer::with_capacity(DATA_HEADER + s.data.len());
        w.u8(Channel::Control as u8).u8(sub::DATA).u32(s.seq).u8(s.kind).u8(s.flags).bytes(&s.data);
        w.buf
    }

    /// Segments to (re)send now.
    pub fn poll(&mut self, now: Instant, rto: Duration) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for s in self.inflight.values_mut() {
            if s.sent_at.is_some_and(|t| now >= t + rto * (1 << s.tries.min(4))) {
                s.sent_at = Some(now);
                s.tries += 1;
                out.push(Self::encode(s));
            }
        }
        while self.inflight.len() < WINDOW {
            let Some(mut s) = self.pending.pop_front() else { break };
            s.sent_at = Some(now);
            out.push(Self::encode(&s));
            self.inflight.insert(s.seq, s);
        }
        out
    }

    pub fn poll_timeout(&self, rto: Duration) -> Option<Instant> {
        self.inflight.values().filter_map(|s| Some(s.sent_at? + rto * (1 << s.tries.min(4)))).min()
    }

    /// Handle an ACK (after the subtype byte).
    pub fn on_ack(&mut self, r: &mut Reader) -> Option<()> {
        let cum = r.u32()?;
        let sack = r.u64()?;
        self.inflight.retain(|&seq, _| seq32_newer(seq, cum));
        for i in 0..64u32 {
            if sack & (1 << i) != 0 {
                self.inflight.remove(&cum.wrapping_add(2 + i));
            }
        }
        Some(())
    }

    pub fn idle(&self) -> bool {
        self.pending.is_empty() && self.inflight.is_empty()
    }
}

#[derive(Default)]
pub struct ReliableReceiver {
    next: u32,
    buffered: BTreeMap<u32, (u8, u8, Vec<u8>)>,
    partial: Vec<u8>,
    too_large: bool,
}

impl ReliableReceiver {
    /// Handle a DATA segment (after the subtype byte). Returns whole messages
    /// now deliverable in order, and the ACK to send. `None` for a malformed
    /// segment, or once a message grows past [`MAX_MESSAGE`] ([`Self::too_large`]).
    pub fn on_data(&mut self, r: &mut Reader) -> Option<(Vec<(u8, Vec<u8>)>, Vec<u8>)> {
        if self.too_large {
            return None;
        }
        let seq = r.u32()?;
        let kind = r.u8()?;
        let flags = r.u8()?;
        let data = r.rest().to_vec();
        if seq == self.next || seq32_newer(seq, self.next) {
            if seq.wrapping_sub(self.next) < 4 * WINDOW as u32 {
                self.buffered.insert(seq, (kind, flags, data));
            }
        }
        let mut out = Vec::new();
        while let Some((kind, flags, data)) = self.buffered.remove(&self.next) {
            self.next = self.next.wrapping_add(1);
            if self.partial.len() + data.len() > MAX_MESSAGE {
                self.too_large = true;
                self.partial = Vec::new();
                self.buffered.clear();
                return None;
            }
            self.partial.extend_from_slice(&data);
            if flags & FLAG_MORE == 0 {
                out.push((kind, std::mem::take(&mut self.partial)));
            }
        }
        let cum = self.next.wrapping_sub(1);
        let mut sack = 0u64;
        for i in 0..64u32 {
            if self.buffered.contains_key(&cum.wrapping_add(2 + i)) {
                sack |= 1 << i;
            }
        }
        let mut w = Writer::with_capacity(14);
        w.u8(Channel::Control as u8).u8(sub::ACK).u32(cum).u64(sack);
        Some((out, w.buf))
    }

    /// The peer sent a message larger than [`MAX_MESSAGE`]: end the session.
    pub fn too_large(&self) -> bool {
        self.too_large
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(p: &[u8]) -> Reader<'_> {
        let mut r = Reader::new(p);
        r.u8();
        r.u8();
        r
    }

    #[test]
    fn a_fragmented_message_survives_a_loss_and_arrives_whole() {
        let now = Instant::now();
        let rto = Duration::from_millis(50);
        let mut tx = ReliableSender::default();
        let mut rx = ReliableReceiver::default();
        let msg: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
        tx.send(9, &msg, 1200);
        let first = tx.poll(now, rto);
        assert_eq!(first.len(), 3);
        let mut got = Vec::new();
        let mut last_ack = None;
        for p in first.iter().skip(1) {
            let (m, ack) = rx.on_data(&mut body(p)).unwrap();
            got.extend(m);
            last_ack = Some(ack);
        }
        assert!(got.is_empty(), "held until the hole is filled");
        tx.on_ack(&mut body(&last_ack.unwrap())).unwrap();
        let again = tx.poll(now + rto, rto);
        assert_eq!(again.len(), 1, "only the lost segment is resent");
        let (m, ack) = rx.on_data(&mut body(&again[0])).unwrap();
        assert_eq!(m, vec![(9, msg)]);
        tx.on_ack(&mut body(&ack)).unwrap();
        assert!(tx.idle());
    }

    #[test]
    fn a_message_past_the_cap_is_refused() {
        let now = Instant::now();
        let mut tx = ReliableSender::default();
        let mut rx = ReliableReceiver::default();
        tx.send(9, &vec![7u8; MAX_MESSAGE], 1200);
        tx.send(9, &vec![7u8; MAX_MESSAGE + 1], 1200);
        let mut got = Vec::new();
        let mut refused = false;
        // Acked as it goes, so the window keeps moving.
        for _ in 0..4000 {
            let out = tx.poll(now, Duration::from_secs(60));
            if out.is_empty() {
                break;
            }
            for p in &out {
                match rx.on_data(&mut body(p)) {
                    Some((m, ack)) => {
                        got.extend(m);
                        tx.on_ack(&mut body(&ack)).unwrap();
                    }
                    None => refused = true,
                }
            }
            if refused {
                break;
            }
        }
        assert_eq!(got.len(), 1, "a message of exactly the cap arrives");
        assert_eq!(got[0].1.len(), MAX_MESSAGE);
        assert!(refused && rx.too_large(), "one byte more is refused");
    }
}
