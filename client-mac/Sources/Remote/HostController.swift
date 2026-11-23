// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import Foundation
import RemoteCore
import RemoteKit
import RemoteUI
import ServiceManagement

extension HostOS {
    /// A device's or computer's OS from the core's `Os` and its os-release `ID` (`flavor`, lowercase, empty
    /// when unknown): a Linux that says "sodiumos" is SodiumOS, any other Linux is "Linux".
    init(_ os: Os, flavor: String) {
        switch os {
        case .linux: self = flavor == "sodiumos" ? .sodiumOS : .linux
        case .mac: self = .mac
        case .windows: self = .windows
        }
    }
}

/// Receives the host's events on a Rust thread and hops to the main actor.
final class HostListener: ThisComputerListener, @unchecked Sendable {
    /// Set on main; called on main.
    var handler: (@MainActor (ThisComputerEvent) -> Void)?

    func onEvent(event: ThisComputerEvent) {
        onMain { [weak self] in
            self?.handler?(event)
        }
    }
}

/// This Mac as a host: binds the core's `ThisComputer` to the "This computer"
/// page and the menu-bar extra. The host runs in this process, so Screen Recording and Accessibility belong to
/// this app's bundle ID. The app keeps the switch, the name and "Start at login"; the host
/// is started at launch when the switch was on.
@MainActor
final class HostController: NSObject, HostPaneActions {
    let pane: HostPaneModel
    private let store: Store
    private let presence: PresenceController
    private let listener = HostListener()
    private var host: ThisComputer?
    /// The server this computer registers with (Settings › Server, the same as the client side); nil: none.
    private var server: ServerSettings?
    private var permTimer: Timer?
    private var screenOpened = false
    private var accessOpened = false
    private var controllingID: String?
    private var lastNotifiedSince: UInt64?

    init(pane: HostPaneModel, store: Store, presence: PresenceController) {
        self.pane = pane
        self.store = store
        self.presence = presence
        super.init()
    }

    // MARK: - Start and stop

