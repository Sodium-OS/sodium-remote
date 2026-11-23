// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The one place that names the OpenH264 build Windows downloads.
//!
//! Sodium Remote does not ship OpenH264. When no graphics hardware can encode H.264, it
//! fetches Cisco's own prebuilt DLL on first need (the pattern Firefox uses), because
//! Cisco's patent cover applies to the binaries Cisco builds and distributes. The DLL is
//! checked against [`SHA256`], unpacked into the app's data folder, and its own hash ([`DLL_SHA256`]) is checked again before every load, by full path.
//!
//! `packaging/windows/pin-openh264.sh` reads [`VERSION`] and [`URL`] from this file,
//! fetches the file on a networked machine and prints the checksum to paste into
//! [`SHA256`]. **Until then the pin is a placeholder and everything fails closed**: nothing
//! is downloaded and nothing is loaded ([`pinned`] returns an error).

/// Cisco's release the headers in `win-deps/include/wels` (build-deps.sh) belong to.
pub const VERSION: &str = "2.7.0";

/// Where Cisco publishes the binary. Cisco's host serves it over HTTPS too (checked with a HEAD request on this path), so
/// that is what is used; the checksum below, not the transport, is still what is trusted.
pub const URL: &str = "https://ciscobinary.openh264.org/openh264-2.7.0-win64.dll.bz2";

/// SHA-256 of the `.bz2` file at [`URL`], 64 hex digits.
///
/// PLACEHOLDER: a maintainer or CI runs `remote/packaging/windows/pin-openh264.sh` on a
/// networked machine and paste its output here.
pub const SHA256: &str = "UNPINNED-run-packaging-windows-pin-openh264.sh-and-paste-the-sha256-here";

/// SHA-256 of the unpacked DLL (`DLL_NAME`), 64 hex digits. The host checks it every time it loads the DLL, because the
/// codecs folder is writable by the user while the host may run elevated. `pin-openh264.sh` prints it with `SHA256`.
///
/// PLACEHOLDER, like `SHA256`: until it is replaced, nothing is loaded.
pub const DLL_SHA256: &str = "UNPINNED-run-packaging-windows-pin-openh264.sh-and-paste-the-sha256-here";

/// The unpacked DLL's file name in the codecs folder (the version is in it, so a new pin
/// downloads a new file and the old one is left for cleanup).
pub const DLL_NAME: &str = "openh264-2.7.0-win64.dll";

/// Cisco's required notice, shown wherever the codec is mentioned and listed in About.
pub const NOTICE: &str = "OpenH264 Video Codec provided by Cisco Systems, Inc.";

/// The checksum as bytes, or why the build is not pinned.
pub fn pinned() -> Result<[u8; 32], String> {
    parse_sha256(SHA256).ok_or_else(|| {
        "OpenH264 is not pinned in this build: a maintainer or CI must run packaging/windows/pin-openh264.sh and put its SHA-256 in host-desktop/src/openh264_pin.rs".to_string()
    })
}

/// The unpacked DLL's checksum as bytes, or why the build is not pinned.
pub fn pinned_dll() -> Result<[u8; 32], String> {
    parse_sha256(DLL_SHA256).ok_or_else(|| {
        "OpenH264 is not pinned in this build: a maintainer or CI must run packaging/windows/pin-openh264.sh and put its SHA-256 values in host-desktop/src/openh264_pin.rs".to_string()
    })
}

/// 64 hex digits (any case) to 32 bytes; anything else, and the all-zero value, is not a pin.
pub fn parse_sha256(s: &str) -> Option<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    (out != [0u8; 32]).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_placeholder_is_not_a_pin() {
        assert!(pinned().is_err(), "a build with the placeholder must fail closed");
        assert!(pinned_dll().is_err());
        assert!(parse_sha256(&"0".repeat(64)).is_none());
        assert!(parse_sha256(&"g".repeat(64)).is_none());
        assert_eq!(parse_sha256(&"AB".repeat(32)), Some([0xab; 32]));
    }

    #[test]
    fn the_names_agree() {
        assert!(URL.ends_with(&format!("openh264-{VERSION}-win64.dll.bz2")));
        assert_eq!(DLL_NAME, format!("openh264-{VERSION}-win64.dll"));
        assert_eq!(NOTICE, "OpenH264 Video Codec provided by Cisco Systems, Inc.");
    }
}
