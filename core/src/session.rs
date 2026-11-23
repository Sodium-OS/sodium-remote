// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The session: one sans-IO state machine per connection that owns the crypto,
//! every channel, the pacer, feedback, congestion control, clock sync and MTU
//! probing. It never touches a socket or a clock: the caller passes datagrams
//! and `Instant`s in and polls datagrams, events and the next deadline out.
//! The session doesn't know which path it runs on (ICE does); when the path
//! changes the keys stay and only MTU and delay baselines start over.
//!
//! Milestone 1 adds:
//! - utilisation by content: `video_budget`, `link_state`, `set_animation`;
//! - liveness and failover hedging: `health`, `poll_path_signal`,
//!   `poll_transmit_via` (which route a datagram goes on);
//! - probe trains for throughput-aware path choice: `start_probe_train`;
//! - rekeying every 2 minutes and an idle timeout (`Event::Disconnected`);
//! - reference-invalidation feedback: `take_reference_hint`, `FrameOpts`;
//! - an audio jitter buffer with capture times for A/V sync: `poll_audio`.
//!
//! And after the first acceptance run: the host
//! probes a quiet link up to the session's route demand with a frame's worth
//! of padding (`set_route_demand`; the controller decides, `cc.rs`).

use crate::alloc::{CRITICAL_SHARE, Content, FrameOpts, LinkState, QUEUE_FREE_FOR_SPLIT, RefHint, VideoBudget};
use crate::audio::{AudioOut, JitterBuffer, JitterStats};
use crate::cc::{CcConfig, CcState, CongestionController};
use crate::clock::ClockSync;
use crate::crypto::{DATA_OVERHEAD, Initiator, Responder, StaticKeypair, Transport};
use crate::feedback::{FeedbackRecorder, SentLog, TransportFeedback, sub as fbsub};
use crate::health::{DEAD_AFTER, ECHO_INTERVAL, Health, LinkHealth, Liveness, PathSignal, SUSPECT_AFTER, THIN_MAX_BPS, Via};
use crate::input::{InputReceiver, InputSender, sub as insub};
use crate::mtu::MtuProber;
use crate::pacer::Pacer;
use crate::reliable::{ReliableReceiver, ReliableSender, sub as relsub};
use crate::train::{self, TRAIN_DATAGRAM, TRAIN_LEN, TrainReceiver};
use crate::video::{self, Depacketizer, RxOutput, SendHistory, VIDEO_HEADER_LEN, VideoPacket};
use crate::wire::{Channel, Reader, Writer, kind, seq32_newer};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod probe {
    pub const PING: u8 = 0;
    pub const PONG: u8 = 1;
    pub const MTU_PROBE: u8 = 2;
    pub const MTU_ACK: u8 = 3;
    pub const TRAIN: u8 = 4;
    pub const TRAIN_REPORT: u8 = 5;
}
mod cursor {
    pub const POSITION: u8 = 0;
}
/// Control message kinds 0..=239 belong to the application; these are ours.
pub mod control_kind {
    pub const CURSOR_SHAPE: u8 = 250;
    /// An ICE candidate (SDP string) gathered while the session runs.
    pub const CANDIDATE: u8 = 251;
}

/// The client starts a new handshake inside the session this often.
pub const REKEY_AFTER: Duration = Duration::from_secs(120);
/// Keys older than this are never used (a rekey must have failed).
pub const REJECT_AFTER: Duration = Duration::from_secs(180);
const REKEY_AFTER_MESSAGES: u64 = 1 << 60;
/// Old keys still open packets this long after a rekey (stragglers).
const PREV_KEYS_KEEP: Duration = Duration::from_secs(10);
/// Nothing from the peer for this long ends the session: the host's
/// 2-minute hold.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The desktop being streamed: sends video, receives input. Noise responder.
    Host,
    /// The viewer: receives video, sends input. Noise initiator.
    Client,
}

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub role: Role,
    /// The stream's frame rate; sets the frame interval for pacing, feedback and NACK.
    pub fps: u32,
    pub cc: CcConfig,
}

impl SessionConfig {
    pub fn new(role: Role) -> Self {
        SessionConfig { role, fps: 60, cc: CcConfig::default() }
    }
}

/// Who the session will talk to.
pub enum Peer {
    /// The client knows the host's pinned key.
    Host([u8; 32]),
    /// The host accepts any of its paired devices.
    PairedClients(Vec<[u8; 32]>),
}

/// An encoded frame for the host to send.
pub struct OutgoingFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// Damage rectangles and other per-frame metadata, opaque here.
    pub meta: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReceivedFrame {
    pub frame_id: u32,
    pub keyframe: bool,
    /// When the host sent it, on the host's clock (µs since its session start).
    pub send_us: u64,
    /// The same instant on our clock, once clock sync has a sample.
    pub send_local_us: Option<i64>,
    pub first_arrival_us: u64,
    pub last_arrival_us: u64,
    pub recovered_shards: usize,
    pub meta: Vec<u8>,
    pub data: Vec<u8>,
    /// The frame is predicted from this earlier frame (a recovery after a loss).
    pub reference: Option<u32>,
    /// Every frame it depends on was decodable, as far as the network knows. False
    /// from a lost frame until a keyframe, a recovery point or a frame that
    /// references a decodable one.
    pub decodable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectReason {
    /// Nothing from the peer for the idle timeout (default 2 minutes).
    IdleTimeout,
    /// The keys outlived `REJECT_AFTER` without a rekey.
    KeyExpired,
    /// The peer sent a control message larger than `reliable::MAX_MESSAGE`.
    MessageTooLarge,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Connected { remote_static: [u8; 32] },
    VideoFrame(ReceivedFrame),
    VideoFrameLost { frame_id: u32 },
    /// Host side: the client lost this frame; invalidate it as a reference
    /// (`take_reference_hint` says which frame to predict from).
    PeerLostFrame { frame_id: u32 },
    Input(Vec<u8>),
    Control { kind: u8, data: Vec<u8> },
    CursorPosition { x: f32, y: f32, visible: bool, shape: u64 },
    CursorShape { hash: u64, data: Vec<u8> },
    Audio { mic: bool, seq: u32, timestamp: u32, data: Vec<u8> },
    ReceiverReport { frame_id: u32, decode_us: u32, present_us: u32 },
    /// The session ended. A host waits for a new handshake; a client stays
    /// closed until `reconnect`.
    Disconnected { reason: DisconnectReason },
}

#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub connected: bool,
    pub rtt_ms: f64,
    pub clock_offset_us: i64,
    pub mtu: usize,
    pub target_bps: u64,
    pub encoder_bps: u64,
    pub fec_ratio: f32,
    pub cc_state: Option<CcState>,
    pub queue_delay_ms: f64,
    pub receive_rate_bps: f64,
    pub loss: f64,
    pub pacer_queue_bytes: usize,
    pub frames_sent: u64,
    pub frames_received: u64,
    pub frames_lost: u64,
    pub recovered_shards: u64,
    pub video_packets_received: u64,
    pub nacks_sent: u64,
    pub retransmits: u64,
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub packets_received: u64,
    pub bytes_received: u64,
    pub nack_enabled: bool,
    /// C_est, the measured bottleneck rate (0: none).
    pub c_est_bps: u64,
    pub queue_free_ms: u64,
    pub latency_critical: bool,
    pub health: LinkHealth,
    pub hedging: bool,
    /// Times the active route went suspect, came back, and was given up.
    pub suspects: u64,
    pub recoveries: u64,
    pub deaths: u64,
    pub rekeys: u64,
    /// Host: the rate the last controller probe tested (0: none yet), and a
    /// refused probe's ceiling (0: none; docs/reference/protocol.md 3.8).
    pub probe_bps: u64,
    pub probe_ceiling_bps: u64,
    /// Host: probes sent, accepted, refused (the ceiling) and over 6 ms (the
    /// decrease), and the largest standing queue a probe left.
    pub probes: u64,
    pub probes_accepted: u64,
    pub probes_refused: u64,
    pub probes_decreased: u64,
    pub probe_queue_max_ms: f64,
}

enum Handshake {
    Initiator(Initiator),
    Responder(Responder),
}

/// Client side: is the chain of frames up to here decodable?
#[derive(Default)]
struct Chain {
    ok: bool,
    decoded: VecDeque<u32>,
    last_good: Option<u32>,
}

impl Chain {
    fn on_frame(&mut self, id: u32, fresh_start: bool, reference: Option<u32>) -> bool {
        self.ok = if fresh_start {
            true
        } else if let Some(r) = reference {
            self.decoded.contains(&r)
        } else {
            self.ok
        };
        if self.ok {
            self.decoded.push_back(id);
            if self.decoded.len() > 128 {
                self.decoded.pop_front();
            }
            self.last_good = Some(id);
        }
        self.ok
    }
}