    func start() {
        pane.actions = self
        pane.name = store.hostName
        pane.sharingName = Host.current().localizedName ?? "Mac"
        pane.startAtLogin = SMAppService.mainApp.status == .enabled
        let dir = store.hostDir
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        do {
            // The host's key is in the Keychain (its own item, `host/device.key`); an older key file in `dir` is imported once.
            host = try ThisComputer(dataDir: dir.path, name: store.hostName, listener: listener, keyStore: KeychainStore.host(directory: dir))
        } catch {
            pane.startError = Self.text(error)
            Log.error("the host didn't start: \(error)")
            return
        }
        host?.setServer(url: server?.url ?? "", caPem: server?.caPEM ?? "", token: server?.token ?? "")
        listener.handler = { [weak self] e in self?.handle(e) }
        refreshPermissions()
        permTimer = Timer.scheduledTimer(withTimeInterval: 1.5, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
        refreshDevices()
        updatePresence()
        if store.hostAllowed { turnOn() }
    }

    /// The server changed (or was set before `start`): the host takes it at the next start. When it is on and nobody is
    /// controlling it, it restarts at once; a session in progress is left alone and the server applies the next time
    /// remote control is switched on.
    func setServer(_ s: ServerSettings?) {
        server = s
        guard let h = host else { return }
        h.setServer(url: s?.url ?? "", caPem: s?.caPEM ?? "", token: s?.token ?? "")
        guard pane.isOn, pane.controller == nil, !pane.starting else { return }
        pane.starting = true
        Task { [weak self] in
            await h.stop()
            self?.stopped()
            self?.turnOn()
        }
    }

    /// The app is quitting: dropping the object stops a running session.
    func shutdown() {
        permTimer?.invalidate()
        permTimer = nil
        host = nil
    }

    private func turnOn() {
        guard let h = host, !pane.starting, !pane.isOn else { return }
        pane.starting = true
        pane.startError = nil
        Task { [weak self] in
            do {
                try await h.start()
                self?.started()
            } catch {
                self?.startFailed(error)
            }
        }
    }

    private func started() {
        pane.starting = false
        pane.isOn = true
        store.hostAllowed = true
        presence.askForNotificationsOnce()
        refreshAddress()
        refreshDevices()
        refreshPermissions()
        updatePresence()
        Log.info("remote control is on")
    }

    private func startFailed(_ error: Error) {
        pane.starting = false
        pane.isOn = false
        store.hostAllowed = false
        pane.startError = Self.text(error)
        updatePresence()
        Log.error("remote control didn't start: \(error)")
    }

    private func turnOff() {
        guard let h = host else { return }
        pane.starting = true
        // Never wait for it on the main thread: the backend reads NSScreen and NSCursor there.
        Task { [weak self] in
            await h.stop()
            self?.stopped()
        }
    }

    private func stopped() {
        pane.starting = false
        pane.isOn = false
        pane.controller = nil
        pane.pairing = nil
        pane.address = nil
        pane.fullAddress = nil
        controllingID = nil
        store.hostAllowed = false
        refreshDevices()
        updatePresence()
        Log.info("remote control is off")
    }

    // MARK: - Events

    private func handle(_ e: ThisComputerEvent) {
        switch e {
        case .state(let s):
            apply(s)
        case .pairingOpened(let p):
            pane.pairing = HostPairing(code: p.code, expiresAt: Date().addingTimeInterval(TimeInterval(p.expiresInSecs)))
            pane.pairingClosedByWrongCodes = false
        case .pairingClosed(let reason):
            pane.pairing = nil
            pane.pairingClosedByWrongCodes = reason == "attempts_used"
        case .pairingAttempt, .pairingFailed:
            break
        case .devicesChanged:
            refreshDevices()
        }
    }

    private func apply(_ s: ThisComputerState) {
        switch s {
        case .off, .waiting:
            pane.controller = nil
            controllingID = nil
        case .beingControlled(let by):
            setController(by, secure: false)
        case .secureScreen(let by):
            setController(by, secure: true)
        }
        refreshDevices()
        updatePresence()
    }

    private func setController(_ by: ControllingDevice, secure: Bool) {
        let since = Date(timeIntervalSince1970: TimeInterval(by.since))
        pane.controller = ControllerModel(name: by.name, os: HostOS(by.os, flavor: by.osFlavor), since: since, secure: secure, reconnecting: by.reconnecting)
        controllingID = by.id
        if lastNotifiedSince != by.since {
            lastNotifiedSince = by.since
            presence.notifyControlled(name: by.name, since: since)
        }
    }

    // MARK: - Reading the host

    private func tick() {
        refreshPermissions()
        if pane.isOn && pane.address == nil { refreshAddress() }
    }

    private func refreshDevices() {
        guard let h = host else { return }
        pane.devices = h.devices().map { d in
            PairedDeviceModel(id: d.id, name: d.name, os: d.os.map { HostOS($0, flavor: d.osFlavor) },
                              lastUsed: d.lastUsed.map { Date(timeIntervalSince1970: TimeInterval($0)) },
                              connectedNow: d.id == controllingID)
        }
    }

    private static func answer(_ s: RemoteCore.PermissionState) -> PermissionAnswer {
        switch s {
        case .allowed: return .allowed
        case .notAllowed: return .notAllowed
        case .reopenToFinish: return .reopenToFinish
        }
    }

    private func refreshPermissions() {
        guard let h = host else { return }
        let p = h.permissions()
        let screen = Self.answer(p.screenRecording)
        let access = Self.answer(p.accessibility)
        if screen == .allowed { screenOpened = false }
        if access == .allowed { accessOpened = false }
        pane.screenRecording = HostPermissionRules.row(screen, settingsOpened: screenOpened)
        pane.accessibility = HostPermissionRules.row(access, settingsOpened: accessOpened)
    }

    /// `IP:PORT` and `ID@IP:PORT` for "connect by address" on another computer: the port is the one the host
    /// bound (`lanPort()`, nil while off), never read from a file.
    private func refreshAddress() {
        guard let h = host, let port = h.lanPort(), let ip = Self.localIPv4() else { return }
        pane.address = HostAddress.hostPort(ip: ip, port: Int(port))
        if let id = try? h.deviceId() {
            pane.fullAddress = HostAddress.string(id: id, ip: ip, port: Int(port))
        }
    }

    /// The first IPv4 address of an `en*` interface (Wi-Fi or Ethernet), not link-local.
    private static func localIPv4() -> String? {
        var list: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&list) == 0, let first = list else { return nil }
        defer { freeifaddrs(list) }
        var cur: UnsafeMutablePointer<ifaddrs>? = first
        while let p = cur {
            let f = p.pointee
            cur = f.ifa_next
            guard let sa = f.ifa_addr, sa.pointee.sa_family == sa_family_t(AF_INET),
                  (f.ifa_flags & UInt32(IFF_UP)) != 0, (f.ifa_flags & UInt32(IFF_LOOPBACK)) == 0,
                  String(cString: f.ifa_name).hasPrefix("en") else { continue }
            var buf = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            if getnameinfo(sa, socklen_t(sa.pointee.sa_len), &buf, socklen_t(buf.count), nil, 0, NI_NUMERICHOST) == 0 {
                let ip = String(cString: buf)
                if !ip.hasPrefix("169.254.") { return ip }
            }
        }
        return nil
    }

