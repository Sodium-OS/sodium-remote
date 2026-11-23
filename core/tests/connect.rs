// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The session API on loopback, LAN only (no rendezvous, no mDNS): pairing over
//! the LAN listener, connecting, the Hello/Welcome exchange, input, and the
//! takeover rule.

use remote_core::connect::{Client, ClientConfig, ClientEvent, ConnectError, Host, HostConfig, HostEvent, PairingTarget};
use remote_core::keys::MemoryStore;
use remote_core::msg::{self, *};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;

const LO: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

async fn host() -> (Host, UnboundedReceiver<HostEvent>) {
    let mut cfg = HostConfig::new(Arc::new(MemoryStore::default()), "test-host");
    cfg.rendezvous = None;
    cfg.mdns = false;
    cfg.lan_bind = SocketAddr::new(LO, 0);
    cfg.bind = Some(vec![LO]);
    Host::start(cfg).await.unwrap()
}

async fn client(name: &str, host: &Host) -> (Client, UnboundedReceiver<ClientEvent>) {
    let mut cfg = ClientConfig::new(Arc::new(MemoryStore::default()), name);
    cfg.rendezvous = None;
    cfg.mdns = false;
    cfg.bind = Some(vec![LO]);
    cfg.lan_hints = vec![(host.id(), SocketAddr::new(LO, host.lan_port().unwrap()))];
    Client::start(cfg).await.unwrap()
}

fn hello() -> Hello {
    Hello {
        version: msg::VERSION,
        min_version: msg::MIN_VERSION,
        name: String::new(),
        display: ClientDisplay { width: 2560, height: 1664, pixels_per_point: 1.741, refresh_mhz: 60000, fullscreen: true, display_width: 2560, display_height: 1664 },
        decode: DecodeSupport { hevc: true, h264: true, hevc_444: true, av1: true, max_width: 0, max_height: 0 },
        settings: StreamSettings::default(),
        client_os: msg::Os::Linux,
        client_os_flavor: String::new(),
    }
}

