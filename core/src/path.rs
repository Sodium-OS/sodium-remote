// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The path: candidate gathering, ICE and route selection, sans-IO.
//!
//! - **ICE agent:** `is`, str0m's standalone agent (full RFC 8445, both roles,
//!   nomination that may change without an ICE restart). See docs/reference/protocol.md.
//! - **Server-reflexive candidates:** a minimal STUN binding client here.
//! - **Relayed candidates:** `turn-client-proto` allocations over UDP, TCP and
//!   TLS-over-TCP. For TLS we wrap rustls around the TCP client here (the
//!   `turn-client-rustls` 0.1.2 adapter drops bytes on large reads). The
//!   relayed transport is always UDP.
//! - **Routes:** every STUN check that gets an answer proves a route (a local
//!   base and a remote address). We time each answer, so every route has an RTT
//!   and a last-heard time. ICE keeps checking every pair at least every 500 ms
//!   (`max_rto`), which doubles as consent freshness (RFC 7675's idea): a route
//!   that stops answering for 1.5 s is dead. Media goes on the best live route:
//!   the first one that works (aggressive), then any live route whose score
//!   beats the current one by 25%. The two sides choose independently;
//!   Remote's packets are authenticated end to end and bound to no address, so
//!   asymmetric routes are fine.
//! - **Score** (milestone 1): the windowed minimum RTT, relay penalties, a loss
//!   penalty, and for the side that sends video a throughput penalty when the
//!   route's measured capacity is below the stream's demand. Capacity comes
//!   from the congestion controller's C_est on the active route and from probe
//!   trains on the others (`poll_train`).
//! - **Failover**: the session judges the active route;
//!   `on_suspect` rechecks every pair and picks a hedge route for duplicates,
//!   `fail_selected` gives the active route up after 700 ms.
//! - **Network changes:** `add_udp_socket` and `remove_local` re-gather on a
//!   new or vanished address while the session carries on.

use is::{Candidate, CandidateKind, IceAgent, IceAgentEvent, IceConnectionState, IceCreds};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use turn_client_proto::api::{TurnClientApi, TurnConfig, TurnEvent, TurnPollRet, TurnRecvRet};
use turn_client_proto::tcp::TurnClientTcp;
use turn_client_proto::types::{TransportType, TurnCredentials};
use turn_client_proto::udp::TurnClientUdp;

turn_client_proto::impl_client!(pub TurnClient, (Udp, TurnClientUdp), (Tcp, TurnClientTcp));

type TurnInstant = turn_client_proto::types::Instant;

