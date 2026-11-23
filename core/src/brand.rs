// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Every public identifier of Sodium Remote, in one place.
//!
//! The Qt app (C++/QML) and the Mac app (Swift) keep their own constants modules
//! with the same names and values; this one is the reference. A grep for an old
//! identifier should find it only here (the `OLD_*` migration values) and in the
//! migration code that reads them.
//!
//! What stays out of this module on purpose: the SodiumOS integration names
//! (D-Bus `org.sodiumos.Remote`, `.Dev`, `.Ime`; `sodiumos-remoted.service`; the
//! capture socket `$XDG_RUNTIME_DIR/sodiumos/remote-capture.sock`). They name the
//! SodiumOS host's contract with its desktop, not the product (open-source plan 5.2).

/// The product's name. A trademark: never translated.
pub const PRODUCT_NAME: &str = "Sodium Remote";
/// The release this build is (the workspace version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The license of every crate in the workspace.
pub const LICENSE: &str = "AGPL-3.0-only";
/// Where the corresponding source is offered (AGPL-3.0 section 13): `--version`,
/// About › Source code, and the rendezvous challenge.
pub const SOURCE_URL: &str = "https://github.com/sodium-os/sodium-remote";
/// "How to run your own server" (Settings › Server).
pub const SELF_HOSTING_URL: &str = "https://github.com/sodium-os/sodium-remote/blob/main/docs/self-hosting.md";
/// "The Linux host" guide: what the generic Linux host can and cannot do, and `sodium-remote-host` for always-on use.
pub const LINUX_HOST_URL: &str = "https://github.com/sodium-os/sodium-remote/blob/main/docs/hosts/linux.md";
/// The path the relay serves its WebSocket on, appended to a bare host typed in Add a server.
pub const DEFAULT_SERVER_PATH: &str = "/remote";

// --- LAN discovery ---------------------------------------------------------

/// The mDNS service type, fully qualified (DNS-SD).
pub const MDNS_SERVICE: &str = "_sodium-remote._tcp.local.";

// --- Files and directories -------------------------------------------------

/// Linux: `$XDG_DATA_HOME/sodium-remote/{host,client}` and
/// `$XDG_CONFIG_HOME/sodium-remote/`.
pub const LINUX_DIR_NAME: &str = "sodium-remote";
/// Linux, system-wide configuration (read after the user's own).
pub const LINUX_SYSTEM_CONFIG_DIR: &str = "/etc/sodium-remote";
/// Windows: `%LOCALAPPDATA%\SodiumRemote\{host,client}` (data) and
/// `%APPDATA%\SodiumRemote\` (configuration).
pub const WINDOWS_DIR_NAME: &str = "SodiumRemote";
/// The host half and the client half of the data directory. Each holds its own
/// `device.key` and `paired.json`.
pub const HOST_SUBDIR: &str = "host";
pub const CLIENT_SUBDIR: &str = "client";
/// The relay configuration file: `[server] url, ca_file, token`.
pub const SERVER_TOML: &str = "server.toml";
/// Named networks for the path chip (`[networks] "CIDR" = "label"`).
pub const NETWORKS_TOML: &str = "networks.toml";

/// Overrides the data directory (the client's, for the app; tests pass their own).
pub const ENV_DATA: &str = "SODIUM_REMOTE_DATA";

// --- Before the rename (read only by the first-start migration) -------------

/// The old name of [`ENV_DATA`], still honoured when the new one is unset.
pub const OLD_ENV_DATA: &str = "SODIUMOS_REMOTE_DATA";
/// The host's old data directory, relative to `$XDG_DATA_HOME`.
pub const OLD_LINUX_HOST_DIR: &str = "sodiumos/remote";
/// The app's old (client) data directory, relative to `$XDG_DATA_HOME`.
pub const OLD_LINUX_CLIENT_DIR: &str = "sodiumos/remote-client";
/// The old clipboard marker, still recognised as ours.
pub const OLD_CLIPBOARD_MIME: &str = "application/x-sodiumos-remote";

// --- Clipboard ---------------------------------------------------------------

/// Marks clipboard offers Sodium Remote made itself, so it never syncs its own
/// copies back.
pub const CLIPBOARD_MIME: &str = "application/x-sodium-remote";

// --- Binaries ----------------------------------------------------------------

/// The desktop host (Windows, macOS, generic Linux).
pub const BIN_HOST: &str = "sodium-remote-host";
/// The app (Linux).
pub const BIN_APP: &str = "sodium-remote";
/// The app (Windows).
pub const BIN_APP_WINDOWS: &str = "SodiumRemote.exe";
/// The SodiumOS host.
pub const BIN_SODIUMOS_HOST: &str = "remoted";
/// The rendezvous server (crate `remote-rendezvous`).
pub const BIN_SERVER: &str = "sodium-remote-server";
/// The Mac app's Rust static library (crate `remote-ffi` surface of host-desktop
/// and remote-core).
pub const MAC_RUST_LIB: &str = "libremote_ffi.a";
/// The Mac app's bundle identifiers.
pub const MAC_BUNDLE_ID: &str = "com.sodiumos.remote";
pub const MAC_BUNDLE_ID_DEV: &str = "com.sodiumos.remote.dev";

// --- The relay -----------------------------------------------------------------

/// The hosted service's host name. Nothing dials it unless a `server.toml`
/// or a link names it; when one does, it is trusted through Web PKI without a CA
/// prompt and labelled [`OFFICIAL_SERVICE_NAME`].
pub const OFFICIAL_SERVER_HOST: &str = "remote.sodiumos.com";
/// How the UI names the hosted service.
pub const OFFICIAL_SERVICE_NAME: &str = "Sodium Remote service";
/// The scheme of server links and their QR codes: `sodium-remote://server?url=…`.
pub const LINK_SCHEME: &str = "sodium-remote";

/// `<binary> 0.1.0`, the license and where the source is: what `--version` prints.
pub fn version_text(binary: &str) -> String {
    format!("{binary} {VERSION}\n{PRODUCT_NAME}, licensed {LICENSE}.\nSource code: {SOURCE_URL}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_help_url_lives_in_the_source_repository_and_the_qt_and_mac_constants_mirror_it() {
        assert_eq!(SELF_HOSTING_URL, format!("{SOURCE_URL}/blob/main/docs/self-hosting.md"));
        assert_eq!(LINUX_HOST_URL, format!("{SOURCE_URL}/blob/main/docs/hosts/linux.md"));
        // app/src/brand.h repeats these values (a header can't import them): they must agree.
        let h = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../app/src/brand.h"));
        if let Ok(h) = h {
            for (name, value) in [("SourceUrl", SOURCE_URL), ("SelfHostingUrl", SELF_HOSTING_URL), ("LinuxHostUrl", LINUX_HOST_URL), ("DefaultServerPath", DEFAULT_SERVER_PATH)] {
                assert!(h.contains(&format!("{name}[] = \"{value}\"")), "brand.h: {name}");
            }
        }
        // So does the Mac's Brand.swift.
        let swift = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../client-mac/Sources/RemoteKit/Brand.swift"));
        if let Ok(swift) = swift {
            for (name, value) in [("sourceURL", SOURCE_URL), ("selfHostingURL", SELF_HOSTING_URL), ("defaultServerPath", DEFAULT_SERVER_PATH)] {
                assert!(swift.contains(&format!("static let {name} = \"{value}\"")), "Brand.swift: {name}");
            }
        }
    }
}