    private static func text(_ e: Error) -> String {
        if let t = e as? ThisComputerError, case .Failed(let message) = t { return message }
        return "\(e)"
    }

    private func updatePresence() {
        presence.update(allowed: pane.isOn, controller: pane.controller, startAtLogin: pane.startAtLogin)
    }

    // MARK: - HostPaneActions

    func setAllowed(_ on: Bool) {
        if on { turnOn() } else { turnOff() }
    }

    func openPairing() {
        guard let h = host, pane.isOn else { return }
        Task { [weak self] in
            do {
                let p = try await h.openPairing()
                self?.pane.pairing = HostPairing(code: p.code, expiresAt: Date().addingTimeInterval(TimeInterval(p.expiresInSecs)))
            } catch {
                Log.error("pairing window: \(error)")
            }
        }
    }

    func closePairing() {
        host?.closePairing()
        pane.pairing = nil
        pane.pairingClosedByWrongCodes = false
    }

    func revoke(deviceID: String) {
        guard let h = host else { return }
        Task { [weak self] in
            do {
                try await h.revoke(id: deviceID)
            } catch {
                Log.error("remove a device: \(error)")
            }
            self?.refreshDevices()
        }
    }

    func disconnect() {
        host?.disconnect()
    }

    func setName(_ name: String) {
        store.hostName = name
        pane.name = name
        host?.setName(name: name)
    }

    func setStartAtLogin(_ on: Bool) {
        do {
            if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
        } catch {
            Log.error("start at login: \(error)")
        }
        pane.startAtLogin = SMAppService.mainApp.status == .enabled
        updatePresence()
    }

    func allowScreenRecording() {
        screenOpened = true
        // macOS's own prompt the first time, nothing after; either way System Settings is where it is turned on.
        _ = host?.requestScreenRecording()
        Permissions.openScreenRecording()
        refreshPermissions()
    }

    func allowAccessibility() {
        accessOpened = true
        _ = host?.requestAccessibility()
        Permissions.openAccessibility()
        refreshPermissions()
    }

    /// "Quit and Reopen": macOS applies Screen Recording only to a process that starts after the grant.
    func quitAndReopen() {
        let cfg = NSWorkspace.OpenConfiguration()
        cfg.createsNewApplicationInstance = true
        NSWorkspace.shared.openApplication(at: Bundle.main.bundleURL, configuration: cfg) { _, error in
            DispatchQueue.main.async {
                if let error = error {
                    Log.error("reopen: \(error)")
                    return
                }
                NSApp.terminate(nil)
            }
        }
    }

    func copyAddress() {
        copy(pane.address)
    }

    func copyFullAddress() {
        copy(pane.fullAddress)
    }

    private func copy(_ text: String?) {
        guard let t = text else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(t, forType: .string)
    }

    // MARK: - From the menu-bar extra

    func togglePairing() {
        pane.pairing == nil ? openPairing() : closePairing()
    }
}
