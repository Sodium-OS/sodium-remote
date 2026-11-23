// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Delay-based congestion control in the GCC / SCReAMv2 family, tuned for a
//! paced, frame-structured desktop stream.
//!
//! Signal: the *standing* queue. One-way delay minus its windowed minimum gives
//! each packet's queueing delay. Within a frame the pacer's burst always builds a
//! small queue; what matters is whether it is still there when the next frame
//! starts, so the controller looks at the minimum queueing delay over the last
//! frame interval.
//! - over 6 ms: overuse, if the queue is ours: what we sent in the last 100 ms was at least half the target, and
//!   the queue didn't arrive as a step of over 6 ms between two packets unless
//!   it then kept rising. Target drops to the receive rate of the last 100 ms
//!   (the bottleneck's rate while our queue stands) times 0.85, less what
//!   drains the standing queue in about half a second (never below 0.5x), and
//!   C_est is set. An unloaded stream's queue that rose 6 ms over 100 ms or
//!   more cuts the target the same way but sets no C_est. Anything else (a
//!   delay hiccup on an idle or app-limited stream) holds.
//! - under 2 ms, loss under 2%: increase. +60%/s far from the last overuse
//!   point, +10%/s near it, never above 1.5x the receive rate.
//! - otherwise hold. Loss over 10% cuts the target by half the loss fraction.
//!
//! The output is a target bitrate, read before every frame, and an FEC ratio.
//!
//! Link state for the allocator and the encoder's split
//! rule:
//! - `c_est()`: the measured bottleneck rate, the receive rate at the last
//!   delay overuse that was ours. Held through idle periods. Raised when more than it gets
//!   through with no standing queue; released after 10 s without a standing
//!   queue while the stream used at least half of it (the link was exercised
//!   and never queued), and on a path change.
//! - `queue_free_for()`: how long the standing queue has stayed under 2 ms.
//!
//! Probing: an idle
//! desktop shows the link nothing, so the app-limited cap would hold the target
//! at its start for good. While no bottleneck is measured, the probe's rate is
//! under any refused probe's,
//! the target is under min(demand, cap) and the link is quiet, the
//! session sends one frame's worth of padding at 1.3 × the target, paced as a
//! frame, at most every 250 ms (`probe_due`, `start_probe`). Its feedback
//! judges it by the standing queue it left one frame interval after its first
//! packet, the queue the next frame would have met: under 2 ms the target
//! becomes its rate, 2 to 6 ms refuses it and sets `probe_ceiling` (forgotten
//! as C_est is), over 6 ms is the decrease.

use crate::feedback::{SentLog, TransportFeedback};
use std::collections::VecDeque;
use std::time::Duration;

const Q_LOW_US: i64 = 2_000;
const Q_HIGH_US: i64 = 6_000;
const BETA: f64 = 0.85;
const RATE_WINDOW_US: u64 = 250_000;
const SHORT_RATE_WINDOW_US: u64 = 100_000;
const DRAIN_TIME_S: f64 = 0.5;
const BASE_BUCKET_US: u64 = 5_000_000;
const LOSS_WINDOW_US: u64 = 500_000;
const FEC_LOSS_WINDOW_US: u64 = 2_000_000;
/// C_est is let go after this long with no standing queue while the stream
/// used at least `C_EST_RELEASE_USE` of it.
const C_EST_RELEASE_US: u64 = 10_000_000;
const C_EST_RELEASE_USE: f64 = 0.5;
/// A probe tests this multiple of the target...
const PROBE_GAIN: f64 = 1.3;
/// ...at most this often (start to start).
const PROBE_GAP_US: u64 = 250_000;
/// A probe with no verdict this long after it started proves nothing (its
/// feedback was lost); the next one may go.
const PROBE_TIMEOUT_US: u64 = 1_000_000;
/// A delay overuse is ours only if what we sent in the last 100 ms was at
/// least this share of the target...
const LOADED_SHARE: f64 = 0.5;
/// ...and an unloaded stream's rising queue cuts the target (never C_est)
/// once it has stood this long.
const CONFIRM_US: u64 = 100_000;

#[derive(Clone, Debug)]
pub struct CcConfig {
    pub start_bps: u64,
    pub min_bps: u64,
    pub max_bps: u64,
}

