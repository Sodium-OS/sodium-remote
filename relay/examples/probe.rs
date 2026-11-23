// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Live check of a deployed rendezvous server (run by deploy/verify.sh).
//!
//!   probe keygen <dir>                       write a test host and client key, print the host ID
//!   probe run <wss-url> <ca.pem> <dir> <out>  register both, pair, signal, get TURN credentials,
//!                                            revoke, unregister; the client's TURN credentials go
//!                                            to <out> (JSON, mode 0600) for the TURN checks.
//!   probe sleep-check <wss-url> <ca.pem> <dir> the host's sleep notice: asleep with since, then
//!                                            cleared when it comes back; unregister both.

#[path = "../tests/common/mod.rs"]
mod common;

use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, bail, ensure};
use common::{Dev, id_of, new_key, signed_list};
use ed25519_dalek::SigningKey;
use rand::RngCore;
use remote_rendezvous::proto::{Role, b64, unb64};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

type Device = Dev<TlsStream<TcpStream>>;

fn write_secret(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(data)?;
    Ok(())
}

fn read_key(path: &Path) -> anyhow::Result<SigningKey> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let seed: [u8; 32] = unb64(text.trim()).and_then(|b| b.try_into().ok()).context("bad key file")?;
    Ok(SigningKey::from_bytes(&seed))
}

async fn connect(url: &str, ca: &Path, key: SigningKey, role: Role) -> anyhow::Result<(Device, Value)> {
    let rest = url.strip_prefix("wss://").context("url must start with wss://")?;
    let authority = rest.split('/').next().unwrap();
    let (host, port) = authority.rsplit_once(':').context("url needs a port")?;
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(ca)? {
        roots.add(cert?)?;
    }
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let tcp = TcpStream::connect((host, port.parse::<u16>()?)).await?;
    let tls = TlsConnector::from(Arc::new(cfg))
        .connect(ServerName::try_from(host.to_string())?, tcp)
        .await
        .context("TLS handshake (certificate verified against the CA file)")?;
    let (ws, _) = tokio_tungstenite::client_async(url, tls).await?;
    // A server in token mode (the default) needs a token for new hosts: verify.sh mints a
    // one-use token and passes it here.
    let token = std::env::var("SODIUM_REMOTE_ENROL_TOKEN").ok().filter(|t| !t.is_empty());
    Ok(Dev::auth_token(ws, key, role, if role == Role::Host { token.as_deref() } else { None }).await)
}

fn step(name: &str, started: Instant) {
    println!("ok    {name} ({} ms)", started.elapsed().as_millis());
}

fn expect(v: &Value, t: &str, what: &str) -> anyhow::Result<()> {
    ensure!(v["t"] == t, "{what}: expected {t}, got {v}");
    Ok(())
}

/// The test devices that authenticated, so they're unregistered even when a step fails.
#[derive(Default)]
struct Registered {
    host: Option<Device>,
    client: Option<Device>,
}

async fn run(url: &str, ca: &Path, dir: &Path, out: Option<&Path>) -> anyhow::Result<()> {
    let mut reg = Registered::default();
    let result = match out {
        Some(out) => steps(url, ca, dir, out, &mut reg).await,
        None => sleep_steps(url, ca, dir, &mut reg).await,
    };
    let t = Instant::now();
    for dev in [reg.client.as_mut(), reg.host.as_mut()].into_iter().flatten() {
        let r = dev.request(json!({ "t": "unregister" })).await;
        ensure!(r["t"] == "result", "unregister: {r}");
    }
    step("test devices unregistered", t);
    result?;
    println!("ALL OK");
    Ok(())
}

