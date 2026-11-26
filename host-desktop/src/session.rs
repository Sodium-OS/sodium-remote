// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The session glue on remote-core's `connect::Host`: pairing,
//! the LAN listener and mDNS, `Welcome` for a physical display, the key table with the
//! foreground app, input, display selection, clipboard, audio and the host's stats.
//! Platform-free: the OS is behind `backend::Backend`.
//!
//! [`Session`] is the running host: the program (`run`) and the Mac app's "This
//! computer" pane (`ffi`, in-process) drive the same one. It reports what a pane shows
//! through [`Note`]s: the status (waiting, controlled by whom, secure screen), the
//! pairing window and its code, new pairings.

use std::io::{IsTerminal, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use remote_core::alloc::RefHint;
use remote_core::connect::{Host, HostConfig, HostEvent, PairingWindow};
use remote_core::keymap::{ForegroundApp, KeyMapper};
use remote_core::keys::{DeviceId, FileStore, PairedDevice, Store};
use remote_core::msg::{self, ClipFormat, ClipIncoming, ClipItem, GestureKind, InputEvent, Msg, Os};
use remote_core::server_config::ServerConfig;
use remote_core::session::SessionStats;

use crate::backend::{Backend, ClipEntry, Clipboard, Consent, Display, Input, InputOut, NotReady};
use crate::media::{Media, MediaEvent, StreamSetup, p50, stream_format};
use crate::opus::OpusEncoder;
use crate::policy;
use crate::vdisplay::{self, Choice, Matched};

/// The LAN listener's port when none is given, so "connect by address" and a firewall
/// rule have one to name (decisions table: builder, for). Taken: any free port.
pub const DEFAULT_PORT: u16 = 47311;
/// Bits per pixel per frame the route demand plans for.
const DEMAND_BPP: f64 = 0.2;
/// The paired clients' OS, as their last `Hello` said (`id os` per line), for the pane's
/// list: core's paired list has no OS.
const CLIENT_OS: &str = "client-os.txt";

/// Where the device key and the pairings are kept when they are not plain files in `Options::data`
/// (the Mac app: the key in the Keychain).
#[derive(Clone)]
pub struct StoreRef(pub Arc<dyn Store>);

impl std::fmt::Debug for StoreRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoreRef")
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// `pairing-code.txt`, `host.txt`, `client-os.txt`, and (with no `store`) the device key and pairings.
    pub data: PathBuf,
    /// The device key and pairings (default: files in `data`).
    pub store: Option<StoreRef>,
    /// The name devices show (default: the computer's name).
    pub name: Option<String>,
    /// The LAN listener's TCP port (0: any).
    pub port: u16,
    /// Open a pairing window now, and again whenever one expires.
    pub pair: bool,
    pub mdns: bool,
    /// Register with the rendezvous.
    pub rendezvous: bool,
    /// The server the app configured (the Mac keeps it in its preferences); used instead of `server.toml` when set.
    pub server: Option<remote_core::rendezvous::RendezvousConfig>,
    pub bind: Option<Vec<IpAddr>>,
    /// Stop after this long (tests).
    pub secs: Option<u64>,
}

/// Who is controlling this computer.
#[derive(Clone, Debug, PartialEq)]
pub struct Controller {
    pub id: DeviceId,
    pub name: String,
    pub os: Os,
    /// The client's os-release `ID` (empty off Linux).
    pub os_flavor: String,
    /// When the session started, Unix seconds.
    pub since: u64,
    /// Input can't be injected (a Mac without Accessibility): the pane shows "View only".
    pub view_only: bool,
}

/// What a "This computer" pane shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// On, nobody connected.
    Waiting,
    /// A client is in a session. `secure`: the lock screen (or Windows' secure desktop)
    /// is up, so the client sees a notice instead. `held`: the connection was lost and
    /// the session is held for it to come back.
    Controlled { client: Controller, secure: bool, held: bool },
}

/// What the session reports to its owner.
#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    Status(Status),
    /// A pairing window opened with this code.
    PairingOpen { code: String, expires_in: Duration },
    /// A device is trying the code.
    PairingAttempt,
    PairingFailed { attempts_left: u32 },
    /// The window ended: "paired", "expired", "attempts_used", "cancelled", …
    PairingClosed { reason: String },
    /// A device paired; the paired list changed.
    Paired(PairedDevice),
    /// The paired list changed otherwise (revoked, renamed).
    PairedChanged,
    /// The sharing permission changed, where the OS asks per session (generic Linux:
    /// the portal's dialog and its restore token).
    Consent(Consent),
    /// "Match the display to the connecting computer" is on, but the virtual display
    /// couldn't start: this session shows the physical display (Windows).
    MatchedDisplayFailed,
}

pub type Notes = Arc<dyn Fn(Note) + Send + Sync>;

/// A paired device, with the OS its last connection said.
#[derive(Clone, Debug, PartialEq)]
pub struct PairedClient {
    pub device: PairedDevice,
    pub os: Option<Os>,
    /// Its os-release `ID` as of the last connection (empty off Linux or unknown).
    pub os_flavor: String,
}

/// How long the client's window must stop changing before the stream follows it.
const RESIZE_QUIET: Duration = Duration::from_millis(150);

#[derive(Default)]
struct Live {
    client: Option<DeviceId>,
    hello: Option<msg::Hello>,
    settings: msg::StreamSettings,
    /// The display the client chose (`SelectDisplay`), kept across connections.
    display: Option<u32>,
    /// The client's window in physical pixels (`Hello.display`, then each `Resize`); 0: unknown.
    window: (u32, u32),
    stats: Option<SessionStats>,
    /// The client's name and when its session started (Unix seconds).
    name: String,
    since: u64,
    secure: bool,
    held: bool,
    /// The client was welcomed (the OS let this computer be shown): until then, and
    /// after the session ends, nothing it sends is applied.
    serving: bool,
    /// The matched display's id while it is up (`vdisplay`): the stream's only display.
    virtual_display: Option<u32>,
    /// The codec the stream fell back to when no encoder of the chosen one would open
    /// (`MediaEvent::CodecFallback`): kept for the session, so a resize doesn't try the other again.
    codec_fallback: Option<msg::Codec>,
}

