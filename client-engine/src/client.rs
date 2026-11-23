// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The client's threads: the network (tokio, `remote_core::connect::Client`),
//! the decoder, the window (or nothing, headless), the connection timeline and
//! the click test. Also `pair`.

use crate::connect::{self, NetOptions};
use crate::decode::{Decoder, HwPref, sniff_codec};
use crate::latency::{Action, ClickConfig, ClickTest};
use crate::msg::{self, ClientDisplay, DisconnectReason, InputEvent, Msg};
use crate::proto::{FrameMeta, StreamInfo, button_from_evdev};
use crate::stats::{FrameRec, Stats};
use crate::status::LinkView;
use crate::{App, Platform, WinMsg, WindowOptions};
use anyhow::{Context, Result};
use remote_core::ReceivedFrame;
use remote_core::connect::{ClientEvent, ConnectError, PairError};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Exit codes: the user ended it (quit, `--secs`, the click test, or
/// `Disconnected(User)`); the session ended otherwise; it never connected.
pub const EXIT_ENDED: i32 = 3;
pub const EXIT_NO_CONNECT: i32 = 4;

pub struct Options {
    pub net: NetOptions,
    /// The paired host: its full ID, its code, or its LAN address `IP[:PORT]` (the ID stored
    /// when it was paired); default: the only one.
    pub host: Option<String>,
    pub headless: bool,
    pub decoder: HwPref,
    pub stats_dir: PathBuf,
    pub click: Option<ClickConfig>,
    pub secs: Option<u64>,
    /// `StreamSettings.refresh_mhz` (default 0: the display's rate).
    pub fps: Option<u32>,
    /// `StreamSettings.bandwidth_limit_kbps` (default: Auto).
    pub max_mbps: Option<f64>,
    /// Answer `Busy` by connecting again with takeover.
    pub takeover: bool,
    /// Log every input event sent (CSV: client µs, event).
    pub input_log: Option<PathBuf>,
    /// Keep NVDEC pictures on the GPU and copy them CUDA → GL (default on).
    pub zero_copy: bool,
    pub window: WindowOptions,
    /// A forced codec (Automatic / HEVC / H.264): clears the other `DecodeSupport` bits.
    pub codec: CodecChoice,
    /// `--script FILE`: timed inputs (see `script`).
    pub script: Option<PathBuf>,
    /// `--dump-frames DIR --dump-every N`: decoded frames as PNG.
    pub dump_frames: Option<(PathBuf, u32)>,
    /// Where to leave a small picture of the last frame when the session ends (the Qt app's computer cards).
    pub thumb: Option<PathBuf>,
}

/// The codec setting: Automatic lets the host pick among what the client decodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecChoice {
    Auto,
    Hevc,
    H264,
}

impl CodecChoice {
    pub fn parse(s: &str) -> Option<CodecChoice> {
        Some(match s {
            "auto" => CodecChoice::Auto,
            "hevc" => CodecChoice::Hevc,
            "h264" => CodecChoice::H264,
            _ => return None,
        })
    }

    /// Narrows what the probe found to this choice (AV1 stays only for Automatic:
    /// "AV1 only where both sides have it" is the host's pick among what is left).
    pub fn apply(self, d: &mut msg::DecodeSupport) {
        match self {
            CodecChoice::Auto => {}
            CodecChoice::Hevc => {
                d.h264 = false;
                d.av1 = false;
            }
            CodecChoice::H264 => {
                d.hevc = false;
                d.hevc_444 = false;
                d.av1 = false;
            }
        }
    }
}

enum DecMsg {
    /// A frame and its session's offset to the client clock (µs).
    Frame(ReceivedFrame, i64),
    Stream(StreamInfo),
}

const BTN_LEFT: u32 = 0x110;
/// Headless: the 13-inch MacBook Air the default size stands for.
const HEADLESS_SIZE: (u32, u32) = (2560, 1664);
const HEADLESS_PPP: f32 = 1.741;

/// The decoder thread: frames in order, pictures out to the presenter.
/// The thumbnail's width: the card's picture at about twice its size.
const THUMB_W: u32 = 480;

