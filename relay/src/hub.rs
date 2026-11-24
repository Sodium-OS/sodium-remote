// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The rendezvous state machine: who is online, signalling sessions, pairing sessions,
//! and which clients each host has authorised. Transport-free, so tests and the
//! WebSocket layer drive it the same way.
//!
//! Logs carry metadata only (shortened device IDs, session IDs, counts and sizes),
//! never blobs, display names, signatures or credentials.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::net::IpAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rand::RngCore;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};
use tracing::{error, info, warn};

use crate::hooks::Denial;
use crate::tokens::{self, TokenError};
use crate::{Config, Enrol};
use crate::limits::{Key, Kind, Limits};
use crate::proto::{
    ApiError, DeviceId, ErrorCode, Event, HostState, MAX_DISCOVERED, MAX_PAIR_ATTEMPTS,
    MAX_PAIR_BLOB, MAX_PAIR_MSGS, MAX_PAIR_NAME,
    MAX_PAIRED_CLIENTS, MAX_PAIRED_LIST_BYTES, MAX_PAIRINGS_PER_HOST, MAX_PRESENCE_QUERY,
    MAX_SESSIONS_PER_CLIENT, MAX_SIGNAL_BLOB, PairedListPayload, Request, Role, b64,
    paired_list_message, unb64,
};
use crate::store::{DeviceRec, PairedRec, Store};
use crate::turn;

pub const MAX_DEVICES: usize = 10_000;
/// Clients register without a token, so they get a share of their own: the rest of the table stays for hosts.
pub const MAX_CLIENTS: usize = 8_000;
const OUTBOX_DEPTH: usize = 256;

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn random_id() -> String {
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b64(&b)
}

fn err(code: ErrorCode, message: &'static str) -> ApiError {
    ApiError::new(code, message)
}

/// Frames queued for one connection. A connection that lets its queue fill up is
/// too slow to keep and gets dropped.
#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::Sender<String>,
    kill: Arc<Notify>,
}

pub struct OutboxRx {
    pub rx: mpsc::Receiver<String>,
    pub kill: Arc<Notify>,
}

pub fn outbox() -> (Outbox, OutboxRx) {
    let (tx, rx) = mpsc::channel(OUTBOX_DEPTH);
    let kill = Arc::new(Notify::new());
    (Outbox { tx, kill: kill.clone() }, OutboxRx { rx, kill })
}

impl Outbox {
    fn send(&self, ev: &Event) {
        let frame = serde_json::to_string(ev).expect("events serialize");
        if self.tx.try_send(frame).is_err() {
            self.kill.notify_one();
        }
    }

    fn kill(&self) {
        self.kill.notify_one();
    }
}

/// Who is on the other end of a connection, once authenticated.
#[derive(Clone, Debug)]
pub struct ConnCtx {
    pub device: DeviceId,
    pub role: Role,
    pub conn: u64,
    pub ip: IpAddr,
}

struct Conn {
    id: u64,
    role: Role,
    outbox: Outbox,
}

struct Session {
    host: DeviceId,
    client: DeviceId,
    signals: u64,
    bytes: u64,
}

/// A host's pairing window. The code never reaches the server: clients find the
/// window by discovery and run SPAKE2 with the host through opaque messages.
struct Pairing {
    host: DeviceId,
    name: String,
    /// The client making the current attempt, if any.
    client: Option<DeviceId>,
    attempts: u32,
    expires: Instant,
    msgs: u32,
}

struct Inner {
    store: Store,
    /// client -> hosts whose current paired list contains it
    authorised_by: HashMap<DeviceId, BTreeSet<DeviceId>>,
    conns: HashMap<DeviceId, Conn>,
    sessions: HashMap<String, Session>,
    pairings: HashMap<String, Pairing>,
    limits: Limits,
    /// Seconds a sleep notice keeps a host "asleep" before it counts as offline.
    asleep_ttl: u64,
}

pub struct Hub {
    cfg: Config,
    inner: Mutex<Inner>,
    next_conn: AtomicU64,
}

pub fn read_allowlist(path: &Path) -> anyhow::Result<HashSet<DeviceId>> {
    let text = std::fs::read_to_string(path)?;
    let mut set = HashSet::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match DeviceId::parse(line) {
            Some(id) => {
                set.insert(id);
            }
            None => warn!(path = %path.display(), "ignoring a line that is not a device ID"),
        }
    }
    Ok(set)
}

