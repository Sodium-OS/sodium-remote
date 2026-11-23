// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Remote's Linux reference client: the Linux platform
//! layer on `remote-engine`. The engine pairs and
//! connects through `remote_core::connect`, decodes HEVC / H.264 / AV1 through
//! FFmpeg and runs the session; this crate presents in a Wayland window (latest
//! frame wins, with wp_presentation stamps and vsync reports for the phase
//! lock) or headless, draws the cursor locally from the cursor channel, sends
//! input, and the click-to-photon test runs in the engine.

pub mod audio;
pub mod clip;
pub mod connect;
pub mod hid;
pub mod window;

pub use remote_engine::{App, Live, Platform, WinMsg, Waker, client, decode, latency, msg, proto, stats, status};

use remote_engine::clipboard::ClipboardBackend;
use remote_engine::{WindowOptions, msg::Os};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

/// Wakes the window's poll loop from other threads (an eventfd).
pub struct Wake(OwnedFd);

impl Wake {
    pub fn new() -> std::io::Result<Wake> {
        let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Wake(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
    pub fn drain(&self) {
        let mut v = 0u64;
        unsafe { libc::read(self.0.as_raw_fd(), (&mut v as *mut u64).cast(), 8) };
    }
    pub fn fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl Waker for Wake {
    fn wake(&self) {
        let one = 1u64;
        unsafe { libc::write(self.0.as_raw_fd(), (&one as *const u64).cast(), 8) };
    }
}

/// The Linux platform layer: a Wayland window and the compositor's input
/// (text-input-v3 for the input method's commits), the Wayland clipboard and
/// PipeWire audio out.
pub struct Linux {
    wake: Arc<Wake>,
    clip: Option<Arc<clip::WlClipboard>>,
    audio: audio::AudioOptions,
}

/// The session's clipboard and sound, which a headless test run leaves off: the
/// shell it runs from is usually the user's own desktop.
pub fn services(clipboard: bool) -> Option<Arc<clip::WlClipboard>> {
    if !clipboard {
        return None;
    }
    match clip::WlClipboard::start() {
        Ok(c) => Some(Arc::new(c)),
        Err(e) => {
            eprintln!("remote-client: no clipboard sync: {e:#}");
            None
        }
    }
}

impl Linux {
    /// No clipboard and no sound (headless runs and tests).
    pub fn new() -> std::io::Result<Linux> {
        Linux::with(false, audio::AudioOptions::default())
    }

    pub fn with(clipboard: bool, audio: audio::AudioOptions) -> std::io::Result<Linux> {
        Ok(Linux { wake: Arc::new(Wake::new()?), clip: services(clipboard), audio })
    }
}

impl Platform for Linux {
    fn client_os(&self) -> Os {
        Os::Linux
    }
    fn client_os_flavor(&self) -> String {
        remote_core::os_release::flavor()
    }
    /// The same table the SodiumOS app sends: a Mac host gets
    /// the PC keyboard's rules (Ctrl acts as Cmd), the same family passes through.
    fn key_table(&self, host_os: Os) -> Vec<remote_engine::msg::KeyRule> {
        remote_core::keymap::default_rules(Os::Linux, host_os)
    }
    fn has_default_route(&self) -> bool {
        connect::has_default_route()
    }
    fn waker(&self) -> Arc<dyn Waker> {
        self.wake.clone()
    }
    fn clipboard(&self) -> Option<Arc<dyn ClipboardBackend>> {
        self.clip.clone().map(|c| c as Arc<dyn ClipboardBackend>)
    }
    fn start_services(&self, app: &Arc<App>) {
        audio::start(app, &self.audio);
    }
    fn run_window(&self, app: Arc<App>, rx: Receiver<WinMsg>, opts: WindowOptions) -> anyhow::Result<()> {
        window::run(app, rx, opts, self.wake.clone())
    }
}
