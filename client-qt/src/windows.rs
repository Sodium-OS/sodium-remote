// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The Windows side of the Qt app: the stream as a child HWND of Qt's stream frame.
//!
//! `surface` is the frame's `HWND`; the engine's D3D11 swapchain lives on a child window of it
//! (`client_windows::win`, embedded mode), with its own Raw Input, the keyboard hook while the frame is
//! foreground and capture is on, and the leave chord Ctrl+Alt+Shift+Esc by scan code. Qt draws the bar and
//! cards in an owned, transparent top-level window whose region covers only what it draws.

use crate::{Rq, Session, Sink, codec_choice, status_json};
use client_windows::embed::{Embed, EmbedEvent, EmbedState};
use client_windows::platform::{AudioOptions, Windows};
use remote_engine::client::{self, Options};
use remote_engine::clipboard::ClipboardBackend;
use remote_engine::connect::NetOptions;
use remote_engine::decode::HwPref;
use remote_engine::msg::{self, KeyRule, Os};
use remote_engine::{App, Platform, SessionStatus, WinMsg, Waker, WindowOptions};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

pub struct Surface {
    pub embed: Arc<Embed>,
}

/// AV1 only where the GPU decodes it.
pub fn adjust(d: &mut msg::DecodeSupport) {
    d.av1 = d.av1 && client_windows::d3d::hardware_av1();
}

struct QtWindows {
    inner: Windows,
    sink: Sink,
    passthrough: Arc<AtomicBool>,
    slot: Arc<Mutex<Option<Session>>>,
    surface: Arc<Surface>,
    ended: Arc<AtomicBool>,
}

impl Platform for QtWindows {
    fn client_os(&self) -> Os {
        Os::Windows
    }
    fn adjust_decode_support(&self, d: &mut msg::DecodeSupport) {
        self.inner.adjust_decode_support(d);
    }
    fn key_table(&self, host_os: Os) -> Vec<KeyRule> {
        if self.passthrough.load(Ordering::Relaxed) { vec![] } else { remote_core::keymap::default_rules(Os::Windows, host_os) }
    }
    fn has_default_route(&self) -> bool {
        true
    }
    fn waker(&self) -> Arc<dyn Waker> {
        self.inner.waker()
    }
    fn clipboard(&self) -> Option<Arc<dyn ClipboardBackend>> {
        self.inner.clipboard()
    }
    fn on_status(&self, s: SessionStatus) {
        if matches!(s, SessionStatus::Ended(_) | SessionStatus::ConnectFailed(_)) {
            self.ended.store(true, Ordering::SeqCst);
        }
        (self.sink)(status_json(&s));
    }
    fn start_services(&self, app: &Arc<App>) {
        *self.slot.lock().unwrap() = Some(Session { app: app.clone(), surface: self.surface.clone() });
        self.inner.start_services(app);
    }
    fn run_window(&self, app: Arc<App>, rx: Receiver<WinMsg>, opts: WindowOptions) -> anyhow::Result<()> {
        // A tee: the app learns of the secure screen and the stream size; the window gets everything.
        let (tx, rx2) = std::sync::mpsc::channel();
        let (sink, wake) = (self.sink.clone(), self.inner.waker());
        std::thread::spawn(move || {
            while let Ok(m) = rx.recv() {
                match &m {
                    WinMsg::SecureScreen(on) => sink(json!({"ev": "secure", "on": on})),
                    WinMsg::Stream(s) => sink(json!({"ev": "stream", "width": s.width, "height": s.height})),
                    _ => {}
                }
                if tx.send(m).is_err() {
                    break;
                }
                wake.wake();
            }
        });
        self.inner.run_window(app, rx2, opts)
    }
}

fn num(o: &Value, k: &str) -> Option<f64> {
    o.get(k).and_then(|v| v.as_f64())
}

