// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The media loop for one display: capture on its own thread,
//! encode on the media thread, frames to core.
//!
//! - **Frames only on damage**, at most one per frame interval; damage that comes while
//!   a frame is encoding is merged into the next (the latest picture wins).
//! - **The rate** is core's congestion-controlled budget (`LinkStatus::budget`), read
//!   before every frame. An encoder that can't change its rate per frame is re-opened
//!   on a lasting change (`policy::RateFollower`).
//! - **One high-quality refresh** 150 ms after the last change (`policy::Refresh`): the
//!   same picture again, with the rest of the frame interval's budget when the encoder
//!   takes per-frame rates.
//! - **Keyframes** on the client's request and core's reference hints (none of the
//!   encoders can predict from an older frame, so a hint is a keyframe), coalesced by
//!   `policy::KeyframeGate`: at most one per 250 ms, stale hints dropped (
//!   review). Also, ungated, for a new encoder and the end of a secure screen. No QP
//!   maps, no split frames.
//! - **The pointer** goes on the cursor channel in stream pixels; shapes once per hash.
//! - **Secure screens**: `Msg::SecureScreen(true)` and no frames until it goes.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use remote_core::alloc::{FrameOpts, RefHint};
use remote_core::connect::Host;
use remote_core::msg::{self, Codec, CursorImage, Msg};
use remote_core::session::OutgoingFrame;

use crate::backend::{Backend, CursorShape, Display, Encoder, EncoderConfig, Picture, Rect};
use crate::policy::{KeyframeGate, RateFollower, Refresh};

/// The encoder's rate until core has a budget (core's controller starts at 8 Mbps).
pub const START_BPS: u64 = 8_000_000;
const MIN_BPS: u64 = 300_000;
/// The wait before an encoder that wouldn't open is opened once more.
const REOPEN_RETRY: Duration = Duration::from_millis(250);

/// What the stream is: one display, at a size and rate, in a codec.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamSetup {
    pub display: Display,
    /// The encoded size (the display's, or smaller to fit the client's window).
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: Codec,
}

/// Frame classes in the frame meta (host/src/media/wire.rs, client-linux's proto.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Passive = 0,
    Interactive = 1,
    Refine = 2,
}

/// Per-second numbers for `Msg::Stats` and the log.
#[derive(Default, Debug)]
pub struct Acc {
    pub frames: u32,
    pub keyframes: u32,
    pub refreshes: u32,
    pub bytes: u64,
    pub capture_us: Vec<u32>,
    pub convert_us: Vec<u32>,
    pub encode_us: Vec<u32>,
    pub encoder: String,
}

/// What the media thread tells the session.
pub enum MediaEvent {
    /// The encoder couldn't be opened or failed for good.
    Failed(String),
    /// A secure screen came up (true) or went away (false).
    Secure(bool),
    /// The person at this computer stopped sharing (`backend::SharingEnded`).
    Ended,
    /// No encoder of the stream's codec would open, so the stream went on in this one
    /// (software H.264); the client has the new `Format`.
    CodecFallback(Codec),
}

enum In {
    Update(crate::backend::CaptureUpdate),
    CaptureFailed(String),
    /// Sharing was stopped at the computer: the capture won't come back.
    Ended,
    /// The client asked for a keyframe.
    Keyframe,
    /// Core's reference hint.
    Hint(RefHint),
    /// The client's window changed: encode at this size from now on (a new encoder).
    Resize(u32, u32),
    /// Answered once everything sent before it is done.
    Barrier(mpsc::Sender<()>),
    Stop,
}

pub struct Media {
    tx: mpsc::Sender<In>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    pub acc: Arc<Mutex<Acc>>,
    /// The stream now; the loop updates it when a resize takes effect.
    setup: Arc<Mutex<StreamSetup>>,
}

