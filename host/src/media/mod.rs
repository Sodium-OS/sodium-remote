// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! media: the host's video pipeline as one API for remoted's session
//!
//! It owns the plugin connection (capture-ipc v3, both directions), Vulkan import
//! and conversion, NVENC, damage-driven QP maps, build-to-lossless refinement,
//! motion hints, the per-frame rate and split rules, loss recovery and the cursor
//! channel. Two threads: `media-ipc` reads the socket (cursor, caret and pointer
//! lock events go straight out from there), `media-enc` owns the GPU and encoder.
//! Input and FRAME_CLOCK are written to the socket on the caller's thread.

mod encode;
pub mod grid;
mod ipc;
pub mod rate;
pub mod wire;

use std::{
    os::fd::OwnedFd,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

use anyhow::{Context, Result, anyhow};

pub use crate::capture::Rect;
use crate::capture::{monotonic_ns, plugin};
pub use grid::{Ending, Focus, RefineConfig};
pub use remote_core::msg::CursorImage;
pub use rate::{FrameClass, RateInput};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Hevc,
    /// 4:2:0 only on this GPU.
    Av1,
}

#[derive(Clone, Debug)]
pub struct MediaConfig {
    /// The plugin's capture socket.
    pub socket: PathBuf,
    /// The output to capture (REMOTE-1).
    pub output: String,
    pub codec: Codec,
    /// HEVC 4:4:4 (when the client decodes it); otherwise NV12.
    pub chroma444: bool,
    /// The stream rate: REMOTE-1's refresh.
    pub fps: u32,
    /// The encoder's rate until the first `set_rate`.
    pub start_bps: u64,
    pub refine: RefineConfig,
    /// Damage-driven QP maps (unchanged cells as skip). On by default; off for comparisons.
    pub damage_qp: bool,
    /// Build-to-lossless refinement frames. On by default.
    pub refinement: bool,
    /// Motion hints from the plugin's HINT messages into NVENC. On by default.
    pub motion_hints: bool,
    /// On a measured bottleneck, give the cells a refinement frame refines QP offsets by their
    /// distance to the caret and the pointer, so the nearest get the most of its budget
    ///. On by default; off for comparisons.
    pub focus_refine: bool,
    /// Tests: write the encoder's input (YUV) of every captured frame that gets refined here (once, at its first refinement frame).
    pub dump: Option<PathBuf>,
}

impl Default for MediaConfig {
    fn default() -> Self {
        MediaConfig {
            socket: plugin::default_socket(),
            output: "REMOTE-1".into(),
            codec: Codec::Hevc,
            chroma444: false,
            fps: 60,
            start_bps: 8_000_000,
            refine: RefineConfig::default(),
            damage_qp: true,
            refinement: true,
            motion_hints: true,
            focus_refine: true,
            dump: None,
        }
    }
}

/// An encoded frame, ready for `Session::send_video_frame`.
#[derive(Clone, Debug)]
pub struct EncodedFrame {
    /// media's own frame number, from 0; give it back to [`Media::predict_from`].
    pub pts: u64,
    /// HEVC Annex-B, or one AV1 temporal unit.
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// Predicted from this earlier frame instead of the one before it (after
    /// [`Media::predict_from`]): send with `FrameOpts { reference: Some(its frame id) }`.
    pub reference: Option<u64>,
    /// An intra refresh completed with this frame: `FrameOpts { recovery_point: true }`.
    pub recovery_point: bool,
    pub class: FrameClass,
    /// `OutgoingFrame.meta` ([`wire::frame_meta`]).
    pub meta: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// The plugin's FRAME time (CLOCK_MONOTONIC ns); for a refinement, the capture it refines.
    pub t_rendered_ns: u64,
    /// The bitstream was ready.
    pub t_encoded_ns: u64,
    pub info: FrameInfo,
}

