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
//! - `sudovda`, `topology`: the matched display (`vdisplay`): the display driver's IOCTLs, and
//!   `SetDisplayConfig` without saving; `vdd_install`: the driver's install and removal (elevated).
//!
//! One host per computer ([`HostLock`]): the screen, the input and the matched display's
//! monitors are the whole machine's, whichever user's session the host runs in.

pub mod audio;
pub mod capture;
pub mod clipboard;
pub mod desktop;
pub mod display;
pub mod input;
pub mod sudovda;
pub mod topology;
pub mod vdd_install;

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
    _lock: HostLock,
}

/// Another Sodium Remote host (the app's "This computer" in this or another user's session, or
/// `sodium-remote-host`) is already running on this computer. The apps recognise it with
/// `downcast_ref` and show their own translated line.
#[derive(Debug)]
pub struct HostInUse;

impl std::fmt::Display for HostInUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("another Sodium Remote host is already sharing this computer: quit it (the app's \"This computer\" switch, or the sodium-remote-host program) and try again")
    }
}

impl std::error::Error for HostInUse {}

/// A machine-wide named mutex, `Global\SodiumRemote.Host`, for as long as it lives: Windows
/// deletes it with the last handle, however the process ends.
#[derive(Debug)]
pub struct HostLock(windows::Win32::Foundation::HANDLE);

// Only closed, from any thread.
unsafe impl Send for HostLock {}
unsafe impl Sync for HostLock {}

impl HostLock {
    pub fn take() -> Result<HostLock> {
        use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GetLastError};
        use windows::Win32::System::Threading::CreateMutexW;
        match unsafe { CreateMutexW(None, false, windows::core::w!("Global\\SodiumRemote.Host")) } {
            Ok(h) if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS => {
                let _ = unsafe { CloseHandle(h) };
                Err(HostInUse.into())
            }
            Ok(h) => Ok(HostLock(h)),
            // Another user's host made it: its mutex isn't open to this user.
            Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => Err(HostInUse.into()),
            Err(e) => Err(anyhow::Error::new(e).context("creating the host's lock")),
        }
    }
}

impl Drop for HostLock {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

impl WinBackend {
    /// With the standard folders for software video (`%LOCALAPPDATA%\SodiumRemote`), no app to tell.
    pub fn new(encoder: Option<&str>) -> Result<WinBackend> {
        Self::with_software(encoder, SoftwareVideo::platform_default())
    }

    /// With the app's own software video (setting, events and download folder).
    /// `HostInUse` when another host runs on this computer.
    pub fn with_software(encoder: Option<&str>, soft: Arc<SoftwareVideo>) -> Result<WinBackend> {
        let lock = HostLock::take()?;
        unsafe {
            // Physical pixels everywhere (duplication, SendInput, monitor rects).
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            // 1 ms timers: the pacer, core's driver and the media loop's frame clock
            // would otherwise tick at 15.6 ms.
            timeBeginPeriod(1);
        }
        let cands = encoder.map(Candidates::only).unwrap_or_else(Candidates::windows);
        Ok(WinBackend { probe: Probe::with_software(cands, soft), _lock: lock })
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
    fn topology(&self) -> Option<Arc<dyn crate::vdisplay::Topology>> {
        Some(Arc::new(topology::WinTopology::new()))
    }
    fn virtual_display(&self) -> Result<Arc<dyn crate::vdisplay::VirtualDisplay>> {
        Ok(Arc::new(sudovda::SudoVda::open()?))
    }
}
