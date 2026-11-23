// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The session: one actor that owns everything a connection changes on the host
//! Connections, the D-Bus API, the IME add-on, the
//! clipboard and the plugin all talk to it through [`Cmd`]s; it publishes a
//! [`Status`] that the D-Bus API mirrors as properties.
//!
//! Connecting runs, in order: the display (REMOTE-1, workspaces, the desk's mode),
//! the IME add-on's Activate, the "Remote" sink, the clipboard; the client then
//! hears Accepted. Ending runs the reverse: the clipboard stops, the sink goes and
//! the old default comes back, Deactivate, INPUT_RESET, and the desktop is put back
//! before the "disconnected" notification (RemoteShell SB4: "the desk is restored
//! first, then this").
//!
//! A lost connection holds the session for 2 minutes: the display, the layout and
//! the audio route stay (a short network blip doesn't bounce sound to the desk
//! speakers), the IME add-on is deactivated and input released (so the desk works).
//!
//! A Privacy or Off session ends into the session lock: every end but
//! the desk password's asks logind to lock and waits until the shell says the lock
//! is up (LockedHint) before the desktop comes back ([`lock_desk_first`]).

pub mod core;
pub mod store;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, bail};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    audio::{self, Audio, Mic, Sink},
    clipboard::{ClipEntry, Clipboard},
    display::{Applied, DisplayManager, HomeMode, Request},
    ime::{ImeBridge, ImeEvent},
    input::PluginLink,
    keys::{DeviceKey, PairedDevice, PairedStore},
};
use self::core::{Client, Decision, Phase, SessionCore};
use self::store::{Recent, RecentLog, Settings};

pub const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);
pub const PAIRING_WINDOW: Duration = Duration::from_secs(300);

/// The window's length: 5 minutes. `REMOTED_PAIRING_SECS` shortens it (tests: the expired state).
fn pairing_window() -> Duration {
    std::env::var("REMOTED_PAIRING_SECS").ok().and_then(|v| v.parse().ok()).map_or(PAIRING_WINDOW, Duration::from_secs)
}
pub const PAIRING_TRIES: u32 = 3;

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Why a session ended, as the client words it.
#[derive(Clone, Debug, PartialEq)]
pub enum EndReason {
    /// "Taken over by <device>".
    TakenOver { by: String },
    /// "Disconnected by <this computer's name>" (Settings, the pill, a notification).
    DisconnectedByHost { host: String },
    /// "Ended at the computer" (the password typed on the privacy screen).
    EndedAtComputer,
    /// The device was removed from the paired list.
    Removed,
    /// Remote was turned off (the switch).
    RemoteOff,
    /// The daemon is stopping.
    DaemonStopping,
    /// The client left, or its held session ran out.
    ClientLeft,
}

/// What the host tells a client.
#[derive(Clone, Debug, PartialEq)]
pub enum HostEvent {
    Accepted { display: Applied, took_over_from: Option<String>, locked: bool, home_mode: HomeMode },
    /// Another device is live: ask the user "In use by X. Take over?", then connect with takeover.
    InUse { by: String },
    Refused { reason: &'static str },
    Ended(EndReason),
    Display(Applied),
    HomeMode(HomeMode),
    Ime(ImeEvent),
    Clipboard(ClipEntry),
    /// One 10 ms Opus packet of the "Remote" sink.
    Audio(Vec<u8>),
    /// capture-ipc POINTER_LOCK: 0 none, 1 locked, 2 confined, and the rect.
    PointerLock { state: u32, x: i32, y: i32, w: i32, h: i32 },
}

/// The connection to one client, provided by the transport (core's session).
pub trait ClientLink: Send + Sync {
    fn send(&self, ev: HostEvent);
}

/// What the transport measures, for the HUD, Settings and the pill.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkStats {
    /// "lan", "direct", "vpn", "tailscale", "relay".
    pub path: String,
    /// "udp", "tcp", "tls".
    pub transport: String,
    /// The label of the named network (`networks.toml`) a "vpn" path is in; "" when none.
    pub network: String,
    pub rtt_ms: u32,
    pub bitrate_kbps: u32,
    pub fps: f64,
}

/// The rendezvous, through core's client once it lands.
pub trait Rendezvous: Send + Sync {
    /// Upload the signed paired-client list.
    fn paired_list(&self, payload: Vec<u8>, sig: [u8; 64]);
    /// Open a pairing window under `name`; SPAKE2 runs against `code`, and its
    /// outcome comes back as [`Cmd::PairAttempt`].
    fn pair_open(&self, name: &str, code: &str);
    fn pair_close(&self);
    /// The host is going to sleep (true) or woke up (false): "Asleep since 23:10".
    fn sleep_notice(&self, asleep: bool);
    fn state(&self) -> RelayView;
    /// True when remote-core's `connect::Host` runs pairing (SPAKE2 against the
    /// code remoted shows), owns `paired.json` and uploads the paired list itself:
    /// remoted then only reads the file, and renames and removes through here.
    fn core(&self) -> bool {
        false
    }
    fn rename(&self, _id: &str, _name: &str) -> bool {
        false
    }
    /// Revoke on the rendezvous and drop it if connected; done when the future is.
    fn remove(&self, _id: &str) -> futures_util::future::BoxFuture<'static, bool> {
        Box::pin(async { false })
    }
    /// The name devices see changed (Settings' Name, or the computer's own name
    /// while no Remote-specific one is set): re-announce under it, live.
    fn set_name(&self, _name: &str) {}
    /// "Visible on this network" changed: announce over mDNS, or withdraw, live.
    fn set_mdns(&self, _on: bool) {}
}

/// logind's view of this user's graphical session (`dbus::Logind`; a stand-in in
/// tests): its LockedHint, which the shell's lock sets once every output shows the
/// lock, and Lock(), which the shell's lock answers.
pub trait DeskLock: Send + Sync {
    /// LockedHint; false when unknown.
    fn locked(&self) -> futures_util::future::BoxFuture<'_, bool>;
    /// Session.Lock(). Err: the call didn't go through.
    fn lock(&self) -> futures_util::future::BoxFuture<'_, std::result::Result<(), String>>;
}

/// Lock() again this often while the lock isn't up.
pub const LOCK_RESEND: Duration = Duration::from_secs(2);
/// The longest wait for the lock once remoted is stopping (SIGTERM, a logout).
pub const LOCK_CAP_STOPPING: Duration = Duration::from_secs(3);

