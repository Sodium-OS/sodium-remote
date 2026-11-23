// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Remote's Qt client app engine: `remote-engine` behind a small C ABI.
//!
//! - **Events** go out as JSON text through one callback (`rq_event_fn`), from any thread; the Qt
//!   side marshals them onto its own. They carry ids, enums and numbers, never translated text.
//! - **Commands** are plain calls that return at once; their results come back as events.
//! - **Two halves:** the *browse* half is a long-lived `remote_core::connect::Client` (paired list,
//!   presence, pairing windows, pairing); the *session* half is `remote_engine::client::run` on its
//!   own thread, with the video as a subsurface under Qt's toplevel (`remote_client::window::embed`).
//!   The browse client is shut down while a session runs (one device identity, one rendezvous
//!   registration) and starts again when it ends.
//!
//! Everything platform-specific is in `linux`; the Windows port adds a `windows` module with the same
//! `Platform` and surface calls.

use remote_core::connect::{Client, ClientEvent, ConnectError, PairError, PairingTarget};
use remote_core::keys::DeviceId;
use remote_core::msg;
use remote_engine::client::{CodecChoice, Options};
use remote_engine::connect::NetOptions;
use remote_engine::{App, SessionStatus};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(windows, all(target_os = "linux", feature = "linux-host")))]
mod host;
#[cfg(windows)]
mod windows;

/// Where events go: one JSON document each.
pub type Sink = Arc<dyn Fn(Value) + Send + Sync>;

pub type EventFn = extern "C" fn(user: *mut c_void, json: *const c_char);

struct Session {
    app: Arc<App>,
    #[cfg(target_os = "linux")]
    surface: Arc<linux::Surface>,
    #[cfg(windows)]
    surface: Arc<windows::Surface>,
}

pub struct Rq {
    rt: tokio::runtime::Runtime,
    data: PathBuf,
    name: String,
    rendezvous: bool,
    sink: Sink,
    gate: Arc<Gate>,
    browse: Mutex<Option<Client>>,
    /// Wrong-code tries per host in this run (the host withdraws a code after three).
    tries: Mutex<HashMap<String, u32>>,
    /// The running session's app, for commands.
    session: Arc<Mutex<Option<Session>>>,
    /// Addresses typed by the user: host id -> LAN listener, kept for connects.
    hints: Mutex<Vec<(DeviceId, std::net::SocketAddr)>>,
    key_passthrough: Arc<std::sync::atomic::AtomicBool>,
    /// A session is starting or running.
    running: std::sync::atomic::AtomicBool,
}

/// The UI's id for a system: "sodiumos" for a Linux whose os-release `ID` says so,
/// "linux" for any other Linux (or one with no flavor), "mac", "windows".
pub fn os_name(o: msg::Os, flavor: &str) -> &'static str {
    match o {
        msg::Os::Linux if flavor == "sodiumos" => "sodiumos",
        msg::Os::Linux => "linux",
        msg::Os::Mac => "mac",
        msg::Os::Windows => "windows",
    }
}

fn reason_json(r: &msg::DisconnectReason) -> Value {
    use msg::DisconnectReason as R;
    match r {
        R::User => json!({"kind": "user"}),
        R::TakenOver { by } => json!({"kind": "taken_over", "by": by}),
        R::HostShutdown => json!({"kind": "host_shutdown"}),
        R::HostSleeping => json!({"kind": "host_sleeping"}),
        R::Revoked => json!({"kind": "revoked"}),
        R::VersionMismatch { host, client } => json!({"kind": "version_mismatch", "host": host, "client": client,
            "update": if host > client { "client" } else { "host" }}),
        R::HoldExpired => json!({"kind": "hold_expired"}),
        R::ControlOff => json!({"kind": "control_off"}),
        R::Error(e) => json!({"kind": "error", "detail": e}),
        R::EndedAtComputer => json!({"kind": "ended_at_computer"}),
        R::HostLocked => json!({"kind": "host_locked"}),
        R::PermissionMissing { screen_recording, accessibility } => {
            json!({"kind": "permission_missing", "screen_recording": screen_recording, "accessibility": accessibility})
        }
    }
}

fn connect_error_json(e: &ConnectError) -> Value {
    match e {
        ConnectError::Busy { by } => json!({"kind": "busy", "by": by}),
        ConnectError::Offline => json!({"kind": "offline"}),
        ConnectError::NotPaired => json!({"kind": "not_paired"}),
        ConnectError::Refused(r) => json!({"kind": "refused", "reason": reason_json(r)}),
        ConnectError::Timeout => json!({"kind": "timeout"}),
        ConnectError::Failed(d) => json!({"kind": "failed", "detail": d}),
    }
}

