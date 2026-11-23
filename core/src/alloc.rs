// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Utilisation by content: the allocator's view of the
//! next frame, and the link state the encoder's split rule reads.
//!
//! The congestion controller stays at about 90% of a bottleneck. On a link
//! measured as the bottleneck (the controller holds `C_est`), a latency-critical
//! frame is capped at 0.65 × C_est × frame interval, so it crosses the
//! bottleneck in about two thirds of an interval. Refinement and passive frames
//! fill the rest of the interval up to the target, behind it. Off a measured
//! bottleneck nothing changes.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Share of C_est a latency-critical frame may use per frame interval.
pub const CRITICAL_SHARE: f64 = 0.65;
/// A frame is latency-critical when input arrived within this long.
pub const INPUT_WINDOW: Duration = Duration::from_secs(1);
/// The split rule's "no standing queue for about 3 s".
pub const QUEUE_FREE_FOR_SPLIT: Duration = Duration::from_secs(3);

/// What the next video frame may use. Read before every frame.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct VideoBudget {
    /// Input in the last second or an animation in flight.
    pub latency_critical: bool,
    /// The 0.65 × C_est cap applies to this frame (latency-critical on a
    /// measured bottleneck, and not hedging).
    pub capped: bool,
    /// While failover hedging thins the stream.
    pub thin: bool,
    /// The encoder bitrate for this frame, FEC excluded: the per-frame
    /// reconfigure value. One frame interval of it is `frame_bytes`.
    pub encoder_bps: u64,
    /// The most this frame should take, FEC excluded.
    pub frame_bytes: usize,
    /// What the whole stream may still send in this frame interval, FEC
    /// excluded (target × interval less the frames already sent in it).
    /// Refinement fills this behind the interactive frame.
    pub interval_remaining_bytes: usize,
}

/// Link state for the encoder (split by link state).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LinkState {
    /// Bottleneck measured: the controller holds C_est (bps).
    pub c_est_bps: Option<u64>,
    /// How long the standing queue has stayed under 2 ms (0 at start-up).
    pub queue_free_ms: u64,
    /// `queue_free_ms` reached 3 s.
    pub queue_free_3s: bool,
    /// the rule in one bit: split with a half-frame VBV only off a
    /// measured bottleneck after 3 s with no standing queue. Tighten at once
    /// (this turns false immediately), loosen slowly (the host flips at a
    /// still moment).
    pub split_ok: bool,
}

/// Per-frame options for `Session::send_video_frame_with`.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct FrameOpts {
    /// The frame is predicted from this earlier frame instead of the one before
    /// it (reference invalidation after a loss or at a failover).
    pub reference: Option<u32>,
    /// Decoding can start here without earlier frames (an intra refresh that
    /// completed). Keyframes are recovery points anyway.
    pub recovery_point: bool,
}

/// Which frame the encoder should predict the next frame from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefHint {
    /// Invalidate every frame after this one and predict from it
    /// (`nvEncInvalidateRefFrames`; intra refresh if it has left the DPB).
    /// Send the frame with `FrameOpts { reference: Some(id) }`.
    Reference(u32),
    /// The client has decoded nothing we can reference: a keyframe (or intra refresh).
    Keyframe,
}

/// Host-side content signals and the frames sent in the current interval.
#[derive(Default)]
pub(crate) struct Content {
    pub last_input: Option<Instant>,
    pub animation: bool,
    /// (sent at, frame bytes before FEC) for frames of the current interval.
    pub recent: VecDeque<(Instant, usize)>,
}

impl Content {
    pub fn latency_critical(&self, now: Instant) -> bool {
        self.animation || self.last_input.is_some_and(|t| now.saturating_duration_since(t) < INPUT_WINDOW)
    }

    /// Bytes of frames sent in this frame interval. A frame counts for half an
    /// interval: a refinement frame right behind the interactive one sees it,
    /// the next interval's frame doesn't, even with a jittery frame clock.
    pub fn sent_this_interval(&self, now: Instant, interval: Duration) -> usize {
        let window = interval / 2;
        self.recent.iter().filter(|(t, _)| now.saturating_duration_since(*t) < window).map(|r| r.1).sum()
    }

    pub fn record_frame(&mut self, now: Instant, interval: Duration, bytes: usize) {
        while self.recent.front().is_some_and(|(t, _)| now.saturating_duration_since(*t) >= interval) {
            self.recent.pop_front();
        }
        self.recent.push_back((now, bytes));
    }
}
