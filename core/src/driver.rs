// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The thin tokio driver: owns the UDP sockets and the TCP/TLS streams to TURN
//! servers, runs a `PathAgent` and a `Session` on one task, and talks to the
//! application over channels. All protocol logic stays in the sans-IO parts.
//!
//! Milestone 1: it routes each session datagram (`Via`): duplicates go on the
//! hedge route while the session hedges, probe trains on the route measured.
//! It carries the session's path signals to the path agent and the active
//! route's C_est the other way, publishes a `LinkStatus` watch (health, hedge
//! route, video budget, link state, reference hint), and, when bound to the
//! unspecified address, re-gathers candidates as interfaces come and go.

use crate::alloc::{FrameOpts, LinkState, RefHint, VideoBudget};
use crate::audio::JitterBuffer;
use crate::health::{Health, PathSignal, Via};
use crate::path::{DEFAULT_DEMAND_BPS, PathAgent, PathConfig, PathEvent, Route, Transport, TurnServer};
use crate::session::{Event, OutgoingFrame, Role, Session, SessionStats};
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};

/// Interfaces are re-read this often when watching them.
const IFACE_POLL: Duration = Duration::from_secs(1);

pub struct EndpointConfig {
    pub path: PathConfig,
    /// Local addresses to bind UDP sockets on (port 0 picks one). Each is a
    /// host candidate. An unspecified address (`0.0.0.0:port`) means every
    /// IPv4 interface, watched: candidates are gathered again when addresses
    /// come or go, and the session carries on.
    pub bind: Vec<SocketAddr>,
    /// How often to publish `Notice::Stats`.
    pub stats_interval: Duration,
}

#[derive(Debug)]
pub enum Command {
    RemoteCredentials { ufrag: String, pass: String },
    RemoteCandidate(String),
    VideoFrame(OutgoingFrame),
    Input(Vec<u8>),
    Control { kind: u8, data: Vec<u8> },
    CursorPosition { x: f32, y: f32, visible: bool, shape: u64 },
    ReceiverReport { frame_id: u32, decode_us: u32, present_us: u32 },
    Close,
    /// A frame with options (its reference after a `RefHint`).
    VideoFrameWith { frame: OutgoingFrame, opts: FrameOpts },
    /// An animation is in flight (HINT start) or settled.
    Animation(bool),
    CursorShape { hash: u64, image: Vec<u8> },
    /// One Opus packet; `capture_us` on the session clock (`Notice::Stats`'s
    /// `now_us`), or None for "now".
    Audio { mic: bool, timestamp: u32, capture_us: Option<u64>, data: Vec<u8> },
    SetMaxBitrate(u64),
    /// The platform saw a network change (e.g. NWPathMonitor): re-read the
    /// interfaces now instead of at the next poll.
    NetworkChanged,
    /// Client: a new handshake after `Event::Disconnected`.
    Reconnect,
    /// How much throughput weighs in route choice: the stream rate this side
    /// would like (bps). The host defaults to 50 Mbps (or its bandwidth cap),
    /// the client to 0 (RTT and loss only). The host's congestion controller
    /// also probes a quiet link up to it; until a demand
    /// comes, it doesn't probe.
    SetRouteDemand(u64),
}

impl std::fmt::Debug for OutgoingFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OutgoingFrame({} bytes)", self.data.len())
    }
}

#[derive(Debug)]
pub enum Notice {
    LocalCredentials { ufrag: String, pass: String },
    LocalCandidate(String),
    Route(Route),
    RouteLost,
    IceState(String),
    Session(Event),
    Stats { now_us: u64, stats: SessionStats, route: Option<Route> },
    /// Local addresses came or went (the driver is bound to `0.0.0.0` and
    /// gathered again). The new candidates follow as `LocalCandidate`, and the
    /// driver also sends them to the peer in-band, which works while any route
    /// survives. Reopen signalling (rendezvous or LAN) to carry them too when
    /// none does.
    NetworkChanged { added: Vec<IpAddr>, removed: Vec<IpAddr> },
}

