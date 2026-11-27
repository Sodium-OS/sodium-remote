// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import Combine
import Foundation
import RemoteCore
import RemoteKit
import RemoteUI

/// The app: the core, the host list and its presence, pairing, the one
/// stream session (one client at a time), settings (one set for
/// every computer, applied at once), first-run permissions.
@MainActor
final class AppController: NSObject, AppActions, PairingActions, SettingsActions {
    let model = AppModel()
    let store = Store()
    let listener = CoreListener()
    let network = NetworkWatch()
    let hevc444Hardware: Bool
    let av1Hardware: Bool
    private(set) var core: RemoteClient?
    /// The menu-bar extra and "being controlled", and this Mac as a host.
    let presence = PresenceController()
    private(set) var hostController: HostController?
    private(set) var session: StreamSession?
    private var hostList: HostListWindowController?
    private var hostPresence: [String: HostPresence] = [:]
    private var presenceTimer: Timer?
    private var lastSettings = RemoteSettings()
    private var subs = Set<AnyCancellable>()

    private var pairing: PairingModel?
    private var pairingSheet: NSWindow?
    private var pairingFor: String?
    private var pairedDevice: Device?
    private var pairingBusy = false
    private var discovering = false

    private var settingsSheet: NSWindow?
    private var dialogSheet: NSWindow?
    private let tap = ShortcutTap()
    private weak var tapSession: StreamSession?
    private let lanProbe = LocalNetworkProbe()
    private var axTimer: Timer?
    private var lastHostID: String?
    /// The app's first start has finished: a link opened before that waits (`openServerLink`).
    private var started = false
    private var linkOpenedBeforeStart: String?
    /// A server was saved while a session ran: the core restarts when the session ends.
    private var serverWaitsForSession = false

    override init() {
        hevc444Hardware = HardwareProbe.hevc444()
        av1Hardware = HardwareProbe.av1()
        super.init()
    }

    // MARK: - Start

    func start() {
        Log.info("HEVC 4:4:4 hardware decode: \(hevc444Hardware), AV1: \(av1Hardware)")
        lastSettings = store.loadSettings()
        model.settings = lastSettings
        model.defaultKeyTable = KeyTable(rules: macDefaultKeyTable().map { KeyRuleModel($0) })
        model.actions = self
        model.settingsActions = self
        model.serverActions = self
        model.showWelcome = !store.welcomeDone
        model.showPreviews = store.showPreviews
        model.accessibility = Permissions.accessibilityAllowed ? .allowed : .unknown
        listener.handler = { [weak self] e in self?.handle(e) }
        network.onChange = { [weak self] in
            Log.info("the network changed")
            self?.core?.networkChanged()
        }
        network.start()
        startCore()
        let hl = HostListWindowController(model: model)
        hostList = hl
        hl.showWindow(nil)
        startHost()
        refreshHosts()
        refreshSettingsInfo()
        startPresence()
        model.$settings.dropFirst().sink { [weak self] s in self?.settingsChanged(s) }.store(in: &subs)
        model.$showPreviews.dropFirst().removeDuplicates().sink { [weak self] on in self?.previewsChanged(on) }.store(in: &subs)
        model.$hosts.sink { [weak self] h in
            // The empty state lists computers with an open pairing window.
            if h.isEmpty { self?.discoverLoop() }
        }.store(in: &subs)
        started = true
        if let link = linkOpenedBeforeStart {
            linkOpenedBeforeStart = nil
            openServerLink(link)
        }
    }

    /// A `sodium-remote://server?…` link was opened (the URL scheme): Settings › Server asks to confirm it.
    func openServerLink(_ link: String) {
        guard started else {
            linkOpenedBeforeStart = link
            return
        }
        switch inspectServerLink(link) {
        case .success(let c):
            confirmServer(c)
        case .failure:
            let a = NSAlert()
            a.messageText = String(localized: "That isn’t a Sodium Remote server link.")
            a.runModal()
        }
    }

    /// Settings › Server, with this server waiting for the person's yes: nothing is saved before it.
    func confirmServer(_ c: ServerCandidate) {
        model.pendingServer = c
        NSApp.activate(ignoringOtherApps: true)
        if settingsSheet == nil {
            showSettings(over: session?.window.window ?? hostList?.window)
        }
    }

