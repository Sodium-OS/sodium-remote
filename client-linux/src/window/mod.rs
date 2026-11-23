// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The stream window: a Wayland xdg_toplevel drawn with EGL + GLES 3.
//!
//! - **Present:** latest frame wins. A new picture is drawn and committed as
//!   soon as it is ready, even before the previous commit's frame callback: the
//!   compositor shows the newest buffer at its next vsync. (Waiting for the
//!   callback during motion put every frame a whole refresh late: measured 22 ms
//!   ready → presented instead of 2.) A redraw with no new picture (the HUD)
//!   waits for the callback.
//!   Every commit asks for wp_presentation feedback, which stamps the frame.
//!   Each presented frame's lead before its vsync feeds the phase lock's
//!   `Msg::Vsync` report, about once a second.
//! - **Size:** `ClientDisplay` is the window's backing pixels (the output's in
//!   full screen), the output's refresh and mode. Every configure that changes
//!   it sends `Msg::Resize` (the host debounces) unless the resolution is fixed.
//!   A stream up to 64 px smaller than the window is drawn 1:1 and centred
//!   (`layout`); anything else is fitted.
//! - **Cursor:** drawn locally. Its image comes from the cursor channel (by
//!   hash) and becomes the Wayland cursor, so it moves with no stream latency.
//!   When the pointer is outside the window, it is drawn at the host's position.
//! - **Input:** pointer (absolute, relative under lock), buttons, scroll with
//!   source, value120, stop and `more`, and keys as HID usages, with the
//!   Wayland event time, into core's input channel (redundant until
//!   acknowledged).
//! - **Input method:** text-input-v3 on the focused surface while the keyboard
//!   goes to the host. The local input method (fcitx5) composes, showing its own
//!   candidates by the pointer; only its committed text goes to the host, as
//!   `ImeCommit` (no remote preedit). Keys it
//!   doesn't take come back to the window and go as keys, as before.
//!
//! PROVISIONAL: the window chrome, HUD and title are the reference client's
//! own; the designed client is the Mac's.

pub mod embed;
mod gl;
mod hud;

use crate::App;
use embed::{Embed, EmbedEvent};
use crate::connect::wayland_time;
use crate::hid;
use crate::msg::{self, ClientDisplay, CursorImage, InputEvent as Input, Msg};
use crate::proto::{StreamInfo, button_from_evdev};
use remote_core::phase::LeadStats;
use anyhow::{Context, Result, anyhow};
use khronos_egl as egl;
use std::collections::{HashMap, HashSet};
use std::os::fd::{AsFd, AsRawFd};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer::WlBuffer,
        wl_callback::{self, WlCallback},
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard},
        wl_output::{self, WlOutput},
        wl_pointer::{self, WlPointer},
        wl_region::WlRegion,
        wl_registry::WlRegistry,
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_subcompositor::WlSubcompositor,
        wl_subsurface::WlSubsurface,
        wl_surface::{self, WlSurface},
    },
};
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_confined_pointer_v1::ZwpConfinedPointerV1,
    zwp_locked_pointer_v1::ZwpLockedPointerV1,
    zwp_pointer_constraints_v1::{Lifetime, ZwpPointerConstraintsV1},
};
use wayland_protocols::wp::presentation_time::client::{
    wp_presentation::{self, WpPresentation},
    wp_presentation_feedback::{self, WpPresentationFeedback},
};
use wayland_protocols::wp::relative_pointer::zv1::client::{
    zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
    zwp_relative_pointer_v1::{self, ZwpRelativePointerV1},
};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3::ZwpTextInputManagerV3,
    zwp_text_input_v3::{self, ContentHint, ContentPurpose, ZwpTextInputV3},
};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

pub use remote_engine::layout;
pub use remote_engine::{Latest, WinMsg, WindowOptions as Options};

/// evdev KEY_ESC.
const KEY_ESC: u32 = 1;

struct FeedbackData {
    frame_id: Option<u32>,
}

#[derive(Default)]
struct AxisAcc {
    /// The first event's time in this pointer frame (ms).
    time: Option<u32>,
    source: Option<u32>,
    value: [f64; 2],
    value120: [i32; 2],
    stop: [bool; 2],
    any: [bool; 2],
}

/// `REMOTE_LOG_IME=1`: every text-input event and request on stderr (never the text, only its length).
fn log_ime() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("REMOTE_LOG_IME").is_some())
}

/// The seat's text-input-v3 object (the local input method's side).
struct Ime {
    ti: ZwpTextInputV3,
    /// Text-input focus is on our surface.
    focused: bool,
    enabled: bool,
    /// The compositor's view: the last state committed. A leave doesn't reset it in Hyprland (0.57),
    /// so after a focus change an enable alone is no change, and the input method is never
    /// activated: re-enabling sends a disable first.
    committed_enabled: bool,
    /// The commit that the next `done` applies.
    pending: Option<String>,
}

struct Gfx {
    egl: egl::Instance<egl::Static>,
    display: egl::Display,
    surface: egl::Surface,
    _ctx: egl::Context,
    window: wayland_egl::WlEglSurface,
    r: gl::Renderer,
    hud_tex: Option<gl::Rgba>,
    cursor_tex: Option<(u64, gl::Rgba, i32, i32)>,
}

