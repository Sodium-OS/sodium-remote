// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Drives `remote_core::connect` end to end: a test host and a test client that
//! pair (rendezvous or LAN/mDNS), connect, and exchange control messages,
//! input, cursor and dummy video. Used for live checks against a rendezvous server
//! and the network-namespace mDNS check.
//!
//!   connect_demo keygen  --store DIR                  print the device ID (creates device.key)
//!   connect_demo host    --store DIR [--name N] [--pair --code-file F] [--no-rdv] [--no-lan]
//!                        [--bind IP]... [--secs N] [--unregister]
//!   connect_demo client  --store DIR [--name N] [--code-file F] [--connect] [--takeover]
//!                        [--no-rdv] [--no-mdns] [--bind IP]... [--frames N] [--unregister]
//!
//! Every event is printed as one line; the last line is a summary.

use remote_core::connect::{Client, ClientConfig, ClientEvent, ConnectError, Host, HostConfig, HostEvent};
use remote_core::keys::{self, FileStore};
use remote_core::msg::{self, *};
use remote_core::session::OutgoingFrame;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Args {
    mode: String,
    store: String,
    name: String,
    pair: bool,
    code_file: Option<String>,
    rdv: bool,
    lan: bool,
    bind: Vec<IpAddr>,
    secs: u64,
    connect: bool,
    takeover: bool,
    frames: u32,
    unregister: bool,
    pairings: usize,
    hold: u64,
    max_kbps: u64,
    relay_only: bool,
    takeover_on_busy: bool,
    sleep_notice: bool,
    presence: bool,
}

fn args() -> Args {
    let mut it = std::env::args().skip(1);
    let mut a = Args {
        mode: it.next().unwrap_or_default(),
        store: String::new(),
        name: String::new(),
        pair: false,
        code_file: None,
        rdv: true,
        lan: true,
        bind: vec![],
        secs: 60,
        connect: false,
        takeover: false,
        frames: 30,
        unregister: false,
        pairings: 1,
        hold: 120,
        max_kbps: 50_000,
        relay_only: false,
        takeover_on_busy: false,
        sleep_notice: false,
        presence: false,
    };
    while let Some(k) = it.next() {
        match k.as_str() {
            "--store" => a.store = it.next().unwrap(),
            "--name" => a.name = it.next().unwrap(),
            "--pair" => a.pair = true,
            "--code-file" => a.code_file = it.next(),
            "--no-rdv" => a.rdv = false,
            "--no-lan" | "--no-mdns" => a.lan = false,
            "--bind" => a.bind.push(it.next().unwrap().parse().expect("--bind IP")),
            "--secs" => a.secs = it.next().unwrap().parse().unwrap(),
            "--connect" => a.connect = true,
            "--takeover" => a.takeover = true,
            "--frames" => a.frames = it.next().unwrap().parse().unwrap(),
            "--unregister" => a.unregister = true,
            "--pairings" => a.pairings = it.next().unwrap().parse().unwrap(),
            "--hold" => a.hold = it.next().unwrap().parse().unwrap(),
            "--max-kbps" => a.max_kbps = it.next().unwrap().parse().unwrap(),
            "--relay-only" => a.relay_only = true,
            "--takeover-on-busy" => a.takeover_on_busy = true,
            "--sleep-notice" => a.sleep_notice = true,
            "--presence" => a.presence = true,
            _ => panic!("unknown argument {k}"),
        }
    }
    a
}

fn log(who: &str, t0: Instant, what: impl std::fmt::Display) {
    println!("{:>8.3} {who} {what}", t0.elapsed().as_secs_f64());
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let a = args();
    let store = Arc::new(FileStore::new(&a.store));
    match a.mode.as_str() {
        "keygen" => println!("{}", keys::load_or_create_device_key(store.as_ref()).unwrap().id()),
        "host" => host(a, store).await,
        "client" => client(a, store).await,
        _ => eprintln!("usage: connect_demo keygen|host|client --store DIR ..."),
    }
}

