// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation

/// A paired computer's state on its card.
public enum HostState: Equatable {
    case online
    /// Online with the lock screen up (known from the last connection).
    case locked
    /// Another paired device is streaming it (known from a Busy answer).
    case busy(by: String)
    case asleep(since: Date)
    case offline
    case connecting
}

/// The card's second line and its path line.
public enum HostLine {
    /// The time of day in the user's own format.
    static func clock(_ d: Date) -> String {
        d.formatted(date: .omitted, time: .shortened)
    }

    /// The day and month, short, in the user's language.
    static func day(_ d: Date) -> String {
        d.formatted(.dateTime.day().month(.abbreviated))
    }

    /// "today, 09:12", "yesterday, 18:40", "3 Nov, 10:02".
    public static func when(_ d: Date, now: Date = Date()) -> String {
        let cal = Calendar.current
        let time = clock(d)
        if cal.isDate(d, inSameDayAs: now) { return String(localized: "today, \(time)") }
        if let y = cal.date(byAdding: .day, value: -1, to: now), cal.isDate(d, inSameDayAs: y) { return String(localized: "yesterday, \(time)") }
        let date = day(d)
        return String(localized: "\(date), \(time)")
    }

    /// The same, but only the time of day when it is today.
    public static func whenShort(_ d: Date, now: Date = Date()) -> String {
        Calendar.current.isDate(d, inSameDayAs: now) ? clock(d) : when(d, now: now)
    }

    /// "3 weeks ago".
    public static func ago(_ d: Date, now: Date = Date()) -> String {
        let f = RelativeDateTimeFormatter()
        f.locale = Locale.current
        f.unitsStyle = .full
        return f.localizedString(for: d, relativeTo: now)
    }

    /// The second line for a state.
    public static func line(_ state: HostState, lastConnected: Date?, lastSeen: Date?, now: Date = Date()) -> String {
        switch state {
        case .online:
            if let c = lastConnected { let w = when(c, now: now); return String(localized: "Last connected \(w)") }
            return String(localized: "Not connected yet")
        case .locked:
            return String(localized: "Locked: unlock in the stream")
        case .busy(let by):
            return String(localized: "In use by \(by)")
        case .asleep(let since):
            let w = whenShort(since, now: now)
            return String(localized: "Asleep since \(w)")
        case .offline:
            if let s = lastSeen { let a = ago(s, now: now); return String(localized: "Last seen \(a)") }
            return String(localized: "Offline")
        case .connecting:
            return String(localized: "Finding the best path…")
        }
    }

    /// "Direct · 9 ms", or "Last time: LAN · 2 ms" when it isn't online now.
    public static func path(_ route: RouteKind?, rttMs: Double?, current: Bool) -> String? {
        guard let r = route else { return nil }
        var s = r.label
        if let ms = rttMs { s = String(localized: "\(s) · \(Int(ms.rounded())) ms") }
        return current ? s : String(localized: "Last time: \(s)")
    }
}
