// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteCore
import RemoteKit
import RemoteUI
import UniformTypeIdentifiers

/// Settings › Server in the app: what the sheets ask for. The core checks every value
/// (`checkServerSettings`, `parseServerLink`, `parseServerToml`: one parser for links, `server.toml` and typed values);
/// the app keeps the result in its preferences and the Keychain (`Store.server`) and starts the core and the host
/// with it. Nothing here saves before the sheet's confirmation: `saveServer` is called by it.
extension AppController: ServerActions {
    // MARK: - Checking

    private func candidate(_ i: ServerLinkInfo, _ source: ServerCandidate.Source) -> ServerCandidate {
        // The core's label is English; the UI's own words for the hosted service are translated.
        ServerCandidate(url: i.url, host: i.host, label: i.official ? String(localized: "Sodium Remote service") : i.host,
                        official: i.official, caPEM: i.caPem, token: i.token, source: source)
    }

    func inspectServerLink(_ text: String) -> Result<ServerCandidate, ServerInputError> {
        do {
            return .success(candidate(try parseServerLink(link: text), .link))
        } catch {
            return .failure(.notALink)
        }
    }

    func inspectServer(url: String, caPEM: String, token: String) -> Result<ServerCandidate, ServerInputError> {
        do {
            return .success(candidate(try checkServerSettings(url: url, caPem: caPEM, token: token), .typed))
        } catch {
            // Say which field: the address alone first, then the certificate.
            if (try? checkServerSettings(url: url, caPem: "", token: "")) == nil { return .failure(.badAddress) }
            return .failure(.badCertificate)
        }
    }

    /// What the Server tab shows for the saved server (nil: none, or the saved value isn't valid).
    func publishServer(_ s: ServerSettings?) {
        guard let s = s, case .success(var c) = inspectServer(url: s.url, caPEM: s.caPEM, token: s.token) else {
            model.server = nil
            model.serverHost = nil
            return
        }
        c.source = .typed
        model.server = c
        model.serverHost = c.host
    }

    // MARK: - Saving

    func saveServer(_ c: ServerCandidate) {
        store.server = ServerSettings(url: c.url, caPEM: c.official ? "" : c.caPEM, token: c.token)
        Log.info("server set: \(c.host)\(c.pinsCA && !c.official ? " (pinned certificate)" : "")")
        serverSettingsChanged()
    }

    func removeServer() {
        store.server = nil
        Log.info("server removed")
        serverSettingsChanged()
    }

    func currentServer() -> ServerCandidate? {
        model.server
    }

    func retryServer() {
        // A restart of the core registers again at once (it also retries by itself).
        if session == nil { serverSettingsChanged() }
    }

    // MARK: - Files

    func chooseCertificateFile() -> (name: String, pem: String)? {
        let p = NSOpenPanel()
        p.canChooseDirectories = false
        p.allowsMultipleSelection = false
        p.allowedContentTypes = ["pem", "crt", "cer"].compactMap { UTType(filenameExtension: $0) } + [.plainText]
        p.message = String(localized: "Choose the server’s certificate authority file.")
        guard p.runModal() == .OK, let url = p.url else { return nil }
        // An unreadable file comes back as text that is no certificate: the sheet says so on the field.
        return (url.lastPathComponent, (try? String(contentsOf: url, encoding: .utf8)) ?? "unreadable")
    }

    /// Import…: a `server.toml` (the Linux and Windows apps' file), confirmed like a link before it is saved.
    func importServerFile() {
        let p = NSOpenPanel()
        p.canChooseDirectories = false
        p.allowsMultipleSelection = false
        p.allowedContentTypes = [UTType(filenameExtension: "toml") ?? .plainText]
        p.message = String(localized: "Choose a server.toml file.")
        guard p.runModal() == .OK, let url = p.url else { return }
        do {
            let text = try String(contentsOf: url, encoding: .utf8)
            // A relative `ca_file` is looked for beside the file, as on Linux and Windows.
            guard let info = try parseServerToml(text: text, baseDir: url.deletingLastPathComponent().path) else {
                alert(String(localized: "That file doesn’t describe a server."))
                return
            }
            confirmServer(candidate(info, .file))
        } catch {
            Log.info("import of \(url.lastPathComponent): \(error)")
            alert(String(localized: "That file doesn’t describe a server."))
        }
    }

    /// Export…: a `server.toml` for the Linux and Windows apps; a pinned CA goes in a file beside it, named in it.
    func exportServerFile() {
        guard let s = store.server else { return }
        let p = NSSavePanel()
        p.nameFieldStringValue = "server.toml"
        p.allowedContentTypes = [UTType(filenameExtension: "toml") ?? .plainText]
        guard p.runModal() == .OK, let url = p.url else { return }
        let caName = s.caPEM.isEmpty ? "" : url.deletingPathExtension().lastPathComponent + "-ca.pem"
        do {
            if !caName.isEmpty {
                try write(Data(s.caPEM.utf8), to: url.deletingLastPathComponent().appendingPathComponent(caName))
            }
            try write(Data(serverToml(url: s.url, caFile: caName, token: s.token).utf8), to: url)
            Log.info("server settings exported to \(url.lastPathComponent)")
        } catch {
            alert(String(localized: "Couldn’t save the file: \(error.localizedDescription)"))
        }
    }

    /// The file holds a token: owner-only, like `server.toml` on Linux.
    private func write(_ data: Data, to url: URL) throws {
        try data.write(to: url, options: [.atomic])
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }

    private func alert(_ text: String) {
        let a = NSAlert()
        a.messageText = text
        a.runModal()
    }

    // MARK: - Small things

    func clipboardText() -> String? {
        NSPasteboard.general.string(forType: .string)
    }

    func openServerGuide() {
        if let u = URL(string: Brand.selfHostingURL) { NSWorkspace.shared.open(u) }
    }
}