struct Shared {
    host: Host,
    backend: Arc<dyn Backend>,
    input: Mutex<Box<dyn Input>>,
    keys: Mutex<KeyMapper>,
    live: Mutex<Live>,
    media: Mutex<Option<Media>>,
    /// Counts `Resize` messages: a resize applies once no newer one came within 150 ms.
    resize_gen: AtomicU64,
    /// The matched display while it is up.
    matched: Mutex<Option<Matched>>,
    /// Counts lost connections and new ones: a lost one's matched display goes 15 s later
    /// unless a connection came since.
    lost_gen: AtomicU64,
    audio: Mutex<Option<(Arc<AtomicBool>, std::thread::JoinHandle<()>)>>,
    clipboard: Option<Arc<dyn Clipboard>>,
    /// The host clipboard entry last offered, for the client's requests.
    clip_out: Mutex<Option<(u32, ClipEntry)>>,
    clip_serial: AtomicU32,
    /// The client's clipboard being assembled from `ClipboardData` chunks.
    clip_in: Mutex<Option<ClipIncoming>>,
    hint_seq: AtomicU64,
    opts: Options,
    /// The name devices show (`Session::set_name` changes it).
    name: Mutex<String>,
    notes: Option<Notes>,
    /// The last status reported.
    status: Mutex<Status>,
    /// The last consent reported.
    consent: Mutex<Option<Consent>>,
    me: std::sync::Weak<Shared>,
}

/// The program: a session until `--secs`, Ctrl+C or SIGTERM.
pub fn run(backend: Arc<dyn Backend>, o: Options) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build()?.block_on(async move {
        let secs = o.secs;
        let pair = o.pair;
        let s = Session::start(backend, o, None).await?;
        if pair || s.host().paired().is_empty() {
            s.open_pairing().await;
        }
        match secs {
            Some(n) => tokio::time::sleep(Duration::from_secs(n)).await,
            None => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
        s.shutdown().await;
        Ok(())
    })
}

/// A running host: listening on the LAN (and mDNS), pairing on request, serving one
/// client at a time. Lives in a tokio runtime (`start` spawns its tasks there); clones
/// are handles to the same host.
#[derive(Clone)]
pub struct Session {
    shared: Arc<Shared>,
    /// The event loop and the tick loop, stopped by `shutdown`.
    tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

/// The OS each paired device last connected from, by device ID (`client-os.txt`).
fn read_client_os(data: &std::path::Path) -> Vec<(String, Os, String)> {
    let text = std::fs::read_to_string(data.join(CLIENT_OS)).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (id, rest) = l.split_once(' ')?;
            // `id Os` or `id Os flavor` (older files have no flavor).
            let (os, flavor) = rest.split_once(' ').unwrap_or((rest, ""));
            let os = match os {
                "Mac" => Os::Mac,
                "Windows" => Os::Windows,
                "Linux" => Os::Linux,
                _ => return None,
            };
            Some((id.to_string(), os, flavor.to_string()))
        })
        .collect()
}

fn with_os(data: &std::path::Path, devices: Vec<PairedDevice>) -> Vec<PairedClient> {
    let os = read_client_os(data);
    devices
        .into_iter()
        .map(|d| {
            let known = os.iter().find(|(id, _, _)| *id == d.id.to_string());
            PairedClient { os: known.map(|(_, o, _)| *o), os_flavor: known.map(|(_, _, f)| f.clone()).unwrap_or_default(), device: d }
        })
        .collect()
}

/// The paired devices in a data directory, for when no session runs (the pane's list
/// while "Allow remote control" is off).
pub fn stored_paired(data: &std::path::Path, store: &dyn Store) -> Vec<PairedClient> {
    with_os(data, remote_core::keys::Pairings::load(store).devices)
}

/// Revokes a paired device in a data directory, for when no session runs.
pub fn stored_revoke(store: &dyn Store, id: &DeviceId) -> std::io::Result<bool> {
    let mut p = remote_core::keys::Pairings::load(store);
    let removed = p.remove(id);
    if removed {
        p.save(store)?;
    }
    Ok(removed)
}

impl Options {
    /// The store of the key and pairings.
    pub fn store(&self) -> Arc<dyn Store> {
        match &self.store {
            Some(s) => s.0.clone(),
            None => Arc::new(FileStore::new(self.data.clone())),
        }
    }
}

/// Creates the data folder 0700 (Linux, macOS) and tightens one that exists: the live pairing code is in it.
fn private_dir(dir: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).with_context(|| format!("securing {}", dir.display()))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(())
}

/// Writes a file 0600 on Linux and macOS (an existing, wider one is tightened too); on Windows the profile's ACL applies.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(bytes)
    }
    #[cfg(not(unix))]
    std::fs::write(path, bytes)
}

