// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The UniFFI surface for Swift: the Mac client's side of `connect`
//! (the Rust core via UniFFI). Built with the `ffi` feature;
//! `build-swift.sh` generates `RemoteCore.swift` and the C header.
//!
//! - `RemoteClient` owns a tokio runtime and a `connect::Client`. Its methods
//!   never block the caller (async ones are Swift `async`).
//! - Events come through `ClientListener.onEvent`, called on a Rust thread:
//!   hop to your own queue. Video frames come the same way.
//! - Storage is `KeyStore`, implemented in Swift: `device.key` in the
//!   Keychain, `paired.json` anywhere.
//! - The schemas (`Msg`, `InputEvent`, `Hello`, …) are `msg`'s own types,
//!   generated as Swift enums and structs.

use crate::connect::{self, Client, ClientConfig, ClientEvent, ConnectError, PairError, PathKind, RouteInfo};
use crate::health::LinkHealth;
use crate::keys::{self, DeviceId, PairedDevice};
use crate::msg::{CursorImage, DisconnectReason, Hello, InputEvent, KeyRule, Msg, VsyncReport};
use crate::path::TurnTransport;
use crate::rendezvous::RendezvousConfig;
use crate::session::SessionStats;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Storage the app provides (the Keychain for `device.key`).
#[uniffi::export(with_foreign)]
pub trait KeyStore: Send + Sync {
    fn load(&self, name: String) -> Option<Vec<u8>>;
    /// False if it couldn't be saved.
    fn save(&self, name: String, data: Vec<u8>) -> bool;
    fn remove(&self, name: String);
}

struct StoreAdapter(Arc<dyn KeyStore>);

impl keys::Store for StoreAdapter {
    fn load(&self, name: &str) -> Option<Vec<u8>> {
        self.0.load(name.to_string())
    }
    fn save(&self, name: &str, data: &[u8]) -> std::io::Result<()> {
        if self.0.save(name.to_string(), data.to_vec()) { Ok(()) } else { Err(std::io::Error::other("KeyStore.save failed")) }
    }
    fn remove(&self, name: &str) -> std::io::Result<()> {
        self.0.remove(name.to_string());
        Ok(())
    }
}

