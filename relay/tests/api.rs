// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The rendezvous API end to end over a real (plain) WebSocket: the happy path and the
//! boundaries that keep it safe.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use common::{Dev, id_of, new_key, signed_list, signed_list_for, temp_dir};
use ed25519_dalek::SigningKey;
use hmac::{Hmac, Mac};
use remote_rendezvous::proto::{Role, b64};
use remote_rendezvous::{Config, Enrol, Hub, server};
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_tungstenite::{MaybeTlsStream, connect_async};

const SECRET: &[u8] = b"test-secret-test-secret-test-secret";

struct Server {
    url: String,
    stop: Option<oneshot::Sender<()>>,
    state_dir: PathBuf,
    keep_state: bool,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        if !self.keep_state {
            let _ = std::fs::remove_dir_all(&self.state_dir);
            let _ = std::fs::remove_file(self.state_dir.with_extension("allow"));
        }
    }
}

async fn start(state_dir: PathBuf, allow: Option<&[&SigningKey]>) -> Server {
    start_with(state_dir, allow, Duration::from_secs(24 * 3600)).await
}

async fn start_with(state_dir: PathBuf, allow: Option<&[&SigningKey]>, asleep_ttl: Duration) -> Server {
    start_cfg(state_dir, allow, |c| c.asleep_ttl = asleep_ttl).await
}

async fn start_cfg(state_dir: PathBuf, allow: Option<&[&SigningKey]>, tweak: impl FnOnce(&mut Config)) -> Server {
    let hosts_allow = allow.map(|keys| {
        let path = state_dir.with_extension("allow");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: String = keys.iter().map(|k| id_of(k) + "\n").collect();
        std::fs::write(&path, format!("# test allowlist\n{text}")).unwrap();
        path
    });
    let mut cfg = Config::new("test.local", state_dir.clone(), SECRET.to_vec());
    cfg.enrol = if hosts_allow.is_some() { Enrol::Allowlist } else { Enrol::Open };
    cfg.hosts_allow = hosts_allow;
    cfg.turn_urls = vec!["turn:test.local:3478?transport=udp".into()];
    tweak(&mut cfg);
    let hub: Arc<Hub> = Hub::new(cfg).unwrap();
    let ticker = hub.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            ticker.tick();
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/remote", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    tokio::spawn(server::serve(listener, None, server::router(hub), async {
        let _ = stopped.await;
    }));
    Server { url, stop: Some(stop), state_dir, keep_state: false }
}

type Device = Dev<MaybeTlsStream<TcpStream>>;

async fn join(srv: &Server, key: SigningKey, role: Role) -> (Device, serde_json::Value) {
    let (ws, _) = connect_async(&srv.url).await.unwrap();
    Dev::auth(ws, key, role).await
}

async fn online(srv: &Server, key: SigningKey, role: Role) -> Device {
    let (dev, ok) = join(srv, key, role).await;
    assert_eq!(ok["t"], "auth_ok", "{ok}");
    dev
}

