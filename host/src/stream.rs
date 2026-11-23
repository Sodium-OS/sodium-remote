// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The stream: joins remote-core's `connect::Host` (pairing, the rendezvous,
//! mDNS, ICE and the protocol), the media pipeline and the session actor
//!
//! - **Core → actor** ([`Stream::run_events`]): a client connected, was lost,
//!   left; its Resize, Settings, home mode and clipboard; pairing outcomes; the
//!   path and stats. Input doesn't go through the actor: keys go through the
//!   key table ([`KeyMapper`]) and straight to `Media::input`.
//! - **Actor → core** ([`CoreLink`]): Accepted becomes `Msg::Welcome`, display
//!   changes `Msg::Format`, the IME's caret and focus, the clipboard, audio, the
//!   pointer lock, and the end of a session `host.disconnect(reason)`.
//! - **Media → core**: encoded frames with their reference (core's frame ids
//!   mapped from media's pts) and the host's per-frame stamps appended to the
//!   frame meta; cursor shapes as `msg::CursorImage` with REMOTE-1's scale and
//!   positions; animation hints. Core's reference hints go back to media
//!   (`predict_from`, `request_keyframe`); a client's `KeyframeRequest` too.
//! - **Phase lock**: the client's `Msg::Vsync` reports become FRAME_CLOCK
//!   through `remote_core::phase::PhaseLock`.
//! - **Route demand**: 0.2 bit/pixel/frame at the session's mode, or the user's
//!   cap if lower, sent at every mode and settings change. Core's
//!   controller probes a quiet link up to it, and media orders refinement under
//!   0.6 × it (`RateInput::demand_bps`).

use std::{
    collections::VecDeque,
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use remote_core::{
    alloc::{FrameOpts, RefHint},
    connect::{Host, HostEvent as CoreEvent, PathKind, RouteInfo},
    keys::DeviceId,
    msg::{self, ClipFormat, ClipIncoming, ClipItem, CodecChoice, Msg},
    path::TurnTransport,
    phase::PhaseLock,
    rendezvous::codes,
    session::{OutgoingFrame, SessionStats},
};
use tokio::sync::mpsc;

use crate::{
    capture::{Rect, monotonic_ns},
    clipboard::ClipEntry,
    display::{Applied, HomeMode, Request, UiSize},
    ime::ImeEvent,
    input::{FocusWatch, KeyMapper, PluginInput, PluginLink, to_plugin},
    media::{Codec, EncodedFrame, Media, MediaConfig, MediaEvent, RateInput},
    session::{ClientLink, Cmd, ConnectRequest, EndReason, HostEvent, LinkStats, RelayView, Rendezvous},
};

/// Chroma for HEVC: what the client's decoder says (Auto), or forced (tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromaPref {
    Auto,
    C420,
    C444,
}

/// Bits per pixel per frame the route demand plans for.
const DEMAND_BPP: f64 = 0.2;
/// The encoder's rate until core has a budget.
const START_BPS: u64 = 8_000_000;

#[derive(Default)]
struct Live {
    device: Option<DeviceId>,
    hello: Option<msg::Hello>,
    settings: msg::StreamSettings,
    display: Option<msg::ClientDisplay>,
    applied: Option<Applied>,
    /// The latest path, for the notification and LinkStats.
    route: Option<RouteInfo>,
    stats: Option<SessionStats>,
    /// The route demand last sent to core (0: none yet).
    demand_bps: u64,
}

struct Running {
    media: Arc<Media>,
    codec: msg::Codec,
    fps: u32,
}

/// Per-second numbers for `Msg::Stats` and the pill.
#[derive(Default)]
struct Acc {
    frames: u32,
    bytes: u64,
    capture_us: Vec<u32>,
    convert_us: Vec<u32>,
    encode_us: Vec<u32>,
    qp: u8,
    fps: f64,
}

fn p50(v: &mut [u32]) -> u32 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

pub struct Stream {
    pub host: Host,
    actor: mpsc::UnboundedSender<Cmd>,
    base: MediaConfig,
    chroma: ChromaPref,
    media: Mutex<Option<Running>>,
    keys: Mutex<KeyMapper>,
    focus: FocusWatch,
    phase: Mutex<PhaseLock>,
    /// media pts → core frame id, newest last (this connection's ids).
    frames: Mutex<VecDeque<(u64, u32)>>,
    live: Mutex<Live>,
    acc: Mutex<Acc>,
    hint_seq: AtomicU64,
    audio_ts: AtomicU32,
    /// LOCAL_INPUT for this session (the actor sets it before media is up).
    local_mode: AtomicU32,
    /// The host clipboard entry last offered, for the client's requests.
    clip_out: Mutex<Option<(u32, ClipEntry)>>,
    clip_serial: AtomicU32,
    /// The client's clipboard being assembled from `ClipboardData` chunks.
    clip_in: Mutex<Option<ClipIncoming>>,
    /// Tests: every frame sent, and every input event received and injected, as
    /// CSV (the end-to-end harness joins them).
    frame_log: Option<Mutex<BufWriter<File>>>,
    input_log: Option<Mutex<BufWriter<File>>>,
    /// Tests: core's session stats, about once a second (the controller's target, its probes).
    link_log: Option<Mutex<BufWriter<File>>>,
    /// Tests: the video budget's encoder rate at every change (5 ms ticks: the probes' ramp).
    target_log: Option<Mutex<BufWriter<File>>>,
    me: Weak<Stream>,
}

