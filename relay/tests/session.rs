// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! A whole session through a locally started server: TLS from a test CA the devices
//! pin (`ca_file`), a host enrolled by token, pairing through the rendezvous (the
//! host's LAN listener is off), then a connection with the Hello/Welcome exchange.
//! The certificates come from the `openssl` command; the test skips without it.

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use common::temp_dir;
use remote_core::connect::{Client, ClientConfig, ClientEvent, Host, HostConfig, HostEvent};
use remote_core::keys::MemoryStore;
use remote_core::msg::{self, *};
use remote_core::keys::DeviceKey;
use remote_core::rendezvous::{DeviceRole, RdvError, Rendezvous, RendezvousConfig};
use remote_rendezvous::{Config, Enrol, Hub, hub::unix_now, server, tokens};
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedReceiver;

const LO: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// A test CA and a leaf for `localhost` signed by it, in `dir`.
fn make_certs(dir: &Path) -> bool {
    let run = |args: &[&str]| Command::new("openssl").args(args).current_dir(dir).output().is_ok_and(|o| o.status.success());
    std::fs::write(dir.join("leaf.ext"), "subjectAltName=DNS:localhost\nbasicConstraints=CA:FALSE\n").unwrap();
    run(&["req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-keyout", "ca.key", "-out", "ca.pem", "-days", "2", "-subj", "/CN=Sodium Remote test CA"])
        && run(&["req", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-keyout", "leaf.key", "-out", "leaf.csr", "-subj", "/CN=localhost"])
        && run(&["x509", "-req", "-in", "leaf.csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial", "-out", "leaf.pem", "-days", "2", "-extfile", "leaf.ext"])
}

async fn next<T>(rx: &mut UnboundedReceiver<T>, mut f: impl FnMut(&T) -> bool) -> T {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let e = rx.recv().await.expect("events ended");
            if f(&e) {
                return e;
            }
        }
    })
    .await
    .expect("timed out waiting for an event")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_through_a_local_server_with_a_pinned_test_ca() {
    let dir = temp_dir("session");
    std::fs::create_dir_all(&dir).unwrap();
    if !make_certs(&dir) {
        eprintln!("no openssl: skipped");
        return;
    }

    // The server: TLS with the leaf, token enrolment (the default).
    let mut cfg = Config::new("localhost", dir.join("state"), b"test-secret-test-secret-test-secret".to_vec());
    cfg.enrol = Enrol::Token;
    cfg.turn_urls = vec!["stun:127.0.0.1:3478".into()];
    let tokens_dir = cfg.tokens_dir();
    let tls = server::load_tls(&dir.join("leaf.pem"), &dir.join("leaf.key")).unwrap();
    let listener = TcpListener::bind((LO, 0)).await.unwrap();
    let url = format!("wss://localhost:{}/remote", listener.local_addr().unwrap().port());
    tokio::spawn(server::serve(listener, Some(tls), server::router(Hub::new(cfg).unwrap()), std::future::pending()));
    let (token, _) = tokens::mint(&tokens_dir, Some(1), Some(unix_now() + 600), None, unix_now()).unwrap();

    let ca = std::fs::read(dir.join("ca.pem")).unwrap();
    let rdv = |token: Option<String>| RendezvousConfig { url: url.clone(), ca_pem: Some(ca.clone()), token };

    // The host: rendezvous only (no LAN listener, no mDNS), enrolled by the token.
    let mut hc = HostConfig::new(Arc::new(MemoryStore::default()), "test-host");
    hc.rendezvous = Some(rdv(Some(token)));
    hc.lan = false;
    hc.mdns = false;
    hc.bind = Some(vec![LO]);
    let (host, mut hev) = Host::start(hc).await.unwrap();
    let HostEvent::Rendezvous { online, error } = next(&mut hev, |e| matches!(e, HostEvent::Rendezvous { .. })).await else {
        unreachable!()
    };
    assert!(online, "the host didn't get on the server: {error:?}");

    let mut cc = ClientConfig::new(Arc::new(MemoryStore::default()), "test-client");
    cc.rendezvous = Some(rdv(None));
    cc.mdns = false;
    cc.bind = Some(vec![LO]);
    let (client, mut cev) = Client::start(cc).await.unwrap();
    assert!(client.wait_rendezvous(Duration::from_secs(10)).await, "the client didn't get on the server");

    // Pairing through the rendezvous: the window is found without the code.
    let w = host.open_pairing().await.unwrap();
    let mut targets = vec![];
    for _ in 0..20 {
        targets = client.discover_pairing(Duration::from_millis(500)).await;
        if !targets.is_empty() {
            break;
        }
    }
    let target = targets.into_iter().find(|t| t.host == host.id()).expect("the pairing window through the server");
    assert!(target.lan.is_none() && target.rendezvous.is_some());
    client.pair(&target, &w.code).await.unwrap();
    next(&mut hev, |e| matches!(e, HostEvent::Paired(_))).await;

    // A session: signalling through the server, ICE on loopback, Hello and Welcome.
    let hello = Hello {
        version: msg::VERSION,
        min_version: msg::MIN_VERSION,
        name: String::new(),
        display: ClientDisplay { width: 1920, height: 1080, pixels_per_point: 1.0, refresh_mhz: 60000, fullscreen: false, display_width: 1920, display_height: 1080 },
        decode: DecodeSupport { hevc: true, h264: true, hevc_444: false, av1: false, max_width: 0, max_height: 0 },
        settings: StreamSettings::default(),
        client_os: Os::Linux,
        client_os_flavor: String::new(),
    };
    client.connect(host.id(), hello, false).await.unwrap();
    let HostEvent::ClientConnected { name, .. } = next(&mut hev, |e| matches!(e, HostEvent::ClientConnected { .. })).await else {
        unreachable!()
    };
    assert_eq!(name, "test-client");
    host.send(&Msg::Welcome(Welcome {
        version: msg::VERSION,
        name: "test-host".into(),
        format: StreamFormat { width: 1920, height: 1080, scale: 1.0, refresh_mhz: 60000, codec: Codec::Hevc420 },
        scales: vec![],
        home_monitor: None,
        locked: false,
        took_over_from: None,
        resumed: false,
        host_os: Os::Linux,
        display_mode: DisplayMode::Physical,
        live_resize: false,
        displays: vec![],
        current_display: 0,
        view_only: false,
        host_os_flavor: String::new(),
    }));
    next(&mut cev, |e| matches!(e, ClientEvent::Control(Msg::Welcome(_)))).await;
    client.disconnect();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A device signs the name of the server it connected to (the URL's host, checked over TLS), not the name
/// the server's challenge carries: a challenge passed on from another server gets a signature that is
/// useless there. A server accepts its aliases too.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_signs_the_server_it_connected_to() {
    let dir = temp_dir("binding");
    std::fs::create_dir_all(&dir).unwrap();
    if !make_certs(&dir) {
        eprintln!("no openssl: skipped");
        return;
    }
    let ca = std::fs::read(dir.join("ca.pem")).unwrap();
    let tls = server::load_tls(&dir.join("leaf.pem"), &dir.join("leaf.key")).unwrap();
    let start = |aliases: Vec<String>, tag: &str| {
        // The server calls itself another name than the one the device reaches it by.
        let mut cfg = Config::new("relay-b.test", dir.join(tag), b"test-secret-test-secret-test-secret".to_vec());
        cfg.server_aliases = aliases;
        let tls = tls.clone();
        async move {
            let listener = TcpListener::bind((LO, 0)).await.unwrap();
            let url = format!("wss://localhost:{}/remote", listener.local_addr().unwrap().port());
            tokio::spawn(server::serve(listener, Some(tls), server::router(Hub::new(cfg).unwrap()), std::future::pending()));
            url
        }
    };
    let key = DeviceKey::generate();

    let url = start(vec![], "plain").await;
    let r = Rendezvous::connect(&RendezvousConfig { url, ca_pem: Some(ca.clone()), token: None }, &key, DeviceRole::Client).await;
    assert!(matches!(r, Err(RdvError::Server { ref code, .. }) if code == "auth_failed"), "{:?}", r.err());

    let url = start(vec!["localhost".into()], "alias").await;
    let r = Rendezvous::connect(&RendezvousConfig { url, ca_pem: Some(ca), token: None }, &key, DeviceRole::Client).await;
    assert!(r.is_ok(), "{:?}", r.err());
    let _ = std::fs::remove_dir_all(&dir);
}
