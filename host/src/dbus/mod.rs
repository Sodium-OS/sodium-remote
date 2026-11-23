// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! `org.sodiumos.Remote` on the session bus: what Settings › Remote and the
//! shell's privacy pill read and do (the
//! reference is docs/reference/host-session.md, "D-Bus API"). Also the clients remoted uses:
//! desktop notifications and logind (the sleep notice, the lock state), and the
//! test-only `org.sodiumos.Remote.Dev` that stands in for a client connection
//! until core's session lands.

use std::{collections::HashMap, os::fd::OwnedFd, sync::Arc, time::Duration};

use anyhow::Result;
use futures_util::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};
use zbus::{
    Connection, fdo, interface, proxy,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

use crate::{
    clipboard::ClipEntry,
    display::{HomeMode, Request, UiSize},
    session::{Cmd, ConnectRequest, Links, Notice, Notifier, RecordingLink, SettingChange, Status},
};

pub const NAME: &str = "org.sodiumos.Remote";
pub const PATH: &str = "/org/sodiumos/Remote";
/// `ApiVersion` of `org.sodiumos.Remote`; the checked-in introspection XML (`dbus/`) is its reference.
pub const API_VERSION: u32 = 2;
pub const DEV_PATH: &str = "/org/sodiumos/Remote/Dev";

type Dict = HashMap<String, OwnedValue>;

fn ov<'a>(v: impl Into<Value<'a>>) -> OwnedValue {
    v.into().try_to_owned().expect("no fds in these values")
}

fn failed(e: String) -> fdo::Error {
    fdo::Error::Failed(e)
}

pub struct RemoteApi {
    tx: mpsc::UnboundedSender<Cmd>,
    status: watch::Receiver<Status>,
}

impl RemoteApi {
    fn st(&self) -> Status {
        self.status.borrow().clone()
    }

    async fn ask<T>(&self, make: impl FnOnce(oneshot::Sender<Result<T, String>>) -> Cmd) -> fdo::Result<T> {
        let (r, w) = oneshot::channel();
        self.tx.send(make(r)).map_err(|_| failed("remoted is stopping".into()))?;
        w.await.map_err(|_| failed("remoted is stopping".into()))?.map_err(failed)
    }

    async fn setting(&self, change: SettingChange) -> fdo::Result<()> {
        self.ask(|reply| Cmd::Setting { change, reply }).await
    }
}

#[interface(name = "org.sodiumos.Remote")]
impl RemoteApi {
    // ---- status ----

    /// The version of this API. 2: `Session.path` says `vpn` and `relay` (v1 used other names for them). Changes after this one only add.
    #[zbus(property)]
    fn api_version(&self) -> u32 {
        API_VERSION
    }

    /// "off", "ready", "not_ready", "connected", "reconnecting".
    #[zbus(property)]
    fn state(&self) -> String {
        self.st().state.into()
    }

    /// This host's device ID (its Ed25519 public key, base64url).
    #[zbus(property)]
    fn device_id(&self) -> String {
        self.st().device_id
    }

    /// The code people compare by eye, `XXXX-XXXX`. A fingerprint only:
    /// copying always copies `DeviceId`.
    #[zbus(property)]
    fn device_code(&self) -> String {
        self.st().device_id.parse::<remote_core::keys::DeviceId>().map(|i| i.code()).unwrap_or_default()
    }

