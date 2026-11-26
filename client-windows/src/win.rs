// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The stream window: a Win32 window drawn by `d3d::Gfx`.
//!
//! - **Present:** latest frame wins. The loop waits on the swapchain's waitable
//!   object (one frame of latency), then takes the newest picture, uploads it,
//!   draws and presents; a frame replaced before that is never shown (counted
//!   as skipped). Present times come from DXGI frame statistics (`QPC` →
//!   client µs) and feed the receiver report and `Msg::Vsync`. A picture still
//!   in its decoder texture is drawn from there (zero copy, `d3d`).
//! - **Present path timing:** per shown frame, the time to hand the picture to
//!   the GPU (upload or view), ready → drawn, drawn → on screen and last packet →
//!   on screen; p50/p95 on stderr every 5 s and on the HUD, with the path
//!   ("zero-copy" or "copy"), so the two can be compared on the same machine.
//! - **Size:** `ClientDisplay` is the client area in pixels (per-monitor DPI
//!   aware, so real pixels), the monitor's refresh and size, `pixels_per_point`
//!   from the DPI (96 → 1.0). Every change sends `Msg::Resize` (the host
//!   debounces) unless the resolution is fixed or the host has no live resize.
//!   Placement is `remote_engine::layout` (1:1 within 64 px, else fitted).
//! - **Cursor:** the host's shape (`CursorImage`) becomes an HCURSOR scaled like
//!   the video; hidden under pointer lock.
//! - **Input:** absolute pointer in stream pixels; Raw Input for relative
//!   motion under `PointerLock::Locked` (the cursor is clipped to one pixel);
//!   buttons incl. X1/X2; wheel and hi-res wheel (`WHEEL_DELTA` 120 →
//!   `value120`), horizontal wheel; keys as HID usages from scan codes.
//!   While the window is the foreground window a `WH_KEYBOARD_LL` hook takes
//!   every key (Win, Alt+Tab, Alt+Esc included) and sends it to the host.
//!   Held keys and buttons are released on focus loss.
//!
//! PROVISIONAL: the chrome, the HUD and the secure-screen notice are plain
//! (GDI text on a dark panel); the designed ones come with the Qt app.

use crate::d3d::{Gfx, Overlay};
use crate::embed::{Embed, EmbedEvent};
use crate::scancode::scancode_to_hid;
use anyhow::Result;
use remote_core::phase::LeadStats;
use remote_engine::layout::{self, Placement};
use remote_engine::msg::{self, Axis, AxisSource, Button, ClientDisplay, CursorImage, InputEvent as Input, Msg};
use remote_engine::proto::StreamInfo;
use remote_engine::{App, WinMsg, WindowOptions};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Input::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const WM_MOUSELEAVE: u32 = 0x02A3;

static WIN: AtomicPtr<Win> = AtomicPtr::new(std::ptr::null_mut());

struct Pending {
    present_id: u32,
    frame_id: u32,
    at: Instant,
    /// When `draw` presented it (client µs).
    commit_us: i64,
    /// `Gfx::show`'s time for it: the upload, or making the decoder texture's views.
    show_ms: f64,
    /// `Gfx::path` when it was drawn.
    path: &'static str,
}

/// The present path's timing over the last few seconds (see the module docs).
struct PathTimes {
    show: Vec<f64>,
    ready_drawn: Vec<f64>,
    drawn_shown: Vec<f64>,
    received_shown: Vec<f64>,
    path: &'static str,
    since: Instant,
    line: Option<String>,
}

impl PathTimes {
    const EVERY: Duration = Duration::from_secs(5);

    fn new() -> PathTimes {
        PathTimes { show: vec![], ready_drawn: vec![], drawn_shown: vec![], received_shown: vec![], path: "", since: Instant::now(), line: None }
    }