/// Per-frame measurements (stats file, HUD, tests).
#[derive(Clone, Debug, Default)]
pub struct FrameInfo {
    /// The plugin's FRAME seq (the last one folded in).
    pub seq: u64,
    /// remoted had the FRAME in hand.
    pub t_ready_ns: u64,
    /// Convert: submit to fence, including the wait on the compositor's copy.
    pub convert_ms: f64,
    pub encode_ms: f64,
    pub avg_qp: u32,
    pub pic_type: i32,
    /// Encoder cells damaged / refined in this frame, out of `cells`.
    pub changed_cells: u32,
    pub refined_cells: u32,
    pub cells: u32,
    /// Motion-hint blocks given to NVENC.
    pub hinted_blocks: u32,
    /// FRAMEs folded into this one because the encoder was behind.
    pub merged: u32,
    pub kbps: u32,
    pub split: bool,
    /// Intra refresh started with this frame. Nothing starts one now (a loss beyond the
    /// DPB gets an IDR); the code stays.
    pub intra_refresh: bool,
    /// Frames invalidated just before this one.
    pub invalidated: u32,
    /// The frame's base QP (codec units; AV1's qindex is 4×), recovered from NVENC's
    /// average over the QP map. Pessimistic by up to one, and 51 (255) when it can't tell.
    pub base_qp: i32,
    /// A change finished with this frame (the longest, if several): ms from the change's
    /// own frame (`t_rendered`) to every cell it touched being coded at base QP 34 or
    /// better (readable), or 20 or better, or out of passes (lossless). Under a user-set
    /// bandwidth cap lossless is reached at 34. See [`MediaStats::readable_ms`].
    pub readable_ms: Option<f64>,
    pub lossless_ms: Option<f64>,
    /// A change that ended parked or at the cap: ms to it, and the ending. `lossless_ms` is the stop only.
    pub settled_ms: Option<f64>,
    pub settled_end: Option<Ending>,
}

/// A cursor image, once per new hash: the plugin's raw premultiplied
/// BGRA in the contract's [`CursorImage`] (`scale` 1.0: the session overwrites it with
/// REMOTE-1's). For `connect::Host::send_cursor_shape(hash, image)`.
#[derive(Clone, Debug)]
pub struct CursorShape {
    pub hash: u64,
    pub image: CursorImage,
}

#[derive(Clone, Debug)]
pub enum MediaEvent {
    Frame(EncodedFrame),
    CursorShape(CursorShape),
    /// The hotspot in buffer pixels; `shape` is the current shape's hash.
    CursorPos { x: i32, y: i32, visible: bool, shape: u64, t_ns: u64 },
    /// The focused text field's caret (purpose 8 password, 9 PIN: secure input).
    Caret { visible: bool, purpose: u32, rect: Rect, t_ns: u64 },
    /// 0 none, 1 locked, 2 confined; `rect` is the locked surface or the confinement.
    PointerLock { state: u32, rect: Rect },
    PluginStats { frames: u64, dropped_no_buffer: u64, dropped_no_client: u64 },
    /// A compositor animation started (true) or settled (false), from the plugin's
    /// HINT: for `Session::set_animation`.
    Animation(bool),
    /// Capture ended (STOPPED, the socket closed, a GPU or encoder error). Both threads exit.
    Stopped(String),
}

/// Input for the plugin (capture-ipc 64–70): buffer pixels, evdev codes, host
/// CLOCK_MONOTONIC `t_ns` (0 = now).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputEvent {
    PointerAbs { x: f64, y: f64, t_ns: u64 },
    PointerRel { dx: f64, dy: f64, dx_unaccel: f64, dy_unaccel: f64, t_ns: u64 },
    Button { button: u32, pressed: bool, t_ns: u64 },
    /// `more`: the next Axis belongs to the same pointer frame.
    Axis { source: u32, axis: u32, value: f64, value120: i32, stop: bool, more: bool, t_ns: u64 },
    Key { keycode: u32, pressed: bool, t_ns: u64 },
    Gesture { kind: u32, phase: u32, fingers: u32, dx: f64, dy: f64, scale: f64, rotation: f64, t_ns: u64 },
    Reset,
}