pub fn status_json(s: &SessionStatus) -> Value {
    match s {
        SessionStatus::Connecting => json!({"ev": "status", "state": "connecting"}),
        SessionStatus::ConnectFailed(e) => json!({"ev": "status", "state": "failed", "error": connect_error_json(e)}),
        SessionStatus::Connected { route } => json!({
            "ev": "status", "state": "connected",
            "route": route.as_ref().map(|r| remote_engine::stats::route_name(r)),
            "rtt_ms": route.as_ref().map(|r| r.rtt_ms),
        }),
        SessionStatus::Welcome(w) => json!({
            "ev": "welcome",
            "name": w.name,
            "host_os": os_name(w.host_os, &w.host_os_flavor),
            "display_mode": if w.display_mode == msg::DisplayMode::Virtual { "virtual" } else { "physical" },
            "live_resize": w.live_resize,
            "scales": w.scales,
            "home_monitor": w.home_monitor.is_some(),
            "current_display": w.current_display,
            "view_only": w.view_only,
            "displays": w.displays.iter().map(|d| json!({"id": d.id, "name": d.name, "width": d.width, "height": d.height, "scale": d.scale, "primary": d.primary})).collect::<Vec<_>>(),
            "width": w.format.width, "height": w.format.height,
            "refresh_mhz": w.format.refresh_mhz,
            "codec": format!("{:?}", w.format.codec),
        }),
        SessionStatus::Reconnecting => json!({"ev": "status", "state": "reconnecting"}),
        SessionStatus::Ended(r) => json!({"ev": "status", "state": "ended", "reason": reason_json(r)}),
    }
}

fn device_json(d: &remote_core::keys::PairedDevice) -> Value {
    json!({"id": d.id.to_string(), "code": d.id.code(), "name": d.name, "paired_at": d.paired_at, "last_connected": d.last_connected})
}

impl Rq {
    fn net(&self) -> NetOptions {
        NetOptions {
            data: self.data.clone(),
            name: self.name.clone(),
            rendezvous: self.rendezvous,
            mdns: true,
            relay_only: false,
            bind: vec![],
            lan_hints: self.hints.lock().unwrap().clone(),
            fps: 60,
        }
    }

    /// Starts the browse client (pairing, presence), if it isn't running.
    fn start_browse(self: &Arc<Self>) {
        if self.browse.lock().unwrap().is_some() {
            return;
        }
        let net = self.net();
        match self.rt.block_on(remote_engine::connect::start(&net)) {
            Ok((client, mut events)) => {
                *self.browse.lock().unwrap() = Some(client);
                let sink = self.sink.clone();
                self.rt.spawn(async move {
                    while let Some(e) = events.recv().await {
                        match e {
                            ClientEvent::Rendezvous { online, error } => sink(json!({"ev": "rendezvous", "online": online, "error": error})),
                            ClientEvent::Revoked { host } => sink(json!({"ev": "revoked", "host": host.to_string()})),
                            _ => {}
                        }
                    }
                });
                let c = self.browse.lock().unwrap().clone().unwrap();
                (self.sink)(json!({"ev": "self", "id": c.id().to_string(), "code": c.id().code()}));
            }
            Err(e) => (self.sink)(json!({"ev": "error", "where": "start", "detail": e.to_string()})),
        }
    }

    fn browse(&self) -> Option<Client> {
        self.browse.lock().unwrap().clone()
    }

    fn refresh(self: &Arc<Self>) {
        let me = self.clone();
        self.rt.spawn(async move {
            let Some(c) = me.browse() else { return };
            let paired = c.paired();
            let (status, found) = tokio::join!(c.presence(), c.discover_pairing(Duration::from_millis(400)));
            let hosts: Vec<Value> = paired
                .iter()
                .map(|d| {
                    let s = status.iter().find(|s| s.host == d.id);
                    let mut v = device_json(d);
                    v["online"] = json!(s.is_some_and(|s| s.online || s.on_lan));
                    v["on_lan"] = json!(s.is_some_and(|s| s.on_lan));
                    v["last_seen"] = json!(s.and_then(|s| s.last_seen));
                    v["asleep_since"] = json!(s.and_then(|s| s.asleep_since));
                    v
                })
                .collect();
            let found: Vec<Value> = found
                .iter()
                .filter(|t| !paired.iter().any(|d| d.id == t.host))
                .map(|t| json!({"id": t.host.to_string(), "code": t.host.code(), "name": t.name,
                                "addrs": t.lan.as_ref().map(|(a, _)| a.iter().map(|i| i.to_string()).collect::<Vec<_>>()),
                                "port": t.lan.as_ref().map(|(_, p)| *p), "rendezvous": t.rendezvous.is_some()}))
                .collect();
            (me.sink)(json!({"ev": "hosts", "paired": hosts, "found": found}));
        });
    }