/// Receives every client event, on a Rust thread.
#[uniffi::export(with_foreign)]
pub trait ClientListener: Send + Sync {
    fn on_event(&self, event: RemoteEvent);
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ClientSettings {
    /// This Mac's display name ("MacBook Air").
    pub name: String,
    /// Use the rendezvous server below. Off, or no URL: LAN and address only.
    pub rendezvous: bool,
    /// The server (the app's Settings › Server, or an imported link): `wss://…`.
    /// Empty: none configured (nothing is compiled in).
    pub rendezvous_url: String,
    /// PEM of a CA to pin for it (empty: Web PKI).
    #[uniffi(default = "")]
    pub rendezvous_ca_pem: String,
    /// The enrolment token, sent when this device registers (empty: none).
    #[uniffi(default = "")]
    pub rendezvous_token: String,
    /// Browse mDNS for hosts on the LAN (needs the Local Network permission).
    pub mdns: bool,
    pub fps: u32,
    /// Known LAN listeners, each `ID@IP:PORT` (`ClientConfig::lan_hints`), for
    /// when mDNS can't be used on the Mac. Malformed entries are skipped.
    pub lan_hints: Vec<String>,
    /// Share only relayed candidates (`ClientConfig::relay_only`): Settings ›
    /// Advanced › Relay "Always through the relay". The start value;
    /// `RemoteClient.set_relay_only` changes it later.
    pub relay_only: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct Device {
    /// The device ID (base64url).
    pub id: String,
    pub name: String,
    pub paired_at: u64,
    pub last_connected: Option<u64>,
}

impl From<PairedDevice> for Device {
    fn from(d: PairedDevice) -> Self {
        Device { id: d.id.to_string(), name: d.name, paired_at: d.paired_at, last_connected: d.last_connected }
    }
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PairingHost {
    pub host: String,
    pub name: String,
    /// "lan", "rendezvous" or "both".
    pub found_via: String,
    /// Opaque: pass it back to `pair`.
    pub handle: u64,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct HostPresence {
    pub host: String,
    pub online: bool,
    pub on_lan: bool,
    pub last_seen: Option<u64>,
    /// Unix seconds; set while the host is asleep.
    pub asleep_since: Option<u64>,
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum Path {
    Lan,
    Direct,
    Vpn,
    Tailscale,
    RelayUdp,
    RelayTcp,
    RelayTls,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct Route {
    pub path: Path,
    pub local: String,
    pub remote: String,
    pub rtt_ms: f64,
    pub interface: Option<String>,
}

impl From<RouteInfo> for Route {
    fn from(r: RouteInfo) -> Self {
        Route {
            path: match r.kind {
                PathKind::Lan => Path::Lan,
                PathKind::Direct => Path::Direct,
                PathKind::Vpn => Path::Vpn,
                PathKind::Tailscale => Path::Tailscale,
                PathKind::Relay { transport: TurnTransport::Udp } => Path::RelayUdp,
                PathKind::Relay { transport: TurnTransport::Tcp } => Path::RelayTcp,
                PathKind::Relay { transport: TurnTransport::Tls } => Path::RelayTls,
            },
            local: r.local.to_string(),
            remote: r.remote.to_string(),
            rtt_ms: r.rtt_ms,
            interface: r.interface,
        }
    }
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct VideoFrame {
    pub frame_id: u32,
    pub keyframe: bool,
    /// The host's send time on the host's session clock (µs): against
    /// `send_local_us` it maps the frame meta's host stamps onto ours.
    pub send_us: u64,
    /// The host's send time on our clock (µs), once clock sync has a sample.
    pub send_local_us: Option<i64>,
    pub first_arrival_us: u64,
    pub last_arrival_us: u64,
    /// Predicted from this frame (a recovery after a loss).
    pub reference: Option<u32>,
    /// Everything it depends on arrived; false until a recovery point.
    pub decodable: bool,
    pub meta: Vec<u8>,
    pub data: Vec<u8>,
}

#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct LinkStats {
    pub rtt_ms: f64,
    pub loss: f64,
    pub target_bps: u64,
    pub receive_rate_bps: f64,
    pub fec_ratio: f32,
    pub frames_received: u64,
    pub frames_lost: u64,
    pub mtu: u64,
    pub hedging: bool,
    /// Bytes received on the session so far, every channel (idle bandwidth).
    pub bytes_received: u64,
}

impl From<SessionStats> for LinkStats {
    fn from(s: SessionStats) -> Self {
        LinkStats {
            rtt_ms: s.rtt_ms,
            loss: s.loss,
            target_bps: s.target_bps,
            receive_rate_bps: s.receive_rate_bps,
            fec_ratio: s.fec_ratio,
            frames_received: s.frames_received,
            frames_lost: s.frames_lost,
            mtu: s.mtu as u64,
            hedging: s.hedging,
            bytes_received: s.bytes_received,
        }
    }
}

/// The live connection's health, for the path chip and the pill.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct LinkInfo {
    /// The route media takes now (None: no live route).
    pub route: Option<Route>,
    /// The route duplicates go on while hedging.
    pub hedge_route: Option<Route>,
    pub hedging: bool,
    /// The active route answers (not suspect or dead).
    pub healthy: bool,
    /// Since anything authenticated arrived from the host: t₀ of the timeline.
    pub silence_ms: u64,
    /// How long there has been no live route at all.
    pub no_route_ms: Option<u64>,
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum RemoteEvent {
    Rendezvous { online: bool, error: Option<String> },
    /// The handshake finished; Hello is out, Welcome follows as `Control`.
    Connected { route: Option<Route> },
    Control { msg: Msg },
    VideoFrame { frame: VideoFrame },
    VideoFrameLost { frame_id: u32 },
    CursorShape { hash: u64, image: CursorImage },
    CursorPosition { x: f32, y: f32, visible: bool, shape: u64 },
    RouteChanged { route: Route },
    Stats { stats: LinkStats },
    /// Retrying; `since_ms` picks the wording.
    Reconnecting { since_ms: u64 },
    Disconnected { reason: DisconnectReason },
    Presence { host: String, online: bool, last_seen: Option<u64>, asleep_since: Option<u64> },
    Revoked { host: String },
}

impl From<ClientEvent> for RemoteEvent {
    fn from(e: ClientEvent) -> Self {
        match e {
            ClientEvent::Rendezvous { online, error } => RemoteEvent::Rendezvous { online, error },
            ClientEvent::Connected { route } => RemoteEvent::Connected { route: route.map(Route::from) },
            ClientEvent::Control(msg) => RemoteEvent::Control { msg },
            ClientEvent::VideoFrame(f) => RemoteEvent::VideoFrame {
                frame: VideoFrame {
                    frame_id: f.frame_id,
                    keyframe: f.keyframe,
                    send_us: f.send_us,
                    send_local_us: f.send_local_us,
                    first_arrival_us: f.first_arrival_us,
                    last_arrival_us: f.last_arrival_us,
                    reference: f.reference,
                    decodable: f.decodable,
                    meta: f.meta,
                    data: f.data,
                },
            },
            ClientEvent::VideoFrameLost { frame_id } => RemoteEvent::VideoFrameLost { frame_id },
            ClientEvent::CursorShape { hash, image } => RemoteEvent::CursorShape { hash, image },
            ClientEvent::CursorPosition { x, y, visible, shape } => RemoteEvent::CursorPosition { x, y, visible, shape },
            ClientEvent::Route(r) => RemoteEvent::RouteChanged { route: r.into() },
            ClientEvent::Stats(s) => RemoteEvent::Stats { stats: s.into() },
            ClientEvent::Reconnecting { since } => RemoteEvent::Reconnecting { since_ms: since.as_millis() as u64 },
            ClientEvent::Disconnected { reason } => RemoteEvent::Disconnected { reason },
            ClientEvent::Presence { host, online, last_seen, asleep_since } => {
                RemoteEvent::Presence { host: host.to_string(), online, last_seen, asleep_since }
            }
            ClientEvent::Revoked { host } => RemoteEvent::Revoked { host: host.to_string() },
        }
    }
}

#[derive(uniffi::Error, Debug, Clone, PartialEq)]
pub enum RemoteError {
    /// Another device is connected right now: ask "In use by <by>. Take over?"
    Busy { by: String },
    Offline,
    NotPaired,
    /// The host refused (version mismatch, …).
    Refused { reason: DisconnectReason },
    WrongCode,
    /// The pairing window is gone.
    PairingClosed { reason: String },
    Timeout,
    Failed { message: String },
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl From<ConnectError> for RemoteError {
    fn from(e: ConnectError) -> Self {
        match e {
            ConnectError::Busy { by } => RemoteError::Busy { by },
            ConnectError::Offline => RemoteError::Offline,
            ConnectError::NotPaired => RemoteError::NotPaired,
            ConnectError::Refused(reason) => RemoteError::Refused { reason },
            ConnectError::Timeout => RemoteError::Timeout,
            ConnectError::Failed(message) => RemoteError::Failed { message },
        }
    }
}

impl From<PairError> for RemoteError {
    fn from(e: PairError) -> Self {
        match e {
            PairError::WrongCode => RemoteError::WrongCode,
            PairError::Closed(reason) => RemoteError::PairingClosed { reason },
            PairError::Failed(message) => RemoteError::Failed { message },
        }
    }
}

#[derive(uniffi::Enum, Clone, Debug)]
pub enum AudioPlay {
    /// Decode this Opus packet.
    Packet { opus: Vec<u8> },
    /// This 10 ms was lost: decode `opus` (the next packet) with FEC on.
    Fec { opus: Vec<u8> },
    /// Nothing usable: run packet loss concealment.
    Conceal,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct AudioFrame {
    pub timestamp: u32,
    pub play: AudioPlay,
    pub capture_local_us: Option<i64>,
}

/// The Mac client.
#[derive(uniffi::Object)]
pub struct RemoteClient {
    rt: tokio::runtime::Runtime,
    client: Client,
    pairing: std::sync::Mutex<Vec<connect::PairingTarget>>,
    /// Each route's kind, once read (`route_info` reads the interfaces), so
    /// `link()` can be polled every 100 ms.
    kinds: Mutex<Vec<(SocketAddr, SocketAddr, Route)>>,
}

fn parse_id(s: &str) -> Result<DeviceId, RemoteError> {
    s.parse().map_err(|e: String| RemoteError::Failed { message: e })
}

/// `ID@IP:PORT`.
fn parse_hint(s: &str) -> Option<(DeviceId, SocketAddr)> {
    let (id, addr) = s.trim().split_once('@')?;
    Some((id.parse().ok()?, addr.parse().ok()?))
}

/// The schema version a `Hello` carries (`msg::VERSION`).
#[uniffi::export]
pub fn protocol_version() -> u16 {
    crate::msg::VERSION
}

/// The oldest schema version this build speaks (`msg::MIN_VERSION`): a `Hello`'s `min_version`.
#[uniffi::export]
pub fn protocol_min_version() -> u16 {
    crate::msg::MIN_VERSION
}

/// For a `VersionMismatch` refusal: which side needs the newer release.
#[uniffi::export]
pub fn version_mismatch_update_side(host: u16, client: u16) -> crate::msg::UpdateSide {
    crate::msg::DisconnectReason::VersionMismatch { host, client }.update_side().expect("a version mismatch")
}

/// The HID usage reserved for Globe (fn), `msg::USAGE_GLOBE`.
#[uniffi::export]
pub fn usage_globe() -> u16 {
    crate::msg::USAGE_GLOBE
}

/// the "Mac behaviour" key table (`KeyRule::mac_defaults`): the
/// client's default, editable in Settings › Input.
#[uniffi::export]
pub fn mac_default_key_table() -> Vec<KeyRule> {
    KeyRule::mac_defaults()
}

/// A device's code, `XXXX-XXXX`; empty for a malformed ID.
#[uniffi::export]
pub fn device_code(id: String) -> String {
    id.parse::<DeviceId>().map(|d| d.code()).unwrap_or_default()
}

/// Does `typed` name this device (its full ID, or its code in any case, with
/// or without the hyphen)?
#[uniffi::export]
pub fn device_matches(id: String, typed: String) -> bool {
    match id.parse::<DeviceId>() {
        Ok(d) => typed.trim() == id || d.code_matches(&typed),
        Err(_) => false,
    }
}

/// An IME commit split into events under `INPUT_TEXT_MAX` bytes, on
/// character boundaries.
#[uniffi::export]
pub fn ime_commit_chunks(text: String) -> Vec<InputEvent> {
    InputEvent::commit_chunks(&text)
}

/// The client side of the phase lock (`phase::LeadStats`):
/// add every frame shown, send `report()` as `Msg::Vsync` about once a second.
/// On the Mac `presented_us` is the display link update's `targetTimestamp`,
/// the drawing deadline (integration.md 9), on the session clock.
#[derive(uniffi::Object, Default)]
pub struct LeadStats {
    inner: Mutex<crate::phase::LeadStats>,
}

#[uniffi::export]
impl LeadStats {
    #[uniffi::constructor]
    pub fn new() -> Arc<LeadStats> {
        Arc::new(LeadStats::default())
    }

    /// A frame ready at `ready_us` was shown for the deadline `presented_us`
    /// (both session-clock µs) on a display with this refresh period.
    pub fn add(&self, ready_us: i64, presented_us: i64, period_ns: u32) {
        self.inner.lock().unwrap().add(ready_us, presented_us, period_ns);
    }

    /// The report for the frames since the last one; None until 10 frames.
    pub fn report(&self) -> Option<VsyncReport> {
        self.inner.lock().unwrap().report()
    }

    /// Start over (a reconnect: a new session clock).
    pub fn reset(&self) {
        *self.inner.lock().unwrap() = crate::phase::LeadStats::default();
    }
}

#[uniffi::export]
impl RemoteClient {
    /// Start: loads (or creates) the device key from `store`, connects to the
    /// rendezvous, starts mDNS. Events go to `listener` from now on.
    #[uniffi::constructor]
    pub fn new(
        settings: ClientSettings,
        store: Arc<dyn KeyStore>,
        listener: Arc<dyn ClientListener>,
    ) -> Result<Arc<RemoteClient>, RemoteError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("remote-core")
            .enable_all()
            .build()
            .map_err(|e| RemoteError::Failed { message: e.to_string() })?;
        let mut cfg = ClientConfig::new(Arc::new(StoreAdapter(store)), settings.name);
        cfg.rendezvous = (settings.rendezvous && !settings.rendezvous_url.is_empty()).then(|| RendezvousConfig {
            url: settings.rendezvous_url.clone(),
            ca_pem: (!settings.rendezvous_ca_pem.is_empty()).then(|| settings.rendezvous_ca_pem.clone().into_bytes()),
            token: (!settings.rendezvous_token.is_empty()).then(|| settings.rendezvous_token.clone()),
        });
        cfg.mdns = settings.mdns;
        if settings.fps > 0 {
            cfg.fps = settings.fps;
        }
        cfg.lan_hints = settings.lan_hints.iter().filter_map(|h| parse_hint(h)).collect();
        cfg.relay_only = settings.relay_only;
        let (client, mut events) =
            rt.block_on(Client::start(cfg)).map_err(|e| RemoteError::Failed { message: e.to_string() })?;
        rt.spawn(async move {
            while let Some(e) = events.recv().await {
                listener.on_event(e.into());
            }
        });
        Ok(Arc::new(RemoteClient { rt, client, pairing: Default::default(), kinds: Default::default() }))
    }

    /// This Mac's device ID.
    pub fn id(&self) -> String {
        self.client.id().to_string()
    }

    /// The paired computers.
    pub fn paired(&self) -> Vec<Device> {
        self.client.paired().into_iter().map(Device::from).collect()
    }

    /// This Mac's display name changed (System Settings › Sharing): pairing,
    /// connection offers and `Hello` carry the new one from now on.
    pub fn set_name(&self, name: String) {
        self.client.set_name(&name);
    }

    /// Settings › Advanced › Relay: share only relayed candidates (`true`) or
    /// all of them. Applies from the next connection, not the current one.
    pub fn set_relay_only(&self, on: bool) {
        self.client.set_relay_only(on);
    }

    /// "Forget this computer".
    pub fn forget(&self, host: String) -> Result<(), RemoteError> {
        self.client.forget(&parse_id(&host)?).map_err(|e| RemoteError::Failed { message: e.to_string() })
    }

    /// Computers with an open pairing window (normally one), from mDNS and
    /// the rendezvous.
    pub async fn discover_pairing(&self, timeout_ms: u64) -> Vec<PairingHost> {
        let c = self.client.clone();
        let found = self
            .rt
            .spawn(async move { c.discover_pairing(Duration::from_millis(timeout_ms)).await })
            .await
            .unwrap_or_default();
        let mut keep = self.pairing.lock().unwrap();
        *keep = found.clone();
        found
            .into_iter()
            .enumerate()
            .map(|(i, t)| PairingHost {
                host: t.host.to_string(),
                name: t.name,
                found_via: match (t.lan.is_some(), t.rendezvous.is_some()) {
                    (true, true) => "both",
                    (true, false) => "lan",
                    _ => "rendezvous",
                }
                .into(),
                handle: i as u64,
            })
            .collect()
    }

    /// SPAKE2 with the typed code. On success both sides have pinned keys.
    pub async fn pair(&self, host: PairingHost, code: String) -> Result<Device, RemoteError> {
        let target = self
            .pairing
            .lock()
            .unwrap()
            .get(host.handle as usize)
            .cloned()
            .ok_or(RemoteError::PairingClosed { reason: "not found".into() })?;
        let c = self.client.clone();
        self.rt
            .spawn(async move { c.pair(&target, &code).await })
            .await
            .map_err(|e| RemoteError::Failed { message: e.to_string() })?
            .map(Device::from)
            .map_err(RemoteError::from)
    }

    /// Online, on the LAN, or last seen when.
    pub async fn presence(&self) -> Vec<HostPresence> {
        let c = self.client.clone();
        self.rt
            .spawn(async move { c.presence().await })
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| HostPresence {
                host: s.host.to_string(),
                online: s.online,
                on_lan: s.on_lan,
                last_seen: s.last_seen,
                asleep_since: s.asleep_since,
            })
            .collect()
    }

    /// Connect. `Busy` means ask "In use by X. Take over?" and call again with
    /// `takeover`. The core sends `hello` on every (re)connect; keep it current
    /// with `send_control(Resize/Settings)`.
    pub async fn connect(&self, host: String, hello: Hello, takeover: bool) -> Result<(), RemoteError> {
        let id = parse_id(&host)?;
        let c = self.client.clone();
        self.rt
            .spawn(async move { c.connect(id, hello, takeover).await })
            .await
            .map_err(|e| RemoteError::Failed { message: e.to_string() })?
            .map_err(RemoteError::from)
    }

    pub fn send_control(&self, msg: Msg) {
        self.client.send(msg);
    }

    /// False when not connected.
    pub fn send_input(&self, event: InputEvent) -> bool {
        self.client.send_input(&event)
    }

    /// An event that happened `age_us` ago (NSEvent.timestamp against
    /// ProcessInfo.systemUptime), so the host keeps the user's spacing.
    pub fn send_input_at(&self, event: InputEvent, age_us: u64) -> bool {
        let at = std::time::Instant::now().checked_sub(std::time::Duration::from_micros(age_us)).unwrap_or_else(std::time::Instant::now);
        self.client.send_input_at(&event, at)
    }

    /// After each frame: decode time and present time (µs).
    pub fn send_receiver_report(&self, frame_id: u32, decode_us: u32, present_us: u32) {
        self.client.send_receiver_report(frame_id, decode_us, present_us);
    }

    /// The microphone: one Opus packet.
    pub fn send_mic(&self, timestamp: u32, opus: Vec<u8>) {
        self.client.send_mic(timestamp, None, opus);
    }

    /// The audio callback: the next 10 ms of desktop audio, if any.
    pub fn pop_audio(&self) -> Option<AudioFrame> {
        let buf = self.client.audio_buffer()?;
        let out = buf.lock().unwrap().pop(Instant::now())?;
        Some(AudioFrame {
            timestamp: out.timestamp,
            play: match out.play {
                crate::audio::AudioPlay::Packet(opus) => AudioPlay::Packet { opus },
                crate::audio::AudioPlay::Fec(opus) => AudioPlay::Fec { opus },
                crate::audio::AudioPlay::Conceal => AudioPlay::Conceal,
            },
            capture_local_us: out.capture_local_us,
        })
    }

    /// NWPathMonitor saw a change: re-gather candidates now.
    pub fn network_changed(&self) {
        self.client.network_changed();
    }

    /// The route media takes now, and the hedge route while one is in use
    /// ("Reconnecting… · relay SG").
    pub fn routes(&self) -> Vec<Route> {
        let Some(s) = self.client.link_status() else { return vec![] };
        [s.route, s.hedge_route].into_iter().flatten().map(|r| connect::route_info(&r).into()).collect()
    }

    /// The live connection's health and routes (None: not connected), for
    /// the timeline; cheap enough to poll every 100 ms.
    pub fn link(&self) -> Option<LinkInfo> {
        let s = self.client.link_status()?;
        let mut kinds = self.kinds.lock().unwrap();
        let mut named = |r: &Option<crate::path::Route>| -> Option<Route> {
            let r = r.as_ref()?;
            if let Some((_, _, k)) = kinds.iter().find(|(l, rm, _)| *l == r.local && *rm == r.remote) {
                let mut k = k.clone();
                k.rtt_ms = r.rtt.as_secs_f64() * 1e3;
                return Some(k);
            }
            let k: Route = connect::route_info(r).into();
            kinds.push((r.local, r.remote, k.clone()));
            if kinds.len() > 32 {
                kinds.remove(0);
            }
            Some(k)
        };
        Some(LinkInfo {
            route: named(&s.route),
            hedge_route: named(&s.hedge_route),
            hedging: s.health.hedging,
            healthy: s.health.state == LinkHealth::Healthy,
            silence_ms: s.health.silence_ms,
            no_route_ms: s.no_route_ms,
        })
    }

    /// Now on the live session's clock (µs; frames' times use it). None when
    /// not connected. The Mac maps `CACurrentMediaTime` onto it with one read.
    pub fn session_clock_us(&self) -> Option<u64> {
        self.client.session_epoch().map(|e| e.elapsed().as_micros() as u64)
    }

    pub fn disconnect(&self) {
        self.client.disconnect();
    }

    /// Stop everything (the app is quitting).
    pub fn shutdown(&self) {
        let c = self.client.clone();
        self.rt.block_on(async move { c.shutdown().await });
    }
}

// --- Server links: the Mac keeps url, CA and token in its
// preferences; links and their QR codes import and export them.

/// A `sodium-remote://server?...` link, parsed.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ServerLinkInfo {
    pub url: String,
    pub host: String,
    /// "Sodium Remote service" for the hosted service, else the host name.
    pub label: String,
    /// The hosted service (`brand::OFFICIAL_SERVER_HOST`): trusted through Web PKI, no prompt.
    pub official: bool,
    /// The CA the link pins, as PEM ("" for none). Ask before trusting it.
    pub ca_pem: String,
    pub token: String,
}

fn link_error(message: String) -> RemoteError {
    RemoteError::Failed { message }
}

/// Parse a server link (or a QR code's text).
#[uniffi::export]
pub fn parse_server_link(link: String) -> Result<ServerLinkInfo, RemoteError> {
    crate::server_config::ServerLink::parse(&link).map(link_info).map_err(link_error)
}

/// The link for these settings (export and QR). `ca_pem` and `token` may be empty.
#[uniffi::export]
pub fn server_link(url: String, ca_pem: String, token: String) -> Result<String, RemoteError> {
    let ca_der = if ca_pem.is_empty() { None } else { Some(crate::server_config::der_from_pem(ca_pem.as_bytes()).map_err(link_error)?) };
    let l = crate::server_config::ServerLink { url, ca_der, token: (!token.is_empty()).then_some(token) };
    // Validate the way an importer will.
    crate::server_config::ServerLink::parse(&l.to_link()).map_err(link_error)?;
    Ok(l.to_link())
}

/// The hosted service's host name; nothing dials it unless configured.
#[uniffi::export]
pub fn official_server_host() -> String {
    crate::brand::OFFICIAL_SERVER_HOST.to_string()
}

fn link_info(l: crate::server_config::ServerLink) -> ServerLinkInfo {
    let host = l.host();
    let official = l.is_official();
    ServerLinkInfo {
        label: if official { crate::brand::OFFICIAL_SERVICE_NAME.to_string() } else { host.clone() },
        url: l.url,
        host,
        official,
        ca_pem: l.ca_der.as_deref().map(crate::server_config::pem_from_der).unwrap_or_default(),
        token: l.token.unwrap_or_default(),
    }
}

/// Check settings typed by hand (URL, CA as PEM or "", token or "") and say what they name. Errors say why.
#[uniffi::export]
pub fn check_server_settings(url: String, ca_pem: String, token: String) -> Result<ServerLinkInfo, RemoteError> {
    crate::server_config::check_settings(&url, &ca_pem, &token).map(link_info).map_err(link_error)
}

/// Read a `server.toml`'s text (`base_dir`: its folder, where a relative `ca_file` is looked for; the CA comes
/// back as PEM). `None`: the file configures no server.
#[uniffi::export]
pub fn parse_server_toml(text: String, base_dir: String) -> Result<Option<ServerLinkInfo>, RemoteError> {
    let Some(c) = crate::server_config::ServerConfig::parse(&text, std::path::Path::new(&base_dir)).map_err(link_error)? else {
        return Ok(None);
    };
    let official = c.is_official();
    let ca_pem = match (&c.ca_file, official) {
        (Some(_), false) => String::from_utf8_lossy(&c.rendezvous().map_err(link_error)?.ca_pem.unwrap_or_default()).into_owned(),
        _ => String::new(),
    };
    Ok(Some(ServerLinkInfo { label: c.label(), host: c.host(), official, ca_pem, token: c.token.unwrap_or_default(), url: c.url }))
}

/// The text of a `server.toml` for these settings (the Mac's Export…). `ca_file` is the CA file's name beside
/// it ("" for none).
#[uniffi::export]
pub fn server_toml(url: String, ca_file: String, token: String) -> String {
    crate::server_config::ServerConfig {
        url,
        ca_file: (!ca_file.is_empty()).then(|| std::path::PathBuf::from(ca_file)),
        token: (!token.is_empty()).then_some(token),
    }
    .to_toml()
}

#[cfg(test)]
mod server_settings_tests {
    use super::*;

    #[test]
    fn the_macs_export_and_import_agree_and_a_hosted_service_pins_nothing() {
        let dir = std::env::temp_dir().join(format!("remote-ffi-toml-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ca = crate::server_config::pem_from_der(&[1, 2, 3]);
        std::fs::write(dir.join("server-ca.pem"), &ca).unwrap();
        let text = server_toml("wss://relay.example.com/remote".into(), "server-ca.pem".into(), "tok".into());
        let back = parse_server_toml(text, dir.to_string_lossy().into_owned()).unwrap().unwrap();
        assert_eq!((back.host.as_str(), back.ca_pem.as_str(), back.token.as_str(), back.official), ("relay.example.com", ca.as_str(), "tok", false));
        assert_eq!(parse_server_toml("# nothing\n".into(), "/".into()).unwrap(), None);
        let o = check_server_settings(format!("wss://{}/remote", official_server_host()), "".into(), "".into()).unwrap();
        assert_eq!((o.official, o.label.as_str()), (true, "Sodium Remote service"));
        assert!(check_server_settings("http://x".into(), "".into(), "".into()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
