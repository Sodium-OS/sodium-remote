// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Session crypto: a Noise IK handshake with pinned static keys, then per-packet
//! ChaCha20-Poly1305 with an explicit 64-bit counter and a replay window
//! (WireGuard's design).
//!
//! The client is the initiator: it already knows (has pinned) the host's static
//! key. The host is the responder and accepts only static keys it has pinned.
//!
//! Rekeying is a fresh IK handshake inside the running session (forward
//! secrecy for long sessions, as WireGuard does every 2 minutes). The first
//! message's encrypted payload then names the session it rekeys, so the host
//! swaps keys instead of starting over; counters carry on across the swap
//! because they double as the transport-wide sequence.

use crate::wire::{kind, Reader, Writer};
use snow::{Builder, HandshakeState, StatelessTransportState};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const NOISE_PARAMS: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
/// Binds the handshake to this protocol and version.
pub const PROLOGUE: &[u8] = b"SodiumOS Remote wire v3";
pub const TAG_LEN: usize = 16;
/// Outer header of a data packet: kind, receiver index, counter.
pub const DATA_HEADER_LEN: usize = 1 + 4 + 8;
/// Bytes a data packet adds around its plaintext.
pub const DATA_OVERHEAD: usize = DATA_HEADER_LEN + TAG_LEN;
const HANDSHAKE_RTO: Duration = Duration::from_millis(400);
const MAX_NOISE_MSG: usize = 65535;

/// A device's long-term X25519 key pair (the "device key" of).
#[derive(Clone)]
pub struct StaticKeypair {
    pub private: Vec<u8>,
    pub public: [u8; 32],
}

impl StaticKeypair {
    pub fn generate() -> Self {
        let kp = Builder::new(NOISE_PARAMS.parse().expect("params"))
            .generate_keypair()
            .expect("keypair");
        StaticKeypair {
            private: kp.private,
            public: kp.public.try_into().expect("32-byte public key"),
        }
    }
}

impl std::fmt::Debug for StaticKeypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StaticKeypair({})", hex(&self.public))
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn builder<'a>() -> Builder<'a> {
    Builder::new(NOISE_PARAMS.parse().expect("params"))
        .prologue(PROLOGUE)
        .expect("prologue")
}

fn unix_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// The anti-replay window: 2048 counters, as in WireGuard (RFC 6479 style ring).
pub struct ReplayWindow {
    /// One more than the highest counter accepted; 0 means none yet.
    next: u64,
    bits: [u64; Self::WORDS],
}

impl Default for ReplayWindow {
    fn default() -> Self {
        ReplayWindow { next: 0, bits: [0; Self::WORDS] }
    }
}

impl ReplayWindow {
    const WORDS: usize = 32;
    pub const SIZE: u64 = (Self::WORDS as u64) * 64;

    fn bit(&self, n: u64) -> bool {
        let i = (n % Self::SIZE) as usize;
        self.bits[i / 64] & (1 << (i % 64)) != 0
    }

    /// Would `n` be accepted? (Checked before decrypting; recorded only after.)
    pub fn check(&self, n: u64) -> bool {
        if n >= self.next {
            return true;
        }
        // Keep a margin of one word so the ring never aliases an unseen counter.
        if n + Self::SIZE - 64 < self.next {
            return false;
        }
        !self.bit(n)
    }

    pub fn record(&mut self, n: u64) {
        if n >= self.next {
            let advance = n + 1 - self.next;
            if advance >= Self::SIZE {
                self.bits = [0; Self::WORDS];
            } else {
                for m in self.next..=n {
                    let i = (m % Self::SIZE) as usize;
                    self.bits[i / 64] &= !(1 << (i % 64));
                }
            }
            self.next = n + 1;
        }
        let i = (n % Self::SIZE) as usize;
        self.bits[i / 64] |= 1 << (i % 64);
    }
}

/// Keys and counters of an established session.
pub struct Transport {
    state: StatelessTransportState,
    /// Our index: the peer writes it in packets meant for us.
    pub local_index: u32,
    /// The peer's index: we write it in packets meant for the peer.
    pub remote_index: u32,
    send_counter: u64,
    replay: ReplayWindow,
    pub remote_static: [u8; 32],
}

impl Transport {
    /// The counter the next sealed packet will carry. It doubles as the
    /// transport-wide sequence number for congestion feedback.
    pub fn next_counter(&self) -> u64 {
        self.send_counter
    }