fn send_to(conns: &HashMap<DeviceId, Conn>, to: &DeviceId, ev: &Event) {
    if let Some(c) = conns.get(to) {
        c.outbox.send(ev);
    }
}

fn check_blob(blob: &str, max: usize) -> Result<usize, ApiError> {
    if blob.len() > max.div_ceil(3) * 4 {
        return Err(err(ErrorCode::TooLarge, "blob is too large"));
    }
    let bytes = unb64(blob).ok_or(err(ErrorCode::BadRequest, "blob is not base64url"))?;
    if bytes.len() > max {
        return Err(err(ErrorCode::TooLarge, "blob is too large"));
    }
    Ok(bytes.len())
}

fn parse_device(s: &str) -> Result<DeviceId, ApiError> {
    DeviceId::parse(s).ok_or(err(ErrorCode::BadRequest, "not a device ID"))
}

fn denied(d: Denial) -> ApiError {
    match d {
        Denial::QuotaExceeded => err(ErrorCode::QuotaExceeded, "this account is over its quota"),
        Denial::PaymentRequired => err(ErrorCode::PaymentRequired, "this account needs a payment"),
    }
}

fn token_refused(e: TokenError) -> ApiError {
    match e {
        TokenError::Invalid => err(ErrorCode::TokenInvalid, "this server doesn't know that enrolment token"),
        TokenError::Expired => err(ErrorCode::TokenExpired, "that enrolment token has expired or been used up"),
    }
}

/// The result of a successful authentication.
pub struct Attached {
    pub ctx: ConnCtx,
    /// This authentication registered the device.
    pub registered: bool,
    pub account: Option<String>,
}

fn need(ctx: &ConnCtx, role: Role) -> Result<(), ApiError> {
    if ctx.role == role {
        Ok(())
    } else {
        Err(err(ErrorCode::Forbidden, "not allowed for this role"))
    }
}

impl Inner {
    fn rebuild_index(&mut self) {
        self.authorised_by.clear();
        for (host, rec) in &self.store.data.paired {
            for c in &rec.clients {
                self.authorised_by.entry(*c).or_default().insert(*host);
            }
        }
    }

    fn is_authorised(&self, client: &DeviceId, host: &DeviceId) -> bool {
        self.authorised_by.get(client).is_some_and(|h| h.contains(host))
    }

    /// What `client` may know of `host`, as an event (also the shape of a presence entry).
    fn host_state(&self, client: &DeviceId, host: &DeviceId) -> Event {
        let rec = self.store.data.devices.get(host);
        let (state, last_seen, since) = if !self.is_authorised(client, host) {
            (HostState::NotAuthorised, None, None)
        } else if self.conns.contains_key(host) {
            (HostState::Online, None, None)
        } else {
            let last_seen = rec.map(|r| r.last_seen);
            match rec.and_then(|r| r.asleep_since) {
                Some(since) if unix_now().saturating_sub(since) < self.asleep_ttl => {
                    (HostState::Asleep, last_seen, Some(since))
                }
                _ => (HostState::Offline, last_seen, None),
            }
        };
        Event::Presence { host: *host, state, last_seen, since }
    }

    /// Tells every online client a host authorises that the host came, went or slept.
    fn broadcast_presence(&self, host: &DeviceId) {
        let Some(rec) = self.store.data.paired.get(host) else {
            return;
        };
        for c in &rec.clients {
            send_to(&self.conns, c, &self.host_state(c, host));
        }
    }

    fn close_sessions(&mut self, pred: impl Fn(&str, &Session) -> bool, reason: &'static str) {
        let ids: Vec<String> =
            self.sessions.iter().filter(|(k, s)| pred(k, s)).map(|(k, _)| k.clone()).collect();
        for id in ids {
            let s = self.sessions.remove(&id).expect("listed above");
            for d in [s.host, s.client] {
                send_to(&self.conns, &d, &Event::SessionClosed { session: id.clone(), reason });
            }
            info!(session = &id[..8], host = %s.host.short(), client = %s.client.short(),
                  signals = s.signals, bytes = s.bytes, reason, "session closed");
        }
    }

