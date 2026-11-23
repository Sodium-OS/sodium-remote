// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Input: libei through `reis` on the portal's
//! ConnectToEIS socket, else the portal's Notify methods.
//!
//! - Keys: HID usages → evdev codes (`hid::hid_to_evdev`); the compositor's own layout
//!   applies, as with a real keyboard.
//! - IME commits: `ei_text.utf8` where the EIS server offers it; else each character's
//!   key on the EIS keymap (xkbcommon: level 1 with Shift, 2 with AltGr, 3 with both);
//!   with Notify, NotifyKeyboardKeysym. Characters on no key are dropped.
//! - Pointer: absolute in the region whose mapping ID is the stream's (logical
//!   coordinates), relative, buttons as evdev BTN_*, wheels as 120ths, finger scrolls in
//!   pixels with a stop. No pinch (libei has none).

use std::collections::HashSet;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use reis::ei;
use reis::event::{Device, DeviceCapability, EiEvent, EiEventConverter};
use remote_core::msg::{Axis, AxisSource, Button};
use xkbcommon::xkb;

use super::names;
use super::portal::{Active, Notify, Portal};
use crate::backend::{Input, InputOut};

const KEY_LEFTSHIFT: u32 = 42;
const KEY_RIGHTALT: u32 = 100;
const KEY_ENTER: u32 = 28;
const KEY_TAB: u32 = 15;

fn button_code(b: Button) -> u32 {
    match b {
        Button::Left => 0x110,
        Button::Right => 0x111,
        Button::Middle => 0x112,
        Button::Back => 0x113,
        Button::Forward => 0x114,
        Button::Other(c) => c,
    }
}

fn now_us() -> u64 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    t.tv_sec as u64 * 1_000_000 + t.tv_nsec as u64 / 1000
}

/// The EIS keymap, for typing characters as keys.
struct Keymap(xkb::Keymap);
// libxkbcommon keymaps are immutable and reference counted: safe to read from any thread.
unsafe impl Send for Keymap {}

impl Keymap {
    /// The evdev key for `c` and the modifiers its level needs.
    fn find(&self, c: char) -> Option<(u32, &'static [u32])> {
        match c {
            '\n' | '\r' => return Some((KEY_ENTER, &[])),
            '\t' => return Some((KEY_TAB, &[])),
            _ => {}
        }
        let sym = xkb::utf32_to_keysym(c as u32);
        let k = &self.0;
        for code in k.min_keycode().raw()..=k.max_keycode().raw() {
            let kc = xkb::Keycode::new(code);
            let levels = k.num_levels_for_key(kc, 0).min(4);
            for level in 0..levels {
                if k.key_get_syms_by_level(kc, 0, level).contains(&sym) {
                    let mods: &'static [u32] = match level {
                        0 => &[],
                        1 => &[KEY_LEFTSHIFT],
                        2 => &[KEY_RIGHTALT],
                        _ => &[KEY_LEFTSHIFT, KEY_RIGHTALT],
                    };
                    return Some((code.saturating_sub(8), mods));
                }
            }
        }
        None
    }
}

#[derive(Default)]
struct EiState {
    /// Devices and whether each is resumed (emulating).
    devices: Vec<(Device, bool)>,
    sequence: u32,
    keymap: Option<Keymap>,
}

/// A libei sender connection on its own thread.
pub struct Ei {
    ctx: Mutex<Option<ei::Context>>,
    conn: Mutex<Option<reis::event::Connection>>,
    st: Mutex<EiState>,
    alive: AtomicBool,
    closed: AtomicBool,
}