/// Live link state for the host's encoder and the client's UI.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkStatus {
    pub health: Health,
    pub route: Option<Route>,
    /// The route duplicates go on while hedging ("Reconnecting… · relay SG").
    pub hedge_route: Option<Route>,
    /// How long there has been no live route at all (None: there is one).
    pub no_route_ms: Option<u64>,
    /// Host: what the next frame may use, for a fresh interval
    /// (`Session::interval_budget`): a refinement frame behind the interactive
    /// one gets `interval_remaining_bytes` less the interactive frame's size.
    pub budget: VideoBudget,
    /// Host: C_est and the split rule.
    pub link: LinkState,
    /// Host: predict the next frame from here. Each new hint gets a new
    /// `ref_hint_seq`; apply each once.
    pub ref_hint: Option<RefHint>,
    pub ref_hint_seq: u64,
}

impl LinkStatus {
    /// What a change notification is worth (not the ever-growing ms counters).
    fn key(&self) -> impl PartialEq + use<> {
        let r = |r: &Option<Route>| r.as_ref().map(|r| (r.local, r.remote, r.kind.clone()));
        (
            self.health.state,
            self.health.hedging,
            r(&self.route),
            r(&self.hedge_route),
            self.no_route_ms.is_some(),
            self.budget,
            self.link.c_est_bps,
            self.link.split_ok,
            self.ref_hint_seq,
        )
    }
}

pub struct EndpointHandle {
    pub commands: mpsc::UnboundedSender<Command>,
    pub notices: mpsc::UnboundedReceiver<Notice>,
    /// The encoder bitrate for the next frame, updated as feedback arrives.
    pub encoder_bitrate: watch::Receiver<u64>,
    pub task: tokio::task::JoinHandle<std::io::Result<()>>,
    /// Health, routes, video budget, link state and reference hints.
    pub status: watch::Receiver<LinkStatus>,
    /// Received audio (host → client) and mic (client → host) jitter buffers:
    /// the audio callback pops 10 ms at a time.
    pub audio_out: Arc<Mutex<JitterBuffer>>,
    pub mic_out: Arc<Mutex<JitterBuffer>>,
    /// The session clock's origin: every µs timestamp of this session
    /// (`now_us`, a frame's arrival times) counts from here.
    pub epoch: Instant,
}

enum Incoming {
    Data { transport: Transport, local: SocketAddr, from: SocketAddr, data: Vec<u8>, at: Instant },
    StreamUp { server: TurnServer, local: SocketAddr, writer: mpsc::UnboundedSender<Vec<u8>> },
}

pub fn spawn_endpoint(cfg: EndpointConfig, session: Session) -> EndpointHandle {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (notice_tx, notice_rx) = mpsc::unbounded_channel();
    let (rate_tx, rate_rx) = watch::channel(0u64);
    let (status_tx, status_rx) = watch::channel(LinkStatus::default());
    let (audio_out, mic_out) = (session.audio_buffer(false), session.audio_buffer(true));
    let epoch = session.epoch();
    let task = tokio::spawn(run(cfg, session, cmd_rx, notice_tx, rate_tx, status_tx));
    EndpointHandle { commands: cmd_tx, notices: notice_rx, encoder_bitrate: rate_rx, task, status: status_rx, audio_out, mic_out, epoch }
}

/// Usable local IPv4 addresses (not loopback or link-local).
fn local_ipv4() -> Vec<IpAddr> {
    let Ok(ifs) = if_addrs::get_if_addrs() else { return Vec::new() };
    let mut v: Vec<IpAddr> = ifs
        .into_iter()
        .filter(|i| !i.is_loopback())
        .map(|i| i.ip())
        .filter(|ip| matches!(ip, IpAddr::V4(v4) if !v4.is_link_local() && !v4.is_unspecified()))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The UDP sockets, one per local address, each with a receive task.
struct Sockets {
    udp: HashMap<SocketAddr, (Arc<UdpSocket>, tokio::task::JoinHandle<()>)>,
    in_tx: mpsc::UnboundedSender<Incoming>,
}

impl Sockets {
    fn bind(&mut self, addr: SocketAddr) -> std::io::Result<SocketAddr> {
        let sock = Arc::new(bind_udp(addr)?);
        let local = sock.local_addr()?;
        let tx = self.in_tx.clone();
        let rx = sock.clone();
        let task = tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                let (n, from) = match rx.recv_from(&mut buf).await {
                    Ok(r) => r,
                    // Windows reports an earlier send's ICMP "port unreachable" on the next
                    // receive (WSAECONNRESET), even on an unconnected socket: the socket is fine.
                    Err(e) if matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::ConnectionRefused) => continue,
                    Err(_) => break,
                };
                let at = Instant::now();
                let data = buf[..n].to_vec();
                if tx.send(Incoming::Data { transport: Transport::Udp, local, from, data, at }).is_err() {
                    break;
                }
            }
        });
        self.udp.insert(local, (sock, task));
        Ok(local)
    }

    fn drop_ip(&mut self, ip: IpAddr) {
        self.udp.retain(|a, (_, task)| {
            let keep = a.ip() != ip;
            if !keep {
                task.abort();
            }
            keep
        });
    }

    fn get(&self, a: &SocketAddr) -> Option<&Arc<UdpSocket>> {
        self.udp.get(a).map(|s| &s.0)
    }
}