    /// One shown frame: ms values.
    fn add(&mut self, path: &'static str, show: f64, ready_drawn: f64, drawn_shown: f64, received_shown: f64) {
        if path != self.path && !self.show.is_empty() {
            self.flush();
        }
        self.path = path;
        self.show.push(show);
        self.ready_drawn.push(ready_drawn);
        self.drawn_shown.push(drawn_shown);
        self.received_shown.push(received_shown);
        if self.since.elapsed() >= Self::EVERY {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let n = self.show.len();
        if n > 0 {
            let pp = |v: &mut Vec<f64>| {
                let r = format!("{:.2}/{:.2}", remote_engine::stats::percentile(v, 0.5), remote_engine::stats::percentile(v, 0.95));
                v.clear();
                r
            };
            let line = format!(
                "present path {}: to GPU {} ms, ready→drawn {} ms, drawn→shown {} ms, last packet→shown {} ms (p50/p95, {n} frames)",
                self.path,
                pp(&mut self.show),
                pp(&mut self.ready_drawn),
                pp(&mut self.drawn_shown),
                pp(&mut self.received_shown)
            );
            eprintln!("remote-client: {line}");
            self.line = Some(line);
        }
        self.since = Instant::now();
    }
}

struct Win {
    app: Arc<App>,
    opts: WindowOptions,
    hwnd: HWND,
    gfx: Gfx,
    stream: Option<StreamInfo>,
    bt601: bool,
    full_range: bool,
    display: Option<ClientDisplay>,
    keys_down: HashSet<u16>,
    buttons: u32,
    lock: msg::PointerLock,
    focused: bool,
    mouse_in: bool,
    hook: Option<HHOOK>,
    embed: Option<Arc<Embed>>,
    embed_rev: u64,
    /// The keyboard goes to the host (embedded: the leave chord turns it off).
    capture: bool,
    /// An app menu is open over the picture: only key-ups and button-ups go to the host (embedded).
    blocked: bool,
    /// The top-level window that must be foreground for the keyboard hook.
    root: HWND,
    hinst: HINSTANCE,
    shapes: HashMap<u64, CursorImage>,
    cursors: HashMap<(u64, i32), isize>,
    remote: (f32, f32, bool, u64),
    /// The cursor to show over the client area (0: none).
    cursor: isize,
    hud: Option<Overlay>,
    next_hud: Instant,
    times: PathTimes,
    notice: Option<Overlay>,
    secure: bool,
    pending: VecDeque<Pending>,
    lead: LeadStats,
    lead_gen: u64,
    next_vsync: Instant,
    vsync_reports: u64,
    qpc_freq: i64,
    dirty: bool,
    closed: bool,
    video_changed: bool,
    logged: (u32, u32, (u32, u32)),
}

fn loword(v: isize) -> i32 {
    (v & 0xFFFF) as u16 as i32
}
fn hiword(v: isize) -> i32 {
    ((v >> 16) & 0xFFFF) as u16 as i32
}
fn sx(v: isize) -> i32 {
    (v & 0xFFFF) as u16 as i16 as i32
}
fn sy(v: isize) -> i32 {
    ((v >> 16) & 0xFFFF) as u16 as i16 as i32
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn run(app: Arc<App>, rx: Receiver<WinMsg>, opts: WindowOptions, wake: HANDLE) -> Result<()> {
    run_with(app, rx, opts, wake, None)
}

/// `embed`: the picture goes on a child window of Qt's frame instead of a window of its own.
pub fn run_with(app: Arc<App>, rx: Receiver<WinMsg>, opts: WindowOptions, wake: HANDLE, embed: Option<Arc<Embed>>) -> Result<()> {
    unsafe {
        let hinst = HINSTANCE(GetModuleHandleW(None)?.0);
        let class = w!("SodiumRemoteStream");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let (style, x, y, cw, ch) = if let Some(e) = &embed {
            let st = e.get();
            let k = GetDpiForWindow(HWND(e.parent as *mut c_void)).max(96) as f64 / 96.0;
            (WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS, (st.origin.0 as f64 * k) as i32, (st.origin.1 as f64 * k) as i32, ((st.size.0 as f64 * k) as i32).max(1), ((st.size.1 as f64 * k) as i32).max(1))
        } else if opts.fullscreen {
            let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            (WS_POPUP | WS_VISIBLE, 0, 0, sw, sh)
        } else {
            let (w0, h0) = (1280, 720); // --size is the stream's size, not the window's
            let mut r = RECT { left: 0, top: 0, right: w0, bottom: h0 };
            let _ = AdjustWindowRect(&mut r, WS_OVERLAPPEDWINDOW, false);
            (WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT, r.right - r.left, r.bottom - r.top)
        };
        let title = wide(&opts.title);
        let parent = embed.as_ref().map(|e| HWND(e.parent as *mut c_void));
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR(title.as_ptr()), style, x, y, cw, ch, parent, None, Some(hinst), None)?;
        let root = if parent.is_some() { GetAncestor(hwnd, GA_ROOT) } else { hwnd };
        let mut cr = RECT::default();
        GetClientRect(hwnd, &mut cr)?;
        let gfx = Gfx::new(hwnd, (cr.right - cr.left) as u32, (cr.bottom - cr.top) as u32)?;
        eprintln!("remote-client: Direct3D 11 {} device, flip-model swapchain {}x{}", if gfx.warp { "WARP (software)" } else { "hardware" }, cr.right - cr.left, cr.bottom - cr.top);
        let mut freq = 0i64;
        let _ = QueryPerformanceFrequency(&mut freq);
        let mut win = Box::new(Win {
            app: app.clone(),
            opts: opts.clone(),
            hwnd,
            gfx,
            stream: None,
            bt601: false,
            full_range: false,
            display: None,
            keys_down: HashSet::new(),
            buttons: 0,
            lock: msg::PointerLock::None,
            focused: false,
            mouse_in: false,
            hook: None,
            embed_rev: embed.as_ref().map_or(0, |e| e.rev()),
            capture: embed.as_ref().is_none_or(|e| e.get().capture),
            blocked: false,
            embed,
            root,
            hinst,
            shapes: HashMap::new(),
            cursors: HashMap::new(),
            remote: (0.0, 0.0, true, 0),
            cursor: LoadCursorW(None, IDC_ARROW)?.0 as isize,
            hud: None,
            next_hud: Instant::now(),
            times: PathTimes::new(),
            notice: None,
            secure: false,
            pending: VecDeque::new(),
            lead: LeadStats::default(),
            lead_gen: 0,
            next_vsync: Instant::now(),
            vsync_reports: 0,
            qpc_freq: freq.max(1),
            dirty: true,
            closed: false,
            video_changed: false,
            logged: (0, 0, (0, 0)),
        });
        WIN.store(&mut *win, Ordering::SeqCst);
        win.update_display();
        let rid = RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: RAWINPUTDEVICE_FLAGS(0), hwndTarget: hwnd };
        let _ = RegisterRawInputDevices(&[rid], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
        if win.embed.is_none() {
            win.hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_keyboard), Some(hinst), 0).ok();
            eprintln!("remote-client: keyboard hook {}", if win.hook.is_some() { "installed" } else { "not available: keys come from the window" });
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            win.focused = GetForegroundWindow() == hwnd;
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
            win.update_hook();
        }

