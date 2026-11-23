// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import Foundation
import RemoteKit
import RemoteUI

/// What this Mac remembers about each paired computer, for its card: the
/// name it was given here, when it was last used and over which path, and the
/// last frame (kept locally, never sent anywhere).
struct HostMeta: Codable {
    var localName: String?
    var lastConnected: Date?
    var lastSeen: Date?
    var lastRoute: RouteKind?
    var lastRttMs: Double?
    /// Known from the last connection (the core's presence doesn't carry them).
    var locked = false
    var busyBy: String?
    /// What it runs, from its last `Welcome` (the OS pill on its card). Stored as a case, never as text.
    var hostOS: HostOS?
}

/// UserDefaults-backed settings and host metadata.
@MainActor
final class Store {
    private let defaults = UserDefaults.standard
    private static let settingsKey = "RemoteSettings"
    private static let metaKey = "RemoteHosts"
    private static let welcomeKey = "RemoteWelcomeDone"
    private static let hintsKey = "RemoteLanHints"
    private static let deviceNameKey = "RemoteDeviceName"
    private static let tcpNetworksKey = "RemoteTcpToastNetworks"
    private static let hostAllowedKey = "RemoteHostAllowed"
    private static let hostNameKey = "RemoteHostName"
    private static let cmdIsWinKey = "RemoteCmdIsWin"
    private static let showPreviewsKey = "RemoteShowPreviews"
    private static let serverURLKey = "RemoteServerURL"
    private static let serverCAKey = "RemoteServerCA"

    private(set) var meta: [String: HostMeta] = [:]

    init() {
        if let d = defaults.data(forKey: Store.metaKey), let m = try? JSONDecoder().decode([String: HostMeta].self, from: d) {
            meta = m
        }
    }

    func loadSettings() -> RemoteSettings {
        guard let d = defaults.data(forKey: Store.settingsKey), var s = try? JSONDecoder().decode(RemoteSettings.self, from: d) else {
            defaults.set(true, forKey: Store.cmdIsWinKey)
            return RemoteSettings()
        }
        // ⌘ became the computer's Win key on 2026-11-17 (PC keys): settings
        // saved before then move once; Mac keys stays a choice.
        if !defaults.bool(forKey: Store.cmdIsWinKey) {
            defaults.set(true, forKey: Store.cmdIsWinKey)
            s.keyboard = .pc
            saveSettings(s)
        }
        return s
    }

    func saveSettings(_ s: RemoteSettings) {
        var keep = s
        // "This session" home monitor choices aren't remembered.
        if keep.rememberHome == .thisSession { keep.homeMode = .mirror }
        if let d = try? JSONEncoder().encode(keep) { defaults.set(d, forKey: Store.settingsKey) }
    }

    var welcomeDone: Bool {
        get { defaults.bool(forKey: Store.welcomeKey) }
        set { defaults.set(newValue, forKey: Store.welcomeKey) }
    }

    /// `ID@IP:PORT` LAN listeners, for when mDNS can't be used on the Mac
    /// (`defaults write com.sodiumos.remote RemoteLanHints -array 'ID@192.0.2.10:47100'`).
    var lanHints: [String] {
        defaults.stringArray(forKey: Store.hintsKey) ?? []
    }

    /// The Sharing computer name at the last launch: a different one
    /// now means System Settings renamed this Mac since.
    var deviceName: String? {
        get { defaults.string(forKey: Store.deviceNameKey) }
        set { defaults.set(newValue, forKey: Store.deviceNameKey) }
    }

    /// "Allow remote control of this computer": the switch is the app's own setting; it is on again at launch.
    var hostAllowed: Bool {
        get { defaults.bool(forKey: Store.hostAllowedKey) }
        set { defaults.set(newValue, forKey: Store.hostAllowedKey) }
    }

    /// "Name shown to other computers"; empty means the Sharing name.
    var hostName: String {
        get { defaults.string(forKey: Store.hostNameKey) ?? "" }
        set { defaults.set(newValue, forKey: Store.hostNameKey) }
    }