    fn close_pairings(&mut self, pred: impl Fn(&str, &Pairing) -> bool, reason: &'static str) {
        let ids: Vec<String> =
            self.pairings.iter().filter(|(k, p)| pred(k, p)).map(|(k, _)| k.clone()).collect();
        for id in ids {
            let p = self.pairings.remove(&id).expect("listed above");
            for d in std::iter::once(p.host).chain(p.client) {
                send_to(&self.conns, &d, &Event::PairClosed { pairing: id.clone(), reason });
            }
            info!(pairing = &id[..8], host = %p.host.short(), attempts = p.attempts,
                  msgs = p.msgs, reason, "pairing window closed");
        }
    }

    /// Ends the current attempt in a pairing window; the window stays open.
    fn end_attempt(&mut self, id: &str, client_reason: Option<&'static str>) {
        let Some(p) = self.pairings.get_mut(id) else { return };
        let Some(client) = p.client.take() else { return };
        let host = p.host;
        match client_reason {
            Some(reason) => {
                send_to(&self.conns, &client, &Event::PairClosed { pairing: id.to_string(), reason })
            }
            None => send_to(&self.conns, &host, &Event::PairLeft { pairing: id.to_string() }),
        }
    }

    fn drop_device(&mut self, d: &DeviceId, reason: &'static str) {
        let d = *d;
        self.close_sessions(|_, s| s.host == d || s.client == d, reason);
        self.close_pairings(|_, p| p.host == d, reason);
        // A client that goes away only ends its attempt; the host's window stays open.
        let attempts: Vec<String> =
            self.pairings.iter().filter(|(_, p)| p.client == Some(d)).map(|(k, _)| k.clone()).collect();
        for id in attempts {
            self.end_attempt(&id, None);
        }
    }

    fn paired_list(&mut self, cfg: &Config, host: DeviceId, list: String, sig: String) -> Result<Value, ApiError> {
        let payload = unb64(&list).ok_or(err(ErrorCode::BadRequest, "list is not base64url"))?;
        if payload.len() > MAX_PAIRED_LIST_BYTES {
            return Err(err(ErrorCode::TooLarge, "list is too large"));
        }
        if !host.verify(&paired_list_message(&payload), &sig) {
            return Err(err(ErrorCode::BadSignature, "list signature does not verify"));
        }
        let p: PairedListPayload = serde_json::from_slice(&payload)
            .map_err(|_| err(ErrorCode::BadRequest, "list payload is not a paired list"))?;
        if p.v != 2 || p.host != host {
            return Err(err(ErrorCode::BadRequest, "list is not this host's v2 list"));
        }
        // Signed for this server: a list another server holds can't be brought here.
        if !cfg.server_names().any(|n| n == p.server) {
            return Err(err(ErrorCode::BadRequest, "list is for another server"));
        }
        if p.clients.len() > MAX_PAIRED_CLIENTS {
            return Err(err(ErrorCode::TooMany, "too many clients in the list"));
        }
        let old_set: BTreeSet<DeviceId> = match self.store.data.paired.get(&host) {
            Some(old) if p.seq <= old.seq => {
                return Err(err(ErrorCode::StaleSeq, "seq must be higher than the stored list's"));
            }
            Some(old) => old.clients.iter().copied().collect(),
            None => BTreeSet::new(),
        };
        let new_set: BTreeSet<DeviceId> = p.clients.iter().copied().collect();
        let rec = PairedRec { seq: p.seq, list, sig, clients: new_set.iter().copied().collect() };
        self.store.data.paired.insert(host, rec);
        self.store.save();
        self.rebuild_index();

        for c in old_set.difference(&new_set) {
            let c = *c;
            self.close_sessions(|_, s| s.host == host && s.client == c, "revoked");
            send_to(&self.conns, &c, &Event::Revoked { host });
            info!(host = %host.short(), client = %c.short(), "client revoked");
        }
        for c in new_set.difference(&old_set) {
            send_to(&self.conns, c, &self.host_state(c, &host));
        }
        info!(host = %host.short(), seq = p.seq, clients = new_set.len(), "paired list stored");
        Ok(json!({ "seq": p.seq, "clients": new_set.len() }))
    }

