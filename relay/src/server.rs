// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! HTTP and WebSocket front end: TLS, the `/remote` endpoint and one task per connection.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use rand::RngCore;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;
use tracing::{debug, info, warn};

use crate::hub::{Attached, ConnCtx, Hub, Outbox, outbox};
use crate::limits::Key;
use crate::proto::{
    ApiError, Auth, DeviceId, ErrorCode, Event, MAX_FRAME, PROTOCOL_VERSION, Request, auth_message,
    b64, error_frame, result_frame,
};

const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(20);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Open connections one address (an IPv6 /64) may hold, and all addresses together.
/// The total stays below the service's file limit (LimitNOFILE=4096), so a flood
/// of half-open connections can't take every descriptor.
const MAX_CONNS_PER_IP: u32 = 256;
const MAX_CONNS: u32 = 3584;

/// Counts open connections at accept, before TLS, so the limit applies to stalled
/// handshakes too. A slot is freed when its [`ConnSlot`] drops.
#[derive(Default)]
struct ConnLimiter {
    inner: Mutex<(u32, HashMap<Key, u32>)>,
}

struct ConnSlot {
    limiter: Arc<ConnLimiter>,
    key: Key,
}

impl ConnLimiter {
    fn acquire(self: &Arc<Self>, ip: IpAddr, per_ip: u32, total: u32) -> Option<ConnSlot> {
        let key = Key::ip(ip);
        let mut g = self.inner.lock().unwrap();
        let (all, by_ip) = &mut *g;
        let mine = by_ip.entry(key).or_insert(0);
        if *all >= total || *mine >= per_ip {
            if *mine == 0 {
                by_ip.remove(&key);
            }
            return None;
        }
        *mine += 1;
        *all += 1;
        Some(ConnSlot { limiter: self.clone(), key })
    }
}

impl Drop for ConnSlot {
    fn drop(&mut self) {
        let mut g = self.limiter.inner.lock().unwrap();
        let (all, by_ip) = &mut *g;
        *all -= 1;
        if let Some(n) = by_ip.get_mut(&self.key) {
            *n -= 1;
            if *n == 0 {
                by_ip.remove(&self.key);
            }
        }
    }
}

pub fn router(hub: Arc<Hub>) -> Router {
    Router::new()
        .route("/remote", get(ws_handler))
        .route("/healthz", get(|| async { "ok\n" }))
        .with_state(hub)
}

/// TLS for the listener: the certificate chain and key from PEM files, HTTP/1.1 only.
pub fn load_tls(cert: &Path, key: &Path) -> anyhow::Result<TlsAcceptor> {
    let certs = CertificateDer::pem_file_iter(cert)
        .with_context(|| format!("reading {}", cert.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parsing {}", cert.display()))?;
    let key = PrivateKeyDer::from_pem_file(key).with_context(|| format!("reading {}", key.display()))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(cfg)))
}

/// Accepts connections until `shutdown` resolves. With `tls` set, every connection is
/// TLS; without it (tests only) they're plain.
pub async fn serve(
    listener: TcpListener,
    tls: Option<TlsAcceptor>,
    app: Router,
    shutdown: impl Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    let limiter = Arc::new(ConnLimiter::default());
    loop {
        let (tcp, peer) = tokio::select! {
            r = listener.accept() => match r {
                Ok(x) => x,
                Err(e) => {
                    warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            _ = &mut shutdown => break,
        };
        let Some(slot) = limiter.acquire(peer.ip(), MAX_CONNS_PER_IP, MAX_CONNS) else {
            debug!(peer = %peer.ip(), "too many open connections; refused");
            drop(tcp);
            continue;
        };
        let _ = tcp.set_nodelay(true);
        let app = app.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            let _slot = slot;
            match tls {
                Some(acceptor) => match timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await {
                    Ok(Ok(stream)) => serve_conn(stream, peer, app).await,
                    Ok(Err(e)) => debug!(error = %e, "tls handshake failed"),
                    Err(_) => debug!("tls handshake timed out"),
                },
                None => serve_conn(tcp, peer, app).await,
            }
        });
    }
}

