// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! test-surface: the host side of the click-to-photon test. A
//! full-screen Wayland surface whose marker flips black ↔ white on every button
//! press. Two buffers are painted up front, so a flip is one attach, a damage
//! of the marker and a commit, stamped and logged with its presentation time.
//! Keys and scrolls are logged too, as an input check.
//!
//!   test-surface [--marker X,Y,W,H] [--log FILE] [--secs N]
//!
//! Log (CSV, CLOCK_MONOTONIC ns): kind,id,t_event_ns,t_commit_ns,t_present_ns,detail

use anyhow::{Context, Result};
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard},
        wl_pointer::{self, WlPointer},
        wl_registry::WlRegistry,
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::wp::presentation_time::client::{
    wp_presentation::WpPresentation,
    wp_presentation_feedback::{self, WpPresentationFeedback},
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

fn mono_ns() -> u64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

struct S {
    qh: QueueHandle<S>,
    surface: WlSurface,
    presentation: Option<WpPresentation>,
    size: (i32, i32),
    configured: bool,
    closed: bool,
    bufs: Vec<WlBuffer>,
    marker: (i32, i32, i32, i32),
    white: bool,
    flips: u32,
    log: std::io::BufWriter<std::fs::File>,
}

impl S {
    fn flip(&mut self, t_event: u64, detail: &str) {
        if self.bufs.len() < 2 {
            return;
        }
        self.white = !self.white;
        self.flips += 1;
        let (x, y, w, h) = self.marker;
        self.surface.attach(Some(&self.bufs[self.white as usize]), 0, 0);
        self.surface.damage_buffer(x, y, w, h);
        if let Some(p) = &self.presentation {
            p.feedback(&self.surface, &self.qh, self.flips);
        }
        self.surface.commit();
        let t_commit = mono_ns();
        let _ = writeln!(self.log, "press,{},{t_event},{t_commit},,{detail} white={}", self.flips, self.white as u8);
        let _ = self.log.flush();
    }
}

/// Paints both buffers: a dark desktop-ish background with a grid, the marker
/// black in buffer 0 and white in buffer 1.
fn make_buffers(s: &S, shm: &WlShm, qh: &QueueHandle<S>) -> Result<Vec<WlBuffer>> {
    let (w, h) = s.size;
    let stride = w * 4;
    let one = (stride * h) as usize;
    let fd = unsafe { libc::memfd_create(c"test-surface".as_ptr(), libc::MFD_CLOEXEC) };
    anyhow::ensure!(fd >= 0, "memfd_create");
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    anyhow::ensure!(unsafe { libc::ftruncate(fd.as_raw_fd(), (2 * one) as libc::off_t) } == 0, "ftruncate");
    let map = unsafe { libc::mmap(std::ptr::null_mut(), 2 * one, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
    anyhow::ensure!(map != libc::MAP_FAILED, "mmap");
    let px = unsafe { std::slice::from_raw_parts_mut(map as *mut u32, 2 * one / 4) };
    let (mx, my, mw, mh) = s.marker;
    for b in 0..2usize {
        for y in 0..h {
            for x in 0..w {
                let grid = x % 64 == 0 || y % 64 == 0;
                let in_marker = x >= mx && x < mx + mw && y >= my && y < my + mh;
                let v: u32 = if in_marker {
                    if b == 1 { 0xFFFFFF } else { 0x000000 }
                } else if grid {
                    0x3A3F4A
                } else {
                    0x22252B
                };
                px[b * one / 4 + (y * w + x) as usize] = 0xFF00_0000 | v;
            }
        }
    }
    unsafe { libc::munmap(map, 2 * one) };
    let pool: WlShmPool = shm.create_pool(fd.as_fd(), (2 * one) as i32, qh, ());
    let bufs = (0..2).map(|b| pool.create_buffer((b * one) as i32, w, h, stride, wl_shm::Format::Xrgb8888, qh, ())).collect();
    pool.destroy();
    Ok(bufs)
}

fn main() -> Result<()> {
    let mut marker = (64, 64, 256, 256);
    let mut log_path = String::from("test-surface.csv");
    let mut secs: Option<u64> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let v = it.next().with_context(|| format!("{a} needs a value"))?;
        match a.as_str() {
            "--marker" => {
                let n: Vec<i32> = v.split(',').map(|p| p.parse().unwrap_or(0)).collect();
                anyhow::ensure!(n.len() == 4, "--marker X,Y,W,H");
                marker = (n[0], n[1], n[2], n[3]);
            }
            "--log" => log_path = v,
            "--secs" => secs = Some(v.parse()?),
            _ => anyhow::bail!("unknown option {a}"),
        }
    }
    let conn = Connection::connect_to_env()?;
    let (globals, mut q) = registry_queue_init::<S>(&conn)?;
    let qh = q.handle();
    let compositor: WlCompositor = globals.bind(&qh, 4..=5, ())?;
    let shm: WlShm = globals.bind(&qh, 1..=1, ())?;
    let wm: XdgWmBase = globals.bind(&qh, 1..=5, ())?;
    let _seat: WlSeat = globals.bind(&qh, 5..=8, ())?;
    let presentation: Option<WpPresentation> = globals.bind(&qh, 1..=1, ()).ok();
    let surface = compositor.create_surface(&qh, ());
    let xs = wm.get_xdg_surface(&surface, &qh, ());
    let top = xs.get_toplevel(&qh, ());
    top.set_app_id("remote-test-surface".into());
    top.set_title("Remote test surface".into());
    top.set_fullscreen(None);
    surface.commit();
    let mut log = std::io::BufWriter::new(std::fs::File::create(&log_path)?);
    writeln!(log, "kind,id,t_event_ns,t_commit_ns,t_present_ns,detail")?;
    let mut s = S {
        qh: qh.clone(),
        surface,
        presentation,
        size: (0, 0),
        configured: false,
        closed: false,
        bufs: Vec::new(),
        marker,
        white: false,
        flips: 0,
        log,
    };
    while !(s.configured && s.size.0 > 0) {
        q.blocking_dispatch(&mut s)?;
    }
    s.bufs = make_buffers(&s, &shm, &qh)?;
    s.surface.attach(Some(&s.bufs[0]), 0, 0);
    s.surface.damage_buffer(0, 0, s.size.0, s.size.1);
    s.surface.commit();
    eprintln!("test-surface: {}x{}, marker {:?}", s.size.0, s.size.1, s.marker);
    let start = Instant::now();
    while !s.closed && secs.is_none_or(|n| start.elapsed() < Duration::from_secs(n)) {
        q.flush()?;
        if let Some(g) = q.prepare_read() {
            let mut p = libc::pollfd { fd: g.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
            unsafe { libc::poll(&mut p, 1, 200) };
            if p.revents & libc::POLLIN != 0 {
                let _ = g.read();
            }
        }
        q.dispatch_pending(&mut s)?;
    }
    Ok(())
}

impl Dispatch<WlRegistry, GlobalListContents> for S {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as wayland_client::Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<XdgWmBase, ()> for S {
    fn event(_: &mut Self, wm: &XdgWmBase, ev: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = ev {
            wm.pong(serial);
        }
    }
}
impl Dispatch<XdgSurface, ()> for S {
    fn event(s: &mut Self, x: &XdgSurface, ev: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = ev {
            x.ack_configure(serial);
            s.configured = true;
        }
    }
}
impl Dispatch<XdgToplevel, ()> for S {
    fn event(s: &mut Self, _: &XdgToplevel, ev: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            xdg_toplevel::Event::Configure { width, height, .. } if width > 0 && height > 0 && s.bufs.is_empty() => {
                s.size = (width, height)
            }
            xdg_toplevel::Event::Close => s.closed = true,
            _ => {}
        }
    }
}
impl Dispatch<WlSeat, ()> for S {
    fn event(_: &mut Self, seat: &WlSeat, ev: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = ev {
            if c.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(qh, ());
            }
            if c.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
            }
        }
    }
}
impl Dispatch<WlPointer, ()> for S {
    fn event(s: &mut Self, _: &WlPointer, ev: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let t = mono_ns();
        match ev {
            wl_pointer::Event::Button { button, state: WEnum::Value(wl_pointer::ButtonState::Pressed), time, .. } => {
                s.flip(t, &format!("button={button:#x} time_ms={time}"));
            }
            wl_pointer::Event::AxisSource { axis_source } => {
                let _ = writeln!(s.log, "axis_source,,{t},,,{axis_source:?}");
                let _ = s.log.flush();
            }
            wl_pointer::Event::Axis { axis, value, .. } => {
                let _ = writeln!(s.log, "axis,,{t},,,{axis:?} {value}");
                let _ = s.log.flush();
            }
            wl_pointer::Event::AxisValue120 { axis, value120 } => {
                let _ = writeln!(s.log, "axis120,,{t},,,{axis:?} {value120}");
                let _ = s.log.flush();
            }
            wl_pointer::Event::AxisStop { axis, .. } => {
                let _ = writeln!(s.log, "axis_stop,,{t},,,{axis:?}");
                let _ = s.log.flush();
            }
            _ => {}
        }
    }
}
impl Dispatch<WlKeyboard, ()> for S {
    fn event(s: &mut Self, _: &WlKeyboard, ev: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_keyboard::Event::Key { key, state, .. } = ev {
            let _ = writeln!(s.log, "key,,{},,,code={key} {state:?}", mono_ns());
            let _ = s.log.flush();
        }
    }
}
impl Dispatch<WpPresentationFeedback, u32> for S {
    fn event(s: &mut Self, _: &WpPresentationFeedback, ev: wp_presentation_feedback::Event, id: &u32, _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            wp_presentation_feedback::Event::Presented { tv_sec_hi, tv_sec_lo, tv_nsec, .. } => {
                let ns = (((tv_sec_hi as u64) << 32) | tv_sec_lo as u64) * 1_000_000_000 + tv_nsec as u64;
                let _ = writeln!(s.log, "present,{id},,,{ns},");
            }
            wp_presentation_feedback::Event::Discarded => {
                let _ = writeln!(s.log, "discarded,{id},,,,");
            }
            _ => {}
        }
        let _ = s.log.flush();
    }
}
delegate_noop!(S: ignore WlCompositor);
delegate_noop!(S: ignore WlSurface);
delegate_noop!(S: ignore WlShm);
delegate_noop!(S: ignore WlShmPool);
delegate_noop!(S: ignore WlBuffer);
delegate_noop!(S: ignore WpPresentation);