fn connect_turn_stream(server: TurnServer, tx: mpsc::UnboundedSender<Incoming>) {
    tokio::spawn(async move {
        let stream = match TcpStream::connect(server.addr).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("TURN stream to {} failed: {e}", server.addr);
                return;
            }
        };
        let _ = stream.set_nodelay(true);
        let Ok(local) = stream.local_addr() else { return };
        let (mut rd, mut wr) = stream.into_split();
        let (wtx, mut wrx) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(async move {
            while let Some(d) = wrx.recv().await {
                if wr.write_all(&d).await.is_err() {
                    break;
                }
            }
        });
        let from = server.addr;
        if tx.send(Incoming::StreamUp { server, local, writer: wtx }).is_err() {
            return;
        }
        let mut buf = vec![0u8; 65536];
        while let Ok(n) = rd.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let data = buf[..n].to_vec();
            if tx.send(Incoming::Data { transport: Transport::Tcp, local, from, data, at: Instant::now() }).is_err() {
                break;
            }
        }
    });
}

async fn run(
    cfg: EndpointConfig,
    mut session: Session,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
    notice_tx: mpsc::UnboundedSender<Notice>,
    rate_tx: watch::Sender<u64>,
    status_tx: watch::Sender<LinkStatus>,
) -> std::io::Result<()> {
    let mut path = PathAgent::new(cfg.path.clone(), Instant::now());
    let host = session.role() == Role::Host;
    if host {
        path.set_demand_bps(DEFAULT_DEMAND_BPS);
    }
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<Incoming>();
    let mut socks = Sockets { udp: HashMap::new(), in_tx: in_tx.clone() };
    let mut streams: HashMap<(SocketAddr, SocketAddr), mpsc::UnboundedSender<Vec<u8>>> = HashMap::new();
    let (ufrag, pass) = path.local_credentials();
    let _ = notice_tx.send(Notice::LocalCredentials { ufrag, pass });

    // Fixed addresses, and the port to use for watched interfaces.
    let watch_port = cfg.bind.iter().find(|b| b.ip().is_unspecified()).map(|b| b.port());
    let mut bound_ips: HashSet<IpAddr> = HashSet::new();
    for b in cfg.bind.iter().filter(|b| !b.ip().is_unspecified()) {
        let local = socks.bind(*b)?;
        path.add_udp_socket(Instant::now(), local);
        bound_ips.insert(local.ip());
    }
    let mut next_iface = Instant::now();
    let mut gathered = false; // the first gather is not a change
    for server in path.stream_turn_servers() {
        connect_turn_stream(server, in_tx.clone());
    }

    let mut had_route = false;
    let mut no_route_since: Option<Instant> = None;
    let mut next_stats = Instant::now();
    let mut status = LinkStatus::default();
    loop {
        let now = Instant::now();
        if let Some(port) = watch_port {
            if now >= next_iface {
                next_iface = now + IFACE_POLL;
                let (added, removed) = regather(now, port, &mut bound_ips, &mut socks, &mut path, &mut streams, &in_tx);
                if gathered && (!added.is_empty() || !removed.is_empty()) {
                    let _ = notice_tx.send(Notice::NetworkChanged { added, removed });
                }
                gathered = true;
            }
        }
        path.handle_timeout(now);
        session.handle_timeout(now);
        if session.is_connected() && !session.health(now).hedging {
            if let Some(key) = path.poll_train(now) {
                if let Some(id) = session.start_probe_train(now) {
                    path.train_started(now, id, key);
                }
            }
        }
        while let Some(sig) = session.poll_path_signal() {
            match sig {
                PathSignal::Suspect => {
                    path.on_suspect(now);
                    session.set_hedge_capacity(path.hedge_capacity());
                    tracing::info!("route suspect; hedging on {:?}", path.hedge_route().map(|r| r.kind));
                }
                PathSignal::Recovered => path.on_recovered(now),
                PathSignal::Dead => {
                    tracing::info!("route dead; failing over");
                    path.fail_selected(now);
                }
                PathSignal::TrainReport { train_id, rate_bps, loss } => path.on_train_report(now, train_id, rate_bps, loss),
                PathSignal::RemoteCandidate(c) => {
                    if let Err(e) = path.add_remote_candidate(&c) {
                        tracing::warn!("bad in-band candidate {c:?}: {e}");
                    }
                }
            }
        }
        path.note_capacity(now, session.link_state(now).c_est_bps);
        // The handshake waits for a route; once connected, datagrams with no
        // route to take are stale by the time one comes back.
        let has_route = path.selected_route().is_some();
        if has_route || session.is_connected() {
            let hedging = session.health(now).hedging;
            while let Some((d, via)) = session.poll_transmit_via(now) {
                match via {
                    Via::Active => {
                        path.send(now, &d);
                        if hedging {
                            path.send_hedge(now, &d);
                        }
                    }
                    Via::ActiveOnly => {
                        path.send(now, &d);
                    }
                    Via::Train(id) => path.send_train(now, id, &d),
                }
            }
        }
        while let Some(t) = path.poll_transmit() {
            match t.transport {
                Transport::Udp => {
                    if let Some(s) = socks.get(&t.from) {
                        let _ = s.try_send_to(&t.data, t.to);
                    }
                }
                Transport::Tcp => {
                    if let Some(w) = streams.get(&(t.from, t.to)) {
                        let _ = w.send(t.data);
                    }
                }
            }
        }
        while let Some(e) = path.poll_event() {
            let n = match e {
                PathEvent::LocalCandidate(c) => {
                    if session.is_connected() {
                        session.send_candidate(now, &c); // in-band too: the peer may have no signalling open
                    }
                    Notice::LocalCandidate(c)
                }
                PathEvent::RouteChanged(r) => {
                    if had_route {
                        session.path_changed(now);
                    }
                    had_route = true;
                    no_route_since = None;
                    Notice::Route(r)
                }
                PathEvent::RouteLost => {
                    no_route_since.get_or_insert(now);
                    Notice::RouteLost
                }
                PathEvent::IceState(s) => Notice::IceState(s),
            };
            let _ = notice_tx.send(n);
        }
        while let Some(e) = session.poll_event() {
            let _ = notice_tx.send(Notice::Session(e));
        }
        rate_tx.send_if_modified(|r| {
            let v = session.encoder_bitrate_bps(now);
            let changed = *r != v;
            *r = v;
            changed
        });
        if let Some(h) = session.take_reference_hint() {
            status.ref_hint = Some(h);
            status.ref_hint_seq += 1;
        }
        status.health = session.health(now);
        status.route = path.selected_route();
        status.hedge_route = if status.health.hedging { path.hedge_route() } else { None };
        status.no_route_ms = no_route_since.map(|t| now.saturating_duration_since(t).as_millis() as u64);
        status.budget = session.interval_budget(now);
        status.link = session.link_state(now);
        status_tx.send_if_modified(|s| {
            let changed = s.key() != status.key();
            *s = status.clone();
            changed
        });
        if now >= next_stats {
            next_stats = now + cfg.stats_interval;
            let _ = notice_tx.send(Notice::Stats {
                now_us: session.now_us(now),
                stats: session.stats(now),
                route: path.selected_route(),
            });
        }

        let mut deadline = path.poll_timeout(now).min(next_stats);
        if watch_port.is_some() {
            deadline = deadline.min(next_iface);
        }
        if let Some(t) = session.poll_timeout() {
            deadline = deadline.min(t);
        }
        tokio::select! {
            inc = in_rx.recv() => {
                let Some(inc) = inc else { return Ok(()) };
                handle_incoming(&mut path, &mut session, &mut streams, inc);
                while let Ok(inc) = in_rx.try_recv() {
                    handle_incoming(&mut path, &mut session, &mut streams, inc);
                }
            }
            cmd = cmd_rx.recv() => {
                let now = Instant::now();
                match cmd {
                    None | Some(Command::Close) => return Ok(()),
                    Some(Command::RemoteCredentials { ufrag, pass }) => path.set_remote_credentials(&ufrag, &pass),
                    Some(Command::RemoteCandidate(c)) => {
                        if let Err(e) = path.add_remote_candidate(&c) {
                            tracing::warn!("bad remote candidate {c:?}: {e}");
                        }
                    }
                    Some(Command::VideoFrame(f)) => { session.send_video_frame(now, f); }
                    Some(Command::VideoFrameWith { frame, opts }) => { session.send_video_frame_with(now, frame, opts); }
                    Some(Command::Input(ev)) => session.send_input(now, ev),
                    Some(Command::Control { kind, data }) => session.send_control(now, kind, &data),
                    Some(Command::CursorPosition { x, y, visible, shape }) => session.send_cursor_position(now, x, y, visible, shape),
                    Some(Command::CursorShape { hash, image }) => session.send_cursor_shape(now, hash, &image),
                    Some(Command::ReceiverReport { frame_id, decode_us, present_us }) => {
                        session.send_receiver_report(now, frame_id, decode_us, present_us)
                    }
                    Some(Command::Animation(on)) => session.set_animation(on),
                    Some(Command::Audio { mic, timestamp, capture_us, data }) => {
                        let capture_us = capture_us.unwrap_or_else(|| session.now_us(now));
                        session.send_audio_frame(now, mic, timestamp, capture_us, &data)
                    }
                    Some(Command::SetMaxBitrate(bps)) => {
                        session.set_max_bitrate(bps);
                        if host {
                            path.set_demand_bps(bps.min(DEFAULT_DEMAND_BPS));
                        }
                    }
                    Some(Command::NetworkChanged) => next_iface = now,
                    Some(Command::Reconnect) => session.reconnect(now),
                    Some(Command::SetRouteDemand(bps)) => {
                        path.set_demand_bps(bps);
                        session.set_route_demand(bps); // the controller probes up to it
                    }
                }
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {}
        }
    }
}

/// Bind sockets on new addresses and drop vanished ones; the path agent
/// gathers again (host, server-reflexive and relayed candidates) and trickles
/// the new candidates out as `Notice::LocalCandidate`.
fn regather(
    now: Instant,
    port: u16,
    bound: &mut HashSet<IpAddr>,
    socks: &mut Sockets,
    path: &mut PathAgent,
    streams: &mut HashMap<(SocketAddr, SocketAddr), mpsc::UnboundedSender<Vec<u8>>>,
    in_tx: &mpsc::UnboundedSender<Incoming>,
) -> (Vec<IpAddr>, Vec<IpAddr>) {
    let current: HashSet<IpAddr> = local_ipv4().into_iter().collect();
    let gone: Vec<IpAddr> = bound.difference(&current).copied().collect();
    let mut added = Vec::new();
    for &ip in &gone {
        tracing::info!("address {ip} went away; re-gathering");
        let servers = path.turn_streams_on(ip);
        path.remove_local(now, ip);
        socks.drop_ip(ip);
        streams.retain(|(l, _), _| l.ip() != ip);
        bound.remove(&ip);
        for s in servers {
            connect_turn_stream(s, in_tx.clone());
        }
    }
    let new: Vec<IpAddr> = current.difference(bound).copied().collect();
    for ip in new {
        match socks.bind(SocketAddr::new(ip, port)) {
            Ok(local) => {
                tracing::info!("address {ip} appeared; gathering on {local}");
                path.add_udp_socket(now, local);
                bound.insert(ip);
                added.push(ip);
            }
            Err(e) => tracing::warn!("bind {ip}: {e}"),
        }
    }
    (added, gone)
}

/// A UDP socket with large buffers: a paced 4K frame is hundreds of packets,
/// and the default 208 KB receive buffer overflows on a busy scheduler tick.
/// (The kernel caps the request at net.core.rmem_max / wmem_max.)
fn bind_udp(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
    let _ = s.set_recv_buffer_size(8 << 20);
    let _ = s.set_send_buffer_size(8 << 20);
    s.set_nonblocking(true)?;
    s.bind(&addr.into())?;
    UdpSocket::from_std(s.into())
}

fn handle_incoming(
    path: &mut PathAgent,
    session: &mut Session,
    streams: &mut HashMap<(SocketAddr, SocketAddr), mpsc::UnboundedSender<Vec<u8>>>,
    inc: Incoming,
) {
    match inc {
        Incoming::Data { transport, local, from, data, at } => {
            for d in path.handle_receive(at, transport, local, from, &data) {
                session.handle_datagram(at, &d);
            }
        }
        Incoming::StreamUp { server, local, writer } => {
            streams.insert((local, server.addr), writer);
            if let Err(e) = path.add_turn_stream(&server, local) {
                tracing::warn!("TURN stream {}: {e}", server.addr);
            }
        }
    }
}