    fn pair(self: &Arc<Self>, host: String, code: String, addr: Option<(std::net::IpAddr, u16)>) {
        let me = self.clone();
        self.rt.spawn(async move {
            let sink = me.sink.clone();
            let say = |state: &str, extra: Value| {
                let mut v = json!({"ev": "pair", "state": state, "host": host});
                if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
                    o.extend(e.clone());
                }
                sink(v);
            };
            let Some(c) = me.browse() else { return say("failed", json!({})) };
            say("checking", json!({}));
            // No host ID, only an address: the ID is learned from the pairing, bound by the code.
            if let (true, Some((ip, port))) = (host.is_empty(), addr) {
                match c.pair_by_address(std::net::SocketAddr::new(ip, port), code.trim()).await {
                    Ok(d) => say("ok", json!({"name": d.name, "code": d.id.code(), "id": d.id.to_string()})),
                    Err(PairError::WrongCode) => say("wrong", json!({})),
                    Err(PairError::Closed(r)) => say("closed", json!({"why": r})),
                    Err(PairError::Failed(r)) => say("failed", json!({"detail": r})),
                }
                return me.refresh();
            }
            let Ok(id) = host.parse::<DeviceId>() else { return say("failed", json!({})) };
            let target: Option<PairingTarget> = match addr {
                // A host typed with its address: nothing to find.
                Some((ip, port)) => Some(PairingTarget { host: id, name: String::new(), lan: Some((vec![ip], port)), rendezvous: None }),
                None => c.discover_pairing(Duration::from_secs(2)).await.into_iter().find(|t| t.host == id),
            };
            let Some(t) = target else { return say("closed", json!({"why": "not_found"})) };
            match c.pair(&t, code.trim()).await {
                Ok(d) => {
                    me.tries.lock().unwrap().remove(&host);
                    say("ok", json!({"name": d.name, "code": d.id.code()}))
                }
                Err(PairError::WrongCode) => {
                    let mut t = me.tries.lock().unwrap();
                    let n = t.entry(host.clone()).or_insert(0);
                    *n += 1;
                    let left = 3u32.saturating_sub(*n);
                    if left == 0 {
                        t.remove(&host);
                        say("withdrawn", json!({}))
                    } else {
                        say("wrong", json!({"left": left}))
                    }
                }
                Err(PairError::Closed(r)) => say("closed", json!({"why": r})),
                Err(PairError::Failed(r)) => say("failed", json!({"detail": r})),
            }
            me.refresh();
        });
    }

    /// An address the user typed: `ID@IP:PORT` pairs directly; a bare `IP[:PORT]` is matched against
    /// the hosts seen on the LAN with an open pairing window.
    fn resolve_address(self: &Arc<Self>, text: String) {
        let me = self.clone();
        self.rt.spawn(async move {
            let say = |v: Value| (me.sink)(v);
            let text = text.trim().to_string();
            let (id_part, addr_part) = match text.split_once('@') {
                Some((i, a)) => (Some(i.to_string()), a.to_string()),
                None => (None, text.clone()),
            };
            let (ip_s, port) = match addr_part.rsplit_once(':') {
                Some((i, p)) if p.parse::<u16>().is_ok() && !i.contains(':') => (i.to_string(), p.parse::<u16>().ok()),
                _ => (addr_part.clone(), None),
            };
            let Ok(ip) = ip_s.parse::<std::net::IpAddr>() else {
                return say(json!({"ev": "address", "state": "unreachable", "addr": text}));
            };
            if let (Some(id), Some(port)) = (id_part.as_deref().and_then(|i| i.parse::<DeviceId>().ok()), port) {
                // Reachable? The LAN listener answers a TCP connect.
                let ok = tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect((ip, port))).await.is_ok_and(|r| r.is_ok());
                if !ok {
                    return say(json!({"ev": "address", "state": "unreachable", "addr": text}));
                }
                me.hints.lock().unwrap().push((id, std::net::SocketAddr::new(ip, port)));
                return say(json!({"ev": "address", "state": "found", "addr": text, "host": id.to_string(), "code": id.code(), "ip": ip.to_string(), "port": port, "name": ""}));
            }
            let Some(c) = me.browse() else { return };
            let found = c.discover_pairing(Duration::from_secs(2)).await;
            match found.iter().find(|t| t.lan.as_ref().is_some_and(|(a, p)| a.contains(&ip) && port.is_none_or(|q| q == *p))) {
                Some(t) => say(json!({"ev": "address", "state": "found", "addr": text, "host": t.host.to_string(), "code": t.host.code(), "name": t.name})),
                None => {
                    // Not announced: a host that answers on its LAN listener can still be paired by
                    // address (an empty `host`: `rq_pair("", code, "IP:PORT")`).
                    let port = port.unwrap_or(remote_core::connect::DEFAULT_LAN_PORT);
                    let ok = tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect((ip, port))).await.is_ok_and(|r| r.is_ok());
                    if ok {
                        say(json!({"ev": "address", "state": "found", "addr": text, "host": "", "ip": ip.to_string(), "port": port, "name": ""}))
                    } else {
                        say(json!({"ev": "address", "state": "unreachable", "addr": text}))
                    }
                }
            }
        });
    }

    fn forget(self: &Arc<Self>, host: String) {
        if let (Some(c), Ok(id)) = (self.browse(), host.parse::<DeviceId>()) {
            // Not saved: the computer stays paired (the list below shows it still), so the
            // person can try again; the reason goes to the log.
            if let Err(e) = c.forget(&id) {
                eprintln!("sodium-remote: forgetting {} was not saved: {e}", id.code());
            }
        }
        self.refresh();
    }
}

// ---- C ABI ---------------------------------------------------------------------------------

pub(crate) unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

