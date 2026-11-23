// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The rendezvous client (rendezvous-api.md v1): WSS to the configured server, the
//! challenge, then typed requests and events.
//!
//! - **Which server:** nothing is compiled in. A server comes
//!   from `server.toml` or a `sodium-remote://server` link (`server_config`); with
//!   none, devices work on the LAN and by address only.
//! - **Trust:** Web PKI through the platform's root certificates, unless the
//!   configuration pins a CA (`ca_pem`), in which case only that CA is trusted. The
//!   same TLS config serves TURN over TLS. A CA is never fetched at run time.
//! - **Shape:** `Rendezvous::connect` returns a cloneable handle (requests are
//!   `async fn`s that wait for their own reply) and a receiver of server events.
//!   Reconnecting is the caller's job (`connect.rs` does it with backoff). A
//!   `redirect` on registration is followed once, only to the configured host or a subdomain of it.
//! - The server is untrusted: everything secret travels sealed (`keys::seal`).

use crate::keys::{DeviceId, DeviceKey};
use crate::path::{TurnServer, TurnTransport};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::{CertificateDer, ServerName, pem::PemObject};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

const AUTH_CONTEXT: &[u8] = b"remote-rendezvous/v1/auth\0";
const PAIRED_LIST_CONTEXT: &[u8] = b"remote-rendezvous/v1/paired-list\0";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a relay gets to answer the RTT check before it counts as unreachable.
const RELAY_PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// Error codes a server may send that the apps
/// show words for, beyond the original set.
pub mod codes {
    /// The enrolment token isn't known to this server (or was mistyped).
    pub const TOKEN_INVALID: &str = "token_invalid";
    /// The enrolment token is past its expiry or has been used up.
    pub const TOKEN_EXPIRED: &str = "token_expired";
    /// The account behind this device is over a quota (devices, relay time, …).
    pub const QUOTA_EXCEEDED: &str = "quota_exceeded";
    /// The account behind this device needs a payment before it can go on.
    pub const PAYMENT_REQUIRED: &str = "payment_required";
    /// This host isn't enrolled on the server (no token, not on its allowlist).
    pub const HOST_NOT_ALLOWED: &str = "host_not_allowed";
}

/// Where the rendezvous is and how to trust it.
#[derive(Clone, Debug, PartialEq)]
pub struct RendezvousConfig {
    /// `wss://host[:port]/path`.
    pub url: String,
    /// PEM of a CA to pin. `None`: Web PKI through the platform's roots.
    pub ca_pem: Option<Vec<u8>>,
    /// The enrolment token, sent when this device registers.
    pub token: Option<String>,
}

impl RendezvousConfig {
    pub fn new(url: impl Into<String>) -> Self {
        RendezvousConfig { url: url.into(), ca_pem: None, token: None }
    }

    /// The server's host name, as the URL gives it.
    pub fn host(&self) -> String {
        url_host(&self.url).unwrap_or_default()
    }
}

/// The host part of a `wss://host[:port]/path` URL (IPv6 literals without brackets).
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("wss://")?;
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = if let Some(v6) = hostport.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        hostport.rsplit_once(':').map(|(h, _)| h).unwrap_or(hostport)
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase().trim_end_matches('.').to_string())
}

/// May a registration at `from` (a URL) be redirected to `to`? Only to `wss://`,
/// and only to the configured host itself or a subdomain of it (`sub.example.com`
/// from `example.com`; never a sibling or the parent). An IP address only to itself.
pub fn redirect_allowed(from: &str, to: &str) -> bool {
    let (Some(a), Some(b)) = (url_host(from), url_host(to)) else { return false };
    if a.parse::<std::net::IpAddr>().is_ok() {
        return a == b;
    }
    b == a || b.ends_with(&format!(".{a}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceRole {
    Host,
    Client,
}

impl DeviceRole {
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceRole::Host => "host",
            DeviceRole::Client => "client",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RdvError {
    /// Couldn't reach the server, or the connection dropped.
    Io(String),
    /// The server refused: rendezvous-api.md §10's code (`host_not_allowed`,
    /// `not_authorised`, `rate_limited`, the [`codes`] for enrolment and accounts, …).
    Server { code: String, message: String },
    Timeout,
    Protocol(String),
    /// The server sent this registration elsewhere (`connect` follows it once).
    Redirect(String),
}

impl RdvError {
    pub fn code(&self) -> Option<&str> {
        match self {
            RdvError::Server { code, .. } => Some(code),
            _ => None,
        }
    }
}

impl std::fmt::Display for RdvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RdvError::Io(e) => write!(f, "rendezvous unreachable: {e}"),
            RdvError::Server { code, message } => write!(f, "rendezvous refused: {code} ({message})"),
            RdvError::Timeout => write!(f, "rendezvous timed out"),
            RdvError::Protocol(e) => write!(f, "rendezvous protocol error: {e}"),
            RdvError::Redirect(u) => write!(f, "rendezvous redirected to {u}"),
        }
    }
}