impl InputEvent {
    /// The capture-ipc message: (type, payload).
    pub fn encode(&self) -> (u32, Vec<u8>) {
        let mut b = Vec::with_capacity(56);
        let f = |b: &mut Vec<u8>, v: f64| b.extend_from_slice(&v.to_le_bytes());
        let u = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
        let t = |b: &mut Vec<u8>, v: u64| b.extend_from_slice(&v.to_le_bytes());
        let kind = match *self {
            InputEvent::PointerAbs { x, y, t_ns } => {
                (f(&mut b, x), f(&mut b, y), t(&mut b, t_ns));
                plugin::INPUT_POINTER_ABS
            }
            InputEvent::PointerRel { dx, dy, dx_unaccel, dy_unaccel, t_ns } => {
                (f(&mut b, dx), f(&mut b, dy), f(&mut b, dx_unaccel), f(&mut b, dy_unaccel), t(&mut b, t_ns));
                plugin::INPUT_POINTER_REL
            }
            InputEvent::Button { button, pressed, t_ns } => {
                (u(&mut b, button), u(&mut b, pressed as u32), t(&mut b, t_ns));
                plugin::INPUT_BUTTON
            }
            InputEvent::Axis { source, axis, value, value120, stop, more, t_ns } => {
                (u(&mut b, source), u(&mut b, axis), f(&mut b, value));
                b.extend_from_slice(&value120.to_le_bytes());
                (u(&mut b, stop as u32 | (more as u32) << 1), t(&mut b, t_ns));
                plugin::INPUT_AXIS
            }
            InputEvent::Key { keycode, pressed, t_ns } => {
                (u(&mut b, keycode), u(&mut b, pressed as u32), t(&mut b, t_ns));
                plugin::INPUT_KEY
            }
            InputEvent::Gesture { kind, phase, fingers, dx, dy, scale, rotation, t_ns } => {
                (u(&mut b, kind), u(&mut b, phase), u(&mut b, fingers), u(&mut b, 0));
                (f(&mut b, dx), f(&mut b, dy), f(&mut b, scale), f(&mut b, rotation), t(&mut b, t_ns));
                plugin::INPUT_GESTURE
            }
            InputEvent::Reset => plugin::INPUT_RESET,
        };
        (kind, b)
    }
}

#[derive(Clone, Debug, Default)]
pub struct MediaStats {
    /// Frames and bytes by class: [passive, interactive, refine].
    pub frames: [u64; 3],
    pub bytes: [u64; 3],
    pub keyframes: u64,
    pub invalidations: u64,
    pub intra_refreshes: u64,
    pub last_encode_ms: f64,
    /// The latest settle times, for the stats and the HUD: ms from a change's
    /// own frame to all its cells being readable (coded at base QP 34 or better) and
    /// lossless (20 or better, or out of passes; 34 under a user-set bandwidth cap).
    /// Only changes that needed refinement count: a keystroke coded well at once doesn't
    /// replace the page flip before it. The same figures ride on the `FrameInfo` of every
    /// frame that completes a change, refined or not. `None` until the first.
    ///
    /// What the HUD should call them (integration.md 4, 5): "readable" is luma SSIM about
    /// 0.987 on dense text, reaching 0.99 about three frames (50 ms) later; "lossless" is every
    /// plane at the QP 20 stop. Both are the whole of the latest change that needed
    /// refinement, with no separate number for the caret's box.
    pub readable_ms: Option<f64>,
    pub lossless_ms: Option<f64>,
    /// The latest change that needed refinement, when it ended parked or at the cap (cleared by a change that ends at the stop):
    /// `lossless_ms` is the stop only. The HUD says "settled" for this.
    pub settled_ms: Option<f64>,
    pub settled_end: Option<Ending>,
    /// The slice bytes of a frame with nothing to code, measured when the encoder opened.
    pub empty_slice_bytes: usize,
    pub kbps: u32,
    pub split: bool,
    pub vbv_pct: u32,
    /// The plugin's last STATS: frames, dropped (no buffer), dropped (no client).
    pub plugin: (u64, u64, u64),
    /// The plugin's capture-ipc version and Hyprland version.
    pub plugin_version: u32,
    pub hyprland: String,
}