impl Ei {
    /// Starts the handshake and the event thread on the ConnectToEIS socket.
    pub fn start(fd: OwnedFd) -> Result<Arc<Ei>> {
        let ctx = ei::Context::new(UnixStream::from(fd))?;
        let ei = Arc::new(Ei {
            ctx: Mutex::new(Some(ctx.clone())),
            conn: Mutex::new(None),
            st: Mutex::new(EiState::default()),
            alive: AtomicBool::new(true),
            closed: AtomicBool::new(false),
        });
        let me = ei.clone();
        std::thread::Builder::new().name("libei".into()).spawn(move || {
            if let Err(e) = me.run(ctx) {
                eprintln!("sodium-remote-host: libei: {e:#}");
            }
            me.alive.store(false, Ordering::Relaxed);
        })?;
        Ok(ei)
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed) && !self.closed.load(Ordering::Relaxed)
    }

    fn run(&self, ctx: ei::Context) -> Result<()> {
        let resp = reis::handshake::ei_handshake_blocking(&ctx, names::CLIENT_NAME, ei::handshake::ContextType::Sender)?;
        let debug = std::env::var_os("SODIUM_REMOTE_HOST_DEBUG").is_some();
        if debug {
            eprintln!("sodium-remote-host: libei handshake: {:?}", resp.negotiated_interfaces);
        }
        let mut conv = EiEventConverter::new(&ctx, resp);
        *self.conn.lock().unwrap() = Some(conv.connection().clone());
        // Events may already sit in the buffer from the handshake's read: those first.
        loop {
            while let Some(r) = ctx.pending_event() {
                match r {
                    reis::PendingRequestResult::Request(ev) => {
                        if debug {
                            eprintln!("sodium-remote-host: libei event {ev:?}");
                        }
                        conv.handle_event(ev)?
                    }
                    reis::PendingRequestResult::ParseError(e) => anyhow::bail!("libei protocol: {e}"),
                    reis::PendingRequestResult::InvalidObject(id) => {
                        if debug {
                            eprintln!("sodium-remote-host: libei event for unknown object {id}");
                        }
                    }
                }
            }
            while let Some(ev) = conv.next_event() {
                self.on_event(ev);
            }
            let _ = ctx.flush();
            if self.closed.load(Ordering::Relaxed) {
                break;
            }
            let mut fds = [rustix::event::PollFd::new(&ctx, rustix::event::PollFlags::IN)];
            let timeout = rustix::event::Timespec { tv_sec: 0, tv_nsec: 200_000_000 };
            match rustix::event::poll(&mut fds, Some(&timeout)) {
                Ok(0) | Err(rustix::io::Errno::INTR) => continue,
                Ok(_) => {}
                Err(e) => return Err(e.into()),
            }
            ctx.read()?;
        }
        Ok(())
    }

    fn on_event(&self, ev: EiEvent) {
        let mut st = self.st.lock().unwrap();
        match ev {
            EiEvent::SeatAdded(s) => {
                s.seat.bind_capabilities(
                    DeviceCapability::Pointer
                        | DeviceCapability::PointerAbsolute
                        | DeviceCapability::Keyboard
                        | DeviceCapability::Scroll
                        | DeviceCapability::Button
                        | DeviceCapability::Text,
                );
            }
            EiEvent::DeviceAdded(d) => {
                if let Some(km) = d.device.keymap()
                    && let Ok(fd) = km.fd.try_clone()
                {
                    let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                    if let Ok(Some(k)) = unsafe { xkb::Keymap::new_from_fd(&ctx, fd, km.size as usize, xkb::KEYMAP_FORMAT_TEXT_V1, 0) } {
                        st.keymap = Some(Keymap(k));
                    }
                }
                eprintln!(
                    "sodium-remote-host: libei device {:?}: {}",
                    d.device.name().unwrap_or("?"),
                    [
                        (DeviceCapability::Pointer, "pointer"),
                        (DeviceCapability::PointerAbsolute, "absolute pointer"),
                        (DeviceCapability::Keyboard, "keyboard"),
                        (DeviceCapability::Scroll, "scroll"),
                        (DeviceCapability::Button, "buttons"),
                        (DeviceCapability::Text, "text"),
                    ]
                    .iter()
                    .filter(|(c, _)| d.device.has_capability(*c))
                    .map(|(_, n)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
                );
                st.devices.push((d.device, false));
            }
            EiEvent::DeviceRemoved(d) => st.devices.retain(|(x, _)| *x != d.device),
            EiEvent::DeviceResumed(d) => {
                let seq = st.sequence;
                st.sequence += 1;
                d.device.device().start_emulating(d.serial, seq);
                if let Some(e) = st.devices.iter_mut().find(|(x, _)| *x == d.device) {
                    e.1 = true;
                }
            }
            EiEvent::DevicePaused(d) => {
                if let Some(e) = st.devices.iter_mut().find(|(x, _)| *x == d.device) {
                    e.1 = false;
                }
            }
            EiEvent::Disconnected(d) => {
                eprintln!("sodium-remote-host: libei disconnected: {:?} {:?}", d.reason, d.explanation);
                self.alive.store(false, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    /// A resumed device with `cap`.
    fn device(&self, cap: DeviceCapability) -> Option<Device> {
        self.st.lock().unwrap().devices.iter().find(|(d, on)| *on && d.has_capability(cap)).map(|(d, _)| d.clone())
    }

    /// Ends the device's frame and sends.
    fn frame(&self, d: &Device) {
        let serial = self.conn.lock().unwrap().as_ref().map_or(0, |c| c.serial());
        d.device().frame(serial, now_us());
        if let Some(c) = self.ctx.lock().unwrap().as_ref() {
            let _ = c.flush();
        }
    }
}

/// One EIS region (logical coordinates).
#[derive(Clone, Debug, PartialEq)]
struct Area {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    mapping_id: Option<String>,
}

/// Where the shared stream sits among the EIS device's regions: the region whose mapping ID
/// is the stream's; else the region at the stream's position with the stream's size; else
/// the box around all regions (KDE shares the whole workspace when it has several screens, and
/// that stream has no mapping ID and covers more than any one region).
fn pick_area(regions: &[Area], mapping_id: Option<&str>, position: (i32, i32), logical: (i32, i32)) -> Option<Area> {
    if let Some(id) = mapping_id
        && let Some(r) = regions.iter().find(|r| r.mapping_id.as_deref() == Some(id))
    {
        return Some(r.clone());
    }
    let (lw, lh) = (logical.0 as f64, logical.1 as f64);
    if let Some(r) = regions.iter().find(|r| (r.x as i32, r.y as i32) == position && r.w == lw && r.h == lh) {
        return Some(r.clone());
    }
    let first = regions.first()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x + first.w, first.y + first.h);
    for r in regions {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.x + r.w);
        y1 = y1.max(r.y + r.h);
    }
    Some(Area { x: x0, y: y0, w: x1 - x0, h: y1 - y0, mapping_id: None })
}

/// The injector: libei while it works, else the portal's Notify methods.
pub struct PortalInput {
    portal: Arc<Portal>,
    keys: HashSet<u32>,
    buttons: HashSet<u32>,
    /// Wheel 120ths not yet a whole step (Notify takes steps).
    wheel: [i32; 2],
    warned: Option<Instant>,
}

impl PortalInput {
    pub fn new(portal: Arc<Portal>) -> PortalInput {
        PortalInput { portal, keys: HashSet::new(), buttons: HashSet::new(), wheel: [0; 2], warned: None }
    }

    fn key(&mut self, a: &Active, code: u32, down: bool) {
        if down {
            self.keys.insert(code);
        } else {
            self.keys.remove(&code);
        }
        if let Some(ei) = a.ei.as_ref().filter(|e| e.alive()) {
            if let Some(d) = ei.device(DeviceCapability::Keyboard)
                && let Some(k) = d.interface::<ei::Keyboard>()
            {
                k.key(code, if down { ei::keyboard::KeyState::Press } else { ei::keyboard::KeyState::Released });
                ei.frame(&d);
            }
            return;
        }
        self.portal.notify(Notify::Key { code: code as i32, down });
    }

    fn button(&mut self, a: &Active, code: u32, down: bool) {
        if down {
            self.buttons.insert(code);
        } else {
            self.buttons.remove(&code);
        }
        if let Some(ei) = a.ei.as_ref().filter(|e| e.alive()) {
            if let Some(d) = ei.device(DeviceCapability::Button)
                && let Some(b) = d.interface::<ei::Button>()
            {
                b.button(code, if down { ei::button::ButtonState::Press } else { ei::button::ButtonState::Released });
                ei.frame(&d);
            }
            return;
        }
        self.portal.notify(Notify::Button { code: code as i32, down });
    }

    fn text(&mut self, a: &Active, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(ei) = a.ei.as_ref().filter(|e| e.alive()) {
            if let Some(d) = ei.device(DeviceCapability::Text)
                && let Some(t) = d.interface::<ei::Text>()
            {
                t.utf8(text);
                ei.frame(&d);
                return;
            }
            let Some(d) = ei.device(DeviceCapability::Keyboard) else { return };
            let Some(k) = d.interface::<ei::Keyboard>() else { return };
            let st = ei.st.lock().unwrap();
            let Some(km) = st.keymap.as_ref() else {
                eprintln!("sodium-remote-host: no keymap from libei: typed text dropped");
                return;
            };
            let mut missing = 0;
            for c in text.chars() {
                let Some((code, mods)) = km.find(c) else {
                    missing += 1;
                    continue;
                };
                use ei::keyboard::KeyState::{Press, Released};
                for m in mods {
                    k.key(*m, Press);
                }
                k.key(code, Press);
                ei.frame(&d);
                k.key(code, Released);
                for m in mods.iter().rev() {
                    k.key(*m, Released);
                }
                ei.frame(&d);
            }
            if missing > 0 {
                eprintln!("sodium-remote-host: {missing} typed characters aren't on this keyboard layout");
            }
            return;
        }
        for c in text.chars() {
            let sym = xkb::utf32_to_keysym(c as u32).raw() as i32;
            self.portal.notify(Notify::Keysym { sym, down: true });
            self.portal.notify(Notify::Keysym { sym, down: false });
        }
    }

    fn pointer_abs(&mut self, a: &Active, x: f64, y: f64) {
        let (lx, ly) = a.to_logical(x, y);
        if let Some(ei) = a.ei.as_ref().filter(|e| e.alive()) {
            if let Some(d) = ei.device(DeviceCapability::PointerAbsolute)
                && let Some(p) = d.interface::<ei::PointerAbsolute>()
            {
                let regions: Vec<Area> = d.regions().iter().map(|r| Area { x: r.x as f64, y: r.y as f64, w: r.width as f64, h: r.height as f64, mapping_id: r.mapping_id.clone() }).collect();
                let (ox, oy, sx, sy) = match pick_area(&regions, a.mapping_id.as_deref(), a.position, a.logical) {
                    // The area's own size maps the stream's pixels when it differs from the portal's.
                    Some(r) => (r.x, r.y, r.w / a.pixels.0.max(1) as f64, r.h / a.pixels.1.max(1) as f64),
                    None => (a.position.0 as f64, a.position.1 as f64, lx / x.max(1e-9), ly / y.max(1e-9)),
                };
                p.motion_absolute((ox + x * sx) as f32, (oy + y * sy) as f32);
                ei.frame(&d);
            }
            return;
        }
        self.portal.notify(Notify::Abs { x: lx, y: ly });
    }

    fn pointer_rel(&mut self, a: &Active, dx: f64, dy: f64) {
        let (lx, ly) = a.to_logical(dx, dy);
        if let Some(ei) = a.ei.as_ref().filter(|e| e.alive()) {
            if let Some(d) = ei.device(DeviceCapability::Pointer)
                && let Some(p) = d.interface::<ei::Pointer>()
            {
                p.motion_relative(lx as f32, ly as f32);
                ei.frame(&d);
            }
            return;
        }
        self.portal.notify(Notify::Rel { dx: lx, dy: ly });
    }

    #[allow(clippy::too_many_arguments)]
    fn axis(&mut self, a: &Active, source: AxisSource, axis: Axis, value: f64, value120: i32, stop: bool, more: bool) {
        let vertical = axis == Axis::Vertical;
        let wheel = matches!(source, AxisSource::Wheel | AxisSource::WheelTilt) && value120 != 0;
        if let Some(ei) = a.ei.as_ref().filter(|e| e.alive()) {
            if let Some(d) = ei.device(DeviceCapability::Scroll)
                && let Some(s) = d.interface::<ei::Scroll>()
            {
                if wheel {
                    if vertical { s.scroll_discrete(0, value120) } else { s.scroll_discrete(value120, 0) }
                } else if stop {
                    if vertical { s.scroll_stop(0, 1, 0) } else { s.scroll_stop(1, 0, 0) }
                } else if vertical {
                    s.scroll(0.0, value as f32)
                } else {
                    s.scroll(value as f32, 0.0)
                }
                if !more {
                    ei.frame(&d);
                }
            }
            return;
        }
        if wheel {
            let i = vertical as usize;
            self.wheel[i] += value120;
            let steps = self.wheel[i] / 120;
            self.wheel[i] -= steps * 120;
            if steps != 0 {
                self.portal.notify(Notify::Discrete { vertical, steps });
            }
        } else {
            let (dx, dy) = if vertical { (0.0, value) } else { (value, 0.0) };
            self.portal.notify(Notify::Axis { dx, dy, finish: stop });
        }
    }

    fn release_all(&mut self, a: &Active) {
        for code in std::mem::take(&mut self.keys) {
            self.key(a, code, false);
            self.keys.remove(&code);
        }
        for code in std::mem::take(&mut self.buttons) {
            self.button(a, code, false);
            self.buttons.remove(&code);
        }
    }
}

impl Input for PortalInput {
    fn inject(&mut self, ev: InputOut) {
        let Some(a) = self.portal.active() else {
            self.keys.clear();
            self.buttons.clear();
            return;
        };
        match ev {
            InputOut::Key { usage, down } => {
                if let Some(code) = crate::hid::hid_to_evdev(usage) {
                    self.key(&a, code, down);
                }
            }
            InputOut::Text(t) => self.text(&a, &t),
            InputOut::PointerAbs { x, y, .. } => self.pointer_abs(&a, x, y),
            InputOut::PointerRel { dx, dy } => self.pointer_rel(&a, dx, dy),
            InputOut::Button { button, down } => self.button(&a, button_code(button), down),
            InputOut::Axis { source, axis, value, value120, stop, more } => self.axis(&a, source, axis, value, value120, stop, more),
            InputOut::Zoom { .. } => {
                if self.warned.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
                    eprintln!("sodium-remote-host: pinch zoom isn't available through the portal");
                    self.warned = Some(Instant::now());
                }
            }
            InputOut::ReleaseAll => self.release_all(&a),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: f64, y: f64, w: f64, h: f64, id: &str) -> Area {
        Area { x, y, w, h, mapping_id: Some(id.into()) }
    }

    #[test]
    fn a_stream_is_matched_to_its_region_by_mapping_id() {
        let rs = [area(0.0, 0.0, 1920.0, 1080.0, "A"), area(1920.0, 0.0, 960.0, 540.0, "B")];
        assert_eq!(pick_area(&rs, Some("B"), (0, 0), (960, 540)).unwrap().x, 1920.0);
    }

    #[test]
    fn a_workspace_stream_without_an_id_maps_onto_the_box_around_the_regions() {
        let rs = [area(0.0, 0.0, 1920.0, 1080.0, "A"), area(1920.0, 0.0, 960.0, 540.0, "B")];
        let r = pick_area(&rs, None, (0, 0), (2880, 1080)).unwrap();
        assert_eq!((r.x, r.y, r.w, r.h), (0.0, 0.0, 2880.0, 1080.0));
        // one screen: its own region
        let one = [area(0.0, 0.0, 1920.0, 1080.0, "A")];
        assert_eq!(pick_area(&one, None, (0, 0), (1920, 1080)).unwrap().w, 1920.0);
    }
}