fn decode_loop(app: Arc<App>, rx: mpsc::Receiver<DecMsg>, mut pref: HwPref, windowed: bool, keep_cuda: bool, dump: Option<(PathBuf, u32)>, thumb: Option<PathBuf>) {
    let mut dumped = 0u64;
    // The last frame, small, refreshed every 2 s and written when the session ends.
    let mut last_thumb: Option<(u32, u32, Vec<u8>)> = None;
    let mut thumb_at: Option<Instant> = None;
    let mut dec: Option<Decoder> = None;
    let mut stream: Option<StreamInfo> = None;
    // A fresh decoder needs a keyframe; the network's own chain is `decodable`.
    let mut need_key = true;
    let mut last_req: Option<Instant> = None;
    let mut ok_frames = 0u64;
    // Decodable frames in a row that gave no picture: a decoder missing its
    // references (an intra refresh after a long loss, which FFmpeg's NVDEC path
    // can't rebuild from) says nothing else, so three of them ask for a keyframe.
    let mut no_picture = 0u32;
    let request_keyframe = |last: &mut Option<Instant>| {
        if last.is_none_or(|t| t.elapsed() > Duration::from_millis(500)) {
            app.send_msg(Msg::KeyframeRequest);
            *last = Some(Instant::now());
        }
    };
    let open = |codec, pref, stream: Option<StreamInfo>| match Decoder::open(codec, pref) {
        Ok(mut d) => {
            // NVDEC pictures stay on the GPU: the window copies them CUDA → GL,
            // and the click test reads just its probe.
            d.keep_cuda(keep_cuda);
            eprintln!("remote-client: decoding {codec:?} with {}", d.backend);
            app.stats.set_decoder(&d.backend, stream);
            Some(d)
        }
        Err(e) => {
            eprintln!("remote-client: {e:#}");
            None
        }
    };
    // Frames may never stop coming: check for quitting on every one.
    while !app.quitting() {
        let msg = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        let (f, off) = match msg {
            DecMsg::Stream(s) => {
                if stream.is_some_and(|o| o.codec != s.codec) {
                    dec = None;
                    need_key = true;
                }
                stream = Some(s);
                app.stats.set_decoder(dec.as_ref().map_or("", |d| d.backend.as_str()), stream);
                // Open the decoder now (CUDA's device setup takes ~0.1 s), not at the keyframe.
                if dec.is_none() {
                    dec = open(s.codec, pref, stream);
                    ok_frames = 0;
                }
                // A resumed session (a new process taking over a held one) sends frames only on
                // damage: a still desktop would never give this fresh decoder its keyframe.
                if need_key {
                    request_keyframe(&mut last_req);
                }
                continue;
            }
            DecMsg::Frame(f, off) => (f, off),
        };
        // Wire v2: a lost frame breaks the chain only until a frame that is
        // decodable again (a reference to a good one, a recovery point, a keyframe).
        if !f.decodable {
            app.stats.on_undecodable();
            continue;
        }
        if need_key && !f.keyframe {
            request_keyframe(&mut last_req);
            continue;
        }
        let meta = FrameMeta::decode(&f.meta);
        if dec.is_none() {
            let codec = stream.map(|s| s.codec).unwrap_or_else(|| sniff_codec(&f.data));
            dec = open(codec, pref, stream);
            ok_frames = 0;
            if dec.is_none() {
                continue;
            }
        }
        let d = dec.as_mut().unwrap();
        let decode_start = app.clock.now_us() as i64;
        // A picture for the thumbnail is read back to system memory (NVDEC pictures otherwise stay on the GPU),
        // once every 2 s.
        let thumb_due = thumb.is_some() && thumb_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(2));
        if thumb_due && keep_cuda {
            d.keep_cuda(false);
        }
        let decoded_ok = d.decode(&f.data);
        match decoded_ok {
            Ok(true) => {
                no_picture = 0;
                let decoded = app.clock.now_us() as i64;
                let (pic, transfer_ms) = match d.picture() {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("remote-client: picture: {e:#}");
                        continue;
                    }
                };
                if thumb_due && keep_cuda {
                    d.keep_cuda(true);
                }
                let ready = app.clock.now_us() as i64;
                need_key = false;
                ok_frames += 1;
                if ok_frames == 1 {
                    eprintln!("remote-client: first picture from {} ({:?}, hw {})", d.backend, pic.format, pic.hw);
                    app.stats.set_decoder(&d.backend, stream);
                }
                let mut rec = FrameRec {
                    frame_id: f.frame_id,
                    keyframe: f.keyframe,
                    bytes: f.data.len(),
                    send: f.send_local_us.map(|s| s + off),
                    first_pkt: f.first_arrival_us as i64 + off,
                    last_pkt: f.last_arrival_us as i64 + off,
                    decode_start,
                    decoded,
                    ready,
                    transfer_ms,
                    ..Default::default()
                };
                if let (Some(s), Some(local)) = (meta.stamps, f.send_local_us) {
                    // Host clock minus client clock, through the frame's own send time.
                    rec.set_host(&s, f.send_us as i64 - (local + off));
                }
                if let Some(c) = &app.click {
                    let (x, y, w, h) = c.probe();
                    if let Some(l) = pic.mean_luma(x, y, w, h) {
                        c.on_picture(&rec, l);
                    }
                }
                let id = rec.frame_id;
                app.first_picture.store(true, std::sync::atomic::Ordering::SeqCst);
                if let Some((dir, every)) = &dump {
                    if (ok_frames - 1) % (*every).max(1) as u64 == 0 {
                        match crate::dump::write_png(dir, id, &pic) {
                            Ok(()) => dumped += 1,
                            Err(e) => eprintln!("remote-client: dump-frames: {e:#}"),
                        }
                    }
                }
                if thumb.is_some() && thumb_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
                    if let Some(t) = crate::dump::thumbnail_rgb(&pic, THUMB_W) {
                        last_thumb = Some(t);
                        thumb_at = Some(Instant::now());
                    }
                }
                app.stats.on_decoded(rec);
                if windowed {
                    app.latest.put(pic, id);
                    app.wake.wake();
                }
            }
            Ok(false) => {
                no_picture += 1;
                if no_picture >= 3 && ok_frames > 0 {
                    eprintln!("remote-client: {no_picture} frames gave no picture: asking for a keyframe");
                    no_picture = 0;
                    need_key = true;
                    last_req = None;
                    request_keyframe(&mut last_req);
                }
            }
            Err(e) => {
                eprintln!("remote-client: decode error in frame {} ({}): {e:#}", f.frame_id, d.backend);
                app.stats.on_decode_error();
                if d.backend.contains('+') && ok_frames == 0 && pref != HwPref::Software {
                    eprintln!("remote-client: hardware decode failed at once; software from now on");
                    pref = HwPref::Software;
                }
                // A reset decoder has no references: it needs a keyframe.
                dec = None;
                need_key = true;
                last_req = None;
                request_keyframe(&mut last_req);
            }
        }
    }
    let _ = dumped;
    if let (Some(path), Some((w, h, rgb))) = (&thumb, &last_thumb) {
        if let Err(e) = crate::dump::write_rgb_png(path, *w, *h, rgb) {
            eprintln!("remote-client: thumbnail: {e:#}");
        }
    }
}