impl Stream {
    pub fn new(
        host: Host,
        actor: mpsc::UnboundedSender<Cmd>,
        base: MediaConfig,
        chroma: ChromaPref,
        focus: FocusWatch,
        logs: Option<PathBuf>,
    ) -> anyhow::Result<Arc<Stream>> {
        let open = |name: &str, header: &str| -> anyhow::Result<Option<Mutex<BufWriter<File>>>> {
            let Some(dir) = &logs else { return Ok(None) };
            std::fs::create_dir_all(dir)?;
            let mut f = BufWriter::new(File::create(dir.join(name))?);
            writeln!(f, "{header}")?;
            Ok(Some(Mutex::new(f)))
        };
        let frame_log = open(
            "host-frames.csv",
            "pts,frame_id,class,keyframe,bytes,qp,t_rendered_ns,t_ready_ns,t_encoded_ns,t_sent_ns,readable_ms,lossless_ms,base_qp,changed_cells,refined_cells,kbps,settled,settled_ms,split",
        )?;
        let input_log = open("host-input.csv", "t_recv_ns,t_injected_ns,t_event_ns,event")?;
        let link_log = open(
            "host-link.csv",
            "t_ns,target_bps,encoder_bps,receive_rate_bps,queue_delay_ms,loss,cc_state,c_est_bps,demand_bps,probe_bps,probe_ceiling_bps,probes,probes_accepted,probes_refused,probes_decreased,probe_queue_max_ms,bytes_sent",
        )?;
        let target_log = open("host-target.csv", "t_ns,encoder_bps")?;
        Ok(Arc::new_cyclic(|me| Stream {
            host,
            actor,
            base,
            chroma,
            media: Mutex::new(None),
            keys: Mutex::new(KeyMapper::default()),
            focus,
            phase: Mutex::new(PhaseLock::new(60_000)),
            frames: Mutex::new(VecDeque::new()),
            live: Mutex::new(Live::default()),
            acc: Mutex::new(Acc::default()),
            hint_seq: AtomicU64::new(0),
            audio_ts: AtomicU32::new(0),
            local_mode: AtomicU32::new(0),
            clip_out: Mutex::new(None),
            clip_serial: AtomicU32::new(1),
            clip_in: Mutex::new(None),
            frame_log,
            input_log,
            link_log,
            target_log,
            me: me.clone(),
        }))
    }

    fn media(&self) -> Option<Arc<Media>> {
        self.media.lock().unwrap().as_ref().map(|r| r.media.clone())
    }

    // ---- core → actor ----------------------------------------------------

    /// Reads core's events for the life of the daemon.
    pub async fn run_events(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<CoreEvent>, rdv: Arc<CoreRendezvous>) {
        {
            // Reference hints, stats and the microphone run beside the events.
            let s = self.clone();
            tokio::spawn(async move { s.tick_loop().await });
        }
        while let Some(ev) = rx.recv().await {
            self.on_core(ev, &rdv);
        }
    }

    fn send_actor(&self, c: Cmd) {
        let _ = self.actor.send(c);
    }