impl Session {
    pub async fn start(backend: Arc<dyn Backend>, o: Options, notes: Option<Notes>) -> Result<Session> {
        private_dir(&o.data)?;
        let name = o.name.clone().unwrap_or_else(|| backend.computer_name());
        remote_core::connect::load_named_networks();
        let mut cfg = HostConfig::new(o.store(), name.clone());
        cfg.rendezvous = if o.server.is_some() { o.server.clone() } else if o.rendezvous { ServerConfig::load_rendezvous() } else { None };
        cfg.lan = true;
        cfg.mdns = o.mdns;
        cfg.bind = o.bind.clone();
        cfg.lan_bind = SocketAddr::from(([0, 0, 0, 0], o.port));
        if o.port != 0 && std::net::TcpListener::bind(cfg.lan_bind).is_err() {
            eprintln!("sodium-remote-host: port {} is taken; listening on any free port", o.port);
            cfg.lan_bind = SocketAddr::from(([0, 0, 0, 0], 0));
        }
        // Probe the encoders before anyone connects (opening NVENC or an MFT takes a moment).
        let caps = {
            let b = backend.clone();
            tokio::task::spawn_blocking(move || b.encoder_caps()).await?
        };
        eprintln!("sodium-remote-host: encoders: hardware HEVC {}, H.264 {}", caps.hevc, caps.h264);
        let (host, mut events) = Host::start(cfg).await.context("starting remote-core's host")?;
        let port = host.lan_port().unwrap_or(0);
        eprintln!(
            "sodium-remote-host: {name:?}, device {} (code {}), LAN port {port}{}, data in {}",
            host.id(),
            host.id().code(),
            if o.mdns { ", announced over mDNS" } else { "" },
            o.data.display()
        );
        write_private(&o.data.join("host.txt"), format!("id={}\ncode={}\nport={port}\nname={name}\n", host.id(), host.id().code()).as_bytes())?;

        // A matched display left up by a host that stopped (a crash, a sign-out): the displays as they were.
        {
            let (b, data) = (backend.clone(), o.data.clone());
            tokio::task::spawn_blocking(move || recover_displays(&*b, &data)).await?;
        }
        let input = backend.input()?;
        // The clipboard's change callback is made before `Shared` exists: it finds it here.
        let slot: Arc<Mutex<std::sync::Weak<Shared>>> = Arc::new(Mutex::new(std::sync::Weak::new()));
        let clip_sink: crate::backend::ClipSink = {
            let slot = slot.clone();
            Arc::new(move |e: ClipEntry| {
                let s = slot.lock().unwrap().upgrade();
                if let Some(s) = s {
                    s.clip_offer_out(e);
                }
            })
        };
        let clipboard = match backend.clipboard(clip_sink) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("sodium-remote-host: no clipboard ({e:#})");
                None
            }
        };
        let shared = Arc::new_cyclic(|me| Shared {
            host: host.clone(),
            backend: backend.clone(),
            input: Mutex::new(input),
            keys: Mutex::new(KeyMapper::default()),
            live: Mutex::new(Live::default()),
            media: Mutex::new(None),
            resize_gen: AtomicU64::new(0),
            matched: Mutex::new(None),
            lost_gen: AtomicU64::new(0),
            audio: Mutex::new(None),
            clipboard,
            clip_out: Mutex::new(None),
            clip_serial: AtomicU32::new(1),
            clip_in: Mutex::new(None),
            hint_seq: AtomicU64::new(0),
            opts: o.clone(),
            name: Mutex::new(name),
            notes,
            status: Mutex::new(Status::Waiting),
            consent: Mutex::new(None),
            me: me.clone(),
        });
        *slot.lock().unwrap() = Arc::downgrade(&shared);
        shared.note(Note::Status(Status::Waiting));
        shared.update_consent();
        let tick = {
            let s = shared.clone();
            tokio::spawn(async move { s.tick_loop().await })
        };
        let task = {
            let s = shared.clone();
            tokio::spawn(async move {
                while let Some(ev) = events.recv().await {
                    s.clone().on_event(ev).await;
                }
            })
        };
        Ok(Session { shared, tasks: Arc::new(Mutex::new(vec![task, tick])) })
    }

    pub fn host(&self) -> &Host {
        &self.shared.host
    }

    /// The paired devices, with the OS each last connected from.
    pub fn paired(&self) -> Vec<PairedClient> {
        with_os(&self.shared.opts.data, self.shared.host.paired())
    }

    /// Open a pairing window (a new code; any old window closes). Its code also goes
    /// out as `Note::PairingOpen`.
    pub async fn open_pairing(&self) -> Option<PairingWindow> {
        self.shared.open_pairing().await
    }

    pub fn close_pairing(&self) {
        self.shared.host.close_pairing();
    }

    /// Revoke a paired device ("Remove"); a session it has ends.
    pub async fn revoke(&self, id: DeviceId) -> bool {
        let ok = self.shared.host.remove_paired(id).await;
        if ok {
            self.shared.note(Note::PairedChanged);
            // The last device gone: the saved screen-sharing permission (Linux restore token) goes with it.
            if self.shared.host.paired().is_empty() {
                self.forget_consent();
            }
        }
        ok
    }

    /// Rename a paired device on this computer's list.
    pub fn rename_device(&self, id: &DeviceId, name: &str) -> bool {
        let ok = self.shared.host.rename_paired(id, name);
        if ok {
            self.shared.note(Note::PairedChanged);
        }
        ok
    }

    /// "Name shown to other computers": mDNS, pairing and the next `Welcome`.
    pub fn set_name(&self, name: &str) {
        *self.shared.name.lock().unwrap() = name.to_string();
        self.shared.host.set_name(name);
    }

    /// End the current session ("Disconnect").
    pub fn disconnect(&self) {
        self.shared.host.disconnect(msg::DisconnectReason::User);
    }

    /// The LAN listener's bound TCP port (what "IP:PORT" shows).
    pub fn lan_port(&self) -> Option<u16> {
        self.shared.host.lan_port()
    }

    /// The computer's own name (the default "Name shown to other computers").
    pub fn computer_name(&self) -> String {
        self.shared.backend.computer_name()
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }

    /// The sharing permission where the OS asks per session (generic Linux), else None.
    pub fn consent(&self) -> Option<Consent> {
        self.shared.backend.consent()
    }

    /// Forgets a remembered sharing permission: the next connection shows the system's
    /// dialog again.
    pub fn forget_consent(&self) {
        self.shared.backend.forget_consent();
        self.shared.update_consent();
    }

    /// Stop serving (the switch goes off): the session ends ("control switched off"), the listener closes.
    pub async fn shutdown(&self) {
        eprintln!("sodium-remote-host: stopping");
        let tasks: Vec<_> = self.tasks.lock().unwrap().drain(..).collect();
        for t in tasks {
            t.abort();
        }
        let s = self.shared.clone();
        let _ = tokio::task::spawn_blocking(move || s.end_session()).await;
        self.shared.host.shutdown_with(msg::DisconnectReason::ControlOff).await;
    }
}