        let result = win.event_loop(&rx, wake);
        win.release_all();
        if let Some(h) = win.hook.take() {
            let _ = UnhookWindowsHookEx(h);
        }
        let _ = ClipCursor(None);
        WIN.store(std::ptr::null_mut(), Ordering::SeqCst);
        if win.closed {
            app.quit();
        }
        let _ = DestroyWindow(hwnd);
        if app.quitting() {
            // fine
        }
        result
    }
}

impl Win {
    fn event_loop(&mut self, rx: &Receiver<WinMsg>, wake: HANDLE) -> Result<()> {
        unsafe {
            let mut m = MSG::default();
            'outer: loop {
                while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
                    if m.message == WM_QUIT {
                        break 'outer;
                    }
                    let _ = DispatchMessageW(&m);
                }
                if self.closed || self.app.quitting() {
                    break;
                }
                self.sync_embed();
                if self.embed.is_some() {
                    self.update_hook();
                }
                while let Ok(msg) = rx.try_recv() {
                    self.handle(msg);
                }
                let fresh = self.app.latest.has();
                let hud_due = self.opts.hud && Instant::now() >= self.next_hud;
                let visible = !IsIconic(self.hwnd).as_bool();
                let want = visible && (fresh || self.dirty || hud_due);
                self.poll_present();
                let handles: Vec<HANDLE> = if want { vec![self.gfx.waitable(), wake] } else { vec![wake] };
                let timeout = if self.pending.is_empty() { 50 } else { 8 };
                let r = MsgWaitForMultipleObjectsEx(Some(&handles), timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                if want && r.0 == WAIT_OBJECT_0.0 {
                    self.draw()?;
                }
            }
        }
        Ok(())
    }

    /// Embedded: follow the video area Qt gives us.
    fn sync_embed(&mut self) {
        let Some(e) = self.embed.clone() else { return };
        let rev = e.rev();
        if rev == self.embed_rev {
            return;
        }
        self.embed_rev = rev;
        let st = e.get();
        unsafe {
            let k = GetDpiForWindow(HWND(e.parent as *mut c_void)).max(96) as f64 / 96.0;
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOP), (st.origin.0 as f64 * k).round() as i32, (st.origin.1 as f64 * k).round() as i32, ((st.size.0 as f64 * k).round() as i32).max(1), ((st.size.1 as f64 * k).round() as i32).max(1), SWP_NOACTIVATE);
        }
        if st.blocked != self.blocked {
            // Held keys and buttons go up on the computer the moment a menu opens.
            if st.blocked {
                self.release_all();
            }
            self.blocked = st.blocked;
        }
        if st.capture != self.capture {
            self.set_capture(st.capture, false);
        }
        self.opts.fullscreen = st.fullscreen;
        self.on_size();
    }

    /// Keyboard capture on or off (embedded): off releases what is held and lets keys go to Qt.
    fn set_capture(&mut self, on: bool, tell: bool) {
        if self.capture == on {
            return;
        }
        self.capture = on;
        if !on {
            self.release_all();
        }
        self.apply_lock();
        if tell && let Some(e) = &self.embed {
            e.update(|s| s.capture = on);
            self.embed_rev = e.rev();
            (e.on_event)(EmbedEvent::Capture(on));
        }
    }

    /// The low-level hook runs only while our top-level window is in the foreground, not minimised, and the
    /// keyboard is captured.
    fn update_hook(&mut self) {
        unsafe {
            let fg = GetForegroundWindow();
            let want = self.capture && fg == self.root && !IsIconic(self.root).as_bool();
            if want && self.hook.is_none() {
                self.hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_keyboard), Some(self.hinst), 0).ok();
                self.focused = true;
                self.apply_lock();
            } else if !want && self.hook.is_some() {
                if let Some(h) = self.hook.take() {
                    let _ = UnhookWindowsHookEx(h);
                }
                self.release_all();
                self.focused = fg == self.root;
                self.apply_lock();
            }
        }
    }

    fn chord_held(&self) -> bool {
        // Ctrl, Alt, Shift (either side) by HID usage: E0/E4, E2/E6, E1/E5
        let h = |a: u16, b: u16| self.keys_down.contains(&a) || self.keys_down.contains(&b);
        h(0xE0, 0xE4) && h(0xE2, 0xE6) && h(0xE1, 0xE5)
    }

    fn client_size(&self) -> (u32, u32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        ((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32)
    }

    fn video_size(&self) -> (u32, u32) {
        self.gfx.video_size().or_else(|| self.stream.map(|s| (s.width, s.height))).unwrap_or((0, 0))
    }

    fn placement(&self) -> Placement {
        layout::place(self.client_size(), self.video_size())
    }

    /// Display pixels per stream pixel.
    fn video_scale(&self) -> f64 {
        let v = self.video_size();
        if v.0 == 0 { 1.0 } else { self.placement().w / v.0 as f64 }
    }

    fn send(&self, ev: Input) {
        if self.blocked && !matches!(ev, Input::Key { down: false, .. } | Input::Button { down: false, .. }) {
            return;
        }
        self.app.send_input(ev);
    }

    fn monitor(&self) -> (u32, u32, u32) {
        unsafe {
            let mon = MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFOEXW::default();
            mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            let (mut w0, mut h0, mut hz) = (0, 0, 60);
            if GetMonitorInfoW(mon, &mut mi as *mut _ as *mut MONITORINFO).as_bool() {
                w0 = (mi.monitorInfo.rcMonitor.right - mi.monitorInfo.rcMonitor.left) as u32;
                h0 = (mi.monitorInfo.rcMonitor.bottom - mi.monitorInfo.rcMonitor.top) as u32;
                let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
                if EnumDisplaySettingsW(PCWSTR(mi.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool() && dm.dmDisplayFrequency > 1 {
                    hz = dm.dmDisplayFrequency;
                }
            }
            (w0, h0, hz * 1000)
        }
    }

    fn client_display(&self) -> ClientDisplay {
        let (cw, ch) = self.client_size();
        let (mw, mh, refresh) = self.monitor();
        let (width, height) = self.opts.size.unwrap_or((cw, ch));
        let dpi = unsafe { GetDpiForWindow(if self.root.0.is_null() { self.hwnd } else { self.root }) }.max(96) as f32;
        ClientDisplay {
            width,
            height,
            pixels_per_point: self.opts.ppp.unwrap_or(dpi / 96.0),
            refresh_mhz: refresh,
            fullscreen: self.opts.fullscreen,
            display_width: if mw > 0 { mw } else { cw },
            display_height: if mh > 0 { mh } else { ch },
        }
    }

    /// The first display goes into `Hello`; then a `Resize` on every change
    /// (none with a fixed or forced size, or a host without live resize).
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
            "remote-client: display {}x{} (monitor {}x{} @{:.3} Hz, ppp {}, fullscreen {}){}",
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

    fn on_size(&mut self) {
        let (w0, h0) = self.client_size();
        if let Err(e) = self.gfx.resize(w0, h0) {
            eprintln!("remote-client: resizing the swapchain: {e:#}");
        }
        self.update_display();
        self.cursors.clear();
        self.apply_cursor();
        self.dirty = true;
    }

    fn handle(&mut self, m: WinMsg) {
        match m {
            WinMsg::Stream(s) => {
                self.stream = Some(s);
                self.cursors.clear();
                self.apply_cursor();
                self.dirty = true;
            }
            WinMsg::CursorShape { hash, image } => {
                self.shapes.insert(hash, image);
                if self.remote.3 == hash {
                    self.apply_cursor();
                }
            }
            WinMsg::CursorPos { x, y, visible, hash } => {
                let changed = self.remote.3 != hash || self.remote.2 != visible;
                self.remote = (x, y, visible, hash);
                if changed {
                    self.apply_cursor();
                }
            }
            WinMsg::PointerLock(l) => {
                self.lock = l;
                self.apply_lock();
                self.apply_cursor();
            }
            WinMsg::SecureScreen(on) => {
                self.secure = on && self.embed.is_none();
                if on && self.embed.is_none() {
                    // wording: "<host> is showing a secure screen",
                    // the body names what that is on the host's OS.
                    let (name, os) = self.app.host.lock().unwrap().as_ref().map_or(("The host".to_string(), msg::Os::Windows), |w| (w.name.clone(), w.host_os));
                    let body = match os {
                        msg::Os::Mac => "The lock screen can't be shown or controlled from here.",
                        _ => "The lock screen, the sign-in screen and User Account Control prompts can't be shown or controlled from here.",
                    };
                    let lines = [format!("{name} is showing a secure screen"), body.to_string(), "The session continues when it closes.".to_string()];
                    let (w0, h0, px) = text_panel(&lines, 22, 200);
                    self.notice = self.gfx.overlay(w0, h0, &px).ok();
                }
                self.dirty = true;
            }
        }
    }

    // ---- cursor and lock ------------------------------------------------------

    fn apply_lock(&mut self) {
        unsafe {
            let mut r = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut r);
            let mut tl = POINT { x: r.left, y: r.top };
            let mut br = POINT { x: r.right, y: r.bottom };
            let _ = ClientToScreen(self.hwnd, &mut tl);
            let _ = ClientToScreen(self.hwnd, &mut br);
            match self.lock {
                msg::PointerLock::Locked if self.focused && self.capture => {
                    let (cx, cy) = ((tl.x + br.x) / 2, (tl.y + br.y) / 2);
                    let _ = SetCursorPos(cx, cy);
                    let _ = ClipCursor(Some(&RECT { left: cx, top: cy, right: cx + 1, bottom: cy + 1 }));
                }
                msg::PointerLock::Confined if self.focused && self.capture => {
                    let _ = ClipCursor(Some(&RECT { left: tl.x, top: tl.y, right: br.x, bottom: br.y }));
                }
                _ => {
                    let _ = ClipCursor(None);
                }
            }
        }
    }

    /// The host's cursor as the local one, scaled like the video.
    fn apply_cursor(&mut self) {
        if self.lock == msg::PointerLock::Locked || !self.remote.2 {
            self.cursor = 0;
        } else {
            let hash = self.remote.3;
            let k = self.video_scale();
            let key = (hash, (k * 1000.0) as i32);
            if let Some(&c) = self.cursors.get(&key) {
                self.cursor = c;
            } else if let Some(img) = self.shapes.get(&hash) {
                match make_cursor(img, k) {
                    Some(c) => {
                        eprintln!("remote-client: cursor shape {hash:016x}: {}x{} hot ({}, {}), scaled by {k:.3}", img.width, img.height, img.hot_x, img.hot_y);
                        self.cursors.insert(key, c);
                        self.cursor = c;
                    }
                    None => self.cursor = unsafe { LoadCursorW(None, IDC_ARROW).map_or(0, |c| c.0 as isize) },
                }
            }
        }
        if self.mouse_in {
            unsafe {
                SetCursor(if self.cursor == 0 { None } else { Some(HCURSOR(self.cursor as *mut c_void)) });
            }
        }
    }

    // ---- input ------------------------------------------------------------------

    fn key(&mut self, scan: u32, ext: bool, down: bool) {
        let Some(usage) = scancode_to_hid(scan, ext) else { return };
        if down {
            if !self.keys_down.insert(usage) {
                return; // auto-repeat: the host repeats by itself
            }
        } else if !self.keys_down.remove(&usage) {
            return;
        }
        if down && usage == 0x29 && self.embed.is_some() && self.chord_held() {
            // Ctrl+Alt+Shift+Esc: never sent; every key held goes up on the host and the keyboard is ours again.
            self.release_all();
            self.set_capture(false, true);
            if let Some(e) = &self.embed {
                (e.on_event)(EmbedEvent::LeaveChord);
            }
            return;
        }
        self.send(Input::Key { usage, down });
    }

    fn release_all(&mut self) {
        for u in std::mem::take(&mut self.keys_down) {
            self.send(Input::Key { usage: u, down: false });
        }
        for (bit, b) in [(1, Button::Left), (2, Button::Right), (4, Button::Middle), (8, Button::Back), (16, Button::Forward)] {
            if self.buttons & bit != 0 {
                self.send(Input::Button { button: b, down: false });
            }
        }
        self.buttons = 0;
    }

    fn button(&mut self, bit: u32, b: Button, down: bool) {
        let before = self.buttons;
        if down {
            if self.buttons == 0 {
                unsafe { SetCapture(self.hwnd) };
            }
            self.buttons |= bit;
        } else {
            self.buttons &= !bit;
            if self.buttons == 0 {
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
        }
        if before != self.buttons {
            self.send(Input::Button { button: b, down });
        }
    }

    fn pointer_abs(&self, x: i32, y: i32) {
        if self.lock == msg::PointerLock::Locked {
            return;
        }
        let v = self.video_size();
        if v.0 == 0 {
            return;
        }
        let (px, py) = layout::to_stream(self.placement(), v, x as f64, y as f64);
        self.send(Input::PointerAbs { x: px, y: py });
    }

    fn wheel(&self, axis: Axis, delta: i32) {
        // Windows: positive is up and right. The wire follows Wayland: positive is down and right.
        let value120 = if axis == Axis::Vertical { -delta } else { delta };
        self.send(Input::Axis { source: AxisSource::Wheel, axis, value: value120 as f64 / 120.0 * 15.0, value120, stop: false, more: false });
    }

    fn raw_mouse(&self, lp: isize) {
        if self.lock != msg::PointerLock::Locked {
            return;
        }
        unsafe {
            let mut buf = [0u8; 128];
            let mut size = buf.len() as u32;
            let n = GetRawInputData(
                HRAWINPUT(lp as *mut c_void),
                RID_INPUT,
                Some(buf.as_mut_ptr().cast()),
                &mut size,
                std::mem::size_of::<RAWINPUTHEADER>() as u32,
            );
            if n == u32::MAX || n == 0 {
                return;
            }
            let ri = &*(buf.as_ptr() as *const RAWINPUT);
            if ri.header.dwType != RIM_TYPEMOUSE.0 {
                return;
            }
            let m = ri.data.mouse;
            if m.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0 || (m.lLastX == 0 && m.lLastY == 0) {
                return;
            }
            let v = self.video_size();
            let k = if v.0 == 0 { 1.0 } else { v.0 as f64 / self.placement().w };
            let (dx, dy) = (m.lLastX as f64 * k, m.lLastY as f64 * k);
            self.send(Input::PointerRel { dx, dy, dx_unaccel: dx, dy_unaccel: dy });
        }
    }

    // ---- drawing ------------------------------------------------------------------

    fn draw(&mut self) -> Result<()> {
        let app = self.app.clone();
        let mut new_frame = None;
        let mut show_ms = 0.0;
        if let Some((pic, id)) = app.latest.take() {
            let before = self.gfx.video_size();
            let (bt601, full_range) = (pic.bt601, pic.full_range);
            let t = Instant::now();
            match self.gfx.show(pic) {
                Err(e) => eprintln!("remote-client: picture: {e:#}"),
                Ok(()) => {
                    show_ms = t.elapsed().as_secs_f64() * 1e3;
                    self.bt601 = bt601;
                    self.full_range = full_range;
                    new_frame = Some(id);
                }
            }
            if self.gfx.video_size() != before {
                self.cursors.clear();
                self.apply_cursor();
                self.video_changed = true;
            }
        }
        let (bw, bh) = self.client_size();
        let v = self.video_size();
        let place = (v.0 > 0).then(|| self.placement());
        if self.opts.hud && Instant::now() >= self.next_hud {
            let mut lines = app.stats.hud_lines(&{
                let mut extra = Vec::new();
                if let Some(t) = &app.timeline.lock().unwrap().shown {
                    extra.push(format!("link: {t}"));
                }
                if let Some(c) = &app.click {
                    extra.push(c.status());
                }
                if let Some(l) = &self.times.line {
                    extra.push(l.clone());
                }
                extra
            });
            if self.gfx.warp {
                lines.push("renderer: WARP (software)".into());
            }
            let (w0, h0, px) = text_panel(&lines, 14, 166);
            self.hud = self.gfx.overlay(w0, h0, &px).ok();
            self.next_hud = Instant::now() + Duration::from_millis(500);
        }
        let mut overlays: Vec<(&Overlay, (f32, f32, f32, f32))> = Vec::new();
        if self.opts.hud {
            if let Some(h) = &self.hud {
                let m = 12.0;
                overlays.push((h, (m, bh as f32 - m - h.h as f32, h.w as f32, h.h as f32)));
            }
        }
        if self.secure {
            if let Some(n) = &self.notice {
                overlays.push((n, ((bw as f32 - n.w as f32) / 2.0, (bh as f32 - n.h as f32) / 2.0, n.w as f32, n.h as f32)));
            }
        }
        let video = place.map(|p| (p.x, p.y, p.w, p.h));
        if (bw, bh, v) != self.logged {
            eprintln!("remote-client: view: client {bw}x{bh}, swapchain {:?}, picture {v:?}, placed {:?}", self.gfx.size(), video);
            self.logged = (bw, bh, v);
        }
        let (bt601, full) = (self.bt601, self.full_range);
        let present_id = self.gfx.draw(video, bt601, full, &overlays)?;
        if let Some(id) = new_frame {
            let commit_us = app.clock.now_us() as i64;
            app.stats.on_commit(id, commit_us);
            self.pending.push_back(Pending { present_id, frame_id: id, at: Instant::now(), commit_us, show_ms, path: self.gfx.path() });
        }
        self.dirty = false;
        Ok(())
    }

    fn qpc_to_us(&self, qpc: i64) -> i64 {
        let mut now = 0i64;
        unsafe {
            let _ = QueryPerformanceCounter(&mut now);
        }
        let mono_now = remote_engine::connect::mono_ns() as i64;
        let ago_ns = (now - qpc) as i128 * 1_000_000_000 / self.qpc_freq as i128;
        self.app.clock.mono_to_us((mono_now as i128 - ago_ns) as u64)
    }

    /// Frame statistics say when each presented frame was shown; frames whose
    /// numbers DXGI never reports (WARP, occluded) are stamped when they were
    /// presented plus a frame.
    fn poll_present(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let stats = self.gfx.frame_stats();
        let refresh_ns = self.display.map_or(16_666_667, |d| (1e12 / d.refresh_mhz.max(1000) as f64) as u32);
        while let Some(p) = self.pending.front() {
            let us = match stats {
                Some((count, qpc)) if p.present_id <= count => Some((p.present_id == count, self.qpc_to_us(qpc))),
                _ if p.at.elapsed() > Duration::from_millis(100) => {
                    Some((true, self.app.clock.now_us() as i64 - p.at.elapsed().as_micros() as i64 + refresh_ns as i64 / 1000))
                }
                _ => None,
            };
            let Some((exact, us)) = us else { break };
            let p = self.pending.pop_front().unwrap();
            if !exact {
                // Replaced before it reached the screen.
                self.app.stats.on_discard(Some(p.frame_id));
                continue;
            }
            if let Some((decode_us, present_us, ready_us)) = self.app.stats.on_present(Some(p.frame_id), us, refresh_ns) {
                self.app.client.send_receiver_report(p.frame_id, decode_us, present_us);
                self.on_lead(ready_us, us, refresh_ns);
                let ms = |d: i64| d.max(0) as f64 / 1e3;
                self.times.add(p.path, p.show_ms, ms(p.commit_us - ready_us), ms(us - p.commit_us), ms(decode_us as i64 + present_us as i64));
            }
            if let Some(c) = &self.app.click {
                c.on_present(p.frame_id, us);
            }
        }
    }

    /// The frame's lead before its vsync, on the live session's clock, for
    /// `Msg::Vsync` about once a second.
    fn on_lead(&mut self, ready_us: i64, present_us: i64, refresh_ns: u32) {
        let Some(live) = self.app.live() else { return };
        if live.generation != self.lead_gen {
            self.lead = LeadStats::default();
            self.lead_gen = live.generation;
        }
        self.lead.add(ready_us - live.offset_us, present_us - live.offset_us, refresh_ns);
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

    // ---- window messages ----------------------------------------------------------

    fn on_message(&mut self, hwnd: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let (wpi, lpi) = (wp.0 as isize, lp.0);
        if m != WM_MOUSEMOVE && m != WM_SETCURSOR && m != WM_NCHITTEST && std::env::var_os("REMOTE_LOG_KEYS").is_some() {
            eprintln!("remote-client: msg {m:#x} wp {wpi:#x} lp {lpi:#x}");
        }
        unsafe {
            match m {
                WM_CLOSE => {
                    self.closed = true;
                    self.app.quit();
                    LRESULT(0)
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                WM_ERASEBKGND => LRESULT(1),
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    BeginPaint(hwnd, &mut ps);
                    let _ = EndPaint(hwnd, &ps);
                    self.dirty = true;
                    LRESULT(0)
                }
                WM_SIZE => {
                    if wpi as u32 != SIZE_MINIMIZED {
                        self.on_size();
                    }
                    LRESULT(0)
                }
                WM_DPICHANGED if self.embed.is_none() => {
                    let r = &*(lp.0 as *const RECT);
                    let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                    LRESULT(0)
                }
                WM_SETFOCUS => {
                    self.focused = true;
                    self.apply_lock();
                    LRESULT(0)
                }
                WM_KILLFOCUS => {
                    self.release_all();
                    self.focused = false;
                    self.apply_lock();
                    LRESULT(0)
                }
                WM_SETCURSOR => {
                    if loword(lpi) as u32 == HTCLIENT {
                        SetCursor(if self.cursor == 0 { None } else { Some(HCURSOR(self.cursor as *mut c_void)) });
                        return LRESULT(1);
                    }
                    DefWindowProcW(hwnd, m, wp, lp)
                }
                WM_MOUSEMOVE => {
                    if !self.mouse_in {
                        self.mouse_in = true;
                        let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        let _ = TrackMouseEvent(&mut t);
                    }
                    self.pointer_abs(sx(lpi), sy(lpi));
                    LRESULT(0)
                }
                WM_MOUSELEAVE => {
                    self.mouse_in = false;
                    LRESULT(0)
                }
                WM_INPUT => {
                    self.raw_mouse(lpi);
                    DefWindowProcW(hwnd, m, wp, lp)
                }
                WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MBUTTONUP => {
                    let (bit, b, down) = match m {
                        WM_LBUTTONDOWN => (1, Button::Left, true),
                        WM_LBUTTONUP => (1, Button::Left, false),
                        WM_RBUTTONDOWN => (2, Button::Right, true),
                        WM_RBUTTONUP => (2, Button::Right, false),
                        WM_MBUTTONDOWN => (4, Button::Middle, true),
                        _ => (4, Button::Middle, false),
                    };
                    if self.blocked {
                        // A press in the video closes the app's menu and goes no further.
                        if down {
                            if let Some(e) = &self.embed {
                                (e.on_event)(EmbedEvent::VideoClick);
                            }
                        }
                        return LRESULT(0);
                    }
                    if down {
                        if self.embed.is_some() && !self.capture {
                            self.set_capture(true, true);
                            let _ = SetFocus(Some(hwnd));
                        }
                        self.pointer_abs(sx(lpi), sy(lpi));
                    }
                    self.button(bit, b, down);
                    LRESULT(0)
                }
                WM_XBUTTONDOWN | WM_XBUTTONUP => {
                    let (bit, b) = if hiword(wpi) as u32 & 1 != 0 { (8, Button::Back) } else { (16, Button::Forward) };
                    if m == WM_XBUTTONDOWN {
                        self.pointer_abs(sx(lpi), sy(lpi));
                    }
                    self.button(bit, b, m == WM_XBUTTONDOWN);
                    LRESULT(1)
                }
                WM_MOUSEWHEEL => {
                    self.wheel(Axis::Vertical, hiword(wpi) as i16 as i32);
                    LRESULT(0)
                }
                WM_MOUSEHWHEEL => {
                    self.wheel(Axis::Horizontal, hiword(wpi) as i16 as i32);
                    LRESULT(0)
                }
                WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
                    if self.blocked {
                        // A menu of the app is open: its keys (Escape, arrows) go to it, nothing to the computer.
                        if m == WM_KEYDOWN || m == WM_SYSKEYDOWN {
                            if let Some(e) = &self.embed {
                                (e.on_event)(EmbedEvent::MenuKey(wpi as u32));
                            }
                        }
                        return LRESULT(0);
                    }
                    // With the hook the keys came already; without it, this is the path.
                    if self.hook.is_none() && self.capture {
                        let scan = ((lpi >> 16) & 0xFF) as u32;
                        let ext = (lpi >> 24) & 1 != 0;
                        self.key(scan, ext, m == WM_KEYDOWN || m == WM_SYSKEYDOWN);
                    }
                    LRESULT(0)
                }
                WM_SYSCOMMAND if (wpi as u32 & 0xFFF0) == SC_KEYMENU => LRESULT(0),
                WM_CHAR | WM_SYSCHAR | WM_UNICHAR => LRESULT(0),
                _ => DefWindowProcW(hwnd, m, wp, lp),
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let p = WIN.load(Ordering::SeqCst);
    if p.is_null() {
        return unsafe { DefWindowProcW(hwnd, m, wp, lp) };
    }
    unsafe { (*p).on_message(hwnd, m, wp, lp) }
}

/// The low-level keyboard hook: while our window is in the foreground, every
/// key (Win, Alt+Tab, Alt+Esc too) goes to the host and nowhere else.
unsafe extern "system" fn ll_keyboard(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let p = WIN.load(Ordering::SeqCst);
        if code == HC_ACTION as i32 && !p.is_null() {
            let w = &mut *p;
            if std::env::var_os("REMOTE_LOG_KEYS").is_some() {
                let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
                eprintln!("remote-client: hook vk {:#x} scan {:#x} flags {:#x} msg {:#x} foreground {}", k.vkCode, k.scanCode, k.flags.0, wp.0, GetForegroundWindow() == w.root);
            }
            if GetForegroundWindow() == w.root && w.capture {
                let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
                // IME-injected characters (VK_PACKET) are not keys.
                if k.vkCode != VK_PACKET.0 as u32 {
                    let mut scan = k.scanCode;
                    if scan == 0 {
                        scan = MapVirtualKeyW(k.vkCode, MAPVK_VK_TO_VSC);
                    }
                    let down = matches!(wp.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
                    w.key(scan, k.flags.0 & LLKHF_EXTENDED.0 != 0, down);
                    return LRESULT(1);
                }
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

// ---- GDI: text panels and cursors ---------------------------------------------------

/// `lines` as a premultiplied BGRA panel: white text on black at `alpha`/255.
fn text_panel(lines: &[String], px: i32, alpha: u8) -> (u32, u32, Vec<u8>) {
    unsafe {
        let dc = CreateCompatibleDC(None);
        let font = CreateFontW(
            -px,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            (FIXED_PITCH.0 | FF_MODERN.0) as u32,
            w!("Consolas"),
        );
        let old = SelectObject(dc, font.into());
        let pad = px / 2 + 2;
        let line_h = px + px / 4 + 2;
        let mut width = 0;
        for l in lines {
            let t: Vec<u16> = l.encode_utf16().collect();
            let mut sz = SIZE::default();
            let _ = GetTextExtentPoint32W(dc, &t, &mut sz);
            width = width.max(sz.cx);
        }
        let (w0, h0) = ((width + 2 * pad).max(1), (line_h * lines.len() as i32 + 2 * pad).max(1));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w0,
                biHeight: -h0,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
        let oldb = SelectObject(dc, bmp.into());
        let n = (w0 * h0) as usize;
        let mut out = vec![0u8; n * 4];
        if !bits.is_null() {
            std::ptr::write_bytes(bits as *mut u8, 0, n * 4);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            for (i, l) in lines.iter().enumerate() {
                let t: Vec<u16> = l.encode_utf16().collect();
                let _ = TextOutW(dc, pad, pad + i as i32 * line_h, &t);
            }
            let src = std::slice::from_raw_parts(bits as *const u8, n * 4);
            for (o, s) in out.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                let lum = s[0].max(s[1]).max(s[2]);
                let a = alpha.max(lum);
                o.copy_from_slice(&[lum, lum, lum, a]);
            }
        }
        SelectObject(dc, oldb);
        SelectObject(dc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteObject(font.into());
        let _ = DeleteDC(dc);
        (w0 as u32, h0 as u32, out)
    }
}

/// An HCURSOR from the host's image (premultiplied BGRA in stream pixels),
/// scaled by `k` (bilinear, at most 256 px).
fn make_cursor(img: &CursorImage, k: f64) -> Option<isize> {
    // Core already drops images that aren't 1..=256 a side with exactly w*h*4 bytes; checked again here
    // before any i32 arithmetic, which would wrap on a bigger one.
    if !img.is_valid() {
        return None;
    }
    let k = k.min(256.0 / img.width.max(img.height) as f64);
    let (dw, dh) = (((img.width as f64 * k).round() as i32).clamp(1, 256), ((img.height as f64 * k).round() as i32).clamp(1, 256));
    let (sw, sh) = (img.width as i32, img.height as i32);
    let mut px = vec![0u8; (dw * dh * 4) as usize];
    for y in 0..dh {
        let fy = ((y as f64 + 0.5) * sh as f64 / dh as f64 - 0.5).clamp(0.0, (sh - 1) as f64);
        let (y0, wy) = (fy.floor() as i32, fy - fy.floor());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..dw {
            let fx = ((x as f64 + 0.5) * sw as f64 / dw as f64 - 0.5).clamp(0.0, (sw - 1) as f64);
            let (x0, wx) = (fx.floor() as i32, fx - fx.floor());
            let x1 = (x0 + 1).min(sw - 1);
            for c in 0..4 {
                let at = |xx: i32, yy: i32| img.pixels[((yy * sw + xx) * 4 + c) as usize] as f64;
                let v = at(x0, y0) * (1.0 - wx) * (1.0 - wy) + at(x1, y0) * wx * (1.0 - wy) + at(x0, y1) * (1.0 - wx) * wy + at(x1, y1) * wx * wy;
                px[((y * dw + x) * 4 + c) as usize] = v.round() as u8;
            }
        }
    }
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: dw,
                biHeight: -dh,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            let _ = DeleteObject(color.into());
            return None;
        }
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mask = CreateBitmap(dw, dh, 1, 1, None);
        let info = ICONINFO {
            fIcon: false.into(),
            xHotspot: ((img.hot_x as f64 * k) as i32).clamp(0, dw - 1) as u32,
            yHotspot: ((img.hot_y as f64 * k) as i32).clamp(0, dh - 1) as u32,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon.ok().map(|h| h.0 as isize)
    }
}