#[tokio::test]
async fn host_and_client_meet_signal_and_get_turn_credentials() {
    let srv = start(temp_dir("happy"), None).await;
    let (mut host, ok) = join(&srv, new_key(), Role::Host).await;
    assert_eq!(ok["registered"], true);
    let mut client = online(&srv, new_key(), Role::Client).await;

    let r = host.request(signed_list(&host.key, 1, &[&client.id])).await;
    assert_eq!(r["t"], "result", "{r}");
    assert_eq!(client.event("presence").await["state"], "online");
    let r = client.request(json!({ "t": "presence", "hosts": [host.id] })).await;
    assert_eq!(r["hosts"][0]["state"], "online");

    let r = client.request(json!({ "t": "connect", "host": host.id })).await;
    let session = r["session"].as_str().expect("session").to_string();
    let inc = host.event("incoming").await;
    assert_eq!((inc["session"].as_str(), inc["client"].as_str()), (Some(&*session), Some(&*client.id)));

    // Blobs pass through untouched, both ways.
    let offer = b64(b"sealed offer");
    let r = client.request(json!({ "t": "signal", "session": session, "blob": offer })).await;
    assert_eq!(r["t"], "result");
    assert_eq!(host.event("signal").await["blob"], offer);
    let answer = b64(b"sealed answer");
    host.request(json!({ "t": "signal", "session": session, "blob": answer })).await;
    assert_eq!(client.event("signal").await["blob"], answer);

    // A blob over 16 KiB is refused.
    let big = b64(&vec![0u8; 16 * 1024 + 1]);
    let r = client.request(json!({ "t": "signal", "session": session, "blob": big })).await;
    assert_eq!(r["code"], "too_large");

    // coturn REST credentials: "<expiry>:<device>", base64(HMAC-SHA1(secret, username)).
    for dev in [&mut host, &mut client] {
        let r = dev.request(json!({ "t": "turn_credentials" })).await;
        let user = r["username"].as_str().unwrap();
        let (expiry, who) = user.split_once(':').unwrap();
        assert_eq!(who, dev.id);
        let left = expiry.parse::<u64>().unwrap() - remote_rendezvous::hub::unix_now();
        assert!((300..=600).contains(&left), "{left}");
        let mut mac = Hmac::<sha1::Sha1>::new_from_slice(SECRET).unwrap();
        mac.update(user.as_bytes());
        let want = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        assert_eq!(r["credential"], want);
        assert!(r["ttl"].as_u64().unwrap().abs_diff(left) <= 1);
        // Asking again in the same step gives the same username, so one device can't spread over coturn's quota.
        let again = dev.request(json!({ "t": "turn_credentials" })).await;
        assert_eq!(again["username"], r["username"]);
    }

    client.request(json!({ "t": "close", "session": session })).await;
    assert_eq!(host.event("session_closed").await["reason"], "closed");
}

#[tokio::test]
async fn pairing_window_found_without_the_code_and_three_attempts_at_most() {
    let srv = start(temp_dir("pair"), None).await;
    let mut host = online(&srv, new_key(), Role::Host).await;
    let mut client = online(&srv, new_key(), Role::Client).await; // not paired with anyone yet

    let r = host.request(json!({ "t": "pair_open", "name": "Studio" })).await;
    let pairing = r["pairing"].as_str().expect("pairing").to_string();
    assert_eq!(r["expires_in"], 300);
    assert_eq!(host.request(json!({ "t": "pair_open", "name": "Studio" })).await["code"], "too_many");

    // The client finds the window with no code at all: host ID, name and expiry.
    let r = client.request(json!({ "t": "pair_discover" })).await;
    let found = &r["hosts"][0];
    assert_eq!(
        (found["pairing"].as_str(), found["host"].as_str(), found["name"].as_str()),
        (Some(&*pairing), Some(&*host.id), Some("Studio"))
    );

    // Attempt 1: SPAKE2 messages pass through untouched, then the host reports a failure.
    assert_eq!(client.request(json!({ "t": "pair_join", "pairing": pairing })).await["attempt"], 1);
    assert_eq!(host.event("pair_joined").await["attempt"], 1);
    let spake_a = b64(b"spake2 message A");
    client.request(json!({ "t": "pair_msg", "pairing": pairing, "blob": spake_a })).await;
    assert_eq!(host.event("pair_msg").await["blob"], spake_a);
    let spake_b = b64(b"spake2 message B");
    host.request(json!({ "t": "pair_msg", "pairing": pairing, "blob": spake_b })).await;
    assert_eq!(client.event("pair_msg").await["blob"], spake_b);

    // While an attempt runs the window is busy: not listed, not joinable.
    let mut other = online(&srv, new_key(), Role::Client).await;
    assert_eq!(other.request(json!({ "t": "pair_discover" })).await["hosts"], json!([]));
    assert_eq!(other.request(json!({ "t": "pair_join", "pairing": pairing })).await["code"], "not_found");

    let r = host.request(json!({ "t": "pair_fail", "pairing": pairing })).await;
    assert_eq!(r["attempts_left"], 2);
    assert_eq!(client.event("pair_closed").await["reason"], "failed");

    // Attempts 2 and 3 fail too, and the window closes after the third.
    for (attempt, left) in [(2, 1), (3, 0)] {
        assert_eq!(other.request(json!({ "t": "pair_join", "pairing": pairing })).await["attempt"], attempt);
        assert_eq!(host.request(json!({ "t": "pair_fail", "pairing": pairing })).await["attempts_left"], left);
    }
    assert_eq!(host.event("pair_closed").await["reason"], "attempts_used");
    assert_eq!(client.request(json!({ "t": "pair_discover" })).await["hosts"], json!([]));
}