    /// The session: empty when nobody is connected.
    #[zbus(property)]
    fn session(&self) -> Dict {
        let Some(s) = self.st().session else { return Dict::new() };
        let mut d = Dict::new();
        d.insert("device".into(), ov(s.device));
        d.insert("name".into(), ov(s.name));
        d.insert("since".into(), ov(s.since));
        d.insert("path".into(), ov(s.stats.path));
        d.insert("transport".into(), ov(s.stats.transport));
        if !s.stats.network.is_empty() {
            // The label of the named network a "vpn" path is in (networks.toml).
            d.insert("network".into(), ov(s.stats.network));
        }
        d.insert("rtt_ms".into(), ov(s.stats.rtt_ms));
        d.insert("bitrate_kbps".into(), ov(s.stats.bitrate_kbps));
        d.insert("fps".into(), ov(s.stats.fps));
        if let Some(a) = s.display {
            d.insert("width".into(), ov(a.width));
            d.insert("height".into(), ov(a.height));
            d.insert("refresh_mhz".into(), ov(a.refresh_mhz));
            d.insert("scale".into(), ov(a.scale));
        }
        d.insert("held".into(), ov(s.held_until.is_some()));
        d.insert("held_until".into(), ov(s.held_until.unwrap_or(0)));
        d.insert("locked_at_connect".into(), ov(s.locked));
        d.insert("home_mode".into(), ov(s.home_mode.as_str()));
        d.insert("mic".into(), ov(s.mic));
        d
    }

    /// Every paired device: id, name, kind, paired_at, last_seen, connected.
    #[zbus(property)]
    fn paired_devices(&self) -> Vec<Dict> {
        self.st()
            .devices
            .into_iter()
            .map(|(dev, connected)| {
                let mut d = Dict::new();
                d.insert("id".into(), ov(dev.id));
                d.insert("name".into(), ov(dev.name));
                d.insert("kind".into(), ov(dev.kind));
                d.insert("paired_at".into(), ov(dev.paired_at));
                // Unix seconds of the last connection, 0 = never.
                d.insert("last_seen".into(), ov(dev.last_connected.unwrap_or(0)));
                d.insert("connected".into(), ov(connected));
                d
            })
            .collect()
    }

    /// The pairing window: code, expires_at, attempts_left, state ("open", "attempt": a
    /// device is running SPAKE2 against the code, "attempts_used": 3 wrong codes closed it,
    /// "expired", "paired") and, once paired, paired_name. Empty when none.
    #[zbus(property)]
    fn pairing(&self) -> Dict {
        let Some(p) = self.st().pairing else { return Dict::new() };
        let mut d = Dict::new();
        d.insert("code".into(), ov(p.code));
        d.insert("expires_at".into(), ov(p.expires_at));
        d.insert("attempts_left".into(), ov(p.attempts_left));
        d.insert("state".into(), ov(p.state));
        d.insert("paired_name".into(), ov(p.paired.unwrap_or_default()));
        d
    }

    /// Settings tells remoted whether the pairing dialog is showing, so a paired
    /// device or pairing closed by wrong codes is notified only when it isn't.
    #[zbus(property)]
    fn pairing_dialog_open(&self) -> bool {
        self.st().pairing_dialog
    }
    #[zbus(property)]
    fn set_pairing_dialog_open(&mut self, open: bool) {
        let _ = self.tx.send(Cmd::PairingDialog(open));
    }

    /// The last 30 days: device, name, start, end (0 = now), path, transport, rtt_ms.
    #[zbus(property)]
    fn recent_connections(&self) -> Vec<Dict> {
        self.st()
            .recent
            .into_iter()
            .map(|r| {
                let mut d = Dict::new();
                d.insert("device".into(), ov(r.device));
                d.insert("name".into(), ov(r.name));
                d.insert("start".into(), ov(r.start));
                d.insert("end".into(), ov(r.end));
                d.insert("path".into(), ov(r.path));
                d.insert("transport".into(), ov(r.transport));
                d.insert("rtt_ms".into(), ov(r.rtt_ms));
                d
            })
            .collect()
    }

    /// The relays: host, state ("unknown", "online", "unreachable", "not_enrolled"), rtt_ms, last_ok.
    #[zbus(property)]
    fn relays(&self) -> Vec<Dict> {
        let r = self.st().relay;
        let mut d = Dict::new();
        d.insert("host".into(), ov(r.host));
        d.insert("state".into(), ov(r.state));
        d.insert("rtt_ms".into(), ov(r.rtt_ms));
        d.insert("last_ok".into(), ov(r.last_ok));
        vec![d]
    }

    /// "plugin", "fallback" (ext-image-copy-capture) or "unknown" (SR9).
    #[zbus(property)]
    fn capture(&self) -> String {
        self.st().capture
    }