async fn next<T>(rx: &mut UnboundedReceiver<T>, mut f: impl FnMut(&T) -> bool) -> T {
    tokio::time::timeout(Duration::from_secs(10), async {
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

async fn pair(host: &Host, hev: &mut UnboundedReceiver<HostEvent>, client: &Client) {
    let w = host.open_pairing().await.unwrap();
    let target = PairingTarget {
        host: host.id(),
        name: "test-host".into(),
        lan: Some((vec![LO], host.lan_port().unwrap())),
        rendezvous: None,
    };
    let dev = client.pair(&target, &w.code).await.unwrap();
    assert_eq!(dev.id, host.id());
    next(hev, |e| matches!(e, HostEvent::Paired(_))).await;
}

/// Answer Hello with a Welcome, as remoted would.
fn welcome(host: &Host, took_over_from: Option<String>, resumed: bool) {
    host.send(&Msg::Welcome(Welcome {
        version: msg::VERSION,
        name: "test-host".into(),
        format: StreamFormat { width: 2560, height: 1664, scale: 1.6, refresh_mhz: 60000, codec: Codec::Hevc444 },
        scales: vec![1.333, 1.6, 2.0],
        home_monitor: Some(HomeMonitorMode::Mirror),
        locked: false,
        took_over_from,
        resumed,
        host_os: msg::Os::Linux,
        display_mode: msg::DisplayMode::Virtual,
        live_resize: true,
        displays: vec![],
        current_display: 0,
        view_only: false,
        host_os_flavor: String::new(),
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_connect_and_talk() {
    let (host, mut hev) = host().await;
    let (client, mut cev) = client("MacBook Air", &host).await;
    pair(&host, &mut hev, &client).await;
    assert_eq!(host.paired().len(), 1);

    // A rename in System Settings reaches the next connection's Hello.
    client.set_name("MacBook Air (renamed)");
    client.connect(host.id(), hello(), false).await.unwrap();
    let HostEvent::ClientConnected { name, hello: h, took_over_from, .. } =
        next(&mut hev, |e| matches!(e, HostEvent::ClientConnected { .. })).await
    else {
        unreachable!()
    };
    assert_eq!((name.as_str(), h.display.width, took_over_from), ("MacBook Air (renamed)", 2560, None));
    welcome(&host, None, false);
    next(&mut cev, |e| matches!(e, ClientEvent::Control(Msg::Welcome(_)))).await;

    client.send_input(&InputEvent::ImeCommit { text: "你好".into() });
    let HostEvent::Input { event: i, .. } = next(&mut hev, |e| matches!(e, HostEvent::Input { .. })).await else { unreachable!() };
    assert_eq!(i, InputEvent::ImeCommit { text: "你好".into() });

    client.disconnect();
    let HostEvent::ClientDisconnected { reason, .. } =
        next(&mut hev, |e| matches!(e, HostEvent::ClientDisconnected { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(reason, DisconnectReason::User);
}

/// One sealed offer to the host's LAN listener, as the client would send it; the host's first reply, opened.
async fn send_offer(host: &Host, client: &Client, offer: &Offer) -> Option<Signal> {
    use remote_core::keys;
    use remote_core::mdns::{self, LanFrame};
    let mut s = tokio::net::TcpStream::connect((LO, host.lan_port().unwrap())).await.unwrap();
    let blob = keys::seal(client.key(), &host.id(), b"lan", &msg::encode(&Signal::Offer(offer.clone()))).unwrap();
    mdns::write_frame(&mut s, &LanFrame::SignalHello { client: client.id().0 }).await.unwrap();
    mdns::write_frame(&mut s, &LanFrame::Signal(blob)).await.unwrap();
    match tokio::time::timeout(Duration::from_secs(5), mdns::read_frame(&mut s)).await.ok()?? {
        LanFrame::Signal(b) => Signal::decode(&keys::open(client.key(), &host.id(), b"lan", &b)?),
        _ => None,
    }
}

/// A recorded offer played again is dropped unanswered, and even a fresh offer leaves the live session
/// alone until its connection proves the client: neither ends it.
#[tokio::test(flavor = "multi_thread")]
async fn a_replayed_offer_is_refused_and_the_live_session_survives() {
    let (host, mut hev) = host().await;
    let (client, mut cev) = client("MacBook Air", &host).await;
    pair(&host, &mut hev, &client).await;
    client.connect(host.id(), hello(), false).await.unwrap();
    next(&mut hev, |e| matches!(e, HostEvent::ClientConnected { .. })).await;
    welcome(&host, None, false);
    next(&mut cev, |e| matches!(e, ClientEvent::Control(Msg::Welcome(_)))).await;

    let mut offer = Offer {
        version: msg::VERSION,
        name: "MacBook Air".into(),
        takeover: false,
        ice_ufrag: "u".into(),
        ice_pass: "p".into(),
        candidates: vec![],
        min_version: msg::MIN_VERSION,
        sent_us: 1,
    };
    assert_eq!(send_offer(&host, &client, &offer).await, None, "an offer older than the last one isn't answered");
    offer.sent_us = Offer::now_us();
    assert!(matches!(send_offer(&host, &client, &offer).await, Some(Signal::Answer(_))), "a fresh one is");
    assert_eq!(send_offer(&host, &client, &offer).await, None, "the same one again isn't");

    // The session is still live: no disconnect on either side, and messages still flow.
    host.send(&Msg::SecureScreen(true));
    next(&mut cev, |e| matches!(e, ClientEvent::Control(Msg::SecureScreen(true)))).await;
    while let Ok(e) = hev.try_recv() {
        assert!(!matches!(e, HostEvent::ClientDisconnected { .. } | HostEvent::ClientLost { .. }), "{e:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_session_is_busy_until_the_user_takes_over() {
    let (host, mut hev) = host().await;
    let (one, mut one_ev) = client("one", &host).await;
    let (two, _two_ev) = client("two", &host).await;
    pair(&host, &mut hev, &one).await;
    pair(&host, &mut hev, &two).await;

    one.connect(host.id(), hello(), false).await.unwrap();
    next(&mut hev, |e| matches!(e, HostEvent::ClientConnected { .. })).await;

    assert_eq!(two.connect(host.id(), hello(), false).await, Err(ConnectError::Busy { by: "one".into() }));
    two.connect(host.id(), hello(), true).await.unwrap();
    let HostEvent::ClientConnected { name, took_over_from, .. } =
        next(&mut hev, |e| matches!(e, HostEvent::ClientConnected { .. })).await
    else {
        unreachable!()
    };
    assert_eq!((name.as_str(), took_over_from.as_deref()), ("two", Some("one")));
    let ClientEvent::Disconnected { reason } = next(&mut one_ev, |e| matches!(e, ClientEvent::Disconnected { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(reason, DisconnectReason::TakenOver { by: "two".into() });
}

/// Settings › Remote's Name and "Visible on this network" apply live: the announcement
/// appears, is replaced under the new name, and goes away, with no restart.
#[tokio::test(flavor = "multi_thread")]
async fn the_mdns_announcement_follows_the_name_and_the_switch() {
    use remote_core::mdns::Browser;
    let (host, _ev) = host().await;
    let browser = Browser::start().expect("mDNS browse");
    let id = host.id();
    // Some(name): one announcement of this host, under that name; None: none at all.
    let seen = |name: Option<&'static str>| {
        let browser = &browser;
        async move {
            for _ in 0..80 {
                let hosts: Vec<_> = browser.hosts().into_iter().filter(|h| h.id == id).collect();
                let ok = match name {
                    Some(n) => hosts.len() == 1 && hosts[0].name == n,
                    None => hosts.is_empty(),
                };
                if ok {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            false
        }
    };
    assert!(seen(None).await, "not announced while mDNS is off");
    host.set_mdns(true);
    host.set_name("Desk A");
    assert!(seen(Some("Desk A")).await, "announced under its name");
    host.set_name("Desk B");
    assert!(seen(Some("Desk B")).await, "re-announced under the new name, the old one withdrawn");
    host.set_mdns(false);
    assert!(seen(None).await, "withdrawn");
}

/// Connect under `name`, wait for the host to take the Hello, and hang up.
async fn connect_as(host: &Host, hev: &mut UnboundedReceiver<HostEvent>, client: &Client, name: &str) {
    client.set_name(name);
    client.connect(host.id(), hello(), false).await.unwrap();
    next(hev, |e| matches!(e, HostEvent::ClientConnected { .. })).await;
    client.disconnect();
    next(hev, |e| matches!(e, HostEvent::ClientDisconnected { .. })).await;
}

/// The host's list follows the device's own name (a Mac renamed in System Settings) unless
/// the desk named it; a desk rename to "" follows the device again.
#[tokio::test(flavor = "multi_thread")]
async fn the_paired_name_follows_the_device_unless_the_desk_named_it() {
    let (host, mut hev) = host().await;
    let (client, _cev) = client("MacBook Air", &host).await;
    pair(&host, &mut hev, &client).await;
    let id = host.paired()[0].id.clone();
    let name = || host.paired()[0].name.clone();

    connect_as(&host, &mut hev, &client, "Nao's Air").await;
    assert_eq!(name(), "Nao's Air", "the device's new name is taken");

    assert!(host.rename_paired(&id, "Laptop"));
    connect_as(&host, &mut hev, &client, "Air 2").await;
    assert_eq!(name(), "Laptop", "the desk's name wins");

    assert!(host.rename_paired(&id, ""));
    assert_eq!(name(), "Laptop", "kept until the device connects");
    connect_as(&host, &mut hev, &client, "Air 2").await;
    assert_eq!(name(), "Air 2", "the device's own name again");

    connect_as(&host, &mut hev, &client, &"x".repeat(65)).await;
    assert_eq!(name(), "Air 2", "a name over 64 bytes is ignored, never refused");
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_and_connect_by_a_bare_address() {
    let (host, mut hev) = host().await;
    let mut cfg = ClientConfig::new(Arc::new(MemoryStore::default()), "MacBook Air");
    cfg.rendezvous = None;
    cfg.mdns = false;
    cfg.bind = Some(vec![LO]);
    let (client, _cev) = Client::start(cfg).await.unwrap();
    let addr = SocketAddr::new(LO, host.lan_port().unwrap());
    let w = host.open_pairing().await.unwrap();

    // A wrong code pairs nothing; the right one pins the ID the exchange carried.
    let wrong = if w.code == "000000" { "000001" } else { "000000" };
    assert!(client.pair_by_address(addr, wrong).await.is_err());
    assert!(client.paired().is_empty());
    let dev = client.pair_by_address(addr, &w.code).await.unwrap();
    assert_eq!(dev.id, host.id());
    next(&mut hev, |e| matches!(e, HostEvent::Paired(_))).await;

    // Connecting by the same address uses the stored ID.
    assert_eq!(client.host_at(addr), Some(host.id()));
    client.connect_by_address(addr, hello(), false).await.unwrap();
    next(&mut hev, |e| matches!(e, HostEvent::ClientConnected { .. })).await;
}

/// Three wrong codes close the window for good: a fourth try, even with the right code, is
/// refused, and no new window opens until the host opens one.
#[tokio::test(flavor = "multi_thread")]
async fn three_wrong_codes_close_pairing_until_the_host_opens_it_again() {
    let (host, mut hev) = host().await;
    let mut cfg = ClientConfig::new(Arc::new(MemoryStore::default()), "MacBook Air");
    cfg.rendezvous = None;
    cfg.mdns = false;
    cfg.bind = Some(vec![LO]);
    let (client, _cev) = Client::start(cfg).await.unwrap();
    let addr = SocketAddr::new(LO, host.lan_port().unwrap());
    let w = host.open_pairing().await.unwrap();
    let wrong = if w.code == "000000" { "000001" } else { "000000" };
    for _ in 0..3 {
        assert!(client.pair_by_address(addr, wrong).await.is_err());
    }
    let HostEvent::PairingClosed { reason } = next(&mut hev, |e| matches!(e, HostEvent::PairingClosed { .. })).await else {
        unreachable!()
    };
    assert_eq!(reason, "attempts_used");
    assert!(client.pair_by_address(addr, &w.code).await.is_err(), "the fourth try is refused");
    assert!(client.paired().is_empty() && host.paired().is_empty());
}

/// `last_connected` is stamped by core in the client's own `paired.json` when `Welcome` arrives.
#[tokio::test(flavor = "multi_thread")]
async fn the_client_stamps_last_connected_on_welcome() {
    let (host, mut hev) = host().await;
    let (client, mut cev) = client("MacBook Air", &host).await;
    pair(&host, &mut hev, &client).await;
    assert_eq!(client.paired()[0].last_connected, None);

    client.connect(host.id(), hello(), false).await.unwrap();
    next(&mut hev, |e| matches!(e, HostEvent::ClientConnected { .. })).await;
    assert_eq!(client.paired()[0].last_connected, None, "an attempt isn't a connection");
    welcome(&host, None, false);
    next(&mut cev, |e| matches!(e, ClientEvent::Control(Msg::Welcome(_)))).await;
    assert!(client.paired()[0].last_connected.is_some());
}

/// Switching the host off and on again keeps its LAN port (the listener is released with the host).
#[tokio::test(flavor = "multi_thread")]
async fn the_lan_port_survives_switching_off_and_on() {
    let port = std::net::TcpListener::bind((LO, 0)).unwrap().local_addr().unwrap().port();
    for _ in 0..2 {
        let mut cfg = HostConfig::new(Arc::new(MemoryStore::default()), "test-host");
        cfg.rendezvous = None;
        cfg.mdns = false;
        cfg.lan_bind = SocketAddr::new(LO, port);
        cfg.bind = Some(vec![LO]);
        let (host, _hev) = Host::start(cfg).await.unwrap();
        assert_eq!(host.lan_port(), Some(port));
        host.shutdown().await;
    }
}
