// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The Windows backend: a per-user process in the
//! interactive session.
//!
//! - `display`: the outputs (DXGI), their friendly names (DisplayConfig), DPI and rate.
//! - `capture`: DXGI Desktop Duplication per output, with dirty and move rects, the
//!   pointer's shape and position; access loss (mode change, secure desktop) re-creates
//!   it, and a secure desktop is reported as `secure`.
//! - encoders: `encode::Probe` with hevc_nvenc, hevc_amf, hevc_qsv; h264_nvenc, h264_amf, h264_qsv,
//!   h264_mf, then OpenH264 from Cisco's DLL, downloaded on first need (`softvideo`).
//! - `input`: SendInput with set-1 scan codes (`hid`), the virtual desktop's absolute
//!   pointer, wheels, Unicode text.
//! - `audio`: WASAPI loopback of the default render endpoint, 48 kHz stereo.
//! - `clipboard`: the Win32 clipboard: text, HTML, PNG (DIB converted).
//! - `desktop`: the foreground app's process name, the input desktop's name.

pub mod audio;
pub mod capture;
pub mod clipboard;
pub mod desktop;
pub mod display;
pub mod input;

use std::sync::Arc;

use anyhow::Result;
use remote_core::msg::Os;
use windows::Win32::Media::timeBeginPeriod;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

use crate::backend::*;
use crate::encode::{Candidates, Probe};
use crate::softvideo::SoftwareVideo;

pub struct WinBackend {
    probe: Probe,
}

impl WinBackend {
    /// With the standard folders for software video (`%LOCALAPPDATA%\SodiumRemote`), no app to tell.
    pub fn new(encoder: Option<&str>) -> Result<WinBackend> {
        Self::with_software(encoder, SoftwareVideo::platform_default())
    }

    /// With the app's own software video (setting, events and download folder).
    pub fn with_software(encoder: Option<&str>, soft: Arc<SoftwareVideo>) -> Result<WinBackend> {
        unsafe {
            // Physical pixels everywhere (duplication, SendInput, monitor rects).
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            // 1 ms timers: the pacer, core's driver and the media loop's frame clock
            // would otherwise tick at 15.6 ms.
            timeBeginPeriod(1);
        }
        let cands = encoder.map(Candidates::only).unwrap_or_else(Candidates::windows);
        Ok(WinBackend { probe: Probe::with_software(cands, soft) })
    }
}

impl Backend for WinBackend {
    fn os(&self) -> Os {
        Os::Windows
    }
    fn computer_name(&self) -> String {
        desktop::computer_name()
    }
    fn displays(&self) -> Vec<Display> {
        display::list()
    }
    fn open_capture(&self, display: u32, _fps: u32) -> Result<Box<dyn Capture>> {
        Ok(Box::new(capture::DuplCapture::open(display)?))
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
        Ok(Box::new(audio::Loopback::open()?))
    }
    fn clipboard(&self, on_change: ClipSink) -> Result<Arc<dyn Clipboard>> {
        clipboard::start(on_change)
    }
    fn foreground(&self) -> Foreground {
        desktop::foreground()
    }
}