/// The sleep notice: the host says it's going to sleep, the server closes it, the
/// client sees "asleep" with `since`, and the host coming back clears it.
async fn sleep_steps(url: &str, ca: &Path, dir: &Path, reg: &mut Registered) -> anyhow::Result<()> {
    let host_key = read_key(&dir.join("host.key"))?;
    let (host, ok) = connect(url, ca, host_key.clone(), Role::Host).await?;
    expect(&ok, "auth_ok", "host auth")?;
    let host = reg.host.insert(host);
    let (client, ok) = connect(url, ca, read_key(&dir.join("client.key"))?, Role::Client).await?;
    expect(&ok, "auth_ok", "client auth")?;
    let client = reg.client.insert(client);
    let r = host.request(signed_list(&host.key, remote_rendezvous::hub::unix_now(), &[&client.id])).await;
    expect(&r, "result", "paired_list")?;
    ensure!(client.event("presence").await["state"] == "online", "host online first");

    let t = Instant::now();
    let r = host.request(json!({ "t": "sleep" })).await;
    let since = r["since"].as_u64().with_context(|| format!("sleep: {r}"))?;
    ensure!(host.event("anything").await.is_null(), "the server closes the host's connection");
    let ev = client.event("presence").await;
    ensure!(ev["state"] == "asleep" && ev["since"] == since, "presence event: {ev}");
    let host_id = host.id.clone();
    let r = client.request(json!({ "t": "presence", "hosts": [host_id] })).await;
    ensure!(r["hosts"][0]["state"] == "asleep" && r["hosts"][0]["since"] == since, "presence: {r}");
    step(&format!("sleep notice: host closed, client sees asleep since {since} (event and query)"), t);

    let t = Instant::now();
    let (host, ok) = connect(url, ca, host_key, Role::Host).await?;
    expect(&ok, "auth_ok", "host back")?;
    reg.host = Some(host);
    let ev = client.event("presence").await;
    ensure!(ev["state"] == "online" && ev["since"].is_null(), "back online: {ev}");
    step("host back: asleep cleared, client sees online", t);
    Ok(())
}

