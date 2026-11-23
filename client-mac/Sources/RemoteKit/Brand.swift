// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation

/// Every public identifier of Sodium Remote, in one place. The reference is
/// `remote/core/src/brand.rs`: the same names and values. The Qt app keeps its own copy in C++.
public enum Brand {
    /// The product's name. A trademark: never translated.
    public static let productName = "Sodium Remote"
    /// The license of every crate in the workspace.
    public static let license = "AGPL-3.0-only"
    /// Where the corresponding source is offered (AGPL-3.0 section 13): About › Source code.
    public static let sourceURL = "https://github.com/sodium-os/sodium-remote"

    /// The mDNS service type without the domain, as Network.framework and Info.plist's NSBonjourServices want it.
    public static let mdnsService = "_sodium-remote._tcp"

    /// The Mac app's bundle identifiers: a release, and the test build (`build-app.sh --test-id`).
    public static let bundleID = "com.sodiumos.remote"
    public static let bundleIDDev = "com.sodiumos.remote.dev"
    /// The Rust static library the Swift package links.
    public static let rustLib = "libremote_ffi.a"

    /// The hosted service's host name. Nothing dials it unless a link or the Server settings name it; when
    /// one does, it is trusted through Web PKI without a certificate prompt and labelled `officialServiceName`.
    public static let officialServerHost = "remote.sodiumos.com"
    /// How the UI names the hosted service. (UI copy for it is `String(localized: "Sodium Remote service")`.)
    public static let officialServiceName = "Sodium Remote service"
    /// How to run your own server (Settings › Server): the self-hosting guide in the repository.
    public static let selfHostingURL = "https://github.com/sodium-os/sodium-remote/blob/main/docs/self-hosting.md"
    /// The path a server address gets when it has none (`brand::DEFAULT_SERVER_PATH`), so a bare host works.
    public static let defaultServerPath = "/remote"
    /// The scheme of server links: `sodium-remote://server?url=…`.
    public static let linkScheme = "sodium-remote"

    /// The identity of a build, from its bundle ID alone (no Info.plist key): a bundle ID ending in `.dev` is the
    /// test build. A binary run outside a bundle has no ID and is never a release.
    public static func isTestBuild(bundleID id: String) -> Bool {
        id.hasSuffix(".dev")
    }

    /// A label under the bundle ID (queue labels, toolbar identifiers, notification categories): `<id>.<part>`.
    public static func label(_ part: String, bundleID id: String = Brand.bundleID) -> String {
        id + "." + part
    }
}