impl std::error::Error for RdvError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostState {
    Online,
    Offline,
    /// The host sent a sleep notice; `asleep_since` says when.
    Asleep,
    NotAuthorised,
}

/// Unsolicited frames from the server.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Incoming { session: String, client: DeviceId },
    Signal { session: String, blob: Vec<u8> },
    SessionClosed { session: String, reason: String },
    PairJoined { pairing: String, attempt: u32 },
    PairMsg { pairing: String, blob: Vec<u8> },
    PairLeft { pairing: String },
    PairClosed { pairing: String, reason: String },
    Presence { host: DeviceId, state: HostState, last_seen: Option<u64>, asleep_since: Option<u64> },
    Revoked { host: DeviceId },
    /// This device authenticated again elsewhere; this connection is closed.
    Replaced,
    /// A message from the server's operator (also given at `auth_ok`).
    Notice(Notice),
    /// The connection is gone (the last event).
    Disconnected { error: Option<String> },
}

/// A message from the server's operator: `{code, text, url}` (rendezvous-api.md
/// section 4). `text` is the operator's own words, shown as they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub code: String,
    pub text: String,
    pub url: Option<String>,
}

fn parse_notice(v: &Value) -> Option<Notice> {
    Some(Notice {
        code: v["code"].as_str()?.to_string(),
        text: v["text"].as_str().unwrap_or_default().to_string(),
        url: v["url"].as_str().filter(|u| u.starts_with("https://")).map(String::from),
    })
}