impl Media {
    pub fn start(
        backend: Arc<dyn Backend>,
        host: Host,
        setup: StreamSetup,
        events: Arc<dyn Fn(MediaEvent) + Send + Sync>,
    ) -> Result<Media> {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let acc = Arc::new(Mutex::new(Acc::default()));
        let shared = Arc::new(Mutex::new(setup.clone()));
        let capture = {
            let (backend, tx, stop, id) = (backend.clone(), tx.clone(), stop.clone(), setup.display.info.id);
            let fps = setup.fps;
            std::thread::Builder::new().name("capture".into()).spawn(move || capture_loop(backend, id, fps, tx, stop))?
        };
        let media = {
            let (acc, setup, stop, shared) = (acc.clone(), setup.clone(), stop.clone(), shared.clone());
            std::thread::Builder::new().name("media".into()).spawn(move || {
                let mut m = Loop::new(backend, host, setup, shared, acc, stop, events.clone());
                if let Err(e) = m.run(rx) {
                    eprintln!("sodium-remote-host: media: {e:#}");
                    events(MediaEvent::Failed(format!("{e:#}")));
                }
            })?
        };
        Ok(Media { tx, stop, threads: vec![capture, media], acc, setup: shared })
    }

    /// The client asked for a keyframe (`Msg::KeyframeRequest`).
    pub fn request_keyframe(&self) {
        let _ = self.tx.send(In::Keyframe);
    }

    /// The stream's setup now.
    pub fn setup(&self) -> StreamSetup {
        self.setup.lock().unwrap().clone()
    }

    /// Encode at another size, on the same capture: a new encoder, a new `Format` and a keyframe.
    pub fn resize(&self, width: u32, height: u32) {
        let _ = self.tx.send(In::Resize(width, height));
    }

    /// Waits (at most `timeout`) until the media loop has done what it was asked so far, a
    /// `resize` (a new encoder) included: a display mode change must not overlap an encoder
    /// being opened (QSV then fails its set-up). False: it timed out or the loop is gone.
    pub fn settle(&self, timeout: Duration) -> bool {
        let (tx, rx) = mpsc::channel();
        self.tx.send(In::Barrier(tx)).is_ok() && rx.recv_timeout(timeout).is_ok()
    }

    /// Core's hint that the client lost frames (`LinkStatus::ref_hint`).
    pub fn ref_hint(&self, hint: RefHint) {
        let _ = self.tx.send(In::Hint(hint));
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.tx.send(In::Stop);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn capture_loop(backend: Arc<dyn Backend>, display: u32, fps: u32, tx: mpsc::Sender<In>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        let mut cap = match backend.open_capture(display, fps) {
            Ok(c) => c,
            Err(e) if e.is::<crate::backend::SharingEnded>() => {
                let _ = tx.send(In::Ended);
                return;
            }
            Err(e) => {
                let _ = tx.send(In::CaptureFailed(format!("{e:#}")));
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
        };
        while !stop.load(Ordering::Relaxed) {
            match cap.next(Duration::from_millis(50)) {
                Ok(u) => {
                    let empty = u.frame.is_none() && u.pointer.is_none() && u.shape.is_none() && u.secure.is_none();
                    if !empty && tx.send(In::Update(u)).is_err() {
                        return;
                    }
                }
                Err(e) if e.is::<crate::backend::SharingEnded>() => {
                    let _ = tx.send(In::Ended);
                    return;
                }
                Err(e) => {
                    let _ = tx.send(In::CaptureFailed(format!("{e:#}")));
                    std::thread::sleep(Duration::from_millis(250));
                    break;
                }
            }
        }
    }
}

struct Loop {
    backend: Arc<dyn Backend>,
    host: Host,
    setup: StreamSetup,
    shared: Arc<Mutex<StreamSetup>>,
    acc: Arc<Mutex<Acc>>,
    stop: Arc<AtomicBool>,
    events: Arc<dyn Fn(MediaEvent) + Send + Sync>,
    encoder: Option<Box<dyn Encoder>>,
    follower: RateFollower,
    picture: Option<Arc<dyn Picture>>,
    captured_at: Option<Instant>,
    damage: Vec<Rect>,
    full: bool,
    dirty: bool,
    /// A keyframe the stream needs (its start, a new encoder, a secure screen's end).
    want_key: bool,
    /// Keyframes the client or core asked for, coalesced.
    gate: KeyframeGate,
    secure: bool,
    refresh: Refresh,
    last_sent: Option<Instant>,
    pointer: Option<crate::backend::PointerState>,
    shape: u64,
    /// The last pointer shape, in display pixels: sent again at a new stream size.
    last_shape: Option<CursorShape>,
    capture_error: Option<String>,
}

impl Loop {
    fn new(
        backend: Arc<dyn Backend>,
        host: Host,
        setup: StreamSetup,
        shared: Arc<Mutex<StreamSetup>>,
        acc: Arc<Mutex<Acc>>,
        stop: Arc<AtomicBool>,
        events: Arc<dyn Fn(MediaEvent) + Send + Sync>,
    ) -> Loop {
        Loop {
            backend,
            host,
            setup,
            shared,
            acc,
            stop,
            events,
            encoder: None,
            follower: RateFollower::new(START_BPS),
            picture: None,
            captured_at: None,
            damage: vec![],
            full: true,
            dirty: false,
            want_key: true,
            gate: KeyframeGate::default(),
            secure: false,
            refresh: Refresh::default(),
            last_sent: None,
            pointer: None,
            shape: 0,
            last_shape: None,
            capture_error: None,
        }
    }

    fn interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / self.setup.fps.max(1) as f64)
    }

