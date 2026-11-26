// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The session API: one place where a host or a client discovers, pairs,
//! connects (signalling, ICE with TURN credentials from the rendezvous, Noise
//! with pinned keys), and then exchanges video, input and control messages.
//! The takeover and hold rules are.2 and
//! `docs/reference/session-api.md` is the guide.
//!
//! Shape: `Host::start` / `Client::start` return a handle (cheap methods,
//! never blocking) and an event receiver. Inside, one "brain" task owns the
//! session state; the rendezvous, the LAN listener and every connection's
//! notices feed it. Hot paths (video, cursor, input) go straight to the
//! current connection's driver.

use crate::alloc::FrameOpts;
use crate::audio::JitterBuffer;
use crate::driver::{Command, EndpointConfig, EndpointHandle, LinkStatus, Notice, spawn_endpoint};
use crate::keys::{self, DeviceId, DeviceKey, PairedDevice, Pairings, Store, unix_now};
use crate::lanlimit;
use crate::mdns::{self, Advertiser, Browser, LanFrame, LanHost};
use crate::msg::{self, CursorImage, DisconnectReason, Hello, InputEvent, Msg, Signal};
use crate::pairing::{self, ClientAttempt, HostWindow, Step};
use crate::path::{PathConfig, Route, RouteKind, TurnTransport};
use crate::rendezvous::{self, DeviceRole, HostState, RdvError, Rendezvous, RendezvousConfig};
use crate::session::{self, OutgoingFrame, Peer, ReceivedFrame, Role, SessionConfig, SessionStats};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};

/// How long the host holds a lost session, and how long a client
/// keeps retrying.
pub const HOLD: Duration = Duration::from_secs(120);

/// The LAN listener port of sodium-remote-host (Windows and Mac): what a bare address
/// without a port means. remoted's listener takes a free port, so it needs `IP:PORT`.
pub const DEFAULT_LAN_PORT: u16 = 47311;

/// `IP` or `IP:PORT` (`[v6]:PORT` for IPv6 with a port) as a LAN listener's address; a
/// missing port is `DEFAULT_LAN_PORT`.
pub fn parse_address(s: &str) -> Option<SocketAddr> {
    let s = s.trim();
    s.parse::<SocketAddr>().ok().or_else(|| s.trim_matches(['[', ']']).parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, DEFAULT_LAN_PORT)))
}
/// No packet at all for this long: the connection is lost (the path layer's
/// own failover is far faster; this is the last resort).
const LIVENESS: Duration = Duration::from_secs(4);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(10);
const STATS_INTERVAL: Duration = Duration::from_millis(250);
const HUD_INTERVAL: Duration = Duration::from_secs(1);
const RETRY: Duration = Duration::from_secs(2);
/// Time for a last control message (Disconnect, TakenOver) to get out.
const LINGER: Duration = Duration::from_millis(300);

// ---------------------------------------------------------------------------
// Shared types

/// The path in use, for the HUD and the path chip.
#[derive(Clone, Debug, PartialEq)]
pub enum PathKind {
    /// Direct, to a private address on the same network.
    Lan,
    /// Direct over the internet (hole-punched).
    Direct,
    /// Direct over a VPN: a VPN interface (tun*, wg*, …) or an address in one of the
    /// named networks (`RouteInfo::network` has its label).
    Vpn,
    /// Direct over Tailscale.
    Tailscale,
    /// Through a TURN relay, reached over this transport.
    Relay { transport: TurnTransport },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RouteInfo {
    pub kind: PathKind,
    pub local: SocketAddr,
    pub remote: SocketAddr,
    pub rtt_ms: f64,
    pub interface: Option<String>,
    /// The named network the peer's address is in (`networks.toml`), for the path
    /// chip: "VPN" becomes its label.
    pub network: Option<String>,
}

/// A network named by the host's configuration: `[networks] "198.51.100.0/24" = "Office VPN"`
/// in `networks.toml` (beside `server.toml`). A direct route to an address in it is a
/// `Vpn` path with that label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedNetwork {
    pub addr: IpAddr,
    pub prefix: u8,
    pub label: String,
}

impl NamedNetwork {
    /// `"198.51.100.0/24"`, `"fd00::/8"`, or a single address.
    pub fn parse(cidr: &str, label: &str) -> Option<NamedNetwork> {
        let (a, p) = cidr.trim().split_once('/').unwrap_or((cidr.trim(), ""));
        let addr: IpAddr = a.parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = if p.is_empty() { max } else { p.parse().ok().filter(|p| *p <= max)? };
        let label = label.trim();
        (!label.is_empty()).then(|| NamedNetwork { addr, prefix, label: label.to_string() })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            v4 => v4,
        };
        match (self.addr, ip) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let mask = u32::MAX.checked_shl(32 - self.prefix as u32).unwrap_or(0);
                u32::from(n) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let mask = u128::MAX.checked_shl(128 - self.prefix as u32).unwrap_or(0);
                u128::from(n) & mask == u128::from(a) & mask
            }
            _ => false,
        }
    }
}

static NAMED_NETWORKS: Mutex<Vec<NamedNetwork>> = Mutex::new(Vec::new());

/// Use these named networks for `route_info` (process-wide; the most specific match wins).
pub fn set_named_networks(mut v: Vec<NamedNetwork>) {
    v.sort_by_key(|n| std::cmp::Reverse(n.prefix));
    *NAMED_NETWORKS.lock().unwrap() = v;
}

/// Parse `networks.toml`'s text.
pub fn parse_named_networks(text: &str) -> Result<Vec<NamedNetwork>, String> {
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(default)]
        networks: std::collections::BTreeMap<String, String>,
    }
    let f: File = toml::from_str(text).map_err(|e| e.to_string())?;
    f.networks
        .iter()
        .map(|(cidr, label)| NamedNetwork::parse(cidr, label).ok_or_else(|| format!("{cidr:?} = {label:?} is not a network and a label")))
        .collect()
}

/// Read `networks.toml` from the configuration directories (first found) and use it.
pub fn load_named_networks() {
    for d in crate::dirs::config_dirs() {
        let path = d.join(crate::brand::NETWORKS_TOML);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        match parse_named_networks(&text) {
            Ok(v) => set_named_networks(v),
            Err(e) => tracing::warn!("{}: {e}", path.display()),
        }
        return;
    }
}

fn named_network(ip: IpAddr) -> Option<String> {
    NAMED_NETWORKS.lock().unwrap().iter().find(|n| n.contains(ip)).map(|n| n.label.clone())
}