/// One relay of the server's list, with the region it is in ("sg", "us-west", …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relay {
    pub region: String,
    pub urls: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HostPresence {
    pub host: DeviceId,
    pub state: HostState,
    pub last_seen: Option<u64>,
    /// Unix seconds; set only while `state` is `Asleep`.
    pub asleep_since: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpenWindow {
    pub pairing: String,
    pub host: DeviceId,
    pub name: String,
    pub expires_in: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TurnCredentials {
    pub username: String,
    pub credential: String,
    pub ttl: u64,
    /// The relay to use when the server lists no regions.
    pub urls: Vec<String>,
    /// Relays in several regions (optional); the client takes the nearest by RTT.
    pub relays: Vec<Relay>,
}

/// `host:port` of a `stun:`/`turn:`/`turns:` URL.
fn ice_url_hostport(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once(':')?;
    let hostport = rest.split('?').next()?;
    hostport.rsplit_once(':').map(|_| hostport)
}

/// The index of the fastest probe; on a tie, or when every probe failed, the
/// first one: the server's order.
fn nearest(rtts: &[Option<Duration>]) -> usize {
    let mut best: Option<(usize, Duration)> = None;
    for (i, t) in rtts.iter().enumerate() {
        if let Some(t) = *t {
            if best.is_none_or(|(_, b)| t < b) {
                best = Some((i, t));
            }
        }
    }
    best.map_or(0, |(i, _)| i)
}

impl TurnCredentials {
    /// The URLs to use: with a region list, the relay that answers fastest (a TCP
    /// connect to its first URL's port, all at once; no picker); else `urls`.
    pub async fn chosen_urls(&self) -> Vec<String> {
        if self.relays.len() < 2 {
            return self.relays.first().map(|r| r.urls.clone()).unwrap_or_else(|| self.urls.clone());
        }
        let probes = self.relays.iter().map(|r| async move {
            let hp = r.urls.iter().find_map(|u| ice_url_hostport(u))?;
            let t0 = Instant::now();
            tokio::time::timeout(RELAY_PROBE_TIMEOUT, TcpStream::connect(hp)).await.ok()?.ok()?;
            Some(t0.elapsed())
        });
        let rtts = futures_util::future::join_all(probes).await;
        let i = nearest(&rtts);
        if let Some(t) = rtts[i] {
            tracing::info!("relay region {} ({} ms)", self.relays[i].region, t.as_millis());
        }
        self.relays[i].urls.clone()
    }

    /// Resolve the chosen URLs into STUN servers and TURN servers for `PathConfig`.
    /// `turns:` entries become TLS; the name on the certificate is the URL's host.
    pub async fn resolve(&self) -> (Vec<SocketAddr>, Vec<TurnServer>) {
        let mut stun = Vec::new();
        let mut turn = Vec::new();
        for url in &self.chosen_urls().await {
            let Some((scheme, rest)) = url.split_once(':') else { continue };
            let (hostport, query) = rest.split_once('?').unwrap_or((rest, ""));
            let Some((host, _)) = hostport.rsplit_once(':') else { continue };
            let Ok(Some(addr)) = tokio::net::lookup_host(hostport).await.map(|mut a| a.find(|a| a.is_ipv4())) else {
                continue;
            };
            let tcp = query.contains("transport=tcp");
            let transport = match (scheme, tcp) {
                ("stun", _) => {
                    stun.push(addr);
                    continue;
                }
                ("turn", false) => TurnTransport::Udp,
                ("turn", true) => TurnTransport::Tcp,
                ("turns", _) => TurnTransport::Tls,
                _ => continue,
            };
            turn.push(TurnServer {
                addr,
                transport,
                server_name: host.to_string(),
                username: self.username.clone(),
                password: self.credential.clone(),
            });
        }
        (stun, turn)
    }
}

/// The platform's root certificates, read once.
fn platform_roots() -> Result<rustls::RootCertStore, String> {
    static ROOTS: OnceLock<Result<rustls::RootCertStore, String>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let found = rustls_native_certs::load_native_certs();
            let mut roots = rustls::RootCertStore::empty();
            let (added, _) = roots.add_parsable_certificates(found.certs);
            if added == 0 {
                return Err(format!("no system root certificates ({:?})", found.errors.first()));
            }
            Ok(roots)
        })
        .clone()
}

/// A rustls client config for the rendezvous and TURN over TLS: only `ca_pem`
/// when given (a pinned CA), else Web PKI through the platform's roots.
pub fn tls_config(ca_pem: Option<&[u8]>, alpn_http: bool) -> Result<Arc<rustls::ClientConfig>, String> {
    let roots = match ca_pem {
        Some(pem) => {
            let mut roots = rustls::RootCertStore::empty();
            for cert in CertificateDer::pem_slice_iter(pem) {
                roots.add(cert.map_err(|e| format!("bad CA PEM: {e}"))?).map_err(|e| format!("bad CA: {e}"))?;
            }
            if roots.is_empty() {
                return Err("no CA certificate in the PEM".into());
            }
            roots
        }
        None => platform_roots()?,
    };
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    if alpn_http {
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    }
    Ok(Arc::new(cfg))
}