    fn target_bps(&self) -> u64 {
        self.host.link_status().map(|l| l.budget.encoder_bps).filter(|&b| b > 0).unwrap_or(START_BPS).max(MIN_BPS)
    }

    /// A new encoder for the stream. A failure is no reason to end the session: once more
    /// (the GPU may have been busy with something else, a display change say), then the
    /// software H.264 encoder for the rest of the session ([`Loop::fall_back`]).
    fn open_encoder(&mut self, bps: u64) -> Result<()> {
        self.encoder = None;
        let first = match self.try_open(bps) {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        eprintln!("sodium-remote-host: encoder: {first:#}; trying again");
        std::thread::sleep(REOPEN_RETRY);
        match self.try_open(bps) {
            Ok(()) => Ok(()),
            Err(e) => self.fall_back(bps, e),
        }
    }

    /// The stream in H.264 (the next encoder: for HEVC, the H.264 ones, software last), told to
    /// the client in a new `Format`. `why` is returned when that doesn't open either.
    fn fall_back(&mut self, bps: u64, why: anyhow::Error) -> Result<()> {
        if self.setup.codec == Codec::H264 {
            return Err(why);
        }
        eprintln!("sodium-remote-host: encoder: {why:#}; using H.264 for this session");
        let hevc = self.setup.codec;
        self.setup.codec = Codec::H264;
        match self.try_open(bps) {
            Ok(()) => {
                *self.shared.lock().unwrap() = self.setup.clone();
                self.host.send(&Msg::Format(stream_format(&self.setup)));
                (self.events)(MediaEvent::CodecFallback(Codec::H264));
                Ok(())
            }
            Err(_) => {
                self.setup.codec = hevc;
                Err(why)
            }
        }
    }

    fn try_open(&mut self, bps: u64) -> Result<()> {
        let cfg = EncoderConfig {
            codec: self.setup.codec,
            width: self.setup.width,
            height: self.setup.height,
            fps: self.setup.fps,
            bitrate_bps: bps,
        };
        let e = self.backend.open_encoder(&cfg)?;
        eprintln!(
            "sodium-remote-host: encoding {:?} {}x{} at {} fps with {} ({} kbit/s{})",
            e.codec(),
            cfg.width,
            cfg.height,
            cfg.fps,
            e.name(),
            bps / 1000,
            if e.dynamic_rate() { ", per-frame rate" } else { "" }
        );
        self.acc.lock().unwrap().encoder = e.name().to_string();
        self.follower = RateFollower::new(bps);
        self.encoder = Some(e);
        self.want_key = true;
        Ok(())
    }

    fn run(&mut self, rx: mpsc::Receiver<In>) -> Result<()> {
        let bps = self.target_bps();
        self.open_encoder(bps)?;
        while !self.stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            let next_frame = self.last_sent.map_or(now, |t| t + self.interval());
            let deadline = if self.dirty || self.want_key {
                Some(next_frame)
            } else {
                // A requested keyframe waits for the gate (and the frame clock).
                let key = self.gate.due().map(|d| d.max(next_frame));
                [key, self.refresh.due()].into_iter().flatten().min()
            };
            let wait = deadline.map_or(Duration::from_millis(100), |d| d.saturating_duration_since(now));
            let first = match rx.recv_timeout(wait) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let mut pending: Vec<In> = first.into_iter().collect();
            pending.extend(rx.try_iter());
            for m in pending {
                match m {
                    In::Stop => return Ok(()),
                    In::Ended => {
                        eprintln!("sodium-remote-host: sharing was stopped at this computer");
                        (self.events)(MediaEvent::Ended);
                        return Ok(());
                    }
                    In::Keyframe | In::Hint(RefHint::Keyframe) => self.gate.request(),
                    In::Hint(RefHint::Reference(acked)) => self.gate.lost_after(acked),
                    In::CaptureFailed(e) => {
                        if self.capture_error.as_deref() != Some(&e) {
                            eprintln!("sodium-remote-host: capture: {e}");
                            self.capture_error = Some(e);
                        }
                    }
                    In::Resize(w, h) => self.resize(w, h)?,
                    In::Barrier(done) => {
                        let _ = done.send(());
                    }
                    In::Update(u) => self.merge(u),
                }
            }
            if self.secure || self.picture.is_none() {
                continue;
            }
            let now = Instant::now();
            let due = self.last_sent.is_none_or(|t| now >= t + self.interval());
            if (self.dirty || self.want_key || self.gate.ready(now)) && due {
                let critical = self.host.link_status().is_some_and(|l| l.budget.latency_critical);
                self.send(if critical { Class::Interactive } else { Class::Passive })?;
            } else if !self.dirty && self.refresh.take(now) {
                self.send(Class::Refine)?;
            }
        }
        Ok(())
    }