    fn on_core(self: &Arc<Self>, ev: CoreEvent, rdv: &CoreRendezvous) {
        match ev {
            CoreEvent::Rendezvous { online, error } => {
                rdv.set_online(online, error);
                self.send_actor(Cmd::Refresh);
            }
            CoreEvent::PairingAttempt { .. } => {
                // Settings shows "A device is pairing…". Core sends nothing when the device leaves in the middle of
                // the run (its outcome events are the other way out), so a guard ends the line after 15 s.
                self.send_actor(Cmd::PairingAttempt(true));
                let actor = self.actor.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                    let _ = actor.send(Cmd::PairingAttempt(false));
                });
            }
            CoreEvent::PairingFailed { attempts_left } => self.send_actor(Cmd::PairFailed { attempts_left }),
            CoreEvent::Paired(d) => self.send_actor(Cmd::Paired { device: d.id.to_string(), name: d.name }),
            CoreEvent::PairingClosed { reason } => self.send_actor(Cmd::PairClosed { reason }),
            CoreEvent::ClientConnected { client, name, hello, took_over_from, resumed } => {
                eprintln!(
                    "remoted: {name} ({}) connected{}{}",
                    client.short(),
                    if resumed { ", resumed" } else { "" },
                    took_over_from.map(|f| format!(", took over from {f}")).unwrap_or_default()
                );
                let (display, route) = {
                    let mut l = self.live.lock().unwrap();
                    l.device = Some(client);
                    l.settings = hello.settings;
                    l.display = Some(hello.display);
                    l.hello = Some(hello);
                    (request_of(&l), l.route.as_ref().map(|r| (path_words(r), r.rtt_ms.round() as u32)))
                };
                // A new connection is a new session: frame ids start at 0, the
                // client's decoder is new, and its table comes after Hello.
                self.frames.lock().unwrap().clear();
                self.hint_seq.store(0, Ordering::Relaxed);
                self.keys.lock().unwrap().set_table(vec![]);
                if let Some(m) = self.media() {
                    m.request_keyframe();
                }
                let link = Arc::new(CoreLink { stream: self.clone(), resumed });
                self.send_actor(Cmd::Connect(ConnectRequest {
                    device: client.to_string(),
                    display,
                    takeover: true,
                    link,
                    route,
                }));
            }
            CoreEvent::ClientLost { client } => self.send_actor(Cmd::Lost { device: client.to_string() }),
            CoreEvent::HoldExpired { client } => self.send_actor(Cmd::Bye { device: client.to_string() }),
            // The actor's own takeover handles the old client; a Bye would end the session.
            CoreEvent::ClientDisconnected { reason: msg::DisconnectReason::TakenOver { .. }, .. } => {}
            CoreEvent::ClientDisconnected { client, reason } => {
                eprintln!("remoted: {} left: {reason:?}", client.short());
                self.send_actor(Cmd::Bye { device: client.to_string() });
            }
            CoreEvent::Input { event, at } => self.on_input(event, at),
            CoreEvent::Control(m) => self.on_control(m),
            CoreEvent::PeerLostFrame { .. } | CoreEvent::ReceiverReport { .. } => {}
            CoreEvent::Route(r) => {
                eprintln!("remoted: route {} {} -> {} ({:.1} ms)", path_words(&r), r.local, r.remote, r.rtt_ms);
                self.live.lock().unwrap().route = Some(r);
                self.push_link_stats();
            }
            CoreEvent::Stats(s) => {
                self.log_link(&s);
                self.live.lock().unwrap().stats = Some(s);
                self.push_link_stats();
            }
        }
    }

    fn device(&self) -> Option<String> {
        self.live.lock().unwrap().device.map(|d| d.to_string())
    }

    fn on_input(&self, event: msg::InputEvent, at: Option<Instant>) {
        let Some(device) = self.device() else { return };
        match event {
            msg::InputEvent::ImePreedit { text, cursor } => self.send_actor(Cmd::Preedit { device, text, cursor }),
            msg::InputEvent::ImeCommit { text } => self.send_actor(Cmd::Commit { device, text }),
            ev => {
                let t_recv = monotonic_ns();
                let t_ns = at.map_or(0, instant_to_mono);
                let class = self.focus.class();
                let out = to_plugin(&ev, t_ns, &mut self.keys.lock().unwrap(), &class);
                if let Some(m) = self.media() {
                    for p in out {
                        if let Err(e) = m.input(p) {
                            eprintln!("remoted: input: {e:#}");
                        }
                    }
                }
                if let Some(log) = &self.input_log {
                    let ev = format!("{ev:?}").replace(',', ";");
                    let _ = writeln!(log.lock().unwrap(), "{t_recv},{},{t_ns},{ev}", monotonic_ns());
                }
            }
        }
    }

    fn on_control(self: &Arc<Self>, m: Msg) {
        let Some(device) = self.device() else { return };
        match m {
            Msg::Resize(d) => {
                let display = {
                    let mut l = self.live.lock().unwrap();
                    l.display = Some(d);
                    request_of(&l)
                };
                self.send_actor(Cmd::Resize { device, display });
            }
            Msg::Settings(st) => {
                let display = {
                    let mut l = self.live.lock().unwrap();
                    l.settings = st;
                    request_of(&l)
                };
                self.send_route_demand(); // the user's cap may have changed
                self.send_actor(Cmd::Resize { device, display });
            }
            Msg::HomeMonitor(m) => self.send_actor(Cmd::ClientHomeMode { device, mode: home_mode(m) }),
            Msg::ClipboardOffer(o) => self.clip_offer_in(&device, o),
            Msg::ClipboardRequest { serial, format } => self.clip_request(serial, &format),
            Msg::ClipboardData(d) => self.clip_data_in(&device, d),
            Msg::KeyTable(t) => {
                eprintln!("remoted: key table with {} rules{}", t.len(), if t.is_empty() { " (PC passthrough)" } else { "" });
                self.keys.lock().unwrap().set_table(t);
            }
            Msg::KeyframeRequest => {
                eprintln!("remoted: the client asked for a keyframe");
                if let Some(m) = self.media() {
                    m.request_keyframe();
                }
            }
            Msg::Vsync(r) => {
                let vsync = self.host.peer_instant(r.vsync_us).map(|i| instant_to_mono(i) as i64);
                let set = self.phase.lock().unwrap().on_report(&r, vsync);
                if let (Some(fc), Some(m)) = (set, self.media()) {
                    eprintln!(
                        "remoted: frame clock phase {} µs, {} mHz, grace {} µs (lead p50 {} µs, p05 {} µs, {} frames)",
                        fc.phase_offset_ns / 1000,
                        fc.refresh_mhz,
                        fc.grace_ns / 1000,
                        r.lead_p50_us,
                        r.lead_p05_us,
                        r.frames
                    );
                    if let Err(e) = m.frame_clock(fc.phase_offset_ns, fc.refresh_mhz, fc.grace_ns) {
                        eprintln!("remoted: frame clock: {e:#}");
                    }
                }
            }
            Msg::Stats(_) | Msg::Hello(_) => {}
            other => eprintln!("remoted: ignoring {}", format!("{other:?}").split(['(', ' ', '{']).next().unwrap_or("")),
        }
    }

    fn push_link_stats(&self) {
        let (device, stats) = {
            let l = self.live.lock().unwrap();
            let Some(d) = l.device else { return };
            let (path, transport) = match l.route.as_ref().map(|r| &r.kind) {
                Some(PathKind::Lan) => ("lan", "udp"),
                Some(PathKind::Direct) => ("direct", "udp"),
                Some(PathKind::Vpn) => ("vpn", "udp"),
                Some(PathKind::Tailscale) => ("tailscale", "udp"),
                Some(PathKind::Relay { transport }) => (
                    "relay",
                    match transport {
                        TurnTransport::Udp => "udp",
                        TurnTransport::Tcp => "tcp",
                        TurnTransport::Tls => "tls",
                    },
                ),
                None => ("", ""),
            };
            let s = l.stats.as_ref();
            (
                d.to_string(),
                LinkStats {
                    path: path.into(),
                    transport: transport.into(),
                    network: l.route.as_ref().filter(|r| r.kind == PathKind::Vpn).and_then(|r| r.network.clone()).unwrap_or_default(),
                    rtt_ms: s.map_or(0.0, |s| s.rtt_ms).round() as u32,
                    bitrate_kbps: s.map_or(0, |s| s.target_bps / 1000) as u32,
                    fps: self.acc.lock().unwrap().fps,
                },
            )
        };
        self.send_actor(Cmd::Stats { device, stats });
    }

    /// Reference hints (every 5 ms), the host's stats to the client (every
    /// second) and the client's microphone (every 10 ms).
    async fn tick_loop(self: Arc<Self>) {
        let mut t = tokio::time::interval(Duration::from_millis(5));
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_stats = Instant::now();
        let mut last_mic = Instant::now();
        let mut mic_on = false;
        let mut last_budget = 0;
        loop {
            t.tick().await;
            if let (Some(log), Some(ls)) = (&self.target_log, self.host.link_status())
                && ls.budget.encoder_bps != last_budget
            {
                last_budget = ls.budget.encoder_bps;
                let _ = writeln!(log.lock().unwrap(), "{},{last_budget}", monotonic_ns());
            }
            if let Some(ls) = self.host.link_status()
                && ls.ref_hint_seq != self.hint_seq.swap(ls.ref_hint_seq, Ordering::Relaxed)
                && let (Some(hint), Some(m)) = (ls.ref_hint, self.media())
            {
                match hint {
                    RefHint::Reference(id) => {
                        let pts = self.frames.lock().unwrap().iter().rev().find(|(_, i)| *i == id).map(|(p, _)| *p);
                        eprintln!("remoted: reference hint: the client has frame {id} (pts {pts:?}) and nothing usable after it");
                        match pts {
                            Some(p) => m.predict_from(p),
                            None => m.request_keyframe(),
                        }
                    }
                    RefHint::Keyframe => {
                        eprintln!("remoted: reference hint: a keyframe");
                        m.request_keyframe()
                    }
                }
            }
            if let Some(buf) = self.host.mic_buffer() {
                let out = buf.lock().unwrap().pop(Instant::now());
                if let (Some(out), Some(device)) = (out, self.device()) {
                    let packet = match out.play {
                        remote_core::audio::AudioPlay::Packet(p) | remote_core::audio::AudioPlay::Fec(p) => Some(p),
                        remote_core::audio::AudioPlay::Conceal => None,
                    };
                    if let Some(packet) = packet {
                        if !mic_on {
                            mic_on = true;
                            self.send_actor(Cmd::MicOn { device: device.clone(), on: true });
                        }
                        last_mic = Instant::now();
                        self.send_actor(Cmd::Mic { device, packet });
                    }
                }
            }
            if mic_on && last_mic.elapsed() > Duration::from_secs(2) {
                mic_on = false;
                if let Some(device) = self.device() {
                    self.send_actor(Cmd::MicOn { device, on: false });
                }
            }
            if last_stats.elapsed() >= Duration::from_secs(1) {
                let secs = last_stats.elapsed().as_secs_f64();
                last_stats = Instant::now();
                let (readable, lossless, settled, end) = self.media().map_or((None, None, None, None), |m| {
                    let s = m.stats();
                    (s.readable_ms, s.lossless_ms, s.settled_ms, s.settled_end)
                });
                let (target_bps, c_est_bps, probe_ceiling_bps) = self
                    .live
                    .lock()
                    .unwrap()
                    .stats
                    .as_ref()
                    .map_or((0, 0, 0), |s| (s.target_bps, s.c_est_bps, s.probe_ceiling_bps));
                let hs = {
                    let mut a = self.acc.lock().unwrap();
                    a.fps = a.frames as f64 / secs;
                    let hs = msg::HostStats {
                        capture_us: p50(&mut a.capture_us),
                        convert_us: p50(&mut a.convert_us),
                        encode_us: p50(&mut a.encode_us),
                        send_us: 0,
                        fps: a.fps as f32,
                        bitrate_bps: (a.bytes as f64 * 8.0 / secs) as u64,
                        qp: a.qp,
                        optimised_look: false,
                        readable_ms: readable.map_or(0, |v| v.round() as u32),
                        lossless_ms: lossless.map_or(0, |v| v.round() as u32),
                        settled_ms: settled.map_or(0, |v| v.round() as u32),
                        settled_end: end.map_or(0, |e| e as u8),
                        target_bps,
                        c_est_bps,
                        probe_ceiling_bps,
                    };
                    *a = Acc { fps: a.fps, qp: a.qp, ..Acc::default() };
                    hs
                };
                self.host.send(&Msg::Stats(msg::Stats { host: Some(hs), client: None }));
            }
        }
    }

    // ---- the session's display → media ----------------------------------

    /// REMOTE-1 is up (or changed): the cursor's scale, the route demand, the
    /// phase lock, and media at the right codec and rate.
    fn on_display(&self, a: Applied) {
        let old = self.live.lock().unwrap().applied.replace(a);
        self.send_route_demand();
        if old.is_none_or(|o| o.refresh_mhz != a.refresh_mhz) {
            self.phase.lock().unwrap().reset(a.refresh_mhz);
        }
        self.ensure_media();
    }

    /// The route demand at the applied mode and the user's cap, to core (path
    /// choice, the controller's probes) and for media's ordering.
    fn send_route_demand(&self) {
        let demand = {
            let mut l = self.live.lock().unwrap();
            let Some(a) = l.applied else { return };
            let fps = (a.refresh_mhz / 1000).max(1);
            let mut demand = DEMAND_BPP * a.width as f64 * a.height as f64 * fps as f64;
            if l.settings.bandwidth_limit_kbps > 0 {
                demand = demand.min(l.settings.bandwidth_limit_kbps as f64 * 1000.0);
            }
            l.demand_bps = demand as u64;
            l.demand_bps
        };
        self.host.set_route_demand(demand);
    }

    fn wanted_codec(&self) -> msg::Codec {
        let l = self.live.lock().unwrap();
        pick_codec(l.settings.codec, l.hello.as_ref().map(|h| h.decode), self.chroma)
    }

    /// Starts media (on a thread: Vulkan, CUDA and NVENC take a moment), or
    /// restarts it when the codec or the stream rate changed. The plugin takes
    /// one connection, so the old one is gone before the new one connects.
    fn ensure_media(&self) {
        let Some(a) = self.live.lock().unwrap().applied else { return };
        let (codec, fps) = (self.wanted_codec(), (a.refresh_mhz / 1000).max(1));
        let old = {
            let mut g = self.media.lock().unwrap();
            if g.as_ref().is_some_and(|r| r.codec == codec && r.fps == fps) {
                return;
            }
            g.take()
        };
        let Some(me) = self.me.upgrade() else { return };
        let cfg = MediaConfig {
            codec: if codec == msg::Codec::Av1 { Codec::Av1 } else { Codec::Hevc },
            chroma444: codec == msg::Codec::Hevc444,
            fps,
            start_bps: START_BPS,
            ..self.base.clone()
        };
        let _ = std::thread::Builder::new().name("media-start".into()).spawn(move || {
            if let Some(old) = old {
                stop_media(old.media);
            }
            let weak = me.me.clone();
            let sink: crate::media::EventSink = Arc::new(move |ev| {
                if let Some(s) = weak.upgrade() {
                    s.on_media(ev);
                }
            });
            let t = Instant::now();
            match Media::start(cfg, sink) {
                Ok(m) => {
                    let weak = me.me.clone();
                    m.set_rate_source(Arc::new(move || weak.upgrade().map_or(RateInput::fixed(START_BPS), |s| s.rate_input())));
                    let m = Arc::new(m);
                    eprintln!("remoted: media {codec:?} at {fps} fps up in {} ms", t.elapsed().as_millis());
                    apply_local_input(&m, me.local_mode.load(Ordering::Relaxed));
                    *me.media.lock().unwrap() = Some(Running { media: m, codec, fps });
                    me.send_actor(Cmd::Media { capture: "plugin".into(), encoder_ready: true });
                    if let Some(fc) = me.phase.lock().unwrap().current() {
                        let _ = me.media().map(|m| m.frame_clock(fc.phase_offset_ns, fc.refresh_mhz, fc.grace_ns));
                    }
                }
                Err(e) => {
                    eprintln!("remoted: media didn't start: {e:#}");
                    me.host.disconnect(msg::DisconnectReason::Error(format!("the host's video couldn't start: {e}")));
                }
            }
        });
    }

    fn rate_input(&self) -> RateInput {
        let (user_cap, demand_bps) = {
            let l = self.live.lock().unwrap();
            (l.settings.bandwidth_limit_kbps > 0, l.demand_bps)
        };
        match self.host.link_status() {
            Some(ls) => RateInput {
                encoder_bps: ls.budget.encoder_bps.max(500_000),
                latency_critical: ls.budget.latency_critical,
                thin: ls.budget.thin,
                interval_remaining_bytes: ls.budget.interval_remaining_bytes,
                c_est_bps: ls.link.c_est_bps,
                split_ok: ls.link.split_ok,
                user_cap,
                demand_bps,
            },
            None => RateInput { user_cap, demand_bps, ..RateInput::fixed(START_BPS) },
        }
    }

    /// Tests: one line of core's session stats (`host-link.csv`).
    fn log_link(&self, s: &SessionStats) {
        let Some(log) = &self.link_log else { return };
        let demand = self.live.lock().unwrap().demand_bps;
        let _ = writeln!(
            log.lock().unwrap(),
            "{},{},{},{:.0},{:.2},{:.4},{:?},{},{demand},{},{},{},{},{},{},{:.2},{}",
            monotonic_ns(),
            s.target_bps,
            s.encoder_bps,
            s.receive_rate_bps,
            s.queue_delay_ms,
            s.loss,
            s.cc_state,
            s.c_est_bps,
            s.probe_bps,
            s.probe_ceiling_bps,
            s.probes,
            s.probes_accepted,
            s.probes_refused,
            s.probes_decreased,
            s.probe_queue_max_ms,
            s.bytes_sent
        );
    }

    // ---- media → core ------------------------------------------------------

    fn on_media(&self, ev: MediaEvent) {
        match ev {
            MediaEvent::Frame(f) => self.on_frame(f),
            MediaEvent::CursorShape(c) => {
                let mut image = c.image;
                image.scale = self.live.lock().unwrap().applied.map_or(1.0, |a| a.scale as f32);
                self.host.send_cursor_shape(c.hash, image);
            }
            MediaEvent::CursorPos { x, y, visible, shape, .. } => self.host.send_cursor_position(x as f32, y as f32, visible, shape),
            MediaEvent::Caret { visible, purpose, rect, .. } => {
                self.send_actor(Cmd::Caret { visible, purpose, x: rect.x, y: rect.y, w: rect.w, h: rect.h })
            }
            MediaEvent::PointerLock { state, rect } => {
                self.send_actor(Cmd::PointerLock { state, x: rect.x, y: rect.y, w: rect.w, h: rect.h })
            }
            MediaEvent::Animation(a) => self.host.set_animation(a),
            MediaEvent::PluginStats { .. } => {}
            MediaEvent::Stopped(why) => eprintln!("remoted: capture stopped: {why}"),
        }
    }

    fn on_frame(&self, f: EncodedFrame) {
        let mut meta = f.meta;
        // The host's stamps for the latency harness and the client's HUD:
        // TLV record 1 (video-pipeline.md, "Frame meta"), µs on this session's clock.
        let epoch_mono = self.host.session_epoch().map(|e| monotonic_ns().saturating_sub(e.elapsed().as_nanos() as u64));
        if let Some(e) = epoch_mono {
            let us = |t: u64| t.saturating_sub(e) / 1000;
            let converted = f.info.t_ready_ns + (f.info.convert_ms * 1e6) as u64;
            meta.push(1);
            meta.extend_from_slice(&32u16.to_be_bytes());
            for t in [f.t_rendered_ns, f.info.t_ready_ns, converted, f.t_encoded_ns] {
                meta.extend_from_slice(&us(t).to_be_bytes());
            }
        }
        let reference = f.reference.and_then(|pts| self.frames.lock().unwrap().iter().rev().find(|(p, _)| *p == pts).map(|(_, id)| *id));
        let opts = FrameOpts { reference, recovery_point: f.recovery_point };
        let bytes = f.data.len();
        let frame = OutgoingFrame { data: f.data, keyframe: f.keyframe, meta };
        let id = self.host.send_video_with(frame, opts);
        if let Some(id) = id {
            let mut fr = self.frames.lock().unwrap();
            fr.push_back((f.pts, id));
            if fr.len() > 256 {
                fr.pop_front();
            }
        }
        {
            let mut a = self.acc.lock().unwrap();
            a.frames += 1;
            a.bytes += bytes as u64;
            a.capture_us.push((f.info.t_ready_ns.saturating_sub(f.t_rendered_ns) / 1000) as u32);
            a.convert_us.push((f.info.convert_ms * 1000.0) as u32);
            a.encode_us.push((f.info.encode_ms * 1000.0) as u32);
            a.qp = f.info.avg_qp.min(255) as u8;
        }
        if let Some(log) = &self.frame_log {
            let opt = |v: Option<f64>| v.map_or(String::new(), |v| format!("{v:.1}"));
            let _ = writeln!(
                log.lock().unwrap(),
                "{},{},{},{},{bytes},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                f.pts,
                id.map_or(-1, |i| i as i64),
                f.class.wire(),
                f.keyframe as u8,
                f.info.avg_qp,
                f.t_rendered_ns,
                f.info.t_ready_ns,
                f.t_encoded_ns,
                monotonic_ns(),
                opt(f.info.readable_ms),
                opt(f.info.lossless_ms),
                f.info.base_qp,
                f.info.changed_cells,
                f.info.refined_cells,
                f.info.kbps,
                if f.info.lossless_ms.is_some() { "stop" } else { f.info.settled_end.map_or("", |e| e.as_str()) },
                opt(f.info.settled_ms),
                f.info.split as u8
            );
        }
    }

    // ---- clipboard ------------------------------------------------

    fn clip_offer_out(&self, entry: ClipEntry) {
        let serial = self.clip_serial.fetch_add(1, Ordering::Relaxed);
        let items = entry
            .items
            .iter()
            .map(|(mime, data)| ClipItem {
                format: format_of(mime),
                size: data.len() as u64,
                inline: (data.len() <= msg::CLIPBOARD_CHUNK).then(|| data.clone()),
            })
            .collect();
        self.host.send(&Msg::ClipboardOffer(msg::ClipboardOffer { serial, items, concealed: entry.concealed }));
        *self.clip_out.lock().unwrap() = Some((serial, entry));
    }

    fn clip_request(&self, serial: u32, format: &ClipFormat) {
        let g = self.clip_out.lock().unwrap();
        let Some((s, entry)) = g.as_ref() else { return };
        if *s != serial {
            return;
        }
        if let Some((_, data)) = entry.items.iter().find(|(m, _)| format_of(m) == *format) {
            for m in msg::ClipboardData::chunks(serial, format, data) {
                self.host.send(&m);
            }
        }
    }

    fn clip_offer_in(&self, device: &str, o: msg::ClipboardOffer) {
        // Only plain text, HTML and PNG from a client, as in host-desktop: any other type (a file list, a URI list) would
        // make a file manager act on the paste.
        let keep = |f: &ClipFormat| matches!(f, ClipFormat::Text | ClipFormat::Html | ClipFormat::Png);
        let Some((inc, asks)) = ClipIncoming::from_offer(&o, keep) else {
            *self.clip_in.lock().unwrap() = None;
            return;
        };
        for format in asks {
            self.host.send(&Msg::ClipboardRequest { serial: o.serial, format });
        }
        *self.clip_in.lock().unwrap() = Some(inc);
        self.clip_finish(device);
    }

    fn clip_data_in(&self, device: &str, d: msg::ClipboardData) {
        if let Some(inc) = self.clip_in.lock().unwrap().as_mut() {
            inc.add(&d);
        }
        self.clip_finish(device);
    }

    /// Every item complete: it becomes the host's selection.
    fn clip_finish(&self, device: &str) {
        let mut g = self.clip_in.lock().unwrap();
        if !g.as_ref().is_some_and(ClipIncoming::is_complete) {
            return;
        }
        let inc = g.take().unwrap();
        let entry = ClipEntry { concealed: inc.concealed, items: inc.into_items().into_iter().map(|(f, b)| (mime_of(&f), b)).collect() };
        self.send_actor(Cmd::Clipboard { device: device.to_string(), entry });
    }
}