/// The desk under the session lock before the desktop comes back (
/// steps 2 to 5): Lock() unless LockedHint is already true, then LockedHint polled
/// every 50 ms, with Lock() again every 2 s and no cap; once `stopping` is set
/// (SIGTERM, a logout: the shell may be stopping too) at most 3 s more. True when
/// the lock is up. Also run at start for a crashed run's Privacy or Off leftover.
pub async fn lock_desk_first(desk: &dyn DeskLock, stopping: &AtomicBool) -> bool {
    let t0 = Instant::now();
    if desk.locked().await {
        eprintln!("remoted: the desk is locked already; putting the desktop back");
        return true;
    }
    eprintln!("remoted: locking the desk before putting the desktop back");
    let mut next = t0;
    let mut first = true;
    let mut stop_seen: Option<Instant> = None;
    loop {
        let now = Instant::now();
        if now >= next {
            if !first {
                eprintln!("remoted: the lock isn't up after {} ms; asking again", t0.elapsed().as_millis());
            }
            first = false;
            if let Err(e) = desk.lock().await {
                eprintln!("remoted: Lock(): {e}");
            }
            next = now + LOCK_RESEND;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        if desk.locked().await {
            eprintln!("remoted: the lock is up after {} ms", t0.elapsed().as_millis());
            return true;
        }
        if stopping.load(Ordering::Relaxed) && stop_seen.get_or_insert(now).elapsed() >= LOCK_CAP_STOPPING {
            eprintln!("remoted: the lock isn't up after {} ms and remoted is stopping; putting the desktop back anyway", t0.elapsed().as_millis());
            return false;
        }
    }
}

/// Desktop notifications, with actions that come back as [`Cmd`]s.
pub trait Notifier: Send + Sync {
    fn notify(&self, n: Notice);
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub summary: String,
    pub body: String,
    /// (key, label): "disconnect", "settings", "pairing".
    pub actions: Vec<(&'static str, &'static str)>,
    /// Through Do not disturb (wrong pairing codes).
    pub critical: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingChange {
    Enabled(bool),
    Mdns(bool),
    ConnectWhileLocked(bool),
    HomeMode(HomeMode),
    Notify(bool),
}

type Reply<T> = oneshot::Sender<std::result::Result<T, String>>;

pub struct ConnectRequest {
    pub device: String,
    pub display: Request,
    pub takeover: bool,
    pub link: Arc<dyn ClientLink>,
    /// The path and RTT so far, for the Privacy and Off notifications ("Through
    /// Direct · 9 ms").
    pub route: Option<(String, u32)>,
}

pub enum Cmd {
    // From connections (the transport).
    Connect(ConnectRequest),
    Lost { device: String },
    Bye { device: String },
    Resize { device: String, display: Request },
    Preedit { device: String, text: String, cursor: i32 },
    Commit { device: String, text: String },
    Clipboard { device: String, entry: ClipEntry },
    MicOn { device: String, on: bool },
    #[allow(dead_code)] // sent by core's session or the media module once they land
    Mic { device: String, packet: Vec<u8> },
    ClientHomeMode { device: String, mode: HomeMode },
    #[allow(dead_code)] // sent by core's session or the media module once they land
    Stats { device: String, stats: LinkStats },
    /// A pairing attempt finished: the code matched (`ok`), and who the device is.
    PairAttempt { code: String, device: String, name: String, kind: String, reply: Option<oneshot::Sender<bool>> },
    /// remote-core's pairing window: a wrong code (SPAKE2 failed), a device paired
    /// (core stored it and uploaded the list), the window closed.
    PairFailed { attempts_left: u32 },
    Paired { device: String, name: String },
    PairClosed { reason: String },
    /// core's `PairingAttempt`: a device started a SPAKE2 run against the open window (`true`: Settings shows "A
    /// device is pairing…"), or that run is taken as over (`false`, from a guard timer: a device that leaves in
    /// the middle sends nothing more).
    PairingAttempt(bool),
    /// Something the published state reads changed elsewhere (the rendezvous).
    Refresh,
    // From the D-Bus API.
    Disconnect { at_desk: bool, reply: Reply<()> },
    SessionHomeMode { mode: HomeMode, reply: Reply<()> },
    Setting { change: SettingChange, reply: Reply<()> },
    Rename { id: String, name: String, reply: Reply<()> },
    Remove { id: String, reply: Reply<()> },
    SetName { name: String, reply: Reply<()> },
    OpenPairing { reply: Reply<(String, u64)> },
    CancelPairing,
    PairingDialog(bool),
    ClearRecent,
    // From the rest of the host.
    Ime(ImeEvent),
    HostClipboard(ClipEntry),
    /// The plugin's CARET, from the media module (`MediaEvent::Caret`).
    #[allow(dead_code)] // sent by the media module once it lands
    Caret { visible: bool, purpose: u32, x: i32, y: i32, w: i32, h: i32 },
    /// The plugin's POINTER_LOCK, from the media module (`MediaEvent::PointerLock`).
    #[allow(dead_code)] // sent by the media module once it lands
    PointerLock { state: u32, x: i32, y: i32, w: i32, h: i32 },
    Sleep(bool),
    #[allow(dead_code)] // sent by core's session or the media module once they land
    Media { capture: String, encoder_ready: bool },
    /// `--dev` only: what the relay row shows, in place of the rendezvous' own state.
    DevRelay(RelayView),
    /// SIGTERM: end the session (everything put back), then stop.
    Shutdown,
}

// ---- the published state ----

#[derive(Clone, Debug, PartialEq)]
pub struct SessionView {
    pub device: String,
    pub name: String,
    pub since: u64,
    pub stats: LinkStats,
    pub display: Option<Applied>,
    pub held_until: Option<u64>,
    pub locked: bool,
    pub home_mode: HomeMode,
    pub mic: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PairingView {
    pub code: String,
    pub expires_at: u64,
    pub attempts_left: u32,
    /// "open", "attempt" (a device is running SPAKE2 against the code), "attempts_used" (3 wrong tries:
    /// pairing closed until the person at the computer opens it again), "expired", "paired".
    pub state: &'static str,
    pub paired: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RelayView {
    /// The configured server's host name ("" when none is configured).
    pub host: String,
    /// "off" (no server configured), "unknown" (not wired yet), "online", "unreachable",
    /// "not_enrolled", or the server's refusal code: "token_invalid", "token_expired",
    /// "quota_exceeded", "payment_required".
    pub state: String,
    pub rtt_ms: u32,
    /// Unix seconds of the last answer.
    pub last_ok: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub settings: Settings,
    /// What devices call this computer: the user's name, or the computer's own while
    /// none is set (followed live).
    pub display_name: String,
    pub device_id: String,
    /// "off", "ready", "not_ready" (no encoder), "connected", "reconnecting" (held).
    pub state: &'static str,
    pub session: Option<SessionView>,
    pub devices: Vec<(PairedDevice, bool)>,
    pub pairing: Option<PairingView>,
    pub pairing_dialog: bool,
    pub recent: Vec<Recent>,
    pub relay: RelayView,
    /// "plugin", "fallback" or "unknown" (the media module says).
    pub capture: String,
    pub encoder_ready: bool,
}

struct Pairing {
    code: String,
    expires: Instant,
    expires_at: u64,
    attempts_left: u32,
    /// "open", "attempts_used", "expired", "paired".
    state: &'static str,
    /// A device is running SPAKE2 against it: cleared by the run's outcome (`PairFailed`, `Paired`), by the
    /// window closing, or by the guard.
    attempt: bool,
    paired: Option<String>,
}

/// Everything the actor reaches. Optional parts are absent in tests and when the
/// host lacks them (no Hyprland, no PipeWire, no add-on): the session still runs.
pub struct Parts {
    pub dir: PathBuf,
    pub key: DeviceKey,
    pub display: Option<DisplayManager>,
    pub plugin: Option<Arc<dyn PluginLink>>,
    pub ime: Option<ImeBridge>,
    pub audio: Option<Arc<Audio>>,
    pub clipboard: bool,
    pub rendezvous: Arc<dyn Rendezvous>,
    pub notifier: Option<Arc<dyn Notifier>>,
    /// logind's LockedHint and Lock() for this user's graphical session.
    pub desk: Arc<dyn DeskLock>,
}

pub struct Host {
    p: Parts,
    tx: mpsc::UnboundedSender<Cmd>,
    core: SessionCore,
    paired: PairedStore,
    settings: Settings,
    recent: RecentLog,
    status: watch::Sender<Status>,
    link: Option<Arc<dyn ClientLink>>,
    /// Where the sink's packets go: the current link, swapped on takeover.
    audio_target: Arc<Mutex<Option<Arc<dyn ClientLink>>>>,
    sink: Option<Sink>,
    mic: Option<Mic>,
    clipboard: Option<Clipboard>,
    since: u64,
    stats: LinkStats,
    locked: bool,
    session_mode: HomeMode,
    resize: Option<(Request, Instant)>,
    pairing: Option<Pairing>,
    pairing_dialog: bool,
    capture: String,
    encoder_ready: bool,
    /// The computer's own name (hostnamed's PrettyHostname, else the hostname) as last read.
    computer: String,
    /// The name core announces now.
    applied_name: String,
    relay_override: Option<RelayView>,
    /// SIGTERM or a logout: a wait for the lock is capped.
    stopping: Arc<AtomicBool>,
}

impl Host {
    #[allow(clippy::type_complexity)]
    pub fn new(p: Parts) -> Result<(Self, mpsc::UnboundedSender<Cmd>, mpsc::UnboundedReceiver<Cmd>, watch::Receiver<Status>)> {
        let paired = PairedStore::open(&p.dir)?;
        let settings = Settings::load(&p.dir);
        let mut recent = RecentLog::open(&p.dir);
        // An entry still open is from a run that crashed: it ended then.
        if recent.entries.iter().any(|e| e.end == 0) {
            recent.finish(unix_now())?;
        }
        let (tx, rx) = mpsc::unbounded_channel::<Cmd>();
        let computer = store::computer_name();
        let display_name = if settings.name.is_empty() { computer.clone() } else { settings.name.clone() };
        let placeholder = Status {
            settings: settings.clone(),
            display_name: display_name.clone(),
            device_id: p.key.id(),
            state: "ready",
            session: None,
            devices: vec![],
            pairing: None,
            pairing_dialog: false,
            recent: vec![],
            relay: RelayView::default(),
            capture: "unknown".into(),
            encoder_ready: true,
        };
        let (status, rx_status) = watch::channel(placeholder);
        let host = Host {
            p,
            tx: tx.clone(),
            core: SessionCore::from_env(),
            paired,
            settings,
            recent,
            status,
            link: None,
            audio_target: Arc::new(Mutex::new(None)),
            sink: None,
            mic: None,
            clipboard: None,
            since: 0,
            stats: LinkStats::default(),
            locked: false,
            session_mode: HomeMode::Mirror,
            resize: None,
            pairing: None,
            pairing_dialog: false,
            capture: "unknown".into(),
            encoder_ready: true,
            computer,
            applied_name: display_name,
            relay_override: None,
            stopping: Arc::new(AtomicBool::new(false)),
        };
        Ok((host, tx, rx, rx_status))
    }

    /// The plugin through the stream (input resets, the desk's input mode, the
    /// end of the stream); it needs the actor's sender, so it comes after `new`.
    pub fn set_plugin(&mut self, p: Arc<dyn PluginLink>) {
        self.p.plugin = Some(p);
    }

    /// Set by the signal handler with [`Cmd::Shutdown`], so a wait for the lock that
    /// is already running gets the stopping cap too.
    pub fn stopping(&self) -> Arc<AtomicBool> {
        self.stopping.clone()
    }

    /// Notifications need the actor's sender (their actions come back as commands).
    pub fn set_notifier(&mut self, n: Arc<dyn Notifier>) {
        self.p.notifier = Some(n);
    }

    /// Runs until every sender is gone. `rx` is the receiver `new` returned.
    pub async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        self.publish();
        loop {
            let deadline = [self.core.deadline(), self.resize.map(|(_, t)| t), self.pairing.as_ref().filter(|p| p.state == "open").map(|p| p.expires)]
                .into_iter()
                .flatten()
                .min();
            let cmd = match deadline {
                Some(t) => tokio::select! {
                    c = rx.recv() => c,
                    _ = tokio::time::sleep_until(t.into()) => { self.timers().await; continue; }
                },
                None => rx.recv().await,
            };
            let Some(cmd) = cmd else { break };
            if let Cmd::Shutdown = cmd {
                self.stopping.store(true, Ordering::Relaxed);
                break;
            }
            self.handle(cmd).await;
            self.publish();
        }
        // Shutting down: never leave the desktop moved.
        if self.core.client().is_some() {
            self.end(EndReason::DaemonStopping).await;
        }
    }

    async fn timers(&mut self) {
        let now = Instant::now();
        if let Some((req, t)) = self.resize
            && now >= t
        {
            self.resize = None;
            if let Some(dm) = self.p.display.as_mut() {
                match tokio::task::block_in_place(|| dm.resize(&req)) {
                    Ok(a) => {
                        if let Some(l) = &self.link {
                            l.send(HostEvent::Display(a));
                        }
                    }
                    Err(e) => eprintln!("remoted: resize: {e:#}"),
                }
            }
        }
        if let Some(c) = self.core.tick(now) {
            eprintln!("remoted: {}'s hold ran out; ending the session", c.name);
            self.teardown(&c, true).await;
        }
        if let Some(p) = self.pairing.as_mut()
            && p.state == "open"
            && now >= p.expires
        {
            p.state = "expired";
            self.p.rendezvous.pair_close();
        }
        self.publish();
    }

    fn name_of(&self, device: &str) -> String {
        self.paired.get(device).map_or_else(|| "A device".to_string(), |d| d.name.clone())
    }

    fn is_current(&self, device: &str) -> bool {
        matches!(self.core.phase(), Phase::Live { client } if client.device == device)
    }

    fn notify(&self, n: Notice) {
        if let Some(nt) = &self.p.notifier
            && (self.settings.notify || n.critical)
        {
            nt.notify(n);
        }
    }

    fn mode_words(&self) -> &'static str {
        // Mirror is the design's; Privacy and Off are
        match self.session_mode {
            HomeMode::Mirror => "The desk monitor mirrors it.",
            HomeMode::Privacy => "The desk monitor shows the privacy screen.",
            HomeMode::Off => "The desk monitor is off.",
        }
    }

    async fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Connect(req) => {
                // Core renames the entry at Hello: the name below is the reloaded one.
                self.reload_paired();
                self.connect(req).await
            }
            Cmd::Lost { device } => {
                if self.core.lost(&device, Instant::now()) {
                    eprintln!("remoted: lost {}; holding the session for 2 minutes", self.name_of(&device));
                    self.link = None;
                    *self.audio_target.lock().unwrap() = None;
                    self.release_input();
                    if let Some(ime) = &self.p.ime {
                        ime.deactivate().await;
                    }
                }
            }
            Cmd::Bye { device } => {
                if self.core.client().is_some_and(|c| c.device == device) {
                    self.end(EndReason::ClientLeft).await;
                }
            }
            Cmd::Resize { device, display } => {
                if self.is_current(&device) {
                    self.resize = Some((display, Instant::now() + RESIZE_DEBOUNCE));
                }
            }
            Cmd::Preedit { device, text, cursor } => {
                if self.is_current(&device)
                    && let Some(ime) = &self.p.ime
                {
                    ime.preedit(&text, cursor).await;
                }
            }
            Cmd::Commit { device, text } => {
                if self.is_current(&device)
                    && let Some(ime) = &self.p.ime
                {
                    ime.commit(&text).await;
                }
            }
            Cmd::Clipboard { device, entry } => {
                if self.is_current(&device)
                    && let Some(c) = &self.clipboard
                {
                    c.set(entry);
                }
            }
            Cmd::MicOn { device, on } => {
                if self.is_current(&device) {
                    self.set_mic(on);
                }
            }
            Cmd::Mic { device, packet } => {
                if self.is_current(&device)
                    && let Some(m) = &self.mic
                {
                    let _ = m.write(&packet);
                }
            }
            Cmd::ClientHomeMode { device, mode } => {
                if self.core.client().is_some_and(|c| c.device == device) {
                    let _ = self.set_session_mode(mode);
                }
            }
            Cmd::Stats { device, stats } => {
                if self.is_current(&device) {
                    if stats.path != self.stats.path || stats.transport != self.stats.transport {
                        self.recent.update_path(&stats.path, &stats.transport, stats.rtt_ms);
                        let _ = self.recent.save(unix_now());
                    }
                    self.stats = stats;
                }
            }
            Cmd::PairAttempt { code, device, name, kind, reply } => {
                let ok = self.pair_attempt(&code, device, name, kind);
                if let Some(r) = reply {
                    let _ = r.send(ok);
                }
            }
            Cmd::PairingAttempt(on) => {
                if let Some(p) = self.pairing.as_mut().filter(|p| p.state == "open") {
                    p.attempt = on;
                }
            }
            Cmd::PairFailed { attempts_left } => {
                if let Some(p) = self.pairing.as_mut().filter(|p| p.state == "open") {
                    p.attempt = false;
                    p.attempts_left = attempts_left;
                    if attempts_left == 0 {
                        self.close_after_wrong_codes();
                    }
                }
            }
            Cmd::Paired { device, name } => {
                self.reload_paired();
                self.paired_ok(&device, name);
            }
            Cmd::PairClosed { reason } => {
                // "cancelled" (a new window replaced it), "attempts_used" (handled by
                // PairFailed, which closed it) and "paired" (Paired) need nothing here but the end of a run.
                if let Some(p) = self.pairing.as_mut() {
                    p.attempt = false;
                }
                if reason == "expired"
                    && let Some(p) = self.pairing.as_mut().filter(|p| p.state == "open")
                {
                    p.state = "expired";
                }
            }
            Cmd::Refresh => {
                // The computer's own name may have changed (hostnamed): follow it unless a Remote name is set.
                self.computer = store::computer_name();
                self.apply_name();
            }
            Cmd::Disconnect { at_desk, reply } => {
                if self.core.client().is_none() {
                    let _ = reply.send(Err("nobody is connected".into()));
                    return;
                }
                let why = if at_desk { EndReason::EndedAtComputer } else { EndReason::DisconnectedByHost { host: self.applied_name.clone() } };
                self.end(why).await;
                let _ = reply.send(Ok(()));
            }
            Cmd::SessionHomeMode { mode, reply } => {
                let _ = reply.send(self.set_session_mode(mode).map_err(|e| format!("{e:#}")));
            }
            Cmd::Setting { change, reply } => {
                let r = self.setting(change).await;
                // Published before the reply, so the setter's change signal has the new value.
                self.publish();
                let _ = reply.send(r.map_err(|e| format!("{e:#}")));
            }
            Cmd::SetName { name, reply } => {
                // "" goes back to following the computer's own name (the reset).
                let name = name.trim().to_string();
                let r = if name.len() > 64 {
                    Err("a name is 1 to 64 bytes".to_string())
                } else {
                    self.settings.name = name;
                    self.computer = store::computer_name();
                    self.apply_name();
                    self.settings.save(&self.p.dir).map_err(|e| format!("{e:#}"))
                };
                self.publish();
                let _ = reply.send(r);
            }
            Cmd::Rename { id, name, reply } => {
                let r = if self.p.rendezvous.core() {
                    let name = name.trim();
                    if name.len() > 64 {
                        Err("a name is 0 to 64 bytes".to_string())
                    } else if self.p.rendezvous.rename(&id, name) {
                        self.reload_paired();
                        Ok(())
                    } else {
                        Err("no such device".to_string())
                    }
                } else {
                    match self.paired.rename(&id, &name) {
                    Ok(true) => Ok(()),
                    Ok(false) => Err("no such device".to_string()),
                    Err(e) => Err(format!("{e:#}")),
                    }
                };
                let _ = reply.send(r);
            }
            Cmd::Remove { id, reply } => {
                let r = self.remove(&id).await;
                let _ = reply.send(r.map_err(|e| format!("{e:#}")));
            }
            Cmd::OpenPairing { reply } => {
                let r = self.open_pairing();
                let _ = reply.send(r.map_err(|e| format!("{e:#}")));
            }
            Cmd::CancelPairing => {
                if self.pairing.take().is_some() {
                    self.p.rendezvous.pair_close();
                }
            }
            Cmd::PairingDialog(open) => self.pairing_dialog = open,
            Cmd::ClearRecent => {
                let _ = self.recent.clear(unix_now());
            }
            Cmd::Ime(ev) => {
                if let Some(l) = &self.link {
                    l.send(HostEvent::Ime(ev));
                }
            }
            Cmd::HostClipboard(entry) => {
                if let Some(l) = &self.link {
                    l.send(HostEvent::Clipboard(entry));
                }
            }
            Cmd::Caret { visible, purpose, x, y, w, h } => {
                if let Some(l) = &self.link {
                    for ev in crate::ime::from_plugin_caret(visible, purpose, x, y, w, h) {
                        l.send(HostEvent::Ime(ev));
                    }
                }
            }
            Cmd::PointerLock { state, x, y, w, h } => {
                if let Some(l) = &self.link {
                    l.send(HostEvent::PointerLock { state, x, y, w, h });
                }
            }
            Cmd::Sleep(asleep) => self.p.rendezvous.sleep_notice(asleep),
            Cmd::Media { capture, encoder_ready } => {
                self.capture = capture;
                self.encoder_ready = encoder_ready;
            }
            Cmd::DevRelay(r) => self.relay_override = Some(r),
            Cmd::Shutdown => {}
        }
    }

    async fn connect(&mut self, req: ConnectRequest) {
        let refuse = |why: &'static str| req.link.send(HostEvent::Refused { reason: why });
        if !self.settings.enabled {
            return refuse("off");
        }
        let Some(dev) = self.paired.get(&req.device).cloned() else { return refuse("not_paired") };
        if !self.encoder_ready {
            return refuse("not_ready");
        }
        let starting = self.core.client().is_none();
        let locked = if starting { self.p.desk.locked().await } else { self.locked };
        if starting && locked && !self.settings.connect_while_locked {
            return refuse("locked");
        }
        let client = Client { device: dev.id.clone(), name: dev.name.clone() };
        let decision = self.core.connect(client.clone(), req.takeover);
        let now = unix_now();
        match decision {
            Decision::AskFirst { in_use_by } => {
                req.link.send(HostEvent::InUse { by: in_use_by.name });
            }
            Decision::Start => {
                self.locked = locked;
                self.session_mode = self.settings.home_mode;
                self.since = now;
                self.stats = LinkStats::default();
                let display = match self.start_display(&req.display, &client.device) {
                    Ok(a) => a,
                    Err(e) => {
                        eprintln!("remoted: display: {e:#}");
                        self.core.end();
                        return refuse("display");
                    }
                };
                self.attach(req.link.clone());
                self.local_input();
                if let Some(ime) = &self.p.ime {
                    ime.activate().await;
                }
                self.start_audio();
                self.start_clipboard();
                self.touch(&client.device, now);
                let _ = self.recent.begin(Recent {
                    device: client.device.clone(),
                    name: client.name.clone(),
                    start: now,
                    end: 0,
                    path: String::new(),
                    transport: String::new(),
                    rtt_ms: 0,
                });
                req.link.send(HostEvent::Accepted { display, took_over_from: None, locked, home_mode: self.session_mode });
                let (summary, body) = if locked {
                    (format!("{} connected while locked", client.name), "The lock screen is showing there too. Unlock happens in the stream.".to_string())
                } else {
                    let through = match (&req.route, self.session_mode) {
                        (Some((path, rtt)), HomeMode::Privacy | HomeMode::Off) => format!("Through {path} · {rtt} ms. "),
                        _ => String::new(),
                    };
                    (format!("{} connected", client.name), format!("{through}{}", self.mode_words()))
                };
                self.notify(Notice { summary, body, actions: vec![("disconnect", "Disconnect")], critical: false });
            }
            Decision::Resume { was_held } => {
                let display = self.resume_display(&req.display);
                self.attach(req.link.clone());
                if was_held && let Some(ime) = &self.p.ime {
                    ime.activate().await;
                }
                self.touch(&client.device, now);
                req.link.send(HostEvent::Accepted { display, took_over_from: None, locked: self.locked, home_mode: self.session_mode });
            }
            Decision::TakeOver { from, from_was_live } => {
                if let Some(old) = self.link.take() {
                    old.send(HostEvent::Ended(EndReason::TakenOver { by: client.name.clone() }));
                }
                self.release_input();
                let display = self.resume_display(&req.display);
                self.attach(req.link.clone());
                if !from_was_live && let Some(ime) = &self.p.ime {
                    ime.activate().await;
                }
                self.set_mic(false);
                let _ = self.recent.finish(now);
                let _ = self.recent.begin(Recent {
                    device: client.device.clone(),
                    name: client.name.clone(),
                    start: now,
                    end: 0,
                    path: String::new(),
                    transport: String::new(),
                    rtt_ms: 0,
                });
                self.since = now;
                self.stats = LinkStats::default();
                self.touch(&client.device, now);
                self.touch(&from.device, now);
                req.link.send(HostEvent::Accepted {
                    display,
                    took_over_from: (!from_was_live).then(|| from.name.clone()),
                    locked: self.locked,
                    home_mode: self.session_mode,
                });
                // One notification, not two (SB4).
                self.notify(Notice {
                    summary: format!("{} took over", client.name),
                    body: format!("{} was disconnected. One device streams at a time.", from.name),
                    actions: vec![("disconnect", "Disconnect")],
                    critical: false,
                });
            }
        }
    }

    fn attach(&mut self, link: Arc<dyn ClientLink>) {
        *self.audio_target.lock().unwrap() = Some(link.clone());
        self.link = Some(link);
    }

    fn start_display(&mut self, req: &Request, device: &str) -> Result<Applied> {
        let mode = self.session_mode;
        match self.p.display.as_mut() {
            Some(dm) => tokio::task::block_in_place(|| dm.connect(req, mode, device)),
            // No compositor (tests): what REMOTE-1 would have been.
            None => Ok(Applied { width: req.width, height: req.height, refresh_mhz: req.refresh_mhz, scale: 1.0 }),
        }
    }

    fn resume_display(&mut self, req: &Request) -> Applied {
        match self.p.display.as_mut() {
            Some(dm) => tokio::task::block_in_place(|| dm.resize(req)).unwrap_or_else(|e| {
                eprintln!("remoted: display: {e:#}");
                dm.held().map(|h| h.applied).unwrap_or(Applied { width: req.width, height: req.height, refresh_mhz: req.refresh_mhz, scale: 1.0 })
            }),
            None => Applied { width: req.width, height: req.height, refresh_mhz: req.refresh_mhz, scale: 1.0 },
        }
    }

    fn start_audio(&mut self) {
        let Some(audio) = self.p.audio.clone() else { return };
        let target = self.audio_target.clone();
        match tokio::task::block_in_place(|| {
            audio.start_sink(move |pkt| {
                if let Some(l) = target.lock().unwrap().as_ref() {
                    l.send(HostEvent::Audio(pkt));
                }
            })
        }) {
            Ok(sink) => {
                let _ = audio::save_state(std::slice::from_ref(&sink.saved));
                self.sink = Some(sink);
            }
            Err(e) => eprintln!("remoted: audio: {e:#}"),
        }
    }

    fn set_mic(&mut self, on: bool) {
        let Some(audio) = self.p.audio.clone() else { return };
        if on && self.mic.is_none() {
            match tokio::task::block_in_place(|| audio.start_mic()) {
                Ok(m) => {
                    let mut saved: Vec<_> = self.sink.iter().map(|s| s.saved.clone()).collect();
                    saved.push(m.saved.clone());
                    let _ = audio::save_state(&saved);
                    self.mic = Some(m);
                }
                Err(e) => eprintln!("remoted: mic: {e:#}"),
            }
        } else if !on && let Some(m) = self.mic.take() {
            match tokio::task::block_in_place(|| m.stop()) {
                Ok(r) => eprintln!("remoted: mic off, default input back ({r:?})"),
                Err(e) => eprintln!("remoted: mic: {e:#}"),
            }
        }
    }

    fn start_clipboard(&mut self) {
        if !self.p.clipboard {
            return;
        }
        let tx = self.tx.clone();
        match Clipboard::start(move |e| {
            let _ = tx.send(Cmd::HostClipboard(e));
        }) {
            Ok(c) => self.clipboard = Some(c),
            Err(e) => eprintln!("remoted: clipboard: {e:#}"),
        }
    }

    fn release_input(&self) {
        if let Some(pl) = &self.p.plugin {
            let _ = crate::input::reset(pl.as_ref());
        }
    }

    fn set_session_mode(&mut self, mode: HomeMode) -> Result<()> {
        if self.core.client().is_none() {
            bail!("nobody is connected");
        }
        if let Some(dm) = self.p.display.as_mut() {
            tokio::task::block_in_place(|| dm.set_mode(mode))?;
        }
        self.session_mode = mode;
        self.local_input();
        if let Some(l) = &self.link {
            l.send(HostEvent::HomeMode(mode));
        }
        Ok(())
    }

    /// The desk's own keyboard and mouse: ignored in Privacy and Off (Off is the
    /// privacy screen with the panel off), with desk keys going to the
    /// privacy screen (capture-ipc v4 LOCAL_INPUT 2); normal in Mirror.
    fn local_input(&self) {
        if let Some(pl) = &self.p.plugin {
            pl.local_input(if self.private() { 2 } else { 0 });
        }
    }

    /// The desk shows nothing of the user's (Privacy, Off): its end goes into the lock.
    fn private(&self) -> bool {
        matches!(self.session_mode, HomeMode::Privacy | HomeMode::Off)
    }

    /// When a device last connected; core keeps it when it owns `paired.json`.
    fn touch(&mut self, device: &str, now: u64) {
        if !self.p.rendezvous.core() {
            let _ = self.paired.touch(device, now);
        }
    }

    fn reload_paired(&mut self) {
        match PairedStore::open(&self.p.dir) {
            Ok(p) => self.paired = p,
            Err(e) => eprintln!("remoted: paired.json: {e:#}"),
        }
    }

    /// Ends the session now, telling the client why. Only the desk password
    /// (`EndedAtComputer`) gives the desk its desktop unlocked.
    async fn end(&mut self, why: EndReason) {
        let lock = why != EndReason::EndedAtComputer;
        if let Some(l) = self.link.take() {
            l.send(HostEvent::Ended(why));
        }
        if let Some(c) = self.core.end() {
            self.teardown(&c, lock).await;
        }
    }

    /// Everything back as it was; then "disconnected". `lock`: a Privacy or Off
    /// session puts the desk under the session lock first. The
    /// published State and Session stay as they were until the desktop is back.
    async fn teardown(&mut self, c: &Client, lock: bool) {
        // 1. The client has heard Ended; clipboard, IME and the mic go (nothing visible yet).
        self.link = None;
        *self.audio_target.lock().unwrap() = None;
        self.resize = None;
        self.clipboard = None;
        self.release_input();
        if let Some(ime) = &self.p.ime {
            ime.deactivate().await;
        }
        self.set_mic(false);
        // 2 to 5. The lock is up before anything of the desktop comes back to the desk.
        if lock && self.private() {
            lock_desk_first(self.p.desk.as_ref(), &self.stopping).await;
        }
        // 6. The desk's input, the stream, the sink, the display.
        if let Some(pl) = &self.p.plugin {
            pl.local_input(0);
            // The stream stops before REMOTE-1 goes.
            pl.end();
        }
        if let Some(sink) = self.sink.take() {
            match tokio::task::block_in_place(|| sink.stop()) {
                Ok(r) => eprintln!("remoted: Remote sink gone, default output back ({r:?})"),
                Err(e) => eprintln!("remoted: audio: {e:#}"),
            }
        }
        audio::clear_state();
        if let Some(dm) = self.p.display.as_mut()
            && let Err(e) = tokio::task::block_in_place(|| dm.disconnect())
        {
            eprintln!("remoted: display: {e:#}");
        }
        let now = unix_now();
        self.touch(&c.device, now);
        let _ = self.recent.finish(now);
        let mins = now.saturating_sub(self.since) / 60;
        let after = if mins >= 60 { format!("{} h {} min", mins / 60, mins % 60) } else { format!("{} min", mins.max(1)) };
        self.notify(Notice {
            summary: format!("{} disconnected", c.name),
            body: format!("After {after}. Windows are back where they were."),
            actions: vec![],
            critical: false,
        });
    }

    async fn setting(&mut self, change: SettingChange) -> Result<()> {
        match change {
            SettingChange::Enabled(on) => {
                self.settings.enabled = on;
                if !on && self.core.client().is_some() {
                    self.end(EndReason::RemoteOff).await;
                }
            }
            SettingChange::Mdns(v) => {
                self.settings.mdns = v;
                self.p.rendezvous.set_mdns(v);
            }
            SettingChange::ConnectWhileLocked(v) => self.settings.connect_while_locked = v,
            SettingChange::HomeMode(m) => self.settings.home_mode = m,
            SettingChange::Notify(v) => self.settings.notify = v,
        }
        self.settings.save(&self.p.dir)
    }

    /// Tell the rendezvous side the name devices see, when it changed.
    fn apply_name(&mut self) {
        let name = if self.settings.name.is_empty() { self.computer.clone() } else { self.settings.name.clone() };
        if name != self.applied_name {
            self.applied_name = name.clone();
            self.p.rendezvous.set_name(&name);
        }
    }

    fn upload_list(&self) {
        if self.p.rendezvous.core() {
            return;
        }
        let (payload, sig) = self.p.key.sign_paired_list(self.paired.list_seq(), &self.paired.client_ids());
        self.p.rendezvous.paired_list(payload, sig);
    }

    async fn remove(&mut self, id: &str) -> Result<()> {
        if self.p.rendezvous.core() {
            if self.paired.get(id).is_none() || !self.p.rendezvous.remove(id).await {
                bail!("no such device");
            }
            self.reload_paired();
        } else if self.paired.remove(id)?.is_none() {
            bail!("no such device");
        }
        self.upload_list();
        // Its access ends at once: a connected device is dropped.
        if self.core.client().is_some_and(|c| c.device == id) {
            self.end(EndReason::Removed).await;
        }
        Ok(())
    }

    fn new_code() -> Result<String> {
        let mut b = [0u8; 4];
        loop {
            rustix::rand::getrandom(&mut b, rustix::rand::GetRandomFlags::empty())?;
            let v = u32::from_le_bytes(b);
            // Unbiased: reject the top sliver.
            if v < u32::MAX - u32::MAX % 1_000_000 {
                return Ok(format!("{:06}", v % 1_000_000));
            }
        }
    }

    fn open_pairing(&mut self) -> Result<(String, u64)> {
        let code = Self::new_code()?;
        let window = pairing_window();
        let expires_at = unix_now() + window.as_secs();
        self.p.rendezvous.pair_open(&self.applied_name, &code);
        self.pairing = Some(Pairing {
            code: code.clone(),
            expires: Instant::now() + window,
            expires_at,
            attempts_left: PAIRING_TRIES,
            state: "open",
            attempt: false,
            paired: None,
        });
        Ok((code, expires_at))
    }

    /// One SPAKE2 run's outcome against the open window (core's pairing reports it).
    fn pair_attempt(&mut self, code: &str, device: String, name: String, kind: String) -> bool {
        let Some(p) = self.pairing.as_mut().filter(|p| p.state == "open") else { return false };
        p.attempt = false;
        if p.code != code {
            p.attempts_left -= 1;
            if p.attempts_left == 0 {
                self.close_after_wrong_codes();
            }
            return false;
        }
        let now = unix_now();
        let dev = PairedDevice { id: device.clone(), name: name.clone(), kind, paired_at: now, last_connected: None, named_at_desk: false };
        if let Err(e) = self.paired.add(dev) {
            eprintln!("remoted: pairing: {e:#}");
            return false;
        }
        self.upload_list();
        self.paired_ok(&device, name);
        true
    }

    /// Three wrong codes close pairing: no new code opens by itself, only when the person at the computer
    /// opens pairing again, so the three tries are all a stranger ever gets. The dialog says so, and a
    /// critical notification does when the dialog isn't showing.
    fn close_after_wrong_codes(&mut self) {
        self.p.rendezvous.pair_close();
        if let Some(p) = self.pairing.as_mut() {
            p.state = "attempts_used";
            p.attempt = false;
        }
        if !self.pairing_dialog {
            self.notify(Notice {
                summary: "Pairing closed".into(),
                body: "Someone typed a wrong code 3 times, so pairing closed. To pair a device, open pairing again.".into(),
                actions: vec![("pairing", "Pair a device")],
                critical: true,
            });
        }
    }

    fn paired_ok(&mut self, _device: &str, name: String) {
        // Core closes its own window once its last message is out.
        if !self.p.rendezvous.core() {
            self.p.rendezvous.pair_close();
        }
        if let Some(p) = self.pairing.as_mut() {
            p.state = "paired";
            p.attempt = false;
            p.paired = Some(name.clone());
        }
        if !self.pairing_dialog {
            self.notify(Notice {
                summary: format!("{name} is paired"),
                body: "It can connect from anywhere now.".into(),
                actions: vec![("settings", "Remote settings")],
                critical: false,
            });
        }
    }

    fn publish(&mut self) {
        let client = self.core.client().cloned();
        let held_until = self.core.deadline().map(|t| unix_now() + t.saturating_duration_since(Instant::now()).as_secs());
        let session = client.as_ref().map(|c| SessionView {
            device: c.device.clone(),
            name: c.name.clone(),
            since: self.since,
            stats: self.stats.clone(),
            display: self.p.display.as_ref().and_then(|d| d.held().map(|h| h.applied)),
            held_until,
            locked: self.locked,
            home_mode: self.session_mode,
            mic: self.mic.is_some(),
        });
        let state = if !self.settings.enabled {
            "off"
        } else if held_until.is_some() {
            "reconnecting"
        } else if client.is_some() {
            "connected"
        } else if !self.encoder_ready {
            "not_ready"
        } else {
            "ready"
        };
        let pairing = self.pairing.as_ref().map(|p| PairingView {
            code: p.code.clone(),
            expires_at: p.expires_at,
            attempts_left: p.attempts_left,
            state: if p.state == "open" && p.attempt {
                "attempt"
            } else {
                p.state
            },
            paired: p.paired.clone(),
        });
        let devices = self
            .paired
            .devices()
            .iter()
            .map(|d| (d.clone(), client.as_ref().is_some_and(|c| c.device == d.id) && held_until.is_none()))
            .collect();
        let st = Status {
            settings: self.settings.clone(),
            display_name: if self.settings.name.is_empty() { self.computer.clone() } else { self.settings.name.clone() },
            device_id: self.p.key.id(),
            state,
            session,
            devices,
            pairing,
            pairing_dialog: self.pairing_dialog,
            recent: self.recent.entries.clone(),
            relay: self.relay_override.clone().unwrap_or_else(|| self.p.rendezvous.state()),
            capture: self.capture.clone(),
            encoder_ready: self.encoder_ready,
        };
        self.status.send_if_modified(|old| {
            if *old != st {
                *old = st;
                true
            } else {
                false
            }
        });
    }
}

