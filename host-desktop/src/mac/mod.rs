// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The macOS backend: pure Rust on the objc2
//! framework crates. It runs in-process in Remote.app (the `ffi` feature) or as
//! `sodium-remote-host --backend mac` for tests; either way the process's main thread
//! must run its run loop (AppKit's, or `run_main_loop` in the program), because NSScreen,
//! NSCursor and NSWorkspace are read there (`main_thread::on_main`).
//!
//! - `display`: CoreGraphics' active displays (backing pixels, points, scale, refresh),
//!   NSScreen's localized names.
//! - `capture`: a ScreenCaptureKit `SCStream` per display: IOSurface-backed NV12 frames
//!   at native pixels with their dirty rects, no cursor in the picture; the pointer's
//!   position (CGEvent) and shape (NSCursor) beside it; the lock screen as `secure`.
//! - `vt`: VideoToolbox encoders (HEVC on the hardware, H.264), low-latency rate
//!   control, per-frame rate, Annex B out.
//! - `input`: CGEvent posting: key codes (`hid::hid_to_mac`) with modifier flags,
//!   absolute pointer in global points, buttons with click counts, scroll with phases,
//!   IME commits as Unicode strings.
//! - `audio`: ScreenCaptureKit's system audio, 48 kHz stereo.
//! - `clipboard`: NSPasteboard: text, HTML, PNG (TIFF converted), polled.
//! - `desktop`: the frontmost app's bundle ID and the terminal list; the computer's name.
//! - `permissions`: Screen Recording and Accessibility (TCC).
//! - `power`: an IOPMAssertion while a session is live.

pub mod audio;
pub mod capture;
pub mod clipboard;
pub mod cursor;
pub mod desktop;
pub mod display;
pub mod input;
pub mod main_thread;
pub mod permissions;
pub mod power;
pub mod vt;

use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use remote_core::msg::Os;

use crate::backend::*;

pub use main_thread::run_main_loop;

pub struct MacBackend {
    probe: vt::Probe,
    power: Mutex<Option<power::DisplayAwake>>,
}

impl MacBackend {
    /// `encoder`: only this one ("videotoolbox_hevc" or "videotoolbox_h264"), for tests.
    pub fn new(encoder: Option<&str>) -> Result<MacBackend> {
        let missing = permissions::missing();
        if !missing.is_empty() {
            eprintln!("sodium-remote-host: {}", permissions::describe_missing(&missing));
        }
        Ok(MacBackend { probe: vt::Probe::new(encoder)?, power: Mutex::new(None) })
    }
}

impl Backend for MacBackend {
    fn os(&self) -> Os {
        Os::Mac
    }
    fn computer_name(&self) -> String {
        desktop::computer_name()
    }
    fn displays(&self) -> Vec<Display> {
        display::list()
    }
    fn open_capture(&self, display: u32, fps: u32) -> Result<Box<dyn Capture>> {
        if !permissions::screen_recording() {
            bail!("{}", permissions::describe_missing(&[permissions::Permission::ScreenRecording]));
        }
        Ok(Box::new(capture::ScCapture::open(display, fps)?))
    }
    fn encoder_caps(&self) -> EncoderCaps {
        self.probe.caps()
    }
    fn open_encoder(&self, cfg: &EncoderConfig) -> Result<Box<dyn Encoder>> {
        self.probe.open(cfg)
    }
    fn input(&self) -> Result<Box<dyn Input>> {
        Ok(Box::new(input::Injector::new()))
    }
    fn audio(&self) -> Result<Box<dyn AudioSource>> {
        if !permissions::screen_recording() {
            bail!("{}", permissions::describe_missing(&[permissions::Permission::ScreenRecording]));
        }
        Ok(Box::new(audio::ScAudio::open()?))
    }
    fn clipboard(&self, on_change: ClipSink) -> Result<Arc<dyn Clipboard>> {
        clipboard::start(on_change)
    }
    fn foreground(&self) -> Foreground {
        desktop::foreground()
    }
    fn check_ready(&self) -> Result<(), NotReady> {
        if permissions::screen_recording() {
            if !permissions::accessibility() {
                // Shown but not driven: say so in the log, serve the picture anyway
                // (`Welcome.view_only` tells the client).
                eprintln!("sodium-remote-host: {}", permissions::describe_missing(&[permissions::Permission::Accessibility]));
            }
            Ok(())
        } else {
            // No Screen Recording: refuse with the typed reason; the client shows its card.
            Err(NotReady::Permission { screen_recording: true, accessibility: !permissions::accessibility() })
        }
    }
    fn view_only(&self) -> bool {
        !permissions::accessibility()
    }
    fn session_active(&self, on: bool) {
        let mut p = self.power.lock().unwrap();
        if on && p.is_none() {
            *p = power::DisplayAwake::new();
        } else if !on {
            *p = None;
        }
    }
}