fn apply_local_input(m: &Media, mode: u32) {
    match m.local_input(mode) {
        Ok(true) => eprintln!("remoted: the desk's own input: mode {mode}"),
        Ok(false) if mode != 0 => eprintln!("remoted: the plugin predates LOCAL_INPUT (capture-ipc v4): the desk's input still works"),
        Ok(false) => {}
        Err(e) => eprintln!("remoted: local input: {e:#}"),
    }
}

/// Drops the last reference to a media pipeline, which STOPs it and closes the
/// plugin connection (an input in flight may hold one for a moment).
fn stop_media(m: Arc<Media>) {
    let t = Instant::now();
    let mut m = m;
    loop {
        match Arc::try_unwrap(m) {
            Ok(media) => return media.stop(),
            Err(back) if t.elapsed() < Duration::from_secs(1) => {
                m = back;
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(_) => return,
        }
    }
}

/// The plugin, as the actor sees it: input resets, the desk's input mode and
/// the end of the stream.
impl PluginLink for Stream {
    fn input(&self, ev: PluginInput) -> anyhow::Result<()> {
        if matches!(ev, PluginInput::Reset) {
            self.keys.lock().unwrap().clear();
        }
        match self.media() {
            Some(m) => m.input(ev),
            None => Ok(()),
        }
    }

    fn local_input(&self, mode: u32) {
        self.local_mode.store(mode, Ordering::Relaxed);
        if let Some(m) = self.media() {
            apply_local_input(&m, mode);
        }
    }

    fn end(&self) {
        let running = self.media.lock().unwrap().take();
        if let Some(r) = running {
            stop_media(r.media);
        }
        self.local_mode.store(0, Ordering::Relaxed);
        let mut l = self.live.lock().unwrap();
        l.applied = None;
        l.device = None;
        l.hello = None;
        *self.clip_out.lock().unwrap() = None;
    }
}

/// The actor's link to the current client, through core.
pub struct CoreLink {
    stream: Arc<Stream>,
    resumed: bool,
}

impl ClientLink for CoreLink {
    fn send(&self, ev: HostEvent) {
        let s = &self.stream;
        let host = &s.host;
        match ev {
            HostEvent::Accepted { display, took_over_from, locked, home_mode } => {
                let (name, scales, version) = {
                    let l = s.live.lock().unwrap();
                    (
                        crate::session::store::Settings::load(&crate::keys::data_dir()).name,
                        crate::display::scales_for(&request_of(&l)),
                        l.hello.as_ref().map_or(msg::VERSION, |h| h.negotiated()),
                    )
                };
                s.on_display(display);
                let format = stream_format(display, s.wanted_codec());
                host.send(&Msg::Welcome(msg::Welcome {
                    version,
                    name,
                    format,
                    scales: scales.into_iter().map(|v| v as f32).collect(),
                    home_monitor: Some(monitor_mode(home_mode)),
                    locked,
                    took_over_from,
                    resumed: self.resumed,
                    host_os: msg::Os::Linux,
                    // REMOTE-1 is a virtual display sized to the client; one display, no picker.
                    display_mode: msg::DisplayMode::Virtual,
                    live_resize: true,
                    displays: vec![msg::DisplayInfo {
                        id: DISPLAY_ID,
                        name: "REMOTE-1".into(),
                        width: display.width,
                        height: display.height,
                        scale: display.scale as f32,
                        primary: true,
                    }],
                    current_display: DISPLAY_ID,
                    view_only: false,
                    host_os_flavor: remote_core::os_release::flavor(),
                }));
                eprintln!(
                    "remoted: welcome: {}x{}@{} scale {:.3} {:?}",
                    display.width,
                    display.height,
                    display.refresh_mhz as f64 / 1000.0,
                    display.scale,
                    format.codec
                );
            }
            HostEvent::Display(a) => {
                s.on_display(a);
                host.send(&Msg::Format(stream_format(a, s.wanted_codec())));
            }
            HostEvent::InUse { by } => host.disconnect(msg::DisconnectReason::TakenOver { by }),
            HostEvent::Refused { reason } => host.disconnect(match reason {
                "off" => msg::DisconnectReason::ControlOff,
                "not_paired" => msg::DisconnectReason::Revoked,
                r => msg::DisconnectReason::Error(r.to_string()),
            }),
            HostEvent::Ended(why) => match why {
                // Core told the old client already; the new one is live.
                EndReason::TakenOver { .. } | EndReason::ClientLeft => {}
                EndReason::DisconnectedByHost { .. } => host.disconnect(msg::DisconnectReason::User),
                EndReason::EndedAtComputer => host.disconnect(msg::DisconnectReason::EndedAtComputer),
                EndReason::Removed => host.disconnect(msg::DisconnectReason::Revoked),
                EndReason::RemoteOff => host.disconnect(msg::DisconnectReason::ControlOff),
                EndReason::DaemonStopping => host.disconnect(msg::DisconnectReason::HostShutdown),
            },
            HostEvent::HomeMode(m) => {
                host.send(&Msg::HomeMonitor(monitor_mode(m)));
            }
            HostEvent::Ime(ImeEvent::Caret { x, y, w, h }) => {
                host.send(&Msg::Caret(msg::Caret { visible: true, rect: msg::Rect { x, y, w, h } }));
                if let Some(m) = s.media() {
                    m.set_focus(Some(Rect { x, y, w, h }), None);
                }
            }
            HostEvent::Ime(ImeEvent::Focus { focused, secure }) => {
                host.send(&Msg::FocusChanged { focused, password: secure });
            }
            HostEvent::Ime(ImeEvent::Reset) => {
                host.send(&Msg::ImeReset);
            }
            HostEvent::Clipboard(entry) => s.clip_offer_out(entry),
            HostEvent::Audio(pkt) => {
                let ts = s.audio_ts.fetch_add(480, Ordering::Relaxed);
                host.send_audio(ts, None, pkt);
            }
            HostEvent::PointerLock { state, .. } => {
                host.send(&Msg::PointerLock(match state {
                    1 => msg::PointerLock::Locked,
                    2 => msg::PointerLock::Confined,
                    _ => msg::PointerLock::None,
                }));
            }
        }
    }
}

/// The rendezvous side of the actor's `Rendezvous`: core does the work.
pub struct CoreRendezvous {
    host: Host,
    view: Mutex<RelayView>,
}

impl CoreRendezvous {
    /// `server`: the configured server's host name; `None`: no server (LAN only).
    pub fn new(host: Host, server: Option<String>) -> Self {
        let state = if server.is_some() { "unknown" } else { "off" };
        let view = RelayView { host: server.unwrap_or_default(), state: state.into(), rtt_ms: 0, last_ok: 0 };
        CoreRendezvous { host, view: Mutex::new(view) }
    }

    fn set_online(&self, online: bool, error: Option<String>) {
        let mut v = self.view.lock().unwrap();
        v.state = if online {
            v.last_ok = crate::session::unix_now();
            "online".into()
        } else if error.as_deref().is_some_and(|e| e.contains(codes::HOST_NOT_ALLOWED)) {
            "not_enrolled".into()
        } else if let Some(code) = [codes::TOKEN_INVALID, codes::TOKEN_EXPIRED, codes::QUOTA_EXCEEDED, codes::PAYMENT_REQUIRED]
            .into_iter()
            .find(|c| error.as_deref().is_some_and(|e| e.contains(c)))
        {
            code.into()
        } else {
            "unreachable".into()
        };
        if let Some(e) = error {
            eprintln!("remoted: rendezvous: {e}");
        } else {
            eprintln!("remoted: rendezvous {}", if online { "online" } else { "offline" });
        }
    }
}

impl Rendezvous for CoreRendezvous {
    fn paired_list(&self, _payload: Vec<u8>, _sig: [u8; 64]) {}
    fn pair_open(&self, _name: &str, code: &str) {
        let (h, code) = (self.host.clone(), code.to_string());
        tokio::spawn(async move {
            if h.open_pairing_with(&code).await.is_none() {
                eprintln!("remoted: core didn't open the pairing window");
            }
        });
    }
    fn pair_close(&self) {
        self.host.close_pairing();
    }
    fn sleep_notice(&self, asleep: bool) {
        let h = self.host.clone();
        if asleep {
            tokio::spawn(async move { h.sleep_notice().await });
        } else {
            h.wake_notice();
        }
    }
    fn state(&self) -> RelayView {
        self.view.lock().unwrap().clone()
    }
    fn core(&self) -> bool {
        true
    }
    fn set_name(&self, name: &str) {
        self.host.set_name(name);
    }
    fn set_mdns(&self, on: bool) {
        self.host.set_mdns(on);
    }
    fn rename(&self, id: &str, name: &str) -> bool {
        id.parse::<DeviceId>().is_ok_and(|id| self.host.rename_paired(&id, name))
    }
    fn remove(&self, id: &str) -> futures_util::future::BoxFuture<'static, bool> {
        let (h, id) = (self.host.clone(), id.parse::<DeviceId>());
        Box::pin(async move {
            match id {
                Ok(id) => h.remove_paired(id).await,
                Err(_) => false,
            }
        })
    }
}

