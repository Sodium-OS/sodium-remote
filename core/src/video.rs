// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The video channel: a packetizer with per-frame Reed-Solomon FEC
//! (`reed-solomon-simd`), the receiver's frame assembly, and NACK.
//!
//! A frame is cut into `k` equal data shards that fit the path MTU, and `m`
//! recovery shards are added (`m / k` is the FEC ratio, 0 to 30%). Any `k` of
//! the `k + m` packets rebuild the frame. Frames are handed to the decoder in
//! order; a frame that can't be rebuilt in time is reported lost so the host
//! can invalidate it as a reference.

use crate::wire::{Channel, Reader, Writer, seq32_newer};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Bytes of the video header inside the decrypted payload (channel byte included).
pub const VIDEO_HEADER_LEN: usize = 1 + 4 + 2 + 2 + 2 + 4 + 1;
pub const MAX_FEC_RATIO: f32 = 0.30;
const MAX_SHARDS: usize = 32_768;

pub mod flags {
    pub const KEYFRAME: u8 = 1;
    /// A retransmission answering a NACK.
    pub const RETRANSMIT: u8 = 2;
    /// The frame is predicted from an earlier frame named in its payload
    /// (`u32` after `send_us`): reference invalidation.
    pub const REFERENCE: u8 = 4;
    /// Decoding can restart here (a completed intra refresh).
    pub const RECOVERY_POINT: u8 = 8;
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoPacket {
    pub frame_id: u32,
    pub index: u16,
    pub k: u16,
    pub m: u16,
    pub frame_len: u32,
    pub flags: u8,
    pub shard: Vec<u8>,
}

impl VideoPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(VIDEO_HEADER_LEN + self.shard.len());
        w.u8(Channel::Video as u8)
            .u32(self.frame_id)
            .u16(self.index)
            .u16(self.k)
            .u16(self.m)
            .u32(self.frame_len)
            .u8(self.flags)
            .bytes(&self.shard);
        w.buf
    }

    /// Decode from the bytes after the channel byte.
    pub fn decode(r: &mut Reader) -> Option<VideoPacket> {
        let p = VideoPacket {
            frame_id: r.u32()?,
            index: r.u16()?,
            k: r.u16()?,
            m: r.u16()?,
            frame_len: r.u32()?,
            flags: r.u8()?,
            shard: r.rest().to_vec(),
        };
        if p.k == 0 || (p.index as usize) >= p.k as usize + p.m as usize || p.shard.is_empty() {
            return None;
        }
        Some(p)
    }
}

/// One padding packet of a congestion-control probe (docs/reference/protocol.md 3.5,
/// 3.8): the video header with `k = 0`, which no frame has, `frame_id` the
/// probe's id, `index` and `m` its packet and count, zeros to `len` bytes of
/// plaintext. A receiver counts its bytes (transport feedback covers it) and
/// drops it; a receiver from before padding drops it too, since its
/// `VideoPacket::decode` refuses `k = 0`.
pub fn padding(probe: u32, index: u16, count: u16, len: usize) -> Vec<u8> {
    let mut w = Writer::with_capacity(len.max(VIDEO_HEADER_LEN));
    w.u8(Channel::Video as u8).u32(probe).u16(index).u16(0).u16(count).u32(0).u8(0);
    w.buf.resize(len.max(VIDEO_HEADER_LEN), 0);
    w.buf
}

/// The probe a plaintext is padding of, if it is padding.
pub fn padding_probe(plain: &[u8]) -> Option<u32> {
    let is = plain.len() >= VIDEO_HEADER_LEN && plain[0] == Channel::Video as u8 && plain[7..9] == [0, 0];
    is.then(|| u32::from_be_bytes([plain[1], plain[2], plain[3], plain[4]]))
}

/// How many recovery shards to add to `k` data shards at a given ratio.
pub fn recovery_count(k: usize, ratio: f32) -> usize {
    let ratio = ratio.clamp(0.0, MAX_FEC_RATIO);
    if ratio <= 0.0 {
        return 0;
    }
    ((k as f32 * ratio).ceil() as usize).max(1)
}