pub(crate) struct UserPtr(pub *mut c_void);
unsafe impl Send for UserPtr {}
unsafe impl Sync for UserPtr {}

/// Lets `rq_free` / `rh_free` end the callbacks before the owner of `user` (a Qt object) is deleted: after `close` no callback
/// starts, and `close` returns only once none is still running.
pub(crate) struct Gate {
    state: Mutex<(bool, usize)>,
    idle: std::sync::Condvar,
}

impl Gate {
    pub(crate) fn new() -> Arc<Gate> {
        Arc::new(Gate { state: Mutex::new((false, 0)), idle: std::sync::Condvar::new() })
    }

    fn enter(&self) -> bool {
        let mut g = self.state.lock().unwrap();
        if g.0 {
            return false;
        }
        g.1 += 1;
        true
    }

    fn leave(&self) {
        let mut g = self.state.lock().unwrap();
        g.1 -= 1;
        if g.1 == 0 {
            self.idle.notify_all();
        }
    }

    pub(crate) fn close(&self) {
        let mut g = self.state.lock().unwrap();
        g.0 = true;
        while g.1 > 0 {
            g = self.idle.wait(g).unwrap();
        }
    }
}

/// The sink that calls `cb(user, json)`, gated.
pub(crate) fn gated_sink(cb: EventFn, user: UserPtr, gate: Arc<Gate>) -> Sink {
    Arc::new(move |v: Value| {
        let _ = &user;
        if !gate.enter() {
            return;
        }
        if let Ok(c) = CString::new(v.to_string()) {
            cb(user.0, c.as_ptr());
        }
        gate.leave();
    })
}

/// Creates the engine. `data_dir` holds this device's key and `paired.json`. `rendezvous` 0 is LAN only.
/// Events are delivered to `cb` from engine threads until `rq_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_new(data_dir: *const c_char, device_name: *const c_char, rendezvous: c_int, cb: EventFn, user: *mut c_void) -> *mut Arc<Rq> {
    let gate = Gate::new();
    let sink = gated_sink(cb, UserPtr(user), gate.clone());
    let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(r) => r,
        Err(_) => return std::ptr::null_mut(),
    };
    let rq = Arc::new(Rq {
        rt,
        data: PathBuf::from(unsafe { cstr(data_dir) }),
        name: unsafe { cstr(device_name) },
        rendezvous: rendezvous != 0,
        sink,
        gate,
        browse: Mutex::new(None),
        tries: Mutex::new(HashMap::new()),
        session: Arc::new(Mutex::new(None)),
        hints: Mutex::new(vec![]),
        key_passthrough: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        running: std::sync::atomic::AtomicBool::new(false),
    });
    rq.start_browse();
    Box::into_raw(Box::new(rq))
}

unsafe fn rq<'a>(p: *mut Arc<Rq>) -> &'a Arc<Rq> {
    unsafe { &*p }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_free(p: *mut Arc<Rq>) {
    if p.is_null() {
        return;
    }
    let b = unsafe { Box::from_raw(p) };
    if let Some(s) = b.session.lock().unwrap().as_ref() {
        s.app.quit();
    }
    if let Some(c) = b.browse() {
        b.rt.block_on(c.shutdown());
    }
    // The session thread and the runtime may still be running and calling back: no callback starts after this, and none is
    // running when it returns, so the caller can delete the object `user` points at.
    b.gate.close();
    drop(b);
}

/// Paired computers with their presence, and hosts nearby with an open pairing window: a `hosts` event.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_refresh(p: *mut Arc<Rq>) {
    unsafe { rq(p) }.refresh();
}

/// Pairs with `host_id` using the typed `code`; `pair` events follow. `addr` (`IP:PORT`, may be empty)
/// is the host's LAN listener when it was typed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_pair(p: *mut Arc<Rq>, host_id: *const c_char, code: *const c_char, addr: *const c_char) {
    let r = unsafe { rq(p) };
    let a = unsafe { cstr(addr) };
    let addr = a.parse::<std::net::SocketAddr>().ok().map(|s| (s.ip(), s.port()));
    r.pair(unsafe { cstr(host_id) }, unsafe { cstr(code) }, addr);
}

/// An address typed in "Connect by address": an `address` event says what was found.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_resolve_address(p: *mut Arc<Rq>, text: *const c_char) {
    unsafe { rq(p) }.resolve_address(unsafe { cstr(text) });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_forget(p: *mut Arc<Rq>, host_id: *const c_char) {
    unsafe { rq(p) }.forget(unsafe { cstr(host_id) });
}