#[tokio::test]
async fn a_pairing_window_is_one_use_and_discovery_is_rate_limited() {
    let srv = start(temp_dir("pair-once"), None).await;
    let mut host = online(&srv, new_key(), Role::Host).await;
    let mut client = online(&srv, new_key(), Role::Client).await;

    let r = host.request(json!({ "t": "pair_open", "name": "Desk" })).await;
    let pairing = r["pairing"].as_str().expect("pairing").to_string();
    client.request(json!({ "t": "pair_join", "pairing": pairing })).await;
    // SPAKE2 succeeded (inside the blobs); the host closes its window.
    host.request(json!({ "t": "pair_close", "pairing": pairing })).await;
    assert_eq!(client.event("pair_closed").await["reason"], "closed");

    // Discovery: 30 a minute per address.
    for _ in 0..30 {
        assert_eq!(client.request(json!({ "t": "pair_discover" })).await["hosts"], json!([]));
    }
    assert_eq!(client.request(json!({ "t": "pair_discover" })).await["code"], "rate_limited");
}

#[tokio::test]
async fn only_the_hosts_signed_list_authorises_and_revocation_is_immediate() {
    let srv = start(temp_dir("revoke"), None).await;
    let mut host = online(&srv, new_key(), Role::Host).await;
    let mut client = online(&srv, new_key(), Role::Client).await;
    let mut stranger = online(&srv, new_key(), Role::Client).await;

    host.request(signed_list(&host.key, 5, &[&client.id])).await;
    let r = client.request(json!({ "t": "connect", "host": host.id })).await;
    let session = r["session"].as_str().expect("session").to_string();

    // Not on the list: no session, no TURN, presence says so.
    let r = stranger.request(json!({ "t": "connect", "host": host.id })).await;
    assert_eq!(r["code"], "not_authorised");
    let r = stranger.request(json!({ "t": "turn_credentials" })).await;
    assert_eq!(r["code"], "not_authorised");
    let r = stranger.request(json!({ "t": "presence", "hosts": [host.id] })).await;
    assert_eq!(r["hosts"][0]["state"], "not_authorised");

    // A list signed by someone else, or an old one replayed, is refused.
    let mut forged = signed_list(&stranger.key, 6, &[&stranger.id]);
    forged["list"] = signed_list(&host.key, 6, &[&stranger.id])["list"].clone();
    assert_eq!(host.request(forged).await["code"], "bad_signature");
    assert_eq!(host.request(signed_list(&host.key, 5, &[&client.id])).await["code"], "stale_seq");
    // A newer list, but signed for another server (one a malicious server held on to), is refused too.
    let other = host.request(signed_list_for("other.example", &host.key, 9, &[&client.id, &stranger.id])).await;
    assert_eq!(other["code"], "bad_request");

    // Dropping the client from the list ends its session and its access at once.
    host.request(signed_list(&host.key, 6, &[])).await;
    let closed = client.event("session_closed").await;
    assert_eq!((closed["session"].as_str(), closed["reason"].as_str()), (Some(&*session), Some("revoked")));
    assert_eq!(client.event("revoked").await["host"], host.id);
    let r = client.request(json!({ "t": "connect", "host": host.id })).await;
    assert_eq!(r["code"], "not_authorised");
    let r = client.request(json!({ "t": "turn_credentials" })).await;
    assert_eq!(r["code"], "not_authorised");
}

#[tokio::test]
async fn auth_boundaries() {
    let enrolled = new_key();
    let both = new_key(); // enrolled as a host, but registers as a client first
    let srv = start(temp_dir("auth"), Some(&[&enrolled, &both])).await;

    let (_, r) = join(&srv, new_key(), Role::Host).await;
    assert_eq!(r["code"], "host_not_allowed");

    let (ws, _) = connect_async(&srv.url).await.unwrap();
    let (_, r) = Dev::auth_with(ws, new_key(), &new_key(), Role::Client).await;
    assert_eq!(r["code"], "auth_failed");

    let mut as_client = online(&srv, both.clone(), Role::Client).await;
    let (_, r) = join(&srv, both, Role::Host).await;
    assert_eq!(r["code"], "role_mismatch");
    let r = as_client.request(signed_list(&as_client.key, 1, &[])).await;
    assert_eq!(r["code"], "forbidden");

    // One connection per device: a second one replaces the first.
    let mut first = online(&srv, enrolled.clone(), Role::Host).await;
    let _second = online(&srv, enrolled, Role::Host).await;
    assert_eq!(first.event("replaced").await["t"], "replaced");
}

