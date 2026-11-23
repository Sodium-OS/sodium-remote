// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Remote's client stream engine: everything a Remote
//! client does that isn't a window, an input device, a cursor, a clipboard or a
//! speaker. It pairs and connects through `remote_core::connect`, runs the
//! session loop (Hello, key table, events), decodes HEVC / H.264 / AV1 through
//! the FFmpeg shim, keeps the stats, the latency harness, the connection
//! timeline and the clipboard sync, and drives scripted tests.
//!
//! A platform layer (`client-linux`, `client-windows`, later the Qt app)
//! implements [`Platform`]: it says what the OS is, shows the pictures that
//! land in [`App::latest`] (latest frame wins), feeds input into
//! [`App::send_input`], draws the cursor from [`WinMsg`], and offers a
//! [`ClipboardBackend`]. The public surface is small:
//!
//! - connect / pair: [`client::run`], [`client::pair`] (blocking, CLI-shaped), or
//!   [`connect::start`] for an app that drives `remote_core::connect::Client` itself;
//! - events to the surface: [`WinMsg`] (stream format, cursor, pointer lock, secure screen);
//! - settings: [`client::Options`] (`fps`, `max_mbps`, `takeover`, `codec`, window options);
//! - displays: [`App::host`] (`Welcome.displays`, `display_mode`) and [`App::select_display`];
//! - input: [`App::send_input`], [`App::send_input_at`], [`App::send_msg`];
//! - the present surface: [`App::latest`], [`App::display`], [`App::stats`], [`layout`].

pub mod clipboard;
pub mod client;
pub mod connect;
pub mod decode;
pub mod dump;
pub mod latency;
pub mod layout;
pub mod proto;
pub mod script;
pub mod stats;
pub mod status;

pub use remote_core::msg;

use connect::SessionClock;
use decode::Picture;
use msg::{ClientDisplay, CursorImage, InputEvent, Msg};
use proto::StreamInfo;
use remote_core::connect::Client;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Wakes the platform's UI loop from other threads (an eventfd on Linux, a
/// window message on Windows).
pub trait Waker: Send + Sync {
    fn wake(&self);
}

/// What the engine needs from the OS it runs on.
pub trait Platform: Send + Sync + 'static {
    /// `Hello.client_os`.
    fn client_os(&self) -> msg::Os;
    /// `Hello.client_os_flavor`: the Linux distribution's os-release `ID`; empty elsewhere.
    fn client_os_flavor(&self) -> String {
        String::new()
    }
    /// Corrects the engine's decoder probe (`decode::probe_support`) with what
    /// only the platform knows, e.g. Windows: AV1 only when the GPU decodes it.
    fn adjust_decode_support(&self, _d: &mut msg::DecodeSupport) {}
    /// The key table for this host (`remote_core::keymap::default_rules`); sent
    /// when `Welcome` names the host's OS. Empty is PC passthrough.
    fn key_table(&self, _host_os: msg::Os) -> Vec<msg::KeyRule> {
        vec![]
    }
    /// The machine has a default route (the timeline's "Waiting for the network…").
    fn has_default_route(&self) -> bool;
    /// Wakes the loop `run_window` runs.
    fn waker(&self) -> Arc<dyn Waker>;
    /// The local clipboard, when the platform has one to sync.
    fn clipboard(&self) -> Option<Arc<dyn clipboard::ClipboardBackend>> {
        None
    }
    /// Runs the window (on the calling thread) until it closes or `app.quitting()`:
    /// shows `app.latest`, publishes `app.display`, sends input, plays the
    /// [`WinMsg`]s. Called once, after the session starts.
    fn run_window(&self, app: Arc<App>, rx: Receiver<WinMsg>, opts: WindowOptions) -> anyhow::Result<()>;
    /// The session's progress, for apps that draw their own chrome (default: nothing).
    fn on_status(&self, _s: SessionStatus) {}
    /// Started once with the app, before the window (audio out, for one).
    fn start_services(&self, _app: &Arc<App>) {}
}