/// Connects (answering `Busy` with a takeover if asked), then routes the
/// core's events to the decoder, the window, the stats and the timeline.
async fn session(
    app: Arc<App>,
    mut events: tokio::sync::mpsc::UnboundedReceiver<ClientEvent>,
    host: remote_core::keys::DeviceId,
    o: SessionOpts,
    dec: mpsc::Sender<DecMsg>,
    win: Option<mpsc::Sender<WinMsg>>,
    platform: Arc<dyn Platform>,
) {
    // The window publishes its display after its first configure.
    let display = loop {
        if let Some(d) = *app.display.lock().unwrap() {
            break d;
        }
        if app.quitting() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let hello = msg::Hello {
        version: msg::VERSION,
        min_version: msg::MIN_VERSION,
        name: String::new(), // the core fills in the device name
        display,
        decode: *app.decode_support.lock().unwrap(),
        client_os: platform.client_os(),
        client_os_flavor: platform.client_os_flavor(),
        settings: msg::StreamSettings {
            refresh_mhz: o.fps.map_or(0, |f| f * 1000),
            bandwidth_limit_kbps: o.max_mbps.map_or(0, |m| (m * 1000.0).round() as u32),
            ..Default::default()
        },
    };
    app.stats.on_connect_start();
    platform.on_status(crate::SessionStatus::Connecting);
    eprintln!("remote-client: connecting to {} ({})", host.code(), host);
    let mut r = app.client.connect(host, hello.clone(), false).await;
    if let Err(ConnectError::Busy { by }) = &r {
        if o.takeover {
            eprintln!("remote-client: in use by {by}; taking over");
            r = app.client.connect(host, hello, true).await;
        }
    }
    if let Err(e) = r {
        platform.on_status(crate::SessionStatus::ConnectFailed(e.clone()));
        eprintln!(
            "remote-client: couldn't connect: {}",
            match &e {
                ConnectError::Busy { by } => format!("in use by {by} (--takeover takes over)"),
                e => format!("{e:?}"),
            }
        );
        app.quit_with(EXIT_NO_CONNECT);
        return;
    }
    // The window may have changed between Hello and the core taking the
    // target (its Resize then had nowhere to go): send the latest.
    if let Some(d) = *app.display.lock().unwrap() {
        if d != display && app.live_resize() {
            app.send_msg(Msg::Resize(d));
        }
    }
    // PC passthrough: keys pass as pressed. The core sends it
    // after Hello on every (re)connect.
    app.send_msg(Msg::KeyTable(vec![]));

    let to_win = |m: WinMsg| {
        if let Some(w) = &win {
            let _ = w.send(m);
            app.wake.wake();
        }
    };
    let mut offset = 0i64;
    while let Some(e) = events.recv().await {
        match e {
            ClientEvent::Connected { route } => {
                platform.on_status(crate::SessionStatus::Connected { route: route.clone() });
                let epoch = app.client.session_epoch().unwrap_or_else(Instant::now);
                offset = app.on_connected(epoch).offset_us;
                app.stats.on_connected();
                {
                    let mut t = app.timeline.lock().unwrap();
                    t.lost_at = None;
                }
                eprintln!(
                    "remote-client: connected{}",
                    route.map_or(String::new(), |r| format!(" through {} {} -> {}", crate::stats::route_name(&r), r.local, r.remote))
                );
            }
            ClientEvent::Control(m) => match m {
                Msg::Welcome(w) => {
                    eprintln!(
                        "remote-client: welcome from {:?}: {}x{} @{:.3} Hz {:?}, scale {} of {:?}{}{}",
                        w.name,
                        w.format.width,
                        w.format.height,
                        w.format.refresh_mhz as f64 / 1000.0,
                        w.format.codec,
                        w.format.scale,
                        w.scales,
                        w.took_over_from.as_ref().map_or(String::new(), |b| format!(", took over from {b}")),
                        if w.resumed { ", resumed" } else { "" }
                    );
                    if w.view_only {
                        eprintln!("remote-client: the host can't take input (view only): show the \"View only\" chip");
                    }
                    eprintln!(
                        "remote-client: host is {:?}, {:?} display mode, live resize {}, display {} of {:?}",
                        w.host_os, w.display_mode, w.live_resize, w.current_display, w.displays
                    );
                    app.set_live_resize(w.live_resize);
                    // The key table for this pair (empty is passthrough: already sent).
                    let rules = platform.key_table(w.host_os);
                    if !rules.is_empty() {
                        app.send_msg(Msg::KeyTable(rules));
                    }
                    *app.host.lock().unwrap() = Some(w.clone());
                    platform.on_status(crate::SessionStatus::Welcome(Box::new(w.clone())));
                    let s = StreamInfo::from(&w.format);
                    let _ = dec.send(DecMsg::Stream(s));
                    to_win(WinMsg::Stream(s));
                }
                Msg::SecureScreen(on) => {
                    eprintln!("remote-client: secure screen {}", if on { "up: the host can't show or drive it" } else { "gone" });
                    app.secure_screen.store(on, std::sync::atomic::Ordering::SeqCst);
                    to_win(WinMsg::SecureScreen(on));
                }
                m @ (Msg::ClipboardOffer(_) | Msg::ClipboardRequest { .. } | Msg::ClipboardData(_)) => {
                    if let Some(c) = &app.clipboard {
                        c.on_msg(&m);
                    }
                }
                Msg::Format(f) => {
                    eprintln!("remote-client: format {}x{} {:?}, scale {}", f.width, f.height, f.codec, f.scale);
                    let s = StreamInfo::from(&f);
                    let _ = dec.send(DecMsg::Stream(s));
                    to_win(WinMsg::Stream(s));
                }
                Msg::PointerLock(l) => to_win(WinMsg::PointerLock(l)),
                Msg::Stats(s) => {
                    if let Some(h) = s.host {
                        app.stats.on_host_stats(h);
                    }
                }
                Msg::TookOver { from } => eprintln!("remote-client: took over from {from}"),
                Msg::Disconnect(DisconnectReason::PermissionMissing { screen_recording, accessibility }) => eprintln!(
                    "remote-client: the host's OS hasn't allowed {}: show the permission card",
                    match (screen_recording, accessibility) {
                        (true, true) => "Screen Recording and Accessibility",
                        (true, false) => "Screen Recording",
                        (false, _) => "Accessibility",
                    }
                ),
                Msg::Disconnect(r) => eprintln!("remote-client: the host disconnected: {r:?}"),
                _ => {}
            },
            ClientEvent::VideoFrame(f) => {
                let _ = dec.send(DecMsg::Frame(f, offset));
            }
            ClientEvent::VideoFrameLost { .. } => app.stats.on_lost(),
            ClientEvent::CursorShape { hash, image } => to_win(WinMsg::CursorShape { hash, image }),
            ClientEvent::CursorPosition { x, y, visible, shape } => to_win(WinMsg::CursorPos { x, y, visible, hash: shape }),
            ClientEvent::Route(r) => {
                eprintln!("remote-client: route {} {} -> {} rtt {:.1} ms", crate::stats::route_name(&r), r.local, r.remote, r.rtt_ms);
                app.stats.on_route(&r);
            }
            ClientEvent::Stats(s) => app.stats.on_session_stats(s),
            ClientEvent::Reconnecting { since } => {
                platform.on_status(crate::SessionStatus::Reconnecting);
                let mut t = app.timeline.lock().unwrap();
                if t.lost_at.is_none() {
                    eprintln!("remote-client: connection lost {:.2} s ago (the last packet); reconnecting", since.as_secs_f64());
                }
                t.lost_at = Some(Instant::now() - since);
            }
            ClientEvent::Disconnected { reason } => {
                eprintln!("remote-client: disconnected: {reason:?}");
                app.timeline.lock().unwrap().ended = true;
                platform.on_status(crate::SessionStatus::Ended(reason.clone()));
                app.quit_with(if reason == DisconnectReason::User { 0 } else { EXIT_ENDED });
                break;
            }
            ClientEvent::Rendezvous { online, error } => {
                eprintln!("remote-client: rendezvous {}{}", if online { "online" } else { "offline" }, error.map_or(String::new(), |e| format!(": {e}")));
            }
            ClientEvent::Revoked { host } => eprintln!("remote-client: {} revoked this device", host.code()),
            ClientEvent::Presence { .. } => {}
        }
    }
}

#[derive(Clone, Copy)]
struct SessionOpts {
    fps: Option<u32>,
    max_mbps: Option<f64>,
    takeover: bool,
}

/// The connection timeline: sampled every 100 ms, logged one
/// line per change, shown in the HUD.
async fn timeline(app: Arc<App>, platform: Arc<dyn Platform>) {
    let started = Instant::now();
    let mut kinds = Vec::new();
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if app.quitting() {
            return;
        }
        let live = app.client.link_status().map(|s| LinkView::from_status(&s, &mut kinds));
        let network = platform.has_default_route();
        let mut t = app.timeline.lock().unwrap();
        let since = live.as_ref().map(|l| l.silence).or_else(|| t.lost_at.map(|t0| t0.elapsed()));
        let Some(v) = t.now(live, network) else { return };
        if t.shown.as_ref() != Some(&v) {
            eprintln!(
                "remote-client: link {v} (t0 {} ago, run {:.1} s)",
                since.map_or("-".to_string(), |s| format!("{:.2} s", s.as_secs_f64())),
                started.elapsed().as_secs_f64()
            );
            t.shown = Some(v);
        }
    }
}

