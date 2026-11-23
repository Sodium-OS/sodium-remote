// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The session lifecycle as a plain state machine (design
//! and 7), so its rules are tested without a compositor or a network.
//!
//! - One client at a time.
//! - A paired device connecting while another is **live** must ask its user first
//!   ("In use by X. Take over?"); once it confirms, it takes over and the old client
//!   hears "Taken over by …".
//! - A **held** session (its connection was lost, its display and layout are kept)
//!   is taken over silently; the new client shows "Took over from X".
//! - The same device coming back resumes its own session.
//! - A held session ends after 2 minutes; then windows go back.

use std::time::{Duration, Instant};

pub const HOLD: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    pub device: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Live { client: Client },
    Held { client: Client, until: Instant },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Nobody was connected: set everything up.
    Start,
    /// The same device again (a reconnect, or its own held session): keep the display.
    Resume { was_held: bool },
    /// Another device is live: the newcomer asks its user first.
    AskFirst { in_use_by: Client },
    /// Taken over from `from`. `from_was_live`: the old client is told and the host
    /// notifies "X took over"; otherwise it was held and the takeover is silent.
    TakeOver { from: Client, from_was_live: bool },
}

pub struct SessionCore {
    phase: Phase,
    hold: Duration,
}

impl Default for SessionCore {
    fn default() -> Self {
        SessionCore { phase: Phase::Idle, hold: HOLD }
    }
}

impl SessionCore {
    /// `$REMOTED_HOLD_SECS` shortens the hold for tests; 120 s otherwise.
    pub fn from_env() -> Self {
        let hold = std::env::var("REMOTED_HOLD_SECS").ok().and_then(|v| v.parse().ok()).map_or(HOLD, Duration::from_secs);
        SessionCore { phase: Phase::Idle, hold }
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn client(&self) -> Option<&Client> {
        match &self.phase {
            Phase::Idle => None,
            Phase::Live { client } | Phase::Held { client, .. } => Some(client),
        }
    }

    /// A paired device asks to connect. `confirmed`: its user already chose "Take over".
    pub fn connect(&mut self, c: Client, confirmed: bool) -> Decision {
        let d = match &self.phase {
            Phase::Idle => Decision::Start,
            Phase::Live { client } if client.device == c.device => Decision::Resume { was_held: false },
            Phase::Held { client, .. } if client.device == c.device => Decision::Resume { was_held: true },
            Phase::Live { client } if !confirmed => return Decision::AskFirst { in_use_by: client.clone() },
            Phase::Live { client } => Decision::TakeOver { from: client.clone(), from_was_live: true },
            Phase::Held { client, .. } => Decision::TakeOver { from: client.clone(), from_was_live: false },
        };
        self.phase = Phase::Live { client: c };
        d
    }

    /// The live client's connection dropped: hold its session.
    pub fn lost(&mut self, device: &str, now: Instant) -> bool {
        match &self.phase {
            Phase::Live { client } if client.device == device => {
                self.phase = Phase::Held { client: client.clone(), until: now + self.hold };
                true
            }
            _ => false,
        }
    }

    /// The session ends now (the client left, Disconnect, the device was removed).
    pub fn end(&mut self) -> Option<Client> {
        let c = self.client().cloned();
        self.phase = Phase::Idle;
        c
    }

    /// When the hold runs out, if one is running.
    pub fn deadline(&self) -> Option<Instant> {
        match &self.phase {
            Phase::Held { until, .. } => Some(*until),
            _ => None,
        }
    }

    /// Ends a held session whose 2 minutes are up.
    pub fn tick(&mut self, now: Instant) -> Option<Client> {
        match self.deadline() {
            Some(t) if now >= t => self.end(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(d: &str) -> Client {
        Client { device: d.into(), name: format!("{d} name") }
    }

    #[test]
    fn takeover_asks_only_when_the_other_client_is_live() {
        let mut s = SessionCore::default();
        assert_eq!(s.connect(c("air"), false), Decision::Start);
        // Live: the mini must ask first, and nothing changes until it confirms.
        assert_eq!(s.connect(c("mini"), false), Decision::AskFirst { in_use_by: c("air") });
        assert_eq!(s.client(), Some(&c("air")));
        assert_eq!(s.connect(c("mini"), true), Decision::TakeOver { from: c("air"), from_was_live: true });
        // The mini's connection drops: held, and the Air takes over without asking.
        let t0 = Instant::now();
        assert!(s.lost("mini", t0));
        assert_eq!(s.connect(c("air"), false), Decision::TakeOver { from: c("mini"), from_was_live: false });
        // The same device reconnecting resumes.
        assert_eq!(s.connect(c("air"), false), Decision::Resume { was_held: false });
    }

    #[test]
    fn a_lost_session_is_held_for_two_minutes() {
        let mut s = SessionCore::default();
        let t0 = Instant::now();
        s.connect(c("air"), false);
        assert!(!s.lost("mini", t0)); // not the live device
        assert!(s.lost("air", t0));
        assert_eq!(s.deadline(), Some(t0 + HOLD));
        assert_eq!(s.tick(t0 + HOLD - Duration::from_millis(1)), None);
        // Back within the hold: its own session resumes.
        assert_eq!(s.connect(c("air"), false), Decision::Resume { was_held: true });
        assert!(s.lost("air", t0));
        assert_eq!(s.tick(t0 + HOLD), Some(c("air")));
        assert_eq!(s.phase(), &Phase::Idle);
    }
}