// ---- helpers --------------------------------------------------------------

/// The display request for the actor, from the client's display and settings.
fn request_of(l: &Live) -> Request {
    let d = l.display.unwrap_or(msg::ClientDisplay {
        width: 2560,
        height: 1664,
        pixels_per_point: 1.0,
        refresh_mhz: 60_000,
        fullscreen: true,
        display_width: 0,
        display_height: 0,
    });
    let st = &l.settings;
    Request {
        width: d.width,
        height: d.height,
        refresh_mhz: if st.refresh_mhz != 0 { st.refresh_mhz } else { d.refresh_mhz },
        ui: match st.ui_size {
            msg::UiSize::MatchMac => UiSize::Match,
            msg::UiSize::Larger => UiSize::Larger,
            msg::UiSize::Smaller => UiSize::Smaller,
            msg::UiSize::SameAsDesk => UiSize::Desk,
        },
        ui_ratio: d.pixels_per_point as f64,
        display_width: d.display_width,
        display_height: d.display_height,
    }
}

fn stream_format(a: Applied, codec: msg::Codec) -> msg::StreamFormat {
    msg::StreamFormat { width: a.width, height: a.height, scale: a.scale as f32, refresh_mhz: a.refresh_mhz, codec }
}

fn home_mode(m: msg::HomeMonitorMode) -> HomeMode {
    match m {
        msg::HomeMonitorMode::Mirror => HomeMode::Mirror,
        msg::HomeMonitorMode::Privacy => HomeMode::Privacy,
        msg::HomeMonitorMode::Off => HomeMode::Off,
    }
}