/// What this machine decodes, probed now: `{"hevc":..,"h264":..,"av1":..}` (free with `rq_free_string`).
#[unsafe(no_mangle)]
pub extern "C" fn rq_decode_support() -> *mut c_char {
    let mut d = remote_engine::decode::probe_support(remote_engine::decode::HwPref::Auto);
    #[cfg(target_os = "linux")]
    linux::adjust(&mut d);
    #[cfg(windows)]
    windows::adjust(&mut d);
    let v = json!({"hevc": d.hevc, "h264": d.h264, "av1": d.av1});
    CString::new(v.to_string()).unwrap().into_raw()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

/// Starts a session. `opts` is JSON: `host`, `takeover`, `codec` ("auto", "hevc", "h264"), `max_mbps`, `fps`,
/// `ppp`, `width`/`height` (logical), `scale` (buffer scale), `fullscreen`, `key_passthrough`, `decoder`.
/// `wl_display` and `wl_surface` are Qt's. Returns 0, or -1 when a session is running or the options are bad.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_start(p: *mut Arc<Rq>, opts: *const c_char, wl_display: *mut c_void, wl_surface: *mut c_void) -> c_int {
    let r = unsafe { rq(p) }.clone();
    let o: Value = match serde_json::from_str(&unsafe { cstr(opts) }) {
        Ok(v) => v,
        Err(_) => return -1,
    };
    #[cfg(target_os = "linux")]
    {
        return linux::start(&r, &o, wl_display, wl_surface);
    }
    #[cfg(windows)]
    {
        return windows::start(&r, &o, wl_display, wl_surface);
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (o, wl_display, wl_surface);
        -1
    }
}

/// Ends the session (the user's Disconnect).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_stop(p: *mut Arc<Rq>) {
    if let Some(s) = unsafe { rq(p) }.session.lock().unwrap().as_ref() {
        s.app.quit();
    }
}

/// The video area changed: its origin and size in the window (logical pixels), the buffer scale, full screen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_view(p: *mut Arc<Rq>, x: c_int, y: c_int, w: c_int, h: c_int, scale: c_int, fullscreen: c_int) {
    #[cfg(target_os = "linux")]
    if let Some(s) = unsafe { rq(p) }.session.lock().unwrap().as_ref() {
        s.surface.embed.update(|st| {
            st.origin = (x, y);
            st.size = (w, h);
            st.scale = scale.max(1);
            st.fullscreen = fullscreen != 0;
        });
        s.app.wake.wake();
    }
    #[cfg(windows)]
    if let Some(s) = unsafe { rq(p) }.session.lock().unwrap().as_ref() {
        s.surface.embed.update(|st| {
            st.origin = (x, y);
            st.size = (w, h);
            st.fullscreen = fullscreen != 0;
        });
        s.app.wake.wake();
    }
    let _ = (p, x, y, w, h, scale, fullscreen);
}

/// The app's own bars, menus and toasts over the video, in window logical pixels: `rects` is `n` x,y,w,h.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_hud_rects(p: *mut Arc<Rq>, rects: *const c_int, n: c_int) {
    #[cfg(target_os = "linux")]
    if let Some(s) = unsafe { rq(p) }.session.lock().unwrap().as_ref() {
        let v: Vec<[i32; 4]> = (0..n.max(0) as usize).map(|i| unsafe { [*rects.add(i * 4), *rects.add(i * 4 + 1), *rects.add(i * 4 + 2), *rects.add(i * 4 + 3)] }).collect();
        s.surface.embed.update(|st| st.hud_rects = v);
        s.app.wake.wake();
    }
    let _ = (p, rects, n);
}

/// Keyboard capture on or off (the window lost or got focus, or the leave chord was used).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_capture(p: *mut Arc<Rq>, on: c_int) {
    #[cfg(target_os = "linux")]
    if let Some(s) = unsafe { rq(p) }.session.lock().unwrap().as_ref() {
        s.surface.embed.update(|st| st.capture = on != 0);
        s.app.wake.wake();
    }
    #[cfg(windows)]
    if let Some(s) = unsafe { rq(p) }.session.lock().unwrap().as_ref() {
        s.surface.embed.update(|st| st.capture = on != 0);
        s.app.wake.wake();
    }
    let _ = (p, on);
}

/// An app menu is open over the picture (Windows): no pointer or key input reaches the computer until it closes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_menu_block(p: *mut Arc<Rq>, on: c_int) {
    #[cfg(windows)]
    if let Some(s) = unsafe { rq(p) }.session.lock().unwrap().as_ref() {
        s.surface.embed.update(|st| st.blocked = on != 0);
        s.app.wake.wake();
    }
    let _ = (p, on);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_select_display(p: *mut Arc<Rq>, id: u32) {
    if let Some(s) = unsafe { rq(p) }.session.lock().unwrap().as_ref() {
        s.app.select_display(id);
    }
}

/// The host's key table: 0 automatic (by the two systems), 1 pass through. Applies at the next connection.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_key_passthrough(p: *mut Arc<Rq>, on: c_int) {
    unsafe { rq(p) }.key_passthrough.store(on != 0, std::sync::atomic::Ordering::Relaxed);
}

/// Sends Ctrl+Alt+Del to the host.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_send_cad(p: *mut Arc<Rq>) {
    use msg::InputEvent::Key;
    if let Some(s) = unsafe { rq(p) }.session.lock().unwrap().as_ref() {
        // HID usages: Left Ctrl, Left Alt, Delete.
        for (u, d) in [(0xE0, true), (0xE2, true), (0x4C, true), (0x4C, false), (0xE2, false), (0xE0, false)] {
            s.app.send_input(Key { usage: u, down: d });
        }
    }
}

