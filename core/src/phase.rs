// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Phase lock: REMOTE-1's frame clock is moved so each
//! frame is ready on the client a small margin before the vsync it is shown at,
//! which removes the client's wait for vsync (up to a frame, and jittery).
//!
//! - **Client** ([`LeadStats`]): for every frame shown, how long before its
//!   vsync it was ready to show, modulo the refresh period. About once a second
//!   it becomes a [`VsyncReport`] (`Msg::Vsync`).
//! - **Host** ([`PhaseLock`]): turns reports into FRAME_CLOCK settings
//!   (capture-ipc 5): an absolute phase on the host's `CLOCK_MONOTONIC`, the
//!   refresh, and `grace_ns`. The loop is closed on the measured lead, so it
//!   needs no model of the pipeline's latency; the first report seeds it from
//!   the client's vsync mapped onto the host clock.

use crate::msg::VsyncReport;

/// What the median frame should have left before its vsync, beyond the
/// measured jitter ("about 1–2 ms (plus measured jitter)").
pub const MARGIN_NS: i64 = 1_500_000;
/// Left to the 5th-percentile frame before `grace_ns` may use the rest.
pub const GRACE_SAFETY_NS: i64 = 500_000;
/// Corrections smaller than this are left alone (the plugin holds its phase
/// within about 0.3 ms anyway, M0).
const DEADBAND_NS: i64 = 300_000;
/// A report needs at least this many frames to move the clock.
const MIN_FRAMES: u32 = 10;
/// Before the first report: render-done → ready on the client (encode,
/// network, decode), a guess the loop corrects.
const PIPELINE_GUESS_NS: i64 = 6_000_000;

/// Client side: the lead of each frame shown.
#[derive(Default)]
pub struct LeadStats {
    leads_us: Vec<i64>,
    period_ns: u32,
    last_vsync_us: u64,
}

impl LeadStats {
    /// A frame ready at `ready_us` was shown at the vsync `presented_us` (both
    /// on the session clock, µs) of a display with this refresh period.
    pub fn add(&mut self, ready_us: i64, presented_us: i64, period_ns: u32) {
        if period_ns == 0 || presented_us < 0 {
            return;
        }
        let p = (period_ns as i64 / 1000).max(1);
        self.leads_us.push((presented_us - ready_us).rem_euclid(p));
        self.period_ns = period_ns;
        self.last_vsync_us = self.last_vsync_us.max(presented_us as u64);
    }

    /// The report for the frames since the last one. None until enough frames
    /// were shown (they keep adding up: a still desktop shows few).
    pub fn report(&mut self) -> Option<VsyncReport> {
        if self.leads_us.len() < MIN_FRAMES as usize {
            return None;
        }
        let p = (self.period_ns as i64 / 1000).max(1);
        let mut v = unwrap_around_mean(std::mem::take(&mut self.leads_us), p);
        v.sort_unstable();
        let pick = |q: f64| v[((q * (v.len() - 1) as f64).round() as usize).min(v.len() - 1)];
        let (p50, p05) = (pick(0.5), pick(0.05));
        // The median goes back into [0, period); the 5th percentile keeps its
        // distance below it (negative: those frames missed their vsync).
        let shift = p50.rem_euclid(p) - p50;
        Some(VsyncReport {
            vsync_us: self.last_vsync_us,
            period_ns: self.period_ns,
            lead_p50_us: (p50 + shift) as i32,
            lead_p05_us: (p05 + shift) as i32,
            frames: v.len() as u32,
        })
    }
}

/// Values on a circle of circumference `p`, unwrapped to lie within half a
/// period of their circular mean.
fn unwrap_around_mean(v: Vec<i64>, p: i64) -> Vec<i64> {
    let tau = std::f64::consts::TAU;
    let (s, c) = v.iter().fold((0.0, 0.0), |(s, c), &x| {
        let a = x as f64 / p as f64 * tau;
        (s + a.sin(), c + a.cos())
    });
    let mean = (s.atan2(c) / tau * p as f64).rem_euclid(p as f64) as i64;
    v.into_iter()
        .map(|x| {
            let d = (x - mean).rem_euclid(p);
            mean + if d >= p / 2 { d - p } else { d }
        })
        .collect()
}

/// FRAME_CLOCK's fields (capture-ipc 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameClock {
    pub phase_offset_ns: i64,
    pub refresh_mhz: u32,
    pub grace_ns: u32,
}

/// Host side: the controller behind FRAME_CLOCK.
pub struct PhaseLock {
    /// The stream's frame period (REMOTE-1's refresh).
    period_ns: i64,
    phase_ns: Option<i64>,
    grace_ns: u32,
}

impl PhaseLock {
    pub fn new(stream_refresh_mhz: u32) -> Self {
        PhaseLock { period_ns: period_of(stream_refresh_mhz), phase_ns: None, grace_ns: 0 }
    }