/// Why the session ended when the person at the computer didn't choose to stop it: a lock screen
/// (consent stays) or the person stopping the share.
fn ended_reason(locked: bool) -> msg::DisconnectReason {
    if locked { msg::DisconnectReason::HostLocked } else { msg::DisconnectReason::EndedAtComputer }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Shared {
    fn note(&self, n: Note) {
        if let Some(f) = &self.notes {
            f(n);
        }
    }

    /// Works the status out from the session and reports it when it changed.
    fn update_status(&self) {
        let status = {
            let l = self.live.lock().unwrap();
            match (l.client, &l.hello) {
                (Some(id), Some(h)) => Status::Controlled {
                    client: Controller { id, name: l.name.clone(), os: h.client_os, os_flavor: h.client_os_flavor.clone(), since: l.since, view_only: self.backend.view_only() },
                    secure: l.secure,
                    held: l.held,
                },
                _ => Status::Waiting,
            }
        };
        let changed = {
            let mut s = self.status.lock().unwrap();
            let changed = *s != status;
            *s = status.clone();
            changed
        };
        if changed {
            self.note(Note::Status(status));
        }
    }

    /// Reports the backend's consent when it changed.
    fn update_consent(&self) {
        let now = self.backend.consent();
        let changed = {
            let mut c = self.consent.lock().unwrap();
            let changed = *c != now;
            *c = now.clone();
            changed
        };
        if changed && let Some(c) = now {
            self.note(Note::Consent(c));
        }
    }

    fn remember_os(&self, id: &DeviceId, os: Os, flavor: &str) {
        let id = id.to_string();
        let mut v = read_client_os(&self.opts.data);
        v.retain(|(i, _, _)| *i != id);
        // Only a plain id is written: it is read back split on spaces.
        let flavor = if flavor.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) { flavor } else { "" };
        v.push((id, os, flavor.to_string()));
        let text: String = v.iter().map(|(i, o, f)| format!("{i} {o:?} {f}\n")).collect();
        let _ = std::fs::write(self.opts.data.join(CLIENT_OS), text);
    }

    async fn open_pairing(&self) -> Option<PairingWindow> {
        match self.host.open_pairing().await {
            Some(w) => {
                // The live code goes only where the person at the computer sees it: an interactive terminal. Never the journal
                // or a log file (those are read by others); everyone else reads the 0600 file in the 0700 data folder.
                let file = self.opts.data.join("pairing-code.txt");
                if std::io::stdout().is_terminal() {
                    println!("pairing code {}", w.code);
                    let _ = std::io::stdout().flush();
                }
                eprintln!("sodium-remote-host: pairing is open for {} s; the code is in {}", w.expires_in.as_secs(), file.display());
                let _ = write_private(&file, format!("{}\n", w.code).as_bytes());
                self.note(Note::PairingOpen { code: w.code.clone(), expires_in: w.expires_in });
                Some(w)
            }
            None => {
                eprintln!("sodium-remote-host: core didn't open a pairing window");
                None
            }
        }
    }

    async fn on_event(self: Arc<Self>, ev: HostEvent) {
        match ev {
            HostEvent::Rendezvous { online, error } => {
                eprintln!("sodium-remote-host: rendezvous {}{}", if online { "online" } else { "offline" }, error.map_or(String::new(), |e| format!(": {e}")))
            }
            HostEvent::PairingAttempt { attempt } => {
                eprintln!("sodium-remote-host: a device is pairing (attempt {attempt})");
                self.note(Note::PairingAttempt);
            }
            HostEvent::PairingFailed { attempts_left } => {
                eprintln!("sodium-remote-host: wrong code; {attempts_left} attempts left");
                self.note(Note::PairingFailed { attempts_left });
            }
            HostEvent::Paired(d) => {
                eprintln!("sodium-remote-host: paired with {:?} ({})", d.name, d.id.code());
                let _ = std::fs::remove_file(self.opts.data.join("pairing-code.txt"));
                self.note(Note::Paired(d));
            }
            HostEvent::PairingClosed { reason } => {
                if reason == "attempts_used" {
                    eprintln!("sodium-remote-host: pairing closed: 3 wrong codes were typed; pair again with --pair or the app");
                } else {
                    eprintln!("sodium-remote-host: pairing window closed: {reason}");
                }
                let _ = std::fs::remove_file(self.opts.data.join("pairing-code.txt"));
                self.note(Note::PairingClosed { reason: reason.clone() });
                // `--pair` keeps a code open while nobody pairs, but only past an expiry: after 3 wrong codes
                // pairing stays closed until someone at this computer opens it again.
                if self.opts.pair && reason == "expired" {
                    self.open_pairing().await;
                }
            }
            HostEvent::ClientConnected { client, name, hello, took_over_from, resumed } => {
                eprintln!(
                    "sodium-remote-host: {name:?} ({}) connected from {:?}{}{}",
                    client.code(),
                    hello.client_os,
                    if resumed { ", resumed" } else { "" },
                    took_over_from.as_ref().map(|f| format!(", took over from {f}")).unwrap_or_default()
                );
                // The OS may ask the person at this computer first (generic Linux: the
                // portal's dialog), which can take a while: off the event loop's thread.
                let ready = {
                    let b = self.backend.clone();
                    tokio::task::spawn_blocking(move || b.check_ready()).await.unwrap_or_else(|e| Err(NotReady::Other(e.to_string())))
                };
                self.update_consent();
                // The matched display first (it may take up to 3 s to appear): Welcome names it.
                if ready.is_ok() {
                    let (s, h) = (self.clone(), hello.clone());
                    let _ = tokio::task::spawn_blocking(move || s.prepare_display(&h)).await;
                }
                self.on_connected(client, name, hello, took_over_from, resumed, ready);
            }
            HostEvent::ClientLost { client } => {
                eprintln!("sodium-remote-host: lost {}; holding the session", client.code());
                self.pause();
                self.live.lock().unwrap().held = true;
                self.update_status();
                // The matched display waits 15 s for the client to come back.
                let generation = self.lost_gen.fetch_add(1, Ordering::Relaxed) + 1;
                let me = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(vdisplay::LOST_GRACE).await;
                    if me.lost_gen.load(Ordering::Relaxed) == generation && me.live.lock().unwrap().held {
                        let _ = tokio::task::spawn_blocking(move || me.end_matched()).await;
                    }
                });
            }
            HostEvent::HoldExpired { client } => {
                eprintln!("sodium-remote-host: {} didn't come back", client.code());
                if self.live.lock().unwrap().client == Some(client) {
                    let s = self.clone();
                    let _ = tokio::task::spawn_blocking(move || s.end_session()).await;
                }
            }
            HostEvent::ClientDisconnected { client, reason } => {
                eprintln!("sodium-remote-host: {} left: {reason:?}", client.code());
                if self.live.lock().unwrap().client == Some(client) {
                    let s = self.clone();
                    let _ = tokio::task::spawn_blocking(move || s.end_session()).await;
                }
            }
            // Input and the clipboard only from a client this computer is serving, and never in
            // view-only mode; control messages (display, settings, keyframes) whenever it is served.
            HostEvent::Input { event, .. } => {
                if self.live.lock().unwrap().serving && !self.backend.view_only() {
                    self.on_input(event);
                }
            }
            HostEvent::Control(m) => {
                let clipboard = matches!(m, Msg::ClipboardOffer(_) | Msg::ClipboardData(_));
                if self.live.lock().unwrap().serving && !(clipboard && self.backend.view_only()) {
                    self.on_control(m);
                }
            }
            HostEvent::PeerLostFrame { .. } | HostEvent::ReceiverReport { .. } => {}
            HostEvent::Route(r) => eprintln!("sodium-remote-host: route {:?} {} -> {} ({:.1} ms)", r.kind, r.local, r.remote, r.rtt_ms),
            HostEvent::Stats(s) => self.live.lock().unwrap().stats = Some(s),
        }
    }

    /// The stream for this client: its display (the one it chose, else the primary),
    /// the size that fits its window and its decoder (never above native), its rate (at most the display's), and
    /// the codec both sides have.
    fn setup(&self) -> Result<StreamSetup, String> {
        let (hello, settings, chosen, window, fallback) = {
            let l = self.live.lock().unwrap();
            // The matched display is the only one while it is up.
            (l.hello.clone(), l.settings, l.virtual_display.or(l.display), l.window, l.codec_fallback)
        };
        let hello = hello.ok_or("no Hello")?;
        let displays = self.backend.displays();
        let display = chosen
            .and_then(|id| displays.iter().find(|d| d.info.id == id))
            .or_else(|| displays.iter().find(|d| d.info.primary))
            .or(displays.first())
            .cloned()
            .ok_or("this computer has no display to show")?;
        let codec = policy::choose_codec(self.backend.encoder_caps(), &hello.decode, settings.codec)
            .ok_or("no video codec in common: this computer encodes H.264 (and HEVC with a GPU) only")?;
        let codec = fallback.filter(|_| codec == msg::Codec::Hevc420).unwrap_or(codec);
        let (width, height) = policy::stream_size(
            (display.info.width, display.info.height),
            window,
            (hello.decode.max_width, hello.decode.max_height),
        );
        let asked = if settings.refresh_mhz != 0 { settings.refresh_mhz } else { hello.display.refresh_mhz };
        let host_hz = if display.refresh_mhz > 0 { display.refresh_mhz } else { 60_000 };
        let fps = (asked.min(host_hz).max(1000) as f64 / 1000.0).round().max(1.0) as u32;
        Ok(StreamSetup { display, width, height, fps, codec })
    }

    fn on_connected(
        self: &Arc<Self>,
        client: DeviceId,
        name: String,
        hello: msg::Hello,
        took_over_from: Option<String>,
        resumed: bool,
        ready: Result<(), NotReady>,
    ) {
        self.remember_os(&client, hello.client_os, &hello.client_os_flavor);
        let version = hello.negotiated();
        {
            let mut l = self.live.lock().unwrap();
            // A resumed session keeps its start time.
            if !(resumed && l.client == Some(client)) {
                l.since = unix_now();
            }
            l.client = Some(client);
            l.name = name;
            l.settings = hello.settings;
            l.window = (hello.display.width, hello.display.height);
            l.hello = Some(hello);
            l.secure = false;
            l.held = false;
            l.serving = false;
            l.codec_fallback = None;
        }
        // A new connection is a new session: its table comes after Hello.
        self.keys.lock().unwrap().set_table(vec![]);
        self.input.lock().unwrap().inject(InputOut::ReleaseAll);
        // What keeps this computer from being shown (a Mac without Screen Recording)
        // goes to the client as the reason.
        let setup = match ready.map_err(|n| match n {
            NotReady::Permission { screen_recording, accessibility } => {
                msg::DisconnectReason::PermissionMissing { screen_recording, accessibility }
            }
            NotReady::Declined => msg::DisconnectReason::EndedAtComputer,
            NotReady::Locked => msg::DisconnectReason::HostLocked,
            NotReady::Other(e) => msg::DisconnectReason::Error(e),
        }).and_then(|()| self.setup().map_err(msg::DisconnectReason::Error)) {
            Ok(s) => s,
            Err(reason) => {
                eprintln!("sodium-remote-host: can't serve this client: {reason:?}");
                self.host.disconnect(reason);
                return;
            }
        };
        self.live.lock().unwrap().serving = true;
        self.backend.session_active(true);
        self.update_status();
        let displays = self.backend.displays();
        let virtual_display = self.live.lock().unwrap().virtual_display.is_some();
        self.host.send(&Msg::Welcome(msg::Welcome {
            version,
            name: self.name.lock().unwrap().clone(),
            format: stream_format(&setup),
            scales: vec![],
            home_monitor: None,
            locked: false,
            took_over_from,
            resumed,
            host_os: self.backend.os(),
            display_mode: if virtual_display { msg::DisplayMode::Virtual } else { msg::DisplayMode::Physical },
            live_resize: self.backend.live_resize(),
            displays: displays.iter().map(|d| d.info.clone()).collect(),
            current_display: setup.display.info.id,
            view_only: self.backend.view_only(),
            host_os_flavor: self.backend.os_flavor(),
        }));
        eprintln!(
            "sodium-remote-host: welcome: display {} {:?} {}x{} as {}x{} at {} fps, {:?}",
            setup.display.info.id,
            setup.display.info.name,
            setup.display.info.width,
            setup.display.info.height,
            setup.width,
            setup.height,
            setup.fps,
            setup.codec
        );
        self.start_media(setup);
        self.start_audio();
        if let Some(e) = self.clipboard.as_ref().and_then(|c| c.get()) {
            self.clip_offer_out(e);
        }
    }

    fn start_media(self: &Arc<Self>, setup: StreamSetup) {
        if let Some(old) = self.media.lock().unwrap().take() {
            old.stop();
        }
        self.send_route_demand(&setup);
        let weak = self.me.clone();
        let events: Arc<dyn Fn(MediaEvent) + Send + Sync> = Arc::new(move |ev| {
            let Some(s) = weak.upgrade() else { return };
            match ev {
                MediaEvent::Failed(e) => s.host.disconnect(msg::DisconnectReason::Error(format!("the host's video stopped: {e}"))),
                // "Ended at the computer": the person there stopped sharing.
                MediaEvent::Ended => {
                    s.host.disconnect(ended_reason(s.backend.locked()));
                    s.update_consent();
                }
                MediaEvent::CodecFallback(c) => s.live.lock().unwrap().codec_fallback = Some(c),
                MediaEvent::Secure(on) => {
                    s.live.lock().unwrap().secure = on;
                    if on {
                        // Not drivable: nothing stays held into it.
                        s.keys.lock().unwrap().clear();
                        s.input.lock().unwrap().inject(InputOut::ReleaseAll);
                    }
                    s.update_status();
                }
            }
        });
        match Media::start(self.backend.clone(), self.host.clone(), setup, events) {
            Ok(m) => *self.media.lock().unwrap() = Some(m),
            Err(e) => {
                eprintln!("sodium-remote-host: media didn't start: {e:#}");
                self.host.disconnect(msg::DisconnectReason::Error(format!("the host's video couldn't start: {e}")));
            }
        }
    }

    /// The route demand at the stream's mode and the user's cap.
    fn send_route_demand(&self, s: &StreamSetup) {
        let cap = self.live.lock().unwrap().settings.bandwidth_limit_kbps;
        let mut demand = DEMAND_BPP * s.width as f64 * s.height as f64 * s.fps as f64;
        if cap > 0 {
            demand = demand.min(cap as f64 * 1000.0);
        }
        self.host.set_route_demand(demand as u64);
    }

    fn start_audio(&self) {
        let mut g = self.audio.lock().unwrap();
        if g.is_some() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (backend, host, st) = (self.backend.clone(), self.host.clone(), stop.clone());
        let t = std::thread::Builder::new().name("audio".into()).spawn(move || audio_loop(backend, host, st));
        if let Ok(t) = t {
            *g = Some((stop, t));
        }
    }

    fn stop_audio(&self) {
        if let Some((stop, t)) = self.audio.lock().unwrap().take() {
            stop.store(true, Ordering::Relaxed);
            let _ = t.join();
        }
    }

    /// The connection was lost: stop the stream (the hold keeps the session in core).
    fn pause(&self) {
        if let Some(m) = self.media.lock().unwrap().take() {
            m.stop();
        }
        self.stop_audio();
        self.keys.lock().unwrap().clear();
        self.input.lock().unwrap().inject(InputOut::ReleaseAll);
    }

    fn end_session(&self) {
        self.pause();
        self.end_matched();
        let was = {
            let mut l = self.live.lock().unwrap();
            let was = l.client.take().is_some();
            l.hello = None;
            l.secure = false;
            l.held = false;
            l.serving = false;
            was
        };
        *self.clip_out.lock().unwrap() = None;
        *self.clip_in.lock().unwrap() = None;
        if was {
            self.backend.session_active(false);
        }
        self.update_status();
        self.update_consent();
    }

    fn current_setup(&self) -> Option<StreamSetup> {
        self.media.lock().unwrap().as_ref().map(|m| m.setup())
    }

    fn on_input(&self, ev: InputEvent) {
        // The secure screen is neither shown nor driven. Windows refuses
        // input there anyway; macOS would let posted events reach the lock screen.
        if self.live.lock().unwrap().secure && !matches!(ev, InputEvent::ReleaseAll) {
            return;
        }
        let mut input = self.input.lock().unwrap();
        match ev {
            InputEvent::Key { usage, down } => {
                let mut keys = self.keys.lock().unwrap();
                // The foreground app matters only when a table applies.
                let fg = if keys.passthrough() { Default::default() } else { self.backend.foreground() };
                let mut out = Vec::with_capacity(4);
                keys.key(usage, down, ForegroundApp { id: &fg.id, terminal: fg.terminal }, &mut out);
                for k in out {
                    input.inject(InputOut::Key { usage: k.usage, down: k.pressed });
                }
            }
            InputEvent::ImeCommit { text } => input.inject(InputOut::Text(text)),
            // No remote preedit on this host: only commits.
            InputEvent::ImePreedit { .. } => {}
            InputEvent::PointerAbs { x, y } => {
                if let Some(s) = self.current_setup() {
                    let i = &s.display.info;
                    let (x, y) = policy::stream_to_display(x, y, (s.width, s.height), (i.width, i.height));
                    input.inject(InputOut::PointerAbs { display: i.id, x, y });
                }
            }
            InputEvent::PointerRel { dx, dy, .. } => {
                let (sx, sy) = self.current_setup().map_or((1.0, 1.0), |s| {
                    (s.display.info.width as f64 / s.width as f64, s.display.info.height as f64 / s.height as f64)
                });
                input.inject(InputOut::PointerRel { dx: dx * sx, dy: dy * sy });
            }
            InputEvent::Button { button, down } => input.inject(InputOut::Button { button, down }),
            InputEvent::Axis { source, axis, value, value120, stop, more } => {
                input.inject(InputOut::Axis { source, axis, value, value120, stop, more })
            }
            InputEvent::Gesture { kind: GestureKind::Pinch, phase, scale, .. } => input.inject(InputOut::Zoom { phase, scale }),
            InputEvent::Gesture { .. } => {}
            InputEvent::ReleaseAll => {
                self.keys.lock().unwrap().clear();
                input.inject(InputOut::ReleaseAll);
            }
        }
    }

    fn on_control(self: &Arc<Self>, m: Msg) {
        match m {
            Msg::KeyTable(t) => {
                eprintln!("sodium-remote-host: key table with {} rules{}", t.len(), if t.is_empty() { " (PC passthrough)" } else { "" });
                self.keys.lock().unwrap().set_table(t);
            }
            Msg::KeyframeRequest => {
                if let Some(m) = self.media.lock().unwrap().as_ref() {
                    m.request_keyframe();
                }
            }
            Msg::SelectDisplay(id) => {
                if self.live.lock().unwrap().virtual_display.is_some() {
                    return; // the matched display is the only one
                }
                if !self.backend.displays().iter().any(|d| d.info.id == id) {
                    eprintln!("sodium-remote-host: the client chose display {id}, which isn't here");
                    return;
                }
                self.live.lock().unwrap().display = Some(id);
                self.restart_stream();
            }
            Msg::Settings(st) => {
                let old = std::mem::replace(&mut self.live.lock().unwrap().settings, st);
                if old.refresh_mhz != st.refresh_mhz && self.live.lock().unwrap().virtual_display.is_some() {
                    self.schedule_matched_mode();
                } else if old.codec != st.codec || old.refresh_mhz != st.refresh_mhz {
                    self.restart_stream();
                } else if let Some(s) = self.current_setup() {
                    self.send_route_demand(&s);
                }
            }
            Msg::ClipboardOffer(o) => self.clip_offer_in(o),
            Msg::ClipboardRequest { serial, format } => self.clip_request(serial, &format),
            Msg::ClipboardData(d) => self.clip_data_in(d),
            // The client's window changed: the stream follows it once the resizing stops.
            Msg::Resize(d) => {
                if !self.backend.live_resize() {
                    return;
                }
                self.live.lock().unwrap().window = (d.width, d.height);
                let generation = self.resize_gen.fetch_add(1, Ordering::Relaxed) + 1;
                let me = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(RESIZE_QUIET).await;
                    if me.resize_gen.load(Ordering::Relaxed) == generation {
                        me.apply_resize();
                    }
                    // The matched display takes the window's size once it has settled; scaling covered the drag.
                    if me.live.lock().unwrap().virtual_display.is_some() {
                        tokio::time::sleep(vdisplay::MODE_QUIET.saturating_sub(RESIZE_QUIET)).await;
                        if me.resize_gen.load(Ordering::Relaxed) == generation {
                            me.schedule_matched_mode();
                        }
                    }
                });
            }
            // A physical display: no UI size or home monitor mode (Welcome says so).
            Msg::HomeMonitor(_) | Msg::Vsync(_) | Msg::Stats(_) | Msg::Hello(_) => {}
            other => eprintln!("sodium-remote-host: ignoring {}", format!("{other:?}").split(['(', ' ', '{']).next().unwrap_or("")),
        }
    }

    /// The window settled: when only the size changed, a new encoder on the running
    /// capture and a new `Format`; anything else (a display hot-plug since) restarts the stream.
    fn apply_resize(self: &Arc<Self>) {
        let setup = match self.setup() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("sodium-remote-host: {e}");
                return;
            }
        };
        let Some(cur) = self.current_setup() else { return };
        if cur == setup {
            return;
        }
        if cur.display == setup.display && cur.fps == setup.fps && cur.codec == setup.codec {
            self.send_route_demand(&setup);
            if let Some(m) = self.media.lock().unwrap().as_ref() {
                m.resize(setup.width, setup.height);
            }
        } else {
            self.restart_stream();
        }
    }

    /// The display for a connection about to be welcomed: the matched display when the setting is on (created, or
    /// resized for this client), else the physical one. A matched display that doesn't start falls back to the physical
    /// display and scaling for this session, and the app is told. Blocking (up to about 4 s).
    fn prepare_display(&self, hello: &msg::Hello) {
        // A connection came: a lost one's matched display stays.
        self.lost_gen.fetch_add(1, Ordering::Relaxed);
        let topo = self.backend.topology();
        let wanted = topo.is_some() && vdisplay::wanted(&self.opts.data);
        let refresh = if hello.settings.refresh_mhz != 0 { hello.settings.refresh_mhz } else { hello.display.refresh_mhz };
        let mode = vdisplay::matched_mode((hello.display.width, hello.display.height), refresh, (hello.decode.max_width, hello.decode.max_height));
        let mut g = self.matched.lock().unwrap();
        let id = match g.as_mut() {
            // Turned off since the last connection.
            Some(_) if !wanted => {
                if let Some(m) = g.take() {
                    m.end();
                }
                None
            }
            Some(m) => {
                if let Err(e) = m.set_mode(mode, vdisplay::APPEAR_WITHIN) {
                    eprintln!("sodium-remote-host: matched display: {e:#}; keeping its size");
                }
                Some(m.display())
            }
            None => match vdisplay::open(wanted, topo, || self.backend.virtual_display(), self.opts.data.join(vdisplay::JOURNAL), mode, vdisplay::APPEAR_WITHIN) {
                Choice::Virtual(m) => {
                    let id = m.display();
                    *g = Some(m);
                    Some(id)
                }
                Choice::Failed => {
                    self.note(Note::MatchedDisplayFailed);
                    None
                }
                Choice::Physical => None,
            },
        };
        drop(g);
        if let Some(id) = id {
            self.wait_listed(id);
        }
        self.live.lock().unwrap().virtual_display = id;
    }

    /// Until the capture's list of displays has `id` (about 1 s at most).
    fn wait_listed(&self, id: u32) {
        let until = Instant::now() + Duration::from_secs(1);
        while !self.backend.displays().iter().any(|d| d.info.id == id) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The matched display at the client's window size and rate now, then the stream on it (a new one when anything
    /// changed: the display's mode, or only the rate the client asked for).
    fn schedule_matched_mode(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            let s = me.clone();
            let _ = tokio::task::spawn_blocking(move || s.apply_matched_mode()).await;
            me.restart_stream();
        });
    }

    /// True when the matched display changed mode.
    fn apply_matched_mode(&self) -> bool {
        let mode = {
            let l = self.live.lock().unwrap();
            let Some(h) = l.hello.as_ref() else { return false };
            let refresh = if l.settings.refresh_mhz != 0 { l.settings.refresh_mhz } else { h.display.refresh_mhz };
            vdisplay::matched_mode(l.window, refresh, (h.decode.max_width, h.decode.max_height))
        };
        // Not while the media loop opens an encoder at the scaled size: the mode change disturbs the GPU.
        if let Some(media) = self.media.lock().unwrap().as_ref() {
            media.settle(Duration::from_secs(3));
        }
        let mut g = self.matched.lock().unwrap();
        let Some(m) = g.as_mut() else { return false };
        if m.mode() == Some(mode) {
            return false;
        }
        if let Err(e) = m.set_mode(mode, vdisplay::APPEAR_WITHIN) {
            eprintln!("sodium-remote-host: matched display: {e:#}; keeping its size");
            return false;
        }
        let id = m.display();
        drop(g);
        self.wait_listed(id);
        self.live.lock().unwrap().virtual_display = Some(id);
        true
    }

    /// The matched display goes: the displays as they were. Blocking.
    fn end_matched(&self) {
        let m = self.matched.lock().unwrap().take();
        if let Some(m) = m {
            m.end();
        }
        self.live.lock().unwrap().virtual_display = None;
    }

    /// A new display or settings: a new stream, announced with `Format`.
    fn restart_stream(self: &Arc<Self>) {
        let setup = match self.setup() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("sodium-remote-host: {e}");
                return;
            }
        };
        if self.current_setup().as_ref() == Some(&setup) {
            return;
        }
        eprintln!(
            "sodium-remote-host: now display {} at {}x{} {} fps {:?}",
            setup.display.info.id, setup.width, setup.height, setup.fps, setup.codec
        );
        self.host.send(&Msg::Format(stream_format(&setup)));
        self.start_media(setup);
    }

    /// Reference hints (every 5 ms: to the media loop, whose gate turns them into at most
    /// one keyframe per 250 ms, since these encoders can't predict from an older frame)
    /// and the host's stats to the client (every second).
    async fn tick_loop(self: Arc<Self>) {
        let mut t = tokio::time::interval(Duration::from_millis(5));
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_stats = Instant::now();
        let mut last_log = Instant::now();
        let mut totals = (0u64, 0u64, 0u64);
        loop {
            t.tick().await;
            if let Some(ls) = self.host.link_status()
                && ls.ref_hint_seq != self.hint_seq.swap(ls.ref_hint_seq, Ordering::Relaxed)
                && let Some(hint) = ls.ref_hint
                && let Some(m) = self.media.lock().unwrap().as_ref()
            {
                match hint {
                    RefHint::Reference(id) => eprintln!("sodium-remote-host: the client lost frames after {id}"),
                    RefHint::Keyframe => eprintln!("sodium-remote-host: the client has nothing to predict from"),
                }
                m.ref_hint(hint);
            }
            if last_stats.elapsed() < Duration::from_secs(1) {
                continue;
            }
            let secs = last_stats.elapsed().as_secs_f64();
            last_stats = Instant::now();
            // The dialog's state changes while a connection waits on it.
            self.update_consent();
            let Some(acc) = self.media.lock().unwrap().as_ref().map(|m| m.acc.clone()) else { continue };
            let (target_bps, c_est_bps, probe_ceiling_bps, rtt) = self
                .live
                .lock()
                .unwrap()
                .stats
                .as_ref()
                .map_or((0, 0, 0, 0.0), |s| (s.target_bps, s.c_est_bps, s.probe_ceiling_bps, s.rtt_ms));
            let (hs, line) = {
                let mut a = acc.lock().unwrap();
                let fps = a.frames as f64 / secs;
                let hs = msg::HostStats {
                    capture_us: p50(&mut a.capture_us),
                    convert_us: p50(&mut a.convert_us),
                    encode_us: p50(&mut a.encode_us),
                    send_us: 0,
                    fps: fps as f32,
                    bitrate_bps: (a.bytes as f64 * 8.0 / secs) as u64,
                    qp: 0,
                    optimised_look: false,
                    readable_ms: 0,
                    lossless_ms: 0,
                    settled_ms: 0,
                    settled_end: 0,
                    target_bps,
                    c_est_bps,
                    probe_ceiling_bps,
                };
                totals.0 += a.frames as u64;
                totals.1 += a.keyframes as u64;
                totals.2 += a.refreshes as u64;
                let line = format!(
                    "{} {:.1} fps, {:.1} Mbit/s (target {:.1}), capture {:.1} / convert {:.1} / encode {:.1} ms, rtt {rtt:.1} ms; {} frames, {} keyframes, {} refreshes so far",
                    a.encoder,
                    fps,
                    hs.bitrate_bps as f64 / 1e6,
                    target_bps as f64 / 1e6,
                    hs.capture_us as f64 / 1000.0,
                    hs.convert_us as f64 / 1000.0,
                    hs.encode_us as f64 / 1000.0,
                    totals.0,
                    totals.1,
                    totals.2
                );
                let encoder = std::mem::take(&mut a.encoder);
                *a = crate::media::Acc { encoder, ..Default::default() };
                (hs, line)
            };
            self.host.send(&Msg::Stats(msg::Stats { host: Some(hs), client: None }));
            if last_log.elapsed() >= Duration::from_secs(5) {
                last_log = Instant::now();
                eprintln!("sodium-remote-host: {line}");
            }
        }
    }

    // ---- clipboard, as remoted does it --------------------------------

    fn clip_offer_out(&self, entry: ClipEntry) {
        if self.live.lock().unwrap().client.is_none() || entry.items.is_empty() {
            return;
        }
        // One copy can change the clipboard twice (an OLE copy, then its flush): offer it once.
        if self.clip_out.lock().unwrap().as_ref().is_some_and(|(_, e)| e.items == entry.items && e.concealed == entry.concealed) {
            return;
        }
        let serial = self.clip_serial.fetch_add(1, Ordering::Relaxed);
        let items = entry
            .items
            .iter()
            .map(|(format, data)| ClipItem {
                format: format.clone(),
                size: data.len() as u64,
                inline: (data.len() <= msg::CLIPBOARD_CHUNK).then(|| data.clone()),
            })
            .collect();
        eprintln!(
            "sodium-remote-host: clipboard to the client: {}",
            entry.items.iter().map(|(f, d)| format!("{f:?} {} bytes", d.len())).collect::<Vec<_>>().join(", ")
        );
        self.host.send(&Msg::ClipboardOffer(msg::ClipboardOffer { serial, items, concealed: entry.concealed }));
        *self.clip_out.lock().unwrap() = Some((serial, entry));
    }

    fn clip_request(&self, serial: u32, format: &ClipFormat) {
        let g = self.clip_out.lock().unwrap();
        let Some((s, entry)) = g.as_ref() else { return };
        if *s != serial {
            return;
        }
        if let Some((_, data)) = entry.items.iter().find(|(f, _)| f == format) {
            for m in msg::ClipboardData::chunks(serial, format, data) {
                self.host.send(&m);
            }
        }
    }

    fn clip_offer_in(&self, o: msg::ClipboardOffer) {
        // Only what the host's clipboard can hold; the rest isn't fetched.
        let keep = |f: &ClipFormat| matches!(f, ClipFormat::Text | ClipFormat::Html | ClipFormat::Png);
        let Some((inc, asks)) = ClipIncoming::from_offer(&o, keep) else { return };
        for format in asks {
            self.host.send(&Msg::ClipboardRequest { serial: o.serial, format });
        }
        *self.clip_in.lock().unwrap() = Some(inc);
        self.clip_finish();
    }

    fn clip_data_in(&self, d: msg::ClipboardData) {
        if let Some(inc) = self.clip_in.lock().unwrap().as_mut() {
            inc.add(&d);
        }
        self.clip_finish();
    }

    /// Every item complete: it becomes the host's clipboard.
    fn clip_finish(&self) {
        let mut g = self.clip_in.lock().unwrap();
        if !g.as_ref().is_some_and(ClipIncoming::is_complete) {
            return;
        }
        let inc = g.take().unwrap();
        let entry = ClipEntry { concealed: inc.concealed, items: inc.into_items() };
        eprintln!(
            "sodium-remote-host: clipboard from the client: {}",
            entry.items.iter().map(|(f, d)| format!("{f:?} {} bytes", d.len())).collect::<Vec<_>>().join(", ")
        );
        if let Some(c) = &self.clipboard {
            c.set(entry);
        }
    }
}

