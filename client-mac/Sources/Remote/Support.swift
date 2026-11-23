// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation
import os
import QuartzCore
import RemoteCore
import RemoteKit
import RemoteUI

/// Which build this is. A test build (`build-app.sh --test-id`) has the bundle ID `com.sodiumos.remote.dev` and keeps
/// its data, its Keychain items and its logs apart from a release's, so it can never touch that app's pairings or
/// device key. One source builds both; nothing but the bundle ID differs. The constants are `Brand`'s (mirroring
/// remote/core/src/brand.rs); the data folder, Keychain service, log subsystem, queue labels and the defaults domain
/// all follow the bundle ID.
enum AppIdentity {
    /// A binary run outside an app bundle (`swift run`, the bare executable) has no bundle ID: it is never a release,
    /// so it takes the test identity rather than a release's data and Keychain items.
    static let bundleID = Bundle.main.bundleIdentifier ?? Brand.bundleIDDev
    static let isTestBuild = Brand.isTestBuild(bundleID: bundleID)
    /// The folder name under Application Support and Logs: the bundle ID (`com.sodiumos.remote`, or `….dev`).
    static let dataFolder = bundleID
    /// The preferences domain (`~/Library/Preferences/<id>.plist`): `UserDefaults.standard` uses it by itself.
    static let defaultsDomain = bundleID

    /// `<bundle ID>.<part>`: a dispatch queue, a toolbar or a notification identifier.
    static func label(_ part: String) -> String {
        Brand.label(part, bundleID: bundleID)
    }
}

/// The log: stderr (run the binary from a terminal to see it) and the
/// unified log (`log stream --predicate 'subsystem == "com.sodiumos.remote"'`, or `com.sodiumos.remote.dev` in the test build).
enum Log {
    static let logger = Logger(subsystem: AppIdentity.bundleID, category: "remote")

    static func info(_ s: String) {
        FileHandle.standardError.write(("remote: " + s + "\n").data(using: .utf8) ?? Data())
        logger.info("\(s, privacy: .public)")
    }

    static func error(_ s: String) {
        FileHandle.standardError.write(("remote: error: " + s + "\n").data(using: .utf8) ?? Data())
        logger.error("\(s, privacy: .public)")
    }
}

/// Run on the main thread, isolated to the main actor (from any thread).
func onMain(_ body: @escaping @MainActor () -> Void) {
    if Thread.isMainThread {
        MainActor.assumeIsolated { body() }
    } else {
        DispatchQueue.main.async {
            MainActor.assumeIsolated { body() }
        }
    }
}

/// Now, as `CACurrentMediaTime` (seconds of `mach_absolute_time`): the clock
/// of `NSEvent.timestamp`, `CAMetalDisplayLink` and `presentedTime`.
func hostNow() -> Double {
    CACurrentMediaTime()
}

/// Maps host time (CACurrentMediaTime seconds) onto the live session's clock
/// (µs, what the core's frame times and `Msg::Vsync` use) and onto the stats
/// recorder's clock (µs since its epoch). Thread-safe.
final class ClockMap {
    private let lock = NSLock()
    /// sessionUs − hostUs, set on every (re)connection (a new session clock).
    private var sessionOffsetUs: Double?
    /// The recorder's epoch, host seconds.
    let epochHost: Double

    init(epochHost: Double) {
        self.epochHost = epochHost
    }

    /// Read the core's session clock once, between two host reads.
    func sync(with core: RemoteClient) {
        let a = hostNow()
        guard let s = core.sessionClockUs() else { return }
        let b = hostNow()
        lock.lock()
        sessionOffsetUs = Double(s) - (a + b) / 2 * 1e6
        lock.unlock()
    }

    var synced: Bool {
        lock.lock()
        defer { lock.unlock() }
        return sessionOffsetUs != nil
    }

    /// Host seconds → session µs.
    func sessionUs(host t: Double) -> Int64? {
        lock.lock()
        defer { lock.unlock() }
        guard let o = sessionOffsetUs else { return nil }
        return Int64(t * 1e6 + o)
    }

    /// Session µs → recorder µs.
    func recorderUs(session s: Int64) -> Int64 {
        lock.lock()
        let o = sessionOffsetUs ?? 0
        lock.unlock()
        return Int64(Double(s) - o - epochHost * 1e6)
    }

    /// Host seconds → recorder µs.
    func recorderUs(host t: Double) -> Int64 {
        Int64((t - epochHost) * 1e6)
    }
}

// MARK: - Core ↔ RemoteKit / RemoteUI