pub type EventSink = Arc<dyn Fn(MediaEvent) + Send + Sync>;
/// Called before every frame for core's current budget (e.g. a read of the driver's
/// `LinkStatus` watch). Must be quick and must not block.
pub type RateSource = Arc<dyn Fn() -> RateInput + Send + Sync>;

/// One second of input makes frames latency-critical (protocol.md §7.1).
pub const INTERACTIVE_AFTER_INPUT_NS: u64 = 1_000_000_000;

pub(crate) struct Shared {
    sock: OwnedFd,
    rate: Mutex<Option<RateInput>>,
    rate_source: Mutex<Option<RateSource>>,
    last_input_ns: AtomicU64,
    /// The caret and the pointer, from the plugin's CARET and CURSOR_POS (media-ipc keeps
    /// them) or [`Media::set_focus`].
    focus: Mutex<Focus>,
    stats: Mutex<MediaStats>,
    stopping: AtomicBool,
}

pub struct Media {
    shared: Arc<Shared>,
    tx: mpsc::Sender<encode::ToEnc>,
    threads: Vec<JoinHandle<()>>,
}

impl Media {
    /// Connects to the plugin, starts capturing `cfg.output` and the two threads.
    /// The sink is called from those threads; it must not block.
    pub fn start(cfg: MediaConfig, sink: EventSink) -> Result<Media> {
        let (sock, version, hyprland) = plugin::hello(&cfg.socket)?;
        let shared = Arc::new(Shared {
            sock,
            rate: Mutex::new(None),
            rate_source: Mutex::new(None),
            last_input_ns: AtomicU64::new(0),
            focus: Mutex::new(Focus::default()),
            stats: Mutex::new(MediaStats { plugin_version: version, hyprland, ..Default::default() }),
            stopping: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::channel();
        // The encoder thread opens the GPU and NVENC, and says whether that worked.
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let enc = {
            let (cfg, shared, sink) = (cfg.clone(), shared.clone(), sink.clone());
            std::thread::Builder::new()
                .name("media-enc".into())
                .spawn(move || encode::run(cfg, shared, rx, sink, ready_tx))?
        };
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = enc.join();
                return Err(e);
            }
            Err(_) => return Err(anyhow!("the media encoder thread died at start")),
        }
        plugin::send(&shared.sock, plugin::START, &plugin::start_payload(&cfg.output, 3, 0)?, &[])?;
        let ipc = {
            let (shared, sink, tx) = (shared.clone(), sink.clone(), tx.clone());
            std::thread::Builder::new().name("media-ipc".into()).spawn(move || ipc::run(shared, tx, sink))?
        };
        Ok(Media { shared, tx, threads: vec![enc, ipc] })
    }

    /// Core's latest budget (latest wins). Ignored while a rate source is set.
    pub fn set_rate(&self, r: RateInput) {
        *self.shared.rate.lock().unwrap() = Some(r);
    }

    /// Read core's budget through `f` before every frame instead.
    pub fn set_rate_source(&self, f: RateSource) {
        *self.shared.rate_source.lock().unwrap() = Some(f);
    }

    /// Where the user is looking, in buffer pixels: the focused text field's caret and the
    /// pointer's hotspot (`None`: none). On a measured bottleneck (`RateInput::c_est_bps`)
    /// the cells nearest to them get the lowest QP when refinement frames are coded.
    ///
    /// media's ipc thread already keeps both from the plugin's CARET and CURSOR_POS, so a
    /// session need not call this; it replaces both when it does (latest wins, from either).
    pub fn set_focus(&self, caret: Option<Rect>, pointer: Option<(i32, i32)>) {
        *self.shared.focus.lock().unwrap() = Focus { caret, pointer };
    }

