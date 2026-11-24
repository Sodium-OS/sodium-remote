// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The per-frame rate rules. The session's core computes the budget
//! (remote/docs/reference/protocol.md §8.1: `Session::video_budget` and `link_state`); this
//! turns it into encoder settings for one frame of a given class.
//!
//! - **Utilisation by content** (§7.1): core caps latency-critical frames at
//!   0.65 × C_est. media also knows about animations and window moves from the
//!   plugin's hints before core does; such a frame gets the same cap here.
//! - **Refinement** fills what is left of the frame interval
//!   (`interval_remaining_bytes`), behind the interactive frame. It never goes above
//!   the controller's target: a refinement frame's rate is `min(encoder_bps, what is
//!   left of the interval)`, and `encoder_bps` is core's target or lower (§7.1).
//! - **Thin hedge** (§7.2): core's `encoder_bps` is already the thin rate.
//! - **Split by link state and frame class**:
//!   - on a measured bottleneck, during start-up, and for every refinement or
//!     passive frame: no split, with a one-frame VBV (a refinement pass fills its
//!     interval's budget);
//!   - latency-critical frames after about 3 s with no standing queue: forced 2-way
//!     split with a half-frame VBV;
//!   - never split with a one-frame VBV, never while hedging
//!
//!    NVENC changes the split by reconfigure with
//!   no reset and no IDR (measured), so it flips at the next frame both ways.

/// Core's word on the next frame: `VideoBudget` and `LinkState` as plain data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RateInput {
    /// `VideoBudget::encoder_bps`: the per-frame encoder rate, FEC excluded, already
    /// capped (0.65 × C_est) or thinned by core.
    pub encoder_bps: u64,
    /// `VideoBudget::latency_critical`: input in the last second or an animation.
    pub latency_critical: bool,
    /// `VideoBudget::thin`: failover hedging.
    pub thin: bool,
    /// `VideoBudget::interval_remaining_bytes` as core hands it over: the fresh interval's
    /// budget. media takes off the frames it already sent in the interval before it
    /// sizes a refinement frame.
    pub interval_remaining_bytes: usize,
    /// `LinkState::c_est_bps`: the bottleneck, when measured.
    pub c_est_bps: Option<u64>,
    /// `LinkState::split_ok`.
    pub split_ok: bool,
    /// The user set a bandwidth cap (the client's `bandwidth_limit_kbps` > 0): refinement
    /// stops at the luma bar instead of building to lossless, to save data.
    pub user_cap: bool,
    /// The session's route demand, as the host sends it to core (0.2 bit per
    /// pixel per frame, or the user's cap if lower); 0: not known. Refinement is ordered
    /// under 0.6 × it.
    pub demand_bps: u64,
}

impl RateInput {
    /// A fixed rate with no link knowledge (tests, start-up).
    pub fn fixed(bps: u64) -> Self {
        RateInput { encoder_bps: bps, interval_remaining_bytes: usize::MAX, ..Default::default() }
    }
}

/// Why a frame exists, which decides its share of the rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameClass {
    /// Latency-critical: input in the last second, or a compositor animation in flight.
    Interactive,
    /// Changed content nobody is waiting on (a clock, a progress bar, a video).
    Passive,
    /// Build-to-lossless refinement of content that stopped changing.
    Refine,
}

impl FrameClass {
    pub fn wire(self) -> u8 {
        match self {
            FrameClass::Passive => 0,
            FrameClass::Interactive => 1,
            FrameClass::Refine => 2,
        }
    }
}

/// The encoder settings for one frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RatePlan {
    pub kbps: u32,
    /// VBV size in percent of one frame's budget.
    pub vbv_pct: u32,
    /// Forced 2-way split-frame encoding.
    pub split: bool,
}

/// Latency-critical frames' share of C_est on a bottleneck (protocol.md §7.1).
pub const CRITICAL_SHARE: f64 = 0.65;
/// A refinement frame needs at least this much of the interval, or it waits.
pub const MIN_REFINE_BYTES: usize = 2_000;

/// The settings for a frame of `class`; None when a refinement frame should wait
/// for the next interval.
pub fn plan(input: &RateInput, class: FrameClass, fps: u32) -> Option<RatePlan> {
    let mut bps = input.encoder_bps;
    match class {
        FrameClass::Interactive if !input.latency_critical && !input.thin => {
            if let Some(c) = input.c_est_bps {
                bps = bps.min((c as f64 * CRITICAL_SHARE) as u64);
            }
        }
        FrameClass::Refine => {
            if input.interval_remaining_bytes < MIN_REFINE_BYTES {
                return None;
            }
            let rest = (input.interval_remaining_bytes as u64).saturating_mul(8 * fps.max(1) as u64);
            bps = bps.min(rest);
        }
        _ => {}
    }
    let split = input.split_ok && !input.thin && class == FrameClass::Interactive;
    Some(RatePlan { kbps: (bps / 1000).max(100) as u32, vbv_pct: if split { 50 } else { 100 }, split })
}

/// The last capture frame, for the QP ceiling of the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Steady {
    /// The rate controller's base QP (codec units), as `Grid::commit` worked it out.
    pub base_qp: i32,
    pub bytes: usize,
    pub changed: u32,
    pub t_encoded_ns: u64,
}

/// How far the rate controller's QP may rise from one frame of continuous motion to the next
/// (codec units of HEVC; AV1's are 4 to 1).
pub const STEADY_STEP: i32 = 2;