    /// Carry the counter on from the keys this transport replaces.
    pub fn set_send_counter(&mut self, counter: u64) {
        self.send_counter = counter;
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> (u64, Vec<u8>) {
        let counter = self.send_counter;
        self.send_counter += 1;
        let mut out = vec![0u8; DATA_HEADER_LEN + plaintext.len() + TAG_LEN];
        out[0] = kind::DATA;
        out[1..5].copy_from_slice(&self.remote_index.to_be_bytes());
        out[5..13].copy_from_slice(&counter.to_be_bytes());
        let n = self
            .state
            .write_message(counter, plaintext, &mut out[DATA_HEADER_LEN..])
            .expect("seal");
        out.truncate(DATA_HEADER_LEN + n);
        (counter, out)
    }

    /// Decrypts a data packet. `None` for a wrong index, a replay or a bad tag.
    pub fn open(&mut self, packet: &[u8]) -> Option<(u64, Vec<u8>)> {
        if packet.len() < DATA_OVERHEAD || packet[0] != kind::DATA {
            return None;
        }
        let index = u32::from_be_bytes(packet[1..5].try_into().ok()?);
        if index != self.local_index {
            return None;
        }
        let counter = u64::from_be_bytes(packet[5..13].try_into().ok()?);
        if !self.replay.check(counter) {
            return None;
        }
        let mut out = vec![0u8; packet.len() - DATA_HEADER_LEN];
        let n = self
            .state
            .read_message(counter, &packet[DATA_HEADER_LEN..], &mut out)
            .ok()?;
        self.replay.record(counter);
        out.truncate(n);
        Some((counter, out))
    }
}

/// The client side of the handshake. It sends a fresh first message on every
/// retransmit (new ephemeral key and timestamp), like WireGuard.
pub struct Initiator {
    local: StaticKeypair,
    remote_static: [u8; 32],
    state: Option<HandshakeState>,
    index: u32,
    next_send: Option<Instant>,
}

impl Initiator {
    pub fn new(local: StaticKeypair, remote_static: [u8; 32]) -> Self {
        Initiator { local, remote_static, state: None, index: 0, next_send: None }
    }

    pub fn poll_timeout(&self) -> Option<Instant> {
        self.next_send
    }

    /// Build a new first message.
    pub fn start(&mut self, now: Instant) -> Vec<u8> {
        self.start_with(now, None)
    }

    /// A first message that rekeys the running session whose host-side index
    /// (our `remote_index`) is `rekey_of`.
    pub fn start_rekey(&mut self, now: Instant, rekey_of: u32) -> Vec<u8> {
        self.start_with(now, Some(rekey_of))
    }

    fn start_with(&mut self, now: Instant, rekey_of: Option<u32>) -> Vec<u8> {
        let mut hs = builder()
            .local_private_key(&self.local.private)
            .expect("key")
            .remote_public_key(&self.remote_static)
            .expect("key")
            .build_initiator()
            .expect("initiator");
        self.index = rand::random();
        let mut msg = vec![0u8; MAX_NOISE_MSG];
        let mut payload = unix_micros().to_be_bytes().to_vec();
        if let Some(i) = rekey_of {
            payload.extend_from_slice(&i.to_be_bytes());
        }
        let n = hs.write_message(&payload, &mut msg).expect("msg1");
        let mut w = Writer::with_capacity(5 + n);
        w.u8(kind::HANDSHAKE_INIT).u32(self.index).bytes(&msg[..n]);
        self.state = Some(hs);
        self.next_send = Some(now + HANDSHAKE_RTO);
        w.buf
    }

    /// Handle a response; returns the transport on success.
    pub fn handle_response(&mut self, packet: &[u8]) -> Option<Transport> {
        let mut r = Reader::new(packet);
        if r.u8()? != kind::HANDSHAKE_RESP {
            return None;
        }
        let remote_index = r.u32()?;
        let receiver = r.u32()?;
        if receiver != self.index {
            return None;
        }
        // The receiver index travels in the clear, so anyone who sees the first message can
        // answer it. Only an authentic response ends the attempt: a forged one leaves the
        // state as it was (snow rolls a failed read back), and the real one still completes.
        let mut payload = vec![0u8; MAX_NOISE_MSG];
        if self.state.as_mut()?.read_message(r.rest(), &mut payload).is_err() {
            return None;
        }
        let hs = self.state.take()?;
        self.next_send = None;
        Some(Transport {
            state: hs.into_stateless_transport_mode().ok()?,
            local_index: self.index,
            remote_index,
            send_counter: 0,
            replay: ReplayWindow::default(),
            remote_static: self.remote_static,
        })
    }
}

/// The host side: answers first messages from pinned keys only.
pub struct Responder {
    local: StaticKeypair,
    pinned: Vec<[u8; 32]>,
    /// Newest timestamp seen per pinned key, to refuse replayed first messages.
    last_timestamp: Vec<([u8; 32], u64)>,
}

impl Responder {
    pub fn new(local: StaticKeypair, pinned: Vec<[u8; 32]>) -> Self {
        Responder { local, pinned, last_timestamp: Vec::new() }
    }