impl Default for CcConfig {
    fn default() -> Self {
        CcConfig { start_bps: 8_000_000, min_bps: 500_000, max_bps: 300_000_000 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CcState {
    Increase,
    Hold,
    Decrease,
}

/// Windowed minimum of one-way delay: the min of this 5 s bucket and the last.
#[derive(Default)]
struct BaseDelay {
    cur: Option<i64>,
    prev: Option<i64>,
    bucket_start: u64,
}

impl BaseDelay {
    fn update(&mut self, now_us: u64, owd: i64) {
        if now_us >= self.bucket_start + BASE_BUCKET_US {
            self.prev = self.cur;
            self.cur = None;
            self.bucket_start = now_us;
        }
        self.cur = Some(self.cur.map_or(owd, |c| c.min(owd)));
    }
    fn get(&self) -> Option<i64> {
        match (self.cur, self.prev) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// A probe in flight: one frame's worth of padding at `bps`.
struct Probe {
    id: u32,
    bps: f64,
    packets: usize,
    started_us: u64,
    /// How many of its packets left the pacer, the first one's send time and
    /// the last one's counter.
    left: usize,
    first_send_us: Option<u64>,
    last_counter: u64,
    /// Its packets the feedback reported, and of those the ones that arrived:
    /// (send time, one-way delay, arrival, size).
    reported: usize,
    arrivals: Vec<(u64, i64, u64, usize)>,
}

/// What a judged probe found.
struct Verdict {
    bps: f64,
    /// The standing queue one frame interval after its first packet.
    queue_us: i64,
    /// The receive rate while it arrived (the bottleneck's while a queue stood).
    rate: Option<f64>,
}

/// The probes so far, for stats.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProbeStats {
    /// The rate the last probe tested (0: none yet).
    pub last_bps: u64,
    pub sent: u64,
    pub accepted: u64,
    /// Refused (a 2 to 6 ms standing queue): `probe_ceiling` set.
    pub refused: u64,
    /// Over 6 ms: the decrease.
    pub decreased: u64,
    /// Cut by a frame, frozen, all lost or never judged: no verdict.
    pub void: u64,
    /// The largest standing queue a judged probe left.
    pub queue_max_us: i64,
}

pub struct CongestionController {
    cfg: CcConfig,
    target: f64,
    base: BaseDelay,
    /// (arrival_us on the receiver's clock, one-way delay, size, send_us).
    samples: VecDeque<(u64, i64, usize, u64)>,
    /// The previous arrived packet's queueing delay, and when (feedback time)
    /// a packet last read more than `Q_HIGH_US` above it: a queue of ours grows
    /// by one packet's serialisation at a time, so a step that size is a stall
    /// or someone else's queue.
    prev_q: Option<i64>,
    last_jump_us: Option<u64>,
    /// The overuse episode in progress (consecutive feedbacks over `Q_HIGH_US`):
    /// (its first feedback, the signal then, a jump seen during it).
    episode: Option<(u64, i64, bool)>,
    /// (sender time, lost, total) per feedback.
    losses: VecDeque<(u64, u32, u32)>,
    last_update_us: Option<u64>,
    last_loss_cut_us: u64,
    overuse_rate: Option<f64>,
    /// The measured bottleneck rate (C_est); see the module docs.
    c_est: Option<f64>,
    /// Since when the stream has used at least half of C_est with no overuse.
    c_est_clear_since: Option<u64>,
    /// Since when the standing queue has stayed under `Q_LOW_US` (None: no feedback yet).
    queue_free_since: Option<u64>,
    last_feedback_us: Option<u64>,
    /// While frozen (failover hedging) feedback changes nothing: arrivals then
    /// mix two routes and say nothing about either.
    frozen: bool,
    /// The session's route demand.
    demand: f64,
    probe: Option<Probe>,
    next_probe_id: u32,
    last_probe_us: Option<u64>,
    /// The rate of a refused probe: no probe at or over it while it is set.
    probe_ceiling: Option<f64>,
    ceiling_clear_since: Option<u64>,
    /// The last probe the link carried with no queue: the app-limited cap
    /// treats it as seen. Gone at a decrease or a path change.
    probe_seen: Option<f64>,
    /// The newest counter feedback has covered (arrived or lost).
    reported_to: Option<u64>,
    pub probes: ProbeStats,
    pub state: CcState,
    pub frame_interval: Duration,
    /// Last computed signals, for stats.
    pub queue_delay_us: i64,
    pub receive_rate_bps: f64,
    pub loss: f64,
}

impl CongestionController {
    pub fn new(cfg: CcConfig, frame_interval: Duration) -> Self {
        CongestionController {
            target: cfg.start_bps as f64,
            cfg,
            base: BaseDelay::default(),
            samples: VecDeque::new(),
            prev_q: None,
            last_jump_us: None,
            episode: None,
            losses: VecDeque::new(),
            last_update_us: None,
            last_loss_cut_us: 0,
            overuse_rate: None,
            c_est: None,
            c_est_clear_since: None,
            queue_free_since: None,
            last_feedback_us: None,
            frozen: false,
            demand: 0.0,
            probe: None,
            next_probe_id: 0,
            last_probe_us: None,
            probe_ceiling: None,
            ceiling_clear_since: None,
            probe_seen: None,
            reported_to: None,
            probes: ProbeStats::default(),
            state: CcState::Increase,
            frame_interval,
            queue_delay_us: 0,
            receive_rate_bps: 0.0,
            loss: 0.0,
        }
    }

    /// The bitrate the whole video stream (encoder output plus FEC) may use.
    pub fn target_bps(&self) -> u64 {
        self.target as u64
    }

    /// Change the cap (the user's bandwidth limit).
    pub fn set_max_bps(&mut self, max: u64) {
        self.cfg.max_bps = max;
        self.target = self.target.min(max as f64);
    }

    /// After a path switch the old base delay means nothing, and neither does
    /// what was learnt about the old route: C_est, a probe's ceiling.
    pub fn reset_path(&mut self) {
        self.base = BaseDelay::default();
        self.samples.clear();
        self.prev_q = None;
        self.last_jump_us = None;
        self.episode = None;
        self.overuse_rate = None;
        self.c_est = None;
        self.c_est_clear_since = None;
        self.queue_free_since = None;
        self.frozen = false;
        self.void_probe();
        self.last_probe_us = None;
        self.probe_ceiling = None;
        self.ceiling_clear_since = None;
        self.probe_seen = None;
    }

    /// Start over on a new path at this rate (the failover thin stream).
    pub fn rebaseline(&mut self, bps: u64) {
        self.reset_path();
        self.target = (bps as f64).clamp(self.cfg.min_bps as f64, self.cfg.max_bps as f64);
        self.state = CcState::Increase;
    }

    /// Freeze or thaw: while frozen, feedback is logged out but changes nothing,
    /// so thawing restores the target from before (no re-ramp).
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
        if frozen {
            self.void_probe(); // its feedback would mix two routes
        }
    }

    pub fn frozen(&self) -> bool {
        self.frozen
    }

    /// The measured bottleneck rate, C_est.
    pub fn c_est(&self) -> Option<u64> {
        self.c_est.map(|c| c as u64)
    }

    /// The session's route demand: probes lift an idle target up
    /// to min(demand, cap). 0: no probes.
    pub fn set_demand(&mut self, bps: u64) {
        self.demand = bps as f64;
    }

    /// A refused probe's rate: no probe at or over it goes while it is set.
    pub fn probe_ceiling(&self) -> Option<u64> {
        self.probe_ceiling.map(|c| c as u64)
    }

    /// Has feedback covered this counter (arrived or lost)?
    pub fn reported_through(&self, counter: u64) -> bool {
        self.reported_to.is_some_and(|r| r >= counter)
    }

    /// The rate a probe should test now, if one is due: no bottleneck measured,
    /// no probe in flight, not frozen, the target under min(demand, cap), the
    /// probe's rate under a refused probe's, and 250 ms
    /// since the last probe started. The session adds the
    /// link's side: not hedging, and quiet (no video in the last frame interval,
    /// none on the pacer, all of it covered by feedback).
    pub fn probe_due(&mut self, now_us: u64) -> Option<u64> {
        self.expire_probe(now_us);
        if self.probe_not_before().is_none_or(|t| now_us < t) {
            return None;
        }
        Some(self.probe_rate() as u64)
    }

    fn probe_rate(&self) -> f64 {
        (self.target * PROBE_GAIN).min(self.demand.min(self.cfg.max_bps as f64))
    }

    /// When the next probe may start (µs), if one is wanted at all: the
    /// conditions of `probe_due` but the time.
    pub fn probe_not_before(&self) -> Option<u64> {
        let limit = self.demand.min(self.cfg.max_bps as f64);
        // Under a refused probe's rate a probe may still go, so a target cut on
        // that link (loss, an unloaded queue) climbs back while it is idle.
        let refused = self.probe_ceiling.is_some_and(|c| self.probe_rate() >= c);
        let blocked = self.frozen || self.c_est.is_some() || refused || self.probe.is_some();
        (!blocked && self.target < limit).then(|| self.last_probe_us.map_or(0, |t| t + PROBE_GAP_US))
    }

    /// A probe of `packets` padding packets at `bps` goes on the pacer now.
    /// Returns its id, which the sent log carries (`SentLog::record_probe`).
    pub fn start_probe(&mut self, now_us: u64, bps: u64, packets: usize) -> u32 {
        let id = self.next_probe_id;
        self.next_probe_id = self.next_probe_id.wrapping_add(1);
        self.probe = Some(Probe {
            id,
            bps: bps as f64,
            packets,
            started_us: now_us,
            left: 0,
            first_send_us: None,
            last_counter: 0,
            reported: 0,
            arrivals: Vec::new(),
        });
        self.last_probe_us = Some(now_us);
        self.probes.sent += 1;
        self.probes.last_bps = bps;
        id
    }

    /// One of the probe's packets left the pacer as `counter`.
    pub fn probe_packet_left(&mut self, id: u32, counter: u64, send_us: u64) {
        if let Some(p) = self.probe.as_mut().filter(|p| p.id == id) {
            p.left += 1;
            p.first_send_us.get_or_insert(send_us);
            p.last_counter = counter;
        }
    }

    /// A frame cut the probe (the padding still on the pacer was dropped): an
    /// incomplete probe tested less than its rate, so it has no verdict.
    pub fn cut_probe(&mut self) {
        self.void_probe();
    }

    fn void_probe(&mut self) {
        if self.probe.take().is_some() {
            self.probes.void += 1;
        }
    }

    fn expire_probe(&mut self, now_us: u64) {
        if self.probe.as_ref().is_some_and(|p| now_us >= p.started_us + PROBE_TIMEOUT_US) {
            self.void_probe();
        }
    }

    /// How long the standing queue has stayed under 2 ms (zero before any feedback).
    pub fn queue_free_for(&self, now_us: u64) -> Duration {
        self.queue_free_since.map_or(Duration::ZERO, |t| Duration::from_micros(now_us.saturating_sub(t)))
    }

    /// Loss over the last 2 s, for Opus's expected-loss setting.
    pub fn recent_loss(&self, now_us: u64) -> f64 {
        self.loss_over(now_us, FEC_LOSS_WINDOW_US)
    }

    fn loss_over(&self, now_us: u64, window: u64) -> f64 {
        let (lost, total) = self
            .losses
            .iter()
            .filter(|(t, _, _)| *t + window >= now_us)
            .fold((0u32, 0u32), |(l, n), (_, a, b)| (l + a, n + b));
        if total == 0 { 0.0 } else { lost as f64 / total as f64 }
    }

    /// Loss-adaptive FEC: none on a clean path, then 4x the recent loss plus 2%,
    /// up to 30%.
    pub fn fec_ratio(&self, now_us: u64) -> f32 {
        let loss = self.loss_over(now_us, FEC_LOSS_WINDOW_US);
        if loss < 0.001 {
            0.0
        } else {
            ((4.0 * loss + 0.02) as f32).min(crate::video::MAX_FEC_RATIO)
        }
    }

    /// Returns how many of the reported arrivals were packets sent for
    /// congestion control (not probe trains), frozen or not: the session's
    /// proof that the active route delivers.
    pub fn on_feedback(&mut self, now_us: u64, fb: &TransportFeedback, log: &mut SentLog) -> usize {
        let (mut lost, mut total) = (0u32, 0u32);
        let frozen = self.frozen;
        let mut delivered = 0;
        self.expire_probe(now_us);
        for (i, r) in fb.received.iter().enumerate() {
            let Some(sent) = log.get(fb.from + i as u64) else { continue };
            delivered += (sent.cc && r.is_some()) as usize;
            if !sent.cc || frozen {
                continue; // probe trains on other routes; anything while frozen
            }
            total += 1;
            let owd = r.map(|a| a as i64 - sent.send_us as i64);
            match (r, owd) {
                (Some(arrival), Some(owd)) => {
                    self.base.update(now_us, owd);
                    let q = owd - self.base.get().unwrap_or(owd);
                    if self.prev_q.is_some_and(|p| q > p + Q_HIGH_US) {
                        self.last_jump_us = Some(now_us);
                    }
                    self.prev_q = Some(q);
                    self.samples.push_back((*arrival, owd, sent.size, sent.send_us));
                }
                _ => lost += 1, // a probe's lost padding counts as loss too
            }
            if let (Some(id), Some(p)) = (sent.probe, self.probe.as_mut()) {
                if p.id == id {
                    p.reported += 1;
                    if let (Some(arrival), Some(owd)) = (r, owd) {
                        p.arrivals.push((sent.send_us, owd, *arrival, sent.size));
                    }
                }
            }
        }
        if !fb.received.is_empty() {
            let last = fb.from + fb.received.len() as u64 - 1;
            self.reported_to = Some(self.reported_to.map_or(last, |r| r.max(last)));
        }
        log.forget_before(fb.from + fb.received.len() as u64);
        if total == 0 {
            return delivered;
        }
        self.last_feedback_us = Some(now_us);
        self.losses.push_back((now_us, lost, total));
        while self.losses.front().is_some_and(|(t, _, _)| *t + FEC_LOSS_WINDOW_US < now_us) {
            self.losses.pop_front();
        }
        let Some(newest) = self.samples.iter().map(|s| s.0).max() else { return delivered };
        while self.samples.front().is_some_and(|s| s.0 + 2 * RATE_WINDOW_US < newest) {
            self.samples.pop_front();
        }
        let base = self.base.get().unwrap_or(0);
        let fi = self.frame_interval.as_micros() as u64;
        self.queue_delay_us = self
            .samples
            .iter()
            .filter(|s| s.0 + fi >= newest)
            .map(|s| s.1 - base)
            .min()
            .unwrap_or(0);
        let rate_over = |w: u64| {
            let window: Vec<_> = self.samples.iter().filter(|s| s.0 + w >= newest).collect();
            let span = newest - window.iter().map(|s| s.0).min().unwrap_or(newest);
            (span >= w / 2).then(|| window.iter().map(|s| s.2).sum::<usize>() as f64 * 8.0 / (span as f64 / 1e6))
        };
        if let Some(r) = rate_over(RATE_WINDOW_US) {
            self.receive_rate_bps = r;
        }
        let short_rate = rate_over(SHORT_RATE_WINDOW_US);
        self.loss = self.loss_over(now_us, LOSS_WINDOW_US);

        let before = self.target;
        let dt = self
            .last_update_us
            .map_or(0.0, |t| (now_us.saturating_sub(t) as f64 / 1e6).min(0.1));
        self.last_update_us = Some(now_us);
        let rate = (self.receive_rate_bps > 0.0).then_some(self.receive_rate_bps);

        if self.queue_delay_us < Q_LOW_US {
            self.queue_free_since.get_or_insert(now_us);
        } else {
            self.queue_free_since = None;
        }
        // Whose queue is it? Only a stream that kept the link
        // loaded can have built it, and only a queue that grew packet by packet
        // or kept rising is a bottleneck's; a step is a stall or someone else's
        // (busy Wi-Fi, a shared relay, a TCP relay's head-of-line block).
        let over = self.queue_delay_us > Q_HIGH_US;
        let jump = self.last_jump_us.is_some_and(|t| t + SHORT_RATE_WINDOW_US >= now_us);
        if over {
            let ep = self.episode.get_or_insert((now_us, self.queue_delay_us, false));
            ep.2 |= jump;
        } else {
            self.episode = None;
        }
        let newest_send = self.samples.iter().map(|s| s.3).max().unwrap_or(0);
        let sent_bytes: usize =
            self.samples.iter().filter(|s| s.3 + SHORT_RATE_WINDOW_US >= newest_send).map(|s| s.2).sum();
        let sent_bps = sent_bytes as f64 * 8.0 / (SHORT_RATE_WINDOW_US as f64 / 1e6);
        let loaded = sent_bps >= LOADED_SHARE * self.target;
        let (rose, stood, jumped) = self.episode.map_or((false, false, false), |(t, q0, j)| {
            (self.queue_delay_us >= q0 + Q_HIGH_US, now_us >= t + CONFIRM_US, j)
        });
        let ours = loaded && (!jumped || rose);
        let unloaded_but_real = !loaded && rose && stood && sent_bps >= self.cfg.min_bps as f64;
        if over && (ours || unloaded_but_real) {
            self.state = CcState::Decrease;
            // While a queue of ours stands the link is busy, so arrivals measure its rate.
            let r = short_rate.or(rate).unwrap_or(self.target);
            let drain = self.queue_delay_us as f64 / 1e6 / DRAIN_TIME_S;
            self.target = self.target.min(r * (BETA - drain).max(0.5));
            self.overuse_rate = Some(r);
            if ours {
                // C_est is sticky and drives the critical cap and the route
                // score, so only the strong case sets it.
                self.c_est = Some(r);
                self.c_est_clear_since = None;
            }
            self.probe_seen = None;
        } else if self.loss > 0.10 {
            self.state = CcState::Decrease;
            if now_us >= self.last_loss_cut_us + 200_000 {
                self.target *= 1.0 - 0.5 * self.loss;
                self.last_loss_cut_us = now_us;
            }
            self.probe_seen = None;
        } else if self.queue_delay_us < Q_LOW_US && self.loss < 0.02 {
            self.state = CcState::Increase;
            let near = self.overuse_rate.is_some_and(|r| self.target > 0.8 * r && self.target < 1.1 * r);
            if near {
                self.target += self.target * 0.10 * dt;
            } else {
                self.target *= 1.6f64.powf(dt);
            }
            if let Some(r) = rate {
                // An app-limited sender doesn't creep up, but it isn't pulled
                // down either: an idle desktop keeps what was measured, and the
                // last probe the link carried counts as seen.
                let cap = (1.5 * r + 1_000_000.0).max(before).max(self.probe_seen.unwrap_or(0.0));
                self.target = self.target.min(cap);
            }
        } else {
            self.state = CcState::Hold;
        }
        if let Some(v) = self.judge_probe() {
            self.apply_probe(v);
        }
        self.target = self.target.clamp(self.cfg.min_bps as f64, self.cfg.max_bps as f64);
        self.update_c_est(now_us, rate);
        self.update_ceiling(now_us, rate);
        delivered
    }

    /// The verdict on the probe in flight, once all of it left the pacer and
    /// feedback has covered it: the standing queue it left one frame interval
    /// after its first packet, which is what the next frame would have met. A
    /// FIFO bottleneck is busy until its last packet leaves, so that queue is
    /// the latest (send time + queueing delay) less the interval's end.
    fn judge_probe(&mut self) -> Option<Verdict> {
        let p = self.probe.as_ref()?;
        if p.left < p.packets || (p.reported < p.packets && !self.reported_through(p.last_counter)) {
            return None;
        }
        let p = self.probe.take()?;
        let busy_until = match (self.base.get(), p.first_send_us) {
            (Some(base), Some(_)) => p.arrivals.iter().map(|&(s, owd, _, _)| s as i64 + (owd - base)).max(),
            _ => None,
        };
        let (Some(busy_until), Some(start)) = (busy_until, p.first_send_us) else {
            self.probes.void += 1; // all of it lost: the loss rule speaks
            return None;
        };
        let fi = self.frame_interval.as_micros() as i64;
        let queue_us = (busy_until - (start as i64 + fi)).max(0);
        let first = p.arrivals.iter().min_by_key(|a| a.2)?;
        let span = p.arrivals.iter().map(|a| a.2).max()? - first.2;
        let bytes = p.arrivals.iter().map(|a| a.3).sum::<usize>() - first.3;
        let rate = (span > 0).then(|| bytes as f64 * 8.0 / (span as f64 / 1e6));
        Some(Verdict { bps: p.bps, queue_us, rate })
    }

    fn apply_probe(&mut self, v: Verdict) {
        self.probes.queue_max_us = self.probes.queue_max_us.max(v.queue_us);
        self.queue_delay_us = self.queue_delay_us.max(v.queue_us);
        if v.queue_us > Q_HIGH_US {
            // The decrease, as today, at the rate the probe arrived at.
            self.probes.decreased += 1;
            if self.state != CcState::Decrease {
                let r = v.rate.unwrap_or(self.target);
                let drain = v.queue_us as f64 / 1e6 / DRAIN_TIME_S;
                self.target = self.target.min(r * (BETA - drain).max(0.5));
                self.overuse_rate = Some(r);
                self.c_est = Some(r);
                self.c_est_clear_since = None;
                self.state = CcState::Decrease;
            }
            self.queue_free_since = None;
            self.probe_seen = None;
        } else if v.queue_us >= Q_LOW_US {
            self.probes.refused += 1;
            self.probe_ceiling = Some(v.bps);
            self.ceiling_clear_since = None;
            self.queue_free_since = None;
            if self.state == CcState::Increase {
                self.state = CcState::Hold;
            }
        } else if self.state != CcState::Decrease && self.c_est.is_none() && self.loss < 0.02 {
            // The link just carried this rate with no queue: all the increase rule wants to see.
            self.probes.accepted += 1;
            self.target = self.target.max(v.bps);
            self.probe_seen = Some(self.probe_seen.map_or(v.bps, |s| s.max(v.bps)));
            self.state = CcState::Increase;
        }
    }

    /// A refused probe's ceiling is forgotten as C_est is: after
    /// 10 s of traffic at half the target or more with no standing queue.
    fn update_ceiling(&mut self, now_us: u64, rate: Option<f64>) {
        let (Some(_), Some(r)) = (self.probe_ceiling, rate) else { return };
        if self.queue_delay_us >= Q_LOW_US || r < C_EST_RELEASE_USE * self.target {
            self.ceiling_clear_since = None;
        } else {
            let since = *self.ceiling_clear_since.get_or_insert(now_us);
            if now_us >= since + C_EST_RELEASE_US {
                self.probe_ceiling = None;
                self.ceiling_clear_since = None;
            }
        }
    }

    fn update_c_est(&mut self, now_us: u64, rate: Option<f64>) {
        let (Some(c), Some(r)) = (self.c_est, rate) else { return };
        if self.state == CcState::Decrease {
            return;
        }
        if self.queue_delay_us < Q_LOW_US && r > c {
            self.c_est = Some(r); // more got through than we thought, with no queue
        }
        // The release timer runs only while the standing queue stays under 2 ms
        // A queue means the link is still the bottleneck.
        if self.queue_delay_us >= Q_LOW_US {
            self.c_est_clear_since = None;
        } else if r >= C_EST_RELEASE_USE * c {
            let since = *self.c_est_clear_since.get_or_insert(now_us);
            if now_us >= since + C_EST_RELEASE_US {
                self.c_est = None;
                self.c_est_clear_since = None;
            }
        } else {
            self.c_est_clear_since = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fluid model of a bottleneck: packets queue at `capacity` bps.
    fn run(cc: &mut CongestionController, capacity: f64, secs: f64, t0: &mut u64, link_free: &mut f64) {
        let fi = 16_667u64;
        let mut log = SentLog::default();
        let mut counter = 0u64;
        let end = *t0 + (secs * 1e6) as u64;
        while *t0 < end {
            let frame_bytes = (cc.target_bps() as f64 / 60.0 / 8.0) as usize;
            let n = frame_bytes.div_ceil(1200).max(1);
            let mut fb = TransportFeedback { fb_seq: 0, from: counter, received: Vec::new() };
            for i in 0..n {
                // paced over half a frame interval
                let send = *t0 as f64 + i as f64 * (fi as f64 / 2.0) / n as f64;
                log.record(counter, send as u64, 1200);
                let start = link_free.max(send);
                *link_free = start + 1200.0 * 8.0 / capacity * 1e6;
                fb.received.push(Some(*link_free as u64 + 5_000));
                counter += 1;
            }
            *t0 += fi;
            cc.on_feedback(*t0 + 5_000, &fb, &mut log);
        }
    }

    #[test]
    fn converges_after_a_bandwidth_step_down_and_keeps_the_queue_short() {
        let mut cc = CongestionController::new(CcConfig::default(), Duration::from_micros(16_667));
        let (mut t, mut free) = (0u64, 0f64);
        run(&mut cc, 30e6, 6.0, &mut t, &mut free);
        let at30 = cc.target_bps() as f64;
        assert!(at30 > 22e6 && at30 < 33e6, "settled near 30 Mbps: {at30}");
        run(&mut cc, 10e6, 1.0, &mut t, &mut free);
        let at10 = cc.target_bps() as f64;
        assert!(at10 > 7e6 && at10 < 11e6, "within a second of the step: {at10}");
        assert!(cc.queue_delay_us < 10_000, "queue drained: {}", cc.queue_delay_us);
        let c = cc.c_est().expect("a bottleneck measured") as f64;
        assert!(c > 8e6 && c < 11.5e6, "C_est near 10 Mbps: {c}");
        assert!(cc.queue_free_for(t) < Duration::from_secs(3), "no split right after a queue");
        // Capacity comes back: C_est follows it up or lets go, so 0.65 x C_est
        // never pins a link that is no longer the bottleneck.
        run(&mut cc, 30e6, 15.0, &mut t, &mut free);
        assert!(cc.c_est().is_none_or(|c| c > 20_000_000), "{:?}", cc.c_est());
    }

    /// A fluid model of a session on a bottleneck at `capacity` bps, one-way
    /// delay 5 ms, in 100 µs steps: the host's 100 ms pings, frames of
    /// `frame_bps` (None: an idle desktop) and the controller's probes whenever
    /// the link is quiet, both paced over half a frame interval; the receiver
    /// reports what arrived once a frame interval. It tracks the queue the link
    /// really held one frame interval after each probe started.
    struct Bed {
        cc: CongestionController,
        log: SentLog,
        capacity: f64,
        link_free: f64,
        now: u64,
        counter: u64,
        /// To send: (time, size, probe id, is a frame's).
        pending: VecDeque<(u64, usize, Option<u32>, bool)>,
        /// Sent, not reported yet: (counter, arrival).
        flight: VecDeque<(u64, u64)>,
        next_ping: u64,
        next_frame: u64,
        next_fb: u64,
        last_video: Option<(u64, u64)>,
        checks: VecDeque<u64>,
        worst_standing_us: f64,
        worst_signal_us: i64,
        /// Delay the link adds that isn't a queue of ours (Wi-Fi, a shared
        /// relay): packets sent in the first `len` µs of every `period` arrive
        /// `extra` µs late. (period, len, extra)
        spikes: Option<(u64, u64, u64)>,
    }

    const FI: u64 = 16_667;

    impl Bed {
        fn new(capacity: f64, demand: u64) -> Bed {
            let mut cc = CongestionController::new(CcConfig::default(), Duration::from_micros(FI));
            cc.set_demand(demand);
            Bed {
                cc,
                log: SentLog::default(),
                capacity,
                link_free: 0.0,
                now: 0,
                counter: 0,
                pending: VecDeque::new(),
                flight: VecDeque::new(),
                next_ping: 0,
                next_frame: 0,
                next_fb: FI,
                last_video: None,
                checks: VecDeque::new(),
                worst_standing_us: 0.0,
                worst_signal_us: 0,
                spikes: None,
            }
        }

        fn pace(&mut self, bytes: f64, probe: Option<u32>) -> usize {
            let n = (bytes / 1200.0).ceil().max(1.0) as usize;
            for i in 0..n {
                self.pending.push_back((self.now + i as u64 * (FI / 2) / n as u64, 1200, probe, probe.is_none()));
            }
            n
        }

        fn run(&mut self, secs: f64, frame_bps: Option<f64>) {
            let end = self.now + (secs * 1e6) as u64;
            while self.now < end {
                if self.now >= self.next_ping {
                    self.pending.push_front((self.now, 80, None, false));
                    self.next_ping += 100_000;
                }
                self.next_frame = self.next_frame.max(self.now);
                if let Some(bps) = frame_bps.filter(|_| self.now >= self.next_frame) {
                    self.pace(bps * FI as f64 / 1e6 / 8.0, None);
                    self.next_frame += FI;
                }
                let quiet = self.pending.is_empty()
                    && self.last_video.is_none_or(|(t, c)| self.now >= t + FI && self.cc.reported_through(c));
                if let Some(bps) = self.cc.probe_due(self.now).filter(|_| quiet) {
                    let bytes = bps as f64 * FI as f64 / 1e6 / 8.0;
                    let id = self.cc.start_probe(self.now, bps, (bytes / 1200.0).ceil() as usize);
                    self.pace(bytes, Some(id));
                    self.checks.push_back(self.now + FI);
                }
                while self.pending.front().is_some_and(|p| p.0 <= self.now) {
                    let (t, size, probe, frame) = self.pending.pop_front().unwrap();
                    let c = self.counter;
                    self.counter += 1;
                    match probe {
                        Some(id) => {
                            self.log.record_probe(c, t, size, id);
                            self.cc.probe_packet_left(id, c, t);
                        }
                        None => self.log.record(c, t, size),
                    }
                    if frame {
                        self.last_video = Some((t, c));
                    }
                    self.link_free = self.link_free.max(t as f64) + size as f64 * 8.0 / self.capacity * 1e6;
                    let extra = self.spikes.map_or(0, |(period, len, extra)| if t % period < len { extra } else { 0 });
                    self.flight.push_back((c, self.link_free as u64 + 5_000 + extra));
                }
                while self.checks.front().is_some_and(|t| *t <= self.now) {
                    let t = self.checks.pop_front().unwrap();
                    self.worst_standing_us = self.worst_standing_us.max(self.link_free - t as f64);
                }
                if self.now >= self.next_fb {
                    self.next_fb += FI;
                    let from = self.flight.front().map(|f| f.0);
                    let mut fb = TransportFeedback { fb_seq: 0, from: from.unwrap_or(0), received: Vec::new() };
                    while self.flight.front().is_some_and(|f| f.1 <= self.now) {
                        fb.received.push(Some(self.flight.pop_front().unwrap().1));
                    }
                    if !fb.received.is_empty() {
                        self.cc.on_feedback(self.now, &fb, &mut self.log);
                        self.worst_signal_us = self.worst_signal_us.max(self.cc.queue_delay_us);
                    }
                }
                self.now += 100;
            }
        }
    }

    #[test]
    fn an_idle_sender_probes_a_fast_link_up_to_the_demand_with_no_queue() {
        let mut bed = Bed::new(300e6, 65_000_000);
        let mut reached = None;
        while bed.now < 3_000_000 && reached.is_none() {
            bed.run(0.01, None);
            if bed.cc.target_bps() >= 65_000_000 {
                reached = Some(bed.now);
            }
        }
        let reached = reached.unwrap_or_else(|| panic!("at {} after 3 s: {:?}", bed.cc.target_bps(), bed.cc.probes));
        assert!(reached <= 3_000_000, "{reached} µs");
        assert!(bed.worst_standing_us <= 2_000.0 && bed.worst_signal_us <= 2_000, "{} / {} µs", bed.worst_standing_us, bed.worst_signal_us);
        assert!(bed.cc.c_est().is_none() && bed.cc.probe_ceiling().is_none(), "{:?}", bed.cc.probes);
        // At the demand the probes stop, and nothing pulls the target down.
        let sent = bed.cc.probes.sent;
        bed.run(2.0, None);
        assert_eq!((bed.cc.probes.sent, bed.cc.target_bps()), (sent, 65_000_000));
    }

    #[test]
    fn on_a_20_mbit_link_the_probes_stop_below_it_with_no_decrease() {
        let mut bed = Bed::new(20e6, 65_000_000);
        bed.run(5.0, None);
        let t = bed.cc.target_bps() as f64;
        assert!((14e6..=20e6).contains(&t), "{t} {:?}", bed.cc.probes);
        assert!(bed.cc.probe_ceiling().is_some() && bed.cc.c_est().is_none(), "{:?}", bed.cc.probes);
        assert_eq!(bed.cc.probes.decreased, 0);
        assert!(bed.worst_standing_us <= 6_000.0 && bed.worst_signal_us <= 6_000, "{} / {} µs", bed.worst_standing_us, bed.worst_signal_us);
    }

    #[test]
    fn after_a_cut_an_idle_link_probes_back_up_to_its_ceiling() {
        let mut bed = Bed::new(20e6, 65_000_000);
        bed.run(5.0, None);
        let (settled, ceiling) = (bed.cc.target_bps(), bed.cc.probe_ceiling());
        assert!(ceiling.is_some());
        // a loss cut: what the loss rule does
        bed.cc.target = 8e6;
        bed.cc.probe_seen = None;
        bed.run(3.0, None);
        let t = bed.cc.target_bps();
        assert!(t as f64 >= 0.8 * settled as f64, "{t} from {settled}, ceiling {:?} {:?}", bed.cc.probe_ceiling(), bed.cc.probes);
        assert!(bed.cc.probe_ceiling().is_some_and(|c| c >= 15_000_000) && bed.cc.probes.decreased == 0, "{:?}", bed.cc.probes);
        assert!(bed.worst_standing_us <= 6_000.0, "{} µs", bed.worst_standing_us);
    }

    #[test]
    fn a_refused_ceiling_is_forgotten_after_10_s_of_traffic_at_half_the_target() {
        let mut bed = Bed::new(20e6, 65_000_000);
        bed.run(5.0, None);
        assert!(bed.cc.probe_ceiling().is_some());
        let half = 0.5 * bed.cc.target_bps() as f64;
        bed.run(9.0, Some(half));
        assert!(bed.cc.probe_ceiling().is_some(), "kept before 10 s");
        bed.run(2.0, Some(half));
        assert!(bed.cc.probe_ceiling().is_none(), "forgotten after 10 s");
    }

    #[test]
    fn delay_spikes_on_an_idle_link_dont_read_as_a_tiny_bottleneck() {
        // The relay at 19 Mbit/s, the desktop idle, and a 25 ms hiccup that isn't
        // ours every 3 s (busy Wi-Fi, a shared relay).
        let mut bed = Bed::new(19e6, 65_000_000);
        bed.run(5.0, None);
        let settled = bed.cc.target_bps();
        bed.spikes = Some((3_000_000, 40_000, 25_000));
        bed.run(30.0, None);
        let (t, c) = (bed.cc.target_bps(), bed.cc.c_est());
        assert!(t as f64 >= 0.5 * settled as f64, "target {t} from {settled}, C_est {c:?}, {:?}", bed.cc.probes);
        assert!(c.is_none_or(|c| c >= 5_000_000), "C_est {c:?}, target {t}");
        // Scrolling: frames fill the budget, capped at 0.65 x C_est once one is
        // measured, so the stream is loaded but below the link; the hiccups'
        // steps still aren't a bottleneck.
        let end = bed.now + 30_000_000;
        while bed.now < end {
            let cap = bed.cc.c_est().map_or(f64::MAX, |c| 0.65 * c as f64);
            let r = (bed.cc.target_bps() as f64).min(cap);
            bed.run(FI as f64 / 1e6, Some(r));
        }
        let (t, c) = (bed.cc.target_bps(), bed.cc.c_est());
        assert!(t >= 10_000_000 && c.is_none_or(|c| c >= 10_000_000), "scrolling: target {t}, C_est {c:?}");
    }
}