    fn connect(&mut self, client: DeviceId, host: &str) -> Result<Value, ApiError> {
        let host = parse_device(host)?;
        if !self.is_authorised(&client, &host) {
            return Err(err(ErrorCode::NotAuthorised, "this host has not paired this client"));
        }
        let Some(hc) = self.conns.get(&host) else {
            return Err(err(ErrorCode::HostOffline, "the host is offline"));
        };
        if self.sessions.values().filter(|s| s.client == client).count() >= MAX_SESSIONS_PER_CLIENT {
            return Err(err(ErrorCode::TooMany, "too many open sessions"));
        }
        let id = random_id();
        hc.outbox.send(&Event::Incoming { session: id.clone(), client });
        self.sessions.insert(id.clone(), Session { host, client, signals: 0, bytes: 0 });
        info!(session = &id[..8], host = %host.short(), client = %client.short(), "session open");
        Ok(json!({ "session": id }))
    }

    fn signal(&mut self, me: DeviceId, session: String, blob: String) -> Result<Value, ApiError> {
        let len = check_blob(&blob, MAX_SIGNAL_BLOB)?;
        let s = self
            .sessions
            .get_mut(&session)
            .filter(|s| s.host == me || s.client == me)
            .ok_or(err(ErrorCode::NotFound, "no such session"))?;
        let peer = if s.host == me { s.client } else { s.host };
        s.signals += 1;
        s.bytes += len as u64;
        send_to(&self.conns, &peer, &Event::Signal { session, blob });
        Ok(json!({}))
    }

    fn close(&mut self, me: DeviceId, session: &str) -> Result<Value, ApiError> {
        if !self.sessions.get(session).is_some_and(|s| s.host == me || s.client == me) {
            return Err(err(ErrorCode::NotFound, "no such session"));
        }
        // Both sides get session_closed, the closer included, so each sees one ending.
        self.close_sessions(|k, _| k == session, "closed");
        Ok(json!({}))
    }

    fn pair_open(&mut self, cfg: &Config, host: DeviceId, name: String) -> Result<Value, ApiError> {
        if !self.limits.take(Kind::PairOpen, Key::Device(host)) {
            return Err(err(ErrorCode::RateLimited, "too many pairing windows; wait"));
        }
        if name.is_empty() || name.len() > MAX_PAIR_NAME {
            return Err(err(ErrorCode::BadRequest, "name must be 1 to 64 bytes"));
        }
        if self.pairings.values().filter(|p| p.host == host).count() >= MAX_PAIRINGS_PER_HOST {
            return Err(err(ErrorCode::TooMany, "a pairing window is already open"));
        }
        let id = random_id();
        let expires = Instant::now() + cfg.pairing_ttl;
        self.pairings.insert(id.clone(), Pairing { host, name, client: None, attempts: 0, expires, msgs: 0 });
        info!(pairing = &id[..8], host = %host.short(), "pairing window open");
        Ok(json!({ "pairing": id, "expires_in": cfg.pairing_ttl.as_secs() }))
    }

    /// Open windows nobody is attempting right now, newest first.
    fn pair_discover(&mut self, client: DeviceId, ip: IpAddr) -> Result<Value, ApiError> {
        if !self.limits.take(Kind::PairDiscover, Key::ip(ip)) {
            return Err(err(ErrorCode::RateLimited, "too many pairing lookups; wait"));
        }
        let now = Instant::now();
        let mut open: Vec<(&String, &Pairing)> = self
            .pairings
            .iter()
            .filter(|(_, p)| p.client.is_none() && p.attempts < MAX_PAIR_ATTEMPTS && p.expires > now)
            .collect();
        open.sort_by_key(|(_, p)| std::cmp::Reverse(p.expires));
        let hosts: Vec<Value> = open
            .into_iter()
            .take(MAX_DISCOVERED)
            .map(|(id, p)| {
                json!({
                    "pairing": id,
                    "host": p.host,
                    "name": p.name,
                    "expires_in": p.expires.saturating_duration_since(now).as_secs(),
                })
            })
            .collect();
        info!(client = %client.short(), found = hosts.len(), "pairing discovery");
        Ok(json!({ "hosts": hosts }))
    }