    /// False when the video encoder failed to open: devices can't connect (SR9).
    #[zbus(property)]
    fn encoder_ready(&self) -> bool {
        self.st().encoder_ready
    }

    // ---- settings ----

    #[zbus(property)]
    fn enabled(&self) -> bool {
        self.st().settings.enabled
    }
    #[zbus(property)]
    async fn set_enabled(&mut self, v: bool) -> fdo::Result<()> {
        self.setting(SettingChange::Enabled(v)).await
    }

    /// What paired devices call this computer: the user's name, or the computer's
    /// own (hostnamed's PrettyHostname, else the hostname) while none is set, and
    /// following it live. Setting "" goes back to following it.
    #[zbus(property)]
    fn name(&self) -> String {
        self.st().display_name
    }
    #[zbus(property)]
    async fn set_name(&mut self, name: String) -> fdo::Result<()> {
        self.ask(|reply| Cmd::SetName { name, reply }).await
    }

    #[zbus(property)]
    fn mdns_visible(&self) -> bool {
        self.st().settings.mdns
    }
    #[zbus(property)]
    async fn set_mdns_visible(&mut self, v: bool) -> fdo::Result<()> {
        self.setting(SettingChange::Mdns(v)).await
    }

    #[zbus(property)]
    fn connect_while_locked(&self) -> bool {
        self.st().settings.connect_while_locked
    }
    #[zbus(property)]
    async fn set_connect_while_locked(&mut self, v: bool) -> fdo::Result<()> {
        self.setting(SettingChange::ConnectWhileLocked(v)).await
    }

    /// The desk monitor's default mode: "mirror", "privacy", "off".
    #[zbus(property)]
    fn home_mode(&self) -> String {
        self.st().settings.home_mode.as_str().into()
    }
    #[zbus(property)]
    async fn set_home_mode(&mut self, v: String) -> fdo::Result<()> {
        let m = HomeMode::parse(&v).ok_or_else(|| fdo::Error::InvalidArgs(format!("no home mode {v}")))?;
        self.setting(SettingChange::HomeMode(m)).await
    }

    #[zbus(property)]
    fn notify(&self) -> bool {
        self.st().settings.notify
    }
    #[zbus(property)]
    async fn set_notify(&mut self, v: bool) -> fdo::Result<()> {
        self.setting(SettingChange::Notify(v)).await
    }

    // ---- actions ----

    /// Ends the session; the device sees "Disconnected by <Name>".
    async fn disconnect(&self) -> fdo::Result<()> {
        self.ask(|reply| Cmd::Disconnect { at_desk: false, reply }).await
    }

    /// The privacy screen accepted the password at the desk; the device sees
    /// "Ended at the computer".
    async fn end_at_desk(&self) -> fdo::Result<()> {
        self.ask(|reply| Cmd::Disconnect { at_desk: true, reply }).await
    }

    /// The desk monitor's mode for this session only.
    async fn set_session_home_mode(&self, mode: String) -> fdo::Result<()> {
        let mode = HomeMode::parse(&mode).ok_or_else(|| fdo::Error::InvalidArgs(format!("no home mode {mode}")))?;
        self.ask(|reply| Cmd::SessionHomeMode { mode, reply }).await
    }

    async fn rename_device(&self, id: String, name: String) -> fdo::Result<()> {
        self.ask(|reply| Cmd::Rename { id, name, reply }).await
    }

    /// Deletes the pinned key; a connected device is dropped at once.
    async fn remove_device(&self, id: String) -> fdo::Result<()> {
        self.ask(|reply| Cmd::Remove { id, reply }).await
    }

    /// Opens a 5-minute pairing window (one use, 3 wrong tries); returns the code
    /// and when it expires. Calling it again is "New code".
    #[zbus(out_args("code", "expires_at"))]
    async fn open_pairing(&self) -> fdo::Result<(String, u64)> {
        self.ask(|reply| Cmd::OpenPairing { reply }).await
    }

    async fn cancel_pairing(&self) {
        let _ = self.tx.send(Cmd::CancelPairing);
    }