    /// The saved server if the core accepts it (a stored value that doesn't parse is ignored and logged).
    private func activeServer() -> ServerSettings? {
        guard let saved = store.server else { return nil }
        guard case .success = inspectServer(url: saved.url, caPEM: saved.caPEM, token: saved.token) else {
            Log.error("the saved server address isn’t valid; running without a server")
            return nil
        }
        return saved
    }

    func startCore() {
        // The device name is the Sharing computer name, read at every launch.
        let name = Host.current().localizedName ?? "Mac"
        // No server configured: the core never dials one (LAN and address only).
        let server = activeServer()
        publishServer(server)
        model.rendezvous = server == nil ? .off : .connecting
        let settings = ClientSettings(name: name, rendezvous: server != nil, rendezvousUrl: server?.url ?? "",
                                      rendezvousCaPem: server?.caPEM ?? "", rendezvousToken: server?.token ?? "", mdns: true,
                                      fps: UInt32(DisplayInfo.of(NSScreen.main).hz), lanHints: store.lanHints,
                                      relayOnly: model.settings.relay == .alwaysRelay)
        do {
            let c = try RemoteClient(settings: settings, store: KeychainStore(), listener: listener)
            core = c
            if let last = store.deviceName, last != name {
                // Renamed in System Settings since the last launch: pairing, offers and Hello carry the new name.
                Log.info("this Mac was renamed: \(last) → \(name)")
                c.setName(name: name)
            }
            store.deviceName = name
            model.thisMacName = name
            model.thisMacID = c.id()
            model.thisMacCode = deviceCode(id: c.id())
            Log.info("this Mac is \(model.thisMacCode) (\(c.id()))")
        } catch {
            Log.error("the core didn't start: \(error)")
            let detail = "\(error)"
            model.rendezvous = .offline(String(localized: "Remote couldn’t start: \(detail)"))
        }
    }

    /// A server was saved or removed: the host side takes it at once, the client side when no session is running
    /// (a restart of the core ends it).
    func serverSettingsChanged() {
        hostController?.setServer(activeServer())
        if session != nil {
            serverWaitsForSession = true
            model.serverWaitsForSession = true
            publishServer(activeServer())
        } else {
            restartCore()
        }
    }

    private func restartCore() {
        serverWaitsForSession = false
        model.serverWaitsForSession = false
        core?.shutdown()
        core = nil
        hostPresence = [:]
        startCore()
        refreshHosts()
        if core != nil { pollPresence() }
    }

    func shutdown() {
        session?.disconnect()
        hostController?.shutdown()
        core?.shutdown()
    }

    // MARK: - This computer as a host

    private func startHost() {
        let host = HostController(pane: model.hostPane, store: store, presence: presence)
        hostController = host
        host.setServer(activeServer())
        presence.onOpen = { [weak self] in self?.showMainWindow(page: .computers) }
        presence.onOpenThisComputer = { [weak self] in self?.showMainWindow(page: .thisComputer) }
        presence.onToggleAllowed = { [weak host] on in host?.setAllowed(on) }
        presence.onPair = { [weak self, weak host] in
            self?.showMainWindow(page: .thisComputer)
            host?.openPairing()
        }
        presence.onToggleLogin = { [weak host] on in host?.setStartAtLogin(on) }
        presence.onDisconnect = { [weak host] in host?.disconnect() }
        presence.onQuit = { NSApp.terminate(nil) }
        presence.start()
        host.start()
    }