/// What the session reports to an app that shows more than a picture (the Qt app): where the
/// connection is, and why it ended. Never text: the app words it.
#[derive(Clone, Debug)]
pub enum SessionStatus {
    /// `connect` was called.
    Connecting,
    /// `Connect` failed: the card to show.
    ConnectFailed(remote_core::connect::ConnectError),
    /// The handshake finished (the route's kind and rtt).
    Connected { route: Option<remote_core::connect::RouteInfo> },
    /// `Welcome` arrived.
    Welcome(Box<msg::Welcome>),
    /// The connection died and the core is retrying.
    Reconnecting,
    /// The session is over.
    Ended(msg::DisconnectReason),
}

/// Messages from the network side to the window.
pub enum WinMsg {
    Stream(StreamInfo),
    CursorShape { hash: u64, image: CursorImage },
    CursorPos { x: f32, y: f32, visible: bool, hash: u64 },
    PointerLock(msg::PointerLock),
    /// The host's secure screen (lock, login, UAC) is up or gone.
    SecureScreen(bool),
}

/// How the window sizes the stream.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: String,
    pub fullscreen: bool,
    pub hud: bool,
    /// Fixed resolution: never send `Resize`; the picture is scaled.
    pub fixed: bool,
    /// A forced stream size (tests): `Hello` asks for it and nothing else.
    pub size: Option<(u32, u32)>,
    /// `ClientDisplay::pixels_per_point` (default: the output's scale).
    pub ppp: Option<f32>,
}

/// The decoder's newest picture: latest frame wins.
#[derive(Default)]
pub struct Latest {
    slot: Mutex<Option<(Picture, u32)>>,
}

impl Latest {
    /// The newest picture replaces an unshown one.
    pub fn put(&self, p: Picture, frame_id: u32) {
        self.slot.lock().unwrap().replace((p, frame_id));
    }
    pub fn take(&self) -> Option<(Picture, u32)> {
        self.slot.lock().unwrap().take()
    }
    pub fn has(&self) -> bool {
        self.slot.lock().unwrap().is_some()
    }
}

/// The live connection's session clock against the client's own clock: every
/// (re)connect starts a new session clock (`Client::session_epoch`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Live {
    /// Counts connections, so per-session state (vsync leads) can start over.
    pub generation: u64,
    /// The session epoch on the client clock (µs): client µs = session µs + this.
    pub offset_us: i64,
}

/// What the client's threads share.
pub struct App {
    pub client: Client,
    /// The client's own clock for every stats file (fixed for the run; its
    /// epoch is the start of the run). Session times are converted to it.
    pub clock: SessionClock,
    live: Mutex<Option<Live>>,
    pub stats: Arc<stats::Stats>,
    pub click: Option<Arc<latency::ClickTest>>,
    pub latest: Arc<Latest>,
    pub wake: Arc<dyn Waker>,
    /// The display `Hello` and `Resize` describe (the window sets it; headless, the options).
    pub display: Mutex<Option<ClientDisplay>>,
    /// The connection timeline, for the HUD.
    pub timeline: Mutex<status::Timeline>,
    /// Every input event sent, for checking the input path end to end.
    pub input_log: Option<Mutex<std::fs::File>>,
    /// The host as `Welcome` described it (OS, display mode, displays).
    pub host: Mutex<Option<msg::Welcome>>,
    /// The local clipboard sync, when the platform has a clipboard.
    pub clipboard: Option<Arc<clipboard::ClipSync>>,
    /// The host's secure screen is up.
    pub secure_screen: AtomicBool,
    /// The first picture was decoded (the script starts from it).
    pub first_picture: AtomicBool,
    /// What `Hello.decode` said this client decodes (probed at start, then narrowed by the codec choice).
    pub decode_support: Mutex<msg::DecodeSupport>,
    /// The host follows `Resize` (`Welcome.live_resize`); until it says
    /// otherwise, yes. When not, never send `Resize`: the window fits the picture.
    live_resize: AtomicBool,
    quit: AtomicBool,
    exit_code: AtomicI32,
}

