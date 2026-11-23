// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The frontmost app (the key table's `Terminal` and `Class` conditions; cross-platform
//! md 7: the frontmost bundle ID, with a built-in terminal list) and the computer's
//! name (the Sharing name, as the Mac client uses).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use objc2_app_kit::NSWorkspace;
use objc2_core_foundation::{CFRetained, CFString};

use super::main_thread::on_main;
use crate::backend::Foreground;

/// Bundle IDs of terminals: Ctrl stays Control there, and rules marked `Terminal` apply
/// (a PC's Ctrl+Shift+C is Cmd+C).
pub const TERMINALS: &[&str] = &[
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "com.mitchellh.ghostty",
    "net.kovidgoyal.kitty",
    "org.alacritty",
    "io.alacritty",
    "com.github.wez.wezterm",
    "dev.warp.Warp-Stable",
    "dev.warp.Warp-Preview",
    "co.zeit.hyper",
    "org.tabby",
    "com.raphaelamorim.rio",
    "com.termius-dmg.mac",
    "org.contourterminal.Contour",
];

#[link(name = "SystemConfiguration", kind = "framework")]
unsafe extern "C" {
    /// Returns a +1 CFStringRef (the Create rule) or NULL.
    fn SCDynamicStoreCopyComputerName(store: *const std::ffi::c_void, encoding: *mut u32) -> *const CFString;
}

/// The computer's name from System Settings › General › Sharing ("Nao's MacBook Air").
pub fn computer_name() -> String {
    let p = unsafe { SCDynamicStoreCopyComputerName(std::ptr::null(), std::ptr::null_mut()) };
    if let Some(p) = std::ptr::NonNull::new(p as *mut CFString) {
        let s: CFRetained<CFString> = unsafe { CFRetained::from_raw(p) };
        let s = s.to_string();
        if !s.is_empty() {
            return s;
        }
    }
    "Mac".into()
}

/// The frontmost app's bundle ID, cached for 200 ms (every key goes through the table,
/// and the app rarely changes between two keys). Read on the main thread, where
/// NSWorkspace keeps it current; if the main thread is busy, the last answer stands.
pub fn foreground() -> Foreground {
    static CACHE: Mutex<Option<(Instant, Foreground)>> = Mutex::new(None);
    if let Some((t, f)) = CACHE.lock().unwrap().as_ref()
        && t.elapsed() < Duration::from_millis(200)
    {
        return f.clone();
    }
    let id = on_main(Duration::from_millis(50), |_| {
        NSWorkspace::sharedWorkspace().frontmostApplication().and_then(|a| a.bundleIdentifier()).map(|s| s.to_string()).unwrap_or_default()
    });
    let mut cache = CACHE.lock().unwrap();
    let Some(id) = id else {
        return cache.as_ref().map(|(_, f)| f.clone()).unwrap_or_default();
    };
    let terminal = TERMINALS.iter().any(|t| t.eq_ignore_ascii_case(&id));
    let f = Foreground { id, terminal };
    *cache = Some((Instant::now(), f.clone()));
    f
}