/// A rendezvous that isn't there yet: logs, and says "unknown".
pub struct NoRendezvous;

impl Rendezvous for NoRendezvous {
    fn paired_list(&self, payload: Vec<u8>, _sig: [u8; 64]) {
        eprintln!("remoted: rendezvous (not wired yet): paired list, {} bytes", payload.len());
    }
    fn pair_open(&self, _name: &str, _code: &str) {
        eprintln!("remoted: rendezvous (not wired yet): pairing window opened");
    }
    fn pair_close(&self) {}
    fn sleep_notice(&self, asleep: bool) {
        eprintln!("remoted: rendezvous (not wired yet): {}", if asleep { "going to sleep" } else { "awake" });
    }
    fn state(&self) -> RelayView {
        RelayView { host: String::new(), state: "unknown".into(), rtt_ms: 0, last_ok: 0 }
    }
}

/// Collects events per device for tests and the dev interface.
#[derive(Default)]
pub struct RecordingLink {
    pub events: Mutex<Vec<HostEvent>>,
    pub audio_packets: std::sync::atomic::AtomicU64,
}

impl ClientLink for RecordingLink {
    fn send(&self, ev: HostEvent) {
        if let HostEvent::Audio(_) = ev {
            self.audio_packets.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.events.lock().unwrap().push(ev);
        }
    }
}

