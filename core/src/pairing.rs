// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Pairing with a 6-digit code: SPAKE2, sans-IO (design.md 4,
//! rendezvous-api.md §7).
//!
//! The host shows the code and opens a window; the client types it. The code
//! never leaves the client, not even hashed: only SPAKE2 messages travel, as
//! opaque blobs through the rendezvous (`pair_msg`) or the LAN listener.
//!
//! ```text
//! C → H  M1  spake_A
//! H → C  M2  spake_B, host ID, host name, confirm_H = HMAC(K_h, T)
//! C → H  M3  AEAD(K_c, client ID ‖ client name; aad T)
//! H → C  M4  done = HMAC(K_d, T ‖ client ID)
//! ```
//!
//! - SPAKE2 identities: A = `"remote client"`, B = the host's device ID, so a
//!   relay that lies about which host it is makes the keys disagree.
//! - **By address** (M1 `v: 2`): a client that knows only the host's address doesn't know
//!   the host ID, so B is the fixed identity `"remote host"` instead. The host ID then
//!   comes in M2 and is bound by `T` (which holds it) and `confirm_H`, which is keyed
//!   from the code's SPAKE2 output: only a peer that knows the code can produce a
//!   `confirm_H` over a given ID, so the client pins an ID the code vouched for, never
//!   one it merely read from an unauthenticated hello. A party without the code (a man
//!   in the middle, a host that swaps its key in M2) fails the confirm like a wrong code.
//! - `T` = SHA-256 over both SPAKE2 messages and the host's ID. The keys come
//!   from HMAC-SHA256 over SPAKE2's output with distinct labels.
//! - The client checks `confirm_H` before revealing anything; the host checks
//!   M3's AEAD. Either failure is a wrong code (or someone in the middle).
//! - The window: 5 minutes, 3 attempts (one left half-way counts), one use.

use crate::keys::{DeviceId, DeviceKey};
use crate::msg;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::time::{Duration, Instant};

pub const WINDOW: Duration = Duration::from_secs(300);
pub const ATTEMPTS: u32 = 3;
const CLIENT_IDENTITY: &[u8] = b"remote client";
/// SPAKE2's B identity when the client doesn't know the host's ID (M1 `v: 2`).
const UNKNOWN_HOST_IDENTITY: &[u8] = b"remote host";

/// A fresh 6-digit code.
pub fn new_code() -> String {
    format!("{:06}", rand::random_range(0..1_000_000u32))
}

/// Codes are exactly six ASCII digits.
pub fn valid_code(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit())
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
enum PairMsg {
    M1 { v: u16, spake: Vec<u8> },
    M2 { spake: Vec<u8>, host: [u8; 32], name: String, confirm: [u8; 32] },
    M3 { sealed: Vec<u8> },
    M4 { done: [u8; 32] },
    /// The other side gave up (wrong code seen by the client).
    Abort,
}

#[derive(Serialize, Deserialize)]
struct ClientInfo {
    id: [u8; 32],
    name: String,
}

struct Keys {
    transcript: [u8; 32],
    confirm: [u8; 32],
    seal: [u8; 32],
    done_key: [u8; 32],
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any key length");
    for p in parts {
        m.update(p);
    }
    m.finalize().into_bytes().into()
}

fn derive(k: &[u8], spake_a: &[u8], spake_b: &[u8], host: &[u8; 32]) -> Keys {
    let mut h = Sha256::new();
    for p in [spake_a, spake_b, &host[..]] {
        h.update((p.len() as u32).to_be_bytes());
        h.update(p);
    }
    let transcript: [u8; 32] = h.finalize().into();
    Keys {
        confirm: hmac(&hmac(k, &[b"remote pair v1 host confirm"]), &[&transcript]),
        seal: hmac(k, &[b"remote pair v1 client seal"]),
        done_key: hmac(k, &[b"remote pair v1 done"]),
        transcript,
    }
}

fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// What a step of pairing produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Send this to the other side.
    Send(Vec<u8>),
    /// Paired: pin this device. The host also sends `reply`.
    Paired { id: DeviceId, name: String, reply: Option<Vec<u8>> },
    /// This attempt failed (a wrong code, or tampering). Send `reply` if any.
    Failed { reply: Option<Vec<u8>> },
    /// Not for us in this state: ignore it.
    Ignore,
}