/// The HUD's numbers as JSON (`rtt_ms`, `loss_pct`, `mbps`, `fps`, `presented`, `decoder`, `codec`, `width`, `height`),
/// or null when no session runs. Free with `rq_free_string`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_stats(p: *mut Arc<Rq>) -> *mut c_char {
    let v = match unsafe { rq(p) }.session.lock().unwrap().as_ref() {
        Some(s) => {
            let n = s.app.stats.hud_numbers();
            let (codec, w, h) = n.stream.map_or(("", 0, 0), |st| {
                (match (st.codec, st.chroma444) {
                    (remote_engine::proto::Codec::Hevc, true) => "hevc444",
                    (remote_engine::proto::Codec::Hevc, false) => "hevc",
                    (remote_engine::proto::Codec::Av1, _) => "av1",
                    (remote_engine::proto::Codec::H264, _) => "h264",
                }, st.width, st.height)
            });
            json!({"rtt_ms": n.rtt_ms, "loss_pct": n.loss_pct, "mbps": n.mbps, "fps": n.fps, "presented": n.presented, "decoder": n.decoder, "codec": codec, "width": w, "height": h})
        }
        None => Value::Null,
    };
    CString::new(v.to_string()).unwrap().into_raw()
}

pub(crate) fn codec_choice(s: &str) -> CodecChoice {
    CodecChoice::parse(s).unwrap_or(CodecChoice::Auto)
}

#[allow(dead_code)]
pub(crate) fn unused(_: Options) {}

// --- The server and the data directory -------------

fn json_string(v: Value) -> *mut c_char {
    CString::new(v.to_string()).unwrap_or_default().into_raw()
}

unsafe fn c_str(s: *const c_char) -> String {
    if s.is_null() { String::new() } else { unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned() }
}

/// The app's data directory (the client half): `$SODIUM_REMOTE_DATA`, else the old
/// `$SODIUMOS_REMOTE_DATA`, else `~/.local/share/sodium-remote/client` (copied from
/// the pre-rename directory on first start) or `%LOCALAPPDATA%\SodiumRemote\client`.
/// `""` when it can't be found. Free with `rq_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn rq_default_data_dir() -> *mut c_char {
    let d = remote_core::keys::FileStore::client_default().map(|s| s.dir.to_string_lossy().into_owned()).unwrap_or_default();
    CString::new(d).unwrap_or_default().into_raw()
}

/// Settings › Server: `{"configured": false}` when no `server.toml` names a server
/// (LAN and address only), else `{"configured": true, "url", "host", "label",
/// "official", "pinned_ca", "token", "path", "ca_file", "token_text"}`; `{"configured": false, "error"}` for a
/// broken file. Free with `rq_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn rq_server_info() -> *mut c_char {
    use remote_core::server_config::ServerConfig;
    json_string(match ServerConfig::load() {
        Ok(None) => json!({"configured": false}),
        Ok(Some((c, path))) => json!({
            "configured": true, "url": c.url, "host": c.host(), "label": c.label(), "official": c.is_official(),
            "pinned_ca": c.ca_file.is_some(), "token": c.token.is_some(), "path": path.to_string_lossy(),
            "ca_file": c.ca_file.as_ref().map(|p| p.to_string_lossy().into_owned()), "token_text": c.token,
        }),
        Err(e) => json!({"configured": false, "error": e}),
    })
}

/// What the Server sheets show about a link, typed settings or a server.toml: `{"ok": true, "url", "host", "label",
/// "official", "needs_ca_prompt", "token", "link"}`. `link` is the same server as a `sodium-remote://` link, which
/// `rq_server_import` saves after the user agreed (the apps never save anything else).
fn candidate(l: &remote_core::server_config::ServerLink) -> Value {
    json!({
        "ok": true, "url": l.url, "host": l.host(), "official": l.is_official(), "needs_ca_prompt": l.needs_ca_prompt(),
        "label": if l.is_official() { remote_core::brand::OFFICIAL_SERVICE_NAME.to_string() } else { l.host() },
        "token": l.token.is_some(), "link": l.to_link(),
    })
}

/// Parse a `sodium-remote://server?...` link: [`candidate`]'s JSON or `{"ok": false, "error"}`. Ask the user
/// before `rq_server_import` when `needs_ca_prompt`. Free with `rq_free_string`.
///
/// # Safety
/// `link` is a NUL-terminated string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_server_link_parse(link: *const c_char) -> *mut c_char {
    use remote_core::server_config::ServerLink;
    json_string(match ServerLink::parse(&unsafe { c_str(link) }) {
        Ok(l) => candidate(&l),
        Err(e) => json!({"ok": false, "error": e}),
    })
}

