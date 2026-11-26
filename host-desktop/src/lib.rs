// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Sodium Remote's portable host: the
//! session and media glue on remote-core's `connect::Host`, written against
//! `backend::Backend`, with a backend per OS.
//!
//! - `session`: pairing, LAN and mDNS, `Welcome` for a physical display, the key table
//!   with the foreground app, input, display selection, clipboard, audio, stats.
//! - `media`: capture → encode → core, frames only on damage, core's rate, one
//!   refresh 150 ms after the last change, keyframes on request.
//! - `policy`: the codec, the stream size, the refresh and rate rules, pointer maths.
//! - `encode`, `opus`: the C shim (FFmpeg, OpenH264 loaded at run time, swscale, Opus).
//! - `softvideo`, `openh264_pin`, `bunzip`, `winhttp`: software H.264 from Cisco's DLL, fetched on first need.
//! - `hid`: HID usages → PC scan codes.
//! - `vdisplay`: the matched display (a virtual monitor the size of the client's window), behind
//!   the `VirtualDisplay` and `Topology` traits; `vdd_pin`: its driver's files and checksums;
//!   `settings`: the app's `settings.json`.
//! - `win`: the Windows backend; `mac`: the macOS backend;
//!   `linux`: the generic Linux backend through the desktop portal
//!   (`docs/reference/host-linux.md`, feature `linux-portal`); `fake`: synthetic frames for tests.
//! - `ffi` (feature `ffi`): the UniFFI surface for Remote.app's "This computer" pane.

#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();

pub mod backend;
#[cfg(not(target_os = "macos"))]
pub mod bunzip;
#[cfg(not(target_os = "macos"))]
pub mod encode;
pub mod fake;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod hid;
#[cfg(all(target_os = "linux", feature = "linux-portal"))]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod mac;
pub mod media;
#[cfg(not(target_os = "macos"))]
pub mod openh264_pin;
pub mod opus;
pub mod policy;
pub mod session;
pub mod settings;
#[cfg(not(target_os = "macos"))]
pub mod softvideo;
pub mod vdd_pin;
pub mod vdisplay;
#[cfg(windows)]
pub mod win;

/// This OS's own backend, as the program and the apps' "This computer" panes start it:
/// Windows (with `soft`, the app's software video), the Mac, or the generic Linux portal
/// backend (`data` holds its restore token; OpenH264 is the system's library).
pub fn platform_backend(
    data: &std::path::Path,
    encoder: Option<&str>,
    #[cfg(not(target_os = "macos"))] soft: std::sync::Arc<softvideo::SoftwareVideo>,
) -> anyhow::Result<std::sync::Arc<dyn backend::Backend>> {
    let _ = (data, encoder);
    #[cfg(not(target_os = "macos"))]
    let _ = &soft;
    #[cfg(windows)]
    return Ok(std::sync::Arc::new(win::WinBackend::with_software(encoder, soft)?));
    #[cfg(target_os = "macos")]
    return Ok(std::sync::Arc::new(mac::MacBackend::new(encoder)?));
    #[cfg(all(target_os = "linux", feature = "linux-portal"))]
    return Ok(std::sync::Arc::new(linux::PortalBackend::new(data.to_path_buf(), encoder)?));
    #[allow(unreachable_code)]
    Err(anyhow::anyhow!("no backend for this OS in this build"))
}
#[cfg(windows)]
pub mod winhttp;

/// At the app's start: undoes a matched display that a stopped host left up (its journal in `data`; `vdisplay::recover`).
/// Windows only; nothing elsewhere.
pub fn recover_matched_display(data: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        let journal = data.join(vdisplay::JOURNAL);
        if !journal.exists() {
            return false;
        }
        let driver = win::sudovda::SudoVda::open().ok();
        return vdisplay::recover(&journal, driver.as_ref().map(|d| d as &dyn vdisplay::VirtualDisplay), &win::topology::WinTopology::new());
    }
    #[allow(unreachable_code)]
    {
        let _ = data;
        false
    }
}