/// Cut one encoded frame into data shards plus FEC.
/// `max_shard` is the largest shard that fits one datagram on this path.
pub fn packetize(frame_id: u32, frame: &[u8], flags: u8, max_shard: usize, fec_ratio: f32) -> Vec<VideoPacket> {
    assert!(!frame.is_empty() && max_shard >= 2);
    let max_shard = max_shard & !1; // reed-solomon-simd wants even shard sizes
    let k = frame.len().div_ceil(max_shard).clamp(1, MAX_SHARDS);
    let mut shard_len = frame.len().div_ceil(k);
    shard_len += shard_len & 1;
    let m = recovery_count(k, fec_ratio).min(MAX_SHARDS - k);
    let shards: Vec<Vec<u8>> = (0..k)
        .map(|i| {
            let start = (i * shard_len).min(frame.len());
            let end = ((i + 1) * shard_len).min(frame.len());
            let mut s = frame[start..end].to_vec();
            s.resize(shard_len, 0);
            s
        })
        .collect();
    let recovery = if m > 0 {
        reed_solomon_simd::encode(k, m, &shards).expect("rs encode")
    } else {
        Vec::new()
    };
    let head = |index: usize, shard: Vec<u8>| VideoPacket {
        frame_id,
        index: index as u16,
        k: k as u16,
        m: m as u16,
        frame_len: frame.len() as u32,
        flags,
        shard,
    };
    let mut out: Vec<VideoPacket> = shards.into_iter().enumerate().map(|(i, s)| head(i, s)).collect();
    out.extend(recovery.into_iter().enumerate().map(|(j, s)| head(k + j, s)));
    out
}

/// The sender keeps recent frames' packets to answer NACKs.
#[derive(Default)]
pub struct SendHistory {
    frames: BTreeMap<u32, (Instant, Vec<VideoPacket>)>,
}

impl SendHistory {
    const KEEP: Duration = Duration::from_millis(300);

    pub fn insert(&mut self, now: Instant, frame_id: u32, packets: Vec<VideoPacket>) {
        self.frames.insert(frame_id, (now, packets));
        while let Some((&id, (t, _))) = self.frames.iter().next() {
            if now.duration_since(*t) > Self::KEEP || self.frames.len() > 64 {
                self.frames.remove(&id);
            } else {
                break;
            }
        }
    }

    pub fn get(&self, frame_id: u32, index: u16) -> Option<VideoPacket> {
        let (_, packets) = self.frames.get(&frame_id)?;
        let mut p = packets.get(index as usize)?.clone();
        p.flags |= flags::RETRANSMIT;
        Some(p)
    }
}

/// A frame rebuilt by the receiver.
#[derive(Debug, Clone, PartialEq)]
pub struct AssembledFrame {
    pub frame_id: u32,
    pub data: Vec<u8>,
    pub flags: u8,
    /// Data shards that FEC rebuilt.
    pub recovered: usize,
    pub first_arrival: Instant,
    pub last_arrival: Instant,
}

struct Assembly {
    k: usize,
    m: usize,
    frame_len: usize,
    flags: u8,
    shards: Vec<Option<Vec<u8>>>,
    have: usize,
    first_arrival: Instant,
    last_arrival: Instant,
    nacks_sent: u8,
    next_nack: Option<Instant>,
    done: Option<AssembledFrame>,
}

impl Assembly {
    fn rebuild(&mut self, frame_id: u32) -> Option<AssembledFrame> {
        let missing: Vec<usize> = (0..self.k).filter(|&i| self.shards[i].is_none()).collect();
        if !missing.is_empty() {
            let original = (0..self.k).filter_map(|i| self.shards[i].as_ref().map(|s| (i, s)));
            let recovery = (self.k..self.k + self.m).filter_map(|i| self.shards[i].as_ref().map(|s| (i - self.k, s)));
            let restored = reed_solomon_simd::decode(self.k, self.m, original, recovery).ok()?;
            for (i, s) in restored {
                self.shards[i] = Some(s);
            }
        }
        let mut data = Vec::with_capacity(self.frame_len);
        for s in &self.shards[..self.k] {
            data.extend_from_slice(s.as_ref()?);
        }
        data.truncate(self.frame_len);
        Some(AssembledFrame {
            frame_id,
            data,
            flags: self.flags,
            recovered: missing.len(),
            first_arrival: self.first_arrival,
            last_arrival: self.last_arrival,
        })
    }
}

/// What the receiver has to say after handling packets or time.
#[derive(Debug, PartialEq)]
pub enum RxOutput {
    Frame(AssembledFrame),
    Lost(u32),
}