fn monitor_mode(m: HomeMode) -> msg::HomeMonitorMode {
    match m {
        HomeMode::Mirror => msg::HomeMonitorMode::Mirror,
        HomeMode::Privacy => msg::HomeMonitorMode::Privacy,
        HomeMode::Off => msg::HomeMonitorMode::Off,
    }
}

/// The path as the notifications say it: "Through Direct · 9 ms". A VPN
/// path in a named network (`networks.toml`) says the network's label.
pub fn path_words(r: &RouteInfo) -> String {
    match &r.kind {
        PathKind::Lan => "LAN",
        PathKind::Direct => "Direct",
        PathKind::Vpn => return r.network.clone().unwrap_or_else(|| "VPN".into()),
        PathKind::Tailscale => "Tailscale",
        PathKind::Relay { .. } => "Relay",
    }
    .into()
}

/// An instant as CLOCK_MONOTONIC ns (what capture-ipc's `t_ns` takes).
fn instant_to_mono(at: Instant) -> u64 {
    let now = Instant::now();
    let mono = monotonic_ns();
    match now.checked_duration_since(at) {
        Some(ago) => mono.saturating_sub(ago.as_nanos() as u64),
        None => mono + (at - now).as_nanos() as u64,
    }
}

/// REMOTE-1's one display, in `Welcome.displays`.
const DISPLAY_ID: u32 = 1;
/// Whether remoted can encode H.264 (not yet: HEVC and AV1 on NVENC only).
const HOST_ENCODES_H264: bool = false;