struct Win {
    app: Arc<App>,
    opts: Options,
    qh: QueueHandle<Win>,
    compositor: WlCompositor,
    surface: WlSurface,
    _xdg_surface: Option<XdgSurface>,
    _toplevel: Option<XdgToplevel>,
    /// Embedded in the Qt app: Qt's toplevel surface (the pointer and keyboard focus) and our subsurface under it.
    embed: Option<Arc<Embed>>,
    parent: Option<WlSurface>,
    _sub: Option<WlSubsurface>,
    emb_gen: u64,
    /// Whether the last embed state captured the keyboard: the Qt app turning it off releases what is held.
    was_capturing: bool,
    origin: (i32, i32),
    /// The pointer is over the app's own HUD: nothing goes to the host.
    over_hud: bool,
    last_pos: (f64, f64),
    shm: WlShm,
    presentation: Option<WpPresentation>,
    rel_mgr: Option<ZwpRelativePointerManagerV1>,
    constraints: Option<ZwpPointerConstraintsV1>,
    _viewporter: Option<WpViewporter>,
    /// wl_output globals by name.
    outputs: HashMap<u32, Output>,
    /// The output the surface entered last.
    on_output: Option<u32>,
    fullscreen: bool,
    /// The display last published (`app.display`; `Resize` after the first).
    display: Option<ClientDisplay>,
    lead: LeadStats,
    lead_gen: u64,
    next_vsync: Instant,
    vsync_reports: u64,
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    ime_mgr: Option<ZwpTextInputManagerV3>,
    ime: Option<Ime>,
    relative: Option<ZwpRelativePointerV1>,
    locked: Option<ZwpLockedPointerV1>,
    confined: Option<ZwpConfinedPointerV1>,
    cursor_surface: WlSurface,
    cursor_viewport: Option<WpViewport>,
    gfx: Option<Gfx>,
    configured: bool,
    size: (i32, i32),
    pending_size: Option<(i32, i32)>,
    scale: i32,
    closed: bool,
    frame_pending: Option<Instant>,
    dirty: bool,
    clock_id: u32,
    pointer_serial: u32,
    pointer_in: bool,
    axis: AxisAcc,
    keys_down: HashSet<u32>,
    /// The keyboard focus is on our surface (embedded: the stream window, not the app's other windows).
    kb_focus: bool,
    shapes: HashMap<u64, CursorImage>,
    shape_bufs: HashMap<u64, WlBuffer>,
    cursor_applied: Option<(u64, i32)>,
    remote: (f32, f32, bool, u64),
    lock_state: msg::PointerLock,
    stream: Option<StreamInfo>,
    hud: Option<hud::Hud>,
    next_hud: Instant,
}

struct Output {
    /// The current mode: width, height (pixels), refresh (mHz).
    mode: Option<(u32, u32, u32)>,
    scale: i32,
}

impl Win {
    /// Buffer pixels.
    fn buffer_size(&self) -> (u32, u32) {
        ((self.size.0 * self.scale).max(1) as u32, (self.size.1 * self.scale).max(1) as u32)
    }

    /// The picture shown (what the pointer maps onto), else the stream as described.
    fn stream_size(&self) -> Option<(u32, u32)> {
        if let Some(s) = self.gfx.as_ref().and_then(|g| g.r.video_size) {
            return Some(s);
        }
        self.stream.map(|s| (s.width, s.height))
    }

    /// The video's rect in buffer pixels (`layout::place`).
    fn video_rect(&self) -> (f32, f32, f32, f32) {
        let (bw, bh) = self.buffer_size();
        let Some(s) = self.stream_size() else { return (0.0, 0.0, bw as f32, bh as f32) };
        let p = layout::place((bw, bh), s);
        (p.x as f32, p.y as f32, p.w as f32, p.h as f32)
    }

    /// Logical window px per stream px.
    fn video_scale(&self) -> f64 {
        match self.stream_size() {
            Some((sw, _)) => self.video_rect().2 as f64 / self.scale as f64 / sw as f64,
            None => 1.0,
        }
    }

    fn to_stream(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let stream = self.stream_size()?;
        let (vx, vy, vw, vh) = self.video_rect();
        let p = layout::Placement { x: vx as f64, y: vy as f64, w: vw as f64, h: vh as f64 };
        let s = self.scale as f64;
        Some(layout::to_stream(p, stream, (x - self.origin.0 as f64) * s, (y - self.origin.1 as f64) * s))
    }

    fn send(&self, input: Input) {
        self.app.send_input(input);
    }

    /// The surface the seat's focus lands on: the Qt toplevel when embedded.
    fn focus_surface(&self) -> &WlSurface {
        self.parent.as_ref().unwrap_or(&self.surface)
    }

    fn is_focus(&self, s: &WlSurface) -> bool {
        *s == self.surface || self.parent.as_ref() == Some(s)
    }

    /// Takes what the Qt app set: size, scale, full screen, the HUD's rects, keyboard capture.
    fn embed_sync(&mut self) {
        let Some(e) = self.embed.clone() else { return };
        let st = e.state.lock().unwrap().clone();
        if st.rev == self.emb_gen {
            return;
        }
        self.emb_gen = st.rev;
        if self.was_capturing && !st.capture {
            self.release_keys();
        }
        if self.was_capturing != st.capture {
            self.was_capturing = st.capture;
            self.update_ime();
        }
        self.fullscreen = st.fullscreen;
        if st.origin != self.origin {
            self.origin = st.origin;
            if let Some(sub) = &self._sub {
                sub.set_position(st.origin.0, st.origin.1);
            }
        }
        let changed = (st.size != self.size || st.scale != self.scale) && st.size.0 > 0 && st.size.1 > 0;
        if changed {
            self.size = st.size;
            self.scale = st.scale.max(1);
            self.resize();
        } else {
            self.update_display();
        }
        self.dirty = true;
    }

    fn in_hud(&self, x: f64, y: f64) -> bool {
        let Some(e) = &self.embed else { return false };
        e.state.lock().unwrap().hud_rects.iter().any(|r| x >= r[0] as f64 && y >= r[1] as f64 && x < (r[0] + r[2]) as f64 && y < (r[1] + r[3]) as f64)
    }

    fn capturing(&self) -> bool {
        self.embed.as_ref().is_none_or(|e| e.state.lock().unwrap().capture)
    }

    fn set_capture(&mut self, on: bool) {
        if let Some(e) = &self.embed {
            let mut st = e.state.lock().unwrap();
            if st.capture == on {
                return;
            }
            st.capture = on;
            drop(st);
            (e.events)(EmbedEvent::Capture(on));
        }
        self.update_ime();
    }

    /// The input method is on while our surface has the keyboard and it goes to the host
    /// (in the Qt app, Qt's menus and fields take it back).
    fn update_ime(&mut self) {
        let want = self.ime.as_ref().is_some_and(|i| i.focused) && self.capturing();
        let pos = self.last_pos;
        let Some(ime) = self.ime.as_mut() else { return };
        if want == ime.enabled {
            return;
        }
        ime.enabled = want;
        ime.pending = None;
        if log_ime() {
            eprintln!("remote-client: ime: {} at {:?}", if want { "enable" } else { "disable" }, pos);
        }
        if want {
            if ime.committed_enabled {
                ime.ti.disable();
                ime.ti.commit();
            }
            ime.ti.enable();
            ime.ti.set_content_type(ContentHint::None, ContentPurpose::Normal);
            ime.ti.set_cursor_rectangle(pos.0 as i32, pos.1 as i32, 1, 20);
        } else {
            ime.ti.disable();
        }
        ime.ti.commit();
        ime.committed_enabled = want;
    }