    fn pair_join(&mut self, client: DeviceId, ip: IpAddr, pairing: &str) -> Result<Value, ApiError> {
        if !self.limits.take(Kind::PairJoin, Key::Device(client))
            || !self.limits.take(Kind::PairJoinIp, Key::ip(ip))
        {
            return Err(err(ErrorCode::RateLimited, "too many pairing attempts; wait"));
        }
        let p = self
            .pairings
            .get_mut(pairing)
            .filter(|p| p.client.is_none() && p.expires > Instant::now())
            .ok_or(err(ErrorCode::NotFound, "no open pairing window by that ID, or it's busy"))?;
        if p.attempts >= MAX_PAIR_ATTEMPTS {
            return Err(err(ErrorCode::TooMany, "this window has used its attempts"));
        }
        p.attempts += 1;
        p.client = Some(client);
        let (host, attempt) = (p.host, p.attempts);
        let expires_in = p.expires.saturating_duration_since(Instant::now()).as_secs();
        send_to(&self.conns, &host, &Event::PairJoined { pairing: pairing.to_string(), attempt });
        info!(pairing = &pairing[..8], host = %host.short(), client = %client.short(), attempt, "pairing attempt");
        Ok(json!({ "pairing": pairing, "host": host, "attempt": attempt, "expires_in": expires_in }))
    }

    fn pair_msg(&mut self, me: DeviceId, pairing: String, blob: String) -> Result<Value, ApiError> {
        let not_found = err(ErrorCode::NotFound, "no such pairing window");
        check_blob(&blob, MAX_PAIR_BLOB)?;
        let p = self.pairings.get_mut(&pairing).ok_or(not_found.clone())?;
        let peer = if p.host == me {
            p.client.ok_or(err(ErrorCode::NotFound, "nobody is attempting right now"))?
        } else if p.client == Some(me) {
            p.host
        } else {
            return Err(not_found);
        };
        if p.msgs >= MAX_PAIR_MSGS {
            return Err(err(ErrorCode::TooMany, "too many pairing messages"));
        }
        p.msgs += 1;
        send_to(&self.conns, &peer, &Event::PairMsg { pairing, blob });
        Ok(json!({}))
    }

    /// The host reports a failed SPAKE2 attempt: the client is let go, and the window
    /// closes once it has used its attempts.
    fn pair_fail(&mut self, host: DeviceId, pairing: &str) -> Result<Value, ApiError> {
        let p = self
            .pairings
            .get(pairing)
            .filter(|p| p.host == host && p.client.is_some())
            .ok_or(err(ErrorCode::NotFound, "no attempt in progress in that window"))?;
        let left = MAX_PAIR_ATTEMPTS - p.attempts;
        self.end_attempt(pairing, Some("failed"));
        info!(pairing = &pairing[..8], host = %host.short(), attempts_left = left, "pairing attempt failed");
        if left == 0 {
            self.close_pairings(|k, _| k == pairing, "attempts_used");
        }
        Ok(json!({ "attempts_left": left }))
    }

    /// The host closes its window (paired, or cancelled); a client leaves its attempt.
    fn pair_close(&mut self, me: DeviceId, pairing: &str) -> Result<Value, ApiError> {
        match self.pairings.get(pairing) {
            Some(p) if p.host == me => self.close_pairings(|k, _| k == pairing, "closed"),
            Some(p) if p.client == Some(me) => {
                self.end_attempt(pairing, None);
                send_to(&self.conns, &me, &Event::PairClosed { pairing: pairing.to_string(), reason: "closed" });
            }
            _ => return Err(err(ErrorCode::NotFound, "no such pairing window")),
        }
        Ok(json!({}))
    }
}