async fn serve_conn<S>(io: S, peer: SocketAddr, app: Router)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let svc = hyper::service::service_fn(move |mut req: hyper::Request<Incoming>| {
        req.extensions_mut().insert(ConnectInfo(peer));
        app.clone().oneshot(req)
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(Duration::from_secs(10))
        .serve_connection(TokioIo::new(io), svc)
        .with_upgrades();
    let _ = conn.await;
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(hub): State<Arc<Hub>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let ip = client_ip(&hub.config().trusted_proxies, peer.ip(), &headers);
    if !hub.admit(ip) {
        return (StatusCode::TOO_MANY_REQUESTS, "slow down\n").into_response();
    }
    ws.max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| run_connection(hub, socket, ip))
}

/// The address a connection counts as: the TCP peer, or, when the peer is a trusted
/// proxy, the address in its `X-Real-IP` header.
fn client_ip(trusted: &[IpAddr], peer: IpAddr, headers: &HeaderMap) -> IpAddr {
    if !trusted.contains(&peer) {
        return peer;
    }
    headers.get("x-real-ip").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok()).unwrap_or(peer)
}

async fn send(ws: &mut WebSocket, frame: String) -> Result<(), axum::Error> {
    ws.send(Message::Text(frame.into())).await
}

fn bad(message: &'static str) -> ApiError {
    ApiError::new(ErrorCode::BadRequest, message)
}

enum AuthErr {
    Refused(ApiError),
    /// Register at this server instead.
    Redirect(String),
}

impl From<ApiError> for AuthErr {
    fn from(e: ApiError) -> Self {
        AuthErr::Refused(e)
    }
}

async fn authenticate(
    hub: &Hub,
    ws: &mut WebSocket,
    nonce: &[u8],
    ip: IpAddr,
    ob: Outbox,
) -> Result<Attached, AuthErr> {
    let text = loop {
        match ws.recv().await {
            Some(Ok(Message::Text(t))) => break t,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            _ => return Err(bad("expected an auth frame").into()),
        }
    };
    let auth: Auth = serde_json::from_str(text.as_str()).map_err(|_| bad("expected an auth frame"))?;
    if auth.t != "auth" {
        return Err(bad("expected an auth frame").into());
    }
    let failed = ApiError::new(ErrorCode::AuthFailed, "authentication failed");
    let device = DeviceId::parse(&auth.device).ok_or(failed.clone())?;
    // The device signs the host name it connected to over TLS; any of this server's names will do.
    if !hub.config().server_names().any(|name| device.verify(&auth_message(&name, auth.role, nonce), &auth.sig)) {
        info!(device = %device.short(), "auth failed: bad signature");
        return Err(failed.into());
    }
    if let Some(url) = hub.redirect_for(&device) {
        info!(device = %device.short(), "registration redirected");
        return Err(AuthErr::Redirect(url));
    }
    Ok(hub.attach(device, auth.role, ip, ob, auth.token.as_deref())?)
}

/// One request frame in, one reply frame out. The bool says to close afterwards.
fn dispatch(hub: &Hub, ctx: &ConnCtx, text: &str) -> (String, bool) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return (error_frame(None, &bad("not JSON")), false);
    };
    let Some(id) = v.get("id").and_then(Value::as_u64) else {
        return (error_frame(None, &bad("every request needs a numeric id")), false);
    };
    let req: Request = match serde_json::from_value(v) {
        Ok(r) => r,
        Err(_) => return (error_frame(Some(id), &bad("unknown request or bad fields")), false),
    };
    let name = req.name();
    let stop = matches!(req, Request::Unregister {} | Request::Sleep {});
    match hub.handle(ctx, req) {
        Ok(fields) => (result_frame(id, fields), stop),
        Err(e) => {
            debug!(device = %ctx.device.short(), request = name, code = e.code.as_str(), "request refused");
            (error_frame(Some(id), &e), false)
        }
    }
}

