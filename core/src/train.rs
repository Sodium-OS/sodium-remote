// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Probe trains: the capacity and loss of a route we don't send media on, so
//! routes can be compared by measured throughput and not RTT alone
//!
//! The sender puts `TRAIN_LEN` padded 1200-byte packets back to back on one
//! route. The bottleneck spreads them out; the receiver times the spread and
//! reports it, and `bytes after the first / (last − first arrival)` is the
//! bottleneck's rate. Train packets are kept out of congestion control.

use crate::wire::{Reader, Writer};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Packets per train: 12 × 1200 bytes spread over 2.1 ms at 50 Mbps, and
/// queue for 11.5 ms on a 10 Mbps link (trains to other routes often cross
/// the video's own access link).
pub const TRAIN_LEN: u16 = 12;
/// Datagram size: fits every path (BASE_MTU).
pub const TRAIN_DATAGRAM: usize = crate::mtu::BASE_MTU;
/// A train is reported this long after its last packet if the end got lost.
const TRAIN_WAIT: Duration = Duration::from_millis(50);
/// Below this spread the rate is beyond what the clock resolves.
const MIN_SPREAD_US: u64 = 100;

struct RxTrain {
    count: u16,
    received: u16,
    bytes_after_first: u64,
    first: Instant,
    last: Instant,
}

/// Receiver side.
#[derive(Default)]
pub(crate) struct TrainReceiver {
    trains: HashMap<u32, RxTrain>,
}

/// A finished train: (id, count, received, spread µs, bytes after the first).
pub(crate) type TrainResult = (u32, u16, u16, u32, u32);

impl TrainReceiver {
    /// One train packet of `size` bytes on the wire. Returns the result when
    /// the train's last packet arrives.
    pub fn on_packet(&mut self, now: Instant, id: u32, index: u16, count: u16, size: usize) -> Option<TrainResult> {
        let t = self.trains.entry(id).or_insert(RxTrain { count, received: 0, bytes_after_first: 0, first: now, last: now });
        if t.received > 0 {
            t.bytes_after_first += size as u64;
        }
        t.received += 1;
        t.last = now;
        if index + 1 >= count { self.finish(id) } else { None }
    }

    fn finish(&mut self, id: u32) -> Option<TrainResult> {
        let t = self.trains.remove(&id)?;
        let spread = t.last.saturating_duration_since(t.first).as_micros() as u32;
        Some((id, t.count, t.received, spread, t.bytes_after_first as u32))
    }

    /// Trains whose end never came.
    pub fn handle_timeout(&mut self, now: Instant) -> Vec<TrainResult> {
        let stale: Vec<u32> =
            self.trains.iter().filter(|(_, t)| now.saturating_duration_since(t.last) >= TRAIN_WAIT).map(|(&id, _)| id).collect();
        stale.into_iter().filter_map(|id| self.finish(id)).collect()
    }

    pub fn poll_timeout(&self) -> Option<Instant> {
        self.trains.values().map(|t| t.last + TRAIN_WAIT).min()
    }
}

pub(crate) fn encode_report(w: &mut Writer, r: TrainResult) {
    w.u32(r.0).u16(r.1).u16(r.2).u32(r.3).u32(r.4);
}

/// Rate (bps) and loss from a report (after the subtype byte).
pub(crate) fn decode_report(r: &mut Reader) -> Option<(u32, u64, f64)> {
    let (id, count, received, spread, bytes) = (r.u32()?, r.u16()?, r.u16()?, r.u32()?, r.u32()?);
    let rate = bytes as f64 * 8.0 / (spread.max(MIN_SPREAD_US as u32) as f64 / 1e6);
    let loss = 1.0 - received as f64 / count.max(1) as f64;
    Some((id, rate as u64, loss.max(0.0)))
}
