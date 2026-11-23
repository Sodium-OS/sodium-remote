// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! What the Qt app shares with the video surface when it runs as a subsurface under Qt's toplevel
//! Qt's `wl_display` and `wl_surface`, the window's geometry, the HUD's rects
//! (where the pointer belongs to the app, not the host), and keyboard capture.

use std::ffi::c_void;
use std::sync::Mutex;

/// Raw Wayland pointers from Qt. Qt owns them; they outlive the session.
pub struct Embed {
    pub display: *mut c_void,
    pub parent: *mut c_void,
    pub state: Mutex<EmbedState>,
    /// Called from the video thread.
    pub events: Box<dyn Fn(EmbedEvent) + Send + Sync>,
}

unsafe impl Send for Embed {}
unsafe impl Sync for Embed {}

#[derive(Clone, Debug)]
pub struct EmbedState {
    /// Bumped on every change, so the video thread looks only when something changed.
    pub rev: u64,
    /// The window in logical pixels, and its buffer scale.
    pub size: (i32, i32),
    /// Where the video area starts in the window (below the app's title bar), logical pixels.
    pub origin: (i32, i32),
    pub scale: i32,
    pub fullscreen: bool,
    /// x, y, w, h in logical pixels of the window: bars, menus and toasts the app draws over the video.
    pub hud_rects: Vec<[i32; 4]>,
    /// The keyboard goes to the host (false after the leave chord, until a click in the video).
    pub capture: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbedEvent {
    /// Capture changed (the leave chord or the click that resumes it).
    Capture(bool),
    /// The local leave chord was pressed: leave full screen, show the bar.
    LeaveChord,
    /// The video thread ended.
    Closed,
}

impl Embed {
    pub fn new(display: *mut c_void, parent: *mut c_void, state: EmbedState, events: Box<dyn Fn(EmbedEvent) + Send + Sync>) -> Embed {
        Embed { display, parent, state: Mutex::new(state), events }
    }

    pub fn update(&self, f: impl FnOnce(&mut EmbedState)) {
        let mut s = self.state.lock().unwrap();
        f(&mut s);
        s.rev += 1;
    }
}
