// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Liveness of the active route and failover hedging.
//!
//! Each side judges its own sending direction. Things we send that the peer
//! answers (pings, video, input events, control data) are answered by pongs,
//! transport feedback, input ACKs and control ACKs. A 100 ms ping goes out
//! whenever nothing answerable went out, so on a working route some answer
//! comes back at least every ~100 ms whatever the RTT.
//! - **Suspect** after 200 ms without an answer: freeze the controller, ask the
//!   path layer to recheck every pair, and duplicate every datagram on the best
//!   other live route (video thinned). Pings now go on the active route only,
//!   so only an answer to one of those proves the active route.
//! - **Recovered** when such a ping is answered: stop duplicating, thaw the
//!   controller (the pre-suspect target comes back, no re-ramp), no path change.
//! - **Dead** after 700 ms without an answer: the path layer moves media to the
//!   best live route, where the thin stream is already flowing; the controller
//!   re-baselines there.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub const ECHO_INTERVAL: Duration = Duration::from_millis(100);
pub const SUSPECT_AFTER: Duration = Duration::from_millis(200);
pub const DEAD_AFTER: Duration = Duration::from_millis(700);
/// The thin stream while hedging: at most this, the target, and half the hedge
/// route's last measured rate.
pub const THIN_MAX_BPS: u64 = 4_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LinkHealth {
    #[default]
    Healthy,
    /// 200 ms unanswered: duplicating on a second route.
    Suspect,
    /// 700 ms unanswered: the path layer should switch routes.
    Dead,
}

/// What the session asks of the path layer. The driver carries these to
/// `PathAgent`; a sans-IO user drains `Session::poll_path_signal`.
#[derive(Clone, Debug, PartialEq)]
pub enum PathSignal {
    /// The active route went quiet: recheck every pair; duplicate `Via::Active`
    /// datagrams on the best other live route until `Recovered` or a path change.
    Suspect,
    /// The active route answered again: stop duplicating, keep the route.
    Recovered,
    /// No answer for 700 ms: move media to the best live route.
    Dead,
    /// A probe train's result for `Session::start_probe_train`'s id.
    TrainReport { train_id: u32, rate_bps: u64, loss: f64 },
    /// The peer sent one of its ICE candidates in-band (it gathered again
    /// after a network change): add it as a remote candidate.
    RemoteCandidate(String),
}

/// Where a datagram goes (`Session::poll_transmit_via`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// The active route; while hedging, the hedge route as well.
    Active,
    /// The active route only: the liveness probe while suspect.
    ActiveOnly,
    /// A probe train: the route the driver started it for.
    Train(u32),
}

/// The hedge state, for the driver and the client UI.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Health {
    pub state: LinkHealth,
    /// Since the active route last answered something we sent.
    pub unanswered_ms: u64,
    /// Since anything authenticated arrived from the peer.
    pub silence_ms: u64,
    /// Datagrams are duplicated on a second route (suspect or dead, until the
    /// route answers or the path changes).
    pub hedging: bool,
}

pub(crate) struct Liveness {
    pub state: LinkHealth,
    pub last_answer: Instant,
    pub last_rx: Instant,
    pub last_answerable_sent: Option<Instant>,
    /// Next active-route-only probe while suspect.
    pub next_probe: Option<Instant>,
    /// t0 of pings sent on the active route only.
    pub probes: VecDeque<u64>,
}

impl Liveness {
    pub fn new(now: Instant) -> Self {
        Liveness { state: LinkHealth::Healthy, last_answer: now, last_rx: now, last_answerable_sent: None, next_probe: None, probes: VecDeque::new() }
    }

    pub fn hedging(&self) -> bool {
        self.state != LinkHealth::Healthy
    }

    pub fn health(&self, now: Instant) -> Health {
        Health {
            state: self.state,
            unanswered_ms: now.saturating_duration_since(self.last_answer).as_millis() as u64,
            silence_ms: now.saturating_duration_since(self.last_rx).as_millis() as u64,
            hedging: self.hedging(),
        }
    }

    /// The next time liveness needs a look.
    pub fn poll_timeout(&self) -> Instant {
        match self.state {
            LinkHealth::Healthy => {
                let echo = self.last_answerable_sent.unwrap_or(self.last_answer) + ECHO_INTERVAL;
                echo.min(self.last_answer + SUSPECT_AFTER)
            }
            LinkHealth::Suspect => self.next_probe.unwrap_or(self.last_answer).min(self.last_answer + DEAD_AFTER),
            LinkHealth::Dead => self.next_probe.unwrap_or(self.last_answer),
        }
    }
}