    /// The candidate window opens where the pointer last clicked: the host's caret isn't known here.
    fn ime_cursor(&mut self) {
        let pos = self.last_pos;
        if let Some(ime) = self.ime.as_ref().filter(|i| i.enabled) {
            ime.ti.set_cursor_rectangle(pos.0 as i32, pos.1 as i32, 1, 20);
            ime.ti.commit();
        }
    }

    /// An event with its Wayland time (ms).
    fn send_at(&self, input: Input, time_ms: u32) {
        self.app.send_input_at(input, wayland_time(time_ms));
    }

    /// The output the window is on (the one it entered last, else the first known).
    fn output(&self) -> Option<&Output> {
        self.on_output.and_then(|n| self.outputs.get(&n)).or_else(|| self.outputs.values().find(|o| o.mode.is_some()))
    }

    /// What `Hello` and `Resize` say about the display.
    fn client_display(&self) -> ClientDisplay {
        let out = self.output();
        let mode = out.and_then(|o| o.mode);
        let (width, height) = match (self.opts.size, mode) {
            (Some(s), _) => s,
            // Full screen: the output's own pixels.
            (None, Some((w, h, _))) if self.fullscreen => (w, h),
            _ => self.buffer_size(),
        };
        let (dw, dh, refresh) = mode.unwrap_or((width, height, 60_000));
        ClientDisplay {
            width,
            height,
            pixels_per_point: self.opts.ppp.unwrap_or(out.map_or(self.scale, |o| o.scale) as f32),
            refresh_mhz: refresh,
            fullscreen: self.fullscreen,
            display_width: dw,
            display_height: dh,
        }
    }

    /// Publishes the display: the first one for `Hello`, then a `Resize` on
    /// every change (none with a fixed or forced size).
    fn update_display(&mut self) {
        let d = self.client_display();
        if self.display == Some(d) {
            return;
        }
        let first = self.display.is_none();
        if !first && (self.opts.fixed || self.opts.size.is_some() || !self.app.live_resize()) {
            return;
        }
        eprintln!(
            "remote-client: display {}x{} (output {}x{} @{:.3} Hz, ppp {}, fullscreen {}){}",
            d.width,
            d.height,
            d.display_width,
            d.display_height,
            d.refresh_mhz as f64 / 1000.0,
            d.pixels_per_point,
            d.fullscreen,
            if first { "" } else { ": Resize" }
        );
        self.display = Some(d);
        *self.app.display.lock().unwrap() = Some(d);
        if !first {
            self.app.send_msg(Msg::Resize(d));
        }
    }

    /// The display's refresh period (ns), for feedback that has none.
    fn refresh_ns(&self) -> u32 {
        self.output().and_then(|o| o.mode).map_or(0, |(_, _, mhz)| (1e12 / mhz.max(1000) as f64) as u32)
    }

    /// A frame ready at `ready_us` was presented at `present_us` (client µs):
    /// its lead before that vsync, on the live session's clock, for `Msg::Vsync`
    /// about once a second.
    fn on_lead(&mut self, ready_us: i64, present_us: i64, refresh_ns: u32) {
        let Some(live) = self.app.live() else { return };
        if live.generation != self.lead_gen {
            self.lead = LeadStats::default();
            self.lead_gen = live.generation;
        }
        let period = if refresh_ns > 0 { refresh_ns } else { self.refresh_ns() };
        self.lead.add(ready_us - live.offset_us, present_us - live.offset_us, period);
        if Instant::now() < self.next_vsync {
            return;
        }
        self.next_vsync = Instant::now() + Duration::from_secs(1);
        if let Some(r) = self.lead.report() {
            if self.vsync_reports % 10 == 0 {
                eprintln!(
                    "remote-client: vsync report #{}: lead p50 {} µs, p05 {} µs, {} frames, period {} ns",
                    self.vsync_reports + 1,
                    r.lead_p50_us,
                    r.lead_p05_us,
                    r.frames,
                    r.period_ns
                );
            }
            self.vsync_reports += 1;
            self.app.send_msg(Msg::Vsync(r));
        }
    }

    fn handle(&mut self, m: WinMsg) {
        match m {
            WinMsg::Stream(s) => {
                self.stream = Some(s);
                self.cursor_applied = None;
                self.apply_cursor();
                self.dirty = true;
            }
            WinMsg::CursorShape { hash, image } => {
                self.shapes.insert(hash, image);
                if self.remote.3 == hash {
                    self.cursor_applied = None;
                    self.apply_cursor();
                    self.dirty = true;
                }
            }
            WinMsg::CursorPos { x, y, visible, hash } => {
                let changed = self.remote.3 != hash || self.remote.2 != visible;
                self.remote = (x, y, visible, hash);
                if changed {
                    self.apply_cursor();
                }
                if !self.pointer_in {
                    self.dirty = true;
                }
            }
            // The host's secure screen: the Linux reference client only logs it (the engine does).
            WinMsg::SecureScreen(_) => {}
            WinMsg::PointerLock(state) => {
                self.lock_state = state;
                self.apply_lock();
            }
        }
    }

    fn apply_lock(&mut self) {
        let want = if self.pointer_in && !self.over_hud { self.lock_state } else { msg::PointerLock::None };
        if let Some(l) = self.locked.take() {
            l.destroy();
        }
        if let Some(c) = self.confined.take() {
            c.destroy();
        }
        if let Some(r) = self.relative.take() {
            r.destroy();
        }
        let (Some(ptr), Some(pc)) = (&self.pointer, &self.constraints) else { return };
        match want {
            msg::PointerLock::Locked => {
                self.locked = Some(pc.lock_pointer(self.focus_surface(), ptr, None, Lifetime::Persistent, &self.qh, ()));
                if let Some(rm) = &self.rel_mgr {
                    self.relative = Some(rm.get_relative_pointer(ptr, &self.qh, ()));
                }
            }
            msg::PointerLock::Confined => self.confined = Some(pc.confine_pointer(self.focus_surface(), ptr, None, Lifetime::Persistent, &self.qh, ())),
            _ => {}
        }
        self.cursor_applied = None;
        self.apply_cursor();
    }

    fn shm_buffer(&self, img: &CursorImage) -> Option<WlBuffer> {
        let size = img.pixels.len();
        let fd = rustix_memfd(size)?;
        let n = unsafe { libc::pwrite(fd.as_raw_fd(), img.pixels.as_ptr().cast(), size, 0) };
        if n != size as isize {
            return None;
        }
        let pool: WlShmPool = self.shm.create_pool(fd.as_fd(), size as i32, &self.qh, ());
        let b = pool.create_buffer(
            0,
            img.width as i32,
            img.height as i32,
            img.width as i32 * 4,
            wl_shm::Format::Argb8888,
            &self.qh,
            (),
        );
        pool.destroy();
        Some(b)
    }