    /// A new stream size on the same capture: the setup, then `Format`, then a new encoder
    /// whose first frame is a keyframe of the latest picture at the new size.
    fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        if (width, height) == (self.setup.width, self.setup.height) {
            return Ok(());
        }
        self.setup.width = width;
        self.setup.height = height;
        *self.shared.lock().unwrap() = self.setup.clone();
        eprintln!("sodium-remote-host: stream now {width}x{height}");
        self.host.send(&Msg::Format(stream_format(&self.setup)));
        let bps = self.target_bps();
        self.open_encoder(bps)?;
        self.full = true;
        self.dirty = self.picture.is_some();
        if let Some(s) = self.last_shape.clone() {
            self.send_shape(s);
        } else {
            self.send_pointer();
        }
        Ok(())
    }

    fn merge(&mut self, u: crate::backend::CaptureUpdate) {
        self.capture_error = None;
        if let Some(on) = u.secure
            && on != self.secure
        {
            eprintln!("sodium-remote-host: secure screen {}", if on { "up" } else { "gone" });
            self.secure = on;
            self.host.send(&Msg::SecureScreen(on));
            (self.events)(MediaEvent::Secure(on));
            if !on {
                self.want_key = true;
            }
        }
        if let Some((pic, rects)) = u.frame {
            if rects.is_empty() {
                self.full = true;
            } else {
                self.damage.extend(rects);
            }
            self.picture = Some(pic);
            self.captured_at = u.at.or(Some(Instant::now()));
            self.dirty = true;
        }
        if let Some(p) = u.pointer {
            self.pointer = Some(p);
        }
        match u.shape {
            Some(s) => self.send_shape(s), // and the position, under the new hash
            None if u.pointer.is_some() => self.send_pointer(),
            None => {}
        }
    }

    /// Display pixels → stream pixels.
    fn scale(&self) -> (f64, f64) {
        let i = &self.setup.display.info;
        (self.setup.width as f64 / i.width.max(1) as f64, self.setup.height as f64 / i.height.max(1) as f64)
    }

    fn send_pointer(&self) {
        let Some(p) = self.pointer else { return };
        let (sx, sy) = self.scale();
        self.host.send_cursor_position((p.x as f64 * sx) as f32, (p.y as f64 * sy) as f32, p.visible, self.shape);
    }

    fn send_shape(&mut self, s: CursorShape) {
        self.last_shape = Some(s.clone());
        let (sx, sy) = self.scale();
        let image = if (sx - 1.0).abs() < 1e-6 && (sy - 1.0).abs() < 1e-6 {
            CursorImage { width: s.width, height: s.height, hot_x: s.hot_x, hot_y: s.hot_y, scale: 1.0, pixels: s.pixels }
        } else {
            scale_cursor(&s, sx, sy)
        };
        let image = CursorImage { scale: self.setup.display.info.scale, ..image };
        let mut h = DefaultHasher::new();
        (image.width, image.height, image.hot_x, image.hot_y, &image.pixels).hash(&mut h);
        self.shape = h.finish();
        self.host.send_cursor_shape(self.shape, image);
        self.send_pointer();
    }

    fn send(&mut self, class: Class) -> Result<()> {
        let Some(pic) = self.picture.clone() else { return Ok(()) };
        let now = Instant::now();
        let target = self.target_bps();
        let dynamic = self.encoder.as_ref().is_some_and(|e| e.dynamic_rate());
        if !dynamic && self.follower.reopen(target, now) {
            eprintln!("sodium-remote-host: core's target is now {} kbit/s: a new encoder at that rate", target / 1000);
            self.open_encoder(target)?;
        }
        let mut bps = if dynamic { target } else { self.follower.current };
        if class == Class::Refine && dynamic {
            // The refresh may use what is left of the frame interval, up to 3 frames' worth.
            let rest = self.host.link_status().map_or(0, |l| l.budget.interval_remaining_bytes as u64 * 8 * self.setup.fps as u64);
            bps = rest.clamp(target, target * 3);
        }
        let keyframe = self.want_key || self.gate.ready(now);
        let t_start = Instant::now();
        let enc = self.encoder.as_mut().expect("encoder");
        let f = match enc.encode(pic.as_ref(), keyframe, bps) {
            Ok(f) => f,
            Err(e) => {
                // A new encoder (which tries again and falls back by itself), then H.264, then give up.
                eprintln!("sodium-remote-host: encode: {e:#}; opening the encoder again");
                self.open_encoder(target)?;
                match self.encoder.as_mut().unwrap().encode(pic.as_ref(), true, target) {
                    Ok(f) => f,
                    Err(e) => {
                        self.fall_back(target, e)?;
                        self.encoder.as_mut().unwrap().encode(pic.as_ref(), true, target)?
                    }
                }
            }
        };
        let t_encoded = Instant::now();
        self.last_sent = Some(now);
        if class != Class::Refine {
            self.refresh.changed(now);
        }
        if f.data.is_empty() {
            return Ok(());
        }
        let meta = self.meta(class, t_start, t_encoded, &f);
        let bytes = f.data.len();
        let keyframe = f.keyframe;
        let id = self.host.send_video_with(OutgoingFrame { data: f.data, keyframe, meta }, FrameOpts::default());
        if keyframe {
            self.want_key = false;
            self.gate.sent(id, now);
        }
        self.dirty = false;
        self.full = false;
        self.damage.clear();
        let mut a = self.acc.lock().unwrap();
        a.frames += 1;
        a.keyframes += keyframe as u32;
        a.refreshes += (class == Class::Refine) as u32;
        a.bytes += bytes as u64;
        if let Some(c) = self.captured_at {
            a.capture_us.push(t_start.saturating_duration_since(c).as_micros() as u32);
        }
        a.convert_us.push(f.convert_us);
        a.encode_us.push(f.encode_us);
        Ok(())
    }

    /// `OutgoingFrame.meta`: host media's v1 (`u8 1 | u8 class | u16 n | n × (u16 x, y, w,
    /// h)`, stream pixels, more than 32 rects as their bounding box), then TLV record 1:
    /// the host's stamps (rendered = captured, captured, converted, encoded; µs on the
    /// session clock), as remoted sends them.
    fn meta(&self, class: Class, t_start: Instant, t_encoded: Instant, f: &crate::backend::EncodedFrame) -> Vec<u8> {
        let (sx, sy) = self.scale();
        let (w, h) = (self.setup.width as i32, self.setup.height as i32);
        let mut rects: Vec<(i32, i32, i32, i32)> = if self.full || class == Class::Refine {
            vec![(0, 0, w, h)]
        } else {
            self.damage
                .iter()
                .filter_map(|r| {
                    let x0 = ((r.x as f64 * sx).floor() as i32).clamp(0, w);
                    let y0 = ((r.y as f64 * sy).floor() as i32).clamp(0, h);
                    let x1 = (((r.x + r.w) as f64 * sx).ceil() as i32).clamp(0, w);
                    let y1 = (((r.y + r.h) as f64 * sy).ceil() as i32).clamp(0, h);
                    (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
                })
                .collect()
        };
        if rects.len() > 32 {
            let x0 = rects.iter().map(|r| r.0).min().unwrap();
            let y0 = rects.iter().map(|r| r.1).min().unwrap();
            let x1 = rects.iter().map(|r| r.0 + r.2).max().unwrap();
            let y1 = rects.iter().map(|r| r.1 + r.3).max().unwrap();
            rects = vec![(x0, y0, x1 - x0, y1 - y0)];
        }
        let mut b = Vec::with_capacity(4 + rects.len() * 8 + 35);
        b.push(1);
        b.push(class as u8);
        b.extend_from_slice(&(rects.len() as u16).to_be_bytes());
        for (x, y, w, h) in rects {
            for v in [x, y, w, h] {
                b.extend_from_slice(&(v as u16).to_be_bytes());
            }
        }
        if let Some(epoch) = self.host.session_epoch() {
            let us = |t: Instant| t.saturating_duration_since(epoch).as_micros() as u64;
            let captured = self.captured_at.unwrap_or(t_start);
            let converted = t_start + Duration::from_micros(f.convert_us as u64);
            b.push(1);
            b.extend_from_slice(&32u16.to_be_bytes());
            for t in [captured, captured, converted, t_encoded] {
                b.extend_from_slice(&us(t).to_be_bytes());
            }
        }
        b
    }
}