    async fn clear_recent(&self) {
        let _ = self.tx.send(Cmd::ClearRecent);
    }
}

/// Serves the API and keeps its properties' change signals in step with the session.
pub async fn serve(conn: &Connection, tx: mpsc::UnboundedSender<Cmd>, status: watch::Receiver<Status>) -> Result<()> {
    conn.object_server().at(PATH, RemoteApi { tx, status: status.clone() }).await?;
    let iface = conn.object_server().interface::<_, RemoteApi>(PATH).await?;
    tokio::spawn(async move {
        let mut rx = status;
        let mut old = rx.borrow().clone();
        while rx.changed().await.is_ok() {
            let new = rx.borrow_and_update().clone();
            let api = iface.get().await;
            let e = iface.signal_emitter();
            if old.state != new.state {
                let _ = api.state_changed(e).await;
            }
            // (DeviceId and DeviceCode never change.)
            if old.session != new.session {
                let _ = api.session_changed(e).await;
            }
            if old.devices != new.devices {
                let _ = api.paired_devices_changed(e).await;
            }
            if old.pairing != new.pairing {
                let _ = api.pairing_changed(e).await;
            }
            if old.pairing_dialog != new.pairing_dialog {
                let _ = api.pairing_dialog_open_changed(e).await;
            }
            if old.recent != new.recent {
                let _ = api.recent_connections_changed(e).await;
            }
            if old.relay != new.relay {
                let _ = api.relays_changed(e).await;
            }
            if old.capture != new.capture {
                let _ = api.capture_changed(e).await;
            }
            if old.encoder_ready != new.encoder_ready {
                let _ = api.encoder_ready_changed(e).await;
            }
            let (a, b) = (&old.settings, &new.settings);
            if a.enabled != b.enabled {
                let _ = api.enabled_changed(e).await;
            }
            if old.display_name != new.display_name {
                let _ = api.name_changed(e).await;
            }
            if a.mdns != b.mdns {
                let _ = api.mdns_visible_changed(e).await;
            }
            if a.connect_while_locked != b.connect_while_locked {
                let _ = api.connect_while_locked_changed(e).await;
            }
            if a.home_mode != b.home_mode {
                let _ = api.home_mode_changed(e).await;
            }
            if a.notify != b.notify {
                let _ = api.notify_changed(e).await;
            }
            drop(api);
            old = new;
        }
    });
    conn.request_name(NAME).await?;
    Ok(())
}

// ---- notifications ----

#[proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;
}

/// Notifications from "Remote" (SB4): normal toasts, and the critical wrong-code one.
pub struct DbusNotifier {
    proxy: NotificationsProxy<'static>,
    ours: Arc<std::sync::Mutex<std::collections::HashSet<u32>>>,
}

impl DbusNotifier {
    pub async fn new(conn: &Connection, tx: mpsc::UnboundedSender<Cmd>) -> Result<Arc<Self>> {
        let proxy = NotificationsProxy::new(conn).await?;
        let ours: Arc<std::sync::Mutex<std::collections::HashSet<u32>>> = Default::default();
        let (p, o) = (proxy.clone(), ours.clone());
        tokio::spawn(async move {
            let Ok(mut s) = p.receive_action_invoked().await else { return };
            while let Some(sig) = s.next().await {
                let Ok(a) = sig.args() else { continue };
                if !o.lock().unwrap().contains(&a.id) {
                    continue;
                }
                match a.action_key.as_str() {
                    "disconnect" => {
                        let (r, _) = oneshot::channel();
                        let _ = tx.send(Cmd::Disconnect { at_desk: false, reply: r });
                    }
                    "settings" => open_uri("settings://remote"),
                    "pairing" => open_uri("settings://remote/pair"),
                    _ => {}
                }
            }
        });
        Ok(Arc::new(DbusNotifier { proxy, ours }))
    }
}

fn open_uri(uri: &str) {
    // Settings' deep links (SR4b). Only ever runs from a notification the user clicked.
    let _ = std::process::Command::new("xdg-open").arg(uri).spawn();
}