pub struct Session {
    cfg: SessionConfig,
    epoch: Instant,
    frame_interval: Duration,
    handshake: Handshake,
    transport: Option<Transport>,
    transport_born: Instant,
    /// Keys a rekey replaced, kept to open stragglers until the instant.
    prev: Option<(Transport, Instant)>,
    /// Host: new keys from a rekey, used once the client's first packet under them arrives.
    pending: Option<Transport>,
    /// Client: a rekey handshake is out.
    rekeying: bool,
    /// Client: closed after a disconnect, until `reconnect`.
    closed: bool,
    idle_timeout: Duration,
    out: VecDeque<(Vec<u8>, Via)>,
    events: VecDeque<Event>,
    pacer: Pacer,
    next_frame_id: u32,
    history: SendHistory,
    sent_log: SentLog,
    cc: CongestionController,
    depack: Depacketizer,
    fb_rec: FeedbackRecorder,
    next_feedback: Option<Instant>,
    clock: ClockSync,
    mtu: MtuProber,
    input_tx: InputSender,
    input_rx: InputReceiver,
    ctrl_tx: ReliableSender,
    ctrl_rx: ReliableReceiver,
    cursor_seq: u32,
    cursor_rx_seq: Option<u32>,
    shapes_sent: HashSet<u64>,
    audio_seq: [u32; 2],
    /// Shared so an audio callback on another thread can pop (`audio_buffer`).
    jitter: [Arc<Mutex<JitterBuffer>>; 2],
    content: Content,
    live: Liveness,
    hedge_capacity: Option<u64>,
    signals: VecDeque<PathSignal>,
    trains_rx: TrainReceiver,
    trains_tx: HashMap<u32, Instant>,
    next_train: u32,
    /// Host: the newest frame the client reported decodable.
    last_acked: Option<u32>,
    /// Host: the newest frame sent as a keyframe or recovery.
    last_recovery: Option<u32>,
    ref_hint: Option<RefHint>,
    chain: Chain,
    /// Host: when the last video packet left the pacer, and its counter (is
    /// the link quiet enough for a probe?).
    last_video_out: Option<(Instant, u64)>,
    /// Host: when a probe may be due (only time stood in its way).
    next_probe_check: Option<Instant>,
    /// Host: the route demand, kept across a new handshake.
    route_demand: u64,
    stats: SessionStats,
}

/// Does the peer answer this plaintext (so silence means the route is down)?
fn answerable(p: &[u8]) -> bool {
    match (Channel::from_u8(p[0]), p.get(1).copied()) {
        (Some(Channel::Video), _) => true,
        (Some(Channel::Input), Some(insub::EVENTS)) => true,
        (Some(Channel::Control), Some(relsub::DATA)) => true,
        (Some(Channel::Probe), Some(probe::PING)) => true,
        _ => false,
    }
}

impl Session {
    pub fn new(cfg: SessionConfig, local: StaticKeypair, peer: Peer, now: Instant) -> Self {
        let handshake = match (cfg.role, peer) {
            (Role::Client, Peer::Host(key)) => Handshake::Initiator(Initiator::new(local, key)),
            (Role::Host, Peer::PairedClients(keys)) => Handshake::Responder(Responder::new(local, keys)),
            _ => panic!("a client pins one host; a host accepts its paired clients"),
        };
        let frame_interval = Duration::from_secs(1) / cfg.fps.max(1);
        Session {
            cc: CongestionController::new(cfg.cc.clone(), frame_interval),
            depack: Depacketizer::new(frame_interval),
            cfg,
            epoch: now,
            frame_interval,
            handshake,
            transport: None,
            transport_born: now,
            prev: None,
            pending: None,
            rekeying: false,
            closed: false,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            out: VecDeque::new(),
            events: VecDeque::new(),
            pacer: Pacer::new(),
            next_frame_id: 0,
            history: SendHistory::default(),
            sent_log: SentLog::default(),
            fb_rec: FeedbackRecorder::default(),
            next_feedback: None,
            clock: ClockSync::default(),
            mtu: MtuProber::new(now),
            input_tx: InputSender::default(),
            input_rx: InputReceiver::default(),
            ctrl_tx: ReliableSender::default(),
            ctrl_rx: ReliableReceiver::default(),
            cursor_seq: 0,
            cursor_rx_seq: None,
            shapes_sent: HashSet::new(),
            audio_seq: [0; 2],
            jitter: [Arc::new(Mutex::new(JitterBuffer::new(now))), Arc::new(Mutex::new(JitterBuffer::new(now)))],
            content: Content::default(),
            live: Liveness::new(now),
            hedge_capacity: None,
            signals: VecDeque::new(),
            trains_rx: TrainReceiver::default(),
            trains_tx: HashMap::new(),
            next_train: 0,
            last_acked: None,
            last_recovery: None,
            ref_hint: None,
            chain: Chain::default(),
            last_video_out: None,
            next_probe_check: None,
            route_demand: 0,
            stats: SessionStats::default(),
        }
    }

