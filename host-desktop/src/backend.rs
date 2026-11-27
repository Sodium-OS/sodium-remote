// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The backend: everything a host does that depends on the OS.
//! The session and media glue (`session`, `media`) are written against these traits
//! only, so a backend never sees `connect::Host` and the traits hold no OS types.
//!
//! - **Windows** (`win`): DXGI Desktop Duplication, FFmpeg/OpenH264 through the C shim,
//!   SendInput, WASAPI loopback, the Win32 clipboard.
//! - **Fake** (`fake`): synthetic frames, for testing the glue end to end on Linux.
//! - **macOS** (`mac`): ScreenCaptureKit gives IOSurface frames, which its own `Picture`
//!   carries (`Picture::as_any`) to a VideoToolbox `Encoder`; `bgra()` is `None` there.
//!   Audio comes from ScreenCaptureKit through `AudioSource`.
//! - **Generic Linux** (`linux`, feature `linux-portal`): the desktop portal's
//!   RemoteDesktop and ScreenCast over PipeWire, libei, the portal clipboard. The
//!   person at the computer allows sharing in the system's dialog ([`Consent`]); a
//!   capture ends with [`SharingEnded`] when they stop it.

use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, Instant};

use remote_core::msg::{self, ClipFormat, Os};

/// A rectangle in display pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// One display the host can stream.
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    /// `id`, `name` ("" when the OS has no friendly name: clients say "Display %1"),
    /// pixel size, scale, primary.
    pub info: msg::DisplayInfo,
    /// Its refresh rate, mHz.
    pub refresh_mhz: u32,
}

/// A captured picture. CPU backends hold BGRA rows; others hold their own surface and
/// hand it to their own encoder through `as_any`.
pub trait Picture: Send + Sync {
    fn size(&self) -> (u32, u32);
    /// BGRA8 (B, G, R, A in memory) and the row stride in bytes, in system memory.
    fn bgra(&self) -> Option<(&[u8], usize)>;
    fn as_any(&self) -> &dyn Any;
}

/// A BGRA picture in system memory.
#[derive(Clone)]
pub struct CpuPicture {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub data: Vec<u8>,
}

impl CpuPicture {
    pub fn new(width: u32, height: u32) -> CpuPicture {
        let stride = width as usize * 4;
        CpuPicture { width, height, stride, data: vec![0; stride * height as usize] }
    }
}

impl Picture for CpuPicture {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    fn bgra(&self) -> Option<(&[u8], usize)> {
        Some((&self.data, self.stride))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The pointer as the capture sees it: the hotspot in display pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerState {
    pub x: i32,
    pub y: i32,
    pub visible: bool,
}

/// A pointer shape: premultiplied BGRA (`msg::CursorImage`'s layout), display pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorShape {
    pub width: u32,
    pub height: u32,
    pub hot_x: i32,
    pub hot_y: i32,
    pub pixels: Vec<u8>,
}

/// What one `Capture::next` saw; any part may be missing.
#[derive(Default)]
pub struct CaptureUpdate {
    /// The whole current picture, and what changed since the previous one (display
    /// pixels; empty means everything).
    pub frame: Option<(Arc<dyn Picture>, Vec<Rect>)>,
    /// When the picture was captured.
    pub at: Option<Instant>,
    pub pointer: Option<PointerState>,
    pub shape: Option<CursorShape>,
    /// A secure desktop (lock, login, UAC) came up (true) or went away (false).
    pub secure: Option<bool>,
}

/// One display's capture. Runs on its own thread.
pub trait Capture: Send {
    /// Waits up to `timeout` for something new. The first call returns the whole
    /// picture. An error ends this capture; the media loop opens a new one.
    fn next(&mut self, timeout: Duration) -> anyhow::Result<CaptureUpdate>;
}

/// The encoder's set-up.
#[derive(Clone, Debug, PartialEq)]
pub struct EncoderConfig {
    pub codec: msg::Codec,
    /// The encoded size (even): the display's, or smaller to fit the client's window.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u64,
}

pub struct EncodedFrame {
    /// Annex B, one access unit.
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub convert_us: u32,
    pub encode_us: u32,
}

pub trait Encoder: Send {
    /// The implementation ("hevc_nvenc", "h264_mf", "openh264", ...).
    fn name(&self) -> &str;
    fn codec(&self) -> msg::Codec;
    /// The rate given to `encode` applies to that frame at no cost. When false, the rate
    /// is fixed at open: the media loop opens a new encoder for a new rate.
    fn dynamic_rate(&self) -> bool;
    /// Encodes one picture (scaled to the encoder's size when it differs).
    fn encode(&mut self, picture: &dyn Picture, keyframe: bool, bitrate_bps: u64) -> anyhow::Result<EncodedFrame>;
}

/// What the host can encode, found by opening each candidate once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EncoderCaps {
    /// A hardware HEVC encoder.
    pub hevc: bool,
    pub h264: bool,
}