// ---------------------------------------------------------------------------
// Host

/// The host's pairing window: the code, its deadline and its attempts.
pub struct HostWindow {
    key: DeviceKey,
    name: String,
    code: String,
    expires: Instant,
    attempts_started: u32,
    in_attempt: bool,
    used: bool,
    /// The keys of the current attempt, once M1 arrived.
    attempt: Option<Keys>,
}

impl HostWindow {
    pub fn new(key: DeviceKey, name: String, code: String, now: Instant) -> Self {
        HostWindow { key, name, code, expires: now + WINDOW, attempts_started: 0, in_attempt: false, used: false, attempt: None }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn expires(&self) -> Instant {
        self.expires
    }

    /// Open: not used, not expired, and attempts left or one in progress.
    pub fn is_open(&self, now: Instant) -> bool {
        !self.used && now < self.expires && (self.attempts_started < ATTEMPTS || self.in_attempt)
    }

    /// A client paired through this window (it is one use).
    pub fn is_used(&self) -> bool {
        self.used
    }

    pub fn attempts_left(&self) -> u32 {
        ATTEMPTS.saturating_sub(self.attempts_started)
    }

    /// A client started an attempt (the rendezvous' `pair_joined`, or a LAN
    /// pairing connection). Returns false when the window can't take one.
    pub fn begin_attempt(&mut self, now: Instant) -> bool {
        if self.used || now >= self.expires || self.attempts_started >= ATTEMPTS {
            return false;
        }
        self.attempts_started += 1;
        self.in_attempt = true;
        self.attempt = None;
        true
    }

    /// The client left half-way (the attempt still counts).
    pub fn end_attempt(&mut self) {
        self.in_attempt = false;
        self.attempt = None;
    }

    /// A message from the client of the current attempt.
    pub fn handle(&mut self, now: Instant, data: &[u8]) -> Step {
        if self.used || now >= self.expires || !self.in_attempt {
            return Step::Ignore;
        }
        match msg::decode::<PairMsg>(data) {
            Some(PairMsg::M1 { v: v @ (1 | 2), spake }) if self.attempt.is_none() => {
                let host = self.key.id();
                let (state, spake_b) = Spake2::<Ed25519Group>::start_b(
                    &Password::new(self.code.as_bytes()),
                    &Identity::new(CLIENT_IDENTITY),
                    &Identity::new(if v == 2 { UNKNOWN_HOST_IDENTITY } else { &host.0 }),
                );
                let Ok(k) = state.finish(&spake) else { return self.fail(None) };
                let keys = derive(&k, &spake, &spake_b, &host.0);
                let m2 = PairMsg::M2 { spake: spake_b.clone(), host: host.0, name: self.name.clone(), confirm: keys.confirm };
                self.attempt = Some(keys);
                Step::Send(msg::encode(&m2))
            }
            Some(PairMsg::M3 { sealed }) => {
                let Some(keys) = self.attempt.take() else { return Step::Ignore };
                let cipher = ChaCha20Poly1305::new((&keys.seal).into());
                let plain = cipher.decrypt(Nonce::from_slice(&[0u8; 12]), Payload { msg: &sealed, aad: &keys.transcript });
                let Some(info) = plain.ok().and_then(|p| msg::decode::<ClientInfo>(&p)) else {
                    return self.fail(Some(msg::encode(&PairMsg::Abort)));
                };
                let Ok(id) = DeviceId(info.id).to_string().parse::<DeviceId>() else {
                    return self.fail(Some(msg::encode(&PairMsg::Abort)));
                };
                self.used = true;
                self.in_attempt = false;
                let done = hmac(&keys.done_key, &[&keys.transcript, &info.id]);
                Step::Paired { id, name: info.name, reply: Some(msg::encode(&PairMsg::M4 { done })) }
            }
            Some(PairMsg::Abort) => self.fail(None),
            _ => Step::Ignore,
        }
    }

    fn fail(&mut self, reply: Option<Vec<u8>>) -> Step {
        self.end_attempt();
        Step::Failed { reply }
    }
}

// ---------------------------------------------------------------------------
// Client

/// One pairing attempt from the client's side.
pub struct ClientAttempt {
    key: DeviceKey,
    name: String,
    /// The host's ID when the client knew it (discovery); `None` when pairing by address.
    host: Option<DeviceId>,
    /// The ID pinned at the end: the one M2 carried and the code vouched for.
    learned: Option<DeviceId>,
    state: Option<(Spake2<Ed25519Group>, Vec<u8>)>,
    keys: Option<Keys>,
    host_name: String,
}

impl ClientAttempt {
    /// Start against `host` (its ID as discovery reported it). Returns M1.
    pub fn start(key: DeviceKey, name: String, host: DeviceId, code: &str) -> (Self, Vec<u8>) {
        let (state, spake_a) = Spake2::<Ed25519Group>::start_a(
            &Password::new(code.as_bytes()),
            &Identity::new(CLIENT_IDENTITY),
            &Identity::new(&host.0),
        );
        let m1 = msg::encode(&PairMsg::M1 { v: 1, spake: spake_a.clone() });
        (ClientAttempt { key, name, host: Some(host), learned: None, state: Some((state, spake_a)), keys: None, host_name: String::new() }, m1)
    }

