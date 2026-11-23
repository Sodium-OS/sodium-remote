// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation

/// The Mac's server settings: the same three values as `server.toml`'s `[server]`
/// table. The app keeps them in its preferences (the token in the Keychain) and passes them to the core through
/// `ClientSettings` and `ThisComputer.setServer`; Import… and Export… read and write a `server.toml`.
public struct ServerSettings: Equatable, Codable, Sendable {
    /// `wss://host[:port]/path`.
    public var url: String
    /// A CA to pin, PEM; empty: Web PKI.
    public var caPEM: String
    /// The enrollment token; empty: none.
    public var token: String

    public init(url: String, caPEM: String = "", token: String = "") {
        self.url = url
        self.caPEM = caPEM
        self.token = token
    }
}

/// Why a typed server address was refused.
public enum ServerAddressError: Error, Equatable, Sendable {
    /// `http://` or `ws://`: never upgraded, since the person asked for an unencrypted connection.
    case insecure
    /// Empty, or another scheme.
    case invalid
}

/// What a person types into the Add a server sheet, made into what the core wants.
public enum ServerAddress {
    /// The same rules as the core's `server_config::normalize_address` (the Qt app uses that one): a bare host
    /// (`relay.example.org`, `relay.example.org:8443`) gets `wss://` and `Brand.defaultServerPath`; `https://` and
    /// `wss://` become `wss://` (a path is kept as typed, else the default is added); `http://` and `ws://` are
    /// refused, never upgraded.
    public static func normalize(_ typed: String) -> Result<String, ServerAddressError> {
        let t = typed.trimmingCharacters(in: .whitespacesAndNewlines)
        if t.isEmpty { return .failure(.invalid) }
        var rest = Substring(t)
        if let r = t.range(of: "://") {
            switch t[..<r.lowerBound].lowercased() {
            case "https", "wss": rest = t[r.upperBound...]
            case "http", "ws": return .failure(.insecure)
            default: return .failure(.invalid)
            }
        }
        if rest.isEmpty || "/?#".contains(rest.first!) || rest.contains(where: { $0.isWhitespace }) { return .failure(.invalid) }
        let hostEnd = rest.firstIndex(where: { "/?#".contains($0) }) ?? rest.endIndex
        var path = String(rest[hostEnd...])
        if path.isEmpty || path == "/" { path = Brand.defaultServerPath }
        return .success("wss://\(rest[..<hostEnd])\(path)")
    }

    /// The host of a server URL ("" when there is none).
    public static func host(of url: String) -> String {
        URL(string: url)?.host ?? ""
    }

    /// The text is a `sodium-remote://server…` link (the check is the core's; this one picks the sheet's first stage).
    public static func looksLikeLink(_ text: String) -> Bool {
        text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased().hasPrefix(Brand.linkScheme + "://")
    }
}

/// What a server's refusal means to a person: the four codes the apps word themselves
/// (`token_invalid`, `token_expired`, `quota_exceeded`, `payment_required`) and `host_not_allowed`, which means
/// "enrol first". The server's own text is never shown for these.
public enum ServerProblem: Equatable, Sendable {
    case tokenInvalid, tokenExpired, tokenNeeded, quotaExceeded, paymentRequired

    /// The core's `Rendezvous` event error text (`RdvError`'s Display: "rendezvous refused: <code> (<message>)").
    /// `nil` for anything else (an unreachable server, a timeout, a code the apps don't word).
    public static func parse(_ error: String?) -> ServerProblem? {
        guard let e = error, let r = e.range(of: "rendezvous refused: ") else { return nil }
        let code = e[r.upperBound...].prefix { !$0.isWhitespace }
        switch code {
        case "token_invalid": return .tokenInvalid
        case "token_expired": return .tokenExpired
        case "host_not_allowed": return .tokenNeeded
        case "quota_exceeded": return .quotaExceeded
        case "payment_required": return .paymentRequired
        default: return nil
        }
    }
}
