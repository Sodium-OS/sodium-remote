// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Input through `SendInput`, as a real keyboard and mouse would give it:
//!
//! - **Keys** as set-1 scan codes (`hid`), so the host's own layout applies; a few by
//!   virtual-key code. Every key held is remembered for `ReleaseAll`.
//! - **Committed IME text** as `KEYEVENTF_UNICODE` (UTF-16 units, surrogate pairs as two).
//! - **The absolute pointer** over the whole virtual desktop
//!   (`MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`), from the display's pixels.
//! - **Relative motion** (pointer lock), fractions carried over.
//! - **Buttons** including Back and Forward (X buttons 1 and 2).
//! - **Wheels**: `value120` is already in `WHEEL_DELTA` units; Wayland's sign (positive
//!   is down) becomes Windows' (positive is up). Trackpad scrolling without `value120`
//!   is scaled (15 units a notch, as libinput's wheel) and accumulated.
//! - **Pinch** as Ctrl + wheel, which is how Windows apps zoom.
//!
//! Windows doesn't let a normal process drive an elevated window or a secure desktop
//! (UIPI); such input is dropped by Windows, and a note is logged now and then.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use remote_core::msg::{Axis, AxisSource, Button, GesturePhase};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, XBUTTON1, XBUTTON2,
};

use crate::backend::{Input, InputOut, Rect};
use crate::hid::{WinKey, hid_to_win};
use crate::policy::virtual_desk_abs;

/// Wheel units (1/120 of a notch) per unit of a trackpad's scroll value.
const FINGER_TO_WHEEL: f64 = 120.0 / 15.0;
/// Wheel units per unit of ln(scale) in a pinch: a notch for about +22%.
const PINCH_TO_WHEEL: f64 = 600.0;

pub struct Injector {
    keys: HashSet<u16>,
    buttons: Vec<Button>,
    /// Display id → its rectangle on the virtual desktop, refreshed every 2 s.
    displays: Vec<(u32, Rect)>,
    displays_at: Option<Instant>,
    rel: (f64, f64),
    wheel: (f64, f64),
    zoom: Option<(f64, f64)>,
    blocked_note: Option<Instant>,
}

impl Default for Injector {
    fn default() -> Self {
        Self::new()
    }
}

impl Injector {
    pub fn new() -> Injector {
        Injector {
            keys: HashSet::new(),
            buttons: Vec::new(),
            displays: vec![],
            displays_at: None,
            rel: (0.0, 0.0),
            wheel: (0.0, 0.0),
            zoom: None,
            blocked_note: None,
        }
    }