    /// The host's cursor shape becomes our Wayland cursor (scaled like the video).
    fn apply_cursor(&mut self) {
        let Some(ptr) = self.pointer.clone() else { return };
        if !self.pointer_in {
            return;
        }
        if self.locked.is_some() || !self.remote.2 {
            ptr.set_cursor(self.pointer_serial, None, 0, 0);
            self.cursor_applied = None;
            return;
        }
        let hash = self.remote.3;
        let s = self.video_scale();
        let key = (hash, (s * 1000.0) as i32);
        if self.cursor_applied == Some(key) {
            return;
        }
        let Some(img) = self.shapes.get(&hash).cloned() else { return };
        if !self.shape_bufs.contains_key(&hash) {
            let Some(b) = self.shm_buffer(&img) else { return };
            self.shape_bufs.insert(hash, b);
        }
        let buf = &self.shape_bufs[&hash];
        self.cursor_surface.attach(Some(buf), 0, 0);
        let k = if self.cursor_viewport.is_some() { s } else { 1.0 };
        if let Some(vp) = &self.cursor_viewport {
            vp.set_destination(((img.width as f64 * s).round() as i32).max(1), ((img.height as f64 * s).round() as i32).max(1));
        }
        self.cursor_surface.damage_buffer(0, 0, img.width as i32, img.height as i32);
        self.cursor_surface.commit();
        ptr.set_cursor(self.pointer_serial, Some(&self.cursor_surface), (img.hot_x as f64 * k) as i32, (img.hot_y as f64 * k) as i32);
        self.cursor_applied = Some(key);
    }

    /// Ctrl, Alt and Shift (either side) are all down.
    fn chord_held(&self) -> bool {
        let any = |a: u32, b: u32| self.keys_down.contains(&a) || self.keys_down.contains(&b);
        any(29, 97) && any(56, 100) && any(42, 54)
    }

    fn release_keys(&mut self) {
        for k in std::mem::take(&mut self.keys_down) {
            if let Some(usage) = hid::evdev_to_hid(k) {
                self.send(Input::Key { usage, down: false });
            }
        }
    }

    /// `fresh`: a new picture is waiting (it never waits for the frame callback).
    fn can_draw(&self, fresh: bool) -> bool {
        self.configured && self.gfx.is_some() && (fresh || self.frame_pending.is_none_or(|t| t.elapsed() > Duration::from_secs(1)))
    }

    fn draw(&mut self) -> Result<()> {
        let app = self.app.clone();
        let mut new_frame = None;
        if let Some((pic, id)) = app.latest.take() {
            let r = &mut self.gfx.as_mut().unwrap().r;
            let before = r.video_size;
            if let Err(e) = r.upload(&pic) {
                eprintln!("remote-client: upload: {e}");
            }
            let resized = r.video_size != before;
            drop(pic);
            new_frame = Some(id);
            if resized {
                // A new stream size: the placement and the cursor's scale change.
                self.cursor_applied = None;
                self.apply_cursor();
            }
        }
        let (bw, bh) = self.buffer_size();
        let video = self.video_rect();
        let show_remote_cursor = !self.pointer_in && self.remote.2;
        let hud_lines = (self.opts.hud && Instant::now() >= self.next_hud).then(|| {
            let mut extra = Vec::new();
            if let Some(v) = &self.app.timeline.lock().unwrap().shown {
                extra.push(format!("link: {v}"));
            }
            if let Some(c) = &self.app.click {
                extra.push(c.status());
            }
            self.app.stats.hud_lines(&extra)
        });
        let vs = self.video_scale() * self.scale as f64;
        let (remote, shape) = (self.remote, self.shapes.get(&self.remote.3).cloned());
        let gfx = self.gfx.as_mut().unwrap();
        if let (Some(lines), Some(h)) = (hud_lines, self.hud.as_ref()) {
            let (w, hh, px) = h.render(&lines);
            if let Some(old) = gfx.hud_tex.take() {
                gfx.r.delete(old);
            }
            gfx.hud_tex = gfx.r.rgba_texture(w, hh, &px, false, true);
            self.next_hud = Instant::now() + Duration::from_millis(500);
        }
        if show_remote_cursor {
            if let Some(img) = shape.filter(|_| gfx.cursor_tex.as_ref().is_none_or(|c| c.0 != remote.3)) {
                if let Some((_, t, _, _)) = gfx.cursor_tex.take() {
                    gfx.r.delete(t);
                }
                if let Some(t) = gfx.r.rgba_texture(img.width, img.height, &img.pixels, true, false) {
                    gfx.cursor_tex = Some((remote.3, t, img.hot_x, img.hot_y));
                }
            }
        }
        let mut overlays = Vec::new();
        if show_remote_cursor {
            if let Some((_, t, hx, hy)) = &gfx.cursor_tex {
                let x = video.0 + ((remote.0 as f64 - *hx as f64) * vs) as f32;
                let y = video.1 + ((remote.1 as f64 - *hy as f64) * vs) as f32;
                overlays.push((t, (x, y, (t.w as f64 * vs) as f32, (t.h as f64 * vs) as f32)));
            }
        }
        if self.opts.hud {
            if let Some(t) = &gfx.hud_tex {
                let m = 12.0 * self.scale as f32;
                // Bottom left, clear of the click test's marker.
                overlays.push((t, (m, bh as f32 - m - t.h as f32, t.w as f32, t.h as f32)));
            }
        }
        self.surface.frame(&self.qh, ());
        self.frame_pending = Some(Instant::now());
        if let Some(p) = &self.presentation {
            p.feedback(&self.surface, &self.qh, FeedbackData { frame_id: new_frame });
        }
        gfx.r.draw(bw, bh, video, &overlays);
        if let Some(id) = new_frame {
            app.stats.on_commit(id, app.clock.now_us() as i64);
        }
        gfx.egl.swap_buffers(gfx.display, gfx.surface).map_err(|e| anyhow!("eglSwapBuffers: {e}"))?;
        self.dirty = false;
        Ok(())
    }

