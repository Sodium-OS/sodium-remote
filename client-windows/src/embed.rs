// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The stream window as a child of the Qt app's own window.
//!
//! Qt owns the top-level frame; the engine's swapchain sits on a child HWND of it, at the video area's
//! origin. The bar, menus and cards are another, owned top-level Qt window above it whose region covers only
//! what the app draws, so the pointer reaches this child everywhere else.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmbedState {
    /// The video area in the parent's client, logical pixels.
    pub origin: (i32, i32),
    pub size: (i32, i32),
    pub fullscreen: bool,
    /// The keyboard goes to the computer (the leave chord turns it off, a click in the video back on).
    pub capture: bool,
    /// A menu of the app is open over the picture: no pointer or key input reaches the computer (key-ups and
    /// button-ups still do, so nothing stays held there), and a click in the video is reported instead.
    pub blocked: bool,
}

#[derive(Clone, Debug)]
pub enum EmbedEvent {
    Capture(bool),
    LeaveChord,
    /// A press in the video while `blocked` (the app closes its menu).
    VideoClick,
    /// A key pressed while `blocked` (virtual-key code): the app's menu has no keyboard focus of its own.
    MenuKey(u32),
}

pub struct Embed {
    /// The parent's HWND (Qt's stream frame).
    pub parent: isize,
    state: Mutex<EmbedState>,
    rev: AtomicU64,
    pub on_event: Box<dyn Fn(EmbedEvent) + Send + Sync>,
}

impl Embed {
    pub fn new(parent: isize, state: EmbedState, on_event: Box<dyn Fn(EmbedEvent) + Send + Sync>) -> Embed {
        Embed { parent, state: Mutex::new(state), rev: AtomicU64::new(1), on_event }
    }
    pub fn get(&self) -> EmbedState {
        self.state.lock().unwrap().clone()
    }
    pub fn rev(&self) -> u64 {
        self.rev.load(Ordering::Acquire)
    }
    pub fn update(&self, f: impl FnOnce(&mut EmbedState)) {
        let mut s = self.state.lock().unwrap();
        let before = s.clone();
        f(&mut s);
        if *s != before {
            self.rev.fetch_add(1, Ordering::Release);
        }
    }
}
