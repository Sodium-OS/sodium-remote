// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Input through `CGEventPost` at the HID level, as a keyboard and mouse would give it
//! (needs Accessibility; without it macOS drops the events, and the log says so once).
//!
//! - **Keys** as macOS key codes (`hid::hid_to_mac`), so the Mac's own layout gives the
//!   character. Modifier keys are `FlagsChanged` events; every event carries the flags
//!   of the modifiers held (left/right device bits too), plus what the hardware adds:
//!   NumericPad and Fn on the arrows, Fn on the navigation and function keys. Caps Lock
//!   toggles its flag. Every key held is remembered for `ReleaseAll`.
//! - **Committed IME text** as Unicode strings on key events (20 UTF-16 units each, the
//!   API's limit), with no modifier flags; a newline is a Return key.
//! - **The absolute pointer** in global points (`policy::mac_point`) from the display's
//!   pixels; a move with a button held is that button's drag. Relative motion (pointer
//!   lock) moves from the current location and carries the deltas.
//! - **Buttons**: left, right, middle (other 2), back (3), forward (4) and any other
//!   evdev button, with click counts from the system's double-click interval, as
//!   AppKit needs them for double and triple clicks.
//! - **Scroll**: wheels in lines (`value120` / 120, carried over); trackpad scrolling in
//!   pixels, continuous, with the scroll phase (began, changed, and ended at `stop`) so
//!   apps treat it as a trackpad's; the momentum comes from the client's own stream
//!   (the Mac client sends macOS's momentum as more finger scrolling). Both axes of one
//!   pointer frame go in one event. Wayland's sign (positive is down/right) becomes
//!   CoreGraphics' (positive is up/left).
//! - **Pinch** as a posted magnify gesture event (`NSEventTypeGesture` with the
//!   undocumented subtype, magnification and phase fields; no private symbols), with
//!   Cmd + scroll wheel (one line per 22% of scale) as the fallback behind
//!   `SODIUM_REMOTE_PINCH=cmd-scroll`. The active one is logged at the first pinch.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use objc2_app_kit::NSEvent;
use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation, CGEventType, CGMouseButton,
    CGPreflightPostEventAccess, CGScrollEventUnit, CGScrollPhase,
};
use remote_core::msg::{Axis, AxisSource, Button, GesturePhase};

use super::display::{MacDisplay, outputs};
use crate::backend::{Input, InputOut};
use crate::hid::hid_to_mac;
use crate::policy::mac_point;

/// How a pinch is injected. `SODIUM_REMOTE_PINCH=cmd-scroll` switches to the fallback
/// without a rebuild (anything else, or nothing, is the magnify gesture); the host logs
/// the active one at the first pinch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PinchMode {
    /// A posted magnify gesture event (undocumented CGEvent fields).
    Magnify,
    /// Cmd + scroll wheel.
    CmdScroll,
}

impl PinchMode {
    fn from_env() -> PinchMode {
        match std::env::var("SODIUM_REMOTE_PINCH").as_deref() {
            Ok("cmd-scroll") => PinchMode::CmdScroll,
            _ => PinchMode::Magnify,
        }
    }
    fn name(self) -> &'static str {
        match self {
            PinchMode::Magnify => "magnify gesture events (SODIUM_REMOTE_PINCH=cmd-scroll falls back to Cmd + scroll)",
            PinchMode::CmdScroll => "Cmd + scroll (SODIUM_REMOTE_PINCH=cmd-scroll)",
        }
    }
}

// The undocumented CGEvent gesture fields, as public open-source projects use them
// (NSEventTypeGesture events built from CGEventCreate). Unverified until the checklist's
// pinch step is run on the Mac.
/// `NSEventTypeGesture` as a CGEvent type.
const GESTURE_EVENT_TYPE: u32 = 29;
/// The gesture subtype field, and its magnify value.
const FIELD_GESTURE_SUBTYPE: u32 = 110;
const GESTURE_MAGNIFY: i64 = 8;
/// The magnification increment (double), and the gesture phase (`NSEventPhase` bits).
const FIELD_MAGNIFICATION: u32 = 113;
const FIELD_GESTURE_PHASE: u32 = 132;
const PHASE_BEGAN: i64 = 1;
const PHASE_CHANGED: i64 = 2;
const PHASE_ENDED: i64 = 4;