    /// Start against a host known only by its address: its ID comes back in the exchange,
    /// vouched for by the code (see the module docs). Returns M1.
    pub fn start_by_address(key: DeviceKey, name: String, code: &str) -> (Self, Vec<u8>) {
        let (state, spake_a) = Spake2::<Ed25519Group>::start_a(
            &Password::new(code.as_bytes()),
            &Identity::new(CLIENT_IDENTITY),
            &Identity::new(UNKNOWN_HOST_IDENTITY),
        );
        let m1 = msg::encode(&PairMsg::M1 { v: 2, spake: spake_a.clone() });
        (ClientAttempt { key, name, host: None, learned: None, state: Some((state, spake_a)), keys: None, host_name: String::new() }, m1)
    }

    pub fn handle(&mut self, data: &[u8]) -> Step {
        match msg::decode::<PairMsg>(data) {
            Some(PairMsg::M2 { spake, host, name, confirm }) => {
                let Some((state, spake_a)) = self.state.take() else { return Step::Ignore };
                let abort = Some(msg::encode(&PairMsg::Abort));
                if self.host.is_some_and(|h| h.0 != host) {
                    return Step::Failed { reply: abort };
                }
                let Ok(learned) = DeviceId(host).to_string().parse::<DeviceId>() else { return Step::Failed { reply: abort } };
                let Ok(k) = state.finish(&spake) else { return Step::Failed { reply: abort } };
                let keys = derive(&k, &spake_a, &spake, &host);
                if !ct_eq(&keys.confirm, &confirm) {
                    return Step::Failed { reply: abort };
                }
                let info = msg::encode(&ClientInfo { id: self.key.id().0, name: self.name.clone() });
                let cipher = ChaCha20Poly1305::new((&keys.seal).into());
                let sealed = cipher
                    .encrypt(Nonce::from_slice(&[0u8; 12]), Payload { msg: &info, aad: &keys.transcript })
                    .expect("encrypt");
                self.keys = Some(keys);
                self.learned = Some(learned);
                self.host_name = name;
                Step::Send(msg::encode(&PairMsg::M3 { sealed }))
            }
            Some(PairMsg::M4 { done }) => {
                let Some(keys) = self.keys.take() else { return Step::Ignore };
                let want = hmac(&keys.done_key, &[&keys.transcript, &self.key.id().0]);
                if !ct_eq(&want, &done) {
                    return Step::Failed { reply: None };
                }
                let Some(id) = self.learned else { return Step::Failed { reply: None } };
                Step::Paired { id, name: std::mem::take(&mut self.host_name), reply: None }
            }
            Some(PairMsg::Abort) => Step::Failed { reply: None },
            _ => Step::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(code_host: &str, code_client: &str) -> (Step, Step) {
        let now = Instant::now();
        let (hk, ck) = (DeviceKey::generate(), DeviceKey::generate());
        let mut w = HostWindow::new(hk.clone(), "nao-linux".into(), code_host.into(), now);
        assert!(w.begin_attempt(now));
        let (mut c, m1) = ClientAttempt::start(ck.clone(), "MacBook Air".into(), hk.id(), code_client);
        let Step::Send(m2) = w.handle(now, &m1) else { panic!("no M2") };
        match c.handle(&m2) {
            Step::Send(m3) => {
                let host_step = w.handle(now, &m3);
                let Step::Paired { reply: Some(m4), .. } = &host_step else { return (host_step, Step::Ignore) };
                let client_step = c.handle(m4);
                (host_step, client_step)
            }
            client_step => {
                let Step::Failed { reply: Some(abort) } = &client_step else { panic!("{client_step:?}") };
                (w.handle(now, abort), client_step)
            }
        }
    }

    #[test]
    fn the_right_code_pins_both_keys() {
        let (h, c) = run("123456", "123456");
        assert!(matches!(h, Step::Paired { ref name, .. } if name == "MacBook Air"), "{h:?}");
        assert!(matches!(c, Step::Paired { ref name, .. } if name == "nao-linux"), "{c:?}");
    }

    #[test]
    fn a_wrong_code_fails_on_both_sides() {
        let (h, c) = run("123456", "123457");
        assert!(matches!(h, Step::Failed { .. }), "{h:?}");
        assert!(matches!(c, Step::Failed { .. }), "{c:?}");
    }

    #[test]
    fn a_window_takes_three_attempts_then_closes() {
        let now = Instant::now();
        let mut w = HostWindow::new(DeviceKey::generate(), "h".into(), new_code(), now);
        for _ in 0..ATTEMPTS {
            assert!(w.begin_attempt(now));
        }
        assert!(!w.begin_attempt(now));
        let mut w = HostWindow::new(DeviceKey::generate(), "h".into(), new_code(), now);
        assert!(!w.begin_attempt(now + WINDOW), "expired");
    }

    /// One by-address exchange; `tamper` edits M2 on its way to the client.
    fn by_address(hk: &DeviceKey, code_host: &str, code_client: &str, tamper: impl Fn(&mut PairMsg)) -> (Step, Option<DeviceId>) {
        let now = Instant::now();
        let mut w = HostWindow::new(hk.clone(), "nao-linux".into(), code_host.into(), now);
        assert!(w.begin_attempt(now));
        let (mut c, m1) = ClientAttempt::start_by_address(DeviceKey::generate(), "MacBook Air".into(), code_client);
        let Step::Send(m2) = w.handle(now, &m1) else { panic!("no M2") };
        let mut m2: PairMsg = msg::decode(&m2).unwrap();
        tamper(&mut m2);
        match c.handle(&msg::encode(&m2)) {
            Step::Send(m3) => {
                let Step::Paired { reply: Some(m4), .. } = w.handle(now, &m3) else { panic!("host") };
                let s = c.handle(&m4);
                let id = if let Step::Paired { id, .. } = &s { Some(*id) } else { None };
                (s, id)
            }
            s => (s, None),
        }
    }

    #[test]
    fn by_address_learns_the_host_id_the_code_vouched_for() {
        let hk = DeviceKey::generate();
        let (s, id) = by_address(&hk, "123456", "123456", |_| {});
        assert!(matches!(s, Step::Paired { ref name, .. } if name == "nao-linux"), "{s:?}");
        assert_eq!(id, Some(hk.id()));
    }

    #[test]
    fn by_address_with_a_wrong_code_fails() {
        let (s, id) = by_address(&DeviceKey::generate(), "123456", "654321", |_| {});
        assert!(matches!(s, Step::Failed { .. }), "{s:?}");
        assert!(id.is_none());
    }

    #[test]
    fn by_address_rejects_a_host_that_swaps_its_key_in_m2() {
        let other = DeviceKey::generate().id();
        let (s, id) = by_address(&DeviceKey::generate(), "123456", "123456", |m| {
            if let PairMsg::M2 { host, .. } = m {
                *host = other.0;
            }
        });
        assert!(matches!(s, Step::Failed { .. }), "{s:?}");
        assert!(id.is_none());
    }

    #[test]
    fn a_lying_relay_about_the_host_id_looks_like_a_wrong_code() {
        let now = Instant::now();
        let hk = DeviceKey::generate();
        let mut w = HostWindow::new(hk, "h".into(), "111111".into(), now);
        w.begin_attempt(now);
        let (mut c, m1) = ClientAttempt::start(DeviceKey::generate(), "c".into(), DeviceKey::generate().id(), "111111");
        let Step::Send(m2) = w.handle(now, &m1) else { panic!() };
        assert!(matches!(c.handle(&m2), Step::Failed { .. }));
    }
}