impl App {
    pub fn new(
        client: Client,
        clock: SessionClock,
        stats: Arc<stats::Stats>,
        click: Option<Arc<latency::ClickTest>>,
        input_log: Option<std::fs::File>,
        wake: Arc<dyn Waker>,
        clipboard: Option<Arc<dyn clipboard::ClipboardBackend>>,
    ) -> App {
        let sync = clipboard.map(|b| {
            let c = client.clone();
            Arc::new(clipboard::ClipSync::new(b, Box::new(move |m| c.send(m))))
        });
        App {
            client,
            clock,
            live: Mutex::new(None),
            stats,
            click,
            latest: Arc::new(Latest::default()),
            wake,
            display: Mutex::new(None),
            timeline: Mutex::new(status::Timeline::default()),
            input_log: input_log.map(Mutex::new),
            host: Mutex::new(None),
            clipboard: sync,
            secure_screen: AtomicBool::new(false),
            first_picture: AtomicBool::new(false),
            decode_support: Mutex::new(msg::DecodeSupport { hevc: true, h264: true, hevc_444: true, av1: false, max_width: 0, max_height: 0 }),
            live_resize: AtomicBool::new(true),
            quit: AtomicBool::new(false),
            exit_code: AtomicI32::new(-1),
        }
    }

    /// Switches the host to another of `Welcome.displays` (a picker is for
    /// `DisplayMode::Physical` hosts with several displays only).
    pub fn select_display(&self, id: u32) {
        self.send_msg(Msg::SelectDisplay(id));
    }

    pub fn set_live_resize(&self, on: bool) {
        self.live_resize.store(on, Ordering::Relaxed);
    }

    pub fn live_resize(&self) -> bool {
        self.live_resize.load(Ordering::Relaxed)
    }

    /// A connection came up whose session clock starts at `epoch`.
    pub fn on_connected(&self, epoch: Instant) -> Live {
        let mut l = self.live.lock().unwrap();
        let generation = l.map_or(1, |l| l.generation + 1);
        let live = Live { generation, offset_us: self.clock.us_signed(epoch) };
        *l = Some(live);
        live
    }

    pub fn live(&self) -> Option<Live> {
        *self.live.lock().unwrap()
    }

    /// One input event that happened now. Returns when it was handed over (client µs).
    pub fn send_input(&self, ev: InputEvent) -> u64 {
        self.send_input_at(ev, None)
    }

    /// One input event with the platform's event time, if it has one: the host
    /// keeps the user's spacing (double clicks, kinetic scrolling).
    pub fn send_input_at(&self, ev: InputEvent, at: Option<Instant>) -> u64 {
        let t_us = self.clock.now_us();
        match at {
            Some(at) => self.client.send_input_at(&ev, at),
            None => self.client.send_input(&ev),
        };
        if let Some(l) = &self.input_log {
            use std::io::Write;
            let _ = writeln!(l.lock().unwrap(), "{t_us},{}", format!("{ev:?}").replace(',', ";"));
        }
        t_us
    }

    /// One control message (reliable, ordered; remembered for reconnects where
    /// the core does that: `Resize`, `Settings`, `KeyTable`).
    pub fn send_msg(&self, m: Msg) {
        self.client.send(m);
    }

    /// The user ends it (exit 0, unless the session already ended otherwise).
    pub fn quit(&self) {
        self.quit_with(0);
    }

    /// Quit with this process exit code (the first reason wins).
    pub fn quit_with(&self, code: i32) {
        let _ = self.exit_code.compare_exchange(-1, code, Ordering::SeqCst, Ordering::SeqCst);
        self.quit.store(true, Ordering::SeqCst);
        self.wake.wake();
    }

    pub fn quitting(&self) -> bool {
        self.quit.load(Ordering::SeqCst)
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code.load(Ordering::SeqCst).max(0)
    }
}