/// Wheel lines per unit of ln(scale) in a pinch: a line for about +22%.
const PINCH_TO_LINES: f64 = 5.0;
/// NX_DEVICE* bits: which side's modifier is down (IOLLEvent.h).
const DEV_LCTL: u64 = 0x0001;
const DEV_LSHIFT: u64 = 0x0002;
const DEV_RSHIFT: u64 = 0x0004;
const DEV_LCMD: u64 = 0x0008;
const DEV_RCMD: u64 = 0x0010;
const DEV_LALT: u64 = 0x0020;
const DEV_RALT: u64 = 0x0040;
const DEV_RCTL: u64 = 0x2000;

/// A modifier usage's flags: (device-independent mask, device bit).
fn modifier(usage: u16) -> Option<(CGEventFlags, u64)> {
    Some(match usage {
        0xE0 => (CGEventFlags::MaskControl, DEV_LCTL),
        0xE1 => (CGEventFlags::MaskShift, DEV_LSHIFT),
        0xE2 => (CGEventFlags::MaskAlternate, DEV_LALT),
        0xE3 => (CGEventFlags::MaskCommand, DEV_LCMD),
        0xE4 => (CGEventFlags::MaskControl, DEV_RCTL),
        0xE5 => (CGEventFlags::MaskShift, DEV_RSHIFT),
        0xE6 => (CGEventFlags::MaskAlternate, DEV_RALT),
        0xE7 => (CGEventFlags::MaskCommand, DEV_RCMD),
        remote_core::msg::USAGE_GLOBE => (CGEventFlags::MaskSecondaryFn, 0),
        _ => return None,
    })
}

/// The flags the hardware sets on a key by itself.
fn key_flags(code: u16) -> CGEventFlags {
    match code {
        // Arrows.
        0x7B..=0x7E => CGEventFlags::MaskNumericPad | CGEventFlags::MaskSecondaryFn,
        // Help/Insert, Home, Page Up, Forward Delete, End, Page Down, and the F keys.
        0x72..=0x77 | 0x79 | 0x7A | 0x78 | 0x60..=0x6F | 0x71 | 0x40 | 0x4F | 0x50 | 0x5A => CGEventFlags::MaskSecondaryFn,
        // The keypad.
        0x41 | 0x43 | 0x45 | 0x47 | 0x4B | 0x4C | 0x4E | 0x51..=0x59 | 0x5B | 0x5C => CGEventFlags::MaskNumericPad,
        _ => CGEventFlags::empty(),
    }
}

struct Source(CFRetained<CGEventSource>);
// The event source is a CF object used by one thread at a time (the session's input
// lock); CoreGraphics' event calls are thread-safe.
unsafe impl Send for Source {}

pub struct Injector {
    source: Option<Source>,
    keys: HashSet<u16>,
    mods: HashSet<u16>,
    caps: bool,
    buttons: Vec<Button>,
    /// Display id → geometry, refreshed every 2 s.
    displays: Vec<MacDisplay>,
    displays_at: Option<Instant>,
    /// (button, when, where, count) of the last press, for click counts.
    last_click: Option<(Button, Instant, CGPoint, i64)>,
    double_click: Duration,
    finger: bool,
    /// One pointer frame's vertical and horizontal scroll waiting for the other axis.
    pending: Option<(AxisSource, f64, f64, i32, i32)>,
    lines: (f64, f64),
    pixels: (f64, f64),
    zoom: Option<(f64, f64)>,
    pinch: PinchMode,
    pinch_logged: bool,
    warned: bool,
}

impl Default for Injector {
    fn default() -> Self {
        Self::new()
    }
}

