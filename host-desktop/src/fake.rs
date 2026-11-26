// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! A fake backend for testing the session glue end to end without a desktop: two
//! synthetic displays where a "terminal" scrolls text at 30 Hz for 2 s and then stands
//! still for 1 s (so the 150 ms refresh shows), a pointer circling with two shapes, a
//! 440 Hz tone, an in-memory clipboard and logged input. Encoding is real (the shim's
//! encoders: NVENC HEVC and OpenH264 on Linux). Pure Rust, so it also runs under wine.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use remote_core::msg::{self, ClipFormat, Os};

use crate::backend::*;
#[cfg(not(target_os = "macos"))]
use crate::encode::{Candidates, Probe};
// On the Mac the fake screen goes through VideoToolbox (no FFmpeg there): a session
// test without Screen Recording or Accessibility.
#[cfg(target_os = "macos")]
use crate::mac::vt::Probe;

pub struct FakeBackend {
    probe: Probe,
    input_log: Option<PathBuf>,
    size: (u32, u32),
}

impl FakeBackend {
    #[cfg(not(target_os = "macos"))]
    pub fn new(encoder: Option<&str>, size: (u32, u32), input_log: Option<PathBuf>) -> FakeBackend {
        let cands = match encoder {
            Some(n) => Candidates::only(n),
            None if cfg!(windows) => Candidates::windows(),
            None => Candidates { hevc: vec!["hevc_nvenc"], h264: vec!["openh264"] },
        };
        FakeBackend { probe: Probe::new(cands), input_log, size }
    }

    #[cfg(target_os = "macos")]
    pub fn new(encoder: Option<&str>, size: (u32, u32), input_log: Option<PathBuf>) -> FakeBackend {
        let probe = Probe::new(encoder).unwrap_or_else(|e| {
            eprintln!("sodium-remote-host: {e:#}; trying both encoders");
            Probe::new(None).expect("the default probe")
        });
        FakeBackend { probe, input_log, size }
    }
}

impl Backend for FakeBackend {
    fn os(&self) -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Linux
        }
    }
    fn computer_name(&self) -> String {
        "Fake host".into()
    }
    fn displays(&self) -> Vec<Display> {
        let d = |id, name: &str, (width, height), primary| Display {
            info: msg::DisplayInfo { id, name: name.into(), width, height, scale: 1.0, primary },
            refresh_mhz: 60_000,
        };
        vec![d(1, "Fake display", self.size, true), d(2, "", (1280, 720), false)]
    }
    fn open_capture(&self, display: u32, _fps: u32) -> Result<Box<dyn Capture>> {
        let d = self.displays().into_iter().find(|d| d.info.id == display).ok_or_else(|| anyhow::anyhow!("no display {display}"))?;
        Ok(Box::new(FakeCapture::new(d.info.width, d.info.height)))
    }
    fn encoder_caps(&self) -> EncoderCaps {
        self.probe.caps()
    }
    fn open_encoder(&self, cfg: &EncoderConfig) -> Result<Box<dyn Encoder>> {
        self.probe.open(cfg)
    }
    fn input(&self) -> Result<Box<dyn Input>> {
        let log = match &self.input_log {
            Some(p) => Some(std::fs::File::create(p)?),
            None => None,
        };
        Ok(Box::new(FakeInput { log, t0: Instant::now() }))
    }
    fn audio(&self) -> Result<Box<dyn AudioSource>> {
        Ok(Box::new(Tone { phase: 0.0, next: None }))
    }
    fn clipboard(&self, _on_change: ClipSink) -> Result<Arc<dyn Clipboard>> {
        Ok(Arc::new(FakeClipboard(Mutex::new(ClipEntry {
            items: vec![(ClipFormat::Text, b"Sodium Remote fake host clipboard".to_vec())],
            concealed: false,
        }))))
    }
    fn foreground(&self) -> Foreground {
        Foreground { id: "fake-app.exe".into(), terminal: false }
    }
}

struct FakeCapture {
    pic: Arc<CpuPicture>,
    t0: Instant,
    first: bool,
    next_content: Instant,
    next_pointer: Instant,
    line: u64,
    shape_ibeam: Option<bool>,
}

const TERM: Rect = Rect { x: 96, y: 96, w: 960, h: 600 };
const LINE_H: i32 = 20;