/// Settings typed by hand (Add a server, Change): the address as typed (`relay.example.org`, `https://…`), a CA as
/// PEM or "", a token or "". [`candidate`]'s JSON, or `{"ok": false, "error": "insecure" | "address" | "certificate"}`
/// Free with `rq_free_string`.
///
/// # Safety
/// The arguments are NUL-terminated strings or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_server_check(address: *const c_char, ca_pem: *const c_char, token: *const c_char) -> *mut c_char {
    use remote_core::server_config::{AddressError, check_settings, normalize_address};
    let (address, ca, token) = unsafe { (c_str(address), c_str(ca_pem), c_str(token)) };
    let url = match normalize_address(&address) {
        Ok(u) => u,
        Err(AddressError::Insecure) => return json_string(json!({"ok": false, "error": "insecure"})),
        Err(AddressError::Invalid) => return json_string(json!({"ok": false, "error": "address"})),
    };
    json_string(match check_settings(&url, &ca, &token) {
        Ok(l) => candidate(&l),
        Err(_) if check_settings(&url, "", "").is_err() => json!({"ok": false, "error": "address"}),
        Err(_) => json!({"ok": false, "error": "certificate"}),
    })
}

/// Read a `server.toml` the user chose (Import…): [`candidate`]'s JSON, or `{"ok": false, "error": "none"}` for a file
/// that names no server, or `"file"` when it can't be read or isn't valid. A relative `ca_file` is looked for beside it.
/// Free with `rq_free_string`.
///
/// # Safety
/// `path` is a NUL-terminated string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_server_file_parse(path: *const c_char) -> *mut c_char {
    use remote_core::server_config::{ServerConfig, ServerLink};
    let path = PathBuf::from(unsafe { c_str(path) });
    let base = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    json_string(match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| ServerConfig::parse(&t, &base)) {
        Err(_) => json!({"ok": false, "error": "file"}),
        Ok(None) => json!({"ok": false, "error": "none"}),
        Ok(Some(c)) => match ServerLink::from_config(&c) {
            Ok(mut l) => {
                if l.is_official() {
                    l.ca_der = None;
                }
                candidate(&l)
            }
            Err(_) => json!({"ok": false, "error": "file"}),
        },
    })
}

/// Stop using a server: deletes the user's `server.toml` and the CA saved beside it. `{"ok": true}`; the engine and
/// the host drop the server when they next start (`rq_restart_browse`). Free with `rq_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn rq_server_remove() -> *mut c_char {
    let Some(dir) = remote_core::dirs::user_config_dir() else { return json_string(json!({"ok": false, "error": "no configuration directory"})) };
    for f in [remote_core::brand::SERVER_TOML, remote_core::server_config::IMPORTED_CA_FILE] {
        if let Err(e) = std::fs::remove_file(dir.join(f)) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return json_string(json!({"ok": false, "error": e.to_string()}));
            }
        }
    }
    json_string(json!({"ok": true}))
}

/// Export…: writes the configured server as a `server.toml` at `path` (owner-only: it may hold a token) and its pinned
/// CA, if any, beside it as `<name>-ca.pem`, named in it. `{"ok": true}` or `{"ok": false, "error"}`.
/// Free with `rq_free_string`.
///
/// # Safety
/// `path` is a NUL-terminated string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_server_export(path: *const c_char) -> *mut c_char {
    use remote_core::server_config::ServerConfig;
    let path = PathBuf::from(unsafe { c_str(path) });
    let r = (|| -> Result<(), String> {
        let (mut c, _) = ServerConfig::load()?.ok_or("no server is configured")?;
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        if let Some(ca) = c.ca_file.take() {
            let name = format!("{}-ca.pem", path.file_stem().and_then(|s| s.to_str()).unwrap_or("server"));
            write_private(&dir.join(&name), &std::fs::read(&ca).map_err(|e| format!("{}: {e}", ca.display()))?)?;
            c.ca_file = Some(PathBuf::from(name));
        }
        write_private(&path, c.to_toml().as_bytes())
    })();
    json_string(match r {
        Ok(()) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": e}),
    })
}

/// Writes `data` to a new 0600 file beside `path` and renames it over `path`: an existing file with wider permissions (or
/// someone else's link) is replaced, never written through, so the token is never readable by others.
fn write_private(path: &std::path::Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("export");
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let _ = std::fs::remove_file(&tmp);
    let r = o.open(&tmp).and_then(|mut f| f.write_all(data).and_then(|()| f.sync_all())).and_then(|()| std::fs::rename(&tmp, path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(all(test, unix))]
mod export_tests {
    #[test]
    fn an_export_over_a_wide_file_ends_up_0600() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../build/test-tmp/export");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("server.toml");
        std::fs::write(&f, "old").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        super::write_private(&f, b"token").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"token");
        assert_eq!(std::fs::metadata(&f).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "no temp file left");
    }
}

/// Starts the browse client again, so it reads `server.toml` anew (after Add, Change or Remove). Returns 1 when it did,
/// 0 when a session is running: the new server is used when the session ends (the browse client starts again then).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_restart_browse(p: *mut Arc<Rq>) -> c_int {
    let r = unsafe { rq(p) }.clone();
    if r.running.load(std::sync::atomic::Ordering::SeqCst) {
        return 0;
    }
    if let Some(c) = r.browse.lock().unwrap().take() {
        r.rt.block_on(c.shutdown());
    }
    r.start_browse();
    1
}

