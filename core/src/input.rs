// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Input redundancy: every input packet repeats each event the host hasn't
//! acknowledged yet, so a lost packet costs nothing and nothing ever waits for
//! a retransmit. Events are opaque bytes here; their
//! encoding (keys as HID usages, pointer, scroll phases, IME text) belongs to
//! the input module of the host and client.

use crate::wire::{Channel, Reader, Writer, seq32_newer};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub mod sub {
    pub const EVENTS: u8 = 0;
    pub const ACK: u8 = 1;
}

/// Keep repeating unacknowledged events at this pace when no new input comes.
const MIN_RESEND: Duration = Duration::from_millis(10);

#[derive(Default)]
pub struct InputSender {
    next_seq: u32,
    unacked: VecDeque<(u32, Vec<u8>)>,
    next_resend: Option<Instant>,
}

impl InputSender {
    /// Queue an event; returns the packets to send now.
    pub fn push(&mut self, now: Instant, event: Vec<u8>, max_payload: usize, rtt: Duration) -> Vec<Vec<u8>> {
        self.unacked.push_back((self.next_seq, event));
        self.next_seq = self.next_seq.wrapping_add(1);
        self.packets(now, max_payload, rtt)
    }

    fn packets(&mut self, now: Instant, max_payload: usize, rtt: Duration) -> Vec<Vec<u8>> {
        self.next_resend = if self.unacked.is_empty() { None } else { Some(now + rtt.mul_f32(1.25).max(MIN_RESEND)) };
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.unacked.len() {
            let first = self.unacked[i].0;
            let mut w = Writer::with_capacity(max_payload);
            w.u8(Channel::Input as u8).u8(sub::EVENTS).u32(first).u16(0);
            let mut n = 0u16;
            while i < self.unacked.len() && (n == 0 || w.len() + 2 + self.unacked[i].1.len() <= max_payload) {
                w.lp_bytes(&self.unacked[i].1);
                n += 1;
                i += 1;
            }
            w.buf[6..8].copy_from_slice(&n.to_be_bytes());
            out.push(w.buf);
        }
        out
    }

    pub fn on_ack(&mut self, seq: u32) {
        while self.unacked.front().is_some_and(|(s, _)| !seq32_newer(*s, seq)) {
            self.unacked.pop_front();
        }
        if self.unacked.is_empty() {
            self.next_resend = None;
        }
    }

    pub fn poll_timeout(&self) -> Option<Instant> {
        self.next_resend
    }

    /// Repeat unacknowledged events when the resend timer fires.
    pub fn handle_timeout(&mut self, now: Instant, max_payload: usize, rtt: Duration) -> Vec<Vec<u8>> {
        if self.next_resend.is_some_and(|t| now >= t) { self.packets(now, max_payload, rtt) } else { Vec::new() }
    }

    pub fn unacked(&self) -> usize {
        self.unacked.len()
    }
}

#[derive(Default)]
pub struct InputReceiver {
    last: Option<u32>,
}

impl InputReceiver {
    /// Handle an EVENTS message (after the subtype byte). Returns the new
    /// events in order and the ACK message to send back.
    pub fn on_events(&mut self, r: &mut Reader) -> Option<(Vec<Vec<u8>>, Vec<u8>)> {
        let first = r.u32()?;
        let n = r.u16()?;
        let mut fresh = Vec::new();
        for i in 0..n as u32 {
            let ev = r.lp_bytes()?;
            let seq = first.wrapping_add(i);
            let new = match self.last {
                None => true,
                Some(l) => seq32_newer(seq, l),
            };
            if new {
                fresh.push(ev.to_vec());
                self.last = Some(seq);
            }
        }
        let mut w = Writer::with_capacity(6);
        w.u8(Channel::Input as u8).u8(sub::ACK).u32(self.last?);
        Some((fresh, w.buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(p: &[u8]) -> Reader<'_> {
        let mut r = Reader::new(p);
        r.u8();
        assert_eq!(r.u8(), Some(sub::EVENTS));
        r
    }

    #[test]
    fn a_lost_packet_is_covered_by_the_next_one() {
        let now = Instant::now();
        let rtt = Duration::from_millis(10);
        let mut tx = InputSender::default();
        let mut rx = InputReceiver::default();
        let _lost = tx.push(now, b"down".to_vec(), 1100, rtt);
        let p2 = tx.push(now, b"up".to_vec(), 1100, rtt);
        assert_eq!(p2.len(), 1);
        let (fresh, ack) = rx.on_events(&mut events(&p2[0])).unwrap();
        assert_eq!(fresh, vec![b"down".to_vec(), b"up".to_vec()]);
        // A duplicate delivers nothing new.
        assert!(rx.on_events(&mut events(&p2[0])).unwrap().0.is_empty());
        let mut r = Reader::new(&ack);
        r.u8();
        r.u8();
        tx.on_ack(r.u32().unwrap());
        assert_eq!(tx.unacked(), 0);
    }
}