/// The click test's scheduler: clicks at random gaps once the first picture
/// is in, then an idle phase (the test surface still) for idle bandwidth.
async fn click_loop(app: Arc<App>, test: Arc<ClickTest>, idle: Arc<Mutex<Option<(f64, f64, f64)>>>) {
    let now = || app.clock.now_us() as i64;
    while !matches!(test.next(now()), Action::Click { .. }) {
        if app.quitting() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Let the stream settle after the first keyframe.
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let (x, y) = test.target();
    app.send_input(InputEvent::PointerAbs { x, y });
    loop {
        if app.quitting() {
            return;
        }
        match test.next(now()) {
            Action::Finished => {
                // The window starts 5 s after the last click: the refinement tail and the lone late frame are over
                //.
                tokio::time::sleep(Duration::from_millis(5000)).await;
                // The core counts bytes about once a second: the rate between two of its counts.
                let (b0, t0) = app.stats.bytes_received();
                let (mut last, mut worst) = ((b0, t0), 0.0f64);
                let end = std::time::Instant::now() + Duration::from_millis(test.cfg.idle_ms.max(2500));
                while std::time::Instant::now() < end && !app.quitting() {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let (b, t) = app.stats.bytes_received();
                    if t > last.1 {
                        let kbps = b.saturating_sub(last.0) as f64 * 8.0 / ((t - last.1) as f64 / 1e6) / 1e3;
                        worst = worst.max(kbps);
                        last = (b, t);
                    }
                }
                let (b1, t1) = last;
                if t1 > t0 {
                    let secs = (t1 - t0) as f64 / 1e6;
                    let kbps = b1.saturating_sub(b0) as f64 * 8.0 / secs / 1e3;
                    *idle.lock().unwrap() = Some((kbps, secs, worst));
                }
                if test.cfg.quit_when_done {
                    app.quit();
                }
                eprintln!("remote-client: click test done");
                return;
            }
            Action::Wait => tokio::time::sleep(Duration::from_millis(5)).await,
            Action::Click { id } => {
                let (lo, hi) = test.cfg.gap_ms;
                tokio::time::sleep(Duration::from_millis(rand::random_range(lo..=hi.max(lo)))).await;
                app.send_input(InputEvent::PointerAbs { x, y });
                tokio::time::sleep(Duration::from_millis(8)).await;
                let t = app.send_input(InputEvent::Button { button: button_from_evdev(BTN_LEFT), down: true });
                test.clicked(id, t as i64);
                tokio::time::sleep(Duration::from_millis(40)).await;
                app.send_input(InputEvent::Button { button: button_from_evdev(BTN_LEFT), down: false });
            }
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?)
}

/// `pair`: find the host with an open pairing window and pair with the code.
/// Returns the process exit code: 0 paired, 2 wrong code, 3 window closed,
/// 4 failed (or no host found).
pub fn pair(net: NetOptions, code: String, host: Option<String>, timeout: Duration) -> Result<i32> {
    let rt = runtime()?;
    rt.block_on(async move {
        let (client, _events) = connect::start(&net).await?;
        eprintln!("remote-client: this device is {} ({})", client.id().code(), client.id());
        let end = Instant::now() + timeout;
        if !client.has_rendezvous() {
            eprintln!("remote-client: no server configured; looking on the LAN only");
        } else if net.rendezvous && !client.wait_rendezvous(timeout.min(Duration::from_secs(10))).await {
            eprintln!("remote-client: the rendezvous isn't up; looking on the LAN only");
        }
        let target = loop {
            let targets = client.discover_pairing(Duration::from_secs(2)).await;
            let mine: Vec<_> = targets.iter().filter(|t| host.as_deref().is_none_or(|h| connect::names(&t.host, h))).collect();
            match mine.as_slice() {
                [t] => break Some((*t).clone()),
                [] => {}
                many => {
                    eprintln!(
                        "remote-client: {} hosts have a pairing window open; choose one with --host: {}",
                        many.len(),
                        many.iter().map(|t| format!("{} ({})", t.host.code(), t.name)).collect::<Vec<_>>().join(", ")
                    );
                    return Ok(4);
                }
            }
            if Instant::now() >= end {
                break None;
            }
        };
        let Some(t) = target else {
            eprintln!("remote-client: no host with an open pairing window in {} s", timeout.as_secs());
            client.shutdown().await;
            return Ok(4);
        };
        eprintln!(
            "remote-client: pairing with {:?} ({}) {}",
            t.name,
            t.host.code(),
            match (&t.lan, &t.rendezvous) {
                (Some((ips, port)), _) => format!("on the LAN at {ips:?}:{port}"),
                (None, Some(_)) => "through the rendezvous".to_string(),
                _ => String::new(),
            }
        );
        let r = client.pair(&t, code.trim()).await;
        client.shutdown().await;
        Ok(pair_exit(r))
    })
}

fn pair_exit(r: Result<remote_core::keys::PairedDevice, PairError>) -> i32 {
    match r {
        Ok(d) => {
            println!("paired {} {} {:?}", d.id, d.id.code(), d.name);
            0
        }
        Err(e) => {
            eprintln!(
                "remote-client: pairing failed: {}",
                match &e {
                    PairError::WrongCode => "wrong code".to_string(),
                    PairError::Closed(r) => format!("the pairing window closed ({r})"),
                    PairError::Failed(r) => r.clone(),
                }
            );
            match e {
                PairError::WrongCode => 2,
                PairError::Closed(_) => 3,
                PairError::Failed(_) => 4,
            }
        }
    }
}

/// `pair --address IP[:PORT]`: pair with the host whose LAN listener is there (default
/// port 47311), knowing only the address. The host's ID is learned from the pairing
/// exchange itself, bound by the code, and printed with the other `paired` fields. Same
/// exit codes as `pair`.
pub fn pair_by_address(net: NetOptions, code: String, address: &str) -> Result<i32> {
    let addr = remote_core::connect::parse_address(address).with_context(|| format!("--address IP[:PORT], not {address:?}"))?;
    let rt = runtime()?;
    rt.block_on(async move {
        let (client, _events) = connect::start(&net).await?;
        eprintln!("remote-client: this device is {} ({}); pairing by address {addr}", client.id().code(), client.id());
        let r = client.pair_by_address(addr, code.trim()).await;
        client.shutdown().await;
        Ok(pair_exit(r))
    })
}

/// `connect`: the session until it ends. Returns the process exit code.
pub fn run(o: Options, platform: Arc<dyn Platform>) -> Result<i32> {
    let rt = runtime()?;
    let windowed = !o.headless;
    let (client, events) = rt.block_on(connect::start(&o.net))?;
    let host = connect::pick_host_or_address(&client, o.host.as_deref())?;
    let clock = connect::SessionClock::now();
    let stats = Arc::new(Stats::new(&o.stats_dir, clock, windowed)?);
    eprintln!("remote-client: stats in {}", stats.path("*").display());
    let click = match &o.click {
        Some(c) => Some(Arc::new(ClickTest::new(c.clone(), windowed, &stats.path("clicks.csv"), clock.epoch_mono_ns)?)),
        None => None,
    };
    let input_log = match &o.input_log {
        Some(p) => Some(std::fs::File::create(p).with_context(|| format!("creating {}", p.display()))?),
        None => None,
    };
    let app = Arc::new(App::new(client.clone(), clock, stats, click, input_log, platform.waker(), platform.clipboard()));
    {
        // What this machine decodes, probed (never assumed), then the codec choice.
        let mut d = crate::decode::probe_support(o.decoder);
        platform.adjust_decode_support(&mut d);
        o.codec.apply(&mut d);
        eprintln!("remote-client: decodes hevc {} h264 {} av1 {} (codec choice {:?})", d.hevc, d.h264, d.av1, o.codec);
        *app.decode_support.lock().unwrap() = d;
    }
    if !windowed {
        let (w, h) = o.window.size.unwrap_or(HEADLESS_SIZE);
        let refresh = o.fps.unwrap_or(60) * 1000;
        *app.display.lock().unwrap() = Some(ClientDisplay {
            width: w,
            height: h,
            pixels_per_point: o.window.ppp.unwrap_or(HEADLESS_PPP),
            refresh_mhz: refresh,
            fullscreen: o.window.fullscreen,
            display_width: w,
            display_height: h,
        });
    }
    let (dec_tx, dec_rx) = mpsc::channel();
    let (win_tx, win_rx) = mpsc::channel();
    let dec_thread = {
        let app = app.clone();
        let (pref, keep_cuda, dump, thumb) = (o.decoder, o.zero_copy, o.dump_frames.clone(), o.thumb.clone());
        std::thread::Builder::new().name("decode".into()).spawn(move || decode_loop(app, dec_rx, pref, windowed, keep_cuda, dump, thumb))?
    };
    let sopts = SessionOpts { fps: o.fps, max_mbps: o.max_mbps, takeover: o.takeover };
    rt.spawn(session(app.clone(), events, host, sopts, dec_tx, windowed.then_some(win_tx), platform.clone()));
    rt.spawn(timeline(app.clone(), platform.clone()));
    {
        // Client stats for the host's HUD and logs, about once a second.
        let app = app.clone();
        rt.spawn(async move {
            let mut t = tokio::time::interval(Duration::from_secs(1));
            loop {
                t.tick().await;
                if let Some(c) = app.stats.client_stats() {
                    app.send_msg(Msg::Stats(msg::Stats { host: None, client: Some(c) }));
                }
            }
        });
    }
    let idle = Arc::new(Mutex::new(None));
    if let Some(t) = app.click.clone() {
        rt.spawn(click_loop(app.clone(), t, idle.clone()));
    }
    if let Some(secs) = o.secs {
        let app = app.clone();
        rt.spawn(async move {
            tokio::time::sleep(Duration::from_secs(secs)).await;
            app.quit();
        });
    }
    if let Some(p) = &o.script {
        let steps = crate::script::parse(&std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?)?;
        rt.spawn(crate::script::run(app.clone(), steps));
    }
    if let Some(c) = app.clipboard.clone() {
        let app = app.clone();
        rt.spawn(async move {
            while !app.quitting() {
                c.poll();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }
    {
        let app = app.clone();
        rt.spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                let (Ok(mut a), Ok(mut b)) = (signal(SignalKind::interrupt()), signal(SignalKind::terminate())) else { return };
                tokio::select! { _ = a.recv() => {}, _ = b.recv() => {} }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            app.quit();
        });
    }
    platform.start_services(&app);

    let result = if windowed {
        platform.run_window(app.clone(), win_rx, o.window.clone())
    } else {
        while !app.quitting() {
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    };
    app.quit();
    let _ = dec_thread.join();
    let _ = app.send_input(InputEvent::ReleaseAll);
    // Disconnect(User) to the host, then close.
    rt.block_on(client.shutdown());

    let (connected, first) = app.stats.connect_times();
    let idle = *idle.lock().unwrap();
    let mut extra = vec![];
    if let Some((kbps, secs, worst)) = idle {
        extra.push(("idle_kbps".to_string(), format!("{kbps:.2}")));
        extra.push(("idle_secs".to_string(), format!("{secs:.2}")));
        extra.push(("idle_worst_kbps".to_string(), format!("{worst:.2}")));
    }
    let summary = app.stats.finish(&extra);
    eprintln!("{summary}");
    if let Some(c) = &app.click {
        let ms = |v: Option<i64>| v.map_or("n/a".to_string(), |v| format!("{:.0} ms", v as f64 / 1e3));
        let rows = vec![
            (
                "Cursor".to_string(),
                "Drawn locally: no added latency".to_string(),
                if windowed {
                    (if cfg!(windows) { "local: the cursor channel's image as the window's cursor (HCURSOR)" } else { "local: the cursor channel's image as the Wayland cursor" }).to_string()
                } else {
                    "n/a (headless)".to_string()
                },
            ),
            (
                "Idle bandwidth".to_string(),
                "Under 50 kbps on a still desktop".to_string(),
                idle.map_or("n/a".into(), |(k, s, w)| format!("{k:.1} kbps received over {s:.1} s, test surface still (worst second {w:.1})")),
            ),
            (
                "Connect".to_string(),
                "First frame in under 1.5 s direct".to_string(),
                format!("connected {}, first frame decoded {} after connect was called", ms(connected), ms(first)),
            ),
        ];
        let context = format!(
            "{} client, {}",
            if windowed { "windowed" } else { "headless" },
            std::env::var("REMOTE_E2E_CONTEXT").unwrap_or_else(|_| "loopback".into())
        );
        let mut md = c.summary(&context, &rows);
        md.push_str("\n**All frames (p50 / p95 ms, n):**\n\n| Stage | p50 | p95 | n |\n|---|---|---|---|\n");
        for (name, p50, p95, n) in app.stats.stage_table() {
            if n > 0 {
                md.push_str(&format!("| {name} | {p50:.2} | {p95:.2} | {n} |\n"));
            }
        }
        std::fs::write(app.stats.path("latency.md"), &md)?;
        println!("{md}");
    }
    rt.shutdown_timeout(Duration::from_secs(1));
    result?;
    Ok(app.exit_code())
}