    func showMainWindow(page: MainPage) {
        model.page = page
        hostList?.showWindow(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    /// "About Sodium Remote": Settings › About (the board's About tab; the standard panel has no source link).
    func showAbout() {
        model.openSettingsOn = .about
        showSettings(over: session?.window.window ?? hostList?.window)
    }

    func openSettingsSheet() {
        showSettings(over: hostList?.window)
    }

    // MARK: - Core events

    private func handle(_ e: RemoteEvent) {
        switch e {
        case .rendezvous(let online, let error):
            model.rendezvous = online ? .online : .offline(error)
            if let err = error, err.contains("host_not_allowed") == false { Log.info("rendezvous: \(err)") }
        case .presence(let host, let online, let lastSeen, let asleepSince):
            var p = hostPresence[host] ?? HostPresence(host: host, online: false, onLan: false, lastSeen: nil, asleepSince: nil)
            p.online = online
            p.lastSeen = lastSeen
            p.asleepSince = asleepSince
            hostPresence[host] = p
            refreshHosts()
        case .revoked(let host):
            Log.info("\(deviceCode(id: host)) removed this Mac")
            refreshHosts()
        default:
            session?.handle(e)
        }
    }

    // MARK: - Hosts

    private func startPresence() {
        presenceTimer?.invalidate()
        pollPresence()
        presenceTimer = Timer.scheduledTimer(withTimeInterval: 10, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.pollPresence() }
        }
    }

    private func pollPresence() {
        guard let c = core else { return }
        Task { [weak self] in
            let ps = await c.presence()
            guard let self = self else { return }
            for p in ps {
                self.hostPresence[p.host] = p
                if p.online || p.onLan { self.store.update(p.host) { $0.lastSeen = Date() } }
            }
            self.refreshHosts()
        }
    }

    func asleepSince(_ id: String) -> Date? {
        hostPresence[id]?.asleepSince.map { Date(timeIntervalSince1970: TimeInterval($0)) }
    }

    func displayName(_ id: String) -> String {
        if let n = store.hostMeta(id).localName { return n }
        return core?.paired().first { $0.id == id }?.name ?? deviceCode(id: id)
    }

    func refreshHosts() {
        guard let c = core else { return }
        var cards: [HostCardModel] = []
        for d in c.paired() {
            let meta = store.hostMeta(d.id)
            let p = hostPresence[d.id]
            var state: HostState = .offline
            if let s = session, s.hostID == d.id, !s.connected {
                state = .connecting
            } else if let since = p?.asleepSince {
                state = .asleep(since: Date(timeIntervalSince1970: TimeInterval(since)))
            } else if p?.online == true || p?.onLan == true {
                if let by = meta.busyBy {
                    state = .busy(by: by)
                } else if meta.locked {
                    state = .locked
                } else {
                    state = .online
                }
            }
            var lastConnected = meta.lastConnected
            if lastConnected == nil, let t = d.lastConnected { lastConnected = Date(timeIntervalSince1970: TimeInterval(t)) }
            var lastSeen = meta.lastSeen
            if let t = p?.lastSeen { lastSeen = Date(timeIntervalSince1970: TimeInterval(t)) }
            let current = state != .offline && !(state == .connecting)
            let line = HostLine.line(state, lastConnected: lastConnected, lastSeen: lastSeen)
            let band = state == .offline ? QualityBand.none : QualityBand.of(rttMs: meta.lastRttMs, route: meta.lastRoute)
            let path = HostLine.path(meta.lastRoute, rttMs: meta.lastRttMs, current: current)
            cards.append(HostCardModel(id: d.id, name: meta.localName ?? d.name, code: deviceCode(id: d.id), state: state, line: line,
                                       band: band, route: meta.lastRoute, pathText: path, thumbnail: store.thumbnail(d.id), os: meta.hostOS))
        }
        model.hosts = cards
    }

    // MARK: - AppActions

    func connect(hostID: String, fullScreen: Bool) {
        if let s = session {
            if s.hostID == hostID {
                s.window.showWindow(nil)
                return
            }
            s.disconnect()
        }
        guard let c = core else { return }
        guard let s = StreamSession(app: self, hostID: hostID, name: displayName(hostID), core: c, statsDir: store.statsDir,
                                    keep: model.settings.keepSessions) else {
            let a = NSAlert()
            a.messageText = String(localized: "Remote can’t show the stream")
            a.informativeText = String(localized: "Metal or VideoToolbox isn’t available on this Mac.")
            a.runModal()
            return
        }
        session = s
        lastHostID = hostID
        s.open(fullScreen: fullScreen)
        refreshHosts()
    }

    func sessionEnded(_ s: StreamSession) {
        if session === s { session = nil }
        if serverWaitsForSession && session == nil { restartCore() }
        if tapSession === s { tap.active = false }
        refreshHosts()
        hostList?.showWindow(nil)
    }

    func reconnectLast() {
        if let s = session {
            s.reconnect()
        } else if let id = lastHostID {
            connect(hostID: id, fullScreen: false)
        }
    }

    func rename(hostID: String, to name: String) {
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        store.update(hostID) { $0.localName = n.isEmpty ? nil : n }
        if let s = session, s.hostID == hostID, !n.isEmpty { s.model.hostName = n }
        refreshHosts()
    }

    func copyID(hostID: String) {
        // Copy always copies the full ID.
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(hostID, forType: .string)
    }

    func forget(hostID: String) {
        guard let core = core else { return }
        do {
            try core.forget(host: hostID)
        } catch {
            Log.error("forget: \(error)")
            return
        }
        if let s = session, s.hostID == hostID { s.disconnect() }
        store.forget(hostID)
        hostPresence.removeValue(forKey: hostID)
        refreshHosts()
    }

    func requestAccessibility() {
        Permissions.askAccessibility()
        if Permissions.accessibilityAllowed {
            model.accessibility = .allowed
            return
        }
        model.accessibility = .waiting
        axTimer?.invalidate()
        axTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] t in
            MainActor.assumeIsolated {
                guard let self = self else { return }
                if Permissions.accessibilityAllowed {
                    self.model.accessibility = .allowed
                    self.refreshSettingsInfo()
                    t.invalidate()
                }
            }
        }
    }

    func requestLocalNetwork() {
        model.localNetwork = .waiting
        lanProbe.onResult = { [weak self] allowed in
            MainActor.assumeIsolated {
                self?.model.localNetwork = allowed ? .allowed : .denied
            }
        }
        lanProbe.start()
    }

    func openSystemSettings(_ pane: SystemPane) {
        switch pane {
        case .accessibility: Permissions.openAccessibility()
        case .localNetwork: Permissions.openLocalNetwork()
        case .microphone: Permissions.openMicrophone()
        }
    }

    func finishWelcome() {
        store.welcomeDone = true
        model.showWelcome = false
    }

    // MARK: - Pairing (the code never leaves the Mac)

    func openPairing(hostID: String?) {
        guard core != nil, pairingSheet == nil, let hl = hostList, let w = hl.window else { return }
        hl.showWindow(nil)
        let m = PairingModel()
        m.actions = self
        pairing = m
        pairingFor = hostID
        pairedDevice = nil
        let sheet = NSWindow(contentViewController: PairingUI.makeController(model: m))
        pairingSheet = sheet
        w.beginSheet(sheet) { _ in }
    }

    /// Computers with an open pairing window, for the empty state's list.
    private func discoverLoop() {
        guard !discovering, let c = core else { return }
        discovering = true
        Task { [weak self] in
            while true {
                guard let me = self, me.model.hosts.isEmpty || me.pairingSheet != nil else { break }
                if !me.pairingBusy {
                    let found = (try? await c.discoverPairing(timeoutMs: 2000)) ?? []
                    if !me.pairingBusy {
                        me.model.nearby = found.map { NearbyHost(id: $0.host, name: $0.name, foundVia: $0.foundVia) }
                    }
                }
                try? await Task.sleep(nanoseconds: 3_000_000_000)
            }
            self?.discovering = false
        }
    }

    func pairingSubmit(code: String) {
        guard let c = core, let m = pairing, !pairingBusy else { return }
        m.phase = .checking
        pairingBusy = true
        let wanted = pairingFor
        Task { [weak self] in
            // Fresh: `pair` names a computer from the latest discovery.
            var hosts: [PairingHost] = []
            var failure: RemoteError?
            do {
                hosts = try await c.discoverPairing(timeoutMs: 1500)
            } catch let e as RemoteError {
                failure = e
            } catch {
                failure = .Failed(message: "\(error)")
            }
            if let id = wanted {
                let only = hosts.filter { $0.host == id }
                if !only.isEmpty { hosts = only }
            }
            var result: Device?
            for h in hosts {
                do {
                    result = try await c.pair(host: h, code: code)
                    break
                } catch let e as RemoteError {
                    failure = e
                    if case .WrongCode = e { continue }
                    break
                } catch {
                    failure = .Failed(message: "\(error)")
                    break
                }
            }
            guard let self = self else { return }
            self.pairingBusy = false
            if let d = result {
                self.pairedDevice = d
                m.nameHere = d.name
                m.phase = .paired(name: d.name, code: deviceCode(id: d.id))
                self.refreshHosts()
                Log.info("paired with \(d.name) (\(deviceCode(id: d.id)))")
            } else if hosts.isEmpty, !isServerBusy(failure) {
                m.phase = .noComputer
            } else {
                m.phase = AppController.phase(for: failure)
            }
        }
    }

    private func isServerBusy(_ e: RemoteError?) -> Bool {
        if case .ServerBusy = e { return true }
        return false
    }

    static func phase(for e: RemoteError?) -> PairingPhase {
        guard let e = e else { return .noComputer }
        switch e {
        case .WrongCode:
            return .wrong(left: nil)
        case .PairingClosed(let reason):
            if reason.contains("attempts") || reason.contains("too_many") { return .tooMany }
            if reason.contains("expired") { return .expired }
            if reason.contains("not found") || reason.contains("not_found") { return .noComputer }
            return .failed(String(localized: "The computer closed its pairing window. Make a new code there."))
        case .ServerBusy:
            return .failed(String(localized: "The server is busy. Try again in a minute."))
        case .Failed(let message):
            return .failed(message)
        default:
            return .failed("\(e)")
        }
    }

    func pairingCancel() {
        endPairing()
    }

    func pairingTryNew() {
        pairing?.digits = ""
        pairing?.phase = .entering
    }

    func pairingFinish(name: String, connect: Bool) {
        let d = pairedDevice
        endPairing()
        guard let dev = d else { return }
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if !n.isEmpty && n != dev.name { store.update(dev.id) { $0.localName = n } }
        refreshHosts()
        if connect { self.connect(hostID: dev.id, fullScreen: false) }
    }

    private func endPairing() {
        if let s = pairingSheet {
            hostList?.window?.endSheet(s)
        }
        pairingSheet = nil
        pairing = nil
        pairingBusy = false
    }

    // MARK: - Taking over

    func askTakeover(by: String, session s: StreamSession) {
        guard let w = s.window.window, dialogSheet == nil else { return }
        let vc = DialogUI.makeController(
            symbol: "laptopcomputer", title: String(localized: "In use by \(by). Take over?"),
            body: String(localized: "\(by) is streaming \(s.model.hostName) right now. Taking over ends that stream; nothing on the computer closes."),
            cancel: String(localized: "Cancel"), confirm: String(localized: "Take over"), confirmKind: .primary,
            onCancel: { [weak self, weak s] in
                self?.endDialog(on: w)
                s?.closeCard()
            },
            onConfirm: { [weak self, weak s] in
                self?.endDialog(on: w)
                s?.takeOver()
            })
        let sheet = NSWindow(contentViewController: vc)
        dialogSheet = sheet
        w.beginSheet(sheet) { _ in }
    }

    private func endDialog(on w: NSWindow) {
        if let d = dialogSheet { w.endSheet(d) }
        dialogSheet = nil
    }

    // MARK: - Settings

    func showSettings(over window: NSWindow?) {
        guard settingsSheet == nil else { return }
        refreshSettingsInfo()
        guard let parent = window ?? session?.window.window ?? hostList?.window else { return }
        let sheet = NSWindow(contentViewController: SettingsUI.makeController(model: model))
        settingsSheet = sheet
        parent.beginSheet(sheet) { _ in }
    }

    private func settingsChanged(_ s: RemoteSettings) {
        store.saveSettings(s)
        let old = lastSettings
        lastSettings = s
        session?.settingsChanged(from: old, to: s)
        if old.relay != s.relay {
            core?.setRelayOnly(on: s.relay == .alwaysRelay)
            Log.info("relay choice changed: it applies from the next connection")
        }
    }

    /// "Show previews" turned off: the saved pictures go, the cards keep their size; turned on: pictures come back as
    /// sessions save them.
    private func previewsChanged(_ on: Bool) {
        store.showPreviews = on
        if !on {
            store.deleteThumbnails()
            model.listToast = String(localized: "Previews are off. The saved pictures were deleted.")
            Task { @MainActor [weak self] in
                try? await Task.sleep(nanoseconds: 4_000_000_000)
                self?.model.listToast = nil
            }
        }
        refreshHosts()
    }

    func refreshSettingsInfo() {
        var i = SettingsInfo()
        let d = DisplayInfo.of(session?.window.window?.screen ?? NSScreen.main)
        i.displayPixels = "\(d.fullPixels.width) × \(d.fullPixels.height) px"
        i.displayHz = d.hz
        if let s = session {
            let cd = s.currentDisplay()
            i.windowPixels = "\(cd.width) × \(cd.height) px"
        }
        i.hevc444Hardware = hevc444Hardware
        i.av1Hardware = av1Hardware
        i.accessibility = Permissions.accessibilityAllowed ? .allowed : .denied
        i.outputs = AudioDevices.all().map { AudioOutputChoice(uid: $0.uid, name: $0.name) }
        i.awdl = NetworkWatch.awdlActive()
        i.statsFolder = store.statsDir.path.replacingOccurrences(of: NSHomeDirectory(), with: "~")
        model.settingsInfo = i
    }

    func settingsDone() {
        if let s = settingsSheet, let parent = s.sheetParent {
            parent.endSheet(s)
        }
        settingsSheet = nil
    }

    func showStatsFolder() {
        try? FileManager.default.createDirectory(at: store.statsDir, withIntermediateDirectories: true)
        NSWorkspace.shared.open(store.statsDir)
    }

    func runLatencyTest() {
        settingsDone()
        guard let s = session, s.connected else {
            let a = NSAlert()
            a.messageText = String(localized: "Connect first")
            a.informativeText = String(localized: "The latency test clicks the test surface on the computer (spikes/remote-e2e) and times its flip to this screen. Start the test surface there, connect, then run the test.")
            a.runModal()
            return
        }
        s.runLatencyTest()
    }

    func openSourceCode() {
        if let u = URL(string: Brand.sourceURL) { NSWorkspace.shared.open(u) }
    }

    /// The notices file build-app.sh puts in the app (the acknowledgements and cargo-about's list).
    func openNoticesFile() {
        guard let u = Bundle.main.url(forResource: "THIRD-PARTY-NOTICES", withExtension: "txt") else {
            Log.error("THIRD-PARTY-NOTICES.txt is missing from the app")
            return
        }
        NSWorkspace.shared.open(u)
    }

    func showAWDLHelp() {
        let a = NSAlert()
        a.messageText = String(localized: "AirDrop and Handoff on Wi-Fi")
        a.informativeText = String(localized: "While they are on, the Wi-Fi radio hops channels and can hold packets back for up to 90 ms. Turn off AirDrop (Control Center › AirDrop › No One) and Handoff (System Settings › General › AirDrop & Handoff) while streaming, or run `sudo ifconfig awdl0 down` until the next restart.")
        a.runModal()
    }

    func showHelp(anchor: String) {
        let a = NSAlert()
        if anchor == "update" {
            a.messageText = String(localized: "Updating Sodium Remote")
            a.informativeText = String(localized: "Sodium Remote doesn’t check for updates. Build the new version of the app (BUILD.md in the source) and install it on both computers. Each release speaks its own protocol version and the one before it.")
        } else {
            a.messageText = "Sodium Remote"
            a.informativeText = String(localized: "Sodium Remote streams another computer to this Mac. ⌘ is the computer’s Win key. In full screen every key goes to the computer; ⌘Esc leaves full screen and ⌘⇧Esc disconnects. Remote’s own shortcuts are ⌃⌥⌘ + a key: F full screen, S stats, M the in-stream menu, D disconnect, K release the keyboard, L lock.")
        }
        a.runModal()
    }

    func copyDiagnostics() {
        // i18n:off diagnostics are for bug reports, in English
        var lines: [String] = []
        let v = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "?"
        let b = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "?"
        lines.append("Sodium Remote for Mac \(v) (\(b)), protocol \(protocolVersion())")
        lines.append("macOS \(ProcessInfo.processInfo.operatingSystemVersionString)")
        lines.append("This Mac: \(model.thisMacCode) \(model.thisMacID)")
        lines.append("HEVC 4:4:4 hardware: \(hevc444Hardware), AV1 hardware: \(av1Hardware)")
        lines.append("Rendezvous: \(model.rendezvous)")
        for h in model.hosts { lines.append("Computer: \(h.name) \(h.code) \(h.id) · \(h.line)") }
        if let s = session { lines.append(StreamUI.statsText(s.model.hud)) }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(lines.joined(separator: "\n"), forType: .string)
        // i18n:on
    }

    // MARK: - ⌘Tab and friends (the event tap)

    func setShortcutTap(active: Bool, fullScreen: Bool, keepInputSwitch: Bool, session s: StreamSession) {
        if active {
            if !tap.running {
                guard tap.start() else {
                    s.noteShortcutTapUnavailable()
                    return
                }
                tap.onKey = { [weak self] code, down in
                    MainActor.assumeIsolated {
                        self?.tapSession?.window.streamView.tappedKey(keyCode: code, down: down, t: hostNow())
                    }
                }
                tap.onLeave = { [weak self] disconnect in
                    MainActor.assumeIsolated {
                        self?.tapSession?.leave(disconnect: disconnect)
                    }
                }
            }
            tapSession = s
            tap.fullScreen = fullScreen
            tap.keepInputSwitch = keepInputSwitch
            tap.active = true
        } else if tapSession == nil || tapSession === s {
            tap.active = false
        }
    }
}
