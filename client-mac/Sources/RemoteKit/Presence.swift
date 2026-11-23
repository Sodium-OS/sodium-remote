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
    static func clock(_ d: Date) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_GB")
        f.dateFormat = "HH:mm"
        return f.string(from: d)
    }

    static func day(_ d: Date) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_GB")
        f.dateFormat = "d MMM"
        return f.string(from: d)
    }

    /// "today, 09:12", "yesterday, 18:40", "3 Nov, 10:02".
    public static func when(_ d: Date, now: Date = Date()) -> String {
        let cal = Calendar.current
        if cal.isDate(d, inSameDayAs: now) { return "today, " + clock(d) }
        if let y = cal.date(byAdding: .day, value: -1, to: now), cal.isDate(d, inSameDayAs: y) { return "yesterday, " + clock(d) }
        return day(d) + ", " + clock(d)
    }

    /// "3 weeks ago".
    public static func ago(_ d: Date, now: Date = Date()) -> String {
        let f = RelativeDateTimeFormatter()
        f.locale = Locale(identifier: "en_GB")
        f.unitsStyle = .full
        return f.localizedString(for: d, relativeTo: now)
    }

    /// The second line for a state.
    public static func line(_ state: HostState, lastConnected: Date?, lastSeen: Date?, now: Date = Date()) -> String {
        switch state {
        case .online:
            if let c = lastConnected { return "Last connected " + when(c, now: now) }
            return "Not connected yet"
        case .locked:
            return "Locked: unlock in the stream"
        case .busy(let by):
            return "In use by \(by)"
        case .asleep(let since):
            let cal = Calendar.current
            return "Asleep since " + (cal.isDate(since, inSameDayAs: now) ? clock(since) : day(since) + ", " + clock(since))
        case .offline:
            if let s = lastSeen { return "Last seen " + ago(s, now: now) }
            return "Offline"
        case .connecting:
            return "Finding the best path…"
        }
    }

    /// "Direct · 9 ms", or "Last time: LAN · 2 ms" when it isn't online now.
    public static func path(_ route: RouteKind?, rttMs: Double?, current: Bool) -> String? {
        guard let r = route else { return nil }
        var s = r.label
        if let ms = rttMs { s += " · \(Int(ms.rounded())) ms" }
        return current ? s : "Last time: " + s
    }
}