/// Save a link as the user's `server.toml` (and its CA beside it). Call after the user
/// agreed to trust the link's CA. `{"ok": true, "path"}` or `{"ok": false, "error"}`.
/// The engine reads the file when it next starts. Free with `rq_free_string`.
///
/// # Safety
/// `link` is a NUL-terminated string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rq_server_import(link: *const c_char) -> *mut c_char {
    use remote_core::server_config::ServerLink;
    let r = ServerLink::parse(&unsafe { c_str(link) }).and_then(|l| {
        let dir = remote_core::dirs::user_config_dir().ok_or("no configuration directory")?;
        l.save(&dir).map_err(|e| e.to_string())?;
        Ok(dir.join(remote_core::brand::SERVER_TOML))
    });
    json_string(match r {
        Ok(p) => json!({"ok": true, "path": p.to_string_lossy()}),
        Err(e) => json!({"ok": false, "error": e}),
    })
}

#[cfg(test)]
mod reason_tests {
    use super::*;

    #[test]
    fn a_locked_host_is_its_own_reason_kind() {
        assert_eq!(reason_json(&msg::DisconnectReason::HostLocked), json!({"kind": "host_locked"}));
        assert_eq!(connect_error_json(&ConnectError::Refused(msg::DisconnectReason::HostLocked))["reason"]["kind"], "host_locked");
    }
}

#[cfg(test)]
mod server_abi_tests {
    use super::*;

    fn call(p: *mut c_char) -> Value {
        let v = serde_json::from_str(&unsafe { CStr::from_ptr(p) }.to_string_lossy()).unwrap();
        unsafe { rq_free_string(p) };
        v
    }
    fn c(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    // The only test that sets XDG_CONFIG_HOME in this crate (one thread, one directory).
    #[test]
    fn a_server_is_checked_saved_exported_imported_and_removed() {
        let root = std::env::temp_dir().join(format!("remote-qt-server-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("out")).unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", root.join("cfg")) };

        assert_eq!(call(rq_server_info())["configured"], false);
        // Typed: a bare host gets wss:// and the default path; plain text is refused.
        let (ca, tok) = (c(""), c("t0k"));
        let v = call(unsafe { rq_server_check(c("relay.example.org").as_ptr(), ca.as_ptr(), tok.as_ptr()) });
        assert_eq!((v["ok"].clone(), v["url"].clone(), v["needs_ca_prompt"].clone()), (json!(true), json!("wss://relay.example.org/remote"), json!(false)));
        assert_eq!(call(unsafe { rq_server_check(c("http://relay.example.org").as_ptr(), ca.as_ptr(), ca.as_ptr()) })["error"], "insecure");
        assert_eq!(call(unsafe { rq_server_check(c("relay.example.org").as_ptr(), c("junk").as_ptr(), ca.as_ptr()) })["error"], "certificate");
        // A link names its host and says when it carries a certificate; saving writes server.toml.
        let pem = remote_core::server_config::pem_from_der(&[1, 2, 3]);
        let with_ca = remote_core::server_config::ServerLink::parse(&v["link"].as_str().unwrap().replace("&token", "&ca=AQID&token")).unwrap();
        let v = call(unsafe { rq_server_link_parse(c(&with_ca.to_link()).as_ptr()) });
        assert_eq!((v["host"].as_str(), v["needs_ca_prompt"].clone()), (Some("relay.example.org"), json!(true)));
        assert_eq!(call(unsafe { rq_server_import(c(v["link"].as_str().unwrap()).as_ptr()) })["ok"], true);
        let info = call(rq_server_info());
        assert_eq!((info["configured"].clone(), info["label"].as_str(), info["pinned_ca"].clone()), (json!(true), Some("relay.example.org"), json!(true)));

        // Export, then read that file as an import: the same server.
        let out = root.join("out/mine.toml");
        assert_eq!(call(unsafe { rq_server_export(c(out.to_str().unwrap()).as_ptr()) })["ok"], true);
        assert_eq!(std::fs::read_to_string(root.join("out/mine-ca.pem")).unwrap(), pem);
        let back = call(unsafe { rq_server_file_parse(c(out.to_str().unwrap()).as_ptr()) });
        assert_eq!((back["link"].clone(), back["host"].clone()), (v["link"].clone(), json!("relay.example.org")));
        std::fs::write(root.join("out/empty.toml"), "# nothing\n").unwrap();
        assert_eq!(call(unsafe { rq_server_file_parse(c(root.join("out/empty.toml").to_str().unwrap()).as_ptr()) })["error"], "none");

        assert_eq!(call(rq_server_remove())["ok"], true);
        assert_eq!(call(rq_server_info())["configured"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_ui_names_a_linux_by_its_flavor() {
        assert_eq!(os_name(msg::Os::Linux, "sodiumos"), "sodiumos");
        assert_eq!(os_name(msg::Os::Linux, "fedora"), "linux");
        assert_eq!(os_name(msg::Os::Linux, ""), "linux");
        // The flavor is only a Linux word.
        assert_eq!(os_name(msg::Os::Windows, "sodiumos"), "windows");
    }
}
