// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! remoted as a library: the capture sources, the GPU conversion, NVENC and the
//! media pipeline; the host session (the actor,
//! the display manager, input, IME, audio, clipboard, keys, the D-Bus API;
//! remote/docs/reference/host-session.md); and `stream`, which joins them to remote-core's
//! `connect::Host` (pairing, the rendezvous, mDNS, ICE, the protocol).
//! The daemon (main.rs) and the test binaries in src/bin use it.

pub mod audio;
pub mod capture;
pub use remote_wlclip as clipboard;
pub mod daemon;
pub mod dbus;
pub mod display;
pub mod gpu;
pub mod ime;
pub mod input;
pub mod keys;
pub mod media;
pub mod nvenc;
pub mod session;
pub mod stream;

use std::path::PathBuf;

/// `$XDG_RUNTIME_DIR`, the per-user 0700 directory every socket and runtime file here lives under. When it is unset it is
/// `/run/user/<uid>` (where systemd puts it): never `/tmp` or any other folder another user could create first. If that
/// does not exist either, later use of it fails with its path in the error, which says what is missing.
pub fn xdg_runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => d.into(),
        _ => PathBuf::from(format!("/run/user/{}", rustix::process::getuid().as_raw())),
    }
}

/// Per-session runtime files (the display snapshot, the saved audio defaults):
/// `$REMOTED_RUNTIME_DIR`, else `$XDG_RUNTIME_DIR/sodiumos`.
pub fn runtime_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("REMOTED_RUNTIME_DIR") {
        return d.into();
    }
    xdg_runtime_dir().join("sodiumos")
}