    fn send(&mut self, inputs: &[INPUT]) {
        if inputs.is_empty() {
            return;
        }
        let n = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
        if (n as usize) < inputs.len() && self.blocked_note.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
            self.blocked_note = Some(Instant::now());
            eprintln!(
                "sodium-remote-host: Windows dropped input ({}): an elevated window or a secure screen has the focus",
                windows::core::Error::from_thread()
            );
        }
    }

    fn rect_of(&mut self, display: u32) -> Option<Rect> {
        if self.displays_at.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            self.displays = super::display::outputs().into_iter().map(|o| (o.display.info.id, o.rect)).collect();
            self.displays_at = Some(Instant::now());
        }
        self.displays.iter().find(|(id, _)| *id == display).map(|(_, r)| *r)
    }

    fn key(&mut self, usage: u16, down: bool) {
        let Some(k) = hid_to_win(usage) else { return };
        if down {
            self.keys.insert(usage);
        } else {
            self.keys.remove(&usage);
        }
        let up = if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP };
        let ki = match k {
            WinKey::Scan { code, extended } => KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: code,
                dwFlags: KEYEVENTF_SCANCODE | up | if extended { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
            WinKey::Vk { vk, scan, extended } => KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: up | if extended { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        };
        self.send(&[kbd(ki)]);
    }

    fn text(&mut self, text: &str) {
        let mut v = Vec::with_capacity(text.len() * 2);
        for unit in text.replace("\r\n", "\r").replace('\n', "\r").encode_utf16() {
            for up in [KEYBD_EVENT_FLAGS(0), KEYEVENTF_KEYUP] {
                v.push(kbd(KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: unit, dwFlags: KEYEVENTF_UNICODE | up, time: 0, dwExtraInfo: 0 }));
            }
        }
        self.send(&v);
    }

    fn pointer_abs(&mut self, display: u32, x: f64, y: f64) {
        let Some(r) = self.rect_of(display) else { return };
        let (left, top, w, h) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        let (dx, dy) = virtual_desk_abs(r.x + x.floor() as i32, r.y + y.floor() as i32, left, top, w, h);
        self.send(&[mouse(dx, dy, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)]);
    }

    fn pointer_rel(&mut self, dx: f64, dy: f64) {
        self.rel.0 += dx;
        self.rel.1 += dy;
        let (ix, iy) = (self.rel.0.trunc(), self.rel.1.trunc());
        self.rel.0 -= ix;
        self.rel.1 -= iy;
        if ix != 0.0 || iy != 0.0 {
            self.send(&[mouse(ix as i32, iy as i32, 0, MOUSEEVENTF_MOVE)]);
        }
    }

    fn button(&mut self, b: Button, down: bool) {
        let (flags, data) = match (b, down) {
            (Button::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (Button::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (Button::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (Button::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (Button::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (Button::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (Button::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1 as u32),
            (Button::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1 as u32),
            (Button::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2 as u32),
            (Button::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2 as u32),
            (Button::Other(_), _) => return,
        };
        if down {
            if !self.buttons.contains(&b) {
                self.buttons.push(b);
            }
        } else {
            self.buttons.retain(|x| *x != b);
        }
        self.send(&[mouse(0, 0, data, flags)]);
    }

    fn axis(&mut self, source: AxisSource, axis: Axis, value: f64, value120: i32) {
        let units = if value120 != 0 && matches!(source, AxisSource::Wheel | AxisSource::WheelTilt) {
            value120 as f64
        } else {
            value * FINGER_TO_WHEEL
        };
        let (acc, flags, sign) = match axis {
            Axis::Vertical => (&mut self.wheel.1, MOUSEEVENTF_WHEEL, -1.0),
            Axis::Horizontal => (&mut self.wheel.0, MOUSEEVENTF_HWHEEL, 1.0),
        };
        *acc += units * sign;
        let whole = acc.trunc();
        *acc -= whole;
        if whole != 0.0 {
            self.send(&[mouse(0, 0, whole as i32 as u32, flags)]);
        }
    }

    fn zoom(&mut self, phase: GesturePhase, scale: f64) {
        const CTRL: u16 = 0xE0;
        match phase {
            GesturePhase::Begin => {
                if self.zoom.is_none() {
                    self.key(CTRL, true);
                }
                self.zoom = Some((1.0, 0.0));
            }
            GesturePhase::Update => {
                let Some((last, acc)) = self.zoom else { return };
                let scale = if scale > 0.0 { scale } else { last };
                let mut acc = acc + (scale / last).ln() * PINCH_TO_WHEEL;
                let whole = acc.trunc();
                acc -= whole;
                self.zoom = Some((scale, acc));
                if whole != 0.0 {
                    self.send(&[mouse(0, 0, whole as i32 as u32, MOUSEEVENTF_WHEEL)]);
                }
            }
            GesturePhase::End | GesturePhase::Cancel => {
                if self.zoom.take().is_some() {
                    self.key(CTRL, false);
                }
            }
        }
    }

    fn release_all(&mut self) {
        for u in self.keys.clone() {
            self.key(u, false);
        }
        for b in self.buttons.clone() {
            self.button(b, false);
        }
        self.zoom = None;
        self.rel = (0.0, 0.0);
        self.wheel = (0.0, 0.0);
    }
}

fn kbd(ki: KEYBDINPUT) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } }
}

fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

impl Input for Injector {
    fn inject(&mut self, ev: InputOut) {
        match ev {
            InputOut::Key { usage, down } => self.key(usage, down),
            InputOut::Text(t) => self.text(&t),
            InputOut::PointerAbs { display, x, y } => self.pointer_abs(display, x, y),
            InputOut::PointerRel { dx, dy } => self.pointer_rel(dx, dy),
            InputOut::Button { button, down } => self.button(button, down),
            InputOut::Axis { source, axis, value, value120, .. } => self.axis(source, axis, value, value120),
            InputOut::Zoom { phase, scale } => self.zoom(phase, scale),
            InputOut::ReleaseAll => self.release_all(),
        }
    }
}