async fn host(a: Args, store: Arc<FileStore>) {
    let t0 = Instant::now();
    let mut cfg = HostConfig::new(store, if a.name.is_empty() { "test-host".into() } else { a.name.clone() });
    // The server server.toml names (none: LAN only).
    cfg.rendezvous = if a.rdv { remote_core::server_config::ServerConfig::load_rendezvous() } else { None };
    cfg.lan = a.lan;
    if !a.bind.is_empty() {
        cfg.bind = Some(a.bind.clone());
    }
    cfg.hold = Duration::from_secs(a.hold);
    cfg.relay_only = a.relay_only;
    let (host, mut ev) = Host::start(cfg).await.expect("host start");
    log("host", t0, format!("id {} lan_port {:?}", host.id(), host.lan_port()));
    let mut opened = false;
    let mut paired_now = 0;
    let (mut inputs, mut controls, mut connected) = (0, 0, 0);
    let end = tokio::time::Instant::now() + Duration::from_secs(a.secs);
    let mut rdv_up = !a.rdv;
    let mut video: Option<tokio::task::JoinHandle<()>> = None;
    loop {
        if a.pair && !opened && rdv_up && paired_now < a.pairings {
            let w = host.open_pairing().await.unwrap();
            log("host", t0, format!("pairing window open for {:?}", w.expires_in));
            if let Some(f) = &a.code_file {
                std::fs::write(f, &w.code).unwrap();
            }
            opened = true;
        }
        let e = tokio::select! {
            e = ev.recv() => e,
            _ = tokio::time::sleep_until(end) => break,
        };
        let Some(e) = e else { break };
        match &e {
            HostEvent::Rendezvous { online, error } => {
                log("host", t0, format!("rendezvous online={online} error={error:?}"));
                rdv_up |= *online;
                if error.as_deref().is_some_and(|e| e.contains("host_not_allowed")) {
                    log("host", t0, "not enrolled");
                    break;
                }
            }
            HostEvent::ClientConnected { client, name, hello, took_over_from, resumed } => {
                connected += 1;
                log("host", t0, format!("client connected {} {name:?} {}x{} took_over_from={took_over_from:?} resumed={resumed}", client.short(), hello.display.width, hello.display.height));
                host.send(&Msg::Welcome(Welcome {
                    version: msg::VERSION,
                    name: "test-host".into(),
                    format: StreamFormat { width: hello.display.width, height: hello.display.height, scale: 1.6, refresh_mhz: 60000, codec: Codec::Hevc444 },
                    scales: vec![1.333, 1.6, 2.0],
                    home_monitor: Some(HomeMonitorMode::Mirror),
                    locked: false,
                    took_over_from: took_over_from.clone(),
                    resumed: *resumed,
                    host_os: msg::Os::Linux,
                    display_mode: msg::DisplayMode::Virtual,
                    live_resize: true,
                    displays: vec![],
                    current_display: 0,
                    view_only: false,
                    host_os_flavor: String::new(),
                }));
                host.send(&Msg::Caret(Caret { visible: true, rect: Rect { x: 100, y: 200, w: 2, h: 30 } }));
                host.send_cursor_shape(42, CursorImage { width: 2, height: 2, hot_x: 0, hot_y: 0, scale: 1.6, pixels: vec![255; 16] });
                host.send_cursor_position(10.0, 20.0, true, 42);
                // Dummy video at 60 fps, sized by the controller's bitrate.
                let h = host.clone();
                let max_bps = a.max_kbps * 1000;
                if let Some(v) = video.take() {
                    v.abort();
                }
                video = Some(tokio::spawn(async move {
                    let mut i = 0u32;
                    let mut tick = tokio::time::interval(Duration::from_micros(16_667));
                    loop {
                        tick.tick().await;
                        let bps = h.encoder_bitrate_bps().clamp(500_000, max_bps.max(500_000));
                        let size = (bps / 8 / 60) as usize * if i == 0 { 3 } else { 1 };
                        if !h.send_video(OutgoingFrame { data: vec![(i % 251) as u8; size.min(400_000)], keyframe: i == 0, meta: vec![] }) {
                            break;
                        }
                        i += 1;
                    }
                }));
            }
            HostEvent::Paired(d) => {
                paired_now += 1;
                log("host", t0, format!("paired {} {:?}", d.id.short(), d.name));
            }
            HostEvent::PairingClosed { reason } => {
                log("host", t0, format!("pairing window closed: {reason}"));
                if let Some(f) = &a.code_file {
                    let _ = std::fs::remove_file(f);
                }
                opened = false;
            }
            HostEvent::Input { event: i, .. } => {
                inputs += 1;
                log("host", t0, format!("input {i:?}"));
            }
            HostEvent::Control(m) => {
                controls += 1;
                let s = format!("{m:?}");
                log("host", t0, format!("control {}", &s[..s.len().min(160)]));
            }
            HostEvent::Stats(s) => log("host", t0, format!("stats rtt {:.1} ms, sent {} frames, {} kbps target", s.rtt_ms, s.frames_sent, s.target_bps / 1000)),
            e => log("host", t0, format!("{e:?}")),
        }
    }
    if a.sleep_notice {
        host.sleep_notice().await;
        log("host", t0, "sleep notice sent");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    if a.unregister {
        log("host", t0, format!("unregister: {:?}", host.unregister().await));
    }
    host.shutdown().await;
    println!("SUMMARY host connected={connected} inputs={inputs} controls={controls} paired={}", host.paired().len());
}

async fn client(a: Args, store: Arc<FileStore>) {
    let t0 = Instant::now();
    let mut cfg = ClientConfig::new(store, if a.name.is_empty() { "test-client".into() } else { a.name.clone() });
    // The server server.toml names (none: LAN only).
    cfg.rendezvous = if a.rdv { remote_core::server_config::ServerConfig::load_rendezvous() } else { None };
    cfg.mdns = a.lan;
    if !a.bind.is_empty() {
        cfg.bind = Some(a.bind.clone());
    }
    cfg.hold = Duration::from_secs(a.hold);
    cfg.relay_only = a.relay_only;
    let (client, mut ev) = Client::start(cfg).await.expect("client start");
    log("client", t0, format!("id {}", client.id()));
    if a.rdv {
        log("client", t0, format!("rendezvous up: {}", client.wait_rendezvous(Duration::from_secs(10)).await));
    }
    if a.presence {
        log("client", t0, format!("presence {:?}", client.presence().await));
    }
    let mut paired_ok = false;
    if let Some(f) = &a.code_file {
        // Wait for the host's code to appear (the "user" reads it off the screen).
        let mut code = String::new();
        for _ in 0..200 {
            if let Ok(c) = std::fs::read_to_string(f) {
                if c.len() == 6 {
                    code = c;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let targets = client.discover_pairing(Duration::from_secs(5)).await;
        log("client", t0, format!("discovered {targets:?}"));
        let Some(t) = targets.first() else {
            println!("SUMMARY client paired=false");
            return;
        };
        // One wrong try first: the window takes three.
        let wrong = format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000);
        log("client", t0, format!("pair with a wrong code: {:?}", client.pair(t, &wrong).await));
        let targets = client.discover_pairing(Duration::from_secs(5)).await;
        let t = targets.first().unwrap_or(t);
        let r = client.pair(t, &code).await;
        log("client", t0, format!("pair: {r:?}"));
        paired_ok = r.is_ok();
    }
    let mut frames = 0u32;
    let mut welcome = false;
    let mut shape = false;
    let mut controls = 0;
    let mut route = None;
    let mut disconnected = None;
    if a.connect {
        let host = client.paired().first().map(|d| d.id).expect("a paired host");
        let hello = Hello {
            version: msg::VERSION,
            min_version: msg::MIN_VERSION,
            name: String::new(),
            display: ClientDisplay { width: 2560, height: 1664, pixels_per_point: 1.741, refresh_mhz: 60000, fullscreen: false, display_width: 2560, display_height: 1664 },
            decode: DecodeSupport { hevc: true, h264: true, hevc_444: true, av1: true, max_width: 0, max_height: 0 },
            settings: StreamSettings::default(),
            client_os: msg::Os::Linux,
            client_os_flavor: String::new(),
        };
        let mut r = client.connect(host, hello.clone(), a.takeover).await;
        log("client", t0, format!("connect: {r:?}"));
        if let (Err(ConnectError::Busy { by }), true) = (&r, a.takeover_on_busy) {
            log("client", t0, format!("\"In use by {by}. Take over?\" → yes"));
            r = client.connect(host, hello, true).await;
            log("client", t0, format!("connect with takeover: {r:?}"));
        }
        if r.is_ok() {
            let end = tokio::time::Instant::now() + Duration::from_secs(a.secs);
            loop {
                let e = tokio::select! {
                    e = ev.recv() => e,
                    _ = tokio::time::sleep_until(end) => break,
                };
                let Some(e) = e else { break };
                match &e {
                    ClientEvent::VideoFrame(f) => {
                        frames += 1;
                        if frames == 1 || frames % 30 == 0 {
                            log("client", t0, format!("video frame {} ({} bytes, keyframe {})", f.frame_id, f.data.len(), f.keyframe));
                        }
                        if frames >= a.frames && welcome {
                            break;
                        }
                    }
                    ClientEvent::Control(Msg::Welcome(w)) => {
                        welcome = true;
                        controls += 1;
                        log("client", t0, format!("welcome {w:?}"));
                        client.send(Msg::Resize(ClientDisplay { width: 1920, height: 1200, pixels_per_point: 1.741, refresh_mhz: 60000, fullscreen: false, display_width: 2560, display_height: 1664 }));
                        client.send(Msg::ClipboardOffer(ClipboardOffer {
                            serial: 1,
                            items: vec![ClipItem { format: ClipFormat::Text, size: 5, inline: Some(b"hello".to_vec()) }],
                            concealed: false,
                        }));
                        client.send_input(&InputEvent::Key { usage: 0x04, down: true });
                        client.send_input(&InputEvent::Key { usage: 0x04, down: false });
                        client.send_input(&InputEvent::ImeCommit { text: "你好".into() });
                        client.send_input(&InputEvent::PointerAbs { x: 100.5, y: 200.25 });
                    }
                    ClientEvent::Control(m) => {
                        controls += 1;
                        log("client", t0, format!("control {m:?}"));
                    }
                    ClientEvent::CursorShape { hash, image } => {
                        shape = true;
                        log("client", t0, format!("cursor shape {hash} {}x{}", image.width, image.height));
                    }
                    ClientEvent::CursorPosition { .. } => {}
                    ClientEvent::Route(r) => {
                        route = Some(r.kind.clone());
                        log("client", t0, format!("route {:?} {} → {} rtt {:.1} ms via {:?}", r.kind, r.local, r.remote, r.rtt_ms, r.interface));
                    }
                    ClientEvent::Stats(s) => log("client", t0, format!("stats rtt {:.1} ms, received {} frames, lost {}", s.rtt_ms, s.frames_received, s.frames_lost)),
                    ClientEvent::Disconnected { reason } => {
                        log("client", t0, format!("disconnected: {reason:?}"));
                        disconnected = Some(reason.clone());
                        break;
                    }
                    e => log("client", t0, format!("{e:?}")),
                }
            }
            client.disconnect();
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    if a.unregister {
        log("client", t0, format!("unregister: {:?}", client.unregister().await));
    }
    client.shutdown().await;
    println!("SUMMARY client paired={paired_ok} welcome={welcome} frames={frames} cursor_shape={shape} controls={controls} route={route:?} disconnected={disconnected:?}");
}
