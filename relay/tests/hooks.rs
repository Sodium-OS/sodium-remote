// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The optional hook points: `routes`, `service`, the `routes` request and `session_closed`.

mod common;

use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use common::{Dev, id_of, new_key, signed_list, temp_dir};
use remote_rendezvous::proto::{DeviceId, Role};
use remote_rendezvous::{Config, Enrol, Hooks, Hub, server};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[derive(Default)]
struct Probe {
    closed: Mutex<Vec<String>>,
}

struct TestHooks(Arc<Probe>);

impl Hooks for TestHooks {
    fn route_offer(
        &self,
        _session: &str,
        _client: &DeviceId,
        _host: &DeviceId,
        client_ip: IpAddr,
        host_ip: IpAddr,
        wish: Option<u16>,
    ) -> Option<(Value, Value)> {
        Some((json!({ "ip": client_ip.to_string(), "wish": wish }), json!({ "ip": host_ip.to_string() })))
    }
    fn route_refresh(&self, _session: &str, _device: &DeviceId) -> Option<Value> {
        Some(json!("fresh"))
    }
    fn auth_extra(&self, _device: &DeviceId, role: Role, _account: Option<&str>) -> Option<Value> {
        Some(json!({ "role": role.as_str() }))
    }
    fn session_closed(&self, session: &str) {
        self.0.closed.lock().unwrap().push(session.to_string());
    }
}

#[tokio::test]
async fn hooks_add_routes_service_and_close_notice() {
    let probe = Arc::new(Probe::default());
    let mut cfg = Config::new("test.local", temp_dir("hooks"), b"test-secret-test-secret-test-secret".to_vec());
    cfg.enrol = Enrol::Open;
    cfg.trusted_proxies = vec!["127.0.0.1".parse().unwrap()];
    cfg.hooks = Arc::new(TestHooks(probe.clone()));
    let state = cfg.state_dir.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/remote", listener.local_addr().unwrap());
    tokio::spawn(server::serve(listener, None, server::router(Hub::new(cfg).unwrap()), std::future::pending()));

    let (hk, ck, sk) = (new_key(), new_key(), new_key());
    let (ws, _) = connect_async(&url).await.unwrap();
    let (mut host, ok) = Dev::auth(ws, hk.clone(), Role::Host).await;
    assert_eq!(ok["service"], json!({ "role": "host" }));
    host.request(signed_list(&hk, 1, &[&id_of(&ck), &id_of(&sk)])).await;

    // The client arrives through a trusted proxy, which names its address.
    let mut req = url.as_str().into_client_request().unwrap();
    req.headers_mut().insert("x-real-ip", "203.0.113.7".parse().unwrap());
    let (ws, _) = connect_async(req).await.unwrap();
    let (mut client, _) = Dev::auth(ws, ck, Role::Client).await;

    let r = client.request(json!({ "t": "connect", "host": host.id, "node": 7 })).await;
    assert_eq!(r["routes"], json!({ "ip": "203.0.113.7", "wish": 7 }));
    let session = r["session"].as_str().unwrap().to_string();
    let incoming = host.event("incoming").await;
    assert_eq!(incoming["routes"], json!({ "ip": "127.0.0.1" }));

    assert_eq!(client.request(json!({ "t": "routes", "session": session })).await["routes"], "fresh");
    assert_eq!(host.request(json!({ "t": "routes", "session": session })).await["routes"], "fresh");

    // Someone outside the session gets the same error as for `signal`.
    let (ws, _) = connect_async(&url).await.unwrap();
    let (mut other, _) = Dev::auth(ws, sk, Role::Client).await;
    let e = other.request(json!({ "t": "routes", "session": session })).await;
    assert_eq!((e["t"].as_str(), e["code"].as_str()), (Some("error"), Some("not_found")));

    assert!(probe.closed.lock().unwrap().is_empty());
    client.request(json!({ "t": "close", "session": session })).await;
    assert_eq!(*probe.closed.lock().unwrap(), vec![session]);
    let _ = std::fs::remove_dir_all(state);
}
