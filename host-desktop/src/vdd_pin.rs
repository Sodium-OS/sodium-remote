// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The one place that names the matched display's driver: Sodium Remote Display, built from
//! SudoMaker's SudoVDA source at [`COMMIT`] with our patches (`packaging/windows/sudovda/patches/`:
//! our own hardware ID, interface and file names, and the in-place mode update) and signed with
//! Sodium Remote's own certificate (`packaging/windows/sudovda/`). The Windows package carries the files in [`DIR`] next to
//! `SodiumRemote.exe`; before anything is installed, each file's SHA-256 must match its pin
//! here.
//!
//! `packaging/windows/sudovda/pin.sh` prints the lines to paste here after a signed build.
//! **Until then the pins are empty and installing fails closed**: no certificate is
//! imported and no driver installed ([`pinned`] says why).

/// SudoMaker/SudoVDA's commit the driver is built from (the build checks it out and applies the patches).
pub const COMMIT: &str = "a4b09fa2aa731a964d0cb5d139cb1e6240e4da12";

/// The driver's folder, relative to the program's.
pub const DIR: &str = r"drivers\sudovda";

/// The device's hardware ID (SodiumRemoteDisplay.inf). Ours alone: another program's SudoVDA
/// (`Root\SudoMaker\SudoVDA`) is a different device and is never touched.
pub const HARDWARE_ID: &str = r"Root\SodiumRemoteDisplay";

pub const INF: &str = "SodiumRemoteDisplay.inf";
pub const DLL: &str = "SodiumRemoteDisplay.dll";
pub const CAT: &str = "sodiumremotedisplay.cat";
/// Our code-signing certificate (public part, DER), imported into the local machine's
/// Root and TrustedPublisher stores while the setting is on.
pub const CER: &str = "sodium-remote-driver-signing.cer";

/// SHA-256 of each file, 64 hex digits (`pin.sh`).
pub const INF_SHA256: &str = "a7a375dba5d69dc5cd8281ba4f0d9a0b58c4cbae230955ab79a4f6a5811c6cdb";
pub const DLL_SHA256: &str = "5b82297361969b6eac623e94cc5667191075ff2cb53c62a54bc7a15066129eec";
pub const CAT_SHA256: &str = "5e64902fc617e472dded8c546a1d5e53522fde8c8e309782977a490df4713218";
pub const CER_SHA256: &str = "8382cd192562d8c109cc221d02589f9853617d4385bcda6dab6b001d07920e92";
/// The certificate's SHA-1 thumbprint, 40 hex digits: how it is found again to be deleted.
pub const CER_SHA1: &str = "497f2f8bc68bdee0694fb11281c573def71deab6";

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

/// Each file's name and SHA-256.
pub type FilePins = Vec<(&'static str, [u8; 32])>;

/// Every file with its SHA-256, and the certificate's thumbprint; or why this build isn't pinned.
pub fn pinned() -> Result<(FilePins, [u8; 20]), String> {
    let why = || "the virtual display driver is not pinned in this build: run packaging/windows/sudovda/pin.sh after signing (sign.sh) and put its lines in host-desktop/src/vdd_pin.rs".to_string();
    let files = [(INF, INF_SHA256), (DLL, DLL_SHA256), (CAT, CAT_SHA256), (CER, CER_SHA256)]
        .into_iter()
        .map(|(f, h)| parse::<32>(h).map(|h| (f, h)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(why)?;
    Ok((files, parse::<20>(CER_SHA1).ok_or_else(why)?))
}