/// Input to inject, after the key table: platform-free.
#[derive(Clone, Debug, PartialEq)]
pub enum InputOut {
    /// A HID usage (keyboard page) to press or release.
    Key { usage: u16, down: bool },
    /// Committed IME text, typed as Unicode.
    Text(String),
    /// Absolute position in `display`'s pixels.
    PointerAbs { display: u32, x: f64, y: f64 },
    /// Relative motion (pointer lock), pixels.
    PointerRel { dx: f64, dy: f64 },
    Button { button: msg::Button, down: bool },
    /// Scroll as `msg::InputEvent::Axis` has it (Wayland's sign: positive is down/right):
    /// `stop` ends a finger scroll, `more` says the next Axis belongs to the same pointer
    /// frame (both axes of one trackpad event).
    Axis { source: msg::AxisSource, axis: msg::Axis, value: f64, value120: i32, stop: bool, more: bool },
    /// A pinch: the cumulative scale since `Begin`.
    Zoom { phase: msg::GesturePhase, scale: f64 },
    /// Release every key and button this injector holds.
    ReleaseAll,
}

pub trait Input: Send {
    fn inject(&mut self, ev: InputOut);
}

/// Desktop audio: 10 ms at a time.
pub trait AudioSource: Send {
    /// Waits up to `timeout` for the next 10 ms: 480 interleaved stereo f32 frames at
    /// 48 kHz. False on a timeout (nothing playing).
    fn read(&mut self, out: &mut [f32; 960], timeout: Duration) -> anyhow::Result<bool>;
}

/// A clipboard's contents, in protocol formats.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipEntry {
    pub items: Vec<(ClipFormat, Vec<u8>)>,
    pub concealed: bool,
}

/// Called with the host's new clipboard (not for changes `Clipboard::set` made).
pub type ClipSink = Arc<dyn Fn(ClipEntry) + Send + Sync>;

pub trait Clipboard: Send + Sync {
    /// The host's clipboard now (offered when a client connects).
    fn get(&self) -> Option<ClipEntry>;
    /// Puts the client's clipboard on the host's.
    fn set(&self, entry: ClipEntry);
}

/// The foreground app, for the key table's `Terminal` and `Class` conditions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Foreground {
    /// The process image name on Windows, the bundle ID on macOS.
    pub id: String,
    pub terminal: bool,
}

/// Why `Backend::check_ready` refuses a client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotReady {
    /// The OS hasn't allowed something this stream needs: sent as the typed
    /// `DisconnectReason::PermissionMissing`, never as text.
    Permission { screen_recording: bool, accessibility: bool },
    /// The person at this computer declined the system's sharing dialog (generic
    /// Linux): sent as `DisconnectReason::EndedAtComputer`.
    Declined,
    /// This computer is locked (generic Linux: logind's `LockedHint`): sent as `DisconnectReason::HostLocked`.
    Locked,
    /// Anything else, for the client's log.
    Other(String),
}