impl Notifier for DbusNotifier {
    fn notify(&self, n: Notice) {
        let (p, ours) = (self.proxy.clone(), self.ours.clone());
        tokio::spawn(async move {
            let mut actions = Vec::new();
            for (k, l) in &n.actions {
                actions.push(*k);
                actions.push(*l);
            }
            let mut hints = HashMap::new();
            hints.insert("urgency", Value::U8(if n.critical { 2 } else { 1 }));
            hints.insert("category", Value::from("remote"));
            match p.notify("Remote", 0, "sodiumshell-remote", &n.summary, &n.body, &actions, hints, -1).await {
                Ok(id) => {
                    ours.lock().unwrap().insert(id);
                }
                Err(e) => eprintln!("remoted: notification: {e}"),
            }
        });
    }
}

// ---- logind ----

#[proxy(interface = "org.freedesktop.login1.Manager", default_service = "org.freedesktop.login1", default_path = "/org/freedesktop/login1")]
trait LoginManager {
    fn get_user(&self, uid: u32) -> zbus::Result<OwnedObjectPath>;
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<zbus::zvariant::OwnedFd>;
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

#[proxy(interface = "org.freedesktop.login1.User", default_service = "org.freedesktop.login1")]
trait LoginUser {
    #[zbus(property)]
    fn display(&self) -> zbus::Result<(String, OwnedObjectPath)>;
}

#[proxy(interface = "org.freedesktop.login1.Session", default_service = "org.freedesktop.login1")]
trait LoginSession {
    #[zbus(property)]
    fn locked_hint(&self) -> zbus::Result<bool>;
    /// The session's locker locks (logind emits the session's `Lock` signal; its owner needs no polkit prompt).
    fn lock(&self) -> zbus::Result<()>;
}

/// This user's graphical session in logind: its LockedHint, which the shell's lock sets once every
/// output shows the lock, and Lock(). The session is looked up once, and again after a call fails.
pub struct Logind {
    system: Option<Connection>,
    session: tokio::sync::Mutex<Option<LoginSessionProxy<'static>>>,
}

impl Logind {
    pub fn new(system: Option<Connection>) -> Arc<Self> {
        Arc::new(Logind { system, session: Default::default() })
    }

    async fn session(&self) -> zbus::Result<LoginSessionProxy<'static>> {
        let mut cached = self.session.lock().await;
        if let Some(s) = cached.as_ref() {
            return Ok(s.clone());
        }
        let conn = self.system.as_ref().ok_or_else(|| zbus::Error::Failure("no system bus".into()))?;
        let m = LoginManagerProxy::new(conn).await?;
        let user = m.get_user(rustix::process::getuid().as_raw()).await?;
        let u = LoginUserProxy::builder(conn).path(user)?.build().await?;
        let (_, path) = u.display().await?;
        // LockedHint is read fresh on every poll, not from a cache.
        let s = LoginSessionProxy::builder(conn).path(path)?.cache_properties(zbus::proxy::CacheProperties::No).build().await?;
        *cached = Some(s.clone());
        Ok(s)
    }

    async fn forget(&self) {
        *self.session.lock().await = None;
    }
}

impl crate::session::DeskLock for Logind {
    /// False when unknown (no system bus, no graphical session).
    fn locked(&self) -> futures_util::future::BoxFuture<'_, bool> {
        Box::pin(async move {
            let r = match self.session().await {
                Ok(s) => s.locked_hint().await,
                Err(e) => Err(e),
            };
            match r {
                Ok(v) => v,
                Err(_) => {
                    self.forget().await;
                    false
                }
            }
        })
    }

    fn lock(&self) -> futures_util::future::BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let r = match self.session().await {
                Ok(s) => s.lock().await,
                Err(e) => Err(e),
            };
            if r.is_err() {
                self.forget().await;
            }
            r.map_err(|e| e.to_string())
        })
    }
}

