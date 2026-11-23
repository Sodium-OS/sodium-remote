// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The "Reconnecting" timeline: the path chip and the pill, from
//! the time since the last packet from the host (t₀).
//!
//! PROVISIONAL: shown as plain text in the HUD and logged one line per change
//! (`remote-client: link …`); the designed chip and pill are the Mac client's

use remote_core::connect::PathKind;
use remote_core::health::LinkHealth;
use remote_core::path::{Route, TurnTransport};
use std::time::{Duration, Instant};

/// From here the pill says "Reconnecting…".
pub const PILL_AFTER: Duration = Duration::from_secs(1);
/// From here the chip is a red "Lost" and the pill "Connection lost · retrying".
pub const LOST_AFTER: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Amber,
    Red,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// The path chip: its text and tone (None before the first connection).
    pub chip: Option<(String, Tone)>,
    pub pill: Option<&'static str>,
}

impl std::fmt::Display for View {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.chip {
            Some((t, tone)) => write!(f, "chip \"{t}\" {}", format!("{tone:?}").to_lowercase())?,
            None => write!(f, "chip none")?,
        }
        match self.pill {
            Some(p) => write!(f, " · pill \"{p}\""),
            None => write!(f, " · pill none"),
        }
    }
}

pub fn kind_name(k: &PathKind) -> &'static str {
    match k {
        PathKind::Lan => "LAN",
        PathKind::Direct => "direct",
        PathKind::Vpn => "VPN",
        PathKind::Tailscale => "Tailscale",
        PathKind::Relay { .. } => "relay",
    }
}

/// The live connection as `Client::link_status` shows it.
#[derive(Clone, Debug)]
pub struct LinkView {
    pub route: Option<PathKind>,
    pub hedge_route: Option<PathKind>,
    pub hedging: bool,
    pub healthy: bool,
    /// Since anything authenticated arrived (t₀).
    pub silence: Duration,
}

/// The chip and the pill, `since` t₀ (zero while packets arrive).
/// `route` is the route to name (the hedge route while hedging; the last one
/// while reconnecting); `amber` is hedging, an unhealthy link, or a TCP relay.
pub fn view(since: Duration, route: Option<&PathKind>, amber: bool, network: bool) -> View {
    let chip = if since >= LOST_AFTER {
        Some(("Lost".to_string(), Tone::Red))
    } else {
        route.map(|k| (kind_name(k).to_string(), if amber || since >= PILL_AFTER { Tone::Amber } else { Tone::Normal }))
    };
    let pill = if since < PILL_AFTER {
        None
    } else if !network {
        Some("Waiting for the network…")
    } else if since < LOST_AFTER {
        Some("Reconnecting…")
    } else {
        Some("Connection lost · retrying")
    };
    View { chip, pill }
}

/// What the dispatcher knows between `Client::link_status` samples.
#[derive(Default)]
pub struct Timeline {
    /// t₀ of the current outage (from `ClientEvent::Reconnecting { since }`).
    pub lost_at: Option<Instant>,
    /// The last route, named while reconnecting.
    pub last_route: Option<PathKind>,
    pub ended: bool,
    /// The last view shown (to log changes).
    pub shown: Option<View>,
}

impl Timeline {
    /// The view now: from the live link while there is one, else from t₀.
    pub fn now(&mut self, live: Option<LinkView>, network: bool) -> Option<View> {
        if self.ended {
            return None;
        }
        Some(match live {
            Some(l) => {
                if let Some(r) = &l.route {
                    self.last_route = Some(r.clone());
                }
                // The hedge route while hedging; the last route while there is none.
                let named = if l.hedging { l.hedge_route.as_ref().or(l.route.as_ref()) } else { l.route.as_ref() };
                let named = named.or(self.last_route.as_ref());
                let tcp = matches!(named, Some(PathKind::Relay { transport: TurnTransport::Tcp | TurnTransport::Tls }));
                view(l.silence, named, l.hedging || !l.healthy || l.route.is_none() || tcp, network)
            }
            None => match self.lost_at {
                Some(t0) => view(t0.elapsed(), self.last_route.as_ref(), true, network),
                None => View { chip: None, pill: None },
            },
        })
    }
}

impl LinkView {
    /// `kinds` remembers each route's kind (`route_info` reads the interfaces).
    pub fn from_status(s: &remote_core::driver::LinkStatus, kinds: &mut Vec<(Route, PathKind)>) -> LinkView {
        let mut kind = |r: &Option<Route>| {
            let r = r.as_ref()?;
            if let Some((_, k)) = kinds.iter().find(|(c, _)| c.local == r.local && c.remote == r.remote) {
                return Some(k.clone());
            }
            let k = remote_core::connect::route_info(r).kind;
            kinds.push((r.clone(), k.clone()));
            Some(k)
        };
        LinkView {
            route: kind(&s.route),
            hedge_route: kind(&s.hedge_route),
            hedging: s.health.hedging,
            healthy: s.health.state == LinkHealth::Healthy,
            silence: Duration::from_millis(s.health.silence_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thresholds() {
        let lan = PathKind::Lan;
        let s = |ms| Duration::from_millis(ms);
        assert_eq!(view(s(999), Some(&lan), false, true), View { chip: Some(("LAN".into(), Tone::Normal)), pill: None });
        assert_eq!(view(s(1000), Some(&lan), false, true).pill, Some("Reconnecting…"));
        assert_eq!(view(s(1000), Some(&lan), false, true).chip, Some(("LAN".into(), Tone::Amber)));
        assert_eq!(view(s(1000), Some(&lan), false, false).pill, Some("Waiting for the network…"));
        let lost = view(s(30_000), Some(&lan), true, true);
        assert_eq!((lost.chip, lost.pill), (Some(("Lost".into(), Tone::Red)), Some("Connection lost · retrying")));
    }
}