impl Hub {
    pub fn new(cfg: Config) -> anyhow::Result<Arc<Self>> {
        let store = Store::open(&cfg.state_dir)?;
        let mut inner = Inner {
            store,
            authorised_by: HashMap::new(),
            conns: HashMap::new(),
            sessions: HashMap::new(),
            pairings: HashMap::new(),
            limits: Limits::default(),
            asleep_ttl: cfg.asleep_ttl.as_secs(),
        };
        inner.rebuild_index();
        info!(devices = inner.store.data.devices.len(), hosts_with_lists = inner.store.data.paired.len(),
              "state loaded");
        Ok(Arc::new(Self { cfg, inner: Mutex::new(inner), next_conn: AtomicU64::new(1) }))
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Rate-limits new connections per address.
    pub fn admit(&self, ip: IpAddr) -> bool {
        self.lock().limits.take(Kind::Conn, Key::ip(ip))
    }

    /// Whether this host is on the allowlist (false without one, or when it can't be read).
    fn allowlisted(&self, device: &DeviceId) -> bool {
        let Some(path) = &self.cfg.hosts_allow else { return false };
        match read_allowlist(path) {
            Ok(set) => set.contains(device),
            Err(e) => {
                error!(error = %e, "reading the host allowlist failed");
                false
            }
        }
    }

    /// Where a device should register instead (`--redirect`): only for a device this
    /// server doesn't know, so registered ones stay.
    pub fn redirect_for(&self, device: &DeviceId) -> Option<String> {
        let url = self.cfg.redirect.as_ref()?;
        (!self.lock().store.data.devices.contains_key(device)).then(|| url.clone())
    }

    /// Called once a device has answered the challenge with a valid signature.
    /// Registers it on first sight (enrolment: `--enrol`, the token, the hooks);
    /// replaces any older connection of the same device.
    pub fn attach(&self, device: DeviceId, role: Role, ip: IpAddr, outbox: Outbox, token: Option<&str>)
        -> Result<Attached, ApiError> {
        let not_enrolled = |why| {
            info!(device = %device.short(), why, "host not enrolled");
            err(ErrorCode::HostNotAllowed, "this host is not enrolled on this server")
        };
        if role == Role::Host && self.cfg.enrol == Enrol::Allowlist && !self.allowlisted(&device) {
            return Err(not_enrolled("not on the allowlist"));
        }
        let token = token.map(str::trim).filter(|t| !t.is_empty());
        let now = unix_now();
        let mut g = self.lock();
        let inner = &mut *g;
        let (registered, account) = match inner.store.data.devices.get_mut(&device) {
            Some(rec) if rec.role != role => {
                return Err(err(ErrorCode::RoleMismatch, "this device registered with another role"));
            }
            Some(rec) => {
                // Registered already: the token (kept in server.toml) isn't looked at again.
                rec.last_seen = now;
                rec.asleep_since = None; // back from sleep, if it was asleep
                (false, rec.account.clone())
            }
            None => {
                if inner.store.data.devices.len() >= MAX_DEVICES {
                    return Err(err(ErrorCode::ServerFull, "no room for new devices"));
                }
                if role == Role::Client && inner.store.data.devices.values().filter(|r| r.role == Role::Client).count() >= MAX_CLIENTS {
                    return Err(err(ErrorCode::ServerFull, "no room for new devices"));
                }
                // Before the token, so guessing tokens is rate-limited too.
                if !inner.limits.take(Kind::Register, Key::ip(ip)) {
                    return Err(err(ErrorCode::RateLimited, "too many new devices from this address"));
                }
                let needs_token =
                    role == Role::Host && self.cfg.enrol == Enrol::Token && !self.allowlisted(&device);
                let account = match token {
                    Some(t) => tokens::redeem(&self.cfg.tokens_dir(), t, now).map_err(|e| {
                        info!(device = %device.short(), role = role.as_str(), error = ?e, "enrolment token refused");
                        token_refused(e)
                    })?,
                    None if needs_token => {
                        let mut e = not_enrolled("no enrolment token");
                        e.message = "this server needs an enrolment token";
                        return Err(e);
                    }
                    None => None,
                };
                self.cfg.hooks.register(&device, role, account.as_deref()).map_err(denied)?;
                let rec = DeviceRec { role, first_seen: now, last_seen: now, asleep_since: None, account: account.clone() };
                inner.store.data.devices.insert(device, rec);
                (true, account)
            }
        };
        inner.store.save();
        if let Some(old) = inner.conns.remove(&device) {
            old.outbox.send(&Event::Replaced {});
            old.outbox.kill();
            inner.drop_device(&device, "peer_offline");
        }
        let id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        inner.conns.insert(device, Conn { id, role, outbox });
        if role == Role::Host {
            inner.broadcast_presence(&device);
        }
        info!(device = %device.short(), role = role.as_str(), registered, "authenticated");
        Ok(Attached { ctx: ConnCtx { device, role, conn: id, ip }, registered, account })
    }

    /// Called when a connection ends, whatever the reason.
    pub fn detach(&self, ctx: &ConnCtx) {
        let mut g = self.lock();
        let inner = &mut *g;
        if inner.conns.get(&ctx.device).map(|c| c.id) != Some(ctx.conn) {
            return; // replaced or unregistered already
        }
        inner.conns.remove(&ctx.device);
        inner.drop_device(&ctx.device, "peer_offline");
        if let Some(rec) = inner.store.data.devices.get_mut(&ctx.device) {
            rec.last_seen = unix_now();
            inner.store.save();
        }
        if ctx.role == Role::Host {
            inner.broadcast_presence(&ctx.device);
        }
        info!(device = %ctx.device.short(), role = ctx.role.as_str(), "disconnected");
    }

    pub fn handle(&self, ctx: &ConnCtx, req: Request) -> Result<Value, ApiError> {
        let mut g = self.lock();
        let inner = &mut *g;
        if !inner.limits.take(Kind::Msg, Key::Device(ctx.device)) {
            return Err(err(ErrorCode::RateLimited, "too many requests; slow down"));
        }
        let me = ctx.device;
        match req {
            Request::PairedList { list, sig } => {
                need(ctx, Role::Host)?;
                inner.paired_list(&self.cfg, me, list, sig)
            }
            Request::GetPairedList {} => {
                need(ctx, Role::Host)?;
                Ok(match inner.store.data.paired.get(&me) {
                    Some(r) => json!({ "seq": r.seq, "list": r.list, "sig": r.sig }),
                    None => json!({ "seq": null, "list": null, "sig": null }),
                })
            }
            Request::Presence { hosts } => {
                need(ctx, Role::Client)?;
                if hosts.len() > MAX_PRESENCE_QUERY {
                    return Err(err(ErrorCode::TooMany, "ask about fewer hosts"));
                }
                let mut out = Vec::with_capacity(hosts.len());
                for h in &hosts {
                    let host = parse_device(h)?;
                    // The entry is the presence event without its "t".
                    let mut entry = serde_json::to_value(inner.host_state(&me, &host)).expect("serializes");
                    entry.as_object_mut().expect("an object").remove("t");
                    out.push(entry);
                }
                Ok(json!({ "hosts": out }))
            }
            Request::Connect { host } => {
                need(ctx, Role::Client)?;
                let account = inner.store.data.devices.get(&me).and_then(|r| r.account.clone());
                if let Some(h) = DeviceId::parse(&host) {
                    self.cfg.hooks.connect(&me, &h, account.as_deref()).map_err(denied)?;
                }
                inner.connect(me, &host)
            }
            Request::Signal { session, blob } => inner.signal(me, session, blob),
            Request::Close { session } => inner.close(me, &session),
            Request::PairOpen { name } => {
                need(ctx, Role::Host)?;
                inner.pair_open(&self.cfg, me, name)
            }
            Request::PairDiscover {} => {
                need(ctx, Role::Client)?;
                inner.pair_discover(me, ctx.ip)
            }
            Request::PairJoin { pairing } => {
                need(ctx, Role::Client)?;
                inner.pair_join(me, ctx.ip, &pairing)
            }
            Request::PairMsg { pairing, blob } => inner.pair_msg(me, pairing, blob),
            Request::PairFail { pairing } => {
                need(ctx, Role::Host)?;
                inner.pair_fail(me, &pairing)
            }
            Request::PairClose { pairing } => inner.pair_close(me, &pairing),
            Request::TurnCredentials {} => {
                if !inner.limits.take(Kind::Turn, Key::Device(me)) {
                    return Err(err(ErrorCode::RateLimited, "too many credential requests; wait"));
                }
                let authorised = match ctx.role {
                    Role::Host => true,
                    Role::Client => inner.authorised_by.get(&me).is_some_and(|h| !h.is_empty()),
                };
                if !authorised {
                    return Err(err(ErrorCode::NotAuthorised, "no host has paired this client"));
                }
                let account = inner.store.data.devices.get(&me).and_then(|r| r.account.clone());
                self.cfg.hooks.turn_credentials(&me, account.as_deref()).map_err(denied)?;
                let now = unix_now();
                let (username, credential, expires) =
                    turn::credentials(&self.cfg.turn_secret, &me, now, self.cfg.turn_ttl.as_secs());
                let ttl = expires - now;
                info!(device = %me.short(), role = ctx.role.as_str(), ttl, "turn credentials issued");
                // Older clients read `urls` only: the plain list, else the first region's.
                let urls = match (self.cfg.turn_urls.is_empty(), self.cfg.relays.first()) {
                    (true, Some(r)) => r.urls.clone(),
                    _ => self.cfg.turn_urls.clone(),
                };
                let mut reply = json!({ "username": username, "credential": credential, "ttl": ttl, "urls": urls });
                if !self.cfg.relays.is_empty() {
                    reply["relays"] = self
                        .cfg
                        .relays
                        .iter()
                        .map(|r| json!({ "region": r.region, "urls": r.urls }))
                        .collect();
                }
                Ok(reply)
            }
            Request::Unregister {} => {
                inner.conns.remove(&me);
                inner.drop_device(&me, "unregistered");
                if let Some(rec) = inner.store.data.paired.remove(&me) {
                    for c in &rec.clients {
                        send_to(&inner.conns, c, &Event::Revoked { host: me });
                    }
                    inner.rebuild_index();
                }
                inner.store.data.devices.remove(&me);
                inner.store.save();
                inner.limits.refund(Kind::Register, Key::ip(ctx.ip));
                info!(device = %me.short(), role = ctx.role.as_str(), "unregistered");
                Ok(json!({}))
            }
            Request::Sleep {} => {
                // The host is going to sleep and will disconnect (the server closes the
                // connection after this reply); its clients then see it asleep.
                need(ctx, Role::Host)?;
                let since = unix_now();
                if let Some(rec) = inner.store.data.devices.get_mut(&me) {
                    rec.asleep_since = Some(since);
                    inner.store.save();
                }
                info!(device = %me.short(), "host going to sleep");
                Ok(json!({ "since": since }))
            }
        }
    }

    /// Housekeeping: expire pairing sessions, count hosts asleep for over 24 h as
    /// offline, and forget idle rate-limit buckets.
    pub fn tick(&self) {
        let now = Instant::now();
        let mut g = self.lock();
        g.close_pairings(|_, p| p.expires <= now, "expired");
        let (unix, ttl) = (unix_now(), g.asleep_ttl);
        let woke_never: Vec<DeviceId> = g
            .store
            .data
            .devices
            .iter()
            .filter(|(_, r)| r.asleep_since.is_some_and(|s| unix.saturating_sub(s) >= ttl))
            .map(|(d, _)| *d)
            .collect();
        if !woke_never.is_empty() {
            for d in &woke_never {
                if let Some(rec) = g.store.data.devices.get_mut(d) {
                    rec.asleep_since = None;
                }
                g.broadcast_presence(d);
                info!(device = %d.short(), "asleep too long; now counted as offline");
            }
            g.store.save();
        }
        // Clients that no host has paired and that have been gone for client_idle_ttl: forgotten (they register again on
        // their next start). Without this, addresses that register and leave would fill the table.
        let idle = self.cfg.client_idle_ttl.as_secs();
        if idle > 0 {
            let inner = &mut *g;
            let before = inner.store.data.devices.len();
            let (conns, authorised) = (&inner.conns, &inner.authorised_by);
            inner.store.data.devices.retain(|d, r| {
                r.role != Role::Client
                    || conns.contains_key(d)
                    || authorised.get(d).is_some_and(|h| !h.is_empty())
                    || unix.saturating_sub(r.last_seen) < idle
            });
            if inner.store.data.devices.len() != before {
                info!(forgotten = before - inner.store.data.devices.len(), "idle unpaired clients forgotten");
                inner.store.save();
            }
        }
        g.limits.gc();
    }

    /// Re-reads the host allowlist and disconnects hosts no longer on it.
    pub fn reload_allowlist(&self) {
        // Only the allowlist mode drops hosts; with tokens the list only exempts.
        if self.cfg.enrol != Enrol::Allowlist {
            return;
        }
        let Some(path) = &self.cfg.hosts_allow else { return };
        let allow = match read_allowlist(path) {
            Ok(a) => a,
            Err(e) => {
                error!(error = %e, "reading the host allowlist failed; keeping connections");
                return;
            }
        };
        let mut g = self.lock();
        let inner = &mut *g;
        let gone: Vec<DeviceId> = inner
            .conns
            .iter()
            .filter(|(d, c)| c.role == Role::Host && !allow.contains(d))
            .map(|(d, _)| *d)
            .collect();
        for d in gone {
            if let Some(c) = inner.conns.remove(&d) {
                c.outbox.kill();
            }
            inner.drop_device(&d, "peer_offline");
            inner.broadcast_presence(&d);
            info!(device = %d.short(), "host removed from the allowlist; disconnected");
        }
        info!(hosts = allow.len(), "allowlist reloaded");
    }
}
