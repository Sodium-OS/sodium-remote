// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import CoreImage
import Foundation
import RemoteCore
import RemoteKit
import RemoteUI

/// A display's facts for `ClientDisplay`: the
/// stream is in the window's backing pixels, so `pixelsPerPoint` is the
/// screen's backing scale (2 on every Retina Mac, whatever "looks like" it
/// is set to), its refresh rate, and the size full screen would stream, in
/// backing pixels below the camera (2940 × 1848 on the 13" Air).
struct DisplayInfo {
    var pixelsPerPoint: Double
    var hz: Int
    var fullPixels: PixelSize

    @MainActor
    static func of(_ screen: NSScreen?) -> DisplayInfo {
        guard let s = screen ?? NSScreen.main else { return DisplayInfo(pixelsPerPoint: 2, hz: 60, fullPixels: PixelSize(width: 2940, height: 1848)) }
        let scale = Double(s.backingScaleFactor)
        let points = s.frame.size
        let usableHeight = Double(points.height) - Double(s.safeAreaInsets.top)
        let full = PixelSize(width: Int((Double(points.width) * scale).rounded(.down)), height: Int((usableHeight * scale).rounded(.down)))
        let hz = s.maximumFramesPerSecond > 0 ? s.maximumFramesPerSecond : 60
        return DisplayInfo(pixelsPerPoint: scale, hz: hz, fullPixels: full)
    }
}

/// One stream window and its connection: the connect flow and its cards
/// Every core event, the timeline, the HUD,
/// input (StreamInputSink), the clipboard, audio and the latency test.
@MainActor
final class StreamSession: NSObject, StreamActions, StreamInputSink, StreamWindowOwner {
    unowned let app: AppController
    let hostID: String
    let core: RemoteClient
    let model: StreamModel
    let window: StreamWindowController
    let clock: ClockMap
    let stats: StatsRecorder
    private(set) var pipeline: MediaPipeline?
    private var audio: AudioPlayer?
    private let clipboard: ClipboardSync

    private var timeline = ReconnectTimeline()
    private var timer: Timer?
    private var ticks = 0
    private var connectCardTimer: Timer?
    private var retryTimer: Timer?
    private var retryDelay = 10
    private var retryAt: Double?
    private var connecting = false
    private(set) var connected = false
    private var everConnected = false
    private var firstFrame = false
    private var ended = false
    private var closing = false

    private var format: StreamFormat?
    private var display = DisplayInfo(pixelsPerPoint: 2, hz: 60, fullPixels: PixelSize(width: 2940, height: 1848))
    private var sentDisplay: ClientDisplay?
    /// `Welcome.liveResize`: false means the host's display has a fixed size, so never send `Resize` and fit the picture.
    private var hostLiveResize = true
    /// `Welcome.hostOs`: Mac to Mac is a passthrough (an empty key table); the secure-screen wording follows it.
    private var hostOS: HostOS?
    private var route: Route?
    private var hedge: Route?
    private var link = LinkStats(rttMs: 0, loss: 0, targetBps: 0, receiveRateBps: 0, fecRatio: 0, framesReceived: 0, framesLost: 0, mtu: 0,
                                 hedging: false, bytesReceived: 0)
    private var decoderText = ""
    private var hardware = false
    private var awdl = false

    private var caret: Caret?
    private(set) var passwordFocus = false
    private var pointer: PointerLock = PointerLock.none
    private var cursorImages: [UInt64: CursorImage] = [:]
    private var cursorShape: UInt64 = 0
    private var cursorVisible = true
    private var takenOverBy: String?
    private var lastDisconnect: DisconnectReason?
    private(set) var capturingKeys = true
    private var keyFocus = false
    private var clickTest: ClickTest?
    private var lockedToastShown = false

    init?(app: AppController, hostID: String, name: String, core: RemoteClient, statsDir: URL, keep: Int) {
        self.app = app
        self.hostID = hostID
        self.core = core
        let epoch = hostNow()
        clock = ClockMap(epochHost: epoch)
        do {
            stats = try StatsRecorder(directory: statsDir, epochNs: UInt64(epoch * 1e9), keep: keep)
        } catch {
            Log.error("stats file: \(error)")
            return nil
        }
        let m = StreamModel()
        model = m
        window = StreamWindowController(model: m)
        clipboard = ClipboardSync(core: core)
        super.init()
        m.hostName = name
        m.actions = self
        window.streamOwner = self
        window.streamView.sink = self
        guard let presenter = Presenter(layer: window.streamView.metalLayer),
              let p = MediaPipeline(core: core, clock: clock, stats: stats, presenter: presenter) else {
            Log.error("Metal or VideoToolbox isn't available")
            return nil
        }
        pipeline = p
        p.onDecoderChanged = { [weak self] text, hw in
            onMain { self?.decoderChanged(text, hw) }
        }
        p.onFirstPresent = { [weak self] in
            onMain { self?.firstPresented() }
        }
        audio = AudioPlayer(core: core)
        Log.info("stats in \(stats.path("*").path)")
    }

    // MARK: - Opening and connecting

    func open(fullScreen: Bool) {
        window.showWindow(nil)
        window.window?.makeFirstResponder(window.streamView)
        display = DisplayInfo.of(window.window?.screen)
        pipeline?.setRefresh(hz: Double(display.hz))
        pipeline?.presenter.start(preferredHz: display.hz)
        let s = app.model.settings
        model.hudMode = s.hud
        model.keyboard = s.keyboard
        model.homeMode = s.homeMode
        layoutChanged()
        startTimers()
        if fullScreen && !window.isFullScreen {
            window.toggleFullScreen()
        }
        connect(takeover: false)
    }