/// Desktop audio → Opus → core, while a client is connected.
fn audio_loop(backend: Arc<dyn Backend>, host: Host, stop: Arc<AtomicBool>) {
    let Some(mut enc) = OpusEncoder::new() else {
        eprintln!("sodium-remote-host: no Opus encoder");
        return;
    };
    let mut buf = [0f32; 960];
    let mut ts = 0u32;
    let mut packets = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let mut src = match backend.audio() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("sodium-remote-host: no desktop audio ({e:#}); trying again in 5 s");
                for _ in 0..50 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                continue;
            }
        };
        while !stop.load(Ordering::Relaxed) {
            match src.read(&mut buf, Duration::from_millis(100)) {
                Ok(true) => {
                    if let Some(p) = enc.encode(&buf) {
                        host.send_audio(ts, None, p);
                        ts = ts.wrapping_add(480);
                        packets += 1;
                        if packets == 1 {
                            eprintln!("sodium-remote-host: desktop audio is flowing");
                        }
                    }
                }
                Ok(false) => {}
                Err(e) => {
                    eprintln!("sodium-remote-host: desktop audio: {e:#}");
                    break;
                }
            }
        }
    }
}

/// At startup: a matched display left up by a host that stopped (its journal in `data`) is undone
/// (`vdisplay::recover`). Does nothing where the backend has no display configuration.
pub fn recover_displays(backend: &dyn Backend, data: &std::path::Path) -> bool {
    let journal = data.join(vdisplay::JOURNAL);
    if !journal.exists() {
        return false;
    }
    let Some(topo) = backend.topology() else { return false };
    let driver = backend.virtual_display().ok();
    vdisplay::recover(&journal, driver.as_deref(), &*topo)
}

/// Display helper for logs.
pub fn describe(d: &Display) -> String {
    format!("{} {:?} {}x{} @{:.0} Hz scale {}{}", d.info.id, d.info.name, d.info.width, d.info.height, d.refresh_mhz as f64 / 1000.0, d.info.scale, if d.info.primary { " primary" } else { "" })
}

#[cfg(test)]
mod reason_tests {
    use super::*;

    #[test]
    fn a_lock_and_a_stopped_share_end_with_their_own_reasons() {
        assert_eq!(ended_reason(true), msg::DisconnectReason::HostLocked);
        assert_eq!(ended_reason(false), msg::DisconnectReason::EndedAtComputer);
    }
}