    /// Handle a first message; returns the response to send and the transport.
    pub fn handle_init(&mut self, packet: &[u8]) -> Option<(Vec<u8>, Transport)> {
        self.handle_init_ex(packet).map(|(resp, t, _)| (resp, t))
    }

    /// As `handle_init`, plus the session index a rekey names (None for a new session).
    pub fn handle_init_ex(&mut self, packet: &[u8]) -> Option<(Vec<u8>, Transport, Option<u32>)> {
        let mut r = Reader::new(packet);
        if r.u8()? != kind::HANDSHAKE_INIT {
            return None;
        }
        let remote_index = r.u32()?;
        let mut hs = builder()
            .local_private_key(&self.local.private)
            .ok()?
            .build_responder()
            .ok()?;
        let mut payload = vec![0u8; MAX_NOISE_MSG];
        let n = hs.read_message(r.rest(), &mut payload).ok()?;
        let remote_static: [u8; 32] = hs.get_remote_static()?.try_into().ok()?;
        if !self.pinned.contains(&remote_static) {
            tracing::warn!("handshake from an unpinned key {}", hex(&remote_static));
            return None;
        }
        let ts = u64::from_be_bytes(payload.get(..8)?.try_into().ok()?);
        let rekey_of = match n {
            8 => None,
            12 => Some(u32::from_be_bytes(payload[8..12].try_into().ok()?)),
            _ => return None,
        };
        match self.last_timestamp.iter_mut().find(|(k, _)| *k == remote_static) {
            Some((_, last)) if ts <= *last => return None,
            Some((_, last)) => *last = ts,
            None => self.last_timestamp.push((remote_static, ts)),
        }
        let index: u32 = rand::random();
        let mut msg = vec![0u8; MAX_NOISE_MSG];
        let m = hs.write_message(&[], &mut msg).ok()?;
        let mut w = Writer::with_capacity(9 + m);
        w.u8(kind::HANDSHAKE_RESP).u32(index).u32(remote_index).bytes(&msg[..m]);
        let transport = Transport {
            state: hs.into_stateless_transport_mode().ok()?,
            local_index: index,
            remote_index,
            send_counter: 0,
            replay: ReplayWindow::default(),
            remote_static,
        };
        Some((w.buf, transport, rekey_of))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_then_seal_open_and_replay() {
        let host = StaticKeypair::generate();
        let client = StaticKeypair::generate();
        let now = Instant::now();
        let mut ini = Initiator::new(client.clone(), host.public);
        let mut resp = Responder::new(host.clone(), vec![client.public]);
        let m1 = ini.start(now);
        let (m2, mut host_t) = resp.handle_init(&m1).expect("host accepts a pinned key");
        let mut client_t = ini.handle_response(&m2).expect("client finishes");
        assert_eq!(host_t.remote_static, client.public);

        let (_, p) = client_t.seal(b"hello");
        assert_eq!(host_t.open(&p).unwrap().1, b"hello");
        assert!(host_t.open(&p).is_none(), "a replayed packet is dropped");
        let (_, back) = host_t.seal(b"hi");
        assert_eq!(client_t.open(&back).unwrap().1, b"hi");

        // A replayed first message is refused.
        assert!(resp.handle_init(&m1).is_none());
    }

    #[test]
    fn a_forged_response_does_not_end_the_handshake() {
        let host = StaticKeypair::generate();
        let client = StaticKeypair::generate();
        let mut ini = Initiator::new(client.clone(), host.public);
        let mut resp = Responder::new(host, vec![client.public]);
        let m1 = ini.start(Instant::now());
        let (m2, _) = resp.handle_init(&m1).unwrap();
        // Same header (indices are in the clear), garbage Noise payload.
        let mut forged = m2.clone();
        for b in &mut forged[9..] {
            *b ^= 0x5a;
        }
        assert!(ini.handle_response(&forged).is_none());
        assert!(ini.handle_response(&m2).is_some(), "the real response still completes");
    }

    #[test]
    fn unpinned_key_is_refused() {
        let host = StaticKeypair::generate();
        let stranger = StaticKeypair::generate();
        let mut ini = Initiator::new(stranger, host.public);
        let mut resp = Responder::new(host, vec![StaticKeypair::generate().public]);
        assert!(resp.handle_init(&ini.start(Instant::now())).is_none());
    }

    #[test]
    fn replay_window_edges() {
        let mut w = ReplayWindow::default();
        w.record(5000);
        assert!(w.check(5001));
        assert!(w.check(5000 - 100), "reordered within the window is fine");
        assert!(!w.check(5000));
        assert!(!w.check(5000 - ReplayWindow::SIZE), "too old");
    }
}