    fn resize(&mut self) {
        let (bw, bh) = self.buffer_size();
        if let Some(g) = &self.gfx {
            g.window.resize(bw as i32, bh as i32, 0, 0);
        }
        self.surface.set_buffer_scale(self.scale);
        let region: WlRegion = self.compositor.create_region(&self.qh, ());
        region.add(0, 0, self.size.0, self.size.1);
        self.surface.set_opaque_region(Some(&region));
        region.destroy();
        if self.embed.is_some() {
            // The subsurface never takes the pointer: Qt's surface above it does.
            let none: WlRegion = self.compositor.create_region(&self.qh, ());
            self.surface.set_input_region(Some(&none));
            none.destroy();
        }
        self.cursor_applied = None;
        self.apply_cursor();
        self.dirty = true;
        self.update_display();
    }
}

fn rustix_memfd(size: usize) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    let fd = unsafe { libc::memfd_create(c"remote-cursor".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    (unsafe { libc::ftruncate(fd.as_raw_fd(), size as libc::off_t) } == 0).then_some(fd)
}

fn init_gfx(conn: &Connection, surface: &WlSurface, w: u32, h: u32) -> Result<Gfx> {
    let egl = egl::Instance::new(egl::Static);
    let dpy = conn.backend().display_ptr();
    let display = unsafe { egl.get_display(dpy.cast()) }.context("eglGetDisplay")?;
    egl.initialize(display).map_err(|e| anyhow!("eglInitialize: {e}"))?;
    egl.bind_api(egl::OPENGL_ES_API).map_err(|e| anyhow!("eglBindAPI: {e}"))?;
    let attrs = [
        egl::RED_SIZE,
        8,
        egl::GREEN_SIZE,
        8,
        egl::BLUE_SIZE,
        8,
        egl::ALPHA_SIZE,
        0,
        egl::SURFACE_TYPE,
        egl::WINDOW_BIT,
        egl::RENDERABLE_TYPE,
        egl::OPENGL_ES3_BIT,
        egl::NONE,
    ];
    let config = egl
        .choose_first_config(display, &attrs)
        .map_err(|e| anyhow!("eglChooseConfig: {e}"))?
        .context("no EGL config for GLES 3 RGB888")?;
    let ctx = egl
        .create_context(display, config, None, &[egl::CONTEXT_MAJOR_VERSION, 3, egl::NONE])
        .map_err(|e| anyhow!("eglCreateContext: {e}"))?;
    let window = wayland_egl::WlEglSurface::new(surface.id(), w as i32, h as i32).map_err(|e| anyhow!("wl_egl_window: {e:?}"))?;
    let esurf = unsafe { egl.create_window_surface(display, config, window.ptr() as egl::NativeWindowType, None) }
        .map_err(|e| anyhow!("eglCreateWindowSurface: {e}"))?;
    egl.make_current(display, Some(esurf), Some(esurf), Some(ctx)).map_err(|e| anyhow!("eglMakeCurrent: {e}"))?;
    // Never block in eglSwapBuffers: pacing comes from frame callbacks.
    egl.swap_interval(display, 0).map_err(|e| anyhow!("eglSwapInterval: {e}"))?;
    let glc = unsafe {
        glow::Context::from_loader_function(|s| egl.get_proc_address(s).map_or(std::ptr::null(), |p| p as *const _))
    };
    let r = gl::Renderer::new(glc).map_err(|e| anyhow!("GL: {e}"))?;
    Ok(Gfx { egl, display, surface: esurf, _ctx: ctx, window, r, hud_tex: None, cursor_tex: None })
}

/// Runs the window on this thread until it closes or the app quits.
pub fn run(app: Arc<App>, rx: Receiver<WinMsg>, opts: Options, wake_src: Arc<crate::Wake>) -> Result<()> {
    run_in(app, rx, opts, wake_src, None)
}

/// Runs the video surface as a subsurface below the Qt app's toplevel, on Qt's own `wl_display`
/// Qt draws the HUD above it, the engine keeps the seat listeners.
pub fn run_embedded(app: Arc<App>, rx: Receiver<WinMsg>, opts: Options, wake_src: Arc<crate::Wake>, embed: Arc<Embed>) -> Result<()> {
    run_in(app, rx, opts, wake_src, Some(embed))
}

fn run_in(app: Arc<App>, rx: Receiver<WinMsg>, opts: Options, wake_src: Arc<crate::Wake>, embed: Option<Arc<Embed>>) -> Result<()> {
    let conn = match &embed {
        None => Connection::connect_to_env().context("connecting to $WAYLAND_DISPLAY")?,
        Some(e) => {
            let backend = unsafe { wayland_backend::client::Backend::from_foreign_display(e.display.cast()) };
            Connection::from_backend(backend)
        }
    };
    let (globals, mut queue) = registry_queue_init::<Win>(&conn)?;
    let qh = queue.handle();
    let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).context("wl_compositor")?;
    let wm: Option<XdgWmBase> = if embed.is_none() { Some(globals.bind(&qh, 1..=6, ()).context("xdg_wm_base")?) } else { None };
    let subcompositor: Option<WlSubcompositor> = if embed.is_some() { Some(globals.bind(&qh, 1..=1, ()).context("wl_subcompositor")?) } else { None };
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).context("wl_shm")?;
    let _seat: WlSeat = globals.bind(&qh, 5..=8, ()).context("wl_seat")?;
    let presentation: Option<WpPresentation> = globals.bind(&qh, 1..=1, ()).ok();
    let rel_mgr = globals.bind(&qh, 1..=1, ()).ok();
    let constraints = globals.bind(&qh, 1..=1, ()).ok();
    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    let ime_mgr: Option<ZwpTextInputManagerV3> = globals.bind(&qh, 1..=1, ()).ok();
    if ime_mgr.is_none() {
        eprintln!("remote-client: the compositor has no text-input-v3; input methods can't type to the host");
    }
    if presentation.is_none() {
        eprintln!("remote-client: the compositor has no wp_presentation; no present stamps");
    }
    // Every output, for the display's mode, refresh and scale (their events
    // arrive before the first configure).
    let output_globals: Vec<(u32, u32)> = globals
        .contents()
        .with_list(|l| l.iter().filter(|g| g.interface == "wl_output").map(|g| (g.name, g.version)).collect());
    let _outputs: Vec<WlOutput> =
        output_globals.iter().map(|&(name, v)| globals.registry().bind::<WlOutput, u32, Win>(name, v.min(4), &qh, name)).collect();
    let surface = compositor.create_surface(&qh, ());
    let (xdg_surface, toplevel, parent, sub) = match (&wm, &subcompositor, &embed) {
        (Some(wm), _, _) => {
            let xdg_surface = wm.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg_surface.get_toplevel(&qh, ());
            toplevel.set_title(opts.title.clone());
            toplevel.set_app_id("remote-client".into());
            if opts.fullscreen {
                toplevel.set_fullscreen(None);
            }
            (Some(xdg_surface), Some(toplevel), None, None)
        }
        (None, Some(sc), Some(e)) => {
            use wayland_backend::client::ObjectId;
            let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), e.parent.cast()) }.map_err(|e| anyhow!("Qt's wl_surface: {e}"))?;
            let parent = WlSurface::from_id(&conn, id).map_err(|e| anyhow!("Qt's wl_surface: {e}"))?;
            let sub = sc.get_subsurface(&surface, &parent, &qh, ());
            sub.set_position(0, 0);
            sub.place_below(&parent);
            sub.set_desync();
            (None, None, Some(parent), Some(sub))
        }
        _ => unreachable!(),
    };
    surface.commit();
    let cursor_surface = compositor.create_surface(&qh, ());
    let cursor_viewport = viewporter.as_ref().map(|v| v.get_viewport(&cursor_surface, &qh, ()));
    let hud = if opts.hud { hud::Hud::load(15.0) } else { None };
    if opts.hud && hud.is_none() {
        eprintln!("remote-client: no monospace font from fc-match; HUD off");
    }

    let mut win = Win {
        app: app.clone(),
        opts,
        qh: qh.clone(),
        compositor,
        surface,
        _xdg_surface: xdg_surface,
        _toplevel: toplevel,
        emb_gen: 0,
        was_capturing: true,
        origin: embed.as_ref().map_or((0, 0), |e| e.state.lock().unwrap().origin),
        over_hud: false,
        last_pos: (0.0, 0.0),
        parent,
        _sub: sub,
        shm,
        presentation,
        rel_mgr,
        constraints,
        _viewporter: viewporter,
        outputs: HashMap::new(),
        on_output: None,
        fullscreen: false,
        display: None,
        lead: LeadStats::default(),
        lead_gen: 0,
        next_vsync: Instant::now() + Duration::from_secs(1),
        vsync_reports: 0,
        pointer: None,
        keyboard: None,
        ime_mgr,
        ime: None,
        relative: None,
        locked: None,
        confined: None,
        cursor_surface,
        cursor_viewport,
        gfx: None,
        configured: embed.is_some(),
        size: embed.as_ref().map_or((1280, 832), |e| e.state.lock().unwrap().size),
        pending_size: None,
        scale: embed.as_ref().map_or(1, |e| e.state.lock().unwrap().scale.max(1)),
        embed: embed.clone(),
        closed: false,
        frame_pending: None,
        dirty: true,
        clock_id: 1,
        pointer_serial: 0,
        pointer_in: false,
        axis: AxisAcc::default(),
        keys_down: HashSet::new(),
        kb_focus: false,
        shapes: HashMap::new(),
        shape_bufs: HashMap::new(),
        cursor_applied: None,
        remote: (0.0, 0.0, false, 0),
        lock_state: msg::PointerLock::None,
        stream: None,
        hud,
        next_hud: Instant::now(),
    };
    while !win.configured {
        queue.blocking_dispatch(&mut win)?;
    }
    let (bw, bh) = win.buffer_size();
    win.gfx = Some(init_gfx(&conn, &win.surface, bw, bh)?);
    if let Some(e) = &win.embed {
        let st = e.state.lock().unwrap();
        win.fullscreen = st.fullscreen;
        win.emb_gen = st.rev;
    }
    if let Some(sub) = &win._sub {
        sub.set_position(win.origin.0, win.origin.1);
    }
    win.resize();

    let wake = wake_src.fd();
    while !win.closed && !app.quitting() {
        while let Ok(m) = rx.try_recv() {
            win.handle(m);
        }
        win.embed_sync();
        if win.opts.hud && Instant::now() >= win.next_hud {
            win.dirty = true;
        }
        let fresh = app.latest.has();
        if win.can_draw(fresh) && (win.dirty || fresh) {
            win.draw()?;
        }
        queue.flush()?;
        let Some(guard) = queue.prepare_read() else {
            queue.dispatch_pending(&mut win)?;
            continue;
        };
        // Qt reads the same socket: a read there queues our events with no wake-up here, so wake often.
        let timeout = if win.embed.is_some() { 4 } else if win.opts.hud { win.next_hud.saturating_duration_since(Instant::now()).as_millis() as i32 } else { 250 };
        let mut fds = [
            libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: wake, events: libc::POLLIN, revents: 0 },
        ];
        unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout.clamp(1, 250)) };
        if fds[0].revents & libc::POLLIN != 0 {
            if let Err(e) = guard.read() {
                if !matches!(&e, wayland_client::backend::WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock) {
                    return Err(e.into());
                }
            }
        } else {
            drop(guard);
        }
        if fds[1].revents & libc::POLLIN != 0 {
            wake_src.drain();
        }
        queue.dispatch_pending(&mut win)?;
    }
    win.release_keys();
    // Embedded, the connection is Qt's and outlives us: an enabled text input left behind would keep the
    // input method's grab on Qt's window and take its commits.
    if let Some(i) = win.ime.take() {
        i.ti.destroy();
        let _ = queue.flush();
    }
    if let Some(e) = &win.embed {
        (e.events)(EmbedEvent::Closed);
    }
    if win.closed {
        app.quit();
    }
    Ok(())
}