    /// Core's `RefHint::Reference`: the client has frame `pts` and nothing after it
    /// is usable. The next frame predicts from `pts` (every later frame invalidated),
    /// or is an IDR if `pts` has left NVENC's DPB or is from before the stream's keyframe
    /// (an intra refresh would not recover a decoder that lacks the references:
    /// integration.md 2).
    pub fn predict_from(&self, pts: u64) {
        let _ = self.tx.send(encode::ToEnc::PredictFrom(pts));
    }

    /// An IDR next frame: core's `RefHint::Keyframe`, or a new client decoder.
    pub fn request_keyframe(&self) {
        let _ = self.tx.send(encode::ToEnc::Keyframe);
    }

    /// One input event to the plugin, now, on this thread.
    pub fn input(&self, ev: InputEvent) -> Result<()> {
        self.shared.last_input_ns.store(monotonic_ns(), Ordering::Relaxed);
        let (kind, payload) = ev.encode();
        plugin::send(&self.shared.sock, kind, &payload, &[]).context("sending input to the plugin")
    }

    /// LOCAL_INPUT (capture-ipc v4): the desk's own keyboards and pointers. 0 normal,
    /// 1 ignored, 2 ignored but desk keys reach the privacy screen (`sodiumshell-privacy`).
    /// A v3 plugin doesn't know it: Ok(false) and nothing sent.
    pub fn local_input(&self, mode: u32) -> Result<bool> {
        if self.shared.stats.lock().unwrap().plugin_version < 4 {
            return Ok(false);
        }
        let mut p = mode.to_le_bytes().to_vec();
        p.extend_from_slice(&0u32.to_le_bytes());
        plugin::send(&self.shared.sock, plugin::LOCAL_INPUT, &p, &[])?;
        Ok(true)
    }

    /// FRAME_CLOCK: put REMOTE-1's frames on the client's vsync grid.
    pub fn frame_clock(&self, phase_offset_ns: i64, refresh_mhz: u32, grace_ns: u32) -> Result<()> {
        let p = plugin::frame_clock_payload(phase_offset_ns, refresh_mhz, grace_ns);
        plugin::send(&self.shared.sock, plugin::FRAME_CLOCK, &p, &[])
    }

    pub fn stats(&self) -> MediaStats {
        self.shared.stats.lock().unwrap().clone()
    }

    /// STOP, and wait for both threads.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if self.shared.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = plugin::send(&self.shared.sock, plugin::STOP, &[], &[]);
        let _ = rustix::net::shutdown(&self.shared.sock, rustix::net::Shutdown::Read);
        let _ = self.tx.send(encode::ToEnc::Stop);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_messages_have_the_contract_sizes() {
        let sizes = [
            (InputEvent::PointerAbs { x: 1.0, y: 2.0, t_ns: 3 }, 64, 24),
            (InputEvent::PointerRel { dx: 0.0, dy: 0.0, dx_unaccel: 0.0, dy_unaccel: 0.0, t_ns: 0 }, 65, 40),
            (InputEvent::Button { button: 0x110, pressed: true, t_ns: 0 }, 66, 16),
            (InputEvent::Axis { source: 1, axis: 0, value: -3.5, value120: 0, stop: true, more: true, t_ns: 0 }, 67, 32),
            (InputEvent::Key { keycode: 30, pressed: false, t_ns: 0 }, 68, 16),
            (InputEvent::Gesture { kind: 1, phase: 0, fingers: 3, dx: 0.0, dy: 0.0, scale: 1.0, rotation: 0.0, t_ns: 0 }, 69, 56),
            (InputEvent::Reset, 70, 0),
        ];
        for (ev, kind, len) in sizes {
            let (k, p) = ev.encode();
            assert_eq!((k, p.len()), (kind, len), "{ev:?}");
        }
        let (_, axis) = InputEvent::Axis { source: 1, axis: 0, value: -3.5, value120: 0, stop: true, more: true, t_ns: 7 }.encode();
        assert_eq!(&axis[20..24], &3u32.to_le_bytes()); // flags: stop | more
    }
}