pub fn start(r: &Arc<Rq>, o: &Value, _display: *mut std::ffi::c_void, hwnd: *mut std::ffi::c_void) -> i32 {
    let Some(host) = o.get("host").and_then(|v| v.as_str()).map(str::to_string) else { return -1 };
    if hwnd.is_null() {
        return -1;
    }
    let sink = r.sink.clone();
    let fullscreen = o.get("fullscreen").and_then(|v| v.as_bool()).unwrap_or(false);
    let esink = sink.clone();
    let embed = Arc::new(Embed::new(
        hwnd as isize,
        EmbedState {
            origin: (num(o, "x").unwrap_or(0.0) as i32, num(o, "y").unwrap_or(0.0) as i32),
            size: (num(o, "width").unwrap_or(1280.0) as i32, num(o, "height").unwrap_or(800.0) as i32),
            fullscreen,
            capture: true,
            blocked: false,
        },
        Box::new(move |e| {
            esink(match e {
                EmbedEvent::Capture(on) => json!({"ev": "capture", "on": on}),
                EmbedEvent::LeaveChord => json!({"ev": "leave_chord"}),
                EmbedEvent::VideoClick => json!({"ev": "video_click"}),
                EmbedEvent::MenuKey(vk) => json!({"ev": "menu_key", "vk": vk}),
            })
        }),
    ));
    r.key_passthrough.store(o.get("key_passthrough").and_then(|v| v.as_bool()).unwrap_or(false), Ordering::Relaxed);
    let inner = match Windows::embedded(AudioOptions { enabled: true, log: None }, embed.clone()) {
        Ok(w) => w,
        Err(_) => return -1,
    };
    let ended = Arc::new(AtomicBool::new(false));
    let platform = Arc::new(QtWindows {
        inner,
        sink: sink.clone(),
        passthrough: r.key_passthrough.clone(),
        slot: r.session.clone(),
        surface: Arc::new(Surface { embed: embed.clone() }),
        ended: ended.clone(),
    });
    let mut net: NetOptions = r.net();
    net.fps = num(o, "fps").map_or(60, |f| f as u32);
    let opts = Options {
        net,
        host: Some(host),
        headless: false,
        decoder: HwPref::parse(o.get("decoder").and_then(|v| v.as_str()).unwrap_or("auto")).unwrap_or(HwPref::Auto),
        stats_dir: r.data.join("stats"),
        click: None,
        secs: None,
        fps: num(o, "fps").map(|f| f as u32),
        max_mbps: num(o, "max_mbps").filter(|m| *m > 0.0),
        takeover: o.get("takeover").and_then(|v| v.as_bool()).unwrap_or(false),
        input_log: None,
        zero_copy: true,
        window: WindowOptions { title: "Sodium Remote".into(), fullscreen, hud: false, fixed: o.get("fixed").and_then(|v| v.as_bool()).unwrap_or(false), size: None, ppp: None },
        codec: codec_choice(o.get("codec").and_then(|v| v.as_str()).unwrap_or("auto")),
        script: None,
        dump_frames: None,
        thumb: o.get("thumb").and_then(|v| v.as_str()).map(std::path::PathBuf::from),
    };
    if r.running.swap(true, Ordering::SeqCst) {
        return -1;
    }
    if let Some(c) = r.browse.lock().unwrap().take() {
        r.rt.block_on(c.shutdown());
    }
    let r2 = r.clone();
    std::thread::Builder::new()
        .name("remote-session".into())
        .spawn(move || {
            client_windows::platform::init_embedded();
            let mut opts = opts;
            // Draw straight from the decoder's textures where the GPU allows (else pictures are copied).
            client_windows::d3d::decode_adapter();
            opts.zero_copy = client_windows::d3d::share_with_decoder();
            let code = match client::run(opts, platform) {
                Ok(c) => c,
                Err(e) => {
                    (r2.sink)(json!({"ev": "error", "where": "session", "detail": format!("{e:#}")}));
                    1
                }
            };
            *r2.session.lock().unwrap() = None;
            r2.running.store(false, Ordering::SeqCst);
            if !ended.load(Ordering::SeqCst) {
                (r2.sink)(json!({"ev": "status", "state": "ended", "reason": {"kind": "error", "detail": "no session"}}));
            }
            (r2.sink)(json!({"ev": "session_over", "code": code}));
            r2.start_browse();
            r2.refresh();
        })
        .map(|_| 0)
        .unwrap_or(-1)
}