impl FakeCapture {
    fn new(w: u32, h: u32) -> FakeCapture {
        let mut pic = CpuPicture::new(w, h);
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = y * pic.stride + x * 4;
                pic.data[i..i + 4].copy_from_slice(&[(x * 255 / w as usize) as u8, (y * 255 / h as usize) as u8, 0x60, 0xFF]);
            }
        }
        let now = Instant::now();
        FakeCapture { pic: Arc::new(pic), t0: now, first: true, next_content: now, next_pointer: now, line: 0, shape_ibeam: None }
    }

    /// The terminal: lines of "glyphs" (blocks whose pattern depends on the line's
    /// number), scrolled up by one line per frame.
    fn draw_terminal(&mut self) -> Rect {
        let pic = Arc::make_mut(&mut self.pic);
        let r = Rect { x: TERM.x, y: TERM.y, w: TERM.w.min(pic.width as i32 - TERM.x), h: TERM.h.min(pic.height as i32 - TERM.y) };
        let lines = r.h / LINE_H;
        for row in 0..lines {
            let n = self.line + row as u64;
            for y in 0..LINE_H {
                let py = (r.y + row * LINE_H + y) as usize;
                for x in 0..r.w {
                    let px = (r.x + x) as usize;
                    let glyph = (x / 10) as u64;
                    let on = y > 3 && y < 16 && (x % 10) < 7 && glyph < 20 + (n * 7919 % 70) && {
                        let bits = (n.wrapping_mul(2654435761) ^ glyph.wrapping_mul(40503)) >> ((y as u64 * 3 + (x % 10) as u64) % 29);
                        bits & 1 == 1
                    };
                    let i = py * pic.stride + px * 4;
                    let v = if on { [0xD0, 0xD0, 0xD0, 0xFF] } else { [0x20, 0x18, 0x10, 0xFF] };
                    pic.data[i..i + 4].copy_from_slice(&v);
                }
            }
        }
        self.line += 1;
        r
    }
}

impl Capture for FakeCapture {
    fn next(&mut self, timeout: Duration) -> Result<CaptureUpdate> {
        let mut u = CaptureUpdate::default();
        if self.first {
            self.first = false;
            self.draw_terminal();
            u.frame = Some((self.pic.clone() as Arc<dyn Picture>, vec![]));
            u.at = Some(Instant::now());
            return Ok(u);
        }
        let wake = self.next_content.min(self.next_pointer).min(Instant::now() + timeout);
        std::thread::sleep(wake.saturating_duration_since(Instant::now()));
        let now = Instant::now();
        let t = now.duration_since(self.t0).as_secs_f64();
        if now >= self.next_content {
            self.next_content += Duration::from_micros(33_333);
            // 2 s of scrolling, then 1 s still.
            if t % 3.0 < 2.0 {
                let r = self.draw_terminal();
                u.frame = Some((self.pic.clone() as Arc<dyn Picture>, vec![r]));
                u.at = Some(now);
            }
        }
        if now >= self.next_pointer {
            self.next_pointer += Duration::from_micros(16_667);
            let (w, h) = (self.pic.width as f64, self.pic.height as f64);
            let a = t * std::f64::consts::TAU / 4.0;
            u.pointer = Some(PointerState { x: (w / 2.0 + a.cos() * h / 3.0) as i32, y: (h / 2.0 + a.sin() * h / 3.0) as i32, visible: true });
            let ibeam = (t as u64 / 2) % 2 == 1;
            if self.shape_ibeam != Some(ibeam) {
                self.shape_ibeam = Some(ibeam);
                u.shape = Some(if ibeam { ibeam_shape() } else { arrow_shape() });
            }
        }
        Ok(u)
    }
}

fn arrow_shape() -> CursorShape {
    let (w, h) = (12u32, 19u32);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let inside = x <= y.min(11) && !(y > 15 && x > 3);
            let edge = x == 0 || x == y.min(11) || y == h - 1;
            if inside {
                let v = if edge { 0x00 } else { 0xFF };
                let i = ((y * w + x) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&[v, v, v, 0xFF]);
            }
        }
    }
    CursorShape { width: w, height: h, hot_x: 0, hot_y: 0, pixels }
}

fn ibeam_shape() -> CursorShape {
    let (w, h) = (9u32, 18u32);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            if x == 4 || ((y == 0 || y == h - 1) && x != 4) {
                let i = ((y * w + x) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&[0, 0, 0, 0xFF]);
            }
        }
    }
    CursorShape { width: w, height: h, hot_x: 4, hot_y: 9, pixels }
}

struct FakeInput {
    log: Option<std::fs::File>,
    t0: Instant,
}

