// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The one place that names the matched display's driver: SudoVDA, built from SudoMaker's
//! source at [`COMMIT`] and signed with Sodium Remote's own certificate
//! (`packaging/windows/sudovda/`). The Windows package carries the files in [`DIR`] next to
//! `SodiumRemote.exe`; before anything is installed, each file's SHA-256 must match its pin
//! here.
//!
//! `packaging/windows/sudovda/pin.sh` prints the lines to paste here after a signed build.
//! **Until then the pins are empty and installing fails closed**: no certificate is
//! imported and no driver installed ([`pinned`] says why).

/// SudoMaker/SudoVDA's commit the driver is built from (`build-sign.ps1` checks it out).
pub const COMMIT: &str = "a4b09fa2aa731a964d0cb5d139cb1e6240e4da12";

/// The driver's folder, relative to the program's.
pub const DIR: &str = r"drivers\sudovda";

/// The device's hardware ID (SudoVDA.inf).
pub const HARDWARE_ID: &str = r"Root\SudoMaker\SudoVDA";

pub const INF: &str = "SudoVDA.inf";
pub const DLL: &str = "SudoVDA.dll";
pub const CAT: &str = "sudovda.cat";
/// Our code-signing certificate (public part, DER), imported into the local machine's
/// Root and TrustedPublisher stores while the setting is on.
pub const CER: &str = "sodium-remote-driver-signing.cer";

/// SHA-256 of each file, 64 hex digits (`pin.sh`).
pub const INF_SHA256: &str = "";
pub const DLL_SHA256: &str = "";
pub const CAT_SHA256: &str = "";
pub const CER_SHA256: &str = "";
/// The certificate's SHA-1 thumbprint, 40 hex digits: how it is found again to be deleted.
pub const CER_SHA1: &str = "";

fn parse<const N: usize>(s: &str) -> Option<[u8; N]> {
    let s = s.trim();
    if s.len() != 2 * N || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    (out != [0u8; N]).then_some(out)
}

/// Every file with its SHA-256, and the certificate's thumbprint; or why this build isn't pinned.
pub fn pinned() -> Result<(Vec<(&'static str, [u8; 32])>, [u8; 20]), String> {
    let why = || "the virtual display driver is not pinned in this build: run packaging/windows/sudovda/pin.sh after a signed build and put its lines in host-desktop/src/vdd_pin.rs".to_string();
    let files = [(INF, INF_SHA256), (DLL, DLL_SHA256), (CAT, CAT_SHA256), (CER, CER_SHA256)]
        .into_iter()
        .map(|(f, h)| parse::<32>(h).map(|h| (f, h)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(why)?;
    Ok((files, parse::<20>(CER_SHA1).ok_or_else(why)?))
}
