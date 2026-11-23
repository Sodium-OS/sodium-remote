// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! macOS privacy permissions (TCC) the host needs:
//!
//! - **Screen Recording** ("Screen & System Audio Recording"): ScreenCaptureKit's
//!   picture and audio. `CGPreflightScreenCaptureAccess` reads it without asking;
//!   `CGRequestScreenCaptureAccess` shows macOS's prompt once (later it only says no),
//!   and an Allow takes effect when the process is started again.
//! - **Accessibility**: posting keyboard and mouse events. `AXIsProcessTrusted` reads it;
//!   `AXIsProcessTrustedWithOptions` with the prompt option opens macOS's dialog, which
//!   leads to System Settings.
//!
//! Both belong to the program that asks: Remote.app in-process (its bundle ID and
//! signature), or `sodium-remote-host` itself when it is run as a program.

use objc2_core_foundation::{CFBoolean, CFDictionary, CFString};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: Option<&CFDictionary>) -> u8;
    static kAXTrustedCheckOptionPrompt: &'static CFString;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    ScreenRecording,
    Accessibility,
}

/// Whether this process asked for Screen Recording without having it: the pane then
/// shows "Reopen to finish", since an Allow works only after a relaunch and macOS
/// doesn't say whether the person allowed it.
static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn screen_recording() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Asks once (macOS's own prompt); true if it is already allowed. An Allow given in
/// the prompt or in System Settings works after the next launch.
pub fn request_screen_recording() -> bool {
    let ok = CGRequestScreenCaptureAccess();
    if !ok {
        ASKED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    ok
}

/// Not allowed in this process, but asked for: allowed or not, only a relaunch tells.
pub fn screen_recording_asked() -> bool {
    ASKED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn accessibility() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

/// Opens macOS's Accessibility dialog when it isn't allowed; true if it is.
pub fn request_accessibility() -> bool {
    let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
    let opts = CFDictionary::<CFString, CFBoolean>::from_slices(&[key], &[CFBoolean::new(true)]);
    unsafe { AXIsProcessTrustedWithOptions(Some(opts.as_opaque())) != 0 }
}

pub fn missing() -> Vec<Permission> {
    let mut v = vec![];
    if !screen_recording() {
        v.push(Permission::ScreenRecording);
    }
    if !accessibility() {
        v.push(Permission::Accessibility);
    }
    v
}

/// One log line naming what is missing and where to allow it.
pub fn describe_missing(missing: &[Permission]) -> String {
    let who = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    let mut parts = vec![];
    if missing.contains(&Permission::ScreenRecording) {
        parts.push("Screen Recording isn't allowed, so nothing can be shown (System Settings › Privacy & Security › Screen & System Audio Recording; quit and start again after allowing)");
    }
    if missing.contains(&Permission::Accessibility) {
        parts.push("Accessibility isn't allowed, so keys and the pointer are ignored (System Settings › Privacy & Security › Accessibility)");
    }
    format!("macOS permissions for {who}: {}", parts.join("; "))
}