    private func hello() -> Hello {
        let s = app.model.settings
        // What this Mac decodes, as it is: the HEVC choice is `Settings.codec`, not a denied capability.
        // VideoToolbox decodes HEVC 4:2:0 and H.264 on every supported Mac.
        let decode = DecodeSupport(hevc: true, h264: true, hevc444: app.hevc444Hardware, av1: app.av1Hardware, maxWidth: 0, maxHeight: 0)
        return Hello(version: protocolVersion(), name: "", display: currentDisplay(), decode: decode, settings: streamSettings(s), clientOs: .mac)
    }

    private func streamSettings(_ s: RemoteSettings) -> StreamSettings {
        let codec: CodecChoice
        switch s.codec {
        case .auto: codec = .auto
        case .hevc: codec = .hevc420
        case .av1: codec = .av1
        }
        return StreamSettings(refreshMhz: UInt32(max(0, s.refreshHz) * 1000), uiSize: s.uiSize.core, codec: codec,
                              bandwidthLimitKbps: s.bandwidthLimitKbps, optimisedLook: s.optimisedLook)
    }

    func connect(takeover: Bool) {
        guard !connecting else { return }
        connecting = true
        ended = false
        retryTimer?.invalidate()
        retryTimer = nil
        retryAt = nil
        if model.card != nil && model.card != .connecting { model.card = nil }
        model.progress = ConnectProgress()
        // Where the computer is looked for: the configured server's name, or just this network (no server).
        model.serverName = app.model.serverName
        model.progress.findDetail = app.model.serverName.map { String(localized: "through \($0)") } ?? String(localized: "on this network")
        let via = app.network.interfaceName
        model.progress.network = via.isEmpty ? "" : String(localized: "From \(via) · usually under 2 s")
        model.progress.sizeDetail = "\(currentDisplay().width) × \(currentDisplay().height) · \(display.hz) Hz"
        firstFrame = false
        connectCardTimer?.invalidate()
        connectCardTimer = Timer.scheduledTimer(withTimeInterval: 0.6, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self = self, !self.firstFrame, self.model.card == nil else { return }
                self.model.card = .connecting
            }
        }
        stats.onConnectStart(nowUs: clock.recorderUs(host: hostNow()))
        app.listener.setVideoSink(pipeline)
        let h = hello()
        let id = hostID
        let c = core
        Log.info("connecting to \(deviceCode(id: id))\(takeover ? " (taking over)" : "")")
        Task { [weak self] in
            do {
                try await c.connect(host: id, hello: h, takeover: takeover)
                self?.connectReturned()
            } catch let e as RemoteError {
                self?.connectFailed(e)
            } catch {
                self?.connectFailed(.Failed(message: "\(error)"))
            }
        }
    }

    private func connectReturned() {
        connecting = false
        guard !ended else { return }
        // The computer answered: the path and the key are next.
        model.progress.step = max(model.progress.step, 1)
        model.progress.findDetail = "done"
        sendKeyTable()
        // The window may have changed while the core took the target.
        if let d = sentDisplay, d != currentDisplay() { sendResize() }
    }

    private func connectFailed(_ e: RemoteError) {
        connecting = false
        connectCardTimer?.invalidate()
        guard !ended else { return }
        Log.info("couldn't connect: \(e)")
        switch e {
        case .Busy(let by):
            app.store.update(hostID) { $0.busyBy = by }
            model.card = nil
            app.askTakeover(by: by, session: self)
        case .Offline:
            showOffline()
        case .NotPaired:
            model.card = .removed
        case .Refused(let reason):
            model.card = disconnectedCard(reason, host: model.hostName, os: hostOS ?? app.store.hostMeta(hostID).hostOS)
        case .Timeout:
            model.card = .failed(String(localized: "\(model.hostName) didn’t answer in time."))
            scheduleRetry()
        case .Failed(let message):
            model.card = .failed(message)
        case .WrongCode, .PairingClosed:
            model.card = .failed("\(e)")
        }
    }

    private func showOffline() {
        let meta = app.store.hostMeta(hostID)
        if let since = app.asleepSince(hostID) {
            model.card = .asleep(since: HostLine.whenShort(since), retryIn: retryDelay)
        } else if !app.network.satisfied {
            model.card = .macOffline
        } else {
            model.card = .offline(lastSeen: meta.lastSeen.map { HostLine.ago($0) }, retryIn: retryDelay)
        }
        scheduleRetry()
    }

    /// Offline: retry every 10 s, backing off to 60 s, while the window is open.
    private func scheduleRetry() {
        retryTimer?.invalidate()
        retryAt = hostNow() + Double(retryDelay)
        let delay = retryDelay
        retryDelay = min(retryDelay * 2, 60)
        retryTimer = Timer.scheduledTimer(withTimeInterval: Double(delay), repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.connect(takeover: false) }
        }
    }

    private func sendKeyTable() {
        let s = app.model.settings
        // Mac to Mac: Cmd is already Cmd (core's `default_rules(Mac, Mac)` is empty), so nothing is mapped.
        let rules: [KeyRule] = s.keyboard == .mac && hostOS != .mac ? app.model.keyTable.rules.map { $0.core } : []
        core.sendControl(msg: .keyTable(rules))
    }

    // MARK: - Settings (applied at once)

    /// Only what changed goes to the computer. The codec applies at once too:
    /// `Msg.settings` carries it, and the computer restarts its media and
    /// sends a new `Format`.
    func settingsChanged(from old: RemoteSettings, to s: RemoteSettings) {
        model.hudMode = s.hud
        model.keyboard = s.keyboard
        if connected {
            if streamSettings(old) != streamSettings(s) {
                core.sendControl(msg: .settings(streamSettings(s)))
            }
            if old.keyboard != s.keyboard || old.keyTable != s.keyTable {
                sendKeyTable()
            }
            if old.homeMode != s.homeMode {
                model.homeMode = s.homeMode
                core.sendControl(msg: .homeMonitor(s.homeMode.core))
            }
        }
        if old.resolution != s.resolution || old.fixedSize != s.fixedSize || old.customSize != s.customSize
            || old.fixedResolution != s.fixedResolution {
            if !s.fixedResolution { sentDisplay = nil }
            layoutChanged()
            if connected { sendResize() }
        }
        if old.playSound != s.playSound || old.outputDevice != s.outputDevice {
            audio?.stop()
            if s.playSound && connected { audio?.start(deviceUID: s.outputDevice) }
        }
        if old.systemShortcuts != s.systemShortcuts { updateShortcutTap() }
    }

    // MARK: - Size

    /// The stream view's size in backing pixels: the drawable, and what the
    /// stream is asked to be in a window.
    private var backingSize: CGSize {
        window.streamView.convertToBacking(window.streamView.bounds).size
    }

    func currentDisplay() -> ClientDisplay {
        let s = app.model.settings
        let ppp = display.pixelsPerPoint
        let b = backingSize
        var w = Int(Double(b.width).rounded(.down))
        var h = Int(Double(b.height).rounded(.down))
        var full = display.fullPixels
        switch s.resolution {
        case .matchWindow:
            break
        case .matchDisplay:
            w = full.width
            h = full.height
        case .fixed:
            w = s.fixedSize.width
            h = s.fixedSize.height
            full = s.fixedSize
        case .custom:
            w = s.customSize.width
            h = s.customSize.height
            full = s.customSize
        }
        return ClientDisplay(width: UInt32(max(w, 64)), height: UInt32(max(h, 64)), pixelsPerPoint: Float(ppp),
                             refreshMhz: UInt32(display.hz * 1000), fullscreen: window.isFullScreen,
                             displayWidth: UInt32(max(full.width, 64)), displayHeight: UInt32(max(full.height, 64)))
    }

    private func sendResize() {
        let d = currentDisplay()
        if !hostLiveResize { return }
        if app.model.settings.fixedResolution && sentDisplay != nil { return }
        if d == sentDisplay { return }
        sentDisplay = d
        core.sendControl(msg: .resize(d))
    }

    /// The view's size or scale changed: the drawable follows, and the
    /// computer's size with it (debounced there, 150 ms).
    private func layoutChanged() {
        let b = backingSize
        let size = CGSize(width: Double(b.width).rounded(), height: Double(b.height).rounded())
        let bg = Theme.letterboxNS.usingColorSpace(.sRGB) ?? NSColor.black
        pipeline?.presenter.setLayout(drawableSize: size, fitAlways: !hostLiveResize,
                                      background: SIMD4<Double>(Double(bg.redComponent), Double(bg.greenComponent), Double(bg.blueComponent), 1))
        if sentDisplay == nil {
            sentDisplay = currentDisplay()
        } else if connected || connecting {
            sendResize()
        }
        applyCursor()
    }

    private func updateIncrements() {
        guard let f = format else { return }
        window.setResizeIncrement(points: StreamLayout.resizeIncrement(scale: Double(f.scale), backingScale: display.pixelsPerPoint))
    }

    /// Where the stream is in the drawable (physical pixels).
    private func placement() -> (Placement, PixelSize)? {
        guard let f = format else { return nil }
        let b = backingSize
        let target = PixelSize(width: Int(Double(b.width).rounded()), height: Int(Double(b.height).rounded()))
        let stream = PixelSize(width: Int(f.width), height: Int(f.height))
        return (StreamLayout.place(window: target, stream: stream), stream)
    }

    // MARK: - Core events

    func handle(_ e: RemoteEvent) {
        switch e {
        case .connected(let r):
            onConnected(r)
        case .control(let m):
            onControl(m)
        case .cursorShape(let hash, let image):
            cursorImages[hash] = image
            if hash == cursorShape { applyCursor() }
        case .cursorPosition(_, _, let visible, let shape):
            if shape != cursorShape || visible != cursorVisible {
                cursorShape = shape
                cursorVisible = visible
                applyCursor()
            }
        case .routeChanged(let r):
            onRoute(r)
        case .stats(let s):
            link = s
            stats.onLink(rttMs: s.rttMs, loss: s.loss, mtu: Int(s.mtu), bytesReceived: s.bytesReceived, nowUs: clock.recorderUs(host: hostNow()))
        case .reconnecting(let ms):
            if connected { Log.info("connection lost \(Double(ms) / 1000) s ago; reconnecting") }
            connected = false
            timeline.reconnecting(sinceSeconds: Double(ms) / 1000, now: hostNow())
        case .disconnected(let reason):
            onDisconnected(reason)
        case .videoFrame, .videoFrameLost, .presence, .revoked, .rendezvous:
            break
        }
    }

    private func onConnected(_ r: Route?) {
        connected = true
        connecting = false
        everConnected = true
        retryDelay = 10
        clock.sync(with: core)
        pipeline?.reconnected()
        stats.onConnected(nowUs: clock.recorderUs(host: hostNow()))
        timeline.connected()
        if let route = r { onRoute(route) }
        model.progress.step = max(model.progress.step, 3)
        model.progress.keyDetail = "done"
        if case .some(.offline) = model.card { model.card = .connecting }
        if case .some(.asleep) = model.card { model.card = .connecting }
        if case .some(.failed) = model.card { model.card = .connecting }
        app.store.update(hostID) { m in
            m.lastConnected = Date()
            m.lastSeen = Date()
            m.busyBy = nil
        }
        clipboard.start()
        if app.model.settings.playSound { audio?.start(deviceUID: app.model.settings.outputDevice) }
        updateShortcutTap()
        Log.info("connected" + (r.map { " through \(RouteKind($0.path).statsName) \($0.local) -> \($0.remote)" } ?? ""))
    }

    private func onControl(_ m: Msg) {
        switch m {
        case .welcome(let w):
            Log.info("welcome from \(w.name): \(w.format.width)×\(w.format.height) @\(Double(w.format.refreshMhz) / 1000) Hz \(w.format.codec.label), scale \(w.format.scale) of \(w.scales)")
            Log.info("host is \(w.hostOs), \(w.displayMode) display mode, live resize \(w.liveResize), display \(w.currentDisplay) of \(w.displays.map { "\($0.name) \($0.width)×\($0.height)" })")
            setFormat(w.format)
            if app.store.hostMeta(hostID).localName == nil { model.hostName = w.name }
            // A host without a UI size list or a home monitor mode has no such controls: hide them.
            hostLiveResize = w.liveResize
            // v5 hosts: what it runs (the pill and the secure-screen wording), and its displays (the picker is for
            // Physical hosts with more than one; never for SodiumOS's Virtual one).
            let os = HostOS(w.hostOs, flavor: w.hostOsFlavor)
            hostOS = os
            model.hostOS = os
            app.store.update(hostID) { $0.hostOS = os }
            model.hostVirtual = w.displayMode == .virtual
            model.displays = w.displays.map { DisplayChoice(id: $0.id, name: $0.name, width: $0.width, height: $0.height, primary: $0.primary) }
            model.currentDisplay = w.currentDisplay
            // Core's defaults for (Mac, host): the Mac table for Windows and SodiumOS, none for a Mac.
            sendKeyTable()
            app.refreshHosts()
            model.hostOffersUISize = !w.scales.isEmpty
            model.hostOffersHomeMonitor = w.homeMonitor != nil
            app.model.hostOffersUISize = model.hostOffersUISize
            app.model.hostOffersHomeMonitor = model.hostOffersHomeMonitor
            if let hm = w.homeMonitor { model.homeMode = HomeMode(hm) }
            layoutChanged()
            app.store.update(hostID) { $0.locked = w.locked }
            if w.locked { showLockedToast() }
            if let from = w.tookOverFrom {
                model.toast = Toast(symbol: "laptopcomputer", tone: .accent, text: String(localized: "Took over from \(from)"), sub: String(localized: "it was idle"))
            }
        case .format(let f):
            setFormat(f)
        case .homeMonitor(let mode):
            model.homeMode = HomeMode(mode)
        case .locked(let on):
            app.store.update(hostID) { $0.locked = on }
            if on { showLockedToast() } else { lockedToastShown = false }
        case .takenOver(let by):
            takenOverBy = by
        case .tookOver(let from):
            model.toast = Toast(symbol: "laptopcomputer", tone: .accent, text: String(localized: "Took over from \(from)"), sub: String(localized: "it was idle"))
        case .clipboardOffer(let o):
            clipboard.onOffer(o)
        case .clipboardRequest(let serial, let format):
            clipboard.onRequest(serial: serial, format: format)
        case .clipboardData(let d):
            clipboard.onData(d)
        case .caret(let c):
            caret = c.visible ? c : nil
            window.streamView.inputContext?.invalidateCharacterCoordinates()
        case .focusChanged(let focused, let password):
            passwordFocus = focused && password
            window.streamView.setPasswordMode(passwordFocus)
            SecureInput.set(passwordFocus && keyFocus)
        case .imeReset:
            window.streamView.discardComposition()
        case .pointerLock(let l):
            pointer = l
            window.streamView.setPointerLock(l == .locked)
            applyCursor()
        case .stats(let s):
            if let h = s.host { stats.onHostStats(HostStatsSnapshot(h)) }
        case .disconnect(let r):
            lastDisconnect = r
        case .secureScreen(let on):
            Log.info("secure screen \(on ? "up: the host can't show or drive it" : "gone")")
            model.secure = on
            if on {
                // The designed notice: the last frame dimmed, a card, the amber chip; Disconnect only.
                model.toast = nil
                model.card = .secureScreen(os: hostOS ?? .windows)
            } else {
                if case .some(.secureScreen) = model.card { model.card = nil }
                model.toast = Toast(symbol: "checkmark.circle", tone: .success, text: String(localized: "Back on \(model.hostName)’s desktop"), duration: 3)
            }
        case .hello, .resize, .settings, .keyTable, .keyframeRequest, .vsync, .selectDisplay:
            break
        }
    }

    private func showLockedToast() {
        guard !lockedToastShown else { return }
        lockedToastShown = true
        model.toast = Toast(symbol: "lock.fill", tone: .neutral, text: String(localized: "\(model.hostName) is locked. Type your password here."), sub: String(localized: "Secure input is on."))
    }

    private func setFormat(_ f: StreamFormat) {
        format = f
        pipeline?.decoder.setCodec(f.codec)
        stats.setDecoder(decoderText, stream: "\(f.codec.label) \(f.width)×\(f.height)@\(Double(f.refreshMhz) / 1000)")
        updateIncrements()
        model.progress.sizeDetail = "\(f.width) × \(f.height) · \(Int((Double(f.refreshMhz) / 1000).rounded())) Hz"
        applyCursor()
    }

    private func onRoute(_ r: Route) {
        let kind = RouteKind(r.path)
        stats.onRoute(kind.statsName)
        if let old = route {
            let oldKind = RouteKind(old.path)
            if oldKind != kind {
                if r.rttMs + 10 <= old.rttMs || !oldKind.isTCPRelay && r.rttMs < old.rttMs {
                    model.toast = Toast(symbol: "point.topleft.down.to.point.bottomright.curvepath", tone: .accent,
                                        text: String(localized: "Now \(kind.label) · \(Int(r.rttMs.rounded())) ms"), sub: String(localized: "was \(oldKind.label) · \(Int(old.rttMs.rounded())) ms"))
                } else if r.rttMs >= old.rttMs + 10 {
                    model.toast = Toast(symbol: "point.topleft.down.to.point.bottomright.curvepath", tone: .neutral,
                                        text: String(localized: "Now \(kind.label) · \(Int(r.rttMs.rounded())) ms"), sub: String(localized: "\(oldKind.label) stopped answering"))
                }
            }
        }
        route = r
        model.progress.pathDetail = "\(kind.label) · \(Int(r.rttMs.rounded())) ms"
        app.store.update(hostID) { m in
            m.lastRoute = kind
            m.lastRttMs = r.rttMs
        }
        // A TCP relay: a toast once per network, then the amber chip.
        if kind.isTCPRelay {
            let net = r.local.split(separator: ".").prefix(3).joined(separator: ".")
            if !app.store.tcpToastShown(network: net) {
                app.store.markTcpToast(network: net)
                model.toast = Toast(symbol: "exclamationmark.triangle", tone: .warning,
                                    text: String(localized: "UDP is blocked on this network, so Sodium Remote goes through the relay over TCP."),
                                    sub: String(localized: "Expect about \(Int(r.rttMs.rounded())) ms."), action: String(localized: "Details"), actionID: .details, duration: 10)
            }
        }
    }

    private func onDisconnected(_ reason: DisconnectReason) {
        Log.info("disconnected: \(reason)")
        connected = false
        connecting = false
        ended = true
        timeline.ended = true
        model.chip = nil
        model.pill = nil
        clipboard.stop()
        audio?.stop()
        saveThumbnail()
        updateShortcutTap()
        if case .user = reason, closing { return }
        if case .takenOver(let by) = reason {
            model.card = .takenOver(by: by, at: HostLine.whenShort(Date()))
        } else {
            model.card = disconnectedCard(reason, host: model.hostName, os: hostOS ?? app.store.hostMeta(hostID).hostOS)
        }
    }

    // MARK: - The first frame, the decoder

    private func firstPresented() {
        firstFrame = true
        connectCardTimer?.invalidate()
        model.progress.step = 4
        if model.card == .connecting { model.card = nil }
        let (c, f) = stats.connectTimes()
        Log.info("first frame: connected \(c.map { "\($0 / 1000) ms" } ?? "?"), first frame \(f.map { "\($0 / 1000) ms" } ?? "?") after connect")
    }

    private func decoderChanged(_ text: String, _ hw: Bool) {
        decoderText = text
        hardware = hw
        stats.setDecoder(text, stream: "")
    }

    // MARK: - The timeline, the HUD, once a second

    func startTimers() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
    }

    private func tick() {
        ticks += 1
        let now = hostNow()
        var live: LinkSample?
        if let l = core.link() {
            live = LinkSample(route: l.route.map { RouteKind($0.path) }, routeRttMs: l.route?.rttMs, hedgeRoute: l.hedgeRoute.map { RouteKind($0.path) },
                              hedging: l.hedging, healthy: l.healthy, silence: Double(l.silenceMs) / 1000)
            hedge = l.hedgeRoute
        }
        if let v = timeline.now(live: connected ? live : nil, now: now, network: app.network.satisfied) {
            if model.chip != v.chip { model.chip = v.chip }
            if model.pill != v.pill { model.pill = v.pill }
            let since = (v.since * 1).rounded(.down)
            if model.pillSince != since { model.pillSince = since }
        }
        if case .some(.offline(let seen, _)) = model.card, let at = retryAt {
            model.card = .offline(lastSeen: seen, retryIn: max(0, Int((at - now).rounded(.up))))
        }
        if case .some(.asleep(let since, _)) = model.card, let at = retryAt {
            model.card = .asleep(since: since, retryIn: max(0, Int((at - now).rounded(.up))))
        }
        if model.hudMode != .off && ticks % 3 == 0 { model.hud = makeHud() }
        if ticks % 10 == 0 {
            if connected { pipeline?.tick() }
            if ticks % 50 == 0 { awdl = NetworkWatch.awdlActive() }
        }
    }

    private func makeHud() -> HudData {
        var h = HudData()
        let rtt = route?.rttMs ?? link.rttMs
        h.frames = stats.hud(rttMs: rtt, periodMs: 1000 / Double(max(display.hz, 1)))
        h.route = route.map { RouteKind($0.path) }
        h.rttMs = rtt
        h.band = QualityBand.of(rttMs: rtt, route: h.route)
        h.lossPercent = link.loss * 100
        h.fecPercent = Double(link.fecRatio) * 100
        if let f = format {
            h.sizeText = "\(f.width) × \(f.height) · \(Int((Double(f.refreshMhz) / 1000).rounded())) Hz"
            let codec = f.codec.label
            h.codecText = decoderText.isEmpty ? codec : (hardware ? String(localized: "\(codec) · hardware decode") : String(localized: "\(codec) · software decode"))
        }
        var paths: [HudPath] = []
        if let r = route {
            paths.append(HudPath(kind: RouteKind(r.path), state: String(localized: "in use"), rttMs: r.rttMs, inUse: true))
        }
        if let g = hedge, g != route {
            paths.append(HudPath(kind: RouteKind(g.path), state: link.hedging ? String(localized: "hedging") : String(localized: "standby"), rttMs: g.rttMs, inUse: false))
        }
        h.paths = paths
        h.awdl = awdl
        return h
    }

    // MARK: - The cursor

    private func applyCursor() {
        if pointer == .locked || !cursorVisible {
            window.streamView.setCursor(CursorFactory.hidden)
            return
        }
        guard let img = cursorImages[cursorShape] else {
            window.streamView.setCursor(NSCursor.arrow)
            return
        }
        // Stream pixels → points: the placement's factor over the backing scale.
        var k = 1.0
        if let pl = placement(), pl.1.width > 0 { k = pl.0.w / Double(pl.1.width) }
        let ppp = k / display.pixelsPerPoint
        if let c = CursorFactory.make(width: Int(img.width), height: Int(img.height), hotX: Int(img.hotX), hotY: Int(img.hotY),
                                      pixels: img.pixels, pointsPerPixel: ppp) {
            window.streamView.setCursor(c)
        }
    }

    // MARK: - StreamInputSink

    func send(_ e: InputEvent, eventTime: Double?) {
        guard connected else { return }
        if let t = eventTime {
            let age = hostNow() - t
            if age >= 0 && age < 10 {
                _ = core.sendInputAt(event: e, ageUs: UInt64(age * 1e6))
                return
            }
        }
        _ = core.sendInput(event: e)
    }

    func streamPoint(_ pt: NSPoint) -> (x: Double, y: Double)? {
        guard let pl = placement() else { return nil }
        let ppp = display.pixelsPerPoint
        let q = StreamLayout.toStream(pl.0, stream: pl.1, x: Double(pt.x) * ppp, y: Double(pt.y) * ppp)
        return (x: q.x, y: q.y)
    }

    func caretInView() -> NSRect? {
        guard let c = caret, let pl = placement() else { return nil }
        let p = pl.0
        let s = pl.1
        let ppp = display.pixelsPerPoint
        let a = StreamLayout.fromStream(p, stream: s, x: Double(c.rect.x), y: Double(c.rect.y))
        let b = StreamLayout.fromStream(p, stream: s, x: Double(c.rect.x + max(c.rect.w, 1)), y: Double(c.rect.y + c.rect.h))
        return NSRect(x: a.x / ppp, y: a.y / ppp, width: max((b.x - a.x) / ppp, 1), height: max((b.y - a.y) / ppp, 1))
    }

    var macInputMethod: Bool {
        app.model.settings.inputMethod == .mac
    }

    var pointerLocked: Bool {
        pointer == .locked
    }

    func remoteShortcut(_ e: NSEvent) -> Bool {
        let f = e.modifierFlags.rawValue
        if e.type == .keyDown {
            // ⌘Esc and ⌘⇧Esc, when the tap isn't running (no Accessibility).
            switch SystemKeys.decide(keyCode: e.keyCode, flags: f, fullScreen: false, keepInputSwitch: true) {
            case .leave:
                if !e.isARepeat { leave(disconnect: false) }
                return true
            case .disconnect:
                if !e.isARepeat { leave(disconnect: true) }
                return true
            default:
                break
            }
        }
        let remote = KeyMods.flagControl | KeyMods.flagOption | KeyMods.flagCommand
        guard f & remote == remote, e.type == .keyDown else { return false }
        switch e.keyCode {
        case 0x03: toggleFullScreen()                 // F
        case 0x01: cycleHud()                         // S
        case 0x2E: model.revealMenu.toggle()          // M
        case 0x02: disconnect()                       // D
        case 0x28: releaseKeyboard()                  // K
        case 0x25: sendLock()                         // L
        default: return false
        }
        return true
    }

    func streamClicked() {
        if !capturingKeys {
            capturingKeys = true
            updateShortcutTap()
        }
        if model.revealMenu { model.revealMenu = false }
    }

    func viewResized() {
        // The window moved to a display with another backing scale: the drawable
        // follows it (`backingSize`), so the points ↔ pixels factor and the
        // resize steps must too.
        if let s = window.window?.screen, Double(s.backingScaleFactor) != display.pixelsPerPoint {
            display.pixelsPerPoint = Double(s.backingScaleFactor)
            updateIncrements()
        }
        layoutChanged()
    }

    func focusLost() {
        updateShortcutTap()
    }

    /// ⌘Esc: out of full screen, and the keyboard back to the Mac until the
    /// next click; ⌘⇧Esc: disconnect. ⌘ is still down on the computer, so a
    /// key bound to nothing (F24) goes first: letting go of a bare Win would
    /// open the launcher.
    func leave(disconnect d: Bool) {
        window.streamView.cancelModifierTap()
        if d {
            disconnect()
            return
        }
        releaseKeyboard()
        if window.isFullScreen { window.toggleFullScreen() }
    }

    /// ⌃⌥⌘K: ⌘Tab and ⌘Space go back to the Mac until the next click.
    func releaseKeyboard() {
        window.streamView.releaseAll()
        capturingKeys = false
        updateShortcutTap()
    }

    // MARK: - Keys for the computer (Connection › Send Keys, Lock)

    /// A chord in the computer's terms; Win travels as Globe under the Mac
    /// table (which maps ⌘ to Ctrl), as GUI under PC keys.
    func sendChord(_ mods: UInt8, _ key: UInt16) {
        let mac = app.model.settings.keyboard == .mac
        var held: [UInt16] = []
        if mods & KeyMods.ctrl != 0 { held.append(KeyUsage.leftCtrl) }
        if mods & KeyMods.shift != 0 { held.append(KeyUsage.leftShift) }
        if mods & KeyMods.alt != 0 { held.append(KeyUsage.leftAlt) }
        if mods & KeyMods.gui != 0 { held.append(mac ? KeyUsage.globe : KeyUsage.leftGUI) }
        for u in held { send(.key(usage: u, down: true), eventTime: nil) }
        if key != 0 {
            send(.key(usage: key, down: true), eventTime: nil)
            send(.key(usage: key, down: false), eventTime: nil)
        }
        for u in held.reversed() { send(.key(usage: u, down: false), eventTime: nil) }
    }

    func sendLock() {
        sendChord(KeyMods.gui, 0x0F)
    }

    // MARK: - System shortcuts, focus, secure input

    func windowFocusChanged(key: Bool) {
        keyFocus = key
        if !key { window.streamView.releaseAll() }
        SecureInput.set(passwordFocus && key)
        updateShortcutTap()
    }

    func fullScreenChanged(_ on: Bool) {
        if on {
            model.toast = Toast(symbol: "arrow.up.left.and.arrow.down.right", tone: .neutral, text: String(localized: "Full screen. Move to the top edge for the menu;"),
                                sub: String(localized: "⌘Esc leaves, ⌘⇧Esc disconnects."))
        }
        updateShortcutTap()
        layoutChanged()
    }

    func updateShortcutTap() {
        let s = app.model.settings.systemShortcuts
        let full = window.isFullScreen
        let want: Bool
        switch s {
        case .never: want = false
        case .fullScreen: want = full
        case .fullScreenAndFocused: want = true
        }
        let active = want && connected && capturingKeys && keyFocus && window.window?.firstResponder === window.streamView
        app.setShortcutTap(active: active, fullScreen: full, keepInputSwitch: macInputMethod, session: self)
    }

    // MARK: - StreamActions

    func cycleHud() {
        let next: HudMode
        switch model.hudMode {
        case .off: next = .compact
        case .compact: next = .expanded
        case .expanded: next = .off
        }
        setHud(next)
    }

    func openHudExpanded() {
        setHud(model.hudMode == .expanded ? .off : .expanded)
    }

    func closeHud() {
        setHud(.off)
    }

    private func setHud(_ m: HudMode) {
        model.hudMode = m
        if m != .off { model.hud = makeHud() }
        app.model.settings.hud = m
    }

    func copyStats() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(StreamUI.statsText(makeHud()), forType: .string)
    }

    func openSettings() {
        app.showSettings(over: window.window)
    }

    func openServerSettings() {
        app.model.openSettingsOn = .server
        app.showSettings(over: window.window)
    }

    func toggleFullScreen() {
        window.toggleFullScreen()
    }

    func disconnect() {
        closing = true
        window.close()
    }

    func cancelConnect() {
        disconnect()
    }

    func closeCard() {
        disconnect()
    }

    func takeOver() {
        model.card = nil
        connect(takeover: true)
    }

    func reconnect() {
        model.card = nil
        retryDelay = 10
        connect(takeover: false)
    }

    func forgetHost() {
        let id = hostID
        disconnect()
        app.forget(hostID: id)
    }

    func pairAgain() {
        let id = hostID
        disconnect()
        app.forget(hostID: id)
        app.openPairing(hostID: nil)
    }

    func howToUpdate() {
        app.showHelp(anchor: "update")
    }

    func toastAction(_ a: ToastAction) {
        switch a {
        case .details: setHud(.expanded)
        case .allowAccessibility: Permissions.askAccessibility()
        case .keepFullEffects:
            app.model.settings.optimisedLook = false
        }
        model.toast = nil
    }

    func dismissToast() {
        model.toast = nil
    }

    func setKeyboard(_ m: KeyboardMode) {
        app.model.settings.keyboard = m
    }

    func setHomeMode(_ m: HomeMode) {
        // The settings observer sends it (settingsChanged).
        app.model.settings.homeMode = m
    }

    func selectDisplay(id: UInt32) {
        guard id != model.currentDisplay else { return }
        model.currentDisplay = id
        core.sendControl(msg: .selectDisplay(id))
    }

    func hideRevealMenu() {
        model.revealMenu = false
    }

    /// Accessibility is off and ⌘Tab matters (once per session).
    private var accessibilityToastShown = false
    func noteShortcutTapUnavailable() {
        guard !accessibilityToastShown else { return }
        accessibilityToastShown = true
        model.toast = Toast(symbol: "keyboard", tone: .warning, text: String(localized: "⌘Tab stays on this Mac: Accessibility is off"), action: String(localized: "Allow…"),
                            actionID: .allowAccessibility, duration: 8)
    }

    // MARK: - Closing

    func windowWillClose() {
        closing = true
        end()
        app.sessionEnded(self)
    }

    private func end() {
        timer?.invalidate()
        connectCardTimer?.invalidate()
        retryTimer?.invalidate()
        window.streamView.releaseAll()
        SecureInput.set(false)
        app.setShortcutTap(active: false, fullScreen: false, keepInputSwitch: macInputMethod, session: self)
        if connected || connecting { core.disconnect() }
        connected = false
        ended = true
        clipboard.stop()
        audio?.stop()
        saveThumbnail()
        app.listener.setVideoSink(nil)
        pipeline?.stop()
        var extra: [(String, String)] = []
        extra.append(("decoder_hardware", hardware ? "true" : "false"))
        _ = stats.finish(extra: extra)
    }

    private func saveThumbnail() {
        guard let pb = pipeline?.decoder.latestPicture() else { return }
        let img = CIImage(cvPixelBuffer: pb)
        let w = img.extent.width
        guard w > 0 else { return }
        let k = 528 / w
        let small = img.transformed(by: CGAffineTransform(scaleX: k, y: k))
        let ctx = CIContext(options: nil)
        if let cg = ctx.createCGImage(small, from: small.extent) {
            app.store.saveThumbnail(hostID, cg)
        }
    }

    // MARK: - The latency test (Help › Run the Latency Test…)

    func runLatencyTest() {
        guard connected, clickTest == nil else { return }
        let t = ClickTest(config: ClickTest.Config(), csvURL: stats.path("clicks.csv"), epochNs: stats.epochNs)
        clickTest = t
        pipeline?.setClickTest(t)
        model.toast = Toast(symbol: "timer", tone: .neutral, text: String(localized: "Latency test: clicking the test surface…"), sub: String(localized: "100 clicks"), duration: nil)
        Task { [weak self] in
            await self?.clickLoop(t)
        }
    }

    private func recorderNow() -> Int64 {
        clock.recorderUs(host: hostNow())
    }

    private func clickLoop(_ t: ClickTest) async {
        let target = t.target
        while true {
            if case .click = t.next(nowUs: recorderNow()) { break }
            if !connected { return }
            try? await Task.sleep(nanoseconds: 20_000_000)
        }
        try? await Task.sleep(nanoseconds: 1_000_000_000)
        _ = core.sendInput(event: .pointerAbs(x: target.x, y: target.y))
        while connected {
            switch t.next(nowUs: recorderNow()) {
            case .finished:
                await finishClickTest(t)
                return
            case .wait:
                try? await Task.sleep(nanoseconds: 5_000_000)
            case .click(let id):
                let gap = Int.random(in: t.config.gapMs)
                try? await Task.sleep(nanoseconds: UInt64(gap) * 1_000_000)
                _ = core.sendInput(event: .pointerAbs(x: target.x, y: target.y))
                try? await Task.sleep(nanoseconds: 8_000_000)
                _ = core.sendInput(event: .button(button: .left, down: true))
                t.clicked(id: id, tClickUs: recorderNow())
                try? await Task.sleep(nanoseconds: 40_000_000)
                _ = core.sendInput(event: .button(button: .left, down: false))
            }
        }
    }

    private func finishClickTest(_ t: ClickTest) async {
        try? await Task.sleep(nanoseconds: 1_000_000_000)
        let a = stats.bytesReceivedSample()
        try? await Task.sleep(nanoseconds: UInt64(max(t.config.idleMs, 2500)) * 1_000_000)
        let b = stats.bytesReceivedSample()
        // i18n:off the latency report is a file for developers, in English
        var idle = "n/a"
        if b.atUs > a.atUs {
            let secs = Double(b.atUs - a.atUs) / 1e6
            let kbps = Double(b.bytes &- a.bytes) * 8 / secs / 1e3
            idle = String(format: "%.1f kbps received over %.1f s, test surface still", kbps, secs)
        }
        let (c, f) = stats.connectTimes()
        let ms = { (v: Int64?) -> String in v.map { String(format: "%.0f ms", Double($0) / 1e3) } ?? "n/a" }
        let rows: [(String, String, String)] = [
            ("Cursor", "Drawn locally: no added latency", "local: the cursor channel's image as NSCursor"),
            ("Idle bandwidth", "Under 50 kbps on a still desktop", idle),
            ("Connect", "First frame in under 1.5 s direct", "connected \(ms(c)), first frame ready \(ms(f)) after connect was called"),
        ]
        var md = t.summary(context: "Mac client, \(route.map { RouteKind($0.path).statsName } ?? "?")", rows: rows)
        md += "\n**All frames (p50 / p95 ms, n):**\n\n| Stage | p50 | p95 | n |\n|---|---|---|---|\n"
        for row in stats.stageTable() where row.n > 0 {
            md += String(format: "| %@ | %.2f | %.2f | %d |\n", row.name, row.p50 ?? 0, row.p95 ?? 0, row.n)
        }
        try? md.write(to: stats.path("latency.md"), atomically: true, encoding: .utf8)
        // i18n:on
        pipeline?.setClickTest(nil)
        clickTest = nil
        Log.info(t.status())
        model.toast = Toast(symbol: "timer", tone: .success, text: t.status(), sub: String(localized: "latency.md in the stats folder"), duration: 15)
    }
}