    /// The stream's rate changed (a new mode): start over.
    pub fn reset(&mut self, stream_refresh_mhz: u32) {
        *self = PhaseLock::new(stream_refresh_mhz);
    }

    /// The current setting, if any.
    pub fn current(&self) -> Option<FrameClock> {
        Some(FrameClock { phase_offset_ns: self.phase_ns?, refresh_mhz: self.refresh_mhz(), grace_ns: self.grace_ns })
    }

    fn refresh_mhz(&self) -> u32 {
        (1e12 / self.period_ns as f64).round() as u32
    }

    /// A client report. `vsync_host_ns` is its `vsync_us` mapped onto the
    /// host's CLOCK_MONOTONIC (None before clock sync). Returns a new setting
    /// when the clock should move.
    pub fn on_report(&mut self, r: &VsyncReport, vsync_host_ns: Option<i64>) -> Option<FrameClock> {
        if r.period_ns == 0 {
            return None;
        }
        // The client's own period, when it is the stream's (no drift between
        // the two clocks); a stream above the display's rate keeps its own.
        let client = r.period_ns as i64;
        if (client - self.period_ns).abs() * 200 < self.period_ns {
            self.period_ns = client;
        }
        let p = self.period_ns;
        let Some(phase) = self.phase_ns else {
            // Seed: frames done a pipeline's guess plus the margin before the
            // client's vsync.
            let v = vsync_host_ns?;
            self.phase_ns = Some((v - PIPELINE_GUESS_NS - MARGIN_NS).rem_euclid(p));
            return self.current();
        };
        if r.frames < MIN_FRAMES {
            return None;
        }
        let lead_p50 = r.lead_p50_us as i64 * 1000 % p;
        let lead_p05 = r.lead_p05_us as i64 * 1000;
        let jitter = (r.lead_p50_us as i64 - r.lead_p05_us as i64).max(0) * 1000;
        let target = (MARGIN_NS + jitter).min(p / 2);
        // Lead above the target: frames are early, finish them later.
        let mut err = lead_p50 - target;
        if err > p / 2 {
            err -= p;
        }
        let grace = if err.abs() < 1_000_000 { (lead_p05 - GRACE_SAFETY_NS).clamp(0, p / 4) as u32 } else { 0 };
        if err.abs() < DEADBAND_NS && grace.abs_diff(self.grace_ns) < 250_000 {
            return None;
        }
        if err.abs() >= DEADBAND_NS {
            self.phase_ns = Some((phase + err * 4 / 5).rem_euclid(p));
        }
        self.grace_ns = grace;
        self.current()
    }
}

fn period_of(refresh_mhz: u32) -> i64 {
    (1e12 / refresh_mhz.max(1000) as f64).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leads_wrap_around_the_vsync() {
        let mut s = LeadStats::default();
        // Ready 0.2 ms before or 0.2 ms after a vsync (shown a period later).
        for i in 0..20 {
            let vsync = 16_667 * (i + 2);
            let ready = if i % 2 == 0 { vsync - 200 } else { vsync - 16_667 + 200 };
            s.add(ready, vsync, 16_666_667);
        }
        let r = s.report().unwrap();
        assert!(r.lead_p05_us < 0 && r.lead_p05_us > -400, "{r:?}");
        assert_eq!(r.frames, 20);
    }

    /// A simulated pipeline: frames finish on the host's grid, are ready on the
    /// client `latency` later (plus jitter), and are shown at the next vsync.
    #[test]
    fn the_host_clock_converges_on_the_margin() {
        let period = 16_666_667i64;
        let vsync0 = 5_000_000i64; // the client's grid, on the host clock
        let latency = 9_300_000i64;
        let mut lock = PhaseLock::new(60_000);
        let rep = |phase: i64| {
            let mut s = LeadStats::default();
            for i in 0..60 {
                let done = phase + i * period;
                let ready = done + latency + (i % 5 - 2) * 100_000;
                let vsync = vsync0 + ((ready - vsync0).div_euclid(period) + 1) * period;
                s.add(ready / 1000, vsync / 1000, period as u32);
            }
            s.report().unwrap()
        };
        let mut fc = lock.on_report(&rep(0), Some(vsync0)).unwrap();
        for _ in 0..8 {
            if let Some(n) = lock.on_report(&rep(fc.phase_offset_ns), Some(vsync0)) {
                fc = n;
            }
        }
        let r = rep(fc.phase_offset_ns);
        let lead = r.lead_p50_us as i64 * 1000;
        assert!((lead - (MARGIN_NS + 200_000)).abs() < 400_000, "lead {lead} ns, {r:?}");
        assert!(fc.grace_ns > 0 && (fc.grace_ns as i64) < MARGIN_NS, "{fc:?}");
        assert_eq!(fc.refresh_mhz, 60_000);
    }
}