#[tokio::test]
async fn paired_lists_survive_a_restart() {
    let dir = temp_dir("restart");
    let host_key = new_key();
    let client_key = new_key();
    {
        let mut srv = start(dir.clone(), None).await;
        srv.keep_state = true;
        let mut host = online(&srv, host_key.clone(), Role::Host).await;
        host.request(signed_list(&host.key, 1, &[&id_of(&client_key)])).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let srv = start(dir, None).await;
    let mut client = online(&srv, client_key, Role::Client).await;
    let r = client.request(json!({ "t": "presence", "hosts": [id_of(&host_key)] })).await;
    assert_eq!(r["hosts"][0]["state"], "offline", "{r}");
    assert!(r["hosts"][0]["last_seen"].as_u64().is_some());
}

#[tokio::test]
async fn a_sleeping_host_shows_asleep_until_it_returns_or_a_day_passes() {
    let srv = start(temp_dir("sleep"), None).await;
    let host_key = new_key();
    let mut host = online(&srv, host_key.clone(), Role::Host).await;
    let mut client = online(&srv, new_key(), Role::Client).await;
    host.request(signed_list(&host.key, 1, &[&client.id])).await;
    assert_eq!(client.event("presence").await["state"], "online");
    assert_eq!(client.request(json!({ "t": "sleep" })).await["code"], "forbidden");

    // The sleep notice: the server answers, closes the connection, and clients see "asleep".
    let since = host.request(json!({ "t": "sleep" })).await["since"].as_u64().expect("since");
    assert!(host.event("anything").await.is_null(), "the server closes the connection");
    let ev = client.event("presence").await;
    assert_eq!((ev["state"].as_str(), ev["since"].as_u64()), (Some("asleep"), Some(since)));
    let r = client.request(json!({ "t": "presence", "hosts": [host.id] })).await;
    assert_eq!((r["hosts"][0]["state"].as_str(), r["hosts"][0]["since"].as_u64()), (Some("asleep"), Some(since)));

    // Coming back clears it.
    let _host = online(&srv, host_key, Role::Host).await;
    let ev = client.event("presence").await;
    assert_eq!((ev["state"].as_str(), ev["since"].as_u64()), (Some("online"), None));

    // An asleep host that never comes back counts as offline after the TTL (24 h; 1 s here).
    let srv = start_with(temp_dir("sleep-ttl"), None, Duration::from_secs(1)).await;
    let mut host = online(&srv, new_key(), Role::Host).await;
    let mut client = online(&srv, new_key(), Role::Client).await;
    host.request(signed_list(&host.key, 1, &[&client.id])).await;
    host.request(json!({ "t": "sleep" })).await;
    let ask = json!({ "t": "presence", "hosts": [host.id] });
    assert_eq!(client.request(ask.clone()).await["hosts"][0]["state"], "asleep");
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let r = client.request(ask).await;
    assert_eq!((r["hosts"][0]["state"].as_str(), r["hosts"][0]["since"].as_u64()), (Some("offline"), None));
}

#[tokio::test]
async fn an_unpaired_idle_client_is_forgotten_but_a_paired_one_stays() {
    let srv = start_cfg(temp_dir("idle"), None, |c| c.client_idle_ttl = Duration::from_secs(1)).await;
    let mut host = online(&srv, new_key(), Role::Host).await;
    let (stranger, paired) = (new_key(), new_key());
    let mut paired_dev = online(&srv, paired.clone(), Role::Client).await;
    host.request(signed_list(&host.key, 1, &[&paired_dev.id])).await;
    drop(online(&srv, stranger.clone(), Role::Client).await);
    drop(paired_dev);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let (_, ok) = join(&srv, stranger, Role::Client).await;
    assert_eq!(ok["registered"], true, "the idle stranger was forgotten and registers anew");
    let (_, ok) = join(&srv, paired, Role::Client).await;
    assert_eq!(ok["registered"], false, "a client some host lists is kept");
}