/// The WoL sleep notice (design: "Asleep since 23:10"): a delay inhibitor gives
/// remoted time to tell the rendezvous before the machine sleeps.
pub async fn watch_sleep(system: Connection, tx: mpsc::UnboundedSender<Cmd>) -> Result<()> {
    let m = LoginManagerProxy::new(&system).await?;
    let take = |m: LoginManagerProxy<'static>| async move {
        m.inhibit("sleep", "Remote", "Telling the rendezvous this computer is going to sleep", "delay").await.ok().map(OwnedFd::from)
    };
    let mut lock = take(m.clone()).await;
    let mut s = m.receive_prepare_for_sleep().await?;
    tokio::spawn(async move {
        while let Some(sig) = s.next().await {
            let Ok(a) = sig.args() else { continue };
            let _ = tx.send(Cmd::Sleep(a.start));
            if a.start {
                // The rendezvous client sends the notice; give it a moment, then let go.
                tokio::time::sleep(Duration::from_millis(500)).await;
                lock = None;
            } else if lock.is_none() {
                lock = take(m.clone()).await;
            }
        }
    });
    Ok(())
}

/// The computer's own name changed (`hostnamectl set-hostname`, Settings › About): the
/// session re-reads it and, while no Remote-specific name is set, announces the new one.
pub async fn watch_hostname(system: Connection, tx: mpsc::UnboundedSender<Cmd>) -> Result<()> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.hostname1")?
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .build();
    let mut s = zbus::MessageStream::for_match_rule(rule, &system, None).await?;
    tokio::spawn(async move {
        while s.next().await.is_some() {
            let _ = tx.send(Cmd::Refresh);
        }
    });
    Ok(())
}

// ---- the test-only client stand-in ----

/// `org.sodiumos.Remote.Dev` (only with `remoted run --dev`): plays a client
/// connection the way core's session will, and records what each device hears.
pub struct DevApi {
    pub tx: mpsc::UnboundedSender<Cmd>,
    pub links: Links,
}

#[interface(name = "org.sodiumos.Remote.Dev")]
impl DevApi {
    #[allow(clippy::too_many_arguments)]
    async fn connect(&self, device: String, width: u32, height: u32, refresh_mhz: u32, ui: String, ui_ratio: f64, takeover: bool) -> fdo::Result<()> {
        let ui = match ui.as_str() {
            "larger" => UiSize::Larger,
            "smaller" => UiSize::Smaller,
            "desk" => UiSize::Desk,
            _ => UiSize::Match,
        };
        let link = Arc::new(RecordingLink::default());
        self.links.lock().unwrap().insert(device.clone(), link.clone());
        let display = Request { width, height, refresh_mhz, ui, ui_ratio, display_width: 0, display_height: 0 };
        self.tx.send(Cmd::Connect(ConnectRequest { device, display, takeover, link, route: None })).map_err(|e| failed(e.to_string()))
    }

    async fn lose(&self, device: String) {
        let _ = self.tx.send(Cmd::Lost { device });
    }

    async fn bye(&self, device: String) {
        let _ = self.tx.send(Cmd::Bye { device });
    }

    async fn resize(&self, device: String, width: u32, height: u32, refresh_mhz: u32, ui_ratio: f64) {
        let display = Request { width, height, refresh_mhz, ui: UiSize::Match, ui_ratio, display_width: 0, display_height: 0 };
        let _ = self.tx.send(Cmd::Resize { device, display });
    }

    async fn preedit(&self, device: String, text: String, cursor: i32) {
        let _ = self.tx.send(Cmd::Preedit { device, text, cursor });
    }

    async fn commit(&self, device: String, text: String) {
        let _ = self.tx.send(Cmd::Commit { device, text });
    }

    async fn clipboard(&self, device: String, text: String, concealed: bool) {
        let entry = ClipEntry { items: vec![("text/plain;charset=utf-8".into(), text.into_bytes())], concealed };
        let _ = self.tx.send(Cmd::Clipboard { device, entry });
    }

    async fn mic(&self, device: String, on: bool) {
        let _ = self.tx.send(Cmd::MicOn { device, on });
    }