const ROUTE_DEAD: Duration = Duration::from_millis(1500);
/// Score penalties on top of RTT: a UDP relay costs a hop; a relay reached
/// over TCP or TLS adds head-of-line blocking and TCP's own congestion control
/// under our rate control, so it is the last resort.
const RELAY_UDP_PENALTY: Duration = Duration::from_millis(3);
const RELAY_TCP_PENALTY: Duration = Duration::from_millis(25);
const RELAY_TLS_PENALTY: Duration = Duration::from_millis(30);
/// Switch only to a route at least 25% and 1 ms better.
const SWITCH_MARGIN: f64 = 0.75;
const SWITCH_MIN_GAIN: f64 = 0.001;
/// While no direct route is in use, failed pairs are formed again this often, so
/// a path that opens later (a firewall, a NAT mapping, a VPN coming up) is found.
const RECHECK: Duration = Duration::from_secs(5);
const MIN_RTT_WINDOW: Duration = Duration::from_secs(10);
/// A check unanswered this long counts as lost.
const CHECK_LOST_AFTER: Duration = Duration::from_secs(2);
/// Loss is judged over this window, once there are `LOSS_MIN_SAMPLES` samples.
const LOSS_WINDOW: Duration = Duration::from_secs(20);
const LOSS_MIN_SAMPLES: u32 = 20;
/// Score cost of loss above `LOSS_FREE`: each 1% costs 2 ms (FEC overhead and
/// the frames it misses). Below 2% FEC absorbs it, as in congestion control,
/// and a brief outage (one or two lost checks) doesn't read as a lossy path.
const LOSS_WEIGHT_S: f64 = 0.2;
const LOSS_FREE: f64 = 0.02;
/// Score cost of capacity below demand: the extra time a frame of the
/// demanded size spends on the wire, one frame interval per multiple short.
const FRAME_S: f64 = 1.0 / 60.0;
/// The stream a video sender would like (bps); see `set_demand_bps`.
pub const DEFAULT_DEMAND_BPS: u64 = 50_000_000;
/// Probe trains re-measure a route this often...
const TRAIN_INTERVAL: Duration = Duration::from_secs(30);
/// ...and one train at most is on the wire at a time, this far apart: trains
/// to several routes often cross the same access link as the video.
const TRAIN_GAP: Duration = Duration::from_secs(1);
/// After a switch, media stays put this long unless the route dies or a
/// better class of route (direct over relay, UDP over TCP/TLS) turns up.
const SETTLE: Duration = Duration::from_secs(2);
/// A capacity the controller measured outranks probe trains this long.
const CC_CAPACITY_WINS: Duration = Duration::from_secs(60);
/// The active route's capacity is its highest C_est over this window: C_est
/// dips while the controller recovers or shares the link (with a train, say),
/// and a train elsewhere reads the link's full rate.
const CC_WINDOW: Duration = Duration::from_secs(10);
const STUN_COOKIE: u32 = 0x2112_A442;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transport {
    Udp,
    Tcp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnTransport {
    Udp,
    Tcp,
    /// TLS over TCP (to port 443 for networks that only pass HTTPS).
    Tls,
}

#[derive(Clone, Debug)]
pub struct TurnServer {
    pub addr: SocketAddr,
    pub transport: TurnTransport,
    /// For TLS: the name on the server's certificate.
    pub server_name: String,
    pub username: String,
    pub password: String,
}

#[derive(Clone)]
pub struct PathConfig {
    /// The client controls nomination.
    pub controlling: bool,
    pub stun_servers: Vec<SocketAddr>,
    pub turn_servers: Vec<TurnServer>,
    pub tls: Option<Arc<rustls::ClientConfig>>,
}

/// A datagram (or a stream write, for TCP) the driver must send.
#[derive(Debug, Clone)]
pub struct Transmit {
    pub transport: Transport,
    pub from: SocketAddr,
    pub to: SocketAddr,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteKind {
    Direct,
    /// Through our TURN allocation on this server, reached over this transport.
    RelayedLocal { server: SocketAddr, transport: TurnTransport },
    /// To the peer's relayed address; the transport the peer reaches its TURN
    /// server over is read from the candidate's local preference (our convention).
    RelayedRemote { transport: TurnTransport },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub local: SocketAddr,
    pub remote: SocketAddr,
    pub kind: RouteKind,
    pub rtt: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PathEvent {
    /// Send this candidate to the peer (trickle).
    LocalCandidate(String),
    RouteChanged(Route),
    /// No live route left.
    RouteLost,
    IceState(String),
}

/// A route's measured bottleneck capacity.
#[derive(Clone, Copy, Debug)]
struct Capacity {
    bps: u64,
    at: Instant,
    /// Measured by congestion control (the active route), not a train.
    cc: bool,
    /// For a train: the active route's C_est when it ran. If the active route
    /// now measures well below that, the drop may be on an access link both
    /// share, and the train is stale until it runs again.
    active_then: Option<u64>,
}

struct RouteInfo {
    /// When the first check on this route was answered.
    created: Instant,
    last_ok: Instant,
    /// The session gave this route up; a check answered after this revives it.
    failed_at: Option<Instant>,
    capacity: Option<Capacity>,
    /// C_est samples while active, for the windowed maximum.
    cc_window: VecDeque<(Instant, u64)>,
    /// (when, lost, total) from ICE checks and probe trains.
    loss: VecDeque<(Instant, u32, u32)>,
    /// The last check was lost: a run of lost checks is one outage, which
    /// liveness handles, so it counts as one loss.
    in_loss_run: bool,
    /// The last time the session suspected this route (start, end): checks
    /// lost in it are the outage, not the path's loss rate.
    outage: Option<(Instant, Option<Instant>)>,
    next_train: Instant,
    srtt: Duration,
    /// False while the RTT is only a guess (a provisional route).
    measured: bool,
    /// RTT samples of the last `MIN_RTT_WINDOW`. Routes are compared on their
    /// minimum: queueing on a shared bottleneck inflates every route's smoothed
    /// RTT at once and must not make the path flap.
    samples: VecDeque<(Instant, Duration)>,
}

impl RouteInfo {
    fn new(now: Instant, rtt: Duration, measured: bool) -> Self {
        RouteInfo {
            created: now,
            last_ok: now,
            failed_at: None,
            capacity: None,
            cc_window: VecDeque::new(),
            loss: VecDeque::new(),
            in_loss_run: false,
            outage: None,
            next_train: now,
            srtt: rtt,
            measured,
            samples: VecDeque::from([(now, rtt)]),
        }
    }
    fn alive(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_ok) < ROUTE_DEAD && self.failed_at.is_none_or(|f| self.last_ok > f)
    }
    fn add_loss(&mut self, now: Instant, lost: u32, total: u32) {
        self.loss.push_back((now, lost, total));
        while self.loss.front().is_some_and(|l| now.saturating_duration_since(l.0) > LOSS_WINDOW) {
            self.loss.pop_front();
        }
    }
    /// Loss over the window, once there are enough samples.
    fn loss_rate(&self) -> Option<f64> {
        let (lost, total) = self.loss.iter().fold((0, 0), |(l, t), s| (l + s.1, t + s.2));
        (total >= LOSS_MIN_SAMPLES).then(|| lost as f64 / total as f64)
    }
    fn min_rtt(&self) -> Duration {
        self.samples.iter().map(|s| s.1).min().unwrap_or(self.srtt)
    }
}

struct TurnEntry {
    server: TurnServer,
    client: TurnClient,
    relayed: Option<SocketAddr>,
    channels: Vec<SocketAddr>,
    /// Peers we asked a permission for (the request may still be in flight).
    permissions: Vec<IpAddr>,
    /// TLS around the TCP client's byte stream (TURN over TLS).
    tls: Option<rustls::ClientConnection>,
}

impl TurnEntry {
    /// A TURN client's outgoing bytes, through TLS when this server uses it.
    fn emit(&mut self, out: &mut VecDeque<Transmit>, from: SocketAddr, to: SocketAddr, tcp: bool, data: &[u8]) {
        let data = match self.tls.as_mut() {
            None => data.to_vec(),
            Some(tls) => {
                use std::io::Write;
                if tls.writer().write_all(data).is_err() {
                    return;
                }
                tls_ciphertext(tls)
            }
        };
        if !data.is_empty() {
            out.push_back(Transmit { transport: if tcp { Transport::Tcp } else { Transport::Udp }, from, to, data });
        }
    }
}

fn tls_ciphertext(tls: &mut rustls::ClientConnection) -> Vec<u8> {
    let mut out = Vec::new();
    while tls.wants_write() {
        if tls.write_tls(&mut out).is_err() {
            break;
        }
    }
    out
}

/// Feed ciphertext from the stream; returns all plaintext now readable.
fn tls_plaintext(tls: &mut rustls::ClientConnection, mut data: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut plain = Vec::new();
    loop {
        if !data.is_empty() {
            tls.read_tls(&mut data).map_err(|e| e.to_string())?;
        }
        let state = tls.process_new_packets().map_err(|e| e.to_string())?;
        let n = state.plaintext_bytes_to_read();
        if n > 0 {
            let start = plain.len();
            plain.resize(start + n, 0);
            tls.reader().read_exact(&mut plain[start..]).map_err(|e| e.to_string())?;
        }
        if data.is_empty() {
            return Ok(plain);
        }
    }
}

struct StunQuery {
    base: SocketAddr,
    server: SocketAddr,
    txid: [u8; 12],
    next: Instant,
    tries: u32,
    done: bool,
}

pub struct PathAgent {
    cfg: PathConfig,
    epoch: Instant,
    ice: IceAgent,
    turns: Vec<TurnEntry>,
    stun: Vec<StunQuery>,
    /// Our outstanding ICE checks: transaction id → (local, remote, sent).
    checks: HashMap<[u8; 12], (SocketAddr, SocketAddr, Instant)>,
    routes: HashMap<(SocketAddr, SocketAddr), RouteInfo>,
    selected: Option<(SocketAddr, SocketAddr)>,
    /// While the session hedges: the route duplicates go on.
    hedge: Option<(SocketAddr, SocketAddr)>,
    /// Probe trains in flight: id → route.
    trains: HashMap<u32, ((SocketAddr, SocketAddr), Instant)>,
    /// What a frame stream from this side wants (0: throughput doesn't matter).
    demand_bps: u64,
    last_train: Option<Instant>,
    last_switch: Option<Instant>,
    /// The last burst of re-measuring stale trains: at most one per
    /// `TRAIN_INTERVAL`, since the trains cross the congested link too.
    restale: Option<Instant>,
    out: VecDeque<Transmit>,
    events: VecDeque<PathEvent>,
    relay_hints: Arc<Mutex<HashMap<SocketAddr, TurnTransport>>>,
    next_recheck: Instant,
}

/// Local preference of relayed candidates, by how we reach the TURN server.
/// The peer decodes it from the candidate priority, so a host never sends video
/// to a client's TLS allocation when a UDP one works (RFC 8445 leaves the
/// local preference to the agent; both ends here are ours).
fn relay_local_preference(t: TurnTransport) -> u32 {
    match t {
        TurnTransport::Udp => 16_000,
        TurnTransport::Tcp => 8_000,
        TurnTransport::Tls => 4_000,
    }
}

fn relay_transport_from_priority(prio: u32) -> TurnTransport {
    match (prio >> 8) & 0xFFFF {
        p if p > 12_000 => TurnTransport::Udp,
        p if p > 6_000 => TurnTransport::Tcp,
        _ => TurnTransport::Tls,
    }
}

fn route_kind(turns: &[TurnEntry], ice: &IceAgent, local: SocketAddr, remote: SocketAddr) -> RouteKind {
    if let Some(t) = turns.iter().find(|t| t.relayed == Some(local)) {
        return RouteKind::RelayedLocal { server: t.server.addr, transport: t.server.transport };
    }
    if let Some(c) = ice.remote_candidates().find(|c| c.addr() == remote && c.kind() == CandidateKind::Relayed) {
        return RouteKind::RelayedRemote { transport: relay_transport_from_priority(c.prio()) };
    }
    RouteKind::Direct
}

impl PathAgent {
    pub fn new(cfg: PathConfig, now: Instant) -> Self {
        let mut ice = IceAgent::new(IceCreds::new());
        ice.set_controlling(cfg.controlling);
        ice.set_initial_stun_rto(Duration::from_millis(100));
        ice.set_max_stun_rto(Duration::from_millis(500));
        ice.set_timing_advance(Duration::from_millis(20));
        let relay_hints: Arc<Mutex<HashMap<SocketAddr, TurnTransport>>> = Default::default();
        let hints = relay_hints.clone();
        ice.set_local_preference(move |c: &Candidate, same_kind: usize| {
            if c.kind() == CandidateKind::Relayed {
                let t = hints.lock().unwrap().get(&c.addr()).copied().unwrap_or(TurnTransport::Udp);
                relay_local_preference(t) - (2 * same_kind as u32).min(2_000)
            } else {
                is::default_local_preference(c, same_kind)
            }
        });
        PathAgent {
            cfg,
            epoch: now,
            ice,
            turns: Vec::new(),
            stun: Vec::new(),
            checks: HashMap::new(),
            routes: HashMap::new(),
            selected: None,
            hedge: None,
            trains: HashMap::new(),
            demand_bps: 0,
            last_train: None,
            last_switch: None,
            restale: None,
            out: VecDeque::new(),
            events: VecDeque::new(),
            relay_hints,
            next_recheck: now + RECHECK,
        }
    }

    fn turn_now(&self, now: Instant) -> TurnInstant {
        TurnInstant::from_nanos(now.saturating_duration_since(self.epoch).as_nanos() as i64)
    }

    pub fn local_credentials(&self) -> (String, String) {
        let c = self.ice.local_credentials();
        (c.ufrag.clone(), c.pass.clone())
    }

    pub fn set_remote_credentials(&mut self, ufrag: &str, pass: &str) {
        self.ice.set_remote_credentials(IceCreds { ufrag: ufrag.into(), pass: pass.into() });
    }

    pub fn add_remote_candidate(&mut self, sdp: &str) -> Result<(), String> {
        let c = Candidate::from_sdp_string(sdp).map_err(|e| e.to_string())?;
        self.ice.add_remote_candidate(c);
        Ok(())
    }

    fn add_local(&mut self, c: Candidate) {
        if let Some(c) = self.ice.add_local_candidate(c) {
            self.events.push_back(PathEvent::LocalCandidate(c.to_sdp_string()));
        }
    }

    /// A bound UDP socket: a host candidate, STUN queries and UDP TURN
    /// allocations from it.
    pub fn add_udp_socket(&mut self, now: Instant, local: SocketAddr) {
        if let Ok(c) = Candidate::host(local, "udp") {
            self.add_local(c);
        }
        for &server in &self.cfg.stun_servers {
            self.stun.push(StunQuery { base: local, server, txid: rand::random(), next: now, tries: 0, done: false });
        }
        let udp_turns: Vec<TurnServer> =
            self.cfg.turn_servers.iter().filter(|t| t.transport == TurnTransport::Udp).cloned().collect();
        for server in udp_turns {
            let client = TurnClientUdp::allocate(local, server.addr, self.turn_config(&server));
            self.turns.push(TurnEntry { server, client: TurnClient::Udp(client), relayed: None, channels: Vec::new(), permissions: Vec::new(), tls: None });
        }
    }

    /// TURN servers reached over a stream; the driver connects these and calls
    /// `add_turn_stream`.
    pub fn stream_turn_servers(&self) -> Vec<TurnServer> {
        self.cfg.turn_servers.iter().filter(|t| t.transport != TurnTransport::Udp).cloned().collect()
    }

    fn turn_config(&self, s: &TurnServer) -> TurnConfig {
        TurnConfig::new(TurnCredentials::new(&s.username, &s.password))
    }

    /// The driver connected a TCP stream to a TURN server.
    pub fn add_turn_stream(&mut self, server: &TurnServer, local: SocketAddr) -> Result<(), String> {
        let cfg = self.turn_config(server);
        let tls = match server.transport {
            TurnTransport::Tcp => None,
            TurnTransport::Tls => {
                let config = self.cfg.tls.clone().ok_or("no TLS config")?;
                let name = rustls::pki_types::ServerName::try_from(server.server_name.clone())
                    .map_err(|e| e.to_string())?;
                Some(rustls::ClientConnection::new(config, name).map_err(|e| e.to_string())?)
            }
            TurnTransport::Udp => return Err("UDP TURN needs no stream".into()),
        };
        let client = TurnClient::Tcp(TurnClientTcp::allocate(local, server.addr, cfg));
        let mut entry = TurnEntry { server: server.clone(), client, relayed: None, channels: Vec::new(), permissions: Vec::new(), tls };
        if let Some(tls) = entry.tls.as_mut() {
            let hello = tls_ciphertext(tls); // ClientHello
            self.out.push_back(Transmit { transport: Transport::Tcp, from: local, to: server.addr, data: hello });
        }
        self.turns.push(entry);
        Ok(())
    }

    pub fn ice_state(&self) -> IceConnectionState {
        self.ice.state()
    }

    pub fn selected_route(&self) -> Option<Route> {
        self.route(self.selected?)
    }

    fn route(&self, (l, r): (SocketAddr, SocketAddr)) -> Option<Route> {
        let info = self.routes.get(&(l, r))?;
        Some(Route { local: l, remote: r, kind: route_kind(&self.turns, &self.ice, l, r), rtt: info.srtt })
    }

    /// The route duplicates go on while the session hedges.
    pub fn hedge_route(&self) -> Option<Route> {
        self.route(self.hedge?)
    }

    /// Live routes, with what we know about each: (route, capacity bps, loss).
    pub fn live_routes(&self, now: Instant) -> Vec<(Route, Option<u64>, Option<f64>)> {
        self.routes
            .iter()
            .filter(|(_, i)| i.alive(now))
            .filter_map(|(k, i)| Some((self.route(*k)?, i.capacity.map(|c| c.bps), i.loss_rate())))
            .collect()
    }

    /// How much throughput matters when choosing routes: the rate a video
    /// sender would like (`DEFAULT_DEMAND_BPS`, or the user's cap if lower).
    /// 0 (the default, right for a client) leaves it out, and no probe trains run.
    pub fn set_demand_bps(&mut self, bps: u64) {
        self.demand_bps = bps;
    }

    /// The congestion controller's C_est for the active route (None: no
    /// bottleneck measured there).
    pub fn note_capacity(&mut self, now: Instant, c_est: Option<u64>) {
        let Some(key) = self.selected else { return };
        let Some(info) = self.routes.get_mut(&key) else { return };
        match c_est {
            Some(bps) => {
                if info.cc_window.back().is_none_or(|s| s.1 != bps) {
                    info.cc_window.push_back((now, bps));
                }
                while info.cc_window.front().is_some_and(|s| now.saturating_duration_since(s.0) > CC_WINDOW) && info.cc_window.len() > 1 {
                    info.cc_window.pop_front();
                }
                let bps = info.cc_window.iter().map(|s| s.1).max().unwrap_or(bps);
                info.capacity = Some(Capacity { bps, at: now, cc: true, active_then: None });
            }
            None if info.capacity.is_some_and(|c| c.cc) => {
                info.capacity = None;
                info.cc_window.clear();
            }
            None => {}
        }
    }

    /// The active route's C_est, if congestion control holds one.
    fn active_cc(&self) -> Option<u64> {
        self.routes.get(&self.selected?)?.capacity.filter(|c| c.cc).map(|c| c.bps)
    }

    /// A train from before the active route's capacity dropped (see `Capacity`).
    fn stale(&self, c: &Capacity) -> bool {
        !c.cc && self.active_cc().is_some_and(|now| c.active_then.is_none_or(|then| (now as f64) < 0.9 * then as f64))
    }

    /// A route due for a probe train (the session must be up): send
    /// `Session::start_probe_train` on it and report the id to `train_started`.
    ///
    /// Only routes that could win are measured, by the switching test itself:
    /// one whose score without a throughput penalty beats the active route's
    /// by the switching margin and by `SWITCH_MIN_GAIN`,
    /// and that isn't a TCP/TLS relay while a UDP route works.
    pub fn poll_train(&mut self, now: Instant) -> Option<(SocketAddr, SocketAddr)> {
        if self.demand_bps == 0 || self.hedge.is_some() || self.last_train.is_some_and(|t| now < t + TRAIN_GAP) {
            return None;
        }
        let selected = self.selected?;
        let (cur, cur_class) = (self.score(selected), self.class(selected));
        // Stale trains run again in one burst at most every TRAIN_INTERVAL.
        let restale = self.restale.is_none_or(|t| now < t + Duration::from_secs(5) || now >= t + TRAIN_INTERVAL);
        let key = self
            .routes
            .iter()
            .filter(|(k, i)| {
                let stale = restale && i.capacity.is_some_and(|c| self.stale(&c));
                **k != selected && i.alive(now) && i.measured && (now >= i.next_train || stale)
            })
            .filter(|(k, _)| {
                let base = self.base_score(**k);
                base < cur * SWITCH_MARGIN && cur - base > SWITCH_MIN_GAIN && !(self.class(**k) == 2 && cur_class < 2)
            })
            .min_by_key(|(_, i)| i.next_train)
            .map(|(k, _)| *k)?;
        if self.routes[&key].capacity.is_some_and(|c| self.stale(&c)) && self.restale.is_none_or(|t| now >= t + TRAIN_INTERVAL) {
            self.restale = Some(now);
        }
        let info = self.routes.get_mut(&key)?;
        info.next_train = now + TRAIN_INTERVAL;
        info.capacity = info.capacity.filter(|c| c.cc); // a fresh train replaces it
        self.last_train = Some(now);
        Some(key)
    }

    pub fn train_started(&mut self, now: Instant, id: u32, route: (SocketAddr, SocketAddr)) {
        self.trains.retain(|_, (_, t)| now.saturating_duration_since(*t) < Duration::from_secs(5));
        self.trains.insert(id, (route, now));
    }

    /// Send one probe-train datagram on its route.
    pub fn send_train(&mut self, now: Instant, id: u32, data: &[u8]) {
        if let Some(((l, r), _)) = self.trains.get(&id).copied() {
            self.send_from(now, l, r, data);
        }
    }

    pub fn on_train_report(&mut self, now: Instant, id: u32, rate_bps: u64, loss: f64) {
        let Some((key, _)) = self.trains.remove(&id) else { return };
        tracing::info!("train on {} -> {}: {:.1} Mbps, loss {:.1}%", key.0, key.1, rate_bps as f64 / 1e6, loss * 100.0);
        let Some(info) = self.routes.get_mut(&key) else { return };
        let cc_fresh = info.capacity.is_some_and(|c| c.cc && now.saturating_duration_since(c.at) < CC_CAPACITY_WINS);
        let active_then = self.active_cc();
        let Some(info) = self.routes.get_mut(&key) else { return };
        if !cc_fresh {
            info.capacity = Some(Capacity { bps: rate_bps, at: now, cc: false, active_then });
        }
        let total = crate::train::TRAIN_LEN as u32;
        info.add_loss(now, (loss * total as f64).round() as u32, total);
        self.reselect(now);
    }

    /// The session suspects the active route: recheck every pair now and
    /// pick the route duplicates go on.
    pub fn on_suspect(&mut self, now: Instant) {
        if let Some(info) = self.selected.and_then(|k| self.routes.get_mut(&k)) {
            info.outage = Some((now.checked_sub(crate::health::SUSPECT_AFTER).unwrap_or(now), None));
        }
        if self.ice.remote_credentials().is_some() {
            self.ice.recreate_candidate_pairs();
        }
        self.hedge = self.best_other(now);
    }

    /// The active route answered: stop duplicating.
    pub fn on_recovered(&mut self, now: Instant) {
        self.end_outage(now);
        self.hedge = None;
    }

    fn end_outage(&mut self, now: Instant) {
        if let Some(o) = self.selected.and_then(|k| self.routes.get_mut(&k)).and_then(|i| i.outage.as_mut()) {
            o.1.get_or_insert(now);
        }
    }

    /// The session gave the active route up (700 ms without an answer): move
    /// to the best live route. The route comes back when a check succeeds again.
    pub fn fail_selected(&mut self, now: Instant) {
        self.end_outage(now);
        if let Some(info) = self.selected.and_then(|k| self.routes.get_mut(&k)) {
            info.failed_at = Some(now);
        }
        self.hedge = None;
        self.reselect(now);
    }

    /// The measured capacity of the hedge route, if any.
    pub fn hedge_capacity(&self) -> Option<u64> {
        self.routes.get(&self.hedge?)?.capacity.map(|c| c.bps)
    }

    /// The best live route other than the active one, preferring one to a
    /// different remote address (a different path, not just another socket).
    fn best_other(&self, now: Instant) -> Option<(SocketAddr, SocketAddr)> {
        let sel = self.selected;
        self.routes
            .iter()
            .filter(|(k, i)| Some(**k) != sel && i.alive(now))
            .map(|(k, _)| *k)
            .min_by(|a, b| {
                let same = |k: &(SocketAddr, SocketAddr)| sel.is_some_and(|s| s.1 == k.1);
                same(a).cmp(&same(b)).then(self.score(*a).total_cmp(&self.score(*b)))
            })
    }

    /// Send a duplicate on the hedge route (no-op without one).
    pub fn send_hedge(&mut self, now: Instant, data: &[u8]) {
        if self.hedge.is_some_and(|k| !self.routes.get(&k).is_some_and(|i| i.alive(now))) {
            self.hedge = self.best_other(now);
        }
        if let Some((l, r)) = self.hedge {
            self.send_from(now, l, r, data);
        }
    }

    /// A local address went away (a network change): drop its candidates,
    /// routes, STUN queries and TURN allocations, and choose again.
    pub fn remove_local(&mut self, now: Instant, ip: IpAddr) {
        let gone: Vec<Candidate> = self.ice.local_candidates().filter(|c| c.local().ip() == ip || c.addr().ip() == ip).collect();
        for c in &gone {
            self.ice.invalidate_candidate(c);
        }
        let dead_relays: Vec<SocketAddr> =
            self.turns.iter().filter(|t| t.client.local_addr().ip() == ip).filter_map(|t| t.relayed).collect();
        self.turns.retain(|t| t.client.local_addr().ip() != ip);
        let on_ip = |l: &SocketAddr| l.ip() == ip || dead_relays.contains(l);
        self.routes.retain(|(l, _), _| !on_ip(l));
        self.checks.retain(|_, (l, _, _)| !on_ip(l));
        self.stun.retain(|q| q.base.ip() != ip);
        if self.hedge.is_some_and(|(l, _)| on_ip(&l)) {
            self.hedge = None;
        }
        self.reselect(now);
    }

    /// TURN stream servers whose connection used a vanished local address
    /// (the driver reconnects them after `remove_local`).
    pub fn turn_streams_on(&self, ip: IpAddr) -> Vec<TurnServer> {
        self.turns
            .iter()
            .filter(|t| t.server.transport != TurnTransport::Udp && t.client.local_addr().ip() == ip)
            .map(|t| t.server.clone())
            .collect()
    }

    // ---- receive -----------------------------------------------------------

    /// Handle bytes from the network. Returns Remote datagrams for the session.
    pub fn handle_receive(
        &mut self,
        now: Instant,
        transport: Transport,
        local: SocketAddr,
        from: SocketAddr,
        data: &[u8],
    ) -> Vec<Vec<u8>> {
        let mut app = Vec::new();
        if transport == Transport::Udp && self.handle_stun_reply(local, from, data) {
            return app;
        }
        let tnow = self.turn_now(now);
        let tt = match transport {
            Transport::Udp => TransportType::Udp,
            Transport::Tcp => TransportType::Tcp,
        };
        if let Some(i) = self
            .turns
            .iter()
            .position(|t| t.client.local_addr() == local && t.client.remote_addr() == from && t.client.transport() == tt)
        {
            let mut peer_data: Vec<(SocketAddr, Vec<u8>)> = Vec::new();
            let relayed = self.turns[i].relayed;
            let entry = &mut self.turns[i];
            let plain;
            let data = match entry.tls.as_mut() {
                None => data,
                Some(tls) => {
                    plain = match tls_plaintext(tls, data) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!("TURN TLS to {from}: {e}");
                            return app;
                        }
                    };
                    // Handshake replies and anything else rustls wants to say.
                    let reply = tls_ciphertext(tls);
                    if !reply.is_empty() {
                        self.out.push_back(Transmit { transport: Transport::Tcp, from: local, to: from, data: reply });
                    }
                    &plain[..]
                }
            };
            let client = &mut self.turns[i].client;
            if data.is_empty() {
                self.pump_turn(now);
                return app;
            }
            let t = turn_client_proto::stun::agent::Transmit { data, transport: tt, from, to: local };
            if let TurnRecvRet::PeerData(p) = client.recv(t, tnow) {
                peer_data.push((p.peer, p.data().to_vec()));
            }
            while let Some(p) = client.poll_recv(tnow) {
                peer_data.push((p.peer, p.data().to_vec()));
            }
            self.pump_turn(now);
            if let Some(relayed) = relayed {
                for (peer, d) in peer_data {
                    app.extend(self.handle_peer(now, relayed, peer, &d));
                }
            }
            return app;
        }
        if transport == Transport::Udp {
            app.extend(self.handle_peer(now, local, from, data));
        }
        app
    }

    /// A datagram from the peer, on a local base (a socket or a relayed address).
    fn handle_peer(&mut self, now: Instant, local: SocketAddr, from: SocketAddr, data: &[u8]) -> Vec<Vec<u8>> {
        let Some(&first) = data.first() else { return Vec::new() };
        if crate::wire::kind::is_remote(first) {
            return vec![data.to_vec()];
        }
        if first < 4 {
            if let Ok(msg) = is::stun::StunMessage::parse(data) {
                // Integrity-checked with the ICE passwords before it counts.
                if !self.ice.accepts_message(&msg) {
                    return Vec::new();
                }
                if msg.is_successful_binding_response() {
                    let txid: [u8; 12] = data[8..20].try_into().unwrap();
                    if let Some((l, r, sent)) = self.checks.remove(&txid) {
                        if l == local && r == from {
                            self.route_ok(now, l, r, now.saturating_duration_since(sent));
                            if let Some(info) = self.routes.get_mut(&(l, r)) {
                                info.add_loss(now, 0, 1);
                                info.in_loss_run = false;
                            }
                        }
                    }
                } else if msg.is_binding_request() && self.selected.is_none() && !self.routes.contains_key(&(local, from)) {
                    // The peer reached us and knows the password: answer on this
                    // route until our own check measures one (RTT unknown, so
                    // a pessimistic 50 ms that any measured route beats).
                    self.routes.insert((local, from), RouteInfo::new(now, Duration::from_millis(50), false));
                    self.reselect(now);
                }
                let packet = is::stun::StunPacket { proto: is::Protocol::Udp, source: from, destination: local, message: msg };
                self.ice.handle_packet(now, packet);
                self.pump_ice(now);
            }
        }
        Vec::new()
    }

    fn handle_stun_reply(&mut self, local: SocketAddr, from: SocketAddr, data: &[u8]) -> bool {
        if data.len() < 20 || data[0] != 0x01 || data[1] != 0x01 {
            return false;
        }
        let txid: [u8; 12] = data[8..20].try_into().unwrap();
        let Some(q) = self.stun.iter_mut().find(|q| q.txid == txid && q.server == from && q.base == local) else {
            return false;
        };
        q.done = true;
        let base = q.base;
        if let Some(mapped) = parse_xor_mapped(data) {
            if mapped != base {
                if let Ok(c) = Candidate::server_reflexive(mapped, base, "udp") {
                    self.add_local(c);
                }
            }
        }
        true
    }

    fn route_ok(&mut self, now: Instant, local: SocketAddr, remote: SocketAddr, rtt: Duration) {
        let info = self.routes.entry((local, remote)).or_insert_with(|| RouteInfo::new(now, rtt, true));
        info.last_ok = now;
        if info.failed_at.is_some_and(|f| now > f) {
            info.failed_at = None;
        }
        info.srtt = if info.measured { info.srtt.mul_f64(0.75) + rtt.mul_f64(0.25) } else { rtt };
        if !info.measured {
            info.samples.clear();
        }
        info.measured = true;
        info.samples.push_back((now, rtt));
        while info.samples.front().is_some_and(|s| now.saturating_duration_since(s.0) > MIN_RTT_WINDOW) {
            info.samples.pop_front();
        }
        self.reselect(now);
    }

    /// Lower is better: the windowed minimum RTT, a penalty for each relay leg
    /// (ours and the peer's), for loss, and for capacity below demand.
    fn score(&self, key: (SocketAddr, SocketAddr)) -> f64 {
        let info = &self.routes[&key];
        let mut s = self.base_score(key);
        if let (Some(cap), true) = (info.capacity, self.demand_bps > 0) {
            s += FRAME_S * (self.demand_bps as f64 / cap.bps.max(1) as f64 - 1.0).max(0.0);
        }
        s
    }

    /// A route's score against the active one. Routes through the same relay
    /// host share its bottleneck, so throughput can't tell them apart: such a
    /// rival carries the active route's throughput penalty, and RTT, loss and
    /// class decide.
    fn rival_score(&self, key: (SocketAddr, SocketAddr), cur: Option<(SocketAddr, SocketAddr)>) -> f64 {
        match cur {
            Some(cur) if key != cur && self.same_relay(key, cur) => {
                self.base_score(key) + (self.score(cur) - self.base_score(cur))
            }
            _ => self.score(key),
        }
    }

    /// Do two routes pass through a relay at the same address (either leg)?
    fn same_relay(&self, a: (SocketAddr, SocketAddr), b: (SocketAddr, SocketAddr)) -> bool {
        let ips = |k: (SocketAddr, SocketAddr)| {
            let [ours, theirs] = self.legs(k);
            [ours.map(|_| k.0.ip()), theirs.map(|_| k.1.ip())]
        };
        let (x, y) = (ips(a), ips(b));
        x.iter().flatten().any(|ip| y.iter().flatten().any(|o| o == ip))
    }

    /// The relay transports on a route: ours and the peer's leg.
    fn legs(&self, key: (SocketAddr, SocketAddr)) -> [Option<TurnTransport>; 2] {
        let ours = self.turns.iter().find(|t| t.relayed == Some(key.0)).map(|t| t.server.transport);
        let theirs = self
            .ice
            .remote_candidates()
            .find(|c| c.addr() == key.1 && c.kind() == CandidateKind::Relayed)
            .map(|c| relay_transport_from_priority(c.prio()));
        [ours, theirs]
    }

    /// 0 direct, 1 through UDP relays, 2 through a TCP or TLS relay.
    fn class(&self, key: (SocketAddr, SocketAddr)) -> u8 {
        self.legs(key).iter().flatten().map(|t| if *t == TurnTransport::Udp { 1 } else { 2 }).max().unwrap_or(0)
    }

    /// The score without the throughput penalty: RTT, relay legs, loss.
    fn base_score(&self, key: (SocketAddr, SocketAddr)) -> f64 {
        let leg = |t: TurnTransport| match t {
            TurnTransport::Udp => RELAY_UDP_PENALTY,
            TurnTransport::Tcp => RELAY_TCP_PENALTY,
            TurnTransport::Tls => RELAY_TLS_PENALTY,
        };
        let relays: Duration = self.legs(key).iter().flatten().map(|t| leg(*t)).sum();
        let info = &self.routes[&key];
        let mut s = (info.min_rtt() + relays).as_secs_f64();
        s += LOSS_WEIGHT_S * (info.loss_rate().unwrap_or(0.0) - LOSS_FREE).max(0.0);
        s
    }

    /// Throughput matters here and nothing has measured this route yet.
    fn unmeasured(&self, key: (SocketAddr, SocketAddr)) -> bool {
        self.demand_bps > 0 && self.routes[&key].capacity.is_none_or(|c| self.stale(&c))
    }

    fn reselect(&mut self, now: Instant) {
        let alive: Vec<(SocketAddr, SocketAddr)> =
            self.routes.iter().filter(|(_, i)| i.alive(now)).map(|(k, _)| *k).collect();
        let current_alive = self.selected.filter(|s| alive.contains(s));
        // Leaving a working route (a guessed RTT doesn't count as working):
        // - only for one a probe train or the controller has measured, when
        //   throughput matters (trains start as soon as a route could win);
        // - never for a TCP/TLS relay while a UDP route works;
        // - within or down a class only once the last switch has settled.
        let settled = self.last_switch.is_none_or(|t| now >= t + SETTLE)
            || current_alive.is_some_and(|c| !self.routes[&c].measured);
        let cur_class = current_alive.map(|c| self.class(c));
        let best = alive
            .iter()
            .copied()
            .filter(|k| {
                let Some(cur) = current_alive else { return true };
                if *k == cur {
                    return true;
                }
                let class = self.class(*k);
                let cur_class = cur_class.unwrap_or(2);
                !self.unmeasured(*k) && !(class == 2 && cur_class < 2) && (class < cur_class || settled)
            })
            .min_by(|a, b| self.rival_score(*a, current_alive).total_cmp(&self.rival_score(*b, current_alive)));
        let next = match (current_alive, best) {
            (Some(cur), Some(b))
                if self.rival_score(b, Some(cur)) < self.score(cur) * SWITCH_MARGIN
                    && self.score(cur) - self.rival_score(b, Some(cur)) > SWITCH_MIN_GAIN =>
            {
                Some(b)
            }
            (Some(cur), _) => Some(cur),
            (None, b) => b,
        };
        if next != self.selected {
            self.selected = next;
            self.last_switch = Some(now);
            match self.selected_route() {
                Some(route) => {
                    if let RouteKind::RelayedLocal { .. } = route.kind {
                        self.bind_channel(now, route.local, route.remote);
                    }
                    self.events.push_back(PathEvent::RouteChanged(route));
                }
                None => self.events.push_back(PathEvent::RouteLost),
            }
        }
    }

    /// ChannelData (4 bytes of overhead instead of 36) on UDP relays only:
    /// turn-types 0.7.3 neither pads ChannelData to 4 bytes over TCP/TLS nor
    /// skips the server's padding (RFC 8656 section 12.5), which desyncs the
    /// stream. Stream relays keep Send/Data indications, which are aligned.
    fn bind_channel(&mut self, now: Instant, relayed: SocketAddr, peer: SocketAddr) {
        let tnow = self.turn_now(now);
        if let Some(t) = self.turns.iter_mut().find(|t| t.relayed == Some(relayed) && t.server.transport == TurnTransport::Udp) {
            if !t.channels.contains(&peer) && t.client.bind_channel(TransportType::Udp, peer, tnow).is_ok() {
                t.channels.push(peer);
            }
        }
        self.pump_turn(now);
    }

    // ---- send ----------------------------------------------------------------

    /// Send a Remote datagram on the selected route. False if there is none.
    pub fn send(&mut self, now: Instant, data: &[u8]) -> bool {
        let Some((local, remote)) = self.selected else { return false };
        self.send_from(now, local, remote, data);
        true
    }

    fn send_from(&mut self, now: Instant, local: SocketAddr, remote: SocketAddr, data: &[u8]) {
        let tnow = self.turn_now(now);
        if let Some(t) = self.turns.iter_mut().find(|t| t.relayed == Some(local)) {
            if !t.permissions.contains(&remote.ip()) {
                t.permissions.push(remote.ip());
                let _ = t.client.create_permission(TransportType::Udp, remote.ip(), tnow);
            }
            if let Ok(Some(tx)) = t.client.send_to(TransportType::Udp, remote, data, tnow) {
                let tx = tx.build();
                t.emit(&mut self.out, tx.from, tx.to, tx.transport == TransportType::Tcp, &tx.data);
            }
            self.pump_turn(now);
            return;
        }
        self.out.push_back(Transmit { transport: Transport::Udp, from: local, to: remote, data: data.to_vec() });
    }

    // ---- timers ----------------------------------------------------------------

    pub fn handle_timeout(&mut self, now: Instant) {
        for q in self.stun.iter_mut().filter(|q| !q.done && q.tries < 5 && now >= q.next) {
            q.tries += 1;
            q.next = now + Duration::from_millis(250 << q.tries.min(3));
            self.out.push_back(Transmit { transport: Transport::Udp, from: q.base, to: q.server, data: stun_binding_request(q.txid) });
        }
        if now >= self.next_recheck {
            self.next_recheck = now + RECHECK;
            let on_direct = self.selected.is_some_and(|(l, r)| route_kind(&self.turns, &self.ice, l, r) == RouteKind::Direct);
            if !on_direct && self.ice.remote_credentials().is_some() {
                // Our routes keep the current path alive meanwhile.
                self.ice.recreate_candidate_pairs();
            }
        }
        self.ice.handle_timeout(now);
        self.pump_ice(now);
        self.pump_turn(now);
        let mut lost = Vec::new();
        self.checks.retain(|_, (l, r, sent)| {
            let keep = now.saturating_duration_since(*sent) < CHECK_LOST_AFTER;
            if !keep {
                lost.push(((*l, *r), *sent));
            }
            keep
        });
        for (key, sent) in lost {
            // Checks from before the route worked say nothing about its loss.
            let in_outage = |i: &RouteInfo| i.outage.is_some_and(|(a, b)| sent >= a && b.is_none_or(|b| sent <= b));
            if let Some(info) = self.routes.get_mut(&key).filter(|i| sent > i.created && !in_outage(i)) {
                if !info.in_loss_run {
                    info.add_loss(now, 1, 1);
                }
                info.in_loss_run = true;
            }
        }
        self.reselect(now);
    }

    fn pump_ice(&mut self, now: Instant) {
        while let Some(t) = self.ice.poll_transmit() {
            let data: Vec<u8> = t.contents.into();
            if let Ok(msg) = is::stun::StunMessage::parse(&data) {
                if msg.is_binding_request() {
                    self.checks.insert(data[8..20].try_into().unwrap(), (t.source, t.destination, now));
                }
            }
            self.send_from(now, t.source, t.destination, &data);
        }
        while let Some(e) = self.ice.poll_event() {
            if let IceAgentEvent::IceConnectionStateChange(s) = e {
                self.events.push_back(PathEvent::IceState(format!("{s:?}")));
            }
        }
    }

    fn pump_turn(&mut self, now: Instant) {
        let tnow = self.turn_now(now);
        let mut new_relayed = Vec::new();
        for t in self.turns.iter_mut() {
            let _ = t.client.poll(tnow);
            while let Some(tx) = t.client.poll_transmit(tnow) {
                let (from, to, tcp) = (tx.from, tx.to, tx.transport == TransportType::Tcp);
                t.emit(&mut self.out, from, to, tcp, &tx.data);
            }
            while let Some(e) = t.client.poll_event() {
                match e {
                    TurnEvent::AllocationCreated(_, relayed) if relayed.is_ipv4() => {
                        t.relayed = Some(relayed);
                        self.relay_hints.lock().unwrap().insert(relayed, t.server.transport);
                        new_relayed.push((relayed, t.client.local_addr()));
                    }
                    TurnEvent::AllocationCreateFailed(f) => {
                        tracing::warn!("TURN allocation on {} failed ({f:?})", t.server.addr)
                    }
                    _ => {}
                }
            }
        }
        for (relayed, local) in new_relayed {
            tracing::info!("TURN relayed candidate {relayed} (from {local})");
            if let Ok(c) = Candidate::relayed(relayed, local, "udp") {
                self.add_local(c);
            }
        }
    }

    pub fn poll_transmit(&mut self) -> Option<Transmit> {
        self.out.pop_front()
    }

    pub fn poll_event(&mut self) -> Option<PathEvent> {
        self.events.pop_front()
    }

    pub fn poll_timeout(&mut self, now: Instant) -> Instant {
        let mut t = now + Duration::from_millis(250);
        if let Some(i) = self.ice.poll_timeout() {
            t = t.min(i);
        }
        for q in self.stun.iter().filter(|q| !q.done && q.tries < 5) {
            t = t.min(q.next);
        }
        let tnow = self.turn_now(now);
        for turn in self.turns.iter_mut() {
            if let TurnPollRet::WaitUntil(w) = turn.client.poll(tnow) {
                t = t.min(w.to_std(self.epoch));
            }
        }
        t.max(now)
    }

    /// Is the given address one of our TURN servers? (The driver uses it to
    /// route TCP writes.)
    pub fn is_turn_server(&self, addr: SocketAddr) -> bool {
        self.turns.iter().any(|t| t.server.addr == addr)
    }
}