pub fn b64(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

pub fn unb64(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s).ok()
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RdvError>>>>>;

/// A connection to the rendezvous. Clones share it.
#[derive(Clone)]
pub struct Rendezvous {
    out: mpsc::UnboundedSender<Message>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
    pub device: DeviceId,
    pub role: DeviceRole,
    /// The account this device belongs to on the server, if it has accounts.
    pub account: Option<String>,
    /// The operator's message at authentication, if any.
    pub notice: Option<Notice>,
    /// The URL this connection is to (after a redirect, the new one).
    pub url: String,
}

impl Rendezvous {
    /// Connect, authenticate, and start the reader. Returns the handle, the
    /// event receiver, and whether this registered the device just now.
    pub async fn connect(
        cfg: &RendezvousConfig,
        key: &DeviceKey,
        role: DeviceRole,
    ) -> Result<(Rendezvous, mpsc::UnboundedReceiver<Event>, bool), RdvError> {
        let first = tokio::time::timeout(CONNECT_TIMEOUT * 2, Self::connect_inner(cfg, &cfg.url, key, role))
            .await
            .map_err(|_| RdvError::Timeout)?;
        match first {
            // A registration sent elsewhere (a cluster): follow it once, inside the
            // operator's own domain, with the same trust and token.
            Err(RdvError::Redirect(to)) => {
                if !redirect_allowed(&cfg.url, &to) {
                    return Err(RdvError::Protocol(format!("refused a redirect to {to}: not the same domain")));
                }
                tracing::info!("rendezvous: redirected to {to}");
                match tokio::time::timeout(CONNECT_TIMEOUT * 2, Self::connect_inner(cfg, &to, key, role)).await {
                    Ok(Err(RdvError::Redirect(_))) => Err(RdvError::Protocol("redirected twice".into())),
                    Ok(r) => r,
                    Err(_) => Err(RdvError::Timeout),
                }
            }
            r => r,
        }
    }

    async fn connect_inner(
        cfg: &RendezvousConfig,
        url: &str,
        key: &DeviceKey,
        role: DeviceRole,
    ) -> Result<(Rendezvous, mpsc::UnboundedReceiver<Event>, bool), RdvError> {
        let rest = url.strip_prefix("wss://").ok_or_else(|| RdvError::Protocol("the URL must be wss://".into()))?;
        let hostport = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        let host = url_host(url).ok_or_else(|| RdvError::Protocol("no host in the URL".into()))?;
        let addr = if hostport.ends_with(']') || !hostport.contains(':') { format!("{hostport}:443") } else { hostport.to_string() };
        let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&addr))
            .await
            .map_err(|_| RdvError::Timeout)?
            .map_err(|e| RdvError::Io(e.to_string()))?;
        let _ = tcp.set_nodelay(true);
        let tls = tokio_rustls::TlsConnector::from(tls_config(cfg.ca_pem.as_deref(), true).map_err(RdvError::Protocol)?);
        let name = ServerName::try_from(host.clone()).map_err(|e| RdvError::Protocol(e.to_string()))?;
        let stream = tls.connect(name, tcp).await.map_err(|e| RdvError::Io(format!("TLS: {e}")))?;
        let (mut ws, _) = tokio_tungstenite::client_async(url, stream).await.map_err(|e| RdvError::Io(e.to_string()))?;

        // The challenge.
        let ch = next_json(&mut ws).await?;
        if ch["t"] != "challenge" {
            return Err(RdvError::Protocol(format!("expected a challenge, got {ch}")));
        }
        let nonce = ch["nonce"].as_str().and_then(unb64).ok_or_else(|| RdvError::Protocol("bad nonce".into()))?;
        // The signature binds the server this device connected to and checked over TLS (the URL's host,
        // after any redirect), never the name the server sends: a server that forwarded another server's
        // challenge would otherwise get a signature that works there.
        if ch["server"].as_str().is_some_and(|s| !s.eq_ignore_ascii_case(&host)) {
            tracing::debug!("rendezvous: the server calls itself {}; signing for {host}", ch["server"]);
        }
        let mut m = AUTH_CONTEXT.to_vec();
        m.extend_from_slice(host.as_bytes());
        m.push(0);
        m.extend_from_slice(role.as_str().as_bytes());
        m.push(0);
        m.extend_from_slice(&nonce);
        let mut auth = json!({"t": "auth", "device": key.id().to_string(), "role": role.as_str(), "sig": b64(&key.sign(&m))});
        if let Some(token) = cfg.token.as_deref().filter(|t| !t.is_empty()) {
            auth["token"] = json!(token);
        }
        ws.send(Message::Text(auth.to_string().into())).await.map_err(|e| RdvError::Io(e.to_string()))?;
        let ok = next_json(&mut ws).await?;
        match ok["t"].as_str() {
            Some("auth_ok") => {}
            Some("error") => return Err(server_error(&ok)),
            Some("redirect") => {
                let to = ok["url"].as_str().unwrap_or_default().to_string();
                let _ = ws.close(None).await;
                return Err(RdvError::Redirect(to));
            }
            _ => return Err(RdvError::Protocol(format!("expected auth_ok, got {ok}"))),
        }
        let registered = ok["registered"].as_bool().unwrap_or(false);
        let account = ok["account"].as_str().map(String::from);
        let notice = parse_notice(&ok["notice"]);
        if let Some(n) = &notice {
            tracing::info!("rendezvous notice {}: {}", n.code, n.text);
        }

        let (mut sink, mut stream) = ws.split();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let pending: Pending = Default::default();
        tokio::spawn(async move {
            while let Some(m) = out_rx.recv().await {
                let close = matches!(m, Message::Close(_));
                if sink.send(m).await.is_err() || close {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let p = pending.clone();
        tokio::spawn(async move {
            let mut error = None;
            loop {
                match stream.next().await {
                    Some(Ok(Message::Text(t))) => {
                        let Ok(v) = serde_json::from_str::<Value>(t.as_str()) else { continue };
                        dispatch(v, &p, &ev_tx);
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        error = Some(e.to_string());
                        break;
                    }
                }
            }
            for (_, tx) in p.lock().unwrap().drain() {
                let _ = tx.send(Err(RdvError::Io("connection closed".into())));
            }
            let _ = ev_tx.send(Event::Disconnected { error });
        });
        Ok((
            Rendezvous {
                out: out_tx,
                pending,
                next_id: Arc::new(AtomicU64::new(1)),
                device: key.id(),
                role,
                account,
                notice,
                url: url.to_string(),
            },
            ev_rx,
            registered,
        ))
    }

    pub fn is_closed(&self) -> bool {
        self.out.is_closed()
    }

    /// Send a request and wait for its reply.
    pub async fn request(&self, t: &str, mut fields: Value) -> Result<Value, RdvError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        fields["t"] = json!(t);
        fields["id"] = json!(id);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if self.out.send(Message::Text(fields.to_string().into())).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(RdvError::Io("connection closed".into()));
        }
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(RdvError::Io("connection closed".into())),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(RdvError::Timeout)
            }
        }
    }

    /// Close the connection (presence goes offline at once).
    pub fn close(&self) {
        let _ = self.out.send(Message::Close(None));
    }

    // --- host requests -----------------------------------------------------

    /// Upload the signed paired-client list (replaces the stored one).
    pub async fn upload_paired_list(&self, key: &DeviceKey, seq: u64, clients: &[DeviceId]) -> Result<(), RdvError> {
        let ids: Vec<String> = clients.iter().map(|c| c.to_string()).collect();
        // v2 names the server it is for, so a server can't hand an old list on to another one.
        let server = url_host(&self.url).unwrap_or_default();
        let payload = json!({"v": 2, "host": key.id().to_string(), "server": server, "seq": seq, "clients": ids}).to_string();
        let mut m = PAIRED_LIST_CONTEXT.to_vec();
        m.extend_from_slice(payload.as_bytes());
        let sig = key.sign(&m);
        self.request("paired_list", json!({"list": b64(payload.as_bytes()), "sig": b64(&sig)})).await.map(|_| ())
    }

    /// The `seq` of the list the server holds (0 if none).
    pub async fn paired_list_seq(&self) -> Result<u64, RdvError> {
        Ok(self.request("get_paired_list", json!({})).await?["seq"].as_u64().unwrap_or(0))
    }

    /// Open a 5-minute pairing window. Returns its ID and lifetime (s).
    pub async fn pair_open(&self, name: &str) -> Result<(String, u64), RdvError> {
        let r = self.request("pair_open", json!({"name": name})).await?;
        Ok((str_field(&r, "pairing")?, r["expires_in"].as_u64().unwrap_or(0)))
    }

    /// The current attempt failed; returns the attempts left.
    pub async fn pair_fail(&self, pairing: &str) -> Result<u32, RdvError> {
        Ok(self.request("pair_fail", json!({"pairing": pairing})).await?["attempts_left"].as_u64().unwrap_or(0) as u32)
    }

    pub async fn pair_close(&self, pairing: &str) -> Result<(), RdvError> {
        self.request("pair_close", json!({"pairing": pairing})).await.map(|_| ())
    }

    pub async fn pair_msg(&self, pairing: &str, blob: &[u8]) -> Result<(), RdvError> {
        self.request("pair_msg", json!({"pairing": pairing, "blob": b64(blob)})).await.map(|_| ())
    }

    pub async fn signal(&self, session: &str, blob: &[u8]) -> Result<(), RdvError> {
        self.request("signal", json!({"session": session, "blob": b64(blob)})).await.map(|_| ())
    }

    pub async fn close_session(&self, session: &str) -> Result<(), RdvError> {
        self.request("close", json!({"session": session})).await.map(|_| ())
    }

    pub async fn turn_credentials(&self) -> Result<TurnCredentials, RdvError> {
        let r = self.request("turn_credentials", json!({})).await?;
        Ok(TurnCredentials {
            username: str_field(&r, "username")?,
            credential: str_field(&r, "credential")?,
            ttl: r["ttl"].as_u64().unwrap_or(0),
            urls: strings(&r["urls"]),
            relays: r["relays"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| Some(Relay { region: x["region"].as_str()?.to_string(), urls: strings(&x["urls"]) }))
                        .filter(|r| !r.urls.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Forget this device on the server (its record and, for a host, its
    /// list). The server closes the connection afterwards.
    pub async fn unregister(&self) -> Result<(), RdvError> {
        self.request("unregister", json!({})).await.map(|_| ())
    }

    /// The host is about to sleep (logind `PrepareForSleep`): the server marks it
    /// asleep (its clients see "asleep" with `since`) and closes the connection.
    pub async fn sleep_notice(&self) -> Result<(), RdvError> {
        let r = self.request("sleep", json!({})).await;
        self.close();
        r.map(|_| ())
    }

    // --- client requests ---------------------------------------------------

    pub async fn presence(&self, hosts: &[DeviceId]) -> Result<Vec<HostPresence>, RdvError> {
        let ids: Vec<String> = hosts.iter().map(|h| h.to_string()).collect();
        let r = self.request("presence", json!({"hosts": ids})).await?;
        Ok(r["hosts"].as_array().map(|a| a.iter().filter_map(parse_presence).collect()).unwrap_or_default())
    }

    /// Ask for a host; returns the signalling session.
    pub async fn connect_host(&self, host: &DeviceId) -> Result<String, RdvError> {
        str_field(&self.request("connect", json!({"host": host.to_string()})).await?, "session")
    }

    pub async fn pair_discover(&self) -> Result<Vec<OpenWindow>, RdvError> {
        let r = self.request("pair_discover", json!({})).await?;
        Ok(r["hosts"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|w| {
                        Some(OpenWindow {
                            pairing: w["pairing"].as_str()?.to_string(),
                            host: w["host"].as_str()?.parse().ok()?,
                            name: w["name"].as_str()?.to_string(),
                            expires_in: w["expires_in"].as_u64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Start an attempt in a window. Returns the host's ID as the server knows it.
    pub async fn pair_join(&self, pairing: &str) -> Result<DeviceId, RdvError> {
        let r = self.request("pair_join", json!({"pairing": pairing})).await?;
        str_field(&r, "host")?.parse().map_err(RdvError::Protocol)
    }
}

async fn next_json<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>) -> Result<Value, RdvError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match tokio::time::timeout(CONNECT_TIMEOUT, ws.next()).await.map_err(|_| RdvError::Timeout)? {
            Some(Ok(Message::Text(t))) => {
                return serde_json::from_str(t.as_str()).map_err(|e| RdvError::Protocol(e.to_string()));
            }
            Some(Ok(Message::Close(_))) | None => return Err(RdvError::Io("closed".into())),
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(RdvError::Io(e.to_string())),
        }
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|u| u.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn server_error(v: &Value) -> RdvError {
    RdvError::Server {
        code: v["code"].as_str().unwrap_or("unknown").to_string(),
        message: v["message"].as_str().unwrap_or_default().to_string(),
    }
}

fn str_field(v: &Value, k: &str) -> Result<String, RdvError> {
    v[k].as_str().map(String::from).ok_or_else(|| RdvError::Protocol(format!("no {k} in {v}")))
}

fn parse_state(s: &str) -> Option<HostState> {
    Some(match s {
        "online" => HostState::Online,
        "offline" => HostState::Offline,
        "asleep" => HostState::Asleep,
        "not_authorised" => HostState::NotAuthorised,
        _ => return None,
    })
}

fn parse_presence(v: &Value) -> Option<HostPresence> {
    Some(HostPresence {
        host: v["host"].as_str()?.parse().ok()?,
        state: parse_state(v["state"].as_str()?)?,
        last_seen: v["last_seen"].as_u64(),
        asleep_since: v["since"].as_u64(),
    })
}

fn dispatch(v: Value, pending: &Pending, events: &mpsc::UnboundedSender<Event>) {
    let t = v["t"].as_str().unwrap_or_default();
    if t == "result" || t == "error" {
        if let Some(id) = v["id"].as_u64() {
            if let Some(tx) = pending.lock().unwrap().remove(&id) {
                let _ = tx.send(if t == "result" { Ok(v) } else { Err(server_error(&v)) });
            }
        } else {
            tracing::warn!("rendezvous error without id: {v}");
        }
        return;
    }
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let blob = |k: &str| v[k].as_str().and_then(unb64);
    let id = |k: &str| v[k].as_str().and_then(|s| s.parse::<DeviceId>().ok());
    let ev = match t {
        "incoming" => id("client").map(|client| Event::Incoming { session: s("session"), client }),
        "signal" => blob("blob").map(|blob| Event::Signal { session: s("session"), blob }),
        "session_closed" => Some(Event::SessionClosed { session: s("session"), reason: s("reason") }),
        "pair_joined" => Some(Event::PairJoined { pairing: s("pairing"), attempt: v["attempt"].as_u64().unwrap_or(0) as u32 }),
        "pair_msg" => blob("blob").map(|blob| Event::PairMsg { pairing: s("pairing"), blob }),
        "pair_left" => Some(Event::PairLeft { pairing: s("pairing") }),
        "pair_closed" => Some(Event::PairClosed { pairing: s("pairing"), reason: s("reason") }),
        "presence" => parse_presence(&v)
            .map(|p| Event::Presence { host: p.host, state: p.state, last_seen: p.last_seen, asleep_since: p.asleep_since }),
        "revoked" => id("host").map(|host| Event::Revoked { host }),
        "replaced" => Some(Event::Replaced),
        "notice" => parse_notice(&v).map(Event::Notice),
        _ => None,
    };
    if let Some(ev) = ev {
        let _ = events.send(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Mac app finds a server's refusal by this text (`ServerProblem.parse` in client-mac): keep it.
    #[test]
    fn a_refusal_is_reported_as_its_code_for_the_apps_to_word() {
        let e = RdvError::Server { code: codes::TOKEN_INVALID.into(), message: "no such token".into() };
        assert!(e.to_string().starts_with("rendezvous refused: token_invalid "), "{e}");
    }

    #[test]
    fn a_redirect_goes_only_to_the_configured_host_or_a_subdomain() {
        let from = "wss://remote.example.com/remote";
        assert!(redirect_allowed(from, "wss://remote.example.com:8443/r"));
        assert!(redirect_allowed(from, "wss://sg1.remote.example.com:8443/remote"));
        assert!(!redirect_allowed(from, "wss://example.com/remote"), "not the parent");
        assert!(!redirect_allowed(from, "wss://other.example.com/remote"), "not a sibling");
        assert!(!redirect_allowed(from, "wss://remote.example.com.evil.test/remote"));
        assert!(!redirect_allowed(from, "wss://evilremote.example.com/remote"));
        assert!(!redirect_allowed(from, "ws://sg1.remote.example.com/remote"), "never off TLS");
        // An address only to itself.
        assert!(redirect_allowed("wss://203.0.113.5:8443/r", "wss://203.0.113.5:9443/r"));
        assert!(!redirect_allowed("wss://203.0.113.5:8443/r", "wss://203.0.113.6/r"));
        assert_eq!(url_host("wss://[2001:db8::1]:8443/remote").as_deref(), Some("2001:db8::1"));
    }

    #[test]
    fn the_nearest_relay_keeps_the_servers_order_on_a_tie_or_when_all_fail() {
        let ms = |n| Some(Duration::from_millis(n));
        assert_eq!(nearest(&[ms(30), ms(10), ms(20)]), 1);
        assert_eq!(nearest(&[ms(10), ms(10)]), 0);
        assert_eq!(nearest(&[None, ms(5), ms(5)]), 1);
        assert_eq!(nearest(&[None, None]), 0);
    }
}