    async fn home_mode(&self, device: String, mode: String) -> fdo::Result<()> {
        let mode = HomeMode::parse(&mode).ok_or_else(|| fdo::Error::InvalidArgs(mode))?;
        self.tx.send(Cmd::ClientHomeMode { device, mode }).map_err(|e| failed(e.to_string()))
    }

    /// Stands in for a SPAKE2 run: true when `code` is the open window's code.
    async fn pair(&self, code: String, device: String, name: String) -> fdo::Result<bool> {
        let (r, w) = oneshot::channel();
        self.tx
            .send(Cmd::PairAttempt { code, device, name, kind: "mac".into(), reply: Some(r) })
            .map_err(|e| failed(e.to_string()))?;
        w.await.map_err(|e| failed(e.to_string()))
    }

    /// Stands in for core's PairingAttempt: a device started (or, `false`, gave up) a SPAKE2 run against the open
    /// window, which Settings shows as "A device is pairing…".
    async fn pairing_attempt(&self, on: bool) {
        let _ = self.tx.send(Cmd::PairingAttempt(on));
    }

    /// Stands in for the transport's measurements of `device`'s session (the path, RTT and rates Settings and the pill show).
    async fn stats(&self, device: String, path: String, transport: String, rtt_ms: u32, bitrate_kbps: u32, fps: f64) {
        let stats = crate::session::LinkStats { path, transport, network: String::new(), rtt_ms, bitrate_kbps, fps };
        let _ = self.tx.send(Cmd::Stats { device, stats });
    }

    /// Stands in for the media module's report: "plugin" / "fallback" / "unknown", and
    /// whether the video encoder opened.
    async fn media(&self, capture: String, encoder_ready: bool) {
        let _ = self.tx.send(Cmd::Media { capture, encoder_ready });
    }

    /// Stands in for the rendezvous' state: "unknown", "online", "unreachable", "not_enrolled".
    async fn relay(&self, state: String, rtt_ms: u32) {
        let last_ok = if state == "online" { crate::session::unix_now() } else { 0 };
        let _ = self.tx.send(Cmd::DevRelay(crate::session::RelayView { host: String::new(), state, rtt_ms, last_ok }));
    }

    /// What `device` has heard, one Debug line per event (clipboard content and
    /// audio left out), and how many audio packets.
    async fn events(&self, device: String) -> (Vec<String>, u64) {
        let Some(l) = self.links.lock().unwrap().get(&device).cloned() else { return (vec![], 0) };
        let evs = l.events.lock().unwrap().iter().map(|e| format!("{e:?}")).collect();
        (evs, l.audio_packets.load(std::sync::atomic::Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xml_of(i: &dyn zbus::object_server::Interface) -> String {
        let mut out = String::new();
        i.introspect_to_writer(&mut out, 0);
        out
    }

    /// The introspection XML is the contract Settings, the shell and anything else
    /// on the bus build against: it is checked in, and changes to it only add.
    /// `UPDATE_DBUS_XML=1 cargo test -p remoted dbus_introspection` rewrites the files.
    fn check(file: &str, live: &str) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dbus").join(file);
        if std::env::var_os("UPDATE_DBUS_XML").is_some() {
            std::fs::write(&path, live).unwrap();
        }
        let checked_in = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(checked_in, live, "{file} differs from the live interface; if the change only adds, run with UPDATE_DBUS_XML=1");
    }

    fn blank_status() -> crate::session::Status {
        crate::session::Status {
            settings: crate::session::store::Settings::default(),
            display_name: String::new(),
            device_id: String::new(),
            state: "off",
            session: None,
            devices: vec![],
            pairing: None,
            pairing_dialog: false,
            recent: vec![],
            relay: Default::default(),
            capture: String::new(),
            encoder_ready: false,
        }
    }

    #[test]
    fn dbus_introspection_matches_the_checked_in_xml() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (_wtx, status) = watch::channel(blank_status());
        let api = RemoteApi { tx: tx.clone(), status };
        assert_eq!(api.api_version(), API_VERSION);
        check("org.sodiumos.Remote.xml", &xml_of(&api));
        check("org.sodiumos.Remote.Dev.xml", &xml_of(&DevApi { tx, links: Default::default() }));
    }
}
