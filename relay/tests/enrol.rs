// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Enrolment (token, open, allowlist), redirects, region-labelled relays and the
//! source link: the release additions to the rendezvous API.

mod common;

use common::{Dev, new_key, temp_dir};
use remote_rendezvous::proto::Role;
use remote_rendezvous::{Config, Enrol, Hub, Notice, RelayRegion, hub::unix_now, server, tokens};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::connect_async;

struct Server {
    url: String,
    _stop: oneshot::Sender<()>,
    cfg: Config,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.cfg.state_dir);
    }
}

async fn start(tag: &str, edit: impl FnOnce(&mut Config)) -> Server {
    let mut cfg = Config::new("test.local", temp_dir(tag), b"test-secret-test-secret-test-secret".to_vec());
    cfg.turn_urls = vec!["turn:test.local:3478?transport=udp".into()];
    edit(&mut cfg);
    let hub = Hub::new(cfg.clone()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/remote", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel();
    tokio::spawn(server::serve(listener, None, server::router(hub), async {
        let _ = stopped.await;
    }));
    Server { url, _stop: stop, cfg }
}

async fn auth(srv: &Server, key: &ed25519_dalek::SigningKey, role: Role, token: Option<&str>) -> Value {
    let (ws, _) = connect_async(&srv.url).await.unwrap();
    Dev::auth_token(ws, key.clone(), role, token).await.1
}

#[tokio::test]
async fn token_enrolment_accepts_a_valid_token_once_and_refuses_the_rest() {
    let srv = start("token", |c| c.enrol = Enrol::Token).await;
    let dir = srv.cfg.tokens_dir();
    let now = unix_now();
    let (one_use, _) = tokens::mint(&dir, Some(1), Some(now + 3600), Some("acct-1".into()), now).unwrap();
    let (stale, _) = tokens::mint(&dir, None, Some(now - 1), None, now - 10).unwrap();

    // A host without a token, or with a wrong, expired or spent one, isn't enrolled.
    let host = new_key();
    let r = auth(&srv, &host, Role::Host, None).await;
    assert_eq!((r["code"].as_str(), r["message"].as_str()), (Some("host_not_allowed"), Some("this server needs an enrolment token")));
    assert_eq!(auth(&srv, &host, Role::Host, Some("not-a-token-at-all-xxxx")).await["code"], "token_invalid");
    assert_eq!(auth(&srv, &host, Role::Host, Some(&stale)).await["code"], "token_expired");

    // The token registers it and binds the account.
    let ok = auth(&srv, &host, Role::Host, Some(&one_use)).await;
    assert_eq!((ok["t"].as_str(), ok["registered"].as_bool(), ok["account"].as_str()), (Some("auth_ok"), Some(true), Some("acct-1")));
    // Back again with the same (now spent) token in its server.toml: fine, it is registered.
    let ok = auth(&srv, &host, Role::Host, Some(&one_use)).await;
    assert_eq!((ok["t"].as_str(), ok["registered"].as_bool(), ok["account"].as_str()), (Some("auth_ok"), Some(false), Some("acct-1")));
    // Another host can't reuse it.
    assert_eq!(auth(&srv, &new_key(), Role::Host, Some(&one_use)).await["code"], "token_expired");

    // A second server: registrations are rate-limited per address (5 an hour), and
    // failed tokens count, so guessing is slow.
    let srv = start("token2", |c| c.enrol = Enrol::Token).await;
    let dir = srv.cfg.tokens_dir();

    // Clients need none, but a bad one is refused rather than ignored.
    assert_eq!(auth(&srv, &new_key(), Role::Client, None).await["t"], "auth_ok");
    assert_eq!(auth(&srv, &new_key(), Role::Client, Some("not-a-token-at-all-xxxx")).await["code"], "token_invalid");

    // Revoked tokens are unknown.
    let (t, rec) = tokens::mint(&dir, None, None, None, now).unwrap();
    tokens::revoke(&dir, rec.id()).unwrap();
    assert_eq!(auth(&srv, &new_key(), Role::Host, Some(&t)).await["code"], "token_invalid");
}

#[tokio::test]
async fn open_enrolment_lets_any_host_register() {
    let srv = start("open", |c| c.enrol = Enrol::Open).await;
    assert_eq!(auth(&srv, &new_key(), Role::Host, None).await["t"], "auth_ok");
}

#[tokio::test]
async fn new_registrations_are_redirected_and_registered_devices_stay() {
    let srv = start("redirect", |c| {
        c.enrol = Enrol::Open;
        c.notice = Some(Notice { code: "maintenance".into(), text: "Down at 02:00 UTC".into(), url: None });
    })
    .await;
    let known = new_key();
    let ok = auth(&srv, &known, Role::Client, None).await;
    assert_eq!(ok["notice"], json!({"code": "maintenance", "text": "Down at 02:00 UTC"}));

    // The operator now sends new devices to another node (same state: a restart with --redirect).
    let state = srv.cfg.state_dir.clone();
    let srv2 = start("redirect2", |c| {
        c.enrol = Enrol::Open;
        c.state_dir = state;
        c.redirect = Some("wss://node2.relay.example.com/remote".into());
    })
    .await;
    let r = auth(&srv2, &new_key(), Role::Client, None).await;
    assert_eq!(r, json!({"t": "redirect", "url": "wss://node2.relay.example.com/remote"}));
    assert_eq!(auth(&srv2, &known, Role::Client, None).await["t"], "auth_ok", "registered devices stay");
}

#[tokio::test]
async fn turn_credentials_carry_the_region_list_and_the_challenge_the_source() {
    let srv = start("relays", |c| {
        c.enrol = Enrol::Open;
        c.turn_urls = vec![];
        c.relays = vec![
            RelayRegion { region: "sg".into(), urls: vec!["turn:sg.relay.example.com:3478?transport=udp".into()] },
            RelayRegion { region: "us".into(), urls: vec!["turn:us.relay.example.com:3478?transport=udp".into()] },
        ];
    })
    .await;
    let (ws, _) = connect_async(&srv.url).await.unwrap();
    let (mut host, ok) = Dev::auth_token(ws, new_key(), Role::Host, None).await;
    assert_eq!(ok["t"], "auth_ok");
    assert_eq!(host.challenge["source"], remote_core::brand::SOURCE_URL);
    assert_eq!(remote_rendezvous::SOURCE_URL, remote_core::brand::SOURCE_URL);
    let r = host.request(json!({ "t": "turn_credentials" })).await;
    assert_eq!(r["urls"], json!(["turn:sg.relay.example.com:3478?transport=udp"]), "older clients get the first region");
    assert_eq!(r["relays"][1], json!({"region": "us", "urls": ["turn:us.relay.example.com:3478?transport=udp"]}));
}