async fn steps(url: &str, ca: &Path, dir: &Path, out: &Path, reg: &mut Registered) -> anyhow::Result<()> {
    let host_key = read_key(&dir.join("host.key"))?;
    let client_key = read_key(&dir.join("client.key"))?;

    let t = Instant::now();
    let (host, ok) = connect(url, ca, host_key, Role::Host).await?;
    expect(&ok, "auth_ok", "host auth")?;
    let host = reg.host.insert(host);
    step(&format!("host {} authenticated over TLS (registered: {})", &host.id[..12], ok["registered"]), t);

    let t = Instant::now();
    let (client, ok) = connect(url, ca, client_key, Role::Client).await?;
    expect(&ok, "auth_ok", "client auth")?;
    let client = reg.client.insert(client);
    step(&format!("client {} authenticated (registered: {})", &client.id[..12], ok["registered"]), t);

    // Pairing rendezvous: the host opens a window; the client finds it with no code
    // and runs (here, pretends to run) SPAKE2 with the host through opaque messages.
    let t = Instant::now();
    let r = host.request(json!({ "t": "pair_open", "name": "probe host" })).await;
    expect(&r, "result", "pair_open")?;
    let pairing = r["pairing"].as_str().context("pairing id")?.to_string();
    let r = client.request(json!({ "t": "pair_discover" })).await;
    let found = r["hosts"].as_array().into_iter().flatten().find(|h| h["pairing"] == pairing.as_str());
    ensure!(
        found.is_some_and(|h| h["host"] == host.id.as_str() && h["name"] == "probe host"),
        "pair_discover: {r}"
    );
    let mut spake = [0u8; 33];
    rand::thread_rng().fill_bytes(&mut spake);
    for attempt in 1..=2 {
        let r = client.request(json!({ "t": "pair_join", "pairing": pairing })).await;
        ensure!(r["attempt"] == attempt, "pair_join: {r}");
        ensure!(host.event("pair_joined").await["attempt"] == attempt, "pair_joined event");
        client.request(json!({ "t": "pair_msg", "pairing": pairing, "blob": b64(&spake) })).await;
        ensure!(host.event("pair_msg").await["blob"] == b64(&spake).as_str(), "pair_msg relay");
        host.request(json!({ "t": "pair_msg", "pairing": pairing, "blob": b64(&spake[..32]) })).await;
        ensure!(client.event("pair_msg").await["blob"] == b64(&spake[..32]).as_str(), "pair_msg relay back");
        if attempt == 1 {
            // The host's SPAKE2 check "failed": the window stays open for another try.
            let r = host.request(json!({ "t": "pair_fail", "pairing": pairing })).await;
            ensure!(r["attempts_left"] == 2, "pair_fail: {r}");
            ensure!(client.event("pair_closed").await["reason"] == "failed", "attempt closed as failed");
        }
    }
    host.request(json!({ "t": "pair_close", "pairing": pairing })).await;
    ensure!(client.event("pair_closed").await["reason"] == "closed", "window closed after success");
    step("pairing window: discovered without the code, attempt 1 failed, attempt 2 relayed and closed", t);

    // The host's signed list authorises the client.
    let t = Instant::now();
    let seq = remote_rendezvous::hub::unix_now();
    let r = host.request(signed_list(&host.key, seq, &[&client.id])).await;
    expect(&r, "result", "paired_list")?;
    let r = client.request(json!({ "t": "presence", "hosts": [host.id] })).await;
    ensure!(r["hosts"][0]["state"] == "online", "presence: {r}");
    step("signed paired list stored; client sees the host online", t);

    let t = Instant::now();
    let r = client.request(json!({ "t": "connect", "host": host.id })).await;
    let session = r["session"].as_str().with_context(|| format!("connect: {r}"))?.to_string();
    ensure!(host.event("incoming").await["client"] == client.id.as_str(), "incoming event");
    step("connect by host ID; host notified", t);

    let mut blob = vec![0u8; 2048];
    rand::thread_rng().fill_bytes(&mut blob);
    let mut rtts = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        client.request(json!({ "t": "signal", "session": session, "blob": b64(&blob) })).await;
        let got = host.event("signal").await;
        ensure!(got["blob"] == b64(&blob).as_str(), "signal relayed byte for byte");
        host.request(json!({ "t": "signal", "session": session, "blob": got["blob"] })).await;
        ensure!(client.event("signal").await["blob"] == b64(&blob).as_str(), "signal relayed back");
        rtts.push(t.elapsed().as_millis());
    }
    rtts.sort();
    println!("ok    2 KiB opaque signal client -> host -> client, 5 rounds, median {} ms", rtts[2]);

    let t = Instant::now();
    let r = host.request(json!({ "t": "turn_credentials" })).await;
    ensure!(r["username"].as_str().is_some_and(|u| u.ends_with(&host.id)), "host turn: {r}");
    let r = client.request(json!({ "t": "turn_credentials" })).await;
    ensure!(r["username"].as_str().is_some_and(|u| u.ends_with(&client.id)), "client turn: {r}");
    write_secret(out, r.to_string().as_bytes())?;
    step(&format!("TURN credentials for host and client, ttl {} s, urls {}", r["ttl"], r["urls"]), t);

    // Revocation: a new list without the client ends its session and its access.
    let t = Instant::now();
    host.request(signed_list(&host.key, seq + 1, &[])).await;
    ensure!(client.event("session_closed").await["reason"] == "revoked", "session closed as revoked");
    expect(&client.event("revoked").await, "revoked", "revoked event")?;
    let r = client.request(json!({ "t": "turn_credentials" })).await;
    ensure!(r["code"] == "not_authorised", "revoked client denied TURN: {r}");
    step("revocation: session closed, revoked event, TURN refused", t);
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("keygen") if args.len() == 3 => {
            let dir = Path::new(&args[2]);
            std::fs::create_dir_all(dir)?;
            let host = new_key();
            write_secret(&dir.join("host.key"), b64(&host.to_bytes()).as_bytes())?;
            write_secret(&dir.join("client.key"), b64(&new_key().to_bytes()).as_bytes())?;
            println!("{}", id_of(&host));
            Ok(())
        }
        Some("run") if args.len() == 6 => {
            run(&args[2], Path::new(&args[3]), Path::new(&args[4]), Some(Path::new(&args[5]))).await
        }
        Some("sleep-check") if args.len() == 5 => {
            run(&args[2], Path::new(&args[3]), Path::new(&args[4]), None).await
        }
        _ => bail!(
            "usage: probe keygen <dir> | probe run <wss-url> <ca.pem> <dir> <out.json> \
             | probe sleep-check <wss-url> <ca.pem> <dir>"
        ),
    }
}