/// Every recording link by device, so a test can read what each client heard.
pub type Links = Arc<Mutex<HashMap<String, Arc<RecordingLink>>>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::UiSize;

    struct Notes(Mutex<Vec<Notice>>);
    impl Notifier for Notes {
        fn notify(&self, n: Notice) {
            self.0.lock().unwrap().push(n);
        }
    }

    /// A desk whose LockedHint never comes on, and that nobody asks to lock in these tests.
    struct NoLock;
    impl DeskLock for NoLock {
        fn locked(&self) -> futures_util::future::BoxFuture<'_, bool> {
            Box::pin(async { false })
        }
        fn lock(&self) -> futures_util::future::BoxFuture<'_, std::result::Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
    }

    /// The shell's lock as logind sees it: the lock is up after `answer_after` Lock() calls (0: never).
    #[derive(Default)]
    struct ShellLock {
        locked: AtomicBool,
        calls: std::sync::atomic::AtomicU32,
        answer_after: std::sync::atomic::AtomicU32,
    }
    impl DeskLock for ShellLock {
        fn locked(&self) -> futures_util::future::BoxFuture<'_, bool> {
            Box::pin(async { self.locked.load(Ordering::SeqCst) })
        }
        fn lock(&self) -> futures_util::future::BoxFuture<'_, std::result::Result<(), String>> {
            Box::pin(async {
                let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                if n == self.answer_after.load(Ordering::SeqCst) {
                    self.locked.store(true, Ordering::SeqCst);
                }
                Ok(())
            })
        }
    }

    /// Notices, each with whether the desk was locked when it came ("disconnected" is the last step).
    struct LockNotes(Arc<ShellLock>, Mutex<Vec<(String, bool)>>);
    impl Notifier for LockNotes {
        fn notify(&self, n: Notice) {
            self.1.lock().unwrap().push((n.summary, self.0.locked.load(Ordering::SeqCst)));
        }
    }

    /// A Privacy session that the device leaves ends into the lock (Lock() sent again
    /// after 2 s when the first isn't answered), the desk password's end doesn't lock, nor does a
    /// Mirror session's; when remoted stops, an unanswered lock is waited for 3 s, no longer.
    #[tokio::test(flavor = "multi_thread")]
    async fn privacy_and_off_end_into_the_lock() {
        let dir = std::env::temp_dir().join(format!("remoted-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let desk = Arc::new(ShellLock::default());
        let notes = Arc::new(LockNotes(desk.clone(), Mutex::new(vec![])));
        let parts = Parts {
            dir: dir.clone(),
            key: DeviceKey::load_or_create(&dir).unwrap(),
            display: None,
            plugin: None,
            ime: None,
            audio: None,
            clipboard: false,
            rendezvous: Arc::new(NoRendezvous),
            notifier: Some(notes.clone()),
            desk: desk.clone(),
        };
        let (host, tx, rx, mut status) = Host::new(parts).unwrap();
        let actor = tokio::spawn(host.run(rx));
        let (r, w) = oneshot::channel();
        tx.send(Cmd::OpenPairing { reply: r }).unwrap();
        let (code, _) = w.await.unwrap().unwrap();
        let air = crate::keys::device_id(&[1; 32]);
        tx.send(Cmd::PairAttempt { code, device: air.clone(), name: "MacBook Air".into(), kind: "mac".into(), reply: None }).unwrap();
        let session = |mode: HomeMode| {
            let link: Arc<dyn ClientLink> = Arc::new(RecordingLink::default());
            tx.send(Cmd::Connect(ConnectRequest { device: air.clone(), display: req(), takeover: false, link, route: None })).unwrap();
            tx.send(Cmd::ClientHomeMode { device: air.clone(), mode }).unwrap();
        };
        let reset = |answer_after: u32| {
            desk.locked.store(false, Ordering::SeqCst);
            desk.calls.store(0, Ordering::SeqCst);
            desk.answer_after.store(answer_after, Ordering::SeqCst);
            notes.1.lock().unwrap().clear();
        };
        let disconnected = || notes.1.lock().unwrap().iter().find(|(s, _)| s.ends_with("disconnected")).map(|(_, locked)| *locked);

        // Privacy, the device leaves: Lock(), unanswered; again after 2 s; the desktop comes back once locked.
        reset(2);
        session(HomeMode::Privacy);
        status.wait_for(|s| s.session.as_ref().is_some_and(|v| v.home_mode == HomeMode::Privacy)).await.unwrap();
        let t = Instant::now();
        tx.send(Cmd::Bye { device: air.clone() }).unwrap();
        status.wait_for(|s| s.state == "ready").await.unwrap();
        assert!(t.elapsed() >= LOCK_RESEND, "the second Lock() comes 2 s after the first");
        assert_eq!(desk.calls.load(Ordering::SeqCst), 2);
        assert_eq!(disconnected(), Some(true), "the desktop came back only under the lock");

        // Off ends the same way; here the first Lock() is answered.
        reset(1);
        session(HomeMode::Off);
        status.wait_for(|s| s.session.as_ref().is_some_and(|v| v.home_mode == HomeMode::Off)).await.unwrap();
        tx.send(Cmd::Bye { device: air.clone() }).unwrap();
        status.wait_for(|s| s.state == "ready").await.unwrap();
        assert_eq!((desk.calls.load(Ordering::SeqCst), disconnected()), (1, Some(true)));

        // The desk password (EndAtDesk) and a Mirror session: no lock.
        for (mode, at_desk) in [(HomeMode::Privacy, true), (HomeMode::Mirror, false)] {
            reset(1);
            session(mode);
            status.wait_for(|s| s.session.as_ref().is_some_and(|v| v.home_mode == mode)).await.unwrap();
            let (r, w) = oneshot::channel();
            tx.send(Cmd::Disconnect { at_desk, reply: r }).unwrap();
            w.await.unwrap().unwrap();
            assert_eq!((desk.calls.load(Ordering::SeqCst), disconnected()), (0, Some(false)), "{mode:?}");
        }

        // Stopping (SIGTERM) with a Privacy session and a shell that never locks: 3 s, then the desktop back.
        reset(0);
        session(HomeMode::Privacy);
        status.wait_for(|s| s.session.as_ref().is_some_and(|v| v.home_mode == HomeMode::Privacy)).await.unwrap();
        let t = Instant::now();
        tx.send(Cmd::Shutdown).unwrap();
        tokio::time::timeout(Duration::from_secs(6), actor).await.unwrap().unwrap();
        assert!(t.elapsed() >= LOCK_CAP_STOPPING && desk.calls.load(Ordering::SeqCst) == 2);
        assert_eq!(disconnected(), Some(false));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn req() -> Request {
        Request { width: 2560, height: 1664, refresh_mhz: 60_000, ui: UiSize::Match, ui_ratio: 1.74, display_width: 0, display_height: 0 }
    }

    /// Records what the session tells the rendezvous side to apply live.
    #[derive(Default)]
    struct LiveCalls(Mutex<Vec<String>>);
    impl Rendezvous for LiveCalls {
        fn paired_list(&self, _: Vec<u8>, _: [u8; 64]) {}
        fn pair_open(&self, _: &str, _: &str) {
            self.0.lock().unwrap().push("pair_open".into());
        }
        fn pair_close(&self) {}
        fn sleep_notice(&self, _: bool) {}
        fn state(&self) -> RelayView {
            RelayView::default()
        }
        fn set_name(&self, name: &str) {
            self.0.lock().unwrap().push(format!("name {name}"));
        }
        fn set_mdns(&self, on: bool) {
            self.0.lock().unwrap().push(format!("mdns {on}"));
        }
    }

    /// Name and "Visible on this network" reach core at once; "" goes back to the computer's own name.
    #[tokio::test(flavor = "multi_thread")]
    async fn name_and_mdns_apply_live() {
        let dir = std::env::temp_dir().join(format!("remoted-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let calls = Arc::new(LiveCalls::default());
        let parts = Parts {
            dir: dir.clone(),
            key: DeviceKey::load_or_create(&dir).unwrap(),
            display: None,
            plugin: None,
            ime: None,
            audio: None,
            clipboard: false,
            rendezvous: calls.clone(),
            notifier: None,
            desk: Arc::new(NoLock),
        };
        let (host, tx, rx, mut status) = Host::new(parts).unwrap();
        tokio::spawn(host.run(rx));
        let own = status.borrow().display_name.clone();

        let (r, w) = oneshot::channel();
        tx.send(Cmd::SetName { name: " Desk ".into(), reply: r }).unwrap();
        w.await.unwrap().unwrap();
        status.wait_for(|s| s.display_name == "Desk").await.unwrap();
        let (r, w) = oneshot::channel();
        tx.send(Cmd::Setting { change: SettingChange::Mdns(false), reply: r }).unwrap();
        w.await.unwrap().unwrap();
        let (r, w) = oneshot::channel();
        tx.send(Cmd::SetName { name: String::new(), reply: r }).unwrap();
        w.await.unwrap().unwrap();
        status.wait_for(|s| s.display_name == own).await.unwrap();
        assert_eq!(*calls.0.lock().unwrap(), [String::from("name Desk"), "mdns false".into(), format!("name {own}")]);
        // 65 bytes is too long.
        let (r, w) = oneshot::channel();
        tx.send(Cmd::SetName { name: "x".repeat(65), reply: r }).unwrap();
        assert!(w.await.unwrap().is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Settings' "A device is pairing…": core's PairingAttempt shows as state `attempt`, and the run's outcome (a
    /// failed try, a pairing) or the window closing clears it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pairing_attempt_shows_until_its_outcome() {
        let dir = std::env::temp_dir().join(format!("remoted-attempt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let parts = Parts {
            dir: dir.clone(),
            key: DeviceKey::load_or_create(&dir).unwrap(),
            display: None,
            plugin: None,
            ime: None,
            audio: None,
            clipboard: false,
            rendezvous: Arc::new(NoRendezvous),
            notifier: None,
            desk: Arc::new(NoLock),
        };
        let (host, tx, rx, mut status) = Host::new(parts).unwrap();
        tokio::spawn(host.run(rx));
        let (r, w) = oneshot::channel();
        tx.send(Cmd::OpenPairing { reply: r }).unwrap();
        w.await.unwrap().unwrap();
        let state = |s: &Status| s.pairing.as_ref().map(|p| (p.state, p.attempts_left));

        tx.send(Cmd::PairingAttempt(true)).unwrap();
        status.wait_for(|s| state(s) == Some(("attempt", 3))).await.unwrap();
        // a wrong try: the line goes, a try fewer (Settings: the wrong-try state)
        tx.send(Cmd::PairFailed { attempts_left: 2 }).unwrap();
        status.wait_for(|s| state(s) == Some(("open", 2))).await.unwrap();
        // the guard ends a run whose device left
        tx.send(Cmd::PairingAttempt(true)).unwrap();
        status.wait_for(|s| state(s) == Some(("attempt", 2))).await.unwrap();
        tx.send(Cmd::PairingAttempt(false)).unwrap();
        status.wait_for(|s| state(s) == Some(("open", 2))).await.unwrap();
        // the right code: paired
        tx.send(Cmd::PairingAttempt(true)).unwrap();
        status.wait_for(|s| state(s) == Some(("attempt", 2))).await.unwrap();
        tx.send(Cmd::Paired { device: crate::keys::device_id(&[1; 32]), name: "MacBook Air".into() }).unwrap();
        status.wait_for(|s| state(s) == Some(("paired", 2))).await.unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Three wrong codes close pairing, and nothing opens it again but the person at the computer: no new
    /// code, no new window.
    #[tokio::test(flavor = "multi_thread")]
    async fn three_wrong_codes_close_pairing() {
        let dir = std::env::temp_dir().join(format!("remoted-wrong-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let calls = Arc::new(LiveCalls::default());
        let parts = Parts {
            dir: dir.clone(),
            key: DeviceKey::load_or_create(&dir).unwrap(),
            display: None,
            plugin: None,
            ime: None,
            audio: None,
            clipboard: false,
            rendezvous: calls.clone(),
            notifier: None,
            desk: Arc::new(NoLock),
        };
        let (host, tx, rx, mut status) = Host::new(parts).unwrap();
        tokio::spawn(host.run(rx));
        let (r, w) = oneshot::channel();
        tx.send(Cmd::OpenPairing { reply: r }).unwrap();
        let (code, _) = w.await.unwrap().unwrap();
        for left in [2, 1, 0] {
            tx.send(Cmd::PairFailed { attempts_left: left }).unwrap();
        }
        let s = status.wait_for(|s| s.pairing.as_ref().is_some_and(|p| p.state == "attempts_used")).await.unwrap().clone();
        assert_eq!(s.pairing.unwrap().code, code, "no new code");
        // A fourth try reaches nothing.
        let (r, w) = oneshot::channel();
        tx.send(Cmd::PairAttempt { code: code.clone(), device: crate::keys::device_id(&[1; 32]), name: "Air".into(), kind: "mac".into(), reply: Some(r) }).unwrap();
        assert!(!w.await.unwrap());
        assert_eq!(calls.0.lock().unwrap().iter().filter(|c| c.starts_with("pair_open")).count(), 1, "one window, the one opened at the computer");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn connect_takeover_and_pairing_through_the_actor() {
        let dir = std::env::temp_dir().join(format!("remoted-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let notes = Arc::new(Notes(Mutex::new(vec![])));
        let parts = Parts {
            dir: dir.clone(),
            key: DeviceKey::load_or_create(&dir).unwrap(),
            display: None,
            plugin: None,
            ime: None,
            audio: None,
            clipboard: false,
            rendezvous: Arc::new(NoRendezvous),
            notifier: Some(notes.clone()),
            desk: Arc::new(NoLock),
        };
        let (host, tx, rx, mut status) = Host::new(parts).unwrap();
        tokio::spawn(host.run(rx));

        // Pair the Air: two wrong codes, then the right one.
        let (r, w) = oneshot::channel();
        tx.send(Cmd::OpenPairing { reply: r }).unwrap();
        let (code, _) = w.await.unwrap().unwrap();
        let air = crate::keys::device_id(&[1; 32]);
        let mini = crate::keys::device_id(&[2; 32]);
        for (c, dev, name) in [("000000x", &air, "MacBook Air"), (code.as_str(), &air, "MacBook Air")] {
            let (r, w) = oneshot::channel();
            tx.send(Cmd::PairAttempt { code: c.into(), device: dev.clone(), name: name.into(), kind: "mac".into(), reply: Some(r) }).unwrap();
            assert_eq!(w.await.unwrap(), c == code);
        }
        // The mini pairs too (a new window).
        let (r, w) = oneshot::channel();
        tx.send(Cmd::OpenPairing { reply: r }).unwrap();
        let (code2, _) = w.await.unwrap().unwrap();
        let (r, w) = oneshot::channel();
        tx.send(Cmd::PairAttempt { code: code2, device: mini.clone(), name: "Mac mini".into(), kind: "mac".into(), reply: Some(r) }).unwrap();
        assert!(w.await.unwrap());

        let link = |_: &str| Arc::new(RecordingLink::default());
        let (l_air, l_mini) = (link("air"), link("mini"));
        let connect = |dev: &String, l: &Arc<RecordingLink>, takeover| {
            tx.send(Cmd::Connect(ConnectRequest { device: dev.clone(), display: req(), takeover, link: l.clone(), route: None })).unwrap();
        };
        connect(&air, &l_air, false);
        status.wait_for(|s| s.state == "connected").await.unwrap();
        // The mini while the Air is live: asked first; then it takes over.
        connect(&mini, &l_mini, false);
        connect(&mini, &l_mini, true);
        status.wait_for(|s| s.session.as_ref().is_some_and(|v| v.name == "Mac mini")).await.unwrap();
        let ev_mini = l_mini.events.lock().unwrap().clone();
        assert_eq!(ev_mini[0], HostEvent::InUse { by: "MacBook Air".into() });
        assert!(matches!(&ev_mini[1], HostEvent::Accepted { took_over_from: None, .. }));
        assert!(matches!(l_air.events.lock().unwrap().last(), Some(HostEvent::Ended(EndReason::TakenOver { by })) if by == "Mac mini"));
        // The mini's connection drops: held; the Air takes over silently with a toast.
        tx.send(Cmd::Lost { device: mini.clone() }).unwrap();
        status.wait_for(|s| s.state == "reconnecting").await.unwrap();
        let l_air2 = link("air2");
        connect(&air, &l_air2, false);
        status.wait_for(|s| s.state == "connected").await.unwrap();
        assert!(matches!(&l_air2.events.lock().unwrap()[0], HostEvent::Accepted { took_over_from: Some(f), .. } if f == "Mac mini"));
        // Removing the connected device drops it at once.
        let (r, w) = oneshot::channel();
        tx.send(Cmd::Remove { id: air.clone(), reply: r }).unwrap();
        w.await.unwrap().unwrap();
        status.wait_for(|s| s.state == "ready" && s.devices.len() == 1).await.unwrap();
        assert!(matches!(l_air2.events.lock().unwrap().last(), Some(HostEvent::Ended(EndReason::Removed))));
        let summaries: Vec<String> = notes.0.lock().unwrap().iter().map(|n| n.summary.clone()).collect();
        // The pairing dialog wasn't open, so pairing notified too.
        assert_eq!(
            summaries,
            ["MacBook Air is paired", "Mac mini is paired", "MacBook Air connected", "Mac mini took over", "MacBook Air took over", "MacBook Air disconnected"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