// ---- Wayland events ---------------------------------------------------------

impl Dispatch<WlRegistry, GlobalListContents> for Win {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgWmBase, ()> for Win {
    fn event(_: &mut Self, wm: &XdgWmBase, ev: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = ev {
            wm.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for Win {
    fn event(w: &mut Self, s: &XdgSurface, ev: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = ev {
            s.ack_configure(serial);
            if let Some(sz) = w.pending_size.take() {
                if sz != w.size {
                    w.size = sz;
                    if w.gfx.is_some() {
                        w.resize();
                    }
                }
            }
            if w.gfx.is_some() {
                // Full screen on or off at the same size, say.
                w.update_display();
            }
            w.configured = true;
            w.dirty = true;
        }
    }
}

impl Dispatch<XdgToplevel, ()> for Win {
    fn event(w: &mut Self, _: &XdgToplevel, ev: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            xdg_toplevel::Event::Configure { width, height, states } => {
                if width > 0 && height > 0 {
                    w.pending_size = Some((width, height));
                }
                let fullscreen = xdg_toplevel::State::Fullscreen as u32;
                w.fullscreen =
                    states.chunks_exact(4).any(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]) == fullscreen);
            }
            xdg_toplevel::Event::Close => w.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<WlSurface, ()> for Win {
    fn event(w: &mut Self, s: &WlSurface, ev: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if *s != w.surface {
            return;
        }
        match ev {
            wl_surface::Event::PreferredBufferScale { factor } => {
                if factor != w.scale && factor > 0 {
                    w.scale = factor;
                    if w.gfx.is_some() {
                        w.resize();
                    }
                }
            }
            wl_surface::Event::Enter { output } => {
                w.on_output = output.data::<u32>().copied();
                if w.gfx.is_some() {
                    w.update_display();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for Win {
    fn event(w: &mut Self, _: &WlOutput, ev: wl_output::Event, name: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let o = w.outputs.entry(*name).or_insert(Output { mode: None, scale: 1 });
        match ev {
            wl_output::Event::Mode { flags: WEnum::Value(f), width, height, refresh } if f.contains(wl_output::Mode::Current) => {
                o.mode = Some((width.max(0) as u32, height.max(0) as u32, refresh.max(0) as u32));
            }
            wl_output::Event::Scale { factor } => o.scale = factor.max(1),
            wl_output::Event::Done if w.gfx.is_some() => w.update_display(),
            _ => {}
        }
    }
}

impl Dispatch<WlCallback, ()> for Win {
    fn event(w: &mut Self, _: &WlCallback, ev: wl_callback::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { .. } = ev {
            w.frame_pending = None;
        }
    }
}

impl Dispatch<WpPresentation, ()> for Win {
    fn event(w: &mut Self, _: &WpPresentation, ev: wp_presentation::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_presentation::Event::ClockId { clk_id } = ev {
            w.clock_id = clk_id;
            if clk_id != libc::CLOCK_MONOTONIC as u32 {
                eprintln!("remote-client: presentation clock {clk_id} is not CLOCK_MONOTONIC; present stamps are off");
            }
        }
    }
}

impl Dispatch<WpPresentationFeedback, FeedbackData> for Win {
    fn event(
        w: &mut Self,
        _: &WpPresentationFeedback,
        ev: wp_presentation_feedback::Event,
        d: &FeedbackData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match ev {
            wp_presentation_feedback::Event::Presented { tv_sec_hi, tv_sec_lo, tv_nsec, refresh, .. } => {
                let ns = (((tv_sec_hi as u64) << 32) | tv_sec_lo as u64) * 1_000_000_000 + tv_nsec as u64;
                let us = w.app.clock.mono_to_us(ns);
                if let (Some((decode_us, present_us, ready_us)), Some(frame_id)) =
                    (w.app.stats.on_present(d.frame_id, us, refresh), d.frame_id)
                {
                    // Core's per-frame receiver report, for the host's stats.
                    w.app.client.send_receiver_report(frame_id, decode_us, present_us);
                    w.on_lead(ready_us, us, refresh);
                }
                if let (Some(c), Some(id)) = (&w.app.click, d.frame_id) {
                    c.on_present(id, us);
                }
            }
            wp_presentation_feedback::Event::Discarded => w.app.stats.on_discard(d.frame_id),
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for Win {
    fn event(w: &mut Self, seat: &WlSeat, ev: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = ev {
            if c.contains(wl_seat::Capability::Pointer) && w.pointer.is_none() {
                w.pointer = Some(seat.get_pointer(qh, ()));
            }
            if c.contains(wl_seat::Capability::Keyboard) && w.keyboard.is_none() {
                w.keyboard = Some(seat.get_keyboard(qh, ()));
                if let Some(m) = &w.ime_mgr {
                    w.ime = Some(Ime { ti: m.get_text_input(seat, qh, ()), focused: false, enabled: false, committed_enabled: false, pending: None });
                }
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for Win {
    fn event(w: &mut Self, _: &WlPointer, ev: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use wl_pointer::Event as E;
        match ev {
            E::Enter { serial, surface, surface_x, surface_y } => {
                if !w.is_focus(&surface) {
                    return;
                }
                w.pointer_serial = serial;
                w.pointer_in = true;
                w.cursor_applied = None;
                w.last_pos = (surface_x, surface_y);
                w.over_hud = w.in_hud(surface_x, surface_y);
                w.apply_lock();
                if !w.over_hud {
                    if let Some((x, y)) = w.to_stream(surface_x, surface_y) {
                        w.send(Input::PointerAbs { x, y });
                    }
                }
                w.dirty = true;
            }
            E::Leave { .. } => {
                w.pointer_in = false;
                w.over_hud = false;
                w.apply_lock();
                w.dirty = true;
            }
            E::Motion { time, surface_x, surface_y } => {
                w.last_pos = (surface_x, surface_y);
                if w.embed.is_some() {
                    let over = w.in_hud(surface_x, surface_y);
                    if over != w.over_hud {
                        w.over_hud = over;
                        w.apply_lock();
                    }
                    if w.over_hud {
                        return;
                    }
                    // Qt sets its own cursor when the pointer enters; ours goes on top.
                    if w.cursor_applied.is_none() {
                        w.apply_cursor();
                    }
                }
                if w.locked.is_none() {
                    if let Some((x, y)) = w.to_stream(surface_x, surface_y) {
                        w.send_at(Input::PointerAbs { x, y }, time);
                    }
                }
            }
            E::Button { time, button, state, .. } => {
                if w.over_hud {
                    return;
                }
                let down = matches!(state, WEnum::Value(wl_pointer::ButtonState::Pressed));
                if down && !w.capturing() {
                    // A click in the video takes the keyboard back.
                    w.set_capture(true);
                }
                if down {
                    w.ime_cursor();
                }
                w.send_at(Input::Button { button: button_from_evdev(button), down }, time);
            }
            E::Axis { .. } | E::AxisSource { .. } | E::AxisStop { .. } | E::AxisValue120 { .. } | E::AxisDiscrete { .. } if w.over_hud => {}
            E::Axis { time, axis: WEnum::Value(a), value } => {
                let i = (a == wl_pointer::Axis::HorizontalScroll) as usize;
                w.axis.time.get_or_insert(time);
                w.axis.value[i] += value;
                w.axis.any[i] = true;
            }
            E::AxisSource { axis_source: WEnum::Value(s) } => w.axis.source = Some(s as u32),
            E::AxisStop { time, axis: WEnum::Value(a) } => {
                let i = (a == wl_pointer::Axis::HorizontalScroll) as usize;
                w.axis.time.get_or_insert(time);
                w.axis.stop[i] = true;
                w.axis.any[i] = true;
            }
            E::AxisValue120 { axis: WEnum::Value(a), value120 } => {
                let i = (a == wl_pointer::Axis::HorizontalScroll) as usize;
                w.axis.value120[i] += value120;
                w.axis.any[i] = true;
            }
            E::AxisDiscrete { axis: WEnum::Value(a), discrete } => {
                let i = (a == wl_pointer::Axis::HorizontalScroll) as usize;
                w.axis.value120[i] += discrete * 120;
                w.axis.any[i] = true;
            }
            E::Frame if w.over_hud => {
                w.axis = AxisAcc::default();
            }
            E::Frame => {
                let acc = std::mem::take(&mut w.axis);
                let at = acc.time.and_then(wayland_time);
                // Both axes in one frame: the first says another follows.
                let both = acc.any[0] && acc.any[1];
                for i in 0..2 {
                    if acc.any[i] {
                        let ev = Input::Axis {
                            source: match acc.source.unwrap_or(0) {
                                1 => msg::AxisSource::Finger,
                                2 => msg::AxisSource::Continuous,
                                3 => msg::AxisSource::WheelTilt,
                                _ => msg::AxisSource::Wheel,
                            },
                            axis: if i == 1 { msg::Axis::Horizontal } else { msg::Axis::Vertical },
                            value: acc.value[i],
                            value120: acc.value120[i],
                            stop: acc.stop[i],
                            more: both && i == 0,
                        };
                        w.app.send_input_at(ev, at);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpRelativePointerV1, ()> for Win {
    fn event(w: &mut Self, _: &ZwpRelativePointerV1, ev: zwp_relative_pointer_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let zwp_relative_pointer_v1::Event::RelativeMotion { utime_hi, utime_lo, dx, dy, dx_unaccel, dy_unaccel } = ev {
            let k = 1.0 / w.video_scale();
            let at = crate::connect::wayland_time_us(((utime_hi as u64) << 32) | utime_lo as u64);
            w.app.send_input_at(Input::PointerRel { dx: dx * k, dy: dy * k, dx_unaccel, dy_unaccel }, at);
        }
    }
}

impl Dispatch<WlKeyboard, ()> for Win {
    fn event(w: &mut Self, _: &WlKeyboard, ev: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            wl_keyboard::Event::Enter { surface, .. } => w.kb_focus = w.is_focus(&surface),
            wl_keyboard::Event::Key { time, key, state, .. } => {
                // Keys typed into the app's own windows (its computer list, its fields) never go to the host.
                if !w.kb_focus {
                    return;
                }
                let down = matches!(state, WEnum::Value(wl_keyboard::KeyState::Pressed));
                if w.embed.is_some() {
                    if !w.capturing() {
                        return;
                    }
                    // The local leave chord, Ctrl+Alt+Shift+Esc, by scan code; the host never sees it.
                    if down && key == KEY_ESC && w.chord_held() {
                        w.release_keys();
                        w.set_capture(false);
                        if let Some(e) = &w.embed {
                            (e.events)(EmbedEvent::LeaveChord);
                        }
                        return;
                    }
                }
                if down {
                    w.keys_down.insert(key);
                } else if !w.keys_down.remove(&key) {
                    return; // pressed before we had focus
                }
                // Keys exactly as pressed; the host applies the key table.
                if let Some(usage) = hid::evdev_to_hid(key) {
                    w.send_at(Input::Key { usage, down }, time);
                }
            }
            wl_keyboard::Event::Leave { .. } => {
                w.kb_focus = false;
                w.release_keys();
            }
            _ => {}
        }
    }
}

impl Drop for Win {
    /// The error paths too: the destroy goes out with Qt's next flush when embedded.
    fn drop(&mut self) {
        if let Some(i) = self.ime.take() {
            i.ti.destroy();
        }
    }
}

impl Dispatch<ZwpTextInputV3, ()> for Win {
    fn event(w: &mut Self, _: &ZwpTextInputV3, ev: zwp_text_input_v3::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use zwp_text_input_v3::Event as E;
        match ev {
            E::Enter { surface } => {
                if log_ime() {
                    eprintln!("remote-client: ime: enter {:?} (ours: {}, capturing: {})", surface.id(), w.is_focus(&surface), w.capturing());
                }
                if w.is_focus(&surface)
                    && let Some(i) = w.ime.as_mut()
                {
                    i.focused = true;
                    w.update_ime();
                }
            }
            E::Leave { .. } => {
                if log_ime() {
                    eprintln!("remote-client: ime: leave");
                }
                // The compositor ignores this object until the next enter: nothing to send.
                if let Some(i) = w.ime.as_mut() {
                    i.focused = false;
                    i.enabled = false;
                    i.pending = None;
                }
            }
            E::CommitString { text } => {
                if log_ime() {
                    eprintln!("remote-client: ime: commit_string of {} chars", text.as_deref().map_or(0, |t| t.chars().count()));
                }
                if let Some(i) = w.ime.as_mut() {
                    i.pending = text;
                }
            }
            E::Done { .. } => {
                let text = w.ime.as_mut().and_then(|i| i.pending.take());
                if let Some(t) = text.filter(|t| !t.is_empty())
                    && w.capturing()
                {
                    for e in Input::commit_chunks(&t) {
                        w.send(e);
                    }
                }
            }
            // The preedit stays in the input method's own window; surrounding text isn't known here.
            other => {
                if log_ime() {
                    eprintln!("remote-client: ime: {}", format!("{other:?}").split([' ', '{']).next().unwrap_or(""));
                }
            }
        }
    }
}

delegate_noop!(Win: ignore ZwpTextInputManagerV3);
delegate_noop!(Win: ignore WlRegion);
delegate_noop!(Win: ignore WlSubcompositor);
delegate_noop!(Win: ignore WlSubsurface);
delegate_noop!(Win: ignore WlCompositor);
delegate_noop!(Win: ignore WlShm);
delegate_noop!(Win: ignore WlShmPool);
delegate_noop!(Win: ignore WlBuffer);
delegate_noop!(Win: ignore ZwpRelativePointerManagerV1);
delegate_noop!(Win: ignore ZwpPointerConstraintsV1);
delegate_noop!(Win: ignore ZwpLockedPointerV1);
delegate_noop!(Win: ignore ZwpConfinedPointerV1);
delegate_noop!(Win: ignore WpViewporter);
delegate_noop!(Win: ignore WpViewport);
