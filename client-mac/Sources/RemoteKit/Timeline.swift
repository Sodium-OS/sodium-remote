// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation

/// The kinds of route the path chip names (LAN · direct · VPN ·
/// Tailscale · relay; and relay over TCP).
public enum RouteKind: String, Equatable, Codable, CaseIterable {
    case lan, direct, vpn, tailscale, relayUDP, relayTCP, relayTLS

    /// The chip's and the HUD's label (the boards' `PATHS`).
    public var label: String {
        switch self {
        case .lan: return "LAN"
        case .direct: return String(localized: "Direct")
        case .vpn: return "VPN"
        case .tailscale: return "Tailscale"
        case .relayUDP: return String(localized: "Relay")
        case .relayTCP, .relayTLS: return String(localized: "Relay · TCP")
        }
    }

    /// The stats file's name (client-linux's `route_name`).
    public var statsName: String {
        switch self {
        case .lan: return "LAN"
        case .direct: return "direct"
        case .vpn: return "VPN"
        case .tailscale: return "Tailscale"
        case .relayUDP: return "relay udp"  // i18n:ok
        case .relayTCP: return "relay tcp"  // i18n:ok
        case .relayTLS: return "relay tls"  // i18n:ok
        }
    }

    public var isRelay: Bool { self == .relayUDP || self == .relayTCP || self == .relayTLS }
    /// A relay over TCP or TLS: always amber, and "poor".
    public var isTCPRelay: Bool { self == .relayTCP || self == .relayTLS }
}

/// The path quality bands: good under 20 ms, fair 20–40 ms,
/// poor over 40 ms or any TCP relay.
public enum QualityBand: Equatable {
    case good, fair, poor, none

    public static func of(rttMs: Double?, route: RouteKind?) -> QualityBand {
        if route?.isTCPRelay == true { return .poor }
        guard let r = rttMs else { return .none }
        if r < 20 { return .good }
        if r <= 40 { return .fair }
        return .poor
    }

    /// Bars lit, out of three.
    public var bars: Int {
        switch self {
        case .good: return 3
        case .fair: return 2
        case .poor: return 1
        case .none: return 0
        }
    }
}

public enum Tone: Equatable {
    case normal, amber, red
}

/// The live connection, one sample of the core's `link()`.
public struct LinkSample: Equatable {
    public var route: RouteKind?
    public var routeRttMs: Double?
    public var hedgeRoute: RouteKind?
    public var hedging: Bool
    public var healthy: Bool
    /// Since anything authenticated arrived from the host (t₀), seconds.
    public var silence: Double

    public init(route: RouteKind?, routeRttMs: Double?, hedgeRoute: RouteKind?, hedging: Bool, healthy: Bool, silence: Double) {
        self.route = route
        self.routeRttMs = routeRttMs
        self.hedgeRoute = hedgeRoute
        self.hedging = hedging
        self.healthy = healthy
        self.silence = silence
    }
}

/// What the path chip and the pill show.
public struct LinkView: Equatable {
    public struct Chip: Equatable {
        /// The route to name; nil with `lost`.
        public var route: RouteKind?
        public var rttMs: Double?
        public var lost: Bool
        public var tone: Tone

        public var text: String {
            if lost { return String(localized: "Lost") }
            guard let r = route else { return "" }
            if let ms = rttMs, tone == .normal { let l = r.label; return String(localized: "\(l) · \(Int(ms.rounded())) ms") }
            return r.label
        }
    }

    public enum Pill: Equatable {
        case reconnecting
        case lost
        case waitingForNetwork

        public var text: String {
            switch self {
            case .reconnecting: return String(localized: "Reconnecting…")
            case .lost: return String(localized: "Connection lost · retrying")
            case .waitingForNetwork: return String(localized: "Waiting for the network…")
            }
        }
    }

    public var chip: Chip?
    public var pill: Pill?
    /// Seconds since t₀ (0 while packets arrive), for the pill's count.
    public var since: Double

    public init(chip: Chip?, pill: Pill?, since: Double) {
        self.chip = chip
        self.pill = pill
        self.since = since
    }
}

/// The one "Reconnecting" timeline, as client-linux's
/// `status.rs`: from the time since the last packet from the host (t₀).
///
/// | Since t₀ | Chip | Pill |
/// |---|---|---|
/// | frames arriving | amber while hedging; the new route after a switch | none |
/// | under 1 s | as above | none |
/// | 1–30 s | amber, following the route | "Reconnecting…" |
/// | 30 s until the hold ends | red "Lost" | "Connection lost · retrying" |
///
/// With no network at the Mac's end the pill reads "Waiting for the
/// network…" from 1 s on, at any stage. The Disconnected card comes only with
/// the core's `Disconnected` event, never from here.
public struct ReconnectTimeline {
    /// From here the pill says "Reconnecting…".
    public static let pillAfter: Double = 1
    /// From here the chip is a red "Lost".
    public static let lostAfter: Double = 30

    /// t₀ of the current outage, on the caller's monotonic clock (seconds).
    public var lostAt: Double?
    /// The last route, named while reconnecting.
    public var lastRoute: RouteKind?
    public var lastRtt: Double?
    /// The session ended (`Disconnected`): no chip, no pill.
    public var ended = false

    public init() {}

    /// The chip and the pill `since` t₀ (zero while packets arrive). `route`
    /// is the one to name; `amber` is hedging, an unhealthy link or a TCP relay.
    public static func view(since: Double, route: RouteKind?, rttMs: Double?, amber: Bool, network: Bool) -> LinkView {
        let chip: LinkView.Chip?
        if since >= lostAfter {
            chip = LinkView.Chip(route: nil, rttMs: nil, lost: true, tone: .red)
        } else if let r = route {
            chip = LinkView.Chip(route: r, rttMs: rttMs, lost: false, tone: (amber || since >= pillAfter) ? .amber : .normal)
        } else {
            chip = nil
        }
        let pill: LinkView.Pill?
        if since < pillAfter {
            pill = nil
        } else if !network {
            pill = .waitingForNetwork
        } else if since < lostAfter {
            pill = .reconnecting
        } else {
            pill = .lost
        }
        return LinkView(chip: chip, pill: pill, since: since)
    }

    /// The core said `Reconnecting { since }` at `now`.
    public mutating func reconnecting(sinceSeconds: Double, now: Double) {
        lostAt = now - sinceSeconds
    }

    /// A connection came up.
    public mutating func connected() {
        lostAt = nil
        ended = false
    }

    /// The view now: from the live link while there is one, else from t₀.
    public mutating func now(live: LinkSample?, now: Double, network: Bool) -> LinkView? {
        if ended { return nil }
        if let l = live {
            if let r = l.route {
                lastRoute = r
                lastRtt = l.routeRttMs
            }
            // The hedge route while hedging; the last route while there is none.
            let named = (l.hedging ? (l.hedgeRoute ?? l.route) : l.route) ?? lastRoute
            let tcp = named?.isTCPRelay ?? false
            let amber = l.hedging || !l.healthy || l.route == nil || tcp
            let rtt = (named == l.route) ? l.routeRttMs : nil
            return ReconnectTimeline.view(since: l.silence, route: named, rttMs: rtt, amber: amber, network: network)
        }
        if let t0 = lostAt {
            return ReconnectTimeline.view(since: max(0, now - t0), route: lastRoute, rttMs: nil, amber: true, network: network)
        }
        return LinkView(chip: nil, pill: nil, since: 0)
    }
}