impl Input for FakeInput {
    fn inject(&mut self, ev: InputOut) {
        if let Some(f) = &mut self.log {
            let _ = writeln!(f, "{},{}", self.t0.elapsed().as_micros(), format!("{ev:?}").replace(',', ";"));
        }
    }
}

/// A 440 Hz tone at -12 dBFS, in real time.
struct Tone {
    phase: f64,
    next: Option<Instant>,
}

impl AudioSource for Tone {
    fn read(&mut self, out: &mut [f32; 960], timeout: Duration) -> Result<bool> {
        let next = *self.next.get_or_insert_with(Instant::now);
        let now = Instant::now();
        if next > now + timeout {
            std::thread::sleep(timeout);
            return Ok(false);
        }
        std::thread::sleep(next.saturating_duration_since(now));
        for i in 0..480 {
            let v = (self.phase.sin() * 0.25) as f32;
            out[2 * i] = v;
            out[2 * i + 1] = v;
            self.phase = (self.phase + std::f64::consts::TAU * 440.0 / 48_000.0) % std::f64::consts::TAU;
        }
        self.next = Some(next + Duration::from_millis(10));
        Ok(true)
    }
}

struct FakeClipboard(Mutex<ClipEntry>);

impl Clipboard for FakeClipboard {
    fn get(&self) -> Option<ClipEntry> {
        Some(self.0.lock().unwrap().clone())
    }
    fn set(&self, entry: ClipEntry) {
        *self.0.lock().unwrap() = entry;
    }
}

/// A fake virtual display driver and display configuration, for the matched display's
/// tests: a monitor shows as display `100 + target` once it is `appear` old (never with
/// None); `only`, `restore`, removals, rescues and pings are recorded.
pub struct FakeDisplays {
    pub state: Mutex<FakeDisplayState>,
    appear: Option<Duration>,
}

#[derive(Default)]
pub struct FakeDisplayState {
    /// The monitors that exist, with when each was made.
    pub monitors: Vec<(crate::vdisplay::Monitor, Instant)>,
    next_target: u32,
    /// The monitor made the only display, until a restore.
    pub only: Option<crate::vdisplay::Key>,
    pub restored: Vec<Vec<u8>>,
    pub removed: Vec<crate::vdisplay::Key>,
    pub rescued: u32,
    pub pings: u32,
}

impl FakeDisplays {
    /// What `save` returns: the physical topology.
    pub const PHYSICAL: &'static [u8] = b"physical";

    pub fn new(appear: Option<Duration>) -> Arc<FakeDisplays> {
        Arc::new(FakeDisplays { state: Mutex::new(FakeDisplayState::default()), appear })
    }

    pub fn display_id(&self, m: &crate::vdisplay::Monitor) -> u32 {
        100 + m.target
    }
}

impl crate::vdisplay::VirtualDisplay for FakeDisplays {
    fn create(&self, key: crate::vdisplay::Key, mode: crate::vdisplay::Mode) -> Result<crate::vdisplay::Monitor> {
        let mut s = self.state.lock().unwrap();
        s.next_target += 1;
        let m = crate::vdisplay::Monitor { key, target: s.next_target, adapter: 1, mode };
        s.monitors.push((m, Instant::now()));
        Ok(m)
    }
    fn set_mode(&self, _m: &crate::vdisplay::Monitor, _mode: crate::vdisplay::Mode) -> Result<bool> {
        Ok(false)
    }
    fn remove(&self, key: crate::vdisplay::Key) {
        let mut s = self.state.lock().unwrap();
        s.monitors.retain(|(m, _)| m.key != key);
        s.removed.push(key);
    }
    fn keep_alive(&self) -> Result<()> {
        self.state.lock().unwrap().pings += 1;
        Ok(())
    }
}

impl crate::vdisplay::Topology for FakeDisplays {
    fn display_of(&self, m: &crate::vdisplay::Monitor) -> Option<u32> {
        let appear = self.appear?;
        let s = self.state.lock().unwrap();
        s.monitors.iter().any(|(x, at)| x.key == m.key && at.elapsed() >= appear).then(|| self.display_id(m))
    }
    fn save(&self) -> Result<Vec<u8>> {
        Ok(Self::PHYSICAL.to_vec())
    }
    fn only(&self, m: &crate::vdisplay::Monitor) -> Result<()> {
        self.state.lock().unwrap().only = Some(m.key);
        Ok(())
    }
    fn restore(&self, saved: &[u8]) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        s.restored.push(saved.to_vec());
        s.only = None;
        Ok(())
    }
    fn rescue_windows(&self) {
        self.state.lock().unwrap().rescued += 1;
    }
}