/// Receiver-side frame assembly, in-order delivery, loss detection and NACKs.
pub struct Depacketizer {
    frames: BTreeMap<u32, Assembly>,
    /// The next frame id to hand out.
    next: Option<u32>,
    out: Vec<RxOutput>,
    pub frame_interval: Duration,
    /// Set by the session: NACK only while RTT is under half a frame interval.
    pub rtt: Duration,
    pub nack_enabled: bool,
    pub packets: u64,
    pub recovered_shards: u64,
    pub frames_lost: u64,
}

impl Depacketizer {
    pub fn new(frame_interval: Duration) -> Self {
        Depacketizer {
            frames: BTreeMap::new(),
            next: None,
            out: Vec::new(),
            frame_interval,
            rtt: Duration::from_millis(100),
            nack_enabled: false,
            packets: 0,
            recovered_shards: 0,
            frames_lost: 0,
        }
    }

    /// How long an incomplete frame waits after its last packet before it's lost.
    fn give_up_after(&self) -> Duration {
        let base = self.frame_interval * 2;
        if self.nack_enabled { base.max(self.rtt * 3) } else { base }
    }

    pub fn on_packet(&mut self, now: Instant, p: VideoPacket) {
        self.packets += 1;
        if let Some(next) = self.next {
            if p.frame_id != next && !seq32_newer(p.frame_id, next) {
                return; // already delivered or given up
            }
        } else {
            self.next = Some(p.frame_id);
        }
        let frame_id = p.frame_id;
        let a = self.frames.entry(frame_id).or_insert_with(|| Assembly {
            k: p.k as usize,
            m: p.m as usize,
            frame_len: p.frame_len as usize,
            flags: p.flags & !flags::RETRANSMIT,
            shards: vec![None; p.k as usize + p.m as usize],
            have: 0,
            first_arrival: now,
            last_arrival: now,
            nacks_sent: 0,
            next_nack: None,
            done: None,
        });
        if a.done.is_some() || p.k as usize != a.k || p.m as usize != a.m {
            return;
        }
        let i = p.index as usize;
        if a.shards[i].is_none() {
            a.shards[i] = Some(p.shard);
            a.have += 1;
            a.last_arrival = now;
        }
        if a.have >= a.k {
            if let Some(f) = a.rebuild(frame_id) {
                self.recovered_shards += f.recovered as u64;
                a.done = Some(f);
            }
        }
        self.drain(now);
    }

    /// Hand out frames in order; give up on frames that can no longer arrive.
    fn drain(&mut self, now: Instant) {
        let give_up = self.give_up_after();
        loop {
            let Some(next) = self.next else { return };
            let newer_done = self
                .frames
                .iter()
                .any(|(&id, a)| seq32_newer(id, next) && a.done.is_some());
            match self.frames.get_mut(&next) {
                Some(a) if a.done.is_some() => {
                    let f = a.done.take().unwrap();
                    self.frames.remove(&next);
                    self.out.push(RxOutput::Frame(f));
                    self.next = Some(next.wrapping_add(1));
                }
                Some(a) => {
                    // Incomplete. Without NACK a later complete frame means this one
                    // won't finish; with NACK, wait out the retransmissions.
                    let stale = now.duration_since(a.last_arrival) >= give_up;
                    let nack_pending = self.nack_enabled && a.nacks_sent < 2;
                    if stale || (newer_done && !nack_pending) {
                        self.frames.remove(&next);
                        self.frames_lost += 1;
                        self.out.push(RxOutput::Lost(next));
                        self.next = Some(next.wrapping_add(1));
                    } else {
                        return;
                    }
                }
                None => {
                    // Not one packet of this frame arrived.
                    if newer_done || self.frames.keys().any(|&id| {
                        seq32_newer(id, next) && now.duration_since(self.frames[&id].first_arrival) >= give_up
                    }) {
                        self.frames_lost += 1;
                        self.out.push(RxOutput::Lost(next));
                        self.next = Some(next.wrapping_add(1));
                    } else {
                        return;
                    }
                }
            }
        }
    }

    pub fn handle_timeout(&mut self, now: Instant) {
        self.drain(now);
    }

    pub fn poll_output(&mut self) -> Option<RxOutput> {
        if self.out.is_empty() { None } else { Some(self.out.remove(0)) }
    }