/// The rate controller's QP ceiling for a capture frame of `changed` cells (0: none); `held`:
/// the last frame had one.
///
/// NVENC's one-pass rate controller lifts the QP of a frame in continuous motion (a scroll, a
/// video) by 6 to 17 for a frame or a few, then drops it back, though the frames use a third of
/// their budget: lines of text alternate sharp and soft at the frame rate. A larger VBV doesn't
/// stop it (measured: 13 % of frames at 3 frames' VBV, 6 % at 10). So while the motion goes on
/// the QP may rise only `STEADY_STEP` a frame:
/// - the last capture frame was encoded within two intervals (`interval_ns`), and this one
///   changes at most half as many cells again (not a new page after a small change);
/// - the ceiling starts once a frame takes at most half a frame's budget (`plan`'s rate at
///   `fps`), so it never starts on a link the stream fills (a cap, a bottleneck), where the rate
///   controller holds the QP steady itself;
/// - it holds while the frames still fit their VBV, which bounds what a frame coded finer than
///   the rate controller wanted can cost. Ending where it starts would switch it on and off
///   around the threshold, each switch a swing (measured: 49 % of frames).
#[allow(clippy::too_many_arguments)]
pub fn max_qp(last: Option<&Steady>, held: bool, changed: u32, now_ns: u64, interval_ns: u64, plan: &RatePlan, fps: u32, scale: i32) -> u32 {
    let Some(l) = last else { return 0 };
    let budget = plan.kbps as u64 * 1000 / 8 / fps.max(1) as u64;
    let room = if held { budget * plan.vbv_pct as u64 / 100 } else { budget / 2 };
    let steady = now_ns.saturating_sub(l.t_encoded_ns) <= 2 * interval_ns
        && 2 * changed as u64 <= 3 * l.changed as u64
        && l.bytes as u64 <= room;
    if steady { (l.base_qp + STEADY_STEP * scale).max(1) as u32 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_caps_an_interactive_frame_core_did_not_know_about() {
        let r = RateInput {
            encoder_bps: 9_000_000,
            interval_remaining_bytes: 18_750,
            c_est_bps: Some(10_000_000),
            ..Default::default()
        };
        assert_eq!(plan(&r, FrameClass::Interactive, 60), Some(RatePlan { kbps: 6500, vbv_pct: 100, split: false }));
        assert_eq!(plan(&r, FrameClass::Passive, 60).unwrap().kbps, 9000);
        // core already capped it: no second cap
        let core_capped = RateInput { encoder_bps: 6_500_000, latency_critical: true, ..r.clone() };
        assert_eq!(plan(&core_capped, FrameClass::Interactive, 60).unwrap().kbps, 6500);
    }

    #[test]
    fn refinement_fills_the_rest_of_the_interval_or_waits() {
        let mut r = RateInput { encoder_bps: 50_000_000, interval_remaining_bytes: 50_000, split_ok: true, ..Default::default() };
        assert_eq!(plan(&r, FrameClass::Refine, 60), Some(RatePlan { kbps: 24_000, vbv_pct: 100, split: false }));
        r.interval_remaining_bytes = 1_000;
        assert_eq!(plan(&r, FrameClass::Refine, 60), None);
    }

    #[test]
    fn split_is_for_interactive_frames_and_never_while_hedging() {
        let mut r = RateInput { encoder_bps: 4_000_000, split_ok: true, thin: true, ..RateInput::fixed(0) };
        assert_eq!(plan(&r, FrameClass::Interactive, 60), Some(RatePlan { kbps: 4000, vbv_pct: 100, split: false }));
        r.thin = false;
        assert_eq!(plan(&r, FrameClass::Interactive, 60), Some(RatePlan { kbps: 4000, vbv_pct: 50, split: true }));
        r.interval_remaining_bytes = 50_000;
        assert_eq!(plan(&r, FrameClass::Refine, 60), Some(RatePlan { kbps: 4000, vbv_pct: 100, split: false }));
        assert_eq!(plan(&r, FrameClass::Passive, 60), Some(RatePlan { kbps: 4000, vbv_pct: 100, split: false }));
    }

    #[test]
    fn continuous_motion_inside_its_budget_caps_the_qp_rise() {
        let p = RatePlan { kbps: 90_000, vbv_pct: 100, split: false }; // 187.5 kB a frame
        let (ms, iv) = (1_000_000, 16_667_000);
        let last = Steady { base_qp: 5, bytes: 60_000, changed: 4000, t_encoded_ns: 100 * ms };
        let now = 117 * ms;
        assert_eq!(max_qp(Some(&last), false, 4100, now, iv, &p, 60, 1), 7);
        assert_eq!(max_qp(Some(&last), false, 4100, now, iv, &p, 60, 4), 13); // AV1
        // no last frame, motion stopped, or a much larger change: the rate controller's own QP
        assert_eq!(max_qp(None, false, 4100, now, iv, &p, 60, 1), 0);
        assert_eq!(max_qp(Some(&last), true, 4100, 140 * ms, iv, &p, 60, 1), 0);
        assert_eq!(max_qp(Some(&Steady { changed: 40, ..last }), true, 4100, now, iv, &p, 60, 1), 0);
        // starts under half the budget, holds while the frame fits its VBV (half a frame's when split)
        let big = Steady { bytes: 120_000, ..last };
        assert_eq!(max_qp(Some(&big), false, 4100, now, iv, &p, 60, 1), 0);
        assert_eq!(max_qp(Some(&big), true, 4100, now, iv, &p, 60, 1), 7);
        assert_eq!(max_qp(Some(&Steady { bytes: 190_000, ..last }), true, 4100, now, iv, &p, 60, 1), 0);
        let split = RatePlan { vbv_pct: 50, split: true, ..p };
        assert_eq!(max_qp(Some(&big), true, 4100, now, iv, &split, 60, 1), 0);
    }
}