/// A cursor image scaled with the stream (nearest neighbour).
fn scale_cursor(s: &CursorShape, sx: f64, sy: f64) -> CursorImage {
    let w = ((s.width as f64 * sx).round() as u32).max(1);
    let h = ((s.height as f64 * sy).round() as u32).max(1);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let src_y = ((y as f64 / sy) as u32).min(s.height - 1);
        for x in 0..w {
            let src_x = ((x as f64 / sx) as u32).min(s.width - 1);
            let si = ((src_y * s.width + src_x) * 4) as usize;
            let di = ((y * w + x) * 4) as usize;
            pixels[di..di + 4].copy_from_slice(&s.pixels[si..si + 4]);
        }
    }
    CursorImage {
        width: w,
        height: h,
        hot_x: (s.hot_x as f64 * sx) as i32,
        hot_y: (s.hot_y as f64 * sy) as i32,
        scale: 1.0,
        pixels,
    }
}

pub fn p50(v: &mut [u32]) -> u32 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

/// The stream's format for `Welcome` and `Format`.
pub fn stream_format(s: &StreamSetup) -> msg::StreamFormat {
    msg::StreamFormat { width: s.width, height: s.height, scale: s.display.info.scale, refresh_mhz: s.fps * 1000, codec: s.codec }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::*;
    use remote_core::msg::Os;

    /// A backend whose capture reports that sharing was stopped at the computer.
    struct Stopped;

    struct StoppedCapture;

    impl Capture for StoppedCapture {
        fn next(&mut self, _timeout: Duration) -> Result<CaptureUpdate> {
            Err(SharingEnded.into())
        }
    }

    impl Backend for Stopped {
        fn os(&self) -> Os {
            Os::Linux
        }
        fn computer_name(&self) -> String {
            String::new()
        }
        fn displays(&self) -> Vec<Display> {
            vec![]
        }
        fn open_capture(&self, _display: u32, _fps: u32) -> Result<Box<dyn Capture>> {
            Ok(Box::new(StoppedCapture))
        }
        fn encoder_caps(&self) -> EncoderCaps {
            EncoderCaps::default()
        }
        fn open_encoder(&self, _cfg: &EncoderConfig) -> Result<Box<dyn Encoder>> {
            anyhow::bail!("none")
        }
        fn input(&self) -> Result<Box<dyn Input>> {
            anyhow::bail!("none")
        }
        fn audio(&self) -> Result<Box<dyn AudioSource>> {
            anyhow::bail!("none")
        }
        fn clipboard(&self, _on_change: ClipSink) -> Result<Arc<dyn Clipboard>> {
            anyhow::bail!("none")
        }
        fn foreground(&self) -> Foreground {
            Foreground::default()
        }
    }

    #[test]
    fn stopping_the_share_ends_the_capture_instead_of_reopening_it() {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let t = std::thread::spawn(move || capture_loop(Arc::new(Stopped), 1, 30, tx, stop));
        // One Ended, then the loop is gone (no re-open, no CaptureFailed retries).
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)), Ok(In::Ended)));
        t.join().unwrap();
        assert!(rx.try_recv().is_err());
    }
}