pub fn route_info(r: &Route) -> RouteInfo {
    let interface = if_addrs::get_if_addrs()
        .ok()
        .and_then(|a| a.into_iter().find(|i| i.ip() == r.local.ip()).map(|i| i.name));
    let network = named_network(r.remote.ip());
    let kind = match &r.kind {
        RouteKind::RelayedLocal { transport, .. } | RouteKind::RelayedRemote { transport } => {
            PathKind::Relay { transport: *transport }
        }
        RouteKind::Direct if network.is_some() => PathKind::Vpn,
        RouteKind::Direct => {
            let ifname = interface.as_deref().unwrap_or("");
            let cgnat = matches!(r.remote.ip(), IpAddr::V4(a) if a.octets()[0] == 100 && (a.octets()[1] & 0xC0) == 64);
            if cgnat || ifname.starts_with("tailscale") {
                PathKind::Tailscale
            } else if ["tun", "gpd", "wg", "ppp", "ipsec"].iter().any(|p| ifname.starts_with(p)) {
                PathKind::Vpn
            } else if is_private(r.remote.ip()) {
                PathKind::Lan
            } else {
                PathKind::Direct
            }
        }
    };
    let network = if kind == PathKind::Vpn { network } else { None };
    RouteInfo { kind, local: r.local, remote: r.remote, rtt_ms: r.rtt.as_secs_f64() * 1000.0, interface, network }
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_private() || a.is_link_local(),
        IpAddr::V6(a) => (a.segments()[0] & 0xfe00) == 0xfc00 || (a.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// Where to bind UDP sockets: by default `0.0.0.0:0`, which the driver
/// turns into every IPv4 interface (LAN, tun0, tailscale0 all compete, SPEC
/// 7.1), watched and re-gathered on a network change; or the configured list.
fn bind_addrs(cfg: &Option<Vec<IpAddr>>) -> Vec<SocketAddr> {
    match cfg {
        Some(v) => v.iter().map(|ip| SocketAddr::new(*ip, 0)).collect(),
        None => vec![SocketAddr::from(([0, 0, 0, 0], 0))],
    }
}

/// STUN and TURN for one connection: fresh TURN credentials from the
/// rendezvous when it's up; none on the LAN path.
async fn ice_servers(rdv: Option<&Rendezvous>, ca_pem: Option<&[u8]>, controlling: bool) -> PathConfig {
    let mut cfg = PathConfig { controlling, stun_servers: vec![], turn_servers: vec![], tls: None };
    if let Some(rdv) = rdv {
        match rdv.turn_credentials().await {
            Ok(creds) => {
                let (stun, turn) = creds.resolve().await;
                cfg.stun_servers = stun;
                cfg.turn_servers = turn;
                cfg.tls = rendezvous::tls_config(ca_pem, false).ok();
            }
            Err(e) => tracing::warn!("no TURN credentials: {e}"),
        }
    }
    cfg
}

// ---------------------------------------------------------------------------
// Signalling channels (the rendezvous or the LAN listener)

/// One signalling session with a peer: sealed blobs in and out.
struct SigChan {
    key: DeviceKey,
    peer: DeviceId,
    context: Vec<u8>,
    out: mpsc::UnboundedSender<Vec<u8>>,
    inbox: mpsc::UnboundedReceiver<Vec<u8>>,
}

impl SigChan {
    fn send(&self, s: &Signal) -> bool {
        match keys::seal(&self.key, &self.peer, &self.context, &msg::encode(s)) {
            Some(b) => self.out.send(b).is_ok(),
            None => false,
        }
    }

    /// The next signal that opens and decodes (others are dropped).
    async fn recv(&mut self) -> Option<Signal> {
        loop {
            let b = self.inbox.recv().await?;
            if let Some(s) = keys::open(&self.key, &self.peer, &self.context, &b).and_then(|p| Signal::decode(&p)) {
                return Some(s);
            }
            tracing::warn!("dropped a signalling blob that didn't open");
        }
    }
}

type Routes = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Vec<u8>>>>>;

/// A signalling channel over a rendezvous session. The session is closed on
/// the server when the channel is dropped.
fn rdv_chan(rdv: Rendezvous, routes: &Routes, session: String, key: DeviceKey, peer: DeviceId) -> SigChan {
    let (in_tx, inbox) = mpsc::unbounded_channel();
    routes.lock().unwrap().insert(session.clone(), in_tx);
    let (out, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let routes = routes.clone();
    let s = session.clone();
    tokio::spawn(async move {
        while let Some(b) = out_rx.recv().await {
            if let Err(e) = rdv.signal(&s, &b).await {
                tracing::debug!("signal on {s}: {e}");
            }
        }
        routes.lock().unwrap().remove(&s);
        let _ = rdv.close_session(&s).await;
    });
    SigChan { key, peer, context: session.into_bytes(), out, inbox }
}

/// A signalling channel over a LAN listener connection. The connection closes
/// when the channel is dropped. `hold` lives as long as the connection (the
/// host's listener slot).
fn lan_chan(stream: TcpStream, context: Vec<u8>, key: DeviceKey, peer: DeviceId) -> SigChan {
    lan_chan_holding(stream, None, context, key, peer, ())
}

/// The client's side of a LAN signalling connection: `SignalHelloV2`, then the host's
/// nonce, which goes into the seal context. `None` when the host doesn't answer in time.
async fn lan_signal(mut stream: TcpStream, key: DeviceKey, host: DeviceId) -> Option<SigChan> {
    mdns::write_frame(&mut stream, &LanFrame::SignalHelloV2 { client: key.id().0 }).await.ok()?;
    match tokio::time::timeout(SIGNAL_TIMEOUT, mdns::read_frame(&mut stream)).await {
        Ok(Some(LanFrame::SignalNonce { nonce })) => Some(lan_chan(stream, mdns::signal_context(&nonce), key, host)),
        _ => None,
    }
}

fn lan_chan_holding<H: Send + 'static>(
    stream: TcpStream,
    mut first: Option<LanFrame>,
    context: Vec<u8>,
    key: DeviceKey,
    peer: DeviceId,
    hold: H,
) -> SigChan {
    let (mut rd, mut wr) = stream.into_split();
    let (in_tx, inbox) = mpsc::unbounded_channel();
    let (out, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    tokio::spawn(async move {
        // The hello goes before any signal.
        if let Some(f) = first.take() {
            if mdns::write_frame(&mut wr, &f).await.is_err() {
                return;
            }
        }
        while let Some(b) = out_rx.recv().await {
            if mdns::write_frame(&mut wr, &LanFrame::Signal(b)).await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        let _hold = hold;
        loop {
            // Stop reading once the channel is gone, not at the peer's next frame.
            let f = tokio::select! {
                f = mdns::read_frame(&mut rd) => f,
                _ = in_tx.closed() => None,
            };
            let Some(f) = f else { break };
            if let LanFrame::Signal(b) = f {
                if in_tx.send(b).is_err() {
                    break;
                }
            }
        }
    });
    SigChan { key, peer, context, out, inbox }
}

// ---------------------------------------------------------------------------
// Connections

/// What a connection's pump reports.
enum ConnNote {
    Notice(Notice),
    /// Gone: no packets for `LIVENESS`, the handshake timed out, or the driver
    /// stopped. The instant is the last authenticated packet from the peer
    Lost(&'static str, Instant),
}

/// Peer clock minus ours (µs), from the session's clock sync; `UNKNOWN` until
/// the first ping answer.
const OFFSET_UNKNOWN: i64 = i64::MIN;

#[derive(Clone)]
struct ConnHandle {
    id: u64,
    cmds: mpsc::UnboundedSender<Command>,
    rate: watch::Receiver<u64>,
    status: watch::Receiver<LinkStatus>,
    audio_out: Arc<Mutex<JitterBuffer>>,
    mic_out: Arc<Mutex<JitterBuffer>>,
    /// The session clock's origin, and the peer's clock minus ours.
    epoch: Instant,
    offset_us: Arc<AtomicI64>,
    /// The session numbers frames from 0, one per frame sent through this
    /// handle, in order: the id the next frame gets.
    next_frame: Arc<AtomicU32>,
    /// A fresh signalling channel for the pump (after a network change).
    resignal: mpsc::UnboundedSender<SigChan>,
}

impl ConnHandle {
    fn new(id: u64, ep: &EndpointHandle) -> (Self, mpsc::UnboundedReceiver<SigChan>) {
        let (resignal, rx) = mpsc::unbounded_channel();
        let c = ConnHandle {
            id,
            cmds: ep.commands.clone(),
            rate: ep.encoder_bitrate.clone(),
            status: ep.status.clone(),
            audio_out: ep.audio_out.clone(),
            mic_out: ep.mic_out.clone(),
            epoch: ep.epoch,
            offset_us: Arc::new(AtomicI64::new(OFFSET_UNKNOWN)),
            next_frame: Arc::new(AtomicU32::new(0)),
            resignal,
        };
        (c, rx)
    }

    /// Our session clock now (µs).
    fn now_us(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    /// An instant on our clock from a time on the peer's session clock.
    fn peer_instant(&self, peer_us: u64) -> Option<Instant> {
        let off = self.offset_us.load(Ordering::Relaxed);
        if off == OFFSET_UNKNOWN {
            return None;
        }
        let ours = peer_us as i64 - off;
        (ours >= 0).then(|| self.epoch + Duration::from_micros(ours as u64))
    }

    /// Send a frame; its session frame id.
    fn video(&self, frame: OutgoingFrame, opts: FrameOpts) -> Option<u32> {
        let id = self.next_frame.fetch_add(1, Ordering::SeqCst);
        self.cmds.send(Command::VideoFrameWith { frame, opts }).ok().map(|_| id)
    }

    fn control(&self, m: &Msg) {
        let _ = self.cmds.send(Command::Control { kind: msg::CONTROL_KIND, data: m.encode() });
    }

    fn close_after(&self, d: Duration) {
        let c = self.cmds.clone();
        tokio::spawn(async move {
            tokio::time::sleep(d).await;
            let _ = c.send(Command::Close);
        });
    }
}

static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

/// Spawn the driver for one connection and wait for its ICE credentials.
async fn start_endpoint(
    role: Role,
    key: &DeviceKey,
    peer: DeviceId,
    path: PathConfig,
    bind: Vec<SocketAddr>,
    fps: u32,
) -> Result<(EndpointHandle, String, String), String> {
    let peer_noise = peer.noise_public().ok_or("bad peer key")?;
    let p = match role {
        Role::Client => Peer::Host(peer_noise),
        Role::Host => Peer::PairedClients(vec![peer_noise]),
    };
    let mut scfg = SessionConfig::new(role);
    scfg.fps = fps;
    let sess = session::Session::new(scfg, key.noise_keypair(), p, Instant::now());
    if bind.is_empty() {
        return Err("no network interface to bind".into());
    }
    let mut ep = spawn_endpoint(EndpointConfig { path, bind, stats_interval: STATS_INTERVAL }, sess);
    match tokio::time::timeout(Duration::from_secs(2), ep.notices.recv()).await {
        Ok(Some(Notice::LocalCredentials { ufrag, pass })) => Ok((ep, ufrag, pass)),
        _ => Err("the endpoint didn't start".into()),
    }
}

/// Runs for a connection's lifetime: trickles candidates both ways, watches
/// liveness, and forwards everything else to the brain. A new signalling
/// channel on `resignal` (after a network change) replaces the old one, and
/// every local candidate so far goes out on it again.
fn spawn_pump<T: Send + 'static>(
    conn: &ConnHandle,
    mut resignal: mpsc::UnboundedReceiver<SigChan>,
    mut ep: EndpointHandle,
    mut sig: SigChan,
    brain: mpsc::UnboundedSender<T>,
    wrap: fn(u64, ConnNote) -> T,
    relay_only: bool,
) {
    let id = conn.id;
    let offset = conn.offset_us.clone();
    let cmds = ep.commands.clone();
    tokio::spawn(async move {
        let started = Instant::now();
        let mut connected = false;
        let mut last_rx = 0u64;
        let mut last_change = Instant::now();
        let mut sig_open = true;
        let mut cands: Vec<String> = Vec::new();
        let mut resignal_open = true;
        loop {
            tokio::select! {
                n = ep.notices.recv() => {
                    let Some(n) = n else {
                        let _ = brain.send(wrap(id, ConnNote::Lost("closed", last_change)));
                        return;
                    };
                    match &n {
                        Notice::LocalCandidate(c) => {
                            if !relay_only || c.contains("typ relay") {
                                cands.push(c.clone());
                                sig.send(&Signal::Candidate(c.clone()));
                            }
                            continue;
                        }
                        Notice::Session(session::Event::Connected { .. }) => {
                            connected = true;
                            last_change = Instant::now();
                        }
                        // The session's own idle timeout or an expired key: a
                        // fresh dial (new ICE, new handshake) follows.
                        Notice::Session(session::Event::Disconnected { .. }) => {
                            let _ = cmds.send(Command::Close);
                            let _ = brain.send(wrap(id, ConnNote::Lost("session ended", last_change)));
                            return;
                        }
                        Notice::Stats { stats, .. } => {
                            let now = Instant::now();
                            if stats.rtt_ms > 0.0 {
                                offset.store(stats.clock_offset_us, Ordering::Relaxed);
                            }
                            if stats.packets_received != last_rx {
                                last_rx = stats.packets_received;
                                last_change = now;
                            }
                            if connected && now - last_change > LIVENESS {
                                let _ = cmds.send(Command::Close);
                                let _ = brain.send(wrap(id, ConnNote::Lost("no packets", last_change)));
                                return;
                            }
                            if !connected && now - started > HANDSHAKE_TIMEOUT {
                                let _ = cmds.send(Command::Close);
                                let _ = brain.send(wrap(id, ConnNote::Lost("handshake timed out", last_change)));
                                return;
                            }
                        }
                        _ => {}
                    }
                    if brain.send(wrap(id, ConnNote::Notice(n))).is_err() {
                        let _ = cmds.send(Command::Close);
                        return;
                    }
                }
                s = sig.recv(), if sig_open => match s {
                    Some(Signal::Candidate(c)) => { let _ = cmds.send(Command::RemoteCandidate(c)); }
                    Some(_) => {}
                    None => sig_open = false,
                },
                fresh = resignal.recv(), if resignal_open => {
                    let Some(fresh) = fresh else {
                        resignal_open = false;
                        continue;
                    };
                    tracing::info!("signalling reopened; {} candidates sent again", cands.len());
                    sig = fresh;
                    sig_open = true;
                    for c in &cands {
                        sig.send(&Signal::Candidate(c.clone()));
                    }
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// The rendezvous loop (both roles)

struct RdvShared {
    cfg: RendezvousConfig,
    key: DeviceKey,
    role: DeviceRole,
    rdv: Mutex<Option<Rendezvous>>,
    routes: Routes,
    stop: AtomicBool,
    /// Asleep: stay disconnected until `wake_notice`.
    paused: AtomicBool,
}

enum RdvNote {
    Up(Rendezvous),
    Down(Option<String>),
    Event(rendezvous::Event),
}

/// Keep a rendezvous connection up, with backoff. Signals go straight to their
/// route; everything else to `notes`.
fn spawn_rdv_loop(sh: Arc<RdvShared>, notes: mpsc::UnboundedSender<RdvNote>) {
    tokio::spawn(async move {
        let mut backoff = Duration::from_secs(1);
        while !sh.stop.load(Ordering::Relaxed) {
            if sh.paused.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            match Rendezvous::connect(&sh.cfg, &sh.key, sh.role).await {
                Ok((rdv, mut events, _)) => {
                    backoff = Duration::from_secs(1);
                    *sh.rdv.lock().unwrap() = Some(rdv.clone());
                    let _ = notes.send(RdvNote::Up(rdv.clone()));
                    let mut error = None;
                    while let Some(ev) = events.recv().await {
                        match ev {
                            rendezvous::Event::Signal { session, blob } => {
                                let route = sh.routes.lock().unwrap().get(&session).cloned();
                                match route {
                                    Some(tx) => {
                                        let _ = tx.send(blob);
                                    }
                                    None => tracing::debug!("signal for unknown session {session}"),
                                }
                            }
                            rendezvous::Event::SessionClosed { ref session, .. } => {
                                sh.routes.lock().unwrap().remove(session);
                                let _ = notes.send(RdvNote::Event(ev));
                            }
                            rendezvous::Event::Disconnected { error: e } => {
                                error = e;
                                break;
                            }
                            ev => {
                                let _ = notes.send(RdvNote::Event(ev));
                            }
                        }
                    }
                    *sh.rdv.lock().unwrap() = None;
                    let _ = notes.send(RdvNote::Down(error));
                }
                Err(e) => {
                    if e.code() == Some("host_not_allowed") {
                        backoff = Duration::from_secs(30);
                    }
                    let _ = notes.send(RdvNote::Down(Some(e.to_string())));
                }
            }
            if notes.is_closed() || sh.stop.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
        if let Some(r) = sh.rdv.lock().unwrap().take() {
            r.close();
        }
    });
}

// ===========================================================================
// Host

pub struct HostConfig {
    pub store: Arc<dyn Store>,
    /// This host's display name.
    pub name: String,
    /// None: LAN only.
    pub rendezvous: Option<RendezvousConfig>,
    /// The LAN listener and mDNS announcement.
    pub lan: bool,
    /// Where the LAN listener binds (default 0.0.0.0, any port).
    pub lan_bind: SocketAddr,
    /// IPs to gather host candidates on (default: every non-loopback IPv4).
    pub bind: Option<Vec<IpAddr>>,
    pub fps: u32,
    pub hold: Duration,
    /// Share only relayed candidates (tests of the TURN path; hides this
    /// device's addresses from the peer).
    pub relay_only: bool,
    /// Announce the LAN listener over mDNS (off for tests on a shared LAN).
    pub mdns: bool,
}

impl HostConfig {
    pub fn new(store: Arc<dyn Store>, name: impl Into<String>) -> Self {
        HostConfig {
            store,
            name: name.into(),
            rendezvous: None,
            lan: true,
            lan_bind: "0.0.0.0:0".parse().unwrap(),
            bind: None,
            fps: 60,
            hold: HOLD,
            relay_only: false,
            mdns: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PairingWindow {
    pub code: String,
    pub expires_in: Duration,
}

#[derive(Debug, Clone)]
pub enum HostEvent {
    /// The rendezvous connection changed. `error` "host_not_allowed…" is the
    /// "not enrolled" state.
    Rendezvous { online: bool, error: Option<String> },
    PairingAttempt { attempt: u32 },
    PairingFailed { attempts_left: u32 },
    Paired(PairedDevice),
    /// The window ended: "paired", "expired", "attempts_used", "cancelled", …
    PairingClosed { reason: String },
    /// A client is connected and sent `Hello`: set REMOTE-1 up and answer with
    /// `Msg::Welcome`. `took_over_from`/`resumed` go into the Welcome.
    ClientConnected { client: DeviceId, name: String, hello: Hello, took_over_from: Option<String>, resumed: bool },
    /// The connection died: hold REMOTE-1 and the layout for `hold`.
    ClientLost { client: DeviceId },
    /// Nobody came back in time: put the windows back.
    HoldExpired { client: DeviceId },
    /// A clean end (no hold): Disconnect from either side, takeover, revoke.
    ClientDisconnected { client: DeviceId, reason: DisconnectReason },
    /// An input event and when it happened on the client, on our clock (None
    /// until clock sync has a sample: treat it as now). Only while the session is
    /// live (after `ClientConnected`); a view-only host drops them itself.
    Input { event: InputEvent, at: Option<Instant> },
    /// A control message, only while the session is live (`Hello` and `Disconnect`
    /// come as `ClientConnected` and `ClientDisconnected`).
    Control(Msg),
    PeerLostFrame { frame_id: u32 },
    ReceiverReport { frame_id: u32, decode_us: u32, present_us: u32 },
    Route(RouteInfo),
    Stats(SessionStats),
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    /// Accepted; ICE and Noise under way.
    Starting,
    /// Noise done; waiting for Hello.
    Handshaked,
    Live,
    Lost { since: Instant },
}

struct Current {
    conn: Option<ConnHandle>,
    client: DeviceId,
    name: String,
    phase: Phase,
    took_over_from: Option<String>,
    /// It took over a held session (no question asked): send the toast.
    silent_takeover: bool,
    resumed: bool,
    shapes: HashSet<u64>,
}

struct Window {
    w: HostWindow,
    rdv_pairing: Option<String>,
    lan_conn: Option<u64>,
}

enum HostCmd {
    OpenPairing(Option<String>, oneshot::Sender<PairingWindow>),
    ClosePairing,
    SetName(String),
    SetMdns(bool),
    RemovePaired(DeviceId, oneshot::Sender<bool>),
    Disconnect(DisconnectReason),
    CursorShape(u64, CursorImage),
    Shutdown(DisconnectReason, oneshot::Sender<()>),
}

enum HostIn {
    Cmd(HostCmd),
    Rdv(RdvNote),
    Offer { chan: SigChan, offer: msg::Offer },
    /// A client reopened signalling mid-session (its network changed): more
    /// candidates for the current connection.
    Trickle { chan: SigChan, first: String },
    Accepted { id: u64, client: DeviceId, result: Result<(EndpointHandle, SigChan), String> },
    Note(u64, ConnNote),
    LanPairOpen { conn: u64, writer: mpsc::UnboundedSender<LanFrame> },
    LanPair { conn: u64, data: Vec<u8> },
    LanPairClosed { conn: u64 },
    UploadDone { seq: u64 },
}

/// The live connection, for the hot paths.
#[derive(Default)]
struct LiveSlot {
    conn: Option<ConnHandle>,
    /// Host: the route demand for path choice (0: core's default).
    demand_bps: u64,
}

#[derive(Clone)]
pub struct Host {
    key: DeviceKey,
    store: Arc<dyn Store>,
    brain: mpsc::UnboundedSender<HostIn>,
    live: Arc<Mutex<LiveSlot>>,
    pairings: Arc<Mutex<Pairings>>,
    rdv: Arc<RdvShared>,
    lan_port: Option<u16>,
    /// The LAN listener's task: shutdown waits for it, so the port is free when shutdown returns.
    lan_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl Host {
    pub async fn start(cfg: HostConfig) -> std::io::Result<(Host, mpsc::UnboundedReceiver<HostEvent>)> {
        let key = keys::load_or_create_device_key(cfg.store.as_ref())?;
        let pairings = Arc::new(Mutex::new(Pairings::load(cfg.store.as_ref())));
        let (brain_tx, brain_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let routes: Routes = Default::default();
        let rdv = Arc::new(RdvShared {
            cfg: cfg.rendezvous.clone().unwrap_or_else(|| RendezvousConfig::new("")),
            key: key.clone(),
            role: DeviceRole::Host,
            rdv: Mutex::new(None),
            routes: routes.clone(),
            stop: AtomicBool::new(cfg.rendezvous.is_none()),
            paused: AtomicBool::new(false),
        });
        if cfg.rendezvous.is_some() {
            let (ntx, mut nrx) = mpsc::unbounded_channel();
            spawn_rdv_loop(rdv.clone(), ntx);
            let b = brain_tx.clone();
            tokio::spawn(async move {
                while let Some(n) = nrx.recv().await {
                    if b.send(HostIn::Rdv(n)).is_err() {
                        break;
                    }
                }
            });
        }
        let mut lan_port = None;
        let mut lan_task = None;
        let mut advertiser = None;
        if cfg.lan {
            let listener = TcpListener::bind(cfg.lan_bind).await?;
            let port = listener.local_addr()?.port();
            lan_port = Some(port);
            lan_task = Some(spawn_lan_listener(listener, key.clone(), pairings.clone(), brain_tx.clone()));
            if cfg.mdns {
                match Advertiser::start(key.id(), &cfg.name, port, false) {
                    Ok(a) => advertiser = Some(a),
                    Err(e) => tracing::warn!("mDNS: {e}"),
                }
            }
        }
        let live = Arc::new(Mutex::new(LiveSlot::default()));
        let host = Host {
            key: key.clone(),
            store: cfg.store.clone(),
            brain: brain_tx.clone(),
            live: live.clone(),
            pairings: pairings.clone(),
            rdv: rdv.clone(),
            lan_port,
            lan_task: Arc::new(Mutex::new(lan_task)),
        };
        let brain = HostBrain {
            key,
            name: cfg.name,
            store: cfg.store,
            ca_pem: cfg.rendezvous.and_then(|r| r.ca_pem),
            bind: cfg.bind,
            fps: cfg.fps,
            relay_only: cfg.relay_only,
            hold: cfg.hold,
            events: ev_tx,
            me: brain_tx,
            live,
            pairings,
            rdv,
            current: None,
            incoming: None,
            offer_seen: HashMap::new(),
            window: None,
            advertiser,
            lan_port,
            lan_writers: HashMap::new(),
            last_shape: None,
            server_seq: None,
            last_hud: Instant::now(),
            pending_accept: None,
        };
        tokio::spawn(brain.run(brain_rx));
        Ok((host, ev_rx))
    }

    pub fn id(&self) -> DeviceId {
        self.key.id()
    }

    pub fn key(&self) -> &DeviceKey {
        &self.key
    }

    /// The LAN listener's port (announced over mDNS).
    pub fn lan_port(&self) -> Option<u16> {
        self.lan_port
    }

    /// Open a pairing window (a new code; any old window closes).
    pub async fn open_pairing(&self) -> Option<PairingWindow> {
        let (tx, rx) = oneshot::channel();
        self.brain.send(HostIn::Cmd(HostCmd::OpenPairing(None, tx))).ok()?;
        rx.await.ok()
    }

    /// The same with the caller's own 6-digit code (remoted makes and shows it).
    pub async fn open_pairing_with(&self, code: &str) -> Option<PairingWindow> {
        if !pairing::valid_code(code) {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        self.brain.send(HostIn::Cmd(HostCmd::OpenPairing(Some(code.to_string()), tx))).ok()?;
        rx.await.ok()
    }

    /// Rename a paired device (Settings › Remote); saved at once. `""` hands the name back to the
    /// device (its next `Hello` sets it) and keeps the current one until then.
    pub fn rename_paired(&self, id: &DeviceId, name: &str) -> bool {
        let mut p = self.pairings.lock().unwrap();
        let Some(d) = p.devices.iter_mut().find(|d| d.id == *id) else { return false };
        let name = name.trim();
        if name.is_empty() {
            // The device's own name again: kept until its next connection brings it.
            d.named_at_desk = false;
        } else {
            d.name = name.to_string();
            d.named_at_desk = true;
        }
        let _ = p.save(self.store.as_ref());
        true
    }

    pub fn close_pairing(&self) {
        let _ = self.brain.send(HostIn::Cmd(HostCmd::ClosePairing));
    }

    /// Change the name this computer shows to devices, live: the mDNS announcement
    /// is re-announced under it, and the next pairing window carries it (the one
    /// open now keeps the name it was opened with: its rendezvous entry has it).
    pub fn set_name(&self, name: &str) {
        let _ = self.brain.send(HostIn::Cmd(HostCmd::SetName(name.to_string())));
    }

    /// Announce this computer over mDNS (true) or withdraw the announcement (false),
    /// live. The LAN listener keeps running either way, so a device that knows the
    /// address still connects; with `HostConfig::lan` off there is nothing to announce.
    pub fn set_mdns(&self, on: bool) {
        let _ = self.brain.send(HostIn::Cmd(HostCmd::SetMdns(on)));
    }

    pub fn paired(&self) -> Vec<PairedDevice> {
        self.pairings.lock().unwrap().devices.clone()
    }

    /// Unpair a device: revoked on the rendezvous at once (a new signed list),
    /// and disconnected if it's connected, after the local removal is saved.
    /// False if the device is absent or the removal could not be saved.
    pub async fn remove_paired(&self, id: DeviceId) -> bool {
        let (tx, rx) = oneshot::channel();
        if self.brain.send(HostIn::Cmd(HostCmd::RemovePaired(id, tx))).is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    fn live(&self) -> Option<ConnHandle> {
        self.live.lock().unwrap().conn.clone()
    }

    /// A control message to the current client. False when none is live.
    pub fn send(&self, m: &Msg) -> bool {
        self.live().map(|c| c.control(m)).is_some()
    }

    pub fn send_video(&self, frame: OutgoingFrame) -> bool {
        self.send_video_with(frame, FrameOpts::default()).is_some()
    }

    /// A frame with options: its reference after a `RefHint` (protocol.md 8.1).
    /// Returns the frame's id in this connection's session (ids start at 0 on
    /// every new connection), which `RefHint::Reference` and `PeerLostFrame` name.
    pub fn send_video_with(&self, frame: OutgoingFrame, opts: FrameOpts) -> Option<u32> {
        self.live()?.video(frame, opts)
    }

    /// The live session's clock origin: its µs timestamps count from here.
    pub fn session_epoch(&self) -> Option<Instant> {
        self.live().map(|c| c.epoch)
    }

    /// A time on the client's session clock (µs) as an instant on ours, once
    /// clock sync has a sample.
    pub fn peer_instant(&self, client_us: u64) -> Option<Instant> {
        self.live()?.peer_instant(client_us)
    }

    /// The stream rate route choice should plan for: 0.2 bit per
    /// pixel per frame at the session's mode, or the user's cap if lower. Kept
    /// for every later connection of the session.
    pub fn set_route_demand(&self, bps: u64) {
        let mut l = self.live.lock().unwrap();
        l.demand_bps = bps;
        if let Some(c) = &l.conn {
            let _ = c.cmds.send(Command::SetRouteDemand(bps));
        }
    }

    /// The link as the encoder needs it: the video budget, C_est and the split
    /// rule, reference hints, health and routes (protocol.md 8.4).
    pub fn link_status(&self) -> Option<LinkStatus> {
        self.live().map(|c| c.status.borrow().clone())
    }

    /// A watch of the same, for an encoder loop that waits on changes.
    pub fn link_status_watch(&self) -> Option<watch::Receiver<LinkStatus>> {
        self.live().map(|c| c.status.clone())
    }

    /// An animation is in flight (HINT start) or settled.
    pub fn set_animation(&self, on: bool) {
        if let Some(c) = self.live() {
            let _ = c.cmds.send(Command::Animation(on));
        }
    }

    /// One Opus packet of desktop audio; `capture_us` on the session clock.
    pub fn send_audio(&self, timestamp: u32, capture_us: Option<u64>, opus: Vec<u8>) {
        if let Some(c) = self.live() {
            let _ = c.cmds.send(Command::Audio { mic: false, timestamp, capture_us, data: opus });
        }
    }

    /// The client's microphone, jitter-buffered: pop 10 ms at a time.
    pub fn mic_buffer(&self) -> Option<Arc<Mutex<JitterBuffer>>> {
        self.live().map(|c| c.mic_out.clone())
    }

    /// The platform saw a network change: re-gather now.
    pub fn network_changed(&self) {
        if let Some(c) = self.live() {
            let _ = c.cmds.send(Command::NetworkChanged);
        }
    }

    /// The cursor image, sent once per hash (and again to every new client).
    /// An image over 256 pixels a side is scaled down to fit (clients drop larger ones).
    pub fn send_cursor_shape(&self, hash: u64, image: CursorImage) {
        let Some(image) = image.fitted() else {
            tracing::warn!("a cursor image whose pixels don't match its size: not sent");
            return;
        };
        let _ = self.brain.send(HostIn::Cmd(HostCmd::CursorShape(hash, image)));
    }

    /// The cursor's hotspot in stream pixels (unreliable, latest wins).
    pub fn send_cursor_position(&self, x: f32, y: f32, visible: bool, shape: u64) {
        if let Some(c) = self.live() {
            let _ = c.cmds.send(Command::CursorPosition { x, y, visible, shape });
        }
    }

    /// The encoder bitrate for the next frame.
    pub fn encoder_bitrate_bps(&self) -> u64 {
        self.live().map(|c| *c.rate.borrow()).unwrap_or(0)
    }

    pub fn disconnect(&self, reason: DisconnectReason) {
        let _ = self.brain.send(HostIn::Cmd(HostCmd::Disconnect(reason)));
    }

    /// logind's `PrepareForSleep`: tell the client and the rendezvous.
    /// The rendezvous stays disconnected until `wake_notice`.
    pub async fn sleep_notice(&self) {
        self.disconnect(DisconnectReason::HostSleeping);
        self.rdv.paused.store(true, Ordering::Relaxed);
        let rdv = self.rdv.rdv.lock().unwrap().clone();
        if let Some(r) = rdv {
            let _ = r.sleep_notice().await;
        }
    }

    /// logind's `PrepareForSleep(false)`: back online.
    pub fn wake_notice(&self) {
        self.rdv.paused.store(false, Ordering::Relaxed);
    }

    /// Forget this host on the rendezvous (its record and paired list). For
    /// tests and "turn Remote off for good".
    /// The rendezvous loop stops first, so the device isn't registered again.
    pub async fn unregister(&self) -> Result<(), RdvError> {
        self.rdv.stop.store(true, Ordering::Relaxed);
        let rdv = self.rdv.rdv.lock().unwrap().clone();
        match rdv {
            Some(r) => r.unregister().await,
            None => Err(RdvError::Io("not connected".into())),
        }
    }

    pub async fn shutdown(&self) {
        self.shutdown_with(DisconnectReason::HostShutdown).await;
    }

    /// Shut down, telling the connected client `reason` (`ControlOff` when the host's switch goes off).
    pub async fn shutdown_with(&self, reason: DisconnectReason) {
        self.rdv.stop.store(true, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        if self.brain.send(HostIn::Cmd(HostCmd::Shutdown(reason, tx))).is_ok() {
            let _ = rx.await;
        }
        if let Some(r) = self.rdv.rdv.lock().unwrap().take() {
            r.close();
        }
        // The listener would also go once the brain is gone, but on its own time: a start right after this
        // would find the port still taken.
        let task = self.lan_task.lock().unwrap().take();
        if let Some(t) = task {
            t.abort();
            let _ = t.await;
        }
    }
}

fn spawn_lan_listener(
    listener: TcpListener,
    key: DeviceKey,
    pairings: Arc<Mutex<Pairings>>,
    brain: mpsc::UnboundedSender<HostIn>,
) -> tokio::task::JoinHandle<()> {
    let slots = lanlimit::Slots::default();
    let pair_rate = Arc::new(Mutex::new(lanlimit::PairRate::new(Instant::now())));
    tokio::spawn(async move {
        loop {
            // The listener goes with the host: when the brain is gone the port is released at once, not at the
            // next connection (a stop and start of the host would otherwise find the port taken).
            let accepted = tokio::select! {
                r = listener.accept() => r,
                _ = brain.closed() => break,
            };
            let Ok((stream, from)) = accepted else { break };
            if brain.is_closed() {
                break;
            }
            // Anyone on the network can connect: a few at a time from one address, a few dozen in all.
            let Some(slot) = slots.take(from.ip()) else {
                tracing::debug!("LAN listener: too many connections; refused {from}");
                continue;
            };
            let _ = stream.set_nodelay(true);
            let key = key.clone();
            let pairings = pairings.clone();
            let brain = brain.clone();
            let pair_rate = pair_rate.clone();
            tokio::spawn(async move {
                let mut stream = stream;
                let Ok(Some(first)) = tokio::time::timeout(SIGNAL_TIMEOUT, mdns::read_frame(&mut stream)).await else {
                    return;
                };
                match first {
                    LanFrame::PairHello => {
                        // Each try is one guess at the code: a few from one address, a few more from everyone.
                        if !pair_rate.lock().unwrap().allow(from.ip(), Instant::now()) {
                            tracing::info!("LAN pairing from {from}: too many tries; refused");
                            let _ = mdns::write_frame(&mut stream, &LanFrame::PairClosed).await;
                            return;
                        }
                        lan_pairing(stream, brain, slot).await
                    }
                    LanFrame::SignalHelloV2 { client } => {
                        let client = DeviceId(client);
                        if pairings.lock().unwrap().get(&client).is_none() {
                            tracing::info!("LAN signalling from an unpaired device {} at {from}", client.short());
                            return;
                        }
                        let nonce: [u8; 16] = rand::random();
                        let context = mdns::signal_context(&nonce);
                        let chan = lan_chan_holding(stream, Some(LanFrame::SignalNonce { nonce }), context, key, client, slot);
                        wait_offer(chan, brain);
                    }
                    LanFrame::SignalHello { client } => {
                        // The first version sealed every LAN connection with the same context, so a recorded
                        // blob could be played again on a new connection. Refused: the client must update.
                        tracing::info!("LAN signalling from {} at {from} uses the old hello; refused", DeviceId(client).short());
                    }
                    _ => {}
                }
            });
        }
    })
}

/// One LAN pairing connection: its frames go to the brain until either side
/// closes it. The brain's `PairClosed` (a refusal or the outcome) ends it, and
/// so does a peer that sends nothing for `SIGNAL_TIMEOUT`.
async fn lan_pairing(stream: TcpStream, brain: mpsc::UnboundedSender<HostIn>, slot: lanlimit::Slot) {
    let _slot = slot;
    let id = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    let (mut rd, mut wr) = stream.into_split();
    let (wtx, mut wrx) = mpsc::unbounded_channel::<LanFrame>();
    let (done_tx, mut done) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _done = done_tx;
        while let Some(f) = wrx.recv().await {
            let close = f == LanFrame::PairClosed;
            if mdns::write_frame(&mut wr, &f).await.is_err() || close {
                break;
            }
        }
    });
    let _ = brain.send(HostIn::LanPairOpen { conn: id, writer: wtx });
    loop {
        let f = tokio::select! {
            r = tokio::time::timeout(SIGNAL_TIMEOUT, mdns::read_frame(&mut rd)) => r.ok().flatten(),
            _ = &mut done => None,
        };
        let Some(f) = f else { break };
        if let LanFrame::Pair(data) = f {
            let _ = brain.send(HostIn::LanPair { conn: id, data });
        }
    }
    let _ = brain.send(HostIn::LanPairClosed { conn: id });
}

/// Wait for a signalling session's first message: an Offer, or a Candidate
/// when a connected client reopens signalling after a network change.
fn wait_offer(mut chan: SigChan, brain: mpsc::UnboundedSender<HostIn>) {
    tokio::spawn(async move {
        match tokio::time::timeout(SIGNAL_TIMEOUT, chan.recv()).await {
            Ok(Some(Signal::Offer(offer))) => {
                let _ = brain.send(HostIn::Offer { chan, offer });
            }
            Ok(Some(Signal::Candidate(first))) => {
                let _ = brain.send(HostIn::Trickle { chan, first });
            }
            _ => tracing::debug!("signalling from {} gave no offer", chan.peer.short()),
        }
    });
}

struct HostBrain {
    key: DeviceKey,
    name: String,
    store: Arc<dyn Store>,
    /// The pinned CA for TURN over TLS (None: Web PKI).
    ca_pem: Option<Vec<u8>>,
    bind: Option<Vec<IpAddr>>,
    fps: u32,
    relay_only: bool,
    hold: Duration,
    events: mpsc::UnboundedSender<HostEvent>,
    me: mpsc::UnboundedSender<HostIn>,
    live: Arc<Mutex<LiveSlot>>,
    pairings: Arc<Mutex<Pairings>>,
    rdv: Arc<RdvShared>,
    current: Option<Current>,
    /// A new connection being set up beside `current`: it replaces it only once
    /// the client has proved itself in it (`promote_incoming`).
    incoming: Option<Current>,
    /// The newest `Offer::sent_us` taken from each client: an offer not newer is a replay.
    offer_seen: HashMap<DeviceId, u64>,
    window: Option<Window>,
    advertiser: Option<Advertiser>,
    /// The LAN listener's port, once bound: what an announcement started later points at.
    lan_port: Option<u16>,
    lan_writers: HashMap<u64, mpsc::UnboundedSender<LanFrame>>,
    last_shape: Option<(u64, CursorImage)>,
    /// The paired list's seq on the server, once known.
    server_seq: Option<u64>,
    last_hud: Instant,
    /// The accept in flight (connection ID, client).
    pending_accept: Option<(u64, DeviceId)>,
}

impl HostBrain {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<HostIn>) {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                m = rx.recv() => {
                    let Some(m) = m else { break };
                    if let HostIn::Cmd(HostCmd::Shutdown(reason, done)) = m {
                        self.drop_incoming(Some(reason.clone()));
                        self.end_current(reason, true);
                        self.close_window("cancelled");
                        self.advertiser = None;
                        let _ = done.send(());
                        break;
                    }
                    self.handle(m);
                }
                _ = tick.tick() => self.on_tick(),
            }
        }
    }

    fn emit(&self, e: HostEvent) {
        let _ = self.events.send(e);
    }

    fn rdv(&self) -> Option<Rendezvous> {
        self.rdv.rdv.lock().unwrap().clone()
    }

    fn set_live(&self, c: Option<ConnHandle>) {
        let mut l = self.live.lock().unwrap();
        if let Some(c) = &c {
            if l.demand_bps > 0 {
                let _ = c.cmds.send(Command::SetRouteDemand(l.demand_bps));
            }
        }
        l.conn = c;
    }

    fn handle(&mut self, m: HostIn) {
        match m {
            HostIn::Cmd(c) => self.on_cmd(c),
            HostIn::Trickle { chan, first } => {
                let Some(cur) = &self.current else { return };
                if cur.client != chan.peer {
                    return;
                }
                if let Some(c) = &cur.conn {
                    tracing::info!("{} reopened signalling", cur.client.short());
                    let _ = c.cmds.send(Command::RemoteCandidate(first));
                    let _ = c.resignal.send(chan);
                }
            }
            HostIn::Rdv(RdvNote::Up(rdv)) => {
                self.emit(HostEvent::Rendezvous { online: true, error: None });
                self.upload_list(Some(rdv));
                if let Some(w) = &self.window {
                    if w.rdv_pairing.is_none() && w.w.is_open(Instant::now()) {
                        self.open_rdv_window(None);
                    }
                }
            }
            HostIn::Rdv(RdvNote::Down(error)) => {
                if let Some(w) = &mut self.window {
                    w.rdv_pairing = None;
                }
                self.emit(HostEvent::Rendezvous { online: false, error });
            }
            HostIn::Rdv(RdvNote::Event(e)) => self.on_rdv_event(e),
            HostIn::Offer { chan, offer } => self.on_offer(chan, offer),
            HostIn::Accepted { id, client, result } => self.on_accepted(id, client, result),
            HostIn::Note(id, note) => self.on_note(id, note),
            HostIn::LanPairOpen { conn, writer } => {
                let now = Instant::now();
                let busy = self.window.as_ref().map(|w| w.lan_conn.is_some()).unwrap_or(true);
                let ok = !busy && self.window.as_mut().map(|w| w.w.begin_attempt(now)).unwrap_or(false);
                if ok {
                    let w = self.window.as_mut().unwrap();
                    w.lan_conn = Some(conn);
                    let attempt = pairing::ATTEMPTS - w.w.attempts_left();
                    self.lan_writers.insert(conn, writer);
                    self.emit(HostEvent::PairingAttempt { attempt });
                } else {
                    let _ = writer.send(LanFrame::PairClosed);
                }
            }
            HostIn::LanPair { conn, data } => {
                if self.window.as_ref().and_then(|w| w.lan_conn) == Some(conn) {
                    self.pair_step(&data, PairVia::Lan(conn));
                }
            }
            HostIn::LanPairClosed { conn } => {
                self.lan_writers.remove(&conn);
                if let Some(w) = &mut self.window {
                    if w.lan_conn == Some(conn) {
                        w.lan_conn = None;
                        w.w.end_attempt();
                        self.after_attempt();
                    }
                }
            }
            HostIn::UploadDone { seq } => {
                self.server_seq = Some(seq);
            }
        }
    }

    fn on_cmd(&mut self, c: HostCmd) {
        match c {
            HostCmd::OpenPairing(code, reply) => {
                let old = self.window.as_mut().and_then(|w| w.rdv_pairing.take());
                self.close_window("cancelled");
                let code = code.unwrap_or_else(pairing::new_code);
                let w = HostWindow::new(self.key.clone(), self.name.clone(), code.clone(), Instant::now());
                self.window = Some(Window { w, rdv_pairing: None, lan_conn: None });
                self.open_rdv_window(old);
                if let Some(a) = &mut self.advertiser {
                    let _ = a.set_pairing(true);
                }
                let _ = reply.send(PairingWindow { code, expires_in: pairing::WINDOW });
            }
            HostCmd::ClosePairing => self.close_window("cancelled"),
            HostCmd::SetName(name) => {
                self.name = name;
                let pairing = self.window.is_some();
                if let Some(a) = &mut self.advertiser {
                    if let Err(e) = a.rename(&self.name, pairing) {
                        tracing::warn!("mDNS: {e}");
                    }
                }
            }
            HostCmd::SetMdns(on) => {
                if on && self.advertiser.is_none() {
                    if let Some(port) = self.lan_port {
                        match Advertiser::start(self.key.id(), &self.name, port, self.window.is_some()) {
                            Ok(a) => self.advertiser = Some(a),
                            Err(e) => tracing::warn!("mDNS: {e}"),
                        }
                    }
                } else if !on {
                    // Dropping it waits for the goodbye to go out: not on the brain's thread.
                    if let Some(a) = self.advertiser.take() {
                        tokio::task::spawn_blocking(move || drop(a));
                    }
                }
            }
            HostCmd::RemovePaired(id, reply) => {
                let removed = {
                    let mut p = self.pairings.lock().unwrap();
                    let mut next = p.clone();
                    let r = next.remove(&id) && next.save(self.store.as_ref()).is_ok();
                    if r {
                        *p = next;
                    }
                    r
                };
                if removed {
                    self.upload_list(None);
                    if self.incoming.as_ref().is_some_and(|c| c.client == id) {
                        self.drop_incoming(Some(DisconnectReason::Revoked));
                    }
                    if self.current.as_ref().is_some_and(|c| c.client == id) {
                        self.end_current(DisconnectReason::Revoked, true);
                    }
                }
                let _ = reply.send(removed);
            }
            HostCmd::Disconnect(reason) => {
                self.drop_incoming(Some(reason.clone()));
                self.end_current(reason, true);
            }
            HostCmd::CursorShape(hash, image) => {
                if let Some(cur) = &mut self.current {
                    if let Some(c) = &cur.conn {
                        if cur.phase == Phase::Live && cur.shapes.insert(hash) {
                            send_shape(c, hash, &image);
                        }
                    }
                }
                self.last_shape = Some((hash, image));
            }
            HostCmd::Shutdown(..) => {}
        }
    }

    fn on_tick(&mut self) {
        let now = Instant::now();
        if let Some(cur) = &self.current {
            if let Phase::Lost { since } = cur.phase {
                if now - since >= self.hold {
                    let client = cur.client;
                    self.current = None;
                    self.emit(HostEvent::HoldExpired { client });
                }
            }
        }
        if let Some(w) = &self.window {
            // A used window closes as "paired" once M4 is out (pair_step).
            if !w.w.is_open(now) && w.lan_conn.is_none() && !w.w.is_used() {
                let reason = if now >= w.w.expires() { "expired" } else { "attempts_used" };
                self.close_window(reason);
            }
        }
    }

    // --- pairing ---------------------------------------------------------

    /// Open the window on the rendezvous, after closing `old` there (one
    /// window per host: the close must land first).
    fn open_rdv_window(&mut self, old: Option<String>) {
        let Some(rdv) = self.rdv() else { return };
        let name = self.name.clone();
        let me = self.me.clone();
        tokio::spawn(async move {
            if let Some(p) = old {
                let _ = rdv.pair_close(&p).await;
            }
            match rdv.pair_open(&name).await {
                Ok((pairing, _)) => {
                    let _ = me.send(HostIn::Rdv(RdvNote::Event(rendezvous::Event::PairClosed {
                        pairing,
                        reason: "__opened".into(),
                    })));
                }
                Err(e) => tracing::warn!("pair_open: {e}"),
            }
        });
    }

    fn close_window(&mut self, reason: &str) {
        let Some(w) = self.window.take() else { return };
        if let (Some(p), Some(rdv)) = (w.rdv_pairing, self.rdv()) {
            tokio::spawn(async move {
                let _ = rdv.pair_close(&p).await;
            });
        }
        if let Some(c) = w.lan_conn {
            if let Some(wr) = self.lan_writers.remove(&c) {
                let _ = wr.send(LanFrame::PairClosed);
            }
        }
        if let Some(a) = &mut self.advertiser {
            let _ = a.set_pairing(false);
        }
        self.emit(HostEvent::PairingClosed { reason: reason.into() });
    }

    fn after_attempt(&mut self) {
        if let Some(w) = &self.window {
            if w.w.attempts_left() == 0 && w.lan_conn.is_none() {
                self.close_window("attempts_used");
            }
        }
    }

    fn pair_step(&mut self, data: &[u8], via: PairVia) {
        let now = Instant::now();
        let Some(w) = &mut self.window else { return };
        let step = w.w.handle(now, data);
        let send = |this: &Self, b: Vec<u8>| match &via {
            PairVia::Rdv(p) => {
                if let Some(rdv) = this.rdv() {
                    let p = p.clone();
                    tokio::spawn(async move {
                        let _ = rdv.pair_msg(&p, &b).await;
                    });
                }
            }
            PairVia::Lan(c) => {
                if let Some(wr) = this.lan_writers.get(c) {
                    let _ = wr.send(LanFrame::Pair(b));
                }
            }
        };
        match step {
            Step::Send(b) => send(self, b),
            Step::Paired { id, name, reply } => {
                if let Some(b) = reply {
                    send(self, b);
                }
                let dev = PairedDevice { id, name, paired_at: unix_now(), last_connected: None, named_at_desk: false, lan_addr: None };
                {
                    let mut p = self.pairings.lock().unwrap();
                    p.upsert(dev.clone());
                    let _ = p.save(self.store.as_ref());
                }
                self.upload_list(None);
                self.emit(HostEvent::Paired(dev));
                // Give M4 a moment to leave before the window (and a LAN
                // connection) closes.
                let me = self.me.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(LINGER).await;
                    let _ = me.send(HostIn::Rdv(RdvNote::Event(rendezvous::Event::PairClosed {
                        pairing: String::new(),
                        reason: "__paired".into(),
                    })));
                });
            }
            Step::Failed { reply } => {
                let left = w.w.attempts_left();
                if let Some(b) = reply {
                    send(self, b);
                }
                match &via {
                    PairVia::Rdv(p) => {
                        if let Some(rdv) = self.rdv() {
                            let p = p.clone();
                            tokio::spawn(async move {
                                let _ = rdv.pair_fail(&p).await;
                            });
                        }
                    }
                    PairVia::Lan(c) => {
                        if let Some(wr) = self.lan_writers.remove(c) {
                            let _ = wr.send(LanFrame::PairClosed);
                        }
                        if let Some(w) = &mut self.window {
                            w.lan_conn = None;
                        }
                    }
                }
                self.emit(HostEvent::PairingFailed { attempts_left: left });
                self.after_attempt();
            }
            Step::Ignore => {}
        }
    }

    /// Upload the signed paired list with a seq above both ours and the server's.
    fn upload_list(&mut self, rdv: Option<Rendezvous>) {
        let Some(rdv) = rdv.or_else(|| self.rdv()) else { return };
        let key = self.key.clone();
        let store = self.store.clone();
        let pairings = self.pairings.clone();
        let me = self.me.clone();
        let known = self.server_seq;
        tokio::spawn(async move {
            let server = match known {
                Some(s) => s,
                None => rdv.paired_list_seq().await.unwrap_or(0),
            };
            for _ in 0..3 {
                let (seq, clients) = {
                    let mut p = pairings.lock().unwrap();
                    p.seq = p.seq.max(server) + 1;
                    let _ = p.save(store.as_ref());
                    (p.seq, p.devices.iter().map(|d| d.id).collect::<Vec<_>>())
                };
                match rdv.upload_paired_list(&key, seq, &clients).await {
                    Ok(()) => {
                        let _ = me.send(HostIn::UploadDone { seq });
                        return;
                    }
                    Err(e) if e.code() == Some("stale_seq") => {
                        let s = rdv.paired_list_seq().await.unwrap_or(0);
                        let mut p = pairings.lock().unwrap();
                        p.seq = p.seq.max(s);
                    }
                    Err(e) => {
                        tracing::warn!("paired list upload: {e}");
                        return;
                    }
                }
            }
        });
    }

    fn on_rdv_event(&mut self, e: rendezvous::Event) {
        use rendezvous::Event as E;
        match e {
            E::Incoming { session, client } => {
                let Some(rdv) = self.rdv() else { return };
                if self.pairings.lock().unwrap().get(&client).is_none() {
                    tokio::spawn(async move {
                        let _ = rdv.close_session(&session).await;
                    });
                    return;
                }
                let chan = rdv_chan(rdv, &self.rdv.routes, session, self.key.clone(), client);
                wait_offer(chan, self.me.clone());
            }
            E::PairJoined { pairing, attempt } => {
                let now = Instant::now();
                let Some(w) = &mut self.window else { return };
                if w.rdv_pairing.as_deref() != Some(&pairing) {
                    return;
                }
                w.w.begin_attempt(now);
                self.emit(HostEvent::PairingAttempt { attempt });
            }
            E::PairMsg { pairing, blob } => {
                if self.window.as_ref().and_then(|w| w.rdv_pairing.as_deref()) == Some(&pairing) {
                    self.pair_step(&blob, PairVia::Rdv(pairing));
                }
            }
            E::PairLeft { pairing } => {
                if let Some(w) = &mut self.window {
                    if w.rdv_pairing.as_deref() == Some(&pairing) {
                        w.w.end_attempt();
                        self.after_attempt();
                    }
                }
            }
            E::PairClosed { pairing, reason } => match reason.as_str() {
                // Internal notes (see open_rdv_window and pair_step).
                "__opened" => match &mut self.window {
                    Some(w) => w.rdv_pairing = Some(pairing),
                    None => {
                        if let Some(rdv) = self.rdv() {
                            tokio::spawn(async move {
                                let _ = rdv.pair_close(&pairing).await;
                            });
                        }
                    }
                },
                "__paired" => self.close_window("paired"),
                _ => {
                    if let Some(w) = &mut self.window {
                        if w.rdv_pairing.as_deref() == Some(&pairing) {
                            w.rdv_pairing = None;
                            // The server's window ended; the LAN may still pair
                            // within the host's own limits.
                            if reason == "attempts_used" || reason == "expired" {
                                self.close_window(&reason);
                            }
                        }
                    }
                }
            },
            E::SessionClosed { .. } | E::Presence { .. } | E::Revoked { .. } | E::Signal { .. } => {}
            E::Replaced => tracing::warn!("this host's rendezvous connection was replaced by another"),
            E::Notice(n) => tracing::info!("rendezvous notice {}: {}", n.code, n.text),
            E::Disconnected { .. } => {}
        }
    }

    // --- connections -----------------------------------------------------

    fn on_offer(&mut self, chan: SigChan, offer: msg::Offer) {
        let client = chan.peer;
        if let Err(refusal) = msg::negotiate(msg::VersionRange::OURS, offer.versions()) {
            tracing::info!("refused {}: {refusal:?}", client.short());
            chan.send(&Signal::Refused(refusal));
            return;
        }
        let Some(paired) = self.pairings.lock().unwrap().get(&client).cloned() else {
            chan.send(&Signal::Refused(DisconnectReason::Revoked));
            return;
        };
        // Freshness: only an offer newer than the last one taken from this client. A recorded one played
        // again (on the LAN, or by a server reusing a session) is dropped unanswered.
        if offer.sent_us <= self.offer_seen.get(&client).copied().unwrap_or(0) {
            tracing::warn!("dropped an offer from {} that isn't newer than the last one: a replay", client.short());
            return;
        }
        self.offer_seen.insert(client, offer.sent_us);
        // Someone else is connected, or connecting, and the user didn't agree to take over.
        let busy_by = [self.current.as_ref(), self.incoming.as_ref()]
            .into_iter()
            .flatten()
            .find(|c| c.client != client && !matches!(c.phase, Phase::Lost { .. }))
            .map(|c| c.name.clone());
        if let (Some(by), false) = (busy_by, offer.takeover) {
            chan.send(&Signal::Busy { by });
            return;
        }
        // The new connection is set up beside the current session, which it replaces only once the client
        // has proved itself in it (promote_incoming): an offer alone ends nothing.
        // One being set up for another client is told it was taken over; the same client's earlier one is
        // simply dropped (it dialled again).
        let why = self.incoming.as_ref().filter(|c| c.client != client).map(|_| DisconnectReason::TakenOver { by: offer.name.clone() });
        self.drop_incoming(why);
        let id = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
        self.incoming = Some(Current {
            conn: None,
            client,
            name: if offer.name.is_empty() { paired.name } else { offer.name.clone() },
            phase: Phase::Starting,
            took_over_from: None,
            silent_takeover: false,
            resumed: false,
            shapes: HashSet::new(),
        });
        // ICE: TURN credentials, then the endpoint, then the answer.
        let (rdv, key, ca, bind, fps, me) =
            (self.rdv(), self.key.clone(), self.ca_pem.clone(), bind_addrs(&self.bind), self.fps, self.me.clone());
        let lan = chan.context == b"lan";
        tokio::spawn(async move {
            let path = ice_servers(if lan { None } else { rdv.as_ref() }, ca.as_deref(), false).await;
            let result = match start_endpoint(Role::Host, &key, client, path, bind, fps).await {
                Ok((ep, ufrag, pass)) => {
                    let _ = ep.commands.send(Command::RemoteCredentials { ufrag: offer.ice_ufrag, pass: offer.ice_pass });
                    for c in offer.candidates {
                        let _ = ep.commands.send(Command::RemoteCandidate(c));
                    }
                    chan.send(&Signal::Answer(msg::Answer { ice_ufrag: ufrag, ice_pass: pass, candidates: vec![] }));
                    Ok((ep, chan))
                }
                Err(e) => {
                    chan.send(&Signal::Refused(DisconnectReason::Error(e.clone())));
                    Err(e)
                }
            };
            let _ = me.send(HostIn::Accepted { id, client, result });
        });
        self.pending_accept = Some((id, client));
    }

    /// Give up the connection being set up (another offer replaced it, the host disconnects or stops). A
    /// different client's is told why.
    fn drop_incoming(&mut self, why: Option<DisconnectReason>) {
        let Some(inc) = self.incoming.take() else { return };
        if let Some(c) = &inc.conn {
            if let Some(reason) = why {
                c.control(&Msg::Disconnect(reason));
            }
            c.close_after(LINGER);
        }
    }

    /// The new connection's client has proved itself (its first control message, under keys only the
    /// paired device can have): it replaces the current session now. Until here the current one ran on.
    fn promote_incoming(&mut self) {
        let Some(mut inc) = self.incoming.take() else { return };
        if let Some(old) = self.current.take() {
            self.set_live(None);
            if old.client == inc.client {
                inc.resumed = true;
                inc.took_over_from = old.took_over_from.clone();
            } else {
                inc.took_over_from = Some(old.name.clone());
                inc.silent_takeover = matches!(old.phase, Phase::Lost { .. });
            }
            if let Some(c) = &old.conn {
                if old.client != inc.client {
                    c.control(&Msg::TakenOver { by: inc.name.clone() });
                    c.control(&Msg::Disconnect(DisconnectReason::TakenOver { by: inc.name.clone() }));
                    c.close_after(LINGER);
                } else {
                    let _ = c.cmds.send(Command::Close);
                }
            }
            if old.client != inc.client && !matches!(old.phase, Phase::Lost { .. }) {
                self.emit(HostEvent::ClientDisconnected {
                    client: old.client,
                    reason: DisconnectReason::TakenOver { by: inc.name.clone() },
                });
            }
        }
        self.current = Some(inc);
    }

    /// A note from the connection being set up.
    fn on_incoming_note(&mut self, note: ConnNote) {
        let Some(inc) = &mut self.incoming else { return };
        match note {
            ConnNote::Lost(why, _) => {
                tracing::info!("new connection from {} failed: {why}", inc.client.short());
                self.incoming = None;
            }
            ConnNote::Notice(Notice::Session(session::Event::Connected { .. })) => {
                if inc.phase == Phase::Starting {
                    inc.phase = Phase::Handshaked;
                }
            }
            ConnNote::Notice(n @ Notice::Session(session::Event::Control { kind: msg::CONTROL_KIND, .. }))
                if inc.phase == Phase::Handshaked =>
            {
                self.promote_incoming();
                self.on_notice(n);
            }
            // Nothing else counts before then.
            ConnNote::Notice(_) => {}
        }
    }

    fn on_accepted(&mut self, id: u64, client: DeviceId, result: Result<(EndpointHandle, SigChan), String>) {
        let still_wanted = self.pending_accept == Some((id, client))
            && self.incoming.as_ref().is_some_and(|c| c.client == client && c.phase == Phase::Starting);
        match result {
            Ok((ep, chan)) => {
                if !still_wanted {
                    let _ = ep.commands.send(Command::Close);
                    return;
                }
                let (handle, resignal) = ConnHandle::new(id, &ep);
                spawn_pump(&handle, resignal, ep, chan, self.me.clone(), HostIn::Note, self.relay_only);
                if let Some(inc) = &mut self.incoming {
                    inc.conn = Some(handle);
                }
            }
            Err(e) => {
                tracing::warn!("accepting {}: {e}", client.short());
                if still_wanted {
                    self.incoming = None;
                }
            }
        }
        if self.pending_accept == Some((id, client)) {
            self.pending_accept = None;
        }
    }

    fn on_note(&mut self, id: u64, note: ConnNote) {
        if self.incoming.as_ref().and_then(|c| c.conn.as_ref()).is_some_and(|c| c.id == id) {
            self.on_incoming_note(note);
            return;
        }
        let Some(cur) = &mut self.current else { return };
        if cur.conn.as_ref().map(|c| c.id) != Some(id) {
            return;
        }
        match note {
            ConnNote::Lost(why, _) => {
                tracing::info!("connection to {} lost: {why}", cur.client.short());
                let was = std::mem::replace(&mut cur.phase, Phase::Lost { since: Instant::now() });
                cur.conn = None;
                let client = cur.client;
                self.set_live(None);
                match was {
                    Phase::Live | Phase::Handshaked => self.emit(HostEvent::ClientLost { client }),
                    // Never got going: nothing to hold.
                    _ => self.current = None,
                }
            }
            ConnNote::Notice(n) => self.on_notice(n),
        }
    }

    fn on_notice(&mut self, n: Notice) {
        let Some(cur) = &mut self.current else { return };
        match n {
            Notice::Session(session::Event::Connected { .. }) => {
                if cur.phase == Phase::Starting {
                    cur.phase = Phase::Handshaked;
                }
            }
            Notice::Session(session::Event::Control { kind, data }) if kind == msg::CONTROL_KIND => {
                let m = Msg::decode(&data);
                // A Hello this build can't read in full still says its version: refuse it
                // with the right words instead of ignoring it.
                let unreadable_hello = match &m {
                    None => msg::hello_version(&data),
                    _ => None,
                };
                let refusal = match (&m, unreadable_hello) {
                    (_, Some(v)) => msg::negotiate(msg::VersionRange::OURS, msg::VersionRange::from_peer(v, v)).err(),
                    (Some(Msg::Hello(h)), _) => msg::negotiate(msg::VersionRange::OURS, h.versions()).err(),
                    _ => None,
                };
                if let Some(refusal) = refusal
                    && cur.phase == Phase::Handshaked
                {
                    let conn = cur.conn.clone().expect("a handshaked connection has a handle");
                    conn.control(&Msg::Disconnect(refusal));
                    conn.close_after(LINGER);
                    self.current = None;
                    return;
                }
                let Some(m) = m else { return };
                match m {
                    Msg::Hello(hello) if cur.phase == Phase::Handshaked => {
                        let conn = cur.conn.clone().expect("a handshaked connection has a handle");
                        cur.phase = Phase::Live;
                        set_bandwidth_cap(&conn, &hello.settings);
                        if let (Some(from), true) = (&cur.took_over_from, cur.silent_takeover && !cur.resumed) {
                            conn.control(&Msg::TookOver { from: from.clone() });
                        }
                        if let Some((hash, image)) = &self.last_shape {
                            cur.shapes.insert(*hash);
                            send_shape(&conn, *hash, image);
                        }
                        let hello_name = hello.name.clone();
                        let ev = HostEvent::ClientConnected {
                            client: cur.client,
                            name: hello.name.clone(),
                            hello,
                            took_over_from: cur.took_over_from.clone(),
                            resumed: cur.resumed,
                        };
                        let client = cur.client;
                        self.set_live(Some(conn));
                        {
                            let mut p = self.pairings.lock().unwrap();
                            if let Some(d) = p.devices.iter_mut().find(|d| d.id == client) {
                                d.last_connected = Some(unix_now());
                                // The device's own name, unless the desk named it (a bad name never refuses).
                                let n = hello_name.trim();
                                if !d.named_at_desk && (1..=64).contains(&n.len()) && d.name != n {
                                    d.name = n.to_string();
                                }
                            }
                            let _ = p.save(self.store.as_ref());
                        }
                        self.emit(ev);
                    }
                    Msg::Disconnect(reason) => {
                        let client = cur.client;
                        if let Some(c) = &cur.conn {
                            c.close_after(LINGER);
                        }
                        self.current = None;
                        self.set_live(None);
                        self.emit(HostEvent::ClientDisconnected { client, reason });
                    }
                    // Anything else only once the session is Live (after Hello): nothing from the client
                    // reaches the host app before it has a session to apply it to.
                    _ if cur.phase != Phase::Live => {}
                    Msg::Settings(st) => {
                        if let Some(c) = &cur.conn {
                            set_bandwidth_cap(c, &st);
                        }
                        self.emit(HostEvent::Control(Msg::Settings(st)));
                    }
                    m => self.emit(HostEvent::Control(m)),
                }
            }
            Notice::Session(session::Event::Input(b)) if cur.phase == Phase::Live => {
                if let Some((t_us, event)) = InputEvent::decode_at(&b) {
                    let at = cur.conn.as_ref().and_then(|c| c.peer_instant(t_us));
                    self.emit(HostEvent::Input { event, at });
                }
            }
            Notice::Session(session::Event::PeerLostFrame { frame_id }) => self.emit(HostEvent::PeerLostFrame { frame_id }),
            Notice::Session(session::Event::ReceiverReport { frame_id, decode_us, present_us }) => {
                self.emit(HostEvent::ReceiverReport { frame_id, decode_us, present_us })
            }
            Notice::Route(r) => self.emit(HostEvent::Route(route_info(&r))),
            Notice::Stats { stats, .. } => {
                if self.last_hud.elapsed() >= HUD_INTERVAL {
                    self.last_hud = Instant::now();
                    self.emit(HostEvent::Stats(stats));
                }
            }
            _ => {}
        }
    }

    /// End the current client's session cleanly (no hold).
    fn end_current(&mut self, reason: DisconnectReason, tell: bool) {
        let Some(cur) = self.current.take() else { return };
        self.set_live(None);
        if let Some(c) = &cur.conn {
            if tell {
                c.control(&Msg::Disconnect(reason.clone()));
            }
            c.close_after(LINGER);
        }
        self.emit(HostEvent::ClientDisconnected { client: cur.client, reason });
    }
}

enum PairVia {
    Rdv(String),
    Lan(u64),
}

/// The client's bandwidth limit caps the congestion controller.
fn set_bandwidth_cap(c: &ConnHandle, st: &msg::StreamSettings) {
    let bps = match st.bandwidth_limit_kbps {
        0 => crate::cc::CcConfig::default().max_bps,
        k => k as u64 * 1000,
    };
    let _ = c.cmds.send(Command::SetMaxBitrate(bps));
}

fn send_shape(c: &ConnHandle, hash: u64, image: &CursorImage) {
    let _ = c.cmds.send(Command::CursorShape { hash, image: msg::encode(image) });
}

// ===========================================================================
// Client

pub struct ClientConfig {
    pub store: Arc<dyn Store>,
    /// This device's display name ("MacBook Air").
    pub name: String,
    pub rendezvous: Option<RendezvousConfig>,
    /// Browse mDNS for hosts on the LAN.
    pub mdns: bool,
    pub bind: Option<Vec<IpAddr>>,
    pub fps: u32,
    /// How long to keep reconnecting after the connection dies (the host's hold).
    pub hold: Duration,
    /// Share only relayed candidates (see `HostConfig::relay_only`).
    pub relay_only: bool,
    /// Known LAN listeners (host, address), for when mDNS can't be used.
    pub lan_hints: Vec<(DeviceId, SocketAddr)>,
}

impl ClientConfig {
    pub fn new(store: Arc<dyn Store>, name: impl Into<String>) -> Self {
        ClientConfig {
            store,
            name: name.into(),
            rendezvous: None,
            mdns: true,
            bind: None,
            fps: 60,
            hold: HOLD,
            relay_only: false,
            lan_hints: vec![],
        }
    }
}

/// A host with an open pairing window.
#[derive(Debug, Clone, PartialEq)]
pub struct PairingTarget {
    pub host: DeviceId,
    pub name: String,
    /// Found on the LAN (mDNS): the LAN listener.
    pub lan: Option<(Vec<IpAddr>, u16)>,
    /// Found on the rendezvous: its window.
    pub rendezvous: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PairError {
    /// The code didn't match (or someone tampered). The window may take
    /// another try.
    WrongCode,
    /// The window is gone (expired, used, closed, attempts used).
    Closed(String),
    Failed(String),
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PairError {}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectError {
    /// Another device is connected right now: ask "In use by <by>. Take over?"
    Busy { by: String },
    /// Not on the rendezvous and not on the LAN.
    Offline,
    /// This device isn't paired with that host (any more).
    NotPaired,
    Refused(DisconnectReason),
    Timeout,
    Failed(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ConnectError {}

#[derive(Debug, Clone, PartialEq)]
pub struct HostStatus {
    pub host: DeviceId,
    pub online: bool,
    pub on_lan: bool,
    pub last_seen: Option<u64>,
    /// Unix seconds; set while the host is asleep.
    pub asleep_since: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum ClientEvent {
    Rendezvous { online: bool, error: Option<String> },
    /// The handshake finished; `Hello` went out. `Welcome` follows as Control.
    Connected { route: Option<RouteInfo> },
    Control(Msg),
    VideoFrame(ReceivedFrame),
    VideoFrameLost { frame_id: u32 },
    CursorShape { hash: u64, image: CursorImage },
    CursorPosition { x: f32, y: f32, visible: bool, shape: u64 },
    Route(RouteInfo),
    Stats(SessionStats),
    /// The connection died; retrying until the host's hold ends. `since` is
    /// how long ago it died (design.md 7 picks the wording from it).
    Reconnecting { since: Duration },
    /// The session is over.
    Disconnected { reason: DisconnectReason },
    /// `asleep_since` is set while the host is asleep ("Asleep since 23:10").
    Presence { host: DeviceId, online: bool, last_seen: Option<u64>, asleep_since: Option<u64> },
    Revoked { host: DeviceId },
}

enum ClientCmd {
    Connect { host: DeviceId, hello: Hello, takeover: bool, reply: oneshot::Sender<Result<(), ConnectError>> },
    Disconnect,
    /// Where a host's LAN listener is (a bare-address pairing or connect).
    Hint(DeviceId, SocketAddr),
    Send(Msg),
    SetName(String),
    SetRelayOnly(bool),
}

enum ClientIn {
    Cmd(ClientCmd),
    Rdv(RdvNote),
    Dialed { id: u64, result: Result<(EndpointHandle, SigChan), ConnectError>, reply: Option<oneshot::Sender<Result<(), ConnectError>>> },
    Note(u64, ConnNote),
    Retry,
}

#[derive(Clone)]
pub struct Client {
    key: DeviceKey,
    /// This device's display name (`set_name` changes it).
    name: Arc<Mutex<String>>,
    store: Arc<dyn Store>,
    brain: mpsc::UnboundedSender<ClientIn>,
    live: Arc<Mutex<LiveSlot>>,
    pairings: Arc<Mutex<Pairings>>,
    rdv: Arc<RdvShared>,
    browser: Option<Arc<Browser>>,
    pair_route: Arc<Mutex<Option<(String, mpsc::UnboundedSender<rendezvous::Event>)>>>,
}

impl Client {
    pub async fn start(cfg: ClientConfig) -> std::io::Result<(Client, mpsc::UnboundedReceiver<ClientEvent>)> {
        let key = keys::load_or_create_device_key(cfg.store.as_ref())?;
        let pairings = Arc::new(Mutex::new(Pairings::load(cfg.store.as_ref())));
        let (brain_tx, brain_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let rdv = Arc::new(RdvShared {
            cfg: cfg.rendezvous.clone().unwrap_or_else(|| RendezvousConfig::new("")),
            key: key.clone(),
            role: DeviceRole::Client,
            rdv: Mutex::new(None),
            routes: Default::default(),
            stop: AtomicBool::new(cfg.rendezvous.is_none()),
            paused: AtomicBool::new(false),
        });
        let pair_route: Arc<Mutex<Option<(String, mpsc::UnboundedSender<rendezvous::Event>)>>> = Default::default();
        if cfg.rendezvous.is_some() {
            let (ntx, mut nrx) = mpsc::unbounded_channel();
            spawn_rdv_loop(rdv.clone(), ntx);
            let b = brain_tx.clone();
            let pr = pair_route.clone();
            tokio::spawn(async move {
                while let Some(n) = nrx.recv().await {
                    // Pairing traffic goes to the pair() call waiting for it.
                    if let RdvNote::Event(
                        ref e @ (rendezvous::Event::PairMsg { ref pairing, .. } | rendezvous::Event::PairClosed { ref pairing, .. }),
                    ) = n
                    {
                        if let Some((p, tx)) = pr.lock().unwrap().as_ref() {
                            if p == pairing {
                                let _ = tx.send(e.clone());
                            }
                        }
                        continue;
                    }
                    if b.send(ClientIn::Rdv(n)).is_err() {
                        break;
                    }
                }
            });
        }
        let browser = if cfg.mdns {
            match Browser::start() {
                Ok(b) => Some(Arc::new(b)),
                Err(e) => {
                    tracing::warn!("mDNS: {e}");
                    None
                }
            }
        } else {
            None
        };
        let live = Arc::new(Mutex::new(LiveSlot::default()));
        let client = Client {
            key: key.clone(),
            name: Arc::new(Mutex::new(cfg.name.clone())),
            store: cfg.store.clone(),
            brain: brain_tx.clone(),
            live: live.clone(),
            pairings: pairings.clone(),
            rdv: rdv.clone(),
            browser: browser.clone(),
            pair_route,
        };
        let stored_addrs: Vec<(DeviceId, SocketAddr)> = pairings
            .lock()
            .unwrap()
            .devices
            .iter()
            .filter_map(|d| Some((d.id, d.lan_addr.as_deref()?.parse().ok()?)))
            .collect();
        let brain = ClientBrain {
            key,
            name: cfg.name,
            ca_pem: cfg.rendezvous.and_then(|r| r.ca_pem),
            bind: cfg.bind,
            fps: cfg.fps,
            hold: cfg.hold,
            relay_only: cfg.relay_only,
            events: ev_tx,
            me: brain_tx,
            live,
            pairings,
            store: cfg.store.clone(),
            rdv,
            browser,
            target: None,
            conn: None,
            dialing: None,
            route: None,
            // Stored addresses first (where each host's LAN listener was when we paired),
            // then the caller's hints.
            lan_seen: stored_addrs
                .into_iter()
                .chain(cfg.lan_hints.iter().copied())
                .map(|(id, a)| {
                    (id, LanHost { id, name: String::new(), addrs: vec![a.ip()], port: a.port(), pairing: false })
                })
                .collect(),
            last_hud: Instant::now(),
        };
        tokio::spawn(brain.run(brain_rx));
        Ok((client, ev_rx))
    }

    pub fn id(&self) -> DeviceId {
        self.key.id()
    }

    pub fn key(&self) -> &DeviceKey {
        &self.key
    }

    pub fn paired(&self) -> Vec<PairedDevice> {
        self.pairings.lock().unwrap().devices.clone()
    }

    /// Change the name this device shows to computers, live: the next pairing,
    /// the next connection's `Offer` and the `Hello` of every (re)connect carry
    /// it. A computer's paired list follows it at the next connection unless the device
    /// was renamed at the desk.
    pub fn set_name(&self, name: &str) {
        *self.name.lock().unwrap() = name.to_string();
        let _ = self.brain.send(ClientIn::Cmd(ClientCmd::SetName(name.to_string())));
    }

    /// Share only relayed candidates (true) or all of them (false), from the next
    /// connection on: the candidate filter is read when a connection starts, so
    /// the one in progress keeps the choice it began with.
    pub fn set_relay_only(&self, on: bool) {
        let _ = self.brain.send(ClientIn::Cmd(ClientCmd::SetRelayOnly(on)));
    }

    /// Forget a host (after "changed computer key", or by choice).
    /// Keep the current list when saving the removal fails.
    pub fn forget(&self, host: &DeviceId) -> std::io::Result<()> {
        let mut p = self.pairings.lock().unwrap();
        let mut next = p.clone();
        if next.remove(host) {
            next.save(self.store.as_ref())?;
            *p = next;
        }
        Ok(())
    }

    fn rdv(&self) -> Option<Rendezvous> {
        self.rdv.rdv.lock().unwrap().clone()
    }

    /// Whether a rendezvous server is configured at all.
    pub fn has_rendezvous(&self) -> bool {
        !self.rdv.cfg.url.is_empty()
    }

    /// Wait until the rendezvous is connected (or `timeout`). False at once when
    /// no server is configured.
    pub async fn wait_rendezvous(&self, timeout: Duration) -> bool {
        if !self.has_rendezvous() {
            return false;
        }
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            if self.rdv().is_some() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// Hosts with an open pairing window, from mDNS and the rendezvous
    /// (normally exactly one).
    pub async fn discover_pairing(&self, timeout: Duration) -> Vec<PairingTarget> {
        let mut out: Vec<PairingTarget> = Vec::new();
        if let Some(b) = &self.browser {
            b.wait_for(timeout, |h| h.pairing).await;
            for h in b.hosts().into_iter().filter(|h| h.pairing) {
                out.push(PairingTarget { host: h.id, name: h.name, lan: Some((h.addrs, h.port)), rendezvous: None });
            }
        }
        if let Some(rdv) = self.rdv() {
            if let Ok(ws) = rdv.pair_discover().await {
                for w in ws {
                    match out.iter_mut().find(|t| t.host == w.host) {
                        Some(t) => t.rendezvous = Some(w.pairing),
                        None => out.push(PairingTarget { host: w.host, name: w.name, lan: None, rendezvous: Some(w.pairing) }),
                    }
                }
            }
        }
        out
    }

    /// Run SPAKE2 with the typed code against a target, over the LAN when it
    /// was seen there, else through the rendezvous. On success both sides
    /// have pinned each other.
    pub async fn pair(&self, target: &PairingTarget, code: &str) -> Result<PairedDevice, PairError> {
        if !pairing::valid_code(code) {
            return Err(PairError::WrongCode);
        }
        let (attempt, m1) = ClientAttempt::start(self.key.clone(), self.name.lock().unwrap().clone(), target.host, code);
        let (id, name, lan) = if let Some((addrs, port)) = &target.lan {
            let (id, name, at) = self.pair_lan(attempt, m1, addrs, *port).await?;
            (id, name, Some(at))
        } else if let Some(p) = &target.rendezvous {
            let (id, name) = self.pair_rdv(attempt, m1, p).await?;
            (id, name, None)
        } else {
            return Err(PairError::Closed("not found".into()));
        };
        Ok(self.store_pairing(id, name, lan))
    }

    /// Pair with the host whose LAN listener is at `addr`, knowing nothing else about it:
    /// the host's ID comes out of the exchange, bound by the code (`pairing.rs`, "By
    /// address"), and is what gets stored. The address is remembered for connecting.
    pub async fn pair_by_address(&self, addr: SocketAddr, code: &str) -> Result<PairedDevice, PairError> {
        if !pairing::valid_code(code) {
            return Err(PairError::WrongCode);
        }
        let (attempt, m1) = ClientAttempt::start_by_address(self.key.clone(), self.name.lock().unwrap().clone(), code);
        let (id, name, at) = self.pair_lan(attempt, m1, &[addr.ip()], addr.port()).await?;
        Ok(self.store_pairing(id, name, Some(at)))
    }

    fn store_pairing(&self, id: DeviceId, name: String, lan: Option<SocketAddr>) -> PairedDevice {
        let dev = PairedDevice {
            id,
            name,
            paired_at: unix_now(),
            last_connected: None,
            named_at_desk: false,
            lan_addr: lan.map(|a| a.to_string()),
        };
        let mut p = self.pairings.lock().unwrap();
        p.upsert(dev.clone());
        if let Some(a) = lan {
            p.set_lan_addr(&id, &a.to_string());
        }
        let _ = p.save(self.store.as_ref());
        if let Some(a) = lan {
            let _ = self.brain.send(ClientIn::Cmd(ClientCmd::Hint(id, a)));
        }
        dev
    }

    /// The paired host whose LAN listener is at `addr` (as stored when it was paired
    /// over the LAN); with one paired host that has no stored address, that one. Never
    /// an ID read from the network: the connection then authenticates the stored ID.
    pub fn host_at(&self, addr: SocketAddr) -> Option<DeviceId> {
        let p = self.pairings.lock().unwrap();
        let stored = |d: &&PairedDevice| d.lan_addr.as_deref().and_then(|s| s.parse::<SocketAddr>().ok()) == Some(addr);
        match p.devices.iter().find(stored) {
            Some(d) => Some(d.id),
            None => match p.devices.as_slice() {
                [d] if d.lan_addr.is_none() => Some(d.id),
                _ => None,
            },
        }
    }

    /// Connect to a paired host at a bare address: the ID stored for it (`host_at`) is
    /// dialled through that address.
    pub async fn connect_by_address(&self, addr: SocketAddr, hello: Hello, takeover: bool) -> Result<(), ConnectError> {
        let host = self.host_at(addr).ok_or(ConnectError::NotPaired)?;
        self.hint_lan(host, addr);
        self.connect(host, hello, takeover).await
    }

    /// Tell the core where a paired host's LAN listener is, for when mDNS can't find it.
    pub fn hint_lan(&self, host: DeviceId, addr: SocketAddr) {
        let _ = self.brain.send(ClientIn::Cmd(ClientCmd::Hint(host, addr)));
    }

    async fn pair_lan(&self, mut a: ClientAttempt, m1: Vec<u8>, addrs: &[IpAddr], port: u16) -> Result<(DeviceId, String, SocketAddr), PairError> {
        let mut stream = None;
        let mut at = None;
        for ip in addrs {
            if let Ok(Ok(s)) = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect((*ip, port))).await {
                stream = Some(s);
                at = Some(SocketAddr::new(*ip, port));
                break;
            }
        }
        let at = at.unwrap_or(SocketAddr::new(IpAddr::from([0, 0, 0, 0]), port));
        let mut s = stream.ok_or_else(|| PairError::Failed("the host's LAN listener didn't answer".into()))?;
        let io = |e: std::io::Error| PairError::Failed(e.to_string());
        mdns::write_frame(&mut s, &LanFrame::PairHello).await.map_err(io)?;
        mdns::write_frame(&mut s, &LanFrame::Pair(m1)).await.map_err(io)?;
        loop {
            let f = tokio::time::timeout(SIGNAL_TIMEOUT, mdns::read_frame(&mut s))
                .await
                .map_err(|_| PairError::Failed("timed out".into()))?;
            match f {
                Some(LanFrame::Pair(b)) => match a.handle(&b) {
                    Step::Send(b) => mdns::write_frame(&mut s, &LanFrame::Pair(b)).await.map_err(io)?,
                    Step::Paired { id, name, .. } => return Ok((id, name, at)),
                    Step::Failed { reply } => {
                        if let Some(b) = reply {
                            let _ = mdns::write_frame(&mut s, &LanFrame::Pair(b)).await;
                        }
                        return Err(PairError::WrongCode);
                    }
                    Step::Ignore => {}
                },
                Some(LanFrame::PairClosed) | None => return Err(PairError::Closed("closed".into())),
                Some(_) => {}
            }
        }
    }

    async fn pair_rdv(&self, mut a: ClientAttempt, m1: Vec<u8>, pairing: &str) -> Result<(DeviceId, String), PairError> {
        let rdv = self.rdv().ok_or_else(|| PairError::Failed("the rendezvous is offline".into()))?;
        let (tx, mut rx) = mpsc::unbounded_channel();
        *self.pair_route.lock().unwrap() = Some((pairing.to_string(), tx));
        let r = async {
            let host = rdv.pair_join(pairing).await.map_err(|e| match e.code() {
                Some("not_found") | Some("too_many") => PairError::Closed(e.to_string()),
                _ => PairError::Failed(e.to_string()),
            })?;
            let _ = host; // The SPAKE2 transcript checks the host's identity.
            rdv.pair_msg(pairing, &m1).await.map_err(|e| PairError::Failed(e.to_string()))?;
            loop {
                let ev = tokio::time::timeout(SIGNAL_TIMEOUT, rx.recv())
                    .await
                    .map_err(|_| PairError::Failed("timed out".into()))?
                    .ok_or_else(|| PairError::Failed("rendezvous gone".into()))?;
                match ev {
                    rendezvous::Event::PairMsg { blob, .. } => match a.handle(&blob) {
                        Step::Send(b) => rdv.pair_msg(pairing, &b).await.map_err(|e| PairError::Failed(e.to_string()))?,
                        Step::Paired { id, name, .. } => return Ok((id, name)),
                        Step::Failed { reply } => {
                            if let Some(b) = reply {
                                let _ = rdv.pair_msg(pairing, &b).await;
                            }
                            return Err(PairError::WrongCode);
                        }
                        Step::Ignore => {}
                    },
                    rendezvous::Event::PairClosed { reason, .. } => {
                        return Err(if reason == "failed" { PairError::WrongCode } else { PairError::Closed(reason) });
                    }
                    _ => {}
                }
            }
        }
        .await;
        *self.pair_route.lock().unwrap() = None;
        if r.is_err() {
            let _ = rdv.pair_close(pairing).await;
        }
        r
    }

    /// Paired hosts' status: on the rendezvous and on the LAN.
    pub async fn presence(&self) -> Vec<HostStatus> {
        let ids: Vec<DeviceId> = self.paired().iter().map(|d| d.id).collect();
        let mut out: Vec<HostStatus> =
            ids.iter().map(|h| HostStatus { host: *h, online: false, on_lan: false, last_seen: None, asleep_since: None }).collect();
        if let Some(rdv) = self.rdv() {
            if let Ok(ps) = rdv.presence(&ids).await {
                for p in ps {
                    if let Some(s) = out.iter_mut().find(|s| s.host == p.host) {
                        s.online = p.state == HostState::Online;
                        s.last_seen = p.last_seen;
                        s.asleep_since = if p.state == HostState::Asleep { p.asleep_since } else { None };
                    }
                }
            }
        }
        if let Some(b) = &self.browser {
            for h in b.hosts() {
                if let Some(s) = out.iter_mut().find(|s| s.host == h.id) {
                    s.on_lan = true;
                }
            }
        }
        out
    }

    /// Connect to a paired host. `Busy` means another device is connected
    /// right now: ask, then call again with `takeover`. After `Ok`, progress
    /// comes as events; the core sends `hello` on every (re)connect, with the
    /// display and settings kept up to date by `send(Resize/Settings)`.
    pub async fn connect(&self, host: DeviceId, hello: Hello, takeover: bool) -> Result<(), ConnectError> {
        let (tx, rx) = oneshot::channel();
        self.brain
            .send(ClientIn::Cmd(ClientCmd::Connect { host, hello, takeover, reply: tx }))
            .map_err(|_| ConnectError::Failed("client stopped".into()))?;
        rx.await.unwrap_or(Err(ConnectError::Failed("client stopped".into())))
    }

    fn live(&self) -> Option<ConnHandle> {
        self.live.lock().unwrap().conn.clone()
    }

    pub fn send(&self, m: Msg) {
        let _ = self.brain.send(ClientIn::Cmd(ClientCmd::Send(m)));
    }

    /// An input event that happened now (the core stamps it on the session clock).
    pub fn send_input(&self, ev: &InputEvent) -> bool {
        self.live().map(|c| c.cmds.send(Command::Input(ev.encode_at(c.now_us()))).is_ok()).unwrap_or(false)
    }

    /// An input event that happened at `at` (the platform's event time), so the
    /// host keeps the user's spacing for double clicks and kinetic scrolling.
    pub fn send_input_at(&self, ev: &InputEvent, at: Instant) -> bool {
        self.live()
            .map(|c| {
                let t = at.checked_duration_since(c.epoch).map_or(c.now_us(), |d| d.as_micros() as u64);
                c.cmds.send(Command::Input(ev.encode_at(t))).is_ok()
            })
            .unwrap_or(false)
    }

    /// The live session's clock origin: frames' µs times count from here.
    /// Every (re)connection starts a new session clock.
    pub fn session_epoch(&self) -> Option<Instant> {
        self.live().map(|c| c.epoch)
    }

    pub fn send_receiver_report(&self, frame_id: u32, decode_us: u32, present_us: u32) {
        if let Some(c) = self.live() {
            let _ = c.cmds.send(Command::ReceiverReport { frame_id, decode_us, present_us });
        }
    }

    pub fn disconnect(&self) {
        let _ = self.brain.send(ClientIn::Cmd(ClientCmd::Disconnect));
    }

    /// Microphone: one Opus packet; `capture_us` on the session clock.
    pub fn send_mic(&self, timestamp: u32, capture_us: Option<u64>, opus: Vec<u8>) {
        if let Some(c) = self.live() {
            let _ = c.cmds.send(Command::Audio { mic: true, timestamp, capture_us, data: opus });
        }
    }

    /// Desktop audio, jitter-buffered: the audio callback pops 10 ms at a time.
    pub fn audio_buffer(&self) -> Option<Arc<Mutex<JitterBuffer>>> {
        self.live().map(|c| c.audio_out.clone())
    }

    /// Health, route and hedge route ("Reconnecting… · relay SG").
    pub fn link_status(&self) -> Option<LinkStatus> {
        self.live().map(|c| c.status.borrow().clone())
    }

    /// NWPathMonitor (or similar) saw a network change: re-gather now.
    pub fn network_changed(&self) {
        if let Some(c) = self.live() {
            let _ = c.cmds.send(Command::NetworkChanged);
        }
    }

    /// Forget this device on the rendezvous (tests, "reset").
    /// The rendezvous loop stops first, so the device isn't registered again.
    pub async fn unregister(&self) -> Result<(), RdvError> {
        self.rdv.stop.store(true, Ordering::Relaxed);
        match self.rdv() {
            Some(r) => r.unregister().await,
            None => Err(RdvError::Io("not connected".into())),
        }
    }

    pub async fn shutdown(&self) {
        self.rdv.stop.store(true, Ordering::Relaxed);
        self.disconnect();
        tokio::time::sleep(LINGER).await;
        if let Some(r) = self.rdv.rdv.lock().unwrap().take() {
            r.close();
        }
    }
}

struct Target {
    host: DeviceId,
    hello: Hello,
    /// The last `KeyTable` sent: it goes again right after `Hello` on every
    /// (re)connect, like the display and settings inside `Hello`.
    key_table: Option<Msg>,
    /// Set while reconnecting.
    lost_since: Option<Instant>,
}

struct ClientBrain {
    key: DeviceKey,
    name: String,
    /// The pinned CA for TURN over TLS (None: Web PKI).
    ca_pem: Option<Vec<u8>>,
    bind: Option<Vec<IpAddr>>,
    fps: u32,
    hold: Duration,
    relay_only: bool,
    events: mpsc::UnboundedSender<ClientEvent>,
    me: mpsc::UnboundedSender<ClientIn>,
    live: Arc<Mutex<LiveSlot>>,
    pairings: Arc<Mutex<Pairings>>,
    store: Arc<dyn Store>,
    rdv: Arc<RdvShared>,
    browser: Option<Arc<Browser>>,
    target: Option<Target>,
    conn: Option<(ConnHandle, bool)>,
    dialing: Option<u64>,
    route: Option<RouteInfo>,
    /// Where each host was last seen on the LAN: used when mDNS has lost it
    /// for a moment (a dropped link, a Wi-Fi roam).
    lan_seen: HashMap<DeviceId, LanHost>,
    last_hud: Instant,
}

impl ClientBrain {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<ClientIn>) {
        while let Some(m) = rx.recv().await {
            self.handle(m);
        }
        if let Some((c, _)) = self.conn.take() {
            let _ = c.cmds.send(Command::Close);
        }
    }

    fn emit(&self, e: ClientEvent) {
        let _ = self.events.send(e);
    }

    fn set_live(&self, c: Option<ConnHandle>) {
        self.live.lock().unwrap().conn = c;
    }

    fn handle(&mut self, m: ClientIn) {
        match m {
            ClientIn::Cmd(ClientCmd::Connect { host, hello, takeover, reply }) => {
                if self.pairings.lock().unwrap().get(&host).is_none() {
                    let _ = reply.send(Err(ConnectError::NotPaired));
                    return;
                }
                self.drop_conn();
                self.target = Some(Target { host, hello, key_table: None, lost_since: None });
                self.dial(takeover, Some(reply));
            }
            ClientIn::Cmd(ClientCmd::Disconnect) => {
                if let Some((c, _)) = self.conn.take() {
                    c.control(&Msg::Disconnect(DisconnectReason::User));
                    c.close_after(LINGER);
                }
                self.set_live(None);
                self.dialing = None;
                if self.target.take().is_some() {
                    self.emit(ClientEvent::Disconnected { reason: DisconnectReason::User });
                }
            }
            ClientIn::Cmd(ClientCmd::Hint(id, a)) => {
                self.lan_seen.insert(id, LanHost { id, name: String::new(), addrs: vec![a.ip()], port: a.port(), pairing: false });
            }
            ClientIn::Cmd(ClientCmd::SetName(name)) => self.name = name,
            ClientIn::Cmd(ClientCmd::SetRelayOnly(on)) => self.relay_only = on,
            ClientIn::Cmd(ClientCmd::Send(m)) => {
                if let Some(t) = &mut self.target {
                    match &m {
                        Msg::Resize(d) => t.hello.display = *d,
                        Msg::Settings(s) => t.hello.settings = *s,
                        Msg::KeyTable(_) => t.key_table = Some(m.clone()),
                        _ => {}
                    }
                }
                if let Some((c, true)) = &self.conn {
                    c.control(&m);
                }
            }
            ClientIn::Rdv(RdvNote::Up(_)) => self.emit(ClientEvent::Rendezvous { online: true, error: None }),
            ClientIn::Rdv(RdvNote::Down(error)) => self.emit(ClientEvent::Rendezvous { online: false, error }),
            ClientIn::Rdv(RdvNote::Event(e)) => match e {
                rendezvous::Event::Presence { host, state, last_seen, asleep_since } => {
                    let asleep_since = if state == HostState::Asleep { asleep_since } else { None };
                    self.emit(ClientEvent::Presence { host, online: state == HostState::Online, last_seen, asleep_since })
                }
                rendezvous::Event::Revoked { host } => self.emit(ClientEvent::Revoked { host }),
                _ => {}
            },
            ClientIn::Dialed { id, result, reply } => self.on_dialed(id, result, reply),
            ClientIn::Note(id, note) => self.on_note(id, note),
            ClientIn::Retry => {
                if self.target.as_ref().is_some_and(|t| t.lost_since.is_some()) && self.conn.is_none() && self.dialing.is_none() {
                    self.dial(false, None);
                }
            }
        }
    }

    fn drop_conn(&mut self) {
        if let Some((c, _)) = self.conn.take() {
            let _ = c.cmds.send(Command::Close);
        }
        self.set_live(None);
        self.dialing = None;
    }

    fn dial(&mut self, takeover: bool, reply: Option<oneshot::Sender<Result<(), ConnectError>>>) {
        let Some(t) = &self.target else { return };
        let id = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
        self.dialing = Some(id);
        let host = t.host;
        let rdv = self.rdv.rdv.lock().unwrap().clone();
        let lan = self.browser.as_ref().and_then(|b| b.find(&host));
        if let Some(h) = &lan {
            self.lan_seen.insert(host, h.clone());
        }
        let lan = lan.or_else(|| self.lan_seen.get(&host).cloned());
        let ctx = DialCtx {
            key: self.key.clone(),
            name: self.name.clone(),
            host,
            takeover,
            rdv,
            routes: self.rdv.routes.clone(),
            lan,
            browser: self.browser.clone(),
            bind: bind_addrs(&self.bind),
            ca_pem: self.ca_pem.clone(),
            fps: self.fps,
        };
        let me = self.me.clone();
        tokio::spawn(async move {
            let result = dial(ctx).await;
            let _ = me.send(ClientIn::Dialed { id, result, reply });
        });
    }

    fn on_dialed(
        &mut self,
        id: u64,
        result: Result<(EndpointHandle, SigChan), ConnectError>,
        reply: Option<oneshot::Sender<Result<(), ConnectError>>>,
    ) {
        if self.dialing != Some(id) {
            if let Ok((ep, _)) = result {
                let _ = ep.commands.send(Command::Close);
            }
            if let Some(r) = reply {
                let _ = r.send(Err(ConnectError::Failed("superseded".into())));
            }
            return;
        }
        self.dialing = None;
        match result {
            Ok((ep, chan)) => {
                let (handle, resignal) = ConnHandle::new(id, &ep);
                spawn_pump(&handle, resignal, ep, chan, self.me.clone(), ClientIn::Note, self.relay_only);
                self.conn = Some((handle, false));
                if let Some(r) = reply {
                    let _ = r.send(Ok(()));
                }
            }
            Err(e) => {
                if let Some(r) = reply {
                    // The first attempt: the caller decides.
                    self.target = None;
                    let _ = r.send(Err(e));
                    return;
                }
                // A reconnect attempt.
                let reconnecting = self.target.as_ref().and_then(|t| t.lost_since);
                match (e, reconnecting) {
                    (ConnectError::Busy { by }, _) => self.finish(DisconnectReason::TakenOver { by }),
                    (ConnectError::NotPaired, _) | (ConnectError::Refused(DisconnectReason::Revoked), _) => {
                        self.finish(DisconnectReason::Revoked)
                    }
                    (ConnectError::Refused(r), _) => self.finish(r),
                    (e, Some(since)) => {
                        if since.elapsed() >= self.hold {
                            self.finish(DisconnectReason::HoldExpired);
                        } else {
                            tracing::debug!("reconnect failed: {e}");
                            // The loss itself was already reported.
                            if since.elapsed() >= RETRY / 2 {
                                self.emit(ClientEvent::Reconnecting { since: since.elapsed() });
                            }
                            self.retry_later();
                        }
                    }
                    (e, None) => self.finish(DisconnectReason::Error(e.to_string())),
                }
            }
        }
    }

    fn retry_later(&self) {
        let me = self.me.clone();
        tokio::spawn(async move {
            tokio::time::sleep(RETRY).await;
            let _ = me.send(ClientIn::Retry);
        });
    }

    fn finish(&mut self, reason: DisconnectReason) {
        self.drop_conn();
        if self.target.take().is_some() {
            self.emit(ClientEvent::Disconnected { reason });
        }
    }

    fn on_note(&mut self, id: u64, note: ConnNote) {
        let Some((c, connected)) = &mut self.conn else { return };
        if c.id != id {
            return;
        }
        match note {
            ConnNote::Lost(why, last_rx) => {
                tracing::info!("connection lost: {why}");
                let was_connected = *connected;
                self.conn = None;
                self.set_live(None);
                let Some(t) = &mut self.target else { return };
                // The timeline counts from the last packet, not from when the
                // loss was declared.
                let since = *t.lost_since.get_or_insert(if was_connected { last_rx } else { Instant::now() });
                if !was_connected && since.elapsed() >= self.hold {
                    self.finish(DisconnectReason::HoldExpired);
                    return;
                }
                self.emit(ClientEvent::Reconnecting { since: since.elapsed() });
                self.dial(false, None);
            }
            ConnNote::Notice(n) => match n {
                Notice::Session(session::Event::Connected { .. }) => {
                    *connected = true;
                    let c = c.clone();
                    if let Some(t) = &mut self.target {
                        t.lost_since = None;
                        let mut hello = t.hello.clone();
                        hello.name = self.name.clone();
                        // Whatever the app filled in, this build speaks its own range.
                        hello.version = msg::VERSION;
                        hello.min_version = msg::MIN_VERSION;
                        c.control(&Msg::Hello(hello));
                        if let Some(k) = &t.key_table {
                            c.control(k);
                        }
                    }
                    self.set_live(Some(c));
                    self.emit(ClientEvent::Connected { route: self.route.clone() });
                }
                Notice::Session(session::Event::Control { kind, data }) if kind == msg::CONTROL_KIND => {
                    let Some(m) = Msg::decode(&data) else { return };
                    match &m {
                        Msg::Disconnect(r) => {
                            let r = r.clone();
                            self.emit(ClientEvent::Control(m));
                            self.finish(r);
                        }
                        Msg::Welcome(w) => {
                            self.on_welcome(&w.name);
                            self.emit(ClientEvent::Control(m));
                        }
                        _ => self.emit(ClientEvent::Control(m)),
                    }
                }
                Notice::Session(session::Event::CursorShape { hash, data }) => {
                    // A malformed image (a side over 256, or pixels that don't match) is dropped here, so no
                    // platform layer ever sizes a buffer from it.
                    if let Some(image) = msg::decode::<CursorImage>(&data).filter(CursorImage::is_valid) {
                        self.emit(ClientEvent::CursorShape { hash, image });
                    }
                }
                Notice::Session(session::Event::CursorPosition { x, y, visible, shape }) => {
                    self.emit(ClientEvent::CursorPosition { x, y, visible, shape })
                }
                Notice::Session(session::Event::VideoFrame(f)) => self.emit(ClientEvent::VideoFrame(f)),
                Notice::Session(session::Event::VideoFrameLost { frame_id }) => {
                    self.emit(ClientEvent::VideoFrameLost { frame_id })
                }
                Notice::Route(r) => {
                    let info = route_info(&r);
                    self.route = Some(info.clone());
                    self.emit(ClientEvent::Route(info));
                }
                Notice::Stats { stats, .. } => {
                    if self.last_hud.elapsed() >= HUD_INTERVAL {
                        self.last_hud = Instant::now();
                        self.emit(ClientEvent::Stats(stats));
                    }
                }
                Notice::NetworkChanged { added, removed } => {
                    tracing::info!("network changed (+{added:?} -{removed:?}): reopening signalling");
                    let c = c.clone();
                    self.reopen_signalling(c);
                }
                _ => {}
            },
        }
    }

    /// `Welcome` arrived: the connection is real. Stamp the host's `last_connected`, take the host's
    /// own name from it (the name pairing saved isn't covered by the code, so a server or LAN neighbour
    /// could have changed it; `Welcome` comes over the authenticated session), and when the media
    /// runs over the LAN, remember that host's LAN listener address (and drop it from any other host).
    fn on_welcome(&mut self, host_name: &str) {
        let Some(host) = self.target.as_ref().map(|t| t.host) else { return };
        let lan = match &self.route {
            Some(r) if r.kind == PathKind::Lan => {
                let seen = self.browser.as_ref().and_then(|b| b.find(&host)).or_else(|| self.lan_seen.get(&host).cloned());
                seen.and_then(|h| h.addrs.iter().find(|ip| **ip == r.remote.ip()).map(|ip| SocketAddr::new(*ip, h.port)))
            }
            _ => None,
        };
        let mut p = self.pairings.lock().unwrap();
        if let Some(d) = p.devices.iter_mut().find(|d| d.id == host) {
            d.last_connected = Some(unix_now());
            let n = host_name.trim();
            if !d.named_at_desk && (1..=64).contains(&n.len()) && d.name != n {
                d.name = n.to_string();
            }
        }
        if let Some(a) = lan {
            p.set_lan_addr(&host, &a.to_string());
        }
        let _ = p.save(self.store.as_ref());
    }

    /// After a network change the signalling session usually died with the old
    /// address (and in-band candidates only help while a route survives): open
    /// a new one, through the rendezvous or the host's LAN listener, and hand
    /// it to the connection's pump, which sends every local candidate on it.
    fn reopen_signalling(&self, conn: ConnHandle) {
        let Some(t) = &self.target else { return };
        let host = t.host;
        let (rdv, key) = (self.rdv.clone(), self.key.clone());
        let lan = self.browser.as_ref().and_then(|b| b.find(&host)).or_else(|| self.lan_seen.get(&host).cloned());
        tokio::spawn(async move {
            // The rendezvous reconnects from the new address by itself; give it a moment.
            let end = Instant::now() + Duration::from_secs(5);
            let mut chan = None;
            while chan.is_none() && Instant::now() < end {
                let r = rdv.rdv.lock().unwrap().clone();
                if let Some(r) = r {
                    if let Ok(Ok(session)) = tokio::time::timeout(Duration::from_secs(2), r.connect_host(&host)).await {
                        chan = Some(rdv_chan(r, &rdv.routes, session, key.clone(), host));
                        break;
                    }
                }
                if let Some(h) = &lan {
                    for ip in &h.addrs {
                        if let Ok(Ok(s)) = tokio::time::timeout(Duration::from_secs(2), TcpStream::connect((*ip, h.port))).await {
                            let _ = s.set_nodelay(true);
                            if let Some(c) = lan_signal(s, key.clone(), host).await {
                                chan = Some(c);
                                break;
                            }
                        }
                    }
                }
                if chan.is_none() {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
            match chan {
                Some(ch) => {
                    let _ = conn.resignal.send(ch);
                }
                None => tracing::info!("couldn't reopen signalling; in-band candidates only"),
            }
        });
    }
}

struct DialCtx {
    key: DeviceKey,
    name: String,
    host: DeviceId,
    takeover: bool,
    rdv: Option<Rendezvous>,
    routes: Routes,
    lan: Option<LanHost>,
    browser: Option<Arc<Browser>>,
    bind: Vec<SocketAddr>,
    /// The pinned CA for TURN over TLS (None: Web PKI).
    ca_pem: Option<Vec<u8>>,
    fps: u32,
}

/// Signalling, ICE credentials, the offer, and the host's answer.
async fn dial(ctx: DialCtx) -> Result<(EndpointHandle, SigChan), ConnectError> {
    // The signalling path: the rendezvous when the host is online there, else
    // the LAN listener.
    let mut chan = None;
    let mut via_rdv = None;
    if let Some(rdv) = &ctx.rdv {
        match rdv.connect_host(&ctx.host).await {
            Ok(session) => {
                chan = Some(rdv_chan(rdv.clone(), &ctx.routes, session, ctx.key.clone(), ctx.host));
                via_rdv = Some(rdv.clone());
            }
            Err(e) if e.code() == Some("not_authorised") && ctx.lan.is_none() => return Err(ConnectError::NotPaired),
            Err(e) => tracing::debug!("rendezvous connect: {e}"),
        }
    }
    if chan.is_none() {
        // Not on the rendezvous: the LAN. mDNS may need a moment after start.
        let mut lan = ctx.lan.clone();
        if lan.is_none() {
            if let Some(b) = &ctx.browser {
                lan = b.wait_for(Duration::from_secs(3), |h| h.id == ctx.host && !h.addrs.is_empty()).await;
            }
        }
        if let Some(h) = &lan {
            for ip in &h.addrs {
                if let Ok(Ok(s)) = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect((*ip, h.port))).await {
                    let _ = s.set_nodelay(true);
                    if let Some(c) = lan_signal(s, ctx.key.clone(), ctx.host).await {
                        chan = Some(c);
                        break;
                    }
                }
            }
        }
    }
    let mut chan = chan.ok_or(ConnectError::Offline)?;
    let path = ice_servers(via_rdv.as_ref(), ctx.ca_pem.as_deref(), true).await;
    let (ep, ufrag, pass) =
        start_endpoint(Role::Client, &ctx.key, ctx.host, path, ctx.bind, ctx.fps).await.map_err(ConnectError::Failed)?;
    chan.send(&Signal::Offer(msg::Offer {
        version: msg::VERSION,
        name: ctx.name.clone(),
        takeover: ctx.takeover,
        ice_ufrag: ufrag,
        ice_pass: pass,
        candidates: vec![],
        min_version: msg::MIN_VERSION,
        sent_us: msg::Offer::now_us(),
    }));
    let close = |ep: &EndpointHandle| {
        let _ = ep.commands.send(Command::Close);
    };
    loop {
        match tokio::time::timeout(SIGNAL_TIMEOUT, chan.recv()).await {
            Err(_) => {
                close(&ep);
                return Err(ConnectError::Timeout);
            }
            Ok(None) => {
                close(&ep);
                return Err(ConnectError::Offline);
            }
            Ok(Some(Signal::Answer(a))) => {
                let _ = ep.commands.send(Command::RemoteCredentials { ufrag: a.ice_ufrag, pass: a.ice_pass });
                for c in a.candidates {
                    let _ = ep.commands.send(Command::RemoteCandidate(c));
                }
                return Ok((ep, chan));
            }
            Ok(Some(Signal::Candidate(c))) => {
                let _ = ep.commands.send(Command::RemoteCandidate(c));
            }
            Ok(Some(Signal::Busy { by })) => {
                close(&ep);
                return Err(ConnectError::Busy { by });
            }
            Ok(Some(Signal::Refused(r))) => {
                close(&ep);
                return Err(match r {
                    DisconnectReason::Revoked => ConnectError::NotPaired,
                    r => ConnectError::Refused(r),
                });
            }
            Ok(Some(Signal::Offer(_))) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_networks_parse_and_match_by_prefix() {
        let v = parse_named_networks("[networks]\n\"198.51.100.0/24\" = \"Office VPN\"\n\"fd7a::/16\" = \"Mesh\"\n").unwrap();
        let office = v.iter().find(|n| n.label == "Office VPN").unwrap();
        assert!(office.contains("198.51.100.200".parse().unwrap()));
        assert!(office.contains("::ffff:198.51.100.9".parse().unwrap()));
        assert!(!office.contains("203.0.113.1".parse().unwrap()));
        assert!(v.iter().find(|n| n.label == "Mesh").unwrap().contains("fd7a::1".parse().unwrap()));
        assert!(parse_named_networks("[networks]\n\"198.51.100.0/33\" = \"x\"\n").is_err());
        assert!(parse_named_networks("[networks]\n\"198.51.100.0/8\" = \"\"\n").is_err());
        assert_eq!(parse_named_networks("").unwrap(), vec![]);
    }
}