    /// "Show previews": a picture of each computer on its card. On unless turned off.
    var showPreviews: Bool {
        get { defaults.object(forKey: Store.showPreviewsKey) as? Bool ?? true }
        set { defaults.set(newValue, forKey: Store.showPreviewsKey) }
    }

    /// The configured server's address (`wss://host[:port]/path`), or nil: none. The Mac keeps the server in
    /// its preferences (URL and CA) and the Keychain (the enrollment token); Settings › Server sets it, and
    /// Import… and Export… read and write the same values as a `server.toml`.
    var serverURL: String? {
        get {
            let u = defaults.string(forKey: Store.serverURLKey) ?? ""
            return u.isEmpty ? nil : u
        }
        set { defaults.set(newValue ?? "", forKey: Store.serverURLKey) }
    }

    /// The host of the server address, for the UI.
    var serverHost: String? {
        serverURL.map { ServerAddress.host(of: $0) }.flatMap { $0.isEmpty ? nil : $0 }
    }

    /// The whole server setting; nil: none. Setting nil clears the URL, the CA and the token.
    var server: ServerSettings? {
        get {
            guard let url = serverURL else { return nil }
            return ServerSettings(url: url, caPEM: defaults.string(forKey: Store.serverCAKey) ?? "", token: ServerSecret.token() ?? "")
        }
        set {
            serverURL = newValue?.url
            defaults.set(newValue?.caPEM ?? "", forKey: Store.serverCAKey)
            ServerSecret.setToken(newValue?.token)
        }
    }

    /// Where the host keeps its key, paired devices and `host.txt`: apart from the client's files, because as a
    /// host this Mac is another device.
    var hostDir: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Application Support")
        return base.appendingPathComponent(AppIdentity.dataFolder + "/Host", isDirectory: true)
    }

    /// Networks the TCP-relay toast was shown on (once per network).
    func tcpToastShown(network: String) -> Bool {
        (defaults.stringArray(forKey: Store.tcpNetworksKey) ?? []).contains(network)
    }

    func markTcpToast(network: String) {
        var a = defaults.stringArray(forKey: Store.tcpNetworksKey) ?? []
        if !a.contains(network) { a.append(network) }
        defaults.set(a, forKey: Store.tcpNetworksKey)
    }

    func hostMeta(_ id: String) -> HostMeta {
        meta[id] ?? HostMeta()
    }

    func update(_ id: String, _ change: (inout HostMeta) -> Void) {
        var m = meta[id] ?? HostMeta()
        change(&m)
        meta[id] = m
        if let d = try? JSONEncoder().encode(meta) { defaults.set(d, forKey: Store.metaKey) }
    }

    func forget(_ id: String) {
        meta.removeValue(forKey: id)
        if let d = try? JSONEncoder().encode(meta) { defaults.set(d, forKey: Store.metaKey) }
        try? FileManager.default.removeItem(at: thumbnailURL(id))
    }

    // MARK: - Thumbnails

    var thumbnailsDir: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Application Support")
        let d = base.appendingPathComponent(AppIdentity.dataFolder + "/thumbs", isDirectory: true)
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    func thumbnailURL(_ id: String) -> URL {
        thumbnailsDir.appendingPathComponent(id + ".png")
    }

    func thumbnail(_ id: String) -> NSImage? {
        showPreviews ? NSImage(contentsOf: thumbnailURL(id)) : nil
    }

    /// Show previews went off: the saved pictures are deleted.
    func deleteThumbnails() {
        let fm = FileManager.default
        for u in (try? fm.contentsOfDirectory(at: thumbnailsDir, includingPropertiesForKeys: nil)) ?? [] where u.pathExtension == "png" {
            try? fm.removeItem(at: u)
        }
    }

    func saveThumbnail(_ id: String, _ image: CGImage) {
        guard showPreviews else { return }
        let rep = NSBitmapImageRep(cgImage: image)
        if let png = rep.representation(using: .png, properties: [:]) {
            try? png.write(to: thumbnailURL(id), options: [.atomic])
        }
    }

    /// Stats files: ~/Library/Logs/Remote.
    var statsDir: URL {
        let lib = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library")
        return lib.appendingPathComponent("Logs/" + AppIdentity.dataFolder, isDirectory: true)
    }
}