/// The codec for the client's choice (`Settings`, so live) and what its decoder
/// says (`Hello`): AV1 if asked for and decoded; HEVC 4:2:0 if asked for,
/// whatever the client decodes; else HEVC at the
/// chroma `chroma` allows.
fn pick_codec(choice: CodecChoice, decode: Option<msg::DecodeSupport>, chroma: ChromaPref) -> msg::Codec {
    // remoted's NVENC path encodes HEVC and AV1 only. A client that decodes only H.264
    // still gets HEVC, as before.
    if msg::DecodeSupport::use_h264(decode, true, HOST_ENCODES_H264) {
        return msg::Codec::H264;
    }
    match (choice, decode) {
        (CodecChoice::Av1, Some(d)) if d.av1 => msg::Codec::Av1,
        (CodecChoice::Hevc420, _) => msg::Codec::Hevc420,
        (_, d) => match chroma {
            ChromaPref::C420 => msg::Codec::Hevc420,
            ChromaPref::C444 => msg::Codec::Hevc444,
            ChromaPref::Auto if d.is_some_and(|d| d.hevc_444) => msg::Codec::Hevc444,
            ChromaPref::Auto => msg::Codec::Hevc420,
        },
    }
}

fn format_of(mime: &str) -> ClipFormat {
    match mime {
        "text/plain;charset=utf-8" | "text/plain" | "UTF8_STRING" | "STRING" | "TEXT" => ClipFormat::Text,
        "text/html" => ClipFormat::Html,
        "text/rtf" | "application/rtf" => ClipFormat::Rtf,
        "image/png" => ClipFormat::Png,
        "image/jpeg" => ClipFormat::Jpeg,
        m => ClipFormat::Mime(m.to_string()),
    }
}