extension RouteKind {
    init(_ p: Path) {
        switch p {
        case .lan: self = .lan
        case .direct: self = .direct
        case .vpn: self = .vpn
        case .tailscale: self = .tailscale
        case .relayUdp: self = .relayUDP
        case .relayTcp: self = .relayTCP
        case .relayTls: self = .relayTLS
        }
    }
}

extension KeyRuleModel {
    init(_ r: KeyRule) {
        let when: KeyConditionModel
        switch r.when {
        case .any: when = .any
        case .terminal: when = .terminal
        case .class(let c): when = .appClass(c)
        }
        self.init(when: when, from: KeyChordModel(mods: r.from.mods, key: r.from.key),
                  to: r.to.map { KeyChordModel(mods: $0.mods, key: $0.key) })
    }

    var core: KeyRule {
        let w: KeyCondition
        switch when {
        case .any: w = .any
        case .terminal: w = .terminal
        case .appClass(let c): w = .class(c)
        }
        return KeyRule(when: w, from: KeyChord(mods: from.mods, key: from.key), to: to.map { KeyChord(mods: $0.mods, key: $0.key) })
    }
}

extension HostStatsSnapshot {
    init(_ h: HostStats) {
        self.init(captureUs: h.captureUs, convertUs: h.convertUs, encodeUs: h.encodeUs, sendUs: h.sendUs, fps: h.fps,
                  bitrateBps: h.bitrateBps, qp: h.qp, optimisedLook: h.optimisedLook, readableMs: h.readableMs, losslessMs: h.losslessMs,
                  settledMs: h.settledMs, settledEnd: h.settledEnd,
                  targetBps: h.targetBps, cEstBps: h.cEstBps, probeCeilingBps: h.probeCeilingBps)
    }
}

extension HomeMode {
    var core: HomeMonitorMode {
        switch self {
        case .mirror: return .mirror
        case .privacy: return .privacy
        case .off: return .off
        }
    }

    init(_ m: HomeMonitorMode) {
        switch m {
        case .mirror: self = .mirror
        case .privacy: self = .privacy
        case .off: self = .off
        }
    }
}

extension UISizeChoice {
    var core: UiSize {
        switch self {
        case .matchMac: return .matchMac
        case .larger: return .larger
        case .smaller: return .smaller
        case .sameAsDesk: return .sameAsDesk
        }
    }
}

extension Codec {
    var label: String {
        switch self {
        case .hevc420: return "HEVC 4:2:0"
        case .hevc444: return "HEVC 4:4:4"
        case .av1: return "AV1 4:2:0"
        case .h264: return "H.264 4:2:0"
        }
    }
}

/// A disconnect reason as the Disconnected card's title and text. `os` is what the computer runs when this Mac
/// knows (a remembered `Welcome.host_os`): it names the place to act in the control-switched-off card.
func disconnectedCard(_ r: DisconnectReason, host: String, os: HostOS? = nil) -> StreamCard {
    switch r {
    case .holdExpired:
        return .disconnected(title: "Disconnected", body: "\(host)’s 2-minute hold ended, so it put the desk back. Reconnect starts a new session.")
    case .hostShutdown:
        return .disconnected(title: "Disconnected", body: "Remote stopped on \(host). Reconnect when it’s back.")
    case .hostSleeping:
        return .disconnected(title: "\(host) went to sleep", body: "It told this Mac it was going to sleep. Reconnect once it’s awake.")
    case .endedAtComputer:
        return .disconnected(title: "Ended at the computer", body: "Someone typed the password at \(host), which ends the remote session.")
    case .hostLocked:
        // The computer's screen is locked (generic Linux): the typed reason, consent and the saved choice stay on that computer.
        return .disconnected(title: String(localized: "\(host) is locked"), body: String(localized: "Unlock it at that computer, then reconnect."))
    case .user:
        return .disconnected(title: "Disconnected", body: "\(host) ended the session.")
    case .error(let e):
        return .disconnected(title: "Disconnected", body: e)
    case .controlOff:
        // The typed reason: "Allow remote control" is off on the host. No text is matched.
        return .controlOff(os: os)
    case .permissionMissing(let screenRecording, let accessibility):
        // A Mac host without a permission turns this Mac away; the flags say which one (either or both).
        return .hostNeedsPermission(screenRecording: screenRecording, accessibility: accessibility)
    case .takenOver(let by):
        return .takenOver(by: by, at: HostLine.when(Date()).replacingOccurrences(of: "today, ", with: ""))
    case .revoked:
        return .removed
    case .versionMismatch(let h, let c):
        return .versionMismatch(host: Int(h), client: Int(c), updateThisMac: versionMismatchUpdateSide(host: h, client: c) == .client)
    }
}