    fn us(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.epoch).as_micros() as u64
    }

    pub fn is_connected(&self) -> bool {
        self.transport.is_some()
    }

    pub fn role(&self) -> Role {
        self.cfg.role
    }

    /// The largest plaintext payload one datagram carries on this path.
    pub fn max_payload(&self) -> usize {
        self.mtu.current - DATA_OVERHEAD
    }

    /// What the stream may send now: the controller's target, or the thin
    /// rate while hedging.
    fn send_rate_bps(&self) -> u64 {
        if self.live.hedging() { self.thin_bps() } else { self.cc.target_bps() }
    }

    fn thin_bps(&self) -> u64 {
        let mut t = self.cc.target_bps().min(THIN_MAX_BPS);
        if let Some(c) = self.hedge_capacity {
            t = t.min(c / 2);
        }
        t.max(self.cfg.cc.min_bps)
    }

    /// Bitrate the encoder should aim for with the next frame (FEC excluded).
    /// `video_budget` refines this by content.
    pub fn encoder_bitrate_bps(&self, now: Instant) -> u64 {
        let ratio = self.cc.fec_ratio(self.us(now)) as f64;
        (self.send_rate_bps() as f64 / (1.0 + ratio)) as u64
    }

    /// The user's bandwidth cap ("Bandwidth limit" in the client settings).
    pub fn set_max_bitrate(&mut self, bps: u64) {
        self.cc.set_max_bps(bps);
    }

    /// Host: the session's route demand, the rate the controller
    /// probes a quiet link up to (with the user's cap if lower). 0: no probes.
    pub fn set_route_demand(&mut self, bps: u64) {
        self.route_demand = bps;
        self.cc.set_demand(bps);
    }

    /// The session ends after this long with nothing from the peer.
    pub fn set_idle_timeout(&mut self, d: Duration) {
        self.idle_timeout = d;
    }

    /// ICE moved to a different pair: keys stay, path measurements restart.
    /// After a failover the controller starts from the thin stream's rate.
    pub fn path_changed(&mut self, now: Instant) {
        self.mtu.reset(now);
        if self.live.hedging() {
            let thin = self.thin_bps();
            self.cc.rebaseline(thin);
        } else {
            self.cc.reset_path();
        }
        self.live.state = LinkHealth::Healthy;
        self.live.last_answer = now;
        self.live.probes.clear();
        self.live.next_probe = None;
    }

    fn reset_streams(&mut self, now: Instant) {
        let cfg = self.cfg.clone();
        self.pacer = Pacer::new();
        self.history = SendHistory::default();
        self.sent_log = SentLog::default();
        self.cc = CongestionController::new(cfg.cc, self.frame_interval);
        self.cc.set_demand(self.route_demand);
        self.depack = Depacketizer::new(self.frame_interval);
        self.fb_rec = FeedbackRecorder::default();
        self.next_feedback = None;
        self.clock = ClockSync::default();
        self.mtu = MtuProber::new(now);
        self.input_tx = InputSender::default();
        self.input_rx = InputReceiver::default();
        self.ctrl_tx = ReliableSender::default();
        self.ctrl_rx = ReliableReceiver::default();
        self.cursor_rx_seq = None;
        self.shapes_sent.clear();
        for j in &self.jitter {
            *j.lock().unwrap() = JitterBuffer::new(now);
        }
        self.content = Content::default();
        self.live = Liveness::new(now);
        self.signals.clear();
        self.trains_rx = TrainReceiver::default();
        self.trains_tx.clear();
        self.last_acked = None;
        self.last_recovery = None;
        self.ref_hint = None;
        self.chain = Chain::default();
        self.last_video_out = None;
        self.next_probe_check = None;
        self.prev = None;
        self.pending = None;
        self.rekeying = false;
        self.transport_born = now;
    }

    fn seal(&mut self, now: Instant, plaintext: &[u8]) {
        self.seal_via(now, plaintext, Via::Active, true);
    }

    fn seal_via(&mut self, now: Instant, plaintext: &[u8], via: Via, cc: bool) -> Option<u64> {
        self.seal_full(now, plaintext, via, cc, None)
    }

    /// Seal and queue a datagram; `probe` marks a controller probe's padding in
    /// the sent log. Returns its counter.
    fn seal_full(&mut self, now: Instant, plaintext: &[u8], via: Via, cc: bool, probe: Option<u32>) -> Option<u64> {
        let t = self.transport.as_mut()?;
        let (counter, packet) = t.seal(plaintext);
        let us = now.saturating_duration_since(self.epoch).as_micros() as u64;
        match probe {
            Some(id) => {
                self.sent_log.record_probe(counter, us, packet.len(), id);
                self.cc.probe_packet_left(id, counter, us);
            }
            None => self.sent_log.record_as(counter, us, packet.len(), cc),
        }
        self.stats.packets_sent += 1;
        self.stats.bytes_sent += packet.len() as u64;
        if !matches!(via, Via::Train(_)) && answerable(plaintext) {
            self.live.last_answerable_sent = Some(now);
        }
        self.out.push_back((packet, via));
        Some(counter)
    }

    fn connected(&mut self, now: Instant, transport: Transport) {
        let remote_static = transport.remote_static;
        self.reset_streams(now);
        self.transport = Some(transport);
        self.events.push_back(Event::Connected { remote_static });
    }

    fn disconnect(&mut self, reason: DisconnectReason) {
        self.transport = None;
        self.prev = None;
        self.pending = None;
        self.rekeying = false;
        self.out.clear();
        self.pacer = Pacer::new();
        self.closed = self.cfg.role == Role::Client;
        self.events.push_back(Event::Disconnected { reason });
    }

    /// Client: start a new handshake after `Disconnected`.
    pub fn reconnect(&mut self, now: Instant) {
        if self.cfg.role == Role::Client && self.transport.is_none() {
            self.closed = false;
            if let Handshake::Initiator(i) = &mut self.handshake {
                let m = i.start(now);
                self.out.push_back((m, Via::Active));
            }
        }
    }

    /// Open a data packet with whichever keys its receiver index names; the
    /// host switches to a rekey's keys on the first packet under them.
    fn open(&mut self, now: Instant, data: &[u8]) -> Option<(u64, Vec<u8>)> {
        let index = u32::from_be_bytes(data.get(1..5)?.try_into().ok()?);
        if self.pending.as_ref().is_some_and(|p| p.local_index == index) {
            let opened = self.pending.as_mut()?.open(data)?;
            let mut p = self.pending.take()?;
            let old = self.transport.take()?;
            p.set_send_counter(old.next_counter());
            self.prev = Some((old, now + PREV_KEYS_KEEP));
            self.transport = Some(p);
            self.transport_born = now;
            self.stats.rekeys += 1;
            return Some(opened);
        }
        if let Some(t) = self.transport.as_mut().filter(|t| t.local_index == index) {
            return t.open(data);
        }
        self.prev.as_mut().filter(|(t, _)| t.local_index == index)?.0.open(data)
    }

    // ---- I/O -------------------------------------------------------------

    pub fn handle_datagram(&mut self, now: Instant, data: &[u8]) {
        let Some(&first) = data.first() else { return };
        match first {
            kind::HANDSHAKE_INIT => {
                let Handshake::Responder(r) = &mut self.handshake else { return };
                let Some((resp, transport, rekey_of)) = r.handle_init_ex(data) else { return };
                let rekey = matches!((rekey_of, &self.transport), (Some(i), Some(cur))
                    if cur.local_index == i && cur.remote_static == transport.remote_static);
                if rekey {
                    self.pending = Some(transport);
                    self.out.push_back((resp, Via::Active));
                } else {
                    self.connected(now, transport);
                    self.out.push_back((resp, Via::Active));
                }
            }
            kind::HANDSHAKE_RESP => {
                let Handshake::Initiator(i) = &mut self.handshake else { return };
                let Some(mut transport) = i.handle_response(data) else { return };
                match self.transport.take() {
                    Some(old) => {
                        transport.set_send_counter(old.next_counter());
                        self.prev = Some((old, now + PREV_KEYS_KEEP));
                        self.transport = Some(transport);
                        self.transport_born = now;
                        self.rekeying = false;
                        self.stats.rekeys += 1;
                    }
                    None => {
                        self.connected(now, transport);
                        // Speak first so the host's side of every NAT sees us.
                        self.send_ping(now);
                    }
                }
            }
            kind::DATA => {
                let Some((counter, plain)) = self.open(now, data) else { return };
                let now_us = self.us(now);
                self.live.last_rx = self.live.last_rx.max(now);
                self.stats.packets_received += 1;
                self.stats.bytes_received += data.len() as u64;
                if self.cfg.role == Role::Client {
                    self.fb_rec.record(counter, now_us);
                    if self.next_feedback.is_none() {
                        self.next_feedback = Some(now + self.frame_interval);
                    }
                }
                self.dispatch(now, &plain);
            }
            _ => {}
        }
    }

    /// Something we sent was answered. While hedging only an answer to a
    /// probe sent on the active route alone proves that route.
    fn on_answer(&mut self, now: Instant, proves_active: bool) {
        match self.live.state {
            LinkHealth::Healthy => self.live.last_answer = self.live.last_answer.max(now),
            LinkHealth::Suspect | LinkHealth::Dead if proves_active => {
                self.live.state = LinkHealth::Healthy;
                self.live.last_answer = now;
                self.live.probes.clear();
                self.live.next_probe = None;
                self.cc.set_frozen(false);
                self.signals.push_back(PathSignal::Recovered);
                self.stats.recoveries += 1;
            }
            _ => {}
        }
    }

    fn dispatch(&mut self, now: Instant, plain: &[u8]) -> Option<()> {
        let mut r = Reader::new(plain);
        let channel = Channel::from_u8(r.u8()?)?;
        let now_us = self.us(now);
        match channel {
            Channel::Video => {
                if video::padding_probe(plain).is_some() {
                    return Some(()); // a probe's padding: its bytes are counted, nothing more
                }
                let p = VideoPacket::decode(&mut r)?;
                if p.flags & video::flags::RETRANSMIT != 0 {
                    self.stats.retransmits += 1;
                }
                self.depack.on_packet(now, p);
                self.drain_video(now);
            }
            Channel::Feedback => match r.u8()? {
                fbsub::TRANSPORT => {
                    let fb = TransportFeedback::decode(&mut r)?;
                    let delivered = self.cc.on_feedback(now_us, &fb, &mut self.sent_log);
                    if r.remaining() >= 4 {
                        self.frame_acked(r.u32()?);
                    }
                    // Feedback that only covers probe trains (other routes) proves nothing here.
                    if delivered > 0 {
                        self.on_answer(now, false);
                    }
                }
                fbsub::NACK => {
                    let frame_id = r.u32()?;
                    let n = r.u16()?;
                    for _ in 0..n {
                        if let Some(p) = self.history.get(frame_id, r.u16()?) {
                            self.pacer.push_urgent(now, p.encode());
                            self.stats.retransmits += 1;
                        }
                    }
                }
                fbsub::FRAME_LOST => {
                    let frame_id = r.u32()?;
                    if r.remaining() >= 4 {
                        self.frame_acked(r.u32()?);
                    }
                    self.events.push_back(Event::PeerLostFrame { frame_id });
                    // Losses of frames older than the last recovery are already handled.
                    if self.last_recovery.is_none_or(|rcv| !seq32_newer(rcv, frame_id)) {
                        self.ref_hint = Some(self.last_acked.map_or(RefHint::Keyframe, RefHint::Reference));
                    }
                }
                fbsub::RECEIVER_REPORT => {
                    let frame_id = r.u32()?;
                    let decode_us = r.u32()?;
                    let present_us = r.u32()?;
                    self.events.push_back(Event::ReceiverReport { frame_id, decode_us, present_us });
                }
                _ => {}
            },
            Channel::Input => match r.u8()? {
                insub::EVENTS => {
                    let (fresh, ack) = self.input_rx.on_events(&mut r)?;
                    self.seal(now, &ack);
                    if !fresh.is_empty() {
                        self.content.last_input = Some(now);
                    }
                    self.events.extend(fresh.into_iter().map(Event::Input));
                }
                insub::ACK => {
                    self.input_tx.on_ack(r.u32()?);
                    self.on_answer(now, false);
                }
                _ => {}
            },
            Channel::Control => match r.u8()? {
                relsub::DATA => {
                    let Some((msgs, ack)) = self.ctrl_rx.on_data(&mut r) else {
                        if self.ctrl_rx.too_large() && self.transport.is_some() {
                            tracing::warn!("the peer sent a control message over {} bytes: closing", crate::reliable::MAX_MESSAGE);
                            self.disconnect(DisconnectReason::MessageTooLarge);
                        }
                        return None;
                    };
                    self.seal(now, &ack);
                    for (kind, data) in msgs {
                        if kind == control_kind::CURSOR_SHAPE && data.len() >= 8 {
                            let hash = u64::from_be_bytes(data[..8].try_into().ok()?);
                            self.events.push_back(Event::CursorShape { hash, data: data[8..].to_vec() });
                        } else if kind == control_kind::CANDIDATE {
                            if let Ok(sdp) = String::from_utf8(data) {
                                self.signals.push_back(PathSignal::RemoteCandidate(sdp));
                            }
                        } else {
                            self.events.push_back(Event::Control { kind, data });
                        }
                    }
                }
                relsub::ACK => {
                    self.ctrl_tx.on_ack(&mut r)?;
                    self.on_answer(now, false);
                }
                _ => {}
            },
            Channel::Cursor => {
                if r.u8()? == cursor::POSITION {
                    let seq = r.u32()?;
                    if self.cursor_rx_seq.is_some_and(|s| !seq32_newer(seq, s)) {
                        return Some(()); // an older position: latest wins
                    }
                    self.cursor_rx_seq = Some(seq);
                    let (x, y, visible, shape) = (r.f32()?, r.f32()?, r.u8()? != 0, r.u64()?);
                    self.events.push_back(Event::CursorPosition { x, y, visible, shape });
                }
            }
            Channel::Audio | Channel::Mic => {
                let seq = r.u32()?;
                let timestamp = r.u32()?;
                let capture_us = r.u64()?;
                let data = r.rest().to_vec();
                let mic = channel == Channel::Mic;
                let capture_local = self.clock.to_local(capture_us);
                self.jitter[mic as usize].lock().unwrap().push(now, timestamp, data.clone(), capture_local);
                self.events.push_back(Event::Audio { mic, seq, timestamp, data });
            }
            Channel::Probe => match r.u8()? {
                probe::PING => {
                    let t0 = r.u64()?;
                    let mut w = Writer::with_capacity(26);
                    w.u8(Channel::Probe as u8).u8(probe::PONG).u64(t0).u64(now_us).u64(self.us(now));
                    self.seal(now, &w.buf);
                }
                probe::PONG => {
                    let (t0, t1, t2) = (r.u64()?, r.u64()?, r.u64()?);
                    self.clock.on_pong(t0, t1, t2, now_us);
                    self.depack.rtt = self.clock.rtt();
                    self.depack.nack_enabled = self.clock.rtt() < self.frame_interval / 2;
                    let probe = self.live.probes.contains(&t0);
                    self.on_answer(now, probe);
                }
                probe::MTU_PROBE => {
                    let size = r.u16()?;
                    let id = r.u16()?;
                    let mut w = Writer::with_capacity(6);
                    w.u8(Channel::Probe as u8).u8(probe::MTU_ACK).u16(size).u16(id);
                    self.seal(now, &w.buf);
                }
                probe::MTU_ACK => {
                    let size = r.u16()? as usize;
                    let id = r.u16()?;
                    self.mtu.on_ack(size, id);
                    self.on_answer(now, false);
                }
                probe::TRAIN => {
                    let (id, index, count) = (r.u32()?, r.u16()?, r.u16()?);
                    if let Some(res) = self.trains_rx.on_packet(now, id, index, count, plain.len() + DATA_OVERHEAD) {
                        self.send_train_report(now, res);
                    }
                }
                probe::TRAIN_REPORT => {
                    let (train_id, rate_bps, loss) = train::decode_report(&mut r)?;
                    if self.trains_tx.remove(&train_id).is_some() {
                        self.signals.push_back(PathSignal::TrainReport { train_id, rate_bps, loss });
                    }
                }
                _ => {}
            },
        }
        Some(())
    }

    fn frame_acked(&mut self, frame_id: u32) {
        if self.last_acked.is_none_or(|a| seq32_newer(frame_id, a)) {
            self.last_acked = Some(frame_id);
        }
    }

    fn send_train_report(&mut self, now: Instant, res: train::TrainResult) {
        let mut w = Writer::with_capacity(18);
        w.u8(Channel::Probe as u8).u8(probe::TRAIN_REPORT);
        train::encode_report(&mut w, res);
        self.seal(now, &w.buf);
    }

    fn drain_video(&mut self, now: Instant) {
        while let Some(o) = self.depack.poll_output() {
            match o {
                RxOutput::Frame(f) => {
                    let mut r = Reader::new(&f.data);
                    let Some(send_us) = r.u64() else { continue };
                    let reference = if f.flags & video::flags::REFERENCE != 0 { r.u32() } else { None };
                    let Some(meta) = r.lp_bytes() else { continue };
                    let meta = meta.to_vec();
                    let data = r.rest().to_vec();
                    self.stats.frames_received += 1;
                    let keyframe = f.flags & video::flags::KEYFRAME != 0;
                    let fresh = keyframe || f.flags & video::flags::RECOVERY_POINT != 0;
                    let decodable = self.chain.on_frame(f.frame_id, fresh, reference);
                    let frame = ReceivedFrame {
                        frame_id: f.frame_id,
                        keyframe,
                        send_us,
                        send_local_us: self.clock.to_local(send_us),
                        first_arrival_us: self.us(f.first_arrival),
                        last_arrival_us: self.us(f.last_arrival),
                        recovered_shards: f.recovered,
                        meta,
                        data,
                        reference,
                        decodable,
                    };
                    self.events.push_back(Event::VideoFrame(frame));
                }
                RxOutput::Lost(frame_id) => {
                    self.chain.ok = false;
                    let mut w = Writer::with_capacity(10);
                    w.u8(Channel::Feedback as u8).u8(fbsub::FRAME_LOST).u32(frame_id);
                    if let Some(g) = self.chain.last_good {
                        w.u32(g);
                    }
                    self.seal(now, &w.buf);
                    self.events.push_back(Event::VideoFrameLost { frame_id });
                }
            }
        }
    }

    fn send_ping(&mut self, now: Instant) {
        let t0 = self.us(now);
        let mut w = Writer::with_capacity(10);
        w.u8(Channel::Probe as u8).u8(probe::PING).u64(t0);
        if self.live.hedging() {
            self.live.probes.push_back(t0);
            if self.live.probes.len() > 32 {
                self.live.probes.pop_front();
            }
            self.seal_via(now, &w.buf, Via::ActiveOnly, true);
        } else {
            self.seal(now, &w.buf);
        }
    }

    fn enter_suspect(&mut self, now: Instant) {
        self.live.state = LinkHealth::Suspect;
        self.pacer.cut_padding(); // no probe while hedging
        self.cc.set_frozen(true);
        self.signals.push_back(PathSignal::Suspect);
        self.stats.suspects += 1;
        if self.cfg.role == Role::Host && self.stats.frames_sent > 0 {
            // The first thin frame references the last frame the client acknowledged.
            self.ref_hint = Some(self.last_acked.map_or(RefHint::Keyframe, RefHint::Reference));
        }
        self.send_ping(now);
        self.live.next_probe = Some(now + ECHO_INTERVAL);
    }

    fn tick_liveness(&mut self, now: Instant) {
        let quiet = now.saturating_duration_since(self.live.last_answer);
        match self.live.state {
            LinkHealth::Healthy => {
                if quiet >= SUSPECT_AFTER {
                    self.enter_suspect(now);
                } else if self.live.last_answerable_sent.is_none_or(|t| now.saturating_duration_since(t) >= ECHO_INTERVAL) {
                    self.send_ping(now);
                }
            }
            LinkHealth::Suspect | LinkHealth::Dead => {
                if self.live.state == LinkHealth::Suspect && quiet >= DEAD_AFTER {
                    self.live.state = LinkHealth::Dead;
                    self.signals.push_back(PathSignal::Dead);
                    self.stats.deaths += 1;
                }
                if self.live.next_probe.is_none_or(|t| now >= t) {
                    self.send_ping(now);
                    self.live.next_probe = Some(now + ECHO_INTERVAL);
                }
            }
        }
    }

    pub fn handle_timeout(&mut self, now: Instant) {
        if self.transport.is_none() {
            if self.closed {
                return;
            }
            if let Handshake::Initiator(i) = &mut self.handshake {
                if i.poll_timeout().is_none_or(|t| now >= t) {
                    let m = i.start(now);
                    self.out.push_back((m, Via::Active));
                }
            }
            return;
        }
        if now.saturating_duration_since(self.live.last_rx) >= self.idle_timeout {
            self.disconnect(DisconnectReason::IdleTimeout);
            return;
        }
        if now.saturating_duration_since(self.transport_born) >= REJECT_AFTER {
            self.disconnect(DisconnectReason::KeyExpired);
            return;
        }
        if self.prev.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.prev = None;
        }
        self.poll_rekey(now);
        self.tick_liveness(now);
        self.poll_probe(now);
        if self.clock.poll_ping(now) {
            self.send_ping(now);
        }
        if let Some((size, id)) = self.mtu.poll_probe(now, self.clock.rtt()) {
            let mut w = Writer::with_capacity(size);
            w.u8(Channel::Probe as u8).u8(probe::MTU_PROBE).u16(size as u16).u16(id);
            w.buf.resize(size - DATA_OVERHEAD, 0);
            self.seal(now, &w.buf);
        }
        if self.next_feedback.is_some_and(|t| now >= t) {
            self.next_feedback = None;
            if let Some(fb) = self.fb_rec.build() {
                let mut w = Writer::with_capacity(64);
                w.u8(Channel::Feedback as u8);
                fb.encode(&mut w);
                if let Some(g) = self.chain.last_good {
                    w.u32(g); // reference-invalidation feedback: the newest decodable frame
                }
                self.seal(now, &w.buf);
                self.next_feedback = Some(now + self.frame_interval);
            }
        }
        for res in self.trains_rx.handle_timeout(now) {
            self.send_train_report(now, res);
        }
        self.trains_tx.retain(|_, t| now.saturating_duration_since(*t) < Duration::from_secs(5));
        self.depack.handle_timeout(now);
        self.drain_video(now);
        for (frame_id, missing) in self.depack.poll_nacks(now) {
            let mut w = Writer::with_capacity(8 + 2 * missing.len());
            w.u8(Channel::Feedback as u8).u8(fbsub::NACK).u32(frame_id).u16(missing.len() as u16);
            for i in &missing {
                w.u16(*i);
            }
            self.seal(now, &w.buf);
            self.stats.nacks_sent += 1;
        }
        let (mp, rtt) = (self.max_payload(), self.clock.rtt());
        for p in self.input_tx.handle_timeout(now, mp, rtt) {
            self.seal(now, &p);
        }
        for p in self.ctrl_tx.poll(now, self.rto()) {
            self.seal(now, &p);
        }
    }

    /// Host: a controller probe when one is due (`cc.rs`) and the link is
    /// quiet: not hedging, no video on the pacer or in the last frame interval,
    /// and feedback has covered the last video packet. One frame's worth of
    /// padding at the probe's rate, in full datagrams, paced as a frame.
    fn poll_probe(&mut self, now: Instant) {
        self.next_probe_check = None;
        if self.cfg.role != Role::Host || self.live.hedging() || self.pacer.queued_bytes() > 0 {
            return; // the pacer's own deadlines bring us back
        }
        let Some(t_us) = self.cc.probe_not_before() else { return };
        let mut at = self.epoch + Duration::from_micros(t_us);
        if let Some((t, _)) = self.last_video_out {
            at = at.max(t + self.frame_interval);
        }
        if now < at {
            self.next_probe_check = Some(at);
            return;
        }
        if self.last_video_out.is_some_and(|(_, c)| !self.cc.reported_through(c)) {
            return; // the feedback that covers it brings us back
        }
        let now_us = self.us(now);
        let Some(bps) = self.cc.probe_due(now_us) else { return };
        let bytes = bps as f64 * self.frame_interval.as_secs_f64() / 8.0;
        let n = (bytes / self.mtu.current as f64).ceil().clamp(1.0, u16::MAX as f64) as usize;
        let id = self.cc.start_probe(now_us, bps, n);
        let len = self.max_payload();
        let packets = (0..n).map(|i| video::padding(id, i as u16, n as u16, len)).collect();
        self.pacer.push_padding(now, packets, self.send_rate_bps(), self.frame_interval / 2);
    }

    /// Client: a fresh handshake inside the session every `REKEY_AFTER`.
    fn poll_rekey(&mut self, now: Instant) {
        let Some(t) = self.transport.as_ref() else { return };
        let Handshake::Initiator(i) = &mut self.handshake else { return };
        let due = now.saturating_duration_since(self.transport_born) >= REKEY_AFTER || t.next_counter() >= REKEY_AFTER_MESSAGES;
        let resend = self.rekeying && i.poll_timeout().is_some_and(|r| now >= r);
        if (due && !self.rekeying) || resend {
            let m = i.start_rekey(now, t.remote_index);
            self.rekeying = true;
            self.out.push_back((m, Via::Active));
        }
    }

    fn rto(&self) -> Duration {
        (self.clock.rtt() * 2).max(Duration::from_millis(20))
    }

    pub fn poll_transmit(&mut self, now: Instant) -> Option<Vec<u8>> {
        self.poll_transmit_via(now).map(|(d, _)| d)
    }

    /// The next datagram and the route it goes on. While `health().hedging`,
    /// `Via::Active` datagrams go on the hedge route too.
    pub fn poll_transmit_via(&mut self, now: Instant) -> Option<(Vec<u8>, Via)> {
        if let Some(p) = self.out.pop_front() {
            return Some(p);
        }
        let plain = self.pacer.poll(now)?;
        match video::padding_probe(&plain) {
            Some(id) => {
                self.seal_full(now, &plain, Via::Active, true, Some(id));
            }
            None => {
                if let Some(c) = self.seal_via(now, &plain, Via::Active, true) {
                    self.last_video_out = Some((now, c));
                }
            }
        }
        self.out.pop_front()
    }

    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// What the session asks of the path layer (suspect, recovered, dead,
    /// probe train results).
    pub fn poll_path_signal(&mut self) -> Option<PathSignal> {
        self.signals.pop_front()
    }

    /// The next deadline for `handle_timeout`. (Datagrams already queued are
    /// not a deadline: drain `poll_transmit` after every call into the session.)
    pub fn poll_timeout(&self) -> Option<Instant> {
        if self.transport.is_none() {
            return match &self.handshake {
                Handshake::Initiator(_) if self.closed => None,
                Handshake::Initiator(i) => Some(i.poll_timeout().unwrap_or(self.epoch)),
                Handshake::Responder(_) => None,
            };
        }
        let rtt = self.clock.rtt();
        let rekey = match &self.handshake {
            Handshake::Initiator(i) if self.rekeying => i.poll_timeout(),
            Handshake::Initiator(_) => Some(self.transport_born + REKEY_AFTER),
            Handshake::Responder(_) => None,
        };
        [
            self.pacer.poll_timeout(),
            self.clock.poll_timeout(),
            self.mtu.poll_timeout(rtt),
            self.next_feedback,
            self.depack.poll_timeout(),
            self.input_tx.poll_timeout(),
            self.ctrl_tx.poll_timeout(self.rto()),
            self.trains_rx.poll_timeout(),
            self.next_probe_check,
            self.prev.as_ref().map(|p| p.1),
            Some(self.live.poll_timeout()),
            Some(self.live.last_rx + self.idle_timeout),
            Some(self.transport_born + REJECT_AFTER),
            rekey,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    // ---- application API -------------------------------------------------

    /// Host: packetize, protect and queue one encoded frame on the pacer.
    pub fn send_video_frame(&mut self, now: Instant, frame: OutgoingFrame) -> u32 {
        self.send_video_frame_with(now, frame, FrameOpts::default())
    }

    /// Host: as `send_video_frame`, naming the frame's reference after a
    /// reference invalidation (`take_reference_hint`) or a recovery point.
    pub fn send_video_frame_with(&mut self, now: Instant, frame: OutgoingFrame, opts: FrameOpts) -> u32 {
        let frame_id = self.next_frame_id;
        self.next_frame_id = self.next_frame_id.wrapping_add(1);
        let mut w = Writer::with_capacity(14 + frame.meta.len() + frame.data.len());
        w.u64(self.us(now));
        let mut flags = if frame.keyframe { video::flags::KEYFRAME } else { 0 };
        if let Some(r) = opts.reference {
            w.u32(r);
            flags |= video::flags::REFERENCE;
        }
        if opts.recovery_point {
            flags |= video::flags::RECOVERY_POINT;
        }
        if frame.keyframe || opts.reference.is_some() || opts.recovery_point {
            self.last_recovery = Some(frame_id);
        }
        w.lp_bytes(&frame.meta).bytes(&frame.data);
        let max_shard = self.max_payload() - VIDEO_HEADER_LEN;
        let fec = self.cc.fec_ratio(self.us(now));
        let packets = video::packetize(frame_id, &w.buf, flags, max_shard, fec);
        let encoded: Vec<Vec<u8>> = packets.iter().map(VideoPacket::encode).collect();
        self.history.insert(now, frame_id, packets);
        if self.pacer.cut_padding() > 0 {
            self.cc.cut_probe(); // a real frame never waits behind a probe
        }
        self.pacer.push_frame(now, encoded, self.send_rate_bps(), self.frame_interval / 2);
        self.content.record_frame(now, self.frame_interval, frame.data.len() + frame.meta.len());
        self.stats.frames_sent += 1;
        frame_id
    }

    /// Host: an animation is in flight (the compositor's HINT start/settle).
    /// With input in the last second, it makes frames latency-critical.
    pub fn set_animation(&mut self, active: bool) {
        self.content.animation = active;
    }

    /// Input in the last second, or an animation in flight.
    pub fn latency_critical(&self, now: Instant) -> bool {
        self.content.latency_critical(now)
    }

    /// Host: what the next frame may use. Read before
    /// every frame; for a second (refinement) frame in the same interval,
    /// read it again and fill `interval_remaining_bytes`.
    pub fn video_budget(&self, now: Instant) -> VideoBudget {
        self.budget(now, true)
    }

    /// The budget of a fresh interval (nothing sent in it yet): what the
    /// driver publishes, since a snapshot can't know when the next frame
    /// comes. The host then gives a refinement frame
    /// `interval_remaining_bytes` less what its interactive frame took.
    pub fn interval_budget(&self, now: Instant) -> VideoBudget {
        self.budget(now, false)
    }

    fn budget(&self, now: Instant, count_sent: bool) -> VideoBudget {
        let fec = 1.0 + self.cc.fec_ratio(self.us(now)) as f64;
        let hedging = self.live.hedging();
        let rate = self.send_rate_bps() as f64;
        let critical = self.content.latency_critical(now);
        let c_est = if hedging { None } else { self.cc.c_est() };
        let capped = critical && c_est.is_some();
        let frame_rate = match c_est {
            Some(c) if capped => rate.min(CRITICAL_SHARE * c as f64),
            _ => rate,
        };
        let iv = self.frame_interval.as_secs_f64();
        let sent = if count_sent { self.content.sent_this_interval(now, self.frame_interval) } else { 0 };
        let interval_remaining = ((rate / fec * iv / 8.0) as usize).saturating_sub(sent);
        let frame_bytes = ((frame_rate / fec * iv / 8.0) as usize).min(interval_remaining);
        VideoBudget {
            latency_critical: critical,
            capped,
            thin: hedging,
            encoder_bps: (frame_bytes as f64 * 8.0 / iv) as u64,
            frame_bytes,
            interval_remaining_bytes: interval_remaining,
        }
    }

    /// Host: link state for the encoder's split rule.
    pub fn link_state(&self, now: Instant) -> LinkState {
        let c_est_bps = self.cc.c_est();
        let free = self.cc.queue_free_for(self.us(now));
        let queue_free_3s = free >= QUEUE_FREE_FOR_SPLIT;
        LinkState { c_est_bps, queue_free_ms: free.as_millis() as u64, queue_free_3s, split_ok: c_est_bps.is_none() && queue_free_3s }
    }

    /// Host: which frame to predict the next one from, once after a loss or
    /// when a failover starts. None: carry on as usual.
    pub fn take_reference_hint(&mut self) -> Option<RefHint> {
        self.ref_hint.take()
    }

    /// Host: the newest frame the client reported decodable.
    pub fn last_acked_frame(&self) -> Option<u32> {
        self.last_acked
    }

    /// Liveness of the active route and the hedge state.
    pub fn health(&self, now: Instant) -> Health {
        self.live.health(now)
    }

    /// The driver says what the hedge route last measured (bps), which can
    /// thin the stream further.
    pub fn set_hedge_capacity(&mut self, bps: Option<u64>) {
        self.hedge_capacity = bps;
    }

    /// Put a probe train on the wire (`Via::Train(id)`): the driver sends its
    /// datagrams on the route it measures, and the result comes back as
    /// `PathSignal::TrainReport`. None before the session is up.
    pub fn start_probe_train(&mut self, now: Instant) -> Option<u32> {
        self.transport.as_ref()?;
        let id = self.next_train;
        self.next_train = self.next_train.wrapping_add(1);
        for i in 0..TRAIN_LEN {
            let mut w = Writer::with_capacity(TRAIN_DATAGRAM);
            w.u8(Channel::Probe as u8).u8(probe::TRAIN).u32(id).u16(i).u16(TRAIN_LEN);
            w.buf.resize(TRAIN_DATAGRAM - DATA_OVERHEAD, 0);
            self.seal_via(now, &w.buf, Via::Train(id), false);
        }
        self.trains_tx.insert(id, now);
        Some(id)
    }

    /// Client: send one input event, repeated until the host acknowledges it.
    pub fn send_input(&mut self, now: Instant, event: Vec<u8>) {
        let (mp, rtt) = (self.max_payload(), self.clock.rtt());
        for p in self.input_tx.push(now, event, mp, rtt) {
            self.seal(now, &p);
        }
    }

    /// Reliable, ordered control message (kind 0..=239 is the application's).
    pub fn send_control(&mut self, now: Instant, kind: u8, data: &[u8]) {
        let mp = self.max_payload();
        self.ctrl_tx.send(kind, data, mp);
        for p in self.ctrl_tx.poll(now, self.rto()) {
            self.seal(now, &p);
        }
    }

    /// One of our ICE candidates, gathered while the session runs (after a
    /// network change), sent in-band and reliably. The peer's session hands
    /// it to its path layer as `PathSignal::RemoteCandidate`.
    pub fn send_candidate(&mut self, now: Instant, sdp: &str) {
        self.send_control(now, control_kind::CANDIDATE, sdp.as_bytes());
    }

    /// Host: the cursor image, sent reliably once per hash.
    pub fn send_cursor_shape(&mut self, now: Instant, hash: u64, image: &[u8]) {
        if self.shapes_sent.insert(hash) {
            let mut data = hash.to_be_bytes().to_vec();
            data.extend_from_slice(image);
            self.send_control(now, control_kind::CURSOR_SHAPE, &data);
        }
    }

    /// Host: the cursor position, unreliable and latest-wins.
    pub fn send_cursor_position(&mut self, now: Instant, x: f32, y: f32, visible: bool, shape: u64) {
        self.cursor_seq = self.cursor_seq.wrapping_add(1);
        let mut w = Writer::with_capacity(24);
        w.u8(Channel::Cursor as u8).u8(cursor::POSITION).u32(self.cursor_seq).f32(x).f32(y).u8(visible as u8).u64(shape);
        self.seal(now, &w.buf);
    }

    /// Audio (host to client) or mic (client to host): one Opus packet,
    /// captured now.
    pub fn send_audio(&mut self, now: Instant, mic: bool, timestamp: u32, opus: &[u8]) {
        let capture_us = self.us(now);
        self.send_audio_frame(now, mic, timestamp, capture_us, opus);
    }

    /// One Opus packet with its capture time on this session's clock
    /// (`now_us`), for A/V sync on the other side.
    pub fn send_audio_frame(&mut self, now: Instant, mic: bool, timestamp: u32, capture_us: u64, opus: &[u8]) {
        let i = mic as usize;
        self.audio_seq[i] = self.audio_seq[i].wrapping_add(1);
        let channel = if mic { Channel::Mic } else { Channel::Audio };
        let mut w = Writer::with_capacity(17 + opus.len());
        w.u8(channel as u8).u32(self.audio_seq[i]).u32(timestamp).u64(capture_us).bytes(opus);
        self.seal(now, &w.buf);
    }

    /// What to play for the next 10 ms of received audio (`mic`: the host's
    /// microphone stream), from the jitter buffer. None while it fills.
    pub fn poll_audio(&mut self, now: Instant, mic: bool) -> Option<AudioOut> {
        self.jitter[mic as usize].lock().unwrap().pop(now)
    }

    /// The jitter buffer itself, for an audio callback on another thread: it
    /// calls `pop(Instant::now())` every 10 ms while the session fills it.
    pub fn audio_buffer(&self, mic: bool) -> Arc<Mutex<JitterBuffer>> {
        self.jitter[mic as usize].clone()
    }

    pub fn audio_stats(&self, mic: bool) -> JitterStats {
        self.jitter[mic as usize].lock().unwrap().stats()
    }

    /// Host: loss on the way to the client over the last 2 s, for Opus's
    /// expected packet loss (`OPUS_SET_PACKET_LOSS_PERC`), which sizes its
    /// in-band FEC. None on the client (nothing reports its sending loss).
    pub fn send_loss(&self, now: Instant) -> Option<f64> {
        (self.cfg.role == Role::Host).then(|| self.cc.recent_loss(self.us(now)))
    }

    /// Client: per-frame decode and present times for the host's stats.
    pub fn send_receiver_report(&mut self, now: Instant, frame_id: u32, decode_us: u32, present_us: u32) {
        let mut w = Writer::with_capacity(14);
        w.u8(Channel::Feedback as u8).u8(fbsub::RECEIVER_REPORT).u32(frame_id).u32(decode_us).u32(present_us);
        self.seal(now, &w.buf);
    }

    pub fn stats(&self, now: Instant) -> SessionStats {
        let mut s = self.stats.clone();
        s.connected = self.transport.is_some();
        s.rtt_ms = self.clock.srtt.map_or(0.0, |d| d.as_secs_f64() * 1e3);
        s.clock_offset_us = self.clock.offset_us.unwrap_or(0);
        s.mtu = self.mtu.current;
        s.target_bps = self.cc.target_bps();
        s.encoder_bps = self.encoder_bitrate_bps(now);
        s.fec_ratio = self.cc.fec_ratio(self.us(now));
        s.cc_state = Some(self.cc.state);
        s.queue_delay_ms = self.cc.queue_delay_us as f64 / 1e3;
        s.receive_rate_bps = self.cc.receive_rate_bps;
        s.loss = self.cc.loss;
        s.pacer_queue_bytes = self.pacer.queued_bytes();
        s.frames_lost = self.depack.frames_lost;
        s.recovered_shards = self.depack.recovered_shards;
        s.video_packets_received = self.depack.packets;
        s.nack_enabled = self.depack.nack_enabled;
        s.c_est_bps = self.cc.c_est().unwrap_or(0);
        s.queue_free_ms = self.cc.queue_free_for(self.us(now)).as_millis() as u64;
        s.latency_critical = self.content.latency_critical(now);
        s.health = self.live.state;
        s.hedging = self.live.hedging();
        let p = self.cc.probes;
        s.probe_bps = p.last_bps;
        s.probe_ceiling_bps = self.cc.probe_ceiling().unwrap_or(0);
        (s.probes, s.probes_accepted, s.probes_refused, s.probes_decreased) = (p.sent, p.accepted, p.refused, p.decreased);
        s.probe_queue_max_ms = p.queue_max_us as f64 / 1e3;
        s
    }

    /// Microseconds since this session started, on our clock.
    pub fn now_us(&self, now: Instant) -> u64 {
        self.us(now)
    }

    /// The session clock's origin (`now_us` counts from here).
    pub fn epoch(&self) -> Instant {
        self.epoch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shuttle datagrams between two sessions in `step`s until `until`.
    /// `drop(n, from_host)` drops the nth datagram.
    fn pump_with(
        host: &mut Session,
        client: &mut Session,
        mut now: Instant,
        until: Instant,
        step: Duration,
        mut drop: impl FnMut(u64, bool) -> bool,
    ) -> Instant {
        let mut n = 0u64;
        while now < until {
            host.handle_timeout(now);
            client.handle_timeout(now);
            while let Some(p) = client.poll_transmit(now) {
                n += 1;
                if !drop(n, false) {
                    host.handle_datagram(now, &p);
                }
            }
            while let Some(p) = host.poll_transmit(now) {
                n += 1;
                if !drop(n, true) {
                    client.handle_datagram(now, &p);
                }
            }
            now += step;
        }
        now
    }

    /// Drop every `drop_every`th host datagram (0: none).
    fn pump(host: &mut Session, client: &mut Session, now: Instant, until: Instant, drop_every: u64) -> Instant {
        let mut k = 0u64;
        pump_with(host, client, now, until, Duration::from_micros(500), |_, from_host| {
            if !from_host {
                return false;
            }
            k += 1;
            drop_every != 0 && k % drop_every == 0
        })
    }

    fn pair(now: Instant) -> (Session, Session) {
        let hk = StaticKeypair::generate();
        let ck = StaticKeypair::generate();
        let host = Session::new(SessionConfig::new(Role::Host), hk.clone(), Peer::PairedClients(vec![ck.public]), now);
        let client = Session::new(SessionConfig::new(Role::Client), ck, Peer::Host(hk.public), now);
        (host, client)
    }

    fn frame(i: u32, len: u32) -> OutgoingFrame {
        OutgoingFrame { data: (0..len).map(|b| (b ^ i) as u8).collect(), keyframe: i == 0, meta: vec![1, 2] }
    }

    #[test]
    fn connect_stream_frames_through_loss_and_deliver_input_and_control() {
        let t0 = Instant::now();
        let (mut host, mut client) = pair(t0);
        let mut now = pump(&mut host, &mut client, t0, t0 + Duration::from_millis(50), 0);
        assert!(matches!(host.poll_event(), Some(Event::Connected { .. })));
        assert!(matches!(client.poll_event(), Some(Event::Connected { .. })));

        // 30 frames of 40 KB with every 50th host datagram lost.
        for i in 0..30u32 {
            host.send_video_frame(now, frame(i, 40_000));
            now = pump(&mut host, &mut client, now, now + Duration::from_micros(16_667), 50);
        }
        now = pump(&mut host, &mut client, now, now + Duration::from_millis(100), 0);
        let mut frames = 0;
        while let Some(e) = client.poll_event() {
            if let Event::VideoFrame(f) = e {
                assert_eq!(f.data.len(), 40_000);
                assert_eq!(f.meta, vec![1, 2]);
                assert!(f.decodable);
                frames += 1;
            }
        }
        // Lost packets are rebuilt by FEC once loss is measured, or by NACK (RTT ~0).
        assert_eq!(frames, 30, "{:?}", client.stats(now));

        client.send_input(now, b"key A".to_vec());
        client.send_control(now, 3, b"resize 2560x1664");
        host.send_cursor_position(now, 10.0, 20.0, true, 7);
        pump(&mut host, &mut client, now, now + Duration::from_millis(5), 0);
        let host_events: Vec<Event> = std::iter::from_fn(|| host.poll_event()).collect();
        assert!(host_events.contains(&Event::Input(b"key A".to_vec())));
        assert!(host_events.contains(&Event::Control { kind: 3, data: b"resize 2560x1664".to_vec() }));
        assert!(std::iter::from_fn(|| client.poll_event()).any(|e| matches!(e, Event::CursorPosition { shape: 7, .. })));
        assert!(host.latency_critical(now), "input in the last second");
    }

    /// Stream at 60 fps for `ms`, dropping everything inside `blackout`.
    /// The host's path signals land in `log` with their times.
    fn stream(
        host: &mut Session,
        client: &mut Session,
        mut now: Instant,
        ms: u64,
        blackout: std::ops::Range<Instant>,
        log: &mut Vec<(Instant, PathSignal, u64)>,
    ) -> Instant {
        let end = now + Duration::from_millis(ms);
        let mut next_frame = now;
        let mut i = 1u32;
        while now < end {
            if now >= next_frame {
                let bytes = host.video_budget(now).frame_bytes.max(500) as u32;
                host.send_video_frame(now, frame(i, bytes));
                i += 1;
                next_frame += Duration::from_micros(16_667);
            }
            let b = blackout.clone();
            let t = now;
            now = pump_with(host, client, now, now + Duration::from_millis(1), Duration::from_micros(250), |_, _| b.contains(&t));
            while let Some(s) = host.poll_path_signal() {
                log.push((now, s, host.stats(now).target_bps));
            }
            while client.poll_path_signal().is_some() {}
        }
        now
    }

    #[test]
    fn a_blackout_hedges_then_the_route_answers_and_a_longer_one_is_dead() {
        let t0 = Instant::now();
        let (mut host, mut client) = pair(t0);
        let mut now = pump(&mut host, &mut client, t0, t0 + Duration::from_millis(50), 0);
        let mut log = Vec::new();
        host.send_video_frame(now, frame(0, 3000)); // a keyframe starts the decodable chain
        now = stream(&mut host, &mut client, now, 1000, now..now, &mut log);
        assert!(log.is_empty(), "a healthy stream raises nothing: {log:?}");
        let before = host.stats(now).target_bps;

        // 300 ms blackout: suspect at 200 ms, no path change, and the target is
        // back the moment the active route answers.
        let cut = now;
        now = stream(&mut host, &mut client, now, 600, cut..cut + Duration::from_millis(300), &mut log);
        let suspect = log.iter().find(|s| s.1 == PathSignal::Suspect).expect("suspect").0 - cut;
        // Measured from the last answer before the cut (feedback comes once a frame).
        assert!(suspect + Duration::from_millis(20) >= SUSPECT_AFTER && suspect <= SUSPECT_AFTER + Duration::from_millis(5), "{suspect:?}");
        let back = log.iter().find(|s| s.1 == PathSignal::Recovered).expect("recovered");
        assert!(back.0 - cut < Duration::from_millis(420), "{:?}", back.0 - cut);
        assert!(!log.iter().any(|s| s.1 == PathSignal::Dead));
        assert_eq!(back.2, before, "the pre-suspect target, no re-ramp");
        assert!(matches!(host.take_reference_hint(), Some(RefHint::Reference(_))));

        // 900 ms: thin while suspect, dead at 700 ms; the path change
        // re-baselines at the thin rate.
        log.clear();
        let cut = now;
        let mut thin = None;
        while now < cut + Duration::from_millis(750) {
            now = stream(&mut host, &mut client, now, 10, cut..cut + Duration::from_millis(900), &mut log);
            if host.health(now).hedging {
                thin.get_or_insert(host.video_budget(now));
            }
        }
        assert!(thin.is_some_and(|b| b.thin && b.encoder_bps <= THIN_MAX_BPS), "{thin:?}");
        let dead = log.iter().find(|s| s.1 == PathSignal::Dead).expect("dead").0 - cut;
        assert!(dead + Duration::from_millis(20) >= DEAD_AFTER && dead <= DEAD_AFTER + Duration::from_millis(5), "{dead:?}");
        host.path_changed(now);
        assert_eq!(host.stats(now).target_bps, THIN_MAX_BPS.min(before));
        assert!(!host.health(now).hedging);
    }

    #[test]
    fn rekey_keeps_the_session_and_silence_ends_it() {
        let t0 = Instant::now();
        let (mut host, mut client) = pair(t0);
        let step = Duration::from_millis(5);
        let mut now = pump_with(&mut host, &mut client, t0, t0 + REKEY_AFTER + Duration::from_secs(2), step, |_, _| false);
        assert_eq!((host.stats(now).rekeys, client.stats(now).rekeys), (1, 1));
        let connects = std::iter::from_fn(|| host.poll_event()).filter(|e| matches!(e, Event::Connected { .. })).count();
        assert_eq!(connects, 1, "a rekey is not a new session");
        // The stream carries on under the new keys.
        client.send_input(now, b"after".to_vec());
        now = pump_with(&mut host, &mut client, now, now + Duration::from_millis(20), step, |_, _| false);
        assert!(std::iter::from_fn(|| host.poll_event()).any(|e| e == Event::Input(b"after".to_vec())));

        now = pump_with(&mut host, &mut client, now, now + DEFAULT_IDLE_TIMEOUT + step * 2, Duration::from_millis(50), |_, _| true);
        let gone = |s: &mut Session| std::iter::from_fn(|| s.poll_event()).any(|e| e == Event::Disconnected { reason: DisconnectReason::IdleTimeout });
        assert!(gone(&mut host) && gone(&mut client));
        assert!(!host.is_connected() && client.poll_timeout().is_none(), "the client waits for reconnect()");
        client.reconnect(now);
        pump_with(&mut host, &mut client, now, now + Duration::from_millis(20), step, |_, _| false);
        assert!(host.is_connected() && client.is_connected());
    }

    #[test]
    fn a_lost_frame_points_the_encoder_at_the_last_decodable_one() {
        let t0 = Instant::now();
        let (mut host, mut client) = pair(t0);
        let mut now = pump(&mut host, &mut client, t0, t0 + Duration::from_millis(50), 0);
        for i in 0..3u32 {
            host.send_video_frame(now, frame(i, 3000));
            now = pump(&mut host, &mut client, now, now + Duration::from_millis(17), 0);
        }
        let lost = host.send_video_frame(now, frame(3, 3000));
        let mut video = 0;
        now = pump_with(&mut host, &mut client, now, now + Duration::from_millis(17), Duration::from_micros(500), |_, h| {
            video += h as u32;
            h && video <= 3 // all of frame 3's packets (and their NACKed resends) are lost
        });
        host.send_video_frame(now, frame(4, 3000)); // predicted from the lost one
        now = pump(&mut host, &mut client, now, now + Duration::from_millis(60), 0);
        assert!(std::iter::from_fn(|| host.poll_event()).any(|e| e == Event::PeerLostFrame { frame_id: lost }));
        assert_eq!(host.take_reference_hint(), Some(RefHint::Reference(lost - 1)));
        let recovery = host.send_video_frame_with(now, frame(5, 3000), FrameOpts { reference: Some(lost - 1), ..Default::default() });
        pump(&mut host, &mut client, now, now + Duration::from_millis(40), 0);
        let frames: Vec<(u32, bool)> = std::iter::from_fn(|| client.poll_event())
            .filter_map(|e| if let Event::VideoFrame(f) = e { Some((f.frame_id, f.decodable)) } else { None })
            .collect();
        assert_eq!(frames.last(), Some(&(recovery, true)));
        assert!(frames.contains(&(lost + 1, false)), "{frames:?}");
        assert_eq!(host.last_acked_frame(), Some(recovery));
    }

    #[test]
    fn padding_probes_lift_an_idle_target_and_a_frame_cuts_a_probe() {
        let t0 = Instant::now();
        let (mut host, mut client) = pair(t0);
        host.set_route_demand(65_000_000);
        let mut now = pump(&mut host, &mut client, t0, t0 + Duration::from_millis(50), 0);
        host.send_video_frame(now, frame(0, 3000));
        now = pump(&mut host, &mut client, now, now + Duration::from_millis(600), 0);
        let s = host.stats(now);
        assert!(s.probes_accepted >= 2 && s.target_bps > 12_000_000, "{s:?}");
        // Catch the next probe on the pacer, then a frame comes.
        let probes = s.probes;
        loop {
            host.handle_timeout(now);
            if host.stats(now).probes > probes {
                break;
            }
            now = pump(&mut host, &mut client, now, now + Duration::from_micros(500), 0);
        }
        assert!(host.stats(now).pacer_queue_bytes > 10_000);
        host.send_video_frame(now, frame(1, 3000));
        assert!(host.stats(now).pacer_queue_bytes < 4_000, "the padding is gone, the frame waits for nothing");
        pump(&mut host, &mut client, now, now + Duration::from_millis(50), 0);
        let frames: Vec<u32> = std::iter::from_fn(|| client.poll_event())
            .filter_map(|e| if let Event::VideoFrame(f) = e { Some(f.frame_id) } else { None })
            .collect();
        assert_eq!(frames, vec![0, 1], "padding never reaches the decoder");
        assert_eq!(client.stats(now).frames_lost, 0);
    }

    #[test]
    fn a_probe_train_reports_rate_and_loss() {
        let t0 = Instant::now();
        let (mut host, mut client) = pair(t0);
        let now = pump(&mut host, &mut client, t0, t0 + Duration::from_millis(50), 0);
        let id = host.start_probe_train(now).unwrap();
        // One train packet every 0.5 ms; the 5th is lost.
        let mut sent = 0;
        let mut t = now;
        while sent < TRAIN_LEN {
            let Some((p, via)) = host.poll_transmit_via(t) else { break };
            if via == Via::Train(id) {
                sent += 1;
                if sent != 5 {
                    client.handle_datagram(t, &p);
                }
                t += Duration::from_micros(500);
            }
        }
        assert_eq!(sent, TRAIN_LEN);
        pump(&mut host, &mut client, t, t + Duration::from_millis(5), 0);
        let report = std::iter::from_fn(|| host.poll_path_signal()).find(|s| matches!(s, PathSignal::TrainReport { .. }));
        let Some(PathSignal::TrainReport { train_id, rate_bps, loss }) = report else { panic!("{report:?}") };
        assert_eq!(train_id, id);
        // 1200-byte datagrams every 0.5 ms, the first and the lost one not counted.
        let expect = (TRAIN_LEN - 2) as f64 * 1200.0 * 8.0 / ((TRAIN_LEN - 1) as f64 * 0.5e-3);
        assert!((rate_bps as f64 - expect).abs() < 1e6, "{rate_bps} vs {expect}");
        assert!((loss - 1.0 / TRAIN_LEN as f64).abs() < 1e-6);
    }
}