fn mime_of(f: &ClipFormat) -> String {
    match f {
        ClipFormat::Text => "text/plain;charset=utf-8",
        ClipFormat::Html => "text/html",
        ClipFormat::Rtf => "text/rtf",
        ClipFormat::Png => "image/png",
        ClipFormat::Jpeg => "image/jpeg",
        ClipFormat::Mime(m) => m,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codec_choice_applies_live_and_hevc_is_4_2_0_whatever_the_mac_decodes() {
        let d = Some(msg::DecodeSupport { hevc: true, h264: false, hevc_444: true, av1: true, max_width: 0, max_height: 0 });
        let auto = ChromaPref::Auto;
        assert_eq!(pick_codec(CodecChoice::Auto, d, auto), msg::Codec::Hevc444);
        assert_eq!(pick_codec(CodecChoice::Hevc420, d, auto), msg::Codec::Hevc420);
        assert_eq!(pick_codec(CodecChoice::Av1, d, auto), msg::Codec::Av1);
        // No hardware 4:4:4 (or no Hello yet): Auto is 4:2:0 too.
        let no444 = Some(msg::DecodeSupport { hevc_444: false, ..d.unwrap() });
        assert_eq!(pick_codec(CodecChoice::Auto, no444, auto), msg::Codec::Hevc420);
    }
}
