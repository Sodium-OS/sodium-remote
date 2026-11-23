// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The Linux side of the Qt app: the stream as a Wayland subsurface below Qt's toplevel.

use crate::{Rq, Session, Sink, codec_choice, status_json};
use remote_client::window::embed::{Embed, EmbedEvent, EmbedState};
use remote_client::{Wake, window};
use remote_client::audio::{self, AudioOptions};
use remote_client::clip::WlClipboard;
use remote_engine::client::{self, Options};
use remote_engine::clipboard::ClipboardBackend;
use remote_engine::connect::NetOptions;
use remote_engine::decode::HwPref;
use remote_engine::msg::{self, KeyRule, Os};
use remote_engine::{App, Platform, SessionStatus, WinMsg, Waker, WindowOptions};
use serde_json::{Value, json};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

pub struct Surface {
    pub embed: Arc<Embed>,
}

/// Linux has nothing to add to the engine's probe (VA-API and NVDEC are probed by FFmpeg itself).
pub fn adjust(_d: &mut msg::DecodeSupport) {}

struct QtLinux {
    wake: Arc<Wake>,
    embed: Arc<Embed>,
    sink: Sink,
    passthrough: Arc<AtomicBool>,
    slot: Arc<Mutex<Option<Session>>>,
    surface: Arc<Surface>,
    ended: Arc<AtomicBool>,
    clip: Option<Arc<WlClipboard>>,
}

impl Platform for QtLinux {
    fn client_os(&self) -> Os {
        Os::Linux
    }
    fn client_os_flavor(&self) -> String {
        remote_core::os_release::flavor()
    }
    fn key_table(&self, host_os: Os) -> Vec<KeyRule> {
        if self.passthrough.load(Ordering::Relaxed) { vec![] } else { remote_core::keymap::default_rules(Os::Linux, host_os) }
    }
    fn has_default_route(&self) -> bool {
        remote_client::connect::has_default_route()
    }
    fn waker(&self) -> Arc<dyn Waker> {
        self.wake.clone()
    }
    fn clipboard(&self) -> Option<Arc<dyn ClipboardBackend>> {
        self.clip.clone().map(|c| c as Arc<dyn ClipboardBackend>)
    }
    fn on_status(&self, s: SessionStatus) {
        if matches!(s, SessionStatus::Ended(_) | SessionStatus::ConnectFailed(_)) {
            self.ended.store(true, Ordering::SeqCst);
        }
        (self.sink)(status_json(&s));
    }
    fn start_services(&self, app: &Arc<App>) {
        *self.slot.lock().unwrap() = Some(Session { app: app.clone(), surface: self.surface.clone() });
        audio::start(app, &AudioOptions { enabled: true, log: std::env::var_os("REMOTE_AUDIO_LOG").map(PathBuf::from) });
    }
    fn run_window(&self, app: Arc<App>, rx: Receiver<WinMsg>, opts: WindowOptions) -> anyhow::Result<()> {
        // A tee: the app learns of the secure screen and the stream size; the window gets everything.
        let (tx, rx2) = std::sync::mpsc::channel();
        let (sink, wake) = (self.sink.clone(), self.wake.clone());
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
        window::run_embedded(app, rx2, opts, self.wake.clone(), self.embed.clone())
    }
}

fn num(o: &Value, k: &str) -> Option<f64> {
    o.get(k).and_then(|v| v.as_f64())
}

pub fn start(r: &Arc<Rq>, o: &Value, wl_display: *mut c_void, wl_surface: *mut c_void) -> i32 {
    let Some(host) = o.get("host").and_then(|v| v.as_str()).map(str::to_string) else { return -1 };
    if wl_display.is_null() || wl_surface.is_null() {
        return -1;
    }
    let sink = r.sink.clone();
    let wake = match Wake::new() {
        Ok(w) => Arc::new(w),
        Err(_) => return -1,
    };
    let (w, h) = (num(o, "width").unwrap_or(1280.0) as i32, num(o, "height").unwrap_or(800.0) as i32);
    let scale = num(o, "scale").unwrap_or(1.0) as i32;
    let fullscreen = o.get("fullscreen").and_then(|v| v.as_bool()).unwrap_or(false);
    let esink = sink.clone();
    let embed = Arc::new(Embed::new(
        wl_display,
        wl_surface,
        EmbedState { rev: 1, size: (w, h), origin: (num(o, "x").unwrap_or(0.0) as i32, num(o, "y").unwrap_or(0.0) as i32), scale: scale.max(1), fullscreen, hud_rects: vec![], capture: true },
        Box::new(move |e| {
            esink(match e {
                EmbedEvent::Capture(on) => json!({"ev": "capture", "on": on}),
                EmbedEvent::LeaveChord => json!({"ev": "leave_chord"}),
                EmbedEvent::Closed => json!({"ev": "video_closed"}),
            })
        }),
    ));
    r.key_passthrough.store(o.get("key_passthrough").and_then(|v| v.as_bool()).unwrap_or(false), Ordering::Relaxed);
    let ended = Arc::new(AtomicBool::new(false));
    let platform = Arc::new(QtLinux {
        wake: wake.clone(),
        embed: embed.clone(),
        sink: sink.clone(),
        passthrough: r.key_passthrough.clone(),
        slot: r.session.clone(),
        surface: Arc::new(Surface { embed: embed.clone() }),
        ended: ended.clone(),
        clip: remote_client::services(true),
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
        window: WindowOptions { title: "Remote".into(), fullscreen, hud: false, fixed: o.get("fixed").and_then(|v| v.as_bool()).unwrap_or(false), size: None, ppp: num(o, "ppp").map(|p| p as f32) },
        codec: codec_choice(o.get("codec").and_then(|v| v.as_str()).unwrap_or("auto")),
        script: None,
        dump_frames: None,
        thumb: o.get("thumb").and_then(|v| v.as_str()).map(std::path::PathBuf::from),
    };
    if r.running.swap(true, Ordering::SeqCst) {
        return -1;
    }
    // One device identity: the browse client steps aside for the session.
    if let Some(c) = r.browse.lock().unwrap().take() {
        r.rt.block_on(c.shutdown());
    }
    let r2 = r.clone();
    std::thread::Builder::new()
        .name("remote-session".into())
        .spawn(move || {
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