    /// NACKs to send now: (frame, missing data-shard indexes), at most as many
    /// as the frame still needs. A frame is NACKed once a gap is visible (a later
    /// frame started) or its packets stopped for 3 ms, and again after 1.5 RTT.
    pub fn poll_nacks(&mut self, now: Instant) -> Vec<(u32, Vec<u16>)> {
        if !self.nack_enabled {
            return Vec::new();
        }
        let newest = self.frames.keys().copied().max_by(|a, b| {
            if seq32_newer(*a, *b) { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less }
        });
        let rtt = self.rtt;
        let mut out = Vec::new();
        for (&id, a) in self.frames.iter_mut() {
            if a.done.is_some() || a.nacks_sent >= 2 {
                continue;
            }
            let gap_seen = newest.is_some_and(|n| seq32_newer(n, id))
                || now.duration_since(a.last_arrival) >= Duration::from_millis(3);
            let due = match a.next_nack {
                None => gap_seen,
                Some(t) => now >= t,
            };
            if !due {
                continue;
            }
            let need = a.k - a.have;
            let missing: Vec<u16> = (0..a.k)
                .filter(|&i| a.shards[i].is_none())
                .take(need)
                .map(|i| i as u16)
                .collect();
            a.nacks_sent += 1;
            a.next_nack = Some(now + rtt.mul_f32(1.5).max(Duration::from_millis(2)));
            out.push((id, missing));
        }
        out
    }

    pub fn poll_timeout(&self) -> Option<Instant> {
        let give_up = self.give_up_after();
        self.frames
            .values()
            .filter(|a| a.done.is_none())
            .flat_map(|a| {
                let mut t = vec![a.last_arrival + give_up];
                if self.nack_enabled && a.nacks_sent < 2 {
                    t.push(a.next_nack.unwrap_or(a.last_arrival + Duration::from_millis(3)));
                }
                t
            })
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 % 251) as u8).collect()
    }

    #[test]
    fn fec_rebuilds_a_frame_with_lost_packets() {
        let data = frame(50_000);
        let packets = packetize(7, &data, flags::KEYFRAME, 1150, 0.10);
        let k = packets[0].k as usize;
        let m = packets[0].m as usize;
        assert_eq!(k, 44);
        assert_eq!(m, 5);
        let now = Instant::now();
        let mut rx = Depacketizer::new(Duration::from_millis(16));
        // Lose m data packets: still decodable.
        for p in packets.into_iter().filter(|p| !(3..3 + m as u16).contains(&p.index)) {
            rx.on_packet(now, VideoPacket::decode(&mut Reader::new(&p.encode()[1..])).unwrap());
        }
        match rx.poll_output() {
            Some(RxOutput::Frame(f)) => {
                assert_eq!(f.data, data);
                assert_eq!(f.recovered, m);
                assert_eq!(f.flags, flags::KEYFRAME);
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn one_loss_too_many_reports_the_frame_lost_when_the_next_completes() {
        let now = Instant::now();
        let mut rx = Depacketizer::new(Duration::from_millis(16));
        let a = packetize(1, &frame(10_000), 0, 1150, 0.0);
        for p in a.into_iter().skip(1) {
            rx.on_packet(now, p);
        }
        assert_eq!(rx.poll_output(), None, "frame 1 is still waiting");
        for p in packetize(2, &frame(3000), 0, 1150, 0.0) {
            rx.on_packet(now, p);
        }
        assert_eq!(rx.poll_output(), Some(RxOutput::Lost(1)));
        assert!(matches!(rx.poll_output(), Some(RxOutput::Frame(f)) if f.frame_id == 2));
    }

    #[test]
    fn nack_asks_for_only_what_is_missing_and_retransmit_completes() {
        let now = Instant::now();
        let mut rx = Depacketizer::new(Duration::from_millis(16));
        rx.nack_enabled = true;
        rx.rtt = Duration::from_millis(2);
        let mut history = SendHistory::default();
        let packets = packetize(9, &frame(20_000), 0, 1150, 0.0);
        history.insert(now, 9, packets.clone());
        for p in packets.iter().filter(|p| p.index != 4) {
            rx.on_packet(now, p.clone());
        }
        let later = now + Duration::from_millis(4);
        let nacks = rx.poll_nacks(later);
        assert_eq!(nacks, vec![(9, vec![4])]);
        rx.on_packet(later, history.get(9, 4).unwrap());
        assert!(matches!(rx.poll_output(), Some(RxOutput::Frame(f)) if f.frame_id == 9));
    }
}