async fn run_connection(hub: Arc<Hub>, mut ws: WebSocket, ip: IpAddr) {
    let mut nonce = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut nonce);
    let challenge = Event::Challenge {
        v: PROTOCOL_VERSION,
        server: hub.config().server_name.clone(),
        nonce: b64(&nonce),
        source: crate::SOURCE_URL,
    };
    if send(&mut ws, serde_json::to_string(&challenge).expect("serializes")).await.is_err() {
        return;
    }

    let (ob, mut obrx) = outbox();
    let ctx = match timeout(AUTH_TIMEOUT, authenticate(&hub, &mut ws, &nonce, ip, ob)).await {
        Ok(Ok(Attached { ctx, registered, account, service })) => {
            let notice = hub.config().notice.clone();
            let ok = Event::AuthOk { device: ctx.device, role: ctx.role, registered, account, notice, service };
            if send(&mut ws, serde_json::to_string(&ok).expect("serializes")).await.is_err() {
                hub.detach(&ctx);
                return;
            }
            ctx
        }
        Ok(Err(AuthErr::Refused(e))) => {
            let _ = send(&mut ws, error_frame(None, &e)).await;
            let _ = ws.send(Message::Close(None)).await;
            return;
        }
        Ok(Err(AuthErr::Redirect(url))) => {
            let _ = send(&mut ws, serde_json::to_string(&Event::Redirect { url }).expect("serializes")).await;
            let _ = ws.send(Message::Close(None)).await;
            return;
        }
        Err(_) => {
            let _ = send(&mut ws, error_frame(None, &bad("auth timed out"))).await;
            let _ = ws.send(Message::Close(None)).await;
            return;
        }
    };

    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let mut last_rx = Instant::now();
    loop {
        tokio::select! {
            msg = ws.recv() => {
                let Some(Ok(msg)) = msg else { break };
                last_rx = Instant::now();
                match msg {
                    Message::Text(t) => {
                        let (reply, stop) = dispatch(&hub, &ctx, t.as_str());
                        if send(&mut ws, reply).await.is_err() || stop {
                            break;
                        }
                    }
                    Message::Binary(_) => {
                        if send(&mut ws, error_frame(None, &bad("binary frames are not used"))).await.is_err() {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => {}
                }
            }
            out = obrx.rx.recv() => {
                let Some(frame) = out else { break };
                if send(&mut ws, frame).await.is_err() {
                    break;
                }
            }
            _ = obrx.kill.notified() => {
                // Deliver what is already queued (such as "replaced"), briefly, then close.
                let _ = timeout(Duration::from_secs(2), async {
                    while let Ok(frame) = obrx.rx.try_recv() {
                        if send(&mut ws, frame).await.is_err() {
                            break;
                        }
                    }
                }).await;
                break;
            }
            _ = ping.tick() => {
                if last_rx.elapsed() > IDLE_TIMEOUT
                    || ws.send(Message::Ping(Bytes::new())).await.is_err()
                {
                    break;
                }
            }
        }
    }
    let _ = ws.send(Message::Close(None)).await;
    hub.detach(&ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_slots_are_limited_per_address_and_in_total() {
        let l = Arc::new(ConnLimiter::default());
        let a: IpAddr = "203.0.113.1".parse().unwrap();
        let b: IpAddr = "203.0.113.2".parse().unwrap();
        let first = l.acquire(a, 2, 3).unwrap();
        let _second = l.acquire(a, 2, 3).unwrap();
        assert!(l.acquire(a, 2, 3).is_none(), "per-address limit");
        let _third = l.acquire(b, 2, 3).unwrap();
        assert!(l.acquire(b, 2, 3).is_none(), "total limit");
        drop(first);
        assert!(l.acquire(a, 2, 3).is_some(), "a closed connection frees its slot");
    }
}