fn stun_binding_request(txid: [u8; 12]) -> Vec<u8> {
    let mut m = Vec::with_capacity(20);
    m.extend_from_slice(&0x0001u16.to_be_bytes());
    m.extend_from_slice(&0u16.to_be_bytes());
    m.extend_from_slice(&STUN_COOKIE.to_be_bytes());
    m.extend_from_slice(&txid);
    m
}

/// XOR-MAPPED-ADDRESS (IPv4) from a binding success response.
fn parse_xor_mapped(m: &[u8]) -> Option<SocketAddr> {
    let len = u16::from_be_bytes([m[2], m[3]]) as usize;
    let mut i = 20;
    while i + 4 <= (20 + len).min(m.len()) {
        let ty = u16::from_be_bytes([m[i], m[i + 1]]);
        let alen = u16::from_be_bytes([m[i + 2], m[i + 3]]) as usize;
        let v = m.get(i + 4..i + 4 + alen)?;
        if ty == 0x0020 && alen >= 8 && v[1] == 0x01 {
            let port = u16::from_be_bytes([v[2], v[3]]) ^ (STUN_COOKIE >> 16) as u16;
            let ip = u32::from_be_bytes([v[4], v[5], v[6], v[7]]) ^ STUN_COOKIE;
            return Some(SocketAddr::new(IpAddr::V4(ip.into()), port));
        }
        i += 4 + alen.div_ceil(4) * 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two agents on loopback-like addresses, wired back to back.
    #[test]
    fn two_agents_find_a_route() {
        let now0 = Instant::now();
        let cfg = |controlling| PathConfig { controlling, stun_servers: vec![], turn_servers: vec![], tls: None };
        let mut a = PathAgent::new(cfg(true), now0);
        let mut b = PathAgent::new(cfg(false), now0);
        let (aa, ba): (SocketAddr, SocketAddr) = ("192.0.2.1:5000".parse().unwrap(), "192.0.2.2:6000".parse().unwrap());
        a.add_udp_socket(now0, aa);
        b.add_udp_socket(now0, ba);
        let (ua, pa) = a.local_credentials();
        let (ub, pb) = b.local_credentials();
        a.set_remote_credentials(&ub, &pb);
        b.set_remote_credentials(&ua, &pa);
        let mut now = now0;
        let mut a_route = None;
        let mut b_route = None;
        fn events(x: &mut PathAgent, y: &mut PathAgent, slot: &mut Option<Route>) {
            while let Some(e) = x.poll_event() {
                match e {
                    PathEvent::LocalCandidate(c) => y.add_remote_candidate(&c).unwrap(),
                    PathEvent::RouteChanged(r) => *slot = Some(r),
                    _ => {}
                }
            }
        }
        for _ in 0..200 {
            a.handle_timeout(now);
            b.handle_timeout(now);
            events(&mut a, &mut b, &mut a_route);
            events(&mut b, &mut a, &mut b_route);
            while let Some(t) = a.poll_transmit() {
                b.handle_receive(now, Transport::Udp, t.to, t.from, &t.data);
            }
            while let Some(t) = b.poll_transmit() {
                a.handle_receive(now, Transport::Udp, t.to, t.from, &t.data);
            }
            events(&mut a, &mut b, &mut a_route);
            events(&mut b, &mut a, &mut b_route);
            now += Duration::from_millis(10);
        }
        assert_eq!(a_route.map(|r| (r.local, r.remote)), Some((aa, ba)));
        assert_eq!(b_route.map(|r| (r.local, r.remote)), Some((ba, aa)));
        // Remote datagrams pass through untouched.
        assert!(a.send(now, &[0xC3, 1, 2, 3]));
        let t = a.poll_transmit().unwrap();
        assert_eq!(b.handle_receive(now, Transport::Udp, t.to, t.from, &t.data), vec![vec![0xC3, 1, 2, 3]]);
    }

    /// A train only goes to a route the switching test could ever pick.
    #[test]
    fn trains_only_where_the_switching_test_could_win() {
        let now = Instant::now();
        let cfg = PathConfig { controlling: true, stun_servers: vec![], turn_servers: vec![], tls: None };
        let key = |n: u16| -> (SocketAddr, SocketAddr) {
            (format!("192.0.2.1:{n}").parse().unwrap(), format!("192.0.2.2:{n}").parse().unwrap())
        };
        let ms = |v: f64| Duration::from_secs_f64(v / 1e3);
        let train_for = |active_ms: f64, other_ms: f64| {
            let mut a = PathAgent::new(cfg.clone(), now);
            a.routes.insert(key(1), RouteInfo::new(now, ms(active_ms), true));
            a.routes.insert(key(2), RouteInfo::new(now, ms(other_ms), true));
            a.selected = Some(key(1));
            a.demand_bps = 1_000_000;
            a.poll_train(now)
        };
        // 0.3 ms apart: inside the margin's reach but under 1 ms of gain.
        assert_eq!(train_for(0.6, 0.3), None);
        // 2 ms better: it could win.
        assert_eq!(train_for(2.5, 0.5), Some(key(2)));
    }

    /// Two pairs through the same relay share its bottleneck: a train's burst
    /// rate on one doesn't beat the other's C_est. Another relay still can.
    #[test]
    fn throughput_never_switches_between_pairs_on_the_same_relay() {
        let now = Instant::now();
        let cfg = PathConfig { controlling: true, stun_servers: vec![], turn_servers: vec![], tls: None };
        let pick = |other_relay: &str| {
            let mut a = PathAgent::new(cfg.clone(), now);
            let active: (SocketAddr, SocketAddr) = ("192.0.2.1:1".parse().unwrap(), "203.0.113.7:50011".parse().unwrap());
            let other: (SocketAddr, SocketAddr) = ("192.0.2.1:2".parse().unwrap(), format!("{other_relay}:50040").parse().unwrap());
            for k in [active, other] {
                let sdp = format!("candidate:{} 1 UDP 16777215 {} {} typ relay raddr 0.0.0.0 rport 0", k.1.port(), k.1.ip(), k.1.port());
                a.add_remote_candidate(&sdp).unwrap();
                a.routes.insert(k, RouteInfo::new(now, Duration::from_millis(50), true));
            }
            a.demand_bps = 65_000_000;
            a.selected = Some(active);
            a.routes.get_mut(&active).unwrap().capacity = Some(Capacity { bps: 19_000_000, at: now, cc: true, active_then: None });
            a.routes.get_mut(&other).unwrap().capacity =
                Some(Capacity { bps: 60_000_000, at: now, cc: false, active_then: Some(19_000_000) });
            a.reselect(now);
            a.selected == Some(other)
        };
        assert!(!pick("203.0.113.7"), "a sibling pair on the same relay won on throughput");
        assert!(pick("203.0.113.8"), "another relay with more throughput lost");
    }

    #[test]
    fn xor_mapped_address_parses() {
        let mut m = vec![0x01, 0x01, 0, 12];
        m.extend_from_slice(&STUN_COOKIE.to_be_bytes());
        m.extend_from_slice(&[0; 12]);
        m.extend_from_slice(&[0x00, 0x20, 0, 8, 0, 1]);
        m.extend_from_slice(&(3478u16 ^ 0x2112).to_be_bytes());
        m.extend_from_slice(&(u32::from(std::net::Ipv4Addr::new(203, 0, 113, 5)) ^ STUN_COOKIE).to_be_bytes());
        assert_eq!(parse_xor_mapped(&m), Some("203.0.113.5:3478".parse().unwrap()));
    }
}
