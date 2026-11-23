// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The Windows platform layer on `remote-engine`: the window (`win`), the
//! clipboard (`clip`), audio out (`audio`), the key table and the OS facts.

use crate::embed::Embed;
use crate::{audio, clip, d3d, win};
use remote_core::keymap::default_rules;
use remote_engine::clipboard::ClipboardBackend;
use remote_engine::msg::{DecodeSupport, KeyRule, Os};
use remote_engine::{App, Platform, WinMsg, Waker, WindowOptions};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Media::timeBeginPeriod;
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE};
use windows::core::w;

pub struct AudioOptions {
    pub enabled: bool,
    pub log: Option<PathBuf>,
}

/// Real pixels (per-monitor DPI aware) and 1 ms timers, for the whole process.
pub fn init_process() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        timeBeginPeriod(1);
    }
}

/// An event the window loop waits on next to its messages.
struct EventWake(isize);

impl Waker for EventWake {
    fn wake(&self) {
        unsafe {
            let _ = SetEvent(HANDLE(self.0 as *mut c_void));
        }
    }
}

pub struct Windows {
    wake: Arc<EventWake>,
    clip: Arc<clip::WinClipboard>,
    audio: AudioOptions,
    embed: Option<Arc<Embed>>,
}

impl Windows {
    /// For the Qt app: the picture goes on a child window of `embed.parent`.
    pub fn embedded(audio: AudioOptions, embed: Arc<Embed>) -> anyhow::Result<Windows> {
        let mut w = Windows::new(audio)?;
        w.embed = Some(embed);
        Ok(w)
    }

    pub fn new(audio: AudioOptions) -> anyhow::Result<Windows> {
        unsafe {
            let ev = CreateEventW(None, false, false, None)?;
            // The clipboard's owner: a message-only window nobody sees.
            let owner = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("Sodium Remote clipboard"), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None)?;
            Ok(Windows { wake: Arc::new(EventWake(ev.0 as isize)), clip: Arc::new(clip::WinClipboard::new(owner)), audio, embed: None })
        }
    }
}

impl Platform for Windows {
    fn client_os(&self) -> Os {
        Os::Windows
    }

    /// FFmpeg here decodes AV1 only through hardware: say so only when the GPU does.
    fn adjust_decode_support(&self, d: &mut DecodeSupport) {
        d.av1 = d.av1 && d3d::hardware_av1();
    }

    fn key_table(&self, host_os: Os) -> Vec<KeyRule> {
        default_rules(Os::Windows, host_os)
    }

    /// Not checked on Windows yet: the timeline's "Waiting for the network…" never shows.
    fn has_default_route(&self) -> bool {
        true
    }

    fn waker(&self) -> Arc<dyn Waker> {
        self.wake.clone()
    }

    fn clipboard(&self) -> Option<Arc<dyn ClipboardBackend>> {
        Some(self.clip.clone())
    }

    fn run_window(&self, app: Arc<App>, rx: Receiver<WinMsg>, opts: WindowOptions) -> anyhow::Result<()> {
        win::run_with(app, rx, opts, HANDLE(self.wake.0 as *mut c_void), self.embed.clone())
    }

    fn start_services(&self, app: &Arc<App>) {
        audio::start(app, &self.audio);
    }
}

/// For the Qt app, whose process is already DPI aware: only the 1 ms timers.
pub fn init_embedded() {
    unsafe {
        timeBeginPeriod(1);
    }
}