impl Injector {
    pub fn new() -> Injector {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).map(Source);
        let double_click = Duration::from_secs_f64(NSEvent::doubleClickInterval().clamp(0.1, 5.0));
        Injector {
            source,
            keys: HashSet::new(),
            mods: HashSet::new(),
            caps: false,
            buttons: vec![],
            displays: vec![],
            displays_at: None,
            last_click: None,
            double_click,
            finger: false,
            pending: None,
            lines: (0.0, 0.0),
            pixels: (0.0, 0.0),
            zoom: None,
            pinch: PinchMode::from_env(),
            pinch_logged: false,
            warned: false,
        }
    }

    fn src(&self) -> Option<&CGEventSource> {
        self.source.as_ref().map(|s| &*s.0)
    }

    fn warn_without_access(&mut self) {
        if !self.warned && !CGPreflightPostEventAccess() {
            self.warned = true;
            eprintln!("sodium-remote-host: {}", super::permissions::describe_missing(&[super::permissions::Permission::Accessibility]));
        }
    }

    fn post(&mut self, ev: Option<CFRetained<CGEvent>>) {
        self.warn_without_access();
        if let Some(ev) = ev {
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
        }
    }

    /// The flags of the modifiers held now.
    fn flags(&self) -> CGEventFlags {
        let mut f = CGEventFlags::empty();
        let mut dev = 0u64;
        for u in &self.mods {
            if let Some((mask, bit)) = modifier(*u) {
                f |= mask;
                dev |= bit;
            }
        }
        if self.caps {
            f |= CGEventFlags::MaskAlphaShift;
        }
        f | CGEventFlags::from_bits_retain(dev)
    }

    fn display(&mut self, id: u32) -> Option<MacDisplay> {
        if self.displays_at.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
            self.displays = outputs();
            self.displays_at = Some(Instant::now());
        }
        self.displays.iter().find(|d| d.display.info.id == id).cloned()
    }

    fn key(&mut self, usage: u16, down: bool) {
        let Some(code) = hid_to_mac(usage) else { return };
        if modifier(usage).is_some() {
            if down {
                self.mods.insert(usage);
            } else {
                self.mods.remove(&usage);
            }
            let ev = CGEvent::new_keyboard_event(self.src(), code, down);
            CGEvent::set_type(ev.as_deref(), CGEventType::FlagsChanged);
            CGEvent::set_flags(ev.as_deref(), self.flags());
            self.post(ev);
            return;
        }
        if usage == 0x39 {
            // Caps Lock: a press toggles the lock, as the keyboard's own key does.
            if down {
                self.caps = !self.caps;
                let ev = CGEvent::new_keyboard_event(self.src(), code, true);
                CGEvent::set_type(ev.as_deref(), CGEventType::FlagsChanged);
                CGEvent::set_flags(ev.as_deref(), self.flags());
                self.post(ev);
            }
            return;
        }
        if down {
            self.keys.insert(usage);
        } else {
            self.keys.remove(&usage);
        }
        let ev = CGEvent::new_keyboard_event(self.src(), code, down);
        CGEvent::set_flags(ev.as_deref(), self.flags() | key_flags(code));
        self.post(ev);
    }

    fn text(&mut self, text: &str) {
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                // kVK_Return.
                for down in [true, false] {
                    let ev = CGEvent::new_keyboard_event(self.src(), 0x24, down);
                    CGEvent::set_flags(ev.as_deref(), CGEventFlags::empty());
                    self.post(ev);
                }
            }
            let units: Vec<u16> = line.trim_end_matches('\r').encode_utf16().collect();
            for chunk in units.chunks(20) {
                for down in [true, false] {
                    let ev = CGEvent::new_keyboard_event(self.src(), 0, down);
                    CGEvent::set_flags(ev.as_deref(), CGEventFlags::empty());
                    unsafe { CGEvent::keyboard_set_unicode_string(ev.as_deref(), chunk.len() as _, chunk.as_ptr()) };
                    self.post(ev);
                }
            }
        }
    }

    /// The button held whose drag a move is, with its event type.
    fn move_type(&self) -> (CGEventType, CGMouseButton) {
        match self.buttons.first() {
            Some(Button::Left) => (CGEventType::LeftMouseDragged, CGMouseButton::Left),
            Some(Button::Right) => (CGEventType::RightMouseDragged, CGMouseButton::Right),
            Some(b) => (CGEventType::OtherMouseDragged, CGMouseButton(button_number(*b))),
            None => (CGEventType::MouseMoved, CGMouseButton::Left),
        }
    }

    fn move_to(&mut self, p: CGPoint, delta: (f64, f64)) {
        let (kind, button) = self.move_type();
        let ev = CGEvent::new_mouse_event(self.src(), kind, p, button);
        CGEvent::set_integer_value_field(ev.as_deref(), CGEventField::MouseEventDeltaX, delta.0.round() as i64);
        CGEvent::set_integer_value_field(ev.as_deref(), CGEventField::MouseEventDeltaY, delta.1.round() as i64);
        CGEvent::set_flags(ev.as_deref(), self.flags());
        self.post(ev);
    }

    fn pointer_abs(&mut self, display: u32, x: f64, y: f64) {
        let Some(d) = self.display(display) else { return };
        let (px, py) = mac_point(x, y, d.bounds, (d.display.info.width, d.display.info.height));
        let cur = super::cursor::location();
        self.move_to(CGPoint::new(px, py), (px - cur.x, py - cur.y));
    }

    fn pointer_rel(&mut self, dx: f64, dy: f64) {
        let cur = super::cursor::location();
        self.display(0); // refreshes the geometry
        // Pixels to points on the display the pointer is on.
        let scale = self
            .displays
            .iter()
            .find(|d| {
                let (bx, by, bw, bh) = d.bounds;
                cur.x >= bx && cur.y >= by && cur.x < bx + bw && cur.y < by + bh
            })
            .map_or(1.0, |d| d.display.info.scale as f64)
            .max(1.0);
        let (dx, dy) = (dx / scale, dy / scale);
        self.move_to(CGPoint::new(cur.x + dx, cur.y + dy), (dx, dy));
    }

    fn button(&mut self, b: Button, down: bool) {
        let Some(n) = Some(button_number(b)).filter(|n| *n < 32) else { return };
        let (kind, button) = match (b, down) {
            (Button::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
            (Button::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
            (Button::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right),
            (Button::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right),
            (_, true) => (CGEventType::OtherMouseDown, CGMouseButton(n)),
            (_, false) => (CGEventType::OtherMouseUp, CGMouseButton(n)),
        };
        let p = super::cursor::location();
        let count = if down {
            let c = match self.last_click {
                Some((lb, t, lp, c)) if lb == b && t.elapsed() <= self.double_click && (lp.x - p.x).hypot(lp.y - p.y) <= 4.0 => c + 1,
                _ => 1,
            };
            self.last_click = Some((b, Instant::now(), p, c));
            if !self.buttons.contains(&b) {
                self.buttons.push(b);
            }
            c
        } else {
            self.buttons.retain(|x| *x != b);
            self.last_click.filter(|(lb, ..)| *lb == b).map_or(1, |(.., c)| c)
        };
        let ev = CGEvent::new_mouse_event(self.src(), kind, p, button);
        CGEvent::set_integer_value_field(ev.as_deref(), CGEventField::MouseEventClickState, count);
        CGEvent::set_integer_value_field(ev.as_deref(), CGEventField::MouseEventButtonNumber, n as i64);
        CGEvent::set_flags(ev.as_deref(), self.flags());
        self.post(ev);
    }

    /// One scroll event: `v` and `h` in Wayland's sign; lines for wheels, pixels else.
    fn scroll(&mut self, source: AxisSource, v: f64, h: f64, v120: i32, h120: i32, phase: Option<CGScrollPhase>) {
        let wheel = matches!(source, AxisSource::Wheel | AxisSource::WheelTilt);
        let (unit, dv, dh) = if wheel {
            let (lv, lh) = if v120 != 0 || h120 != 0 { (v120 as f64 / 120.0, h120 as f64 / 120.0) } else { (v / 15.0, h / 15.0) };
            self.lines.0 += lv;
            self.lines.1 += lh;
            let (wv, wh) = (self.lines.0.trunc(), self.lines.1.trunc());
            self.lines.0 -= wv;
            self.lines.1 -= wh;
            (CGScrollEventUnit::Line, wv, wh)
        } else {
            self.pixels.0 += v;
            self.pixels.1 += h;
            let (wv, wh) = (self.pixels.0.round(), self.pixels.1.round());
            self.pixels.0 -= wv;
            self.pixels.1 -= wh;
            (CGScrollEventUnit::Pixel, wv, wh)
        };
        if dv == 0.0 && dh == 0.0 && phase.is_none() {
            return;
        }
        // CoreGraphics: wheel 1 positive scrolls up, wheel 2 positive scrolls left.
        let ev = CGEvent::new_scroll_wheel_event2(self.src(), unit, 2, -dv as i32, -dh as i32, 0);
        if !wheel {
            CGEvent::set_integer_value_field(ev.as_deref(), CGEventField::ScrollWheelEventIsContinuous, 1);
        }
        if let Some(ph) = phase {
            CGEvent::set_integer_value_field(ev.as_deref(), CGEventField::ScrollWheelEventScrollPhase, ph.0 as i64);
        }
        CGEvent::set_flags(ev.as_deref(), self.flags());
        self.post(ev);
    }

    fn axis(&mut self, source: AxisSource, axis: Axis, value: f64, value120: i32, stop: bool, more: bool) {
        if source == AxisSource::Finger && stop {
            // The end of a finger scroll: one event with the ended phase.
            if more {
                return; // the other axis's stop follows
            }
            self.pending = None;
            if self.finger {
                self.finger = false;
                self.scroll(source, 0.0, 0.0, 0, 0, Some(CGScrollPhase::Ended));
            }
            self.pixels = (0.0, 0.0);
            return;
        }
        let (mut v, mut h, mut v120, mut h120) = (0.0, 0.0, 0, 0);
        if let Some((_, pv, ph, pv120, ph120)) = self.pending.take() {
            (v, h, v120, h120) = (pv, ph, pv120, ph120);
        }
        match axis {
            Axis::Vertical => {
                v += value;
                v120 += value120;
            }
            Axis::Horizontal => {
                h += value;
                h120 += value120;
            }
        }
        if more {
            self.pending = Some((source, v, h, v120, h120));
            return;
        }
        let phase = (source == AxisSource::Finger).then(|| {
            if self.finger {
                CGScrollPhase::Changed
            } else {
                self.finger = true;
                CGScrollPhase::Began
            }
        });
        self.scroll(source, v, h, v120, h120, phase);
    }

    /// One magnify gesture event: an `NSEventTypeGesture` CGEvent (type 29) with the
    /// gesture subtype, the incremental magnification and the phase in the undocumented
    /// fields that public open-source projects use (no private symbols). `delta` is the
    /// change since the last event (what `NSEvent.magnification` carries): 0.1 is +10%.
    /// It is posted at the pointer, where apps receive it as a trackpad pinch.
    fn magnify(&mut self, phase: i64, delta: f64) {
        let Some(ev) = CGEvent::new(self.src()) else { return };
        // Where the pointer is: gestures go to the window under it.
        let at = CGEvent::location(Some(&ev));
        CGEvent::set_type(Some(&ev), CGEventType(GESTURE_EVENT_TYPE));
        CGEvent::set_location(Some(&ev), at);
        CGEvent::set_integer_value_field(Some(&ev), CGEventField(FIELD_GESTURE_SUBTYPE), GESTURE_MAGNIFY);
        CGEvent::set_double_value_field(Some(&ev), CGEventField(FIELD_MAGNIFICATION), delta);
        CGEvent::set_integer_value_field(Some(&ev), CGEventField(FIELD_GESTURE_PHASE), phase);
        self.warn_without_access();
        CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&ev));
    }

    /// A pinch as magnify gesture events (the default; `pinch_mode`).
    fn zoom_magnify(&mut self, phase: GesturePhase, scale: f64) {
        match phase {
            GesturePhase::Begin => {
                self.zoom = Some((1.0, 0.0));
                self.magnify(PHASE_BEGAN, 0.0);
            }
            GesturePhase::Update => {
                let Some((last, _)) = self.zoom else { return };
                let scale = if scale > 0.0 { scale } else { last };
                self.zoom = Some((scale, 0.0));
                self.magnify(PHASE_CHANGED, scale / last - 1.0);
            }
            GesturePhase::End | GesturePhase::Cancel => {
                if self.zoom.take().is_some() {
                    self.magnify(PHASE_ENDED, 0.0);
                }
            }
        }
    }

    fn zoom(&mut self, phase: GesturePhase, scale: f64) {
        if matches!(phase, GesturePhase::Begin) && !self.pinch_logged {
            self.pinch_logged = true;
            eprintln!("sodium-remote-host: pinch as {}", self.pinch.name());
        }
        match self.pinch {
            PinchMode::Magnify => self.zoom_magnify(phase, scale),
            PinchMode::CmdScroll => self.zoom_cmd_scroll(phase, scale),
        }
    }

    /// The fallback: Cmd + scroll wheel, one line per 22% of scale. Safari and Preview aren't
    /// expected to zoom with it, but many apps do.
    fn zoom_cmd_scroll(&mut self, phase: GesturePhase, scale: f64) {
        const CMD: u16 = 0xE3;
        match phase {
            GesturePhase::Begin => {
                if self.zoom.is_none() {
                    self.key(CMD, true);
                }
                self.zoom = Some((1.0, 0.0));
            }
            GesturePhase::Update => {
                let Some((last, acc)) = self.zoom else { return };
                let scale = if scale > 0.0 { scale } else { last };
                let mut acc = acc + (scale / last).ln() * PINCH_TO_LINES;
                let whole = acc.trunc();
                acc -= whole;
                self.zoom = Some((scale, acc));
                if whole != 0.0 {
                    // Zoom in (scale up) as scroll up.
                    let ev = CGEvent::new_scroll_wheel_event2(self.src(), CGScrollEventUnit::Line, 1, whole as i32, 0, 0);
                    CGEvent::set_flags(ev.as_deref(), self.flags());
                    self.post(ev);
                }
            }
            GesturePhase::End | GesturePhase::Cancel => {
                if self.zoom.take().is_some() {
                    self.key(CMD, false);
                }
            }
        }
    }

    fn release_all(&mut self) {
        for u in self.keys.clone() {
            self.key(u, false);
        }
        for u in self.mods.clone() {
            self.key(u, false);
        }
        for b in self.buttons.clone() {
            self.button(b, false);
        }
        if self.finger {
            self.finger = false;
            self.scroll(AxisSource::Finger, 0.0, 0.0, 0, 0, Some(CGScrollPhase::Ended));
        }
        if self.zoom.take().is_some() && self.pinch == PinchMode::Magnify {
            self.magnify(PHASE_ENDED, 0.0);
        }
        self.pending = None;
        self.lines = (0.0, 0.0);
        self.pixels = (0.0, 0.0);
    }
}

/// CoreGraphics' button number: left 0, right 1, middle 2, back 3, forward 4; other
/// evdev buttons (BTN_LEFT is 0x110) by their offset.
fn button_number(b: Button) -> u32 {
    match b {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
        Button::Back => 3,
        Button::Forward => 4,
        Button::Other(code) => code.saturating_sub(0x110),
    }
}

impl Input for Injector {
    fn inject(&mut self, ev: InputOut) {
        objc2::rc::autoreleasepool(|_| match ev {
            InputOut::Key { usage, down } => self.key(usage, down),
            InputOut::Text(t) => self.text(&t),
            InputOut::PointerAbs { display, x, y } => self.pointer_abs(display, x, y),
            InputOut::PointerRel { dx, dy } => self.pointer_rel(dx, dy),
            InputOut::Button { button, down } => self.button(button, down),
            InputOut::Axis { source, axis, value, value120, stop, more } => self.axis(source, axis, value, value120, stop, more),
            InputOut::Zoom { phase, scale } => self.zoom(phase, scale),
            InputOut::ReleaseAll => self.release_all(),
        })
    }
}
