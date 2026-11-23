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
}