/// A capture's error when the person at this computer stopped sharing (the portal
/// session closed, the stream's node went away): the session ends with
/// `DisconnectReason::EndedAtComputer` instead of re-opening the capture.
#[derive(Debug)]
pub struct SharingEnded;

impl std::fmt::Display for SharingEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("sharing was stopped at this computer")
    }
}

impl std::error::Error for SharingEnded {}

/// Whether this computer may be shown, where the OS asks the person at the computer
/// each time (generic Linux: the portal's dialog, remembered by a restore token).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Consent {
    /// Nothing remembered: the system's dialog appears when a device next connects.
    Ask,
    /// The dialog is up, waiting for the person at this computer.
    Asking,
    /// Allowed and remembered: devices connect without a dialog.
    Remembered,
    /// The person at this computer declined (or closed) the last dialog.
    Declined,
    /// This desktop can't share (no portal with RemoteDesktop and ScreenCast); the
    /// detail is for the log.
    Unavailable(String),
}

pub trait Backend: Send + Sync {
    fn os(&self) -> Os;
    /// The Linux distribution's os-release `ID` (`Welcome.host_os_flavor`); empty elsewhere.
    fn os_flavor(&self) -> String {
        String::new()
    }
    /// This computer's name, as devices show it.
    fn computer_name(&self) -> String;
    fn displays(&self) -> Vec<Display>;
    /// A capture of `display`; `fps` is the stream's rate (a capture that can be told
    /// its rate, as ScreenCaptureKit's, delivers no faster).
    fn open_capture(&self, display: u32, fps: u32) -> anyhow::Result<Box<dyn Capture>>;
    fn encoder_caps(&self) -> EncoderCaps;
    /// Whether the encoder scales any picture to any even size, so the stream can follow
    /// the client's window (`Welcome.live_resize`). Every backend's does.
    fn live_resize(&self) -> bool {
        true
    }
    fn open_encoder(&self, cfg: &EncoderConfig) -> anyhow::Result<Box<dyn Encoder>>;
    fn input(&self) -> anyhow::Result<Box<dyn Input>>;
    fn audio(&self) -> anyhow::Result<Box<dyn AudioSource>>;
    fn clipboard(&self, on_change: ClipSink) -> anyhow::Result<Arc<dyn Clipboard>>;
    fn foreground(&self) -> Foreground;
    /// Why this computer can't be shown now, for the client (macOS: Screen Recording
    /// isn't allowed). Checked when a client connects.
    fn check_ready(&self) -> Result<(), NotReady> {
        Ok(())
    }
    /// Input can't be injected right now (macOS without Accessibility): the picture is
    /// served, `Welcome.view_only` is set, and the host's pane shows the same state.
    /// Read again whenever the status is worked out, so an Allow shows at once.
    fn view_only(&self) -> bool {
        false
    }
    /// A session started (true) or ended (false): keep the display awake meanwhile
    /// (macOS: an IOPMAssertion; generic Linux: the portal session closes at the end).
    fn session_active(&self, _on: bool) {}
    /// The sharing permission, where the OS asks for it per session (generic Linux);
    /// None elsewhere.
    fn consent(&self) -> Option<Consent> {
        None
    }
    /// Forgets a remembered permission: the next connection shows the dialog again.
    fn forget_consent(&self) {}
    /// Whether this computer is locked, read now (generic Linux: logind's `LockedHint`, read again once after
    /// about 300 ms when it says unlocked). Asked when sharing ended without the person choosing to stop it.
    fn locked(&self) -> bool {
        false
    }
    /// The OS's display configuration, for the matched display (`vdisplay`; Windows). None
    /// where there is no matched display.
    fn topology(&self) -> Option<Arc<dyn crate::vdisplay::Topology>> {
        None
    }
    /// The virtual display driver, opened (Windows: SudoVDA); an error when it isn't
    /// installed or doesn't answer.
    fn virtual_display(&self) -> anyhow::Result<Arc<dyn crate::vdisplay::VirtualDisplay>> {
        anyhow::bail!("this computer has no virtual display driver")
    }
}
