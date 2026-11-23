// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation
import RemoteCore
import RemoteKit
import Security

/// The core's storage (session-api §2) for the client half and, as `HostKeyStore`, for the host half of this app:
/// `device.key` (the Ed25519 key, PKCS#8 PEM) in the login Keychain, everything else (`paired.json`) as files in
/// `directory`: `~/Library/Application Support/<bundle ID>` for the client, its `Host` folder for the host.
///
/// The client and the host are two devices, so their keys are two Keychain items under one service: the host's
/// account has the prefix `host/`.
///
/// A key is imported once, on its first read under this build's identity, and never silently regenerated:
/// - from a file `directory/device.key` (the host's key was a file before the Keychain; so was any key written by a
///   build before the client used the Keychain): saved to the Keychain, then the file is deleted;
///
/// With ad-hoc signing every rebuild is a new app to the Keychain, so macOS asks once per build to let it read the
/// key ("Always Allow"); a self-signed identity avoids that (BUILD.md).
final class KeychainStore: KeyStore, HostKeyStore, @unchecked Sendable {
    static let service = AppIdentity.bundleID
    let directory: URL
    /// "" for the client, "host/" for the host.
    private let prefix: String

    /// The client half: Application Support/<bundle ID>.
    convenience init() {
        self.init(directory: KeychainStore.baseDirectory(), prefix: "")
    }

    /// The host half: `directory` is the host's own folder (`Store.hostDir`).
    static func host(directory: URL) -> KeychainStore {
        KeychainStore(directory: directory, prefix: "host/")
    }

    static func baseDirectory() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Application Support")
        return base.appendingPathComponent(AppIdentity.dataFolder, isDirectory: true)
    }

    init(directory: URL, prefix: String) {
        self.directory = directory
        self.prefix = prefix
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    private func inKeychain(_ name: String) -> Bool {
        name.hasSuffix(".key")
    }

    private func query(_ name: String, service: String = KeychainStore.service) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: prefix + name,
        ]
    }

    /// Names whose last Keychain read failed with something other than "not found" (locked, denied, damaged): `load` said
    /// nothing, and `save` must not turn that into a new identity over the one that is there.
    private var unreadable: Set<String> = []
    private let lock = NSLock()

    private func setUnreadable(_ name: String, _ on: Bool) {
        lock.lock()
        defer { lock.unlock() }
        if on { unreadable.insert(name) } else { unreadable.remove(name) }
    }

    private func isUnreadable(_ name: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return unreadable.contains(name)
    }

    /// The item's data; nil when there is none (`failed` false) or when the read failed (`failed` true).
    private func readKeychain(_ name: String, service: String) -> (data: Data?, failed: Bool) {
        var q = query(name, service: service)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        let st = SecItemCopyMatching(q as CFDictionary, &out)
        if st == errSecSuccess { return (out as? Data, out as? Data == nil) }
        if st == errSecItemNotFound { return (nil, false) }
        Log.error("Keychain read of \(prefix + name): OSStatus \(st)")
        return (nil, true)
    }

    func load(name: String) -> Data? {
        guard inKeychain(name) else {
            return FileManager.default.contents(atPath: directory.appendingPathComponent(name).path)
        }
        let r = readKeychain(name, service: KeychainStore.service)
        setUnreadable(name, r.failed)
        if let d = r.data { return d }
        // An error is not "not found": no key file is imported and no new identity follows (save refuses until a read works).
        if r.failed { return nil }
        // First read under this identity: import a key file if there is one.
        let file = directory.appendingPathComponent(name)
        if let d = FileManager.default.contents(atPath: file.path), !d.isEmpty {
            if save(name: name, data: d) {
                try? FileManager.default.removeItem(at: file)
                Log.info("\(prefix + name) moved from a file into the Keychain")
                return d
            }
            return d
        }
        return nil
    }

    func save(name: String, data: Data) -> Bool {
        if inKeychain(name) {
            if isUnreadable(name) {
                Log.error("Keychain write of \(prefix + name) refused: the existing item could not be read")
                return false
            }
            let q = query(name)
            let update: [String: Any] = [kSecValueData as String: data]
            var st = SecItemUpdate(q as CFDictionary, update as CFDictionary)
            if st == errSecItemNotFound {
                var add = q
                add[kSecValueData as String] = data
                add[kSecAttrLabel as String] = prefix.isEmpty ? "\(Brand.productName) device key" : "\(Brand.productName) host key"
                add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
                st = SecItemAdd(add as CFDictionary, nil)
            }
            if st != errSecSuccess { Log.error("Keychain write of \(prefix + name): OSStatus \(st)") }
            return st == errSecSuccess
        }
        let url = directory.appendingPathComponent(name)
        do {
            try data.write(to: url, options: [.atomic])
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
            return true
        } catch {
            Log.error("writing \(url.path): \(error)")
            return false
        }
    }

    func remove(name: String) {
        if inKeychain(name) {
            SecItemDelete(query(name) as CFDictionary)
        } else {
            try? FileManager.default.removeItem(at: directory.appendingPathComponent(name))
        }
    }
}

/// The server's enrollment token: a secret that opens a registration, kept in the login
/// Keychain under the app's service, not in the preferences. (The pinned CA is public and stays in the preferences.)
enum ServerSecret {
    private static var query: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: AppIdentity.bundleID,
            kSecAttrAccount as String: "server-token",
        ]
    }

    static func token() -> String? {
        var q = query
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var out: CFTypeRef?
        let st = SecItemCopyMatching(q as CFDictionary, &out)
        if st != errSecSuccess && st != errSecItemNotFound { Log.error("Keychain read of the server token: OSStatus \(st)") }
        guard st == errSecSuccess, let d = out as? Data else { return nil }
        return String(data: d, encoding: .utf8)
    }

    /// nil or empty: no token (the item is deleted).
    static func setToken(_ token: String?) {
        guard let token = token, !token.isEmpty, let data = token.data(using: .utf8) else {
            SecItemDelete(query as CFDictionary)
            return
        }
        var st = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        if st == errSecItemNotFound {
            var add = query
            add[kSecValueData as String] = data
            add[kSecAttrLabel as String] = "\(Brand.productName) server token"
            add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            st = SecItemAdd(add as CFDictionary, nil)
        }
        if st != errSecSuccess { Log.error("Keychain write of the server token: OSStatus \(st)") }
    }
}

/// Where video frames go without a hop to the main thread (the decoder).
protocol VideoSink: AnyObject {
    func videoFrame(_ f: VideoFrame)
    func videoFrameLost(_ id: UInt32)
}

/// The core's listener. `onEvent` runs on a Rust thread: video frames go
/// straight to the current session's decoder; everything else hops to main.
final class CoreListener: ClientListener, @unchecked Sendable {
    private let lock = NSLock()
    private weak var sink: VideoSink?
    /// Set on main; called on main.
    var handler: (@MainActor (RemoteEvent) -> Void)?

    func setVideoSink(_ s: VideoSink?) {
        lock.lock()
        sink = s
        lock.unlock()
    }

    private func currentSink() -> VideoSink? {
        lock.lock()
        defer { lock.unlock() }
        return sink
    }

    func onEvent(event: RemoteEvent) {
        switch event {
        case .videoFrame(let f):
            currentSink()?.videoFrame(f)
        case .videoFrameLost(let id):
            currentSink()?.videoFrameLost(id)
        default:
            onMain { [weak self] in
                self?.handler?(event)
            }
        }
    }
}
