// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! probe: a scripted test client for a host (the unattended checks of
//!, docs/reference/host-desktop.md "Testing"). It pairs, connects, runs a
//! script of input and clipboard steps, and counts what comes back: video frames (and
//! dumps the bitstream for ffmpeg to decode), cursor shapes and positions, audio
//! (decoded, with its RMS), clipboard offers, secure-screen notices. No window.
//!
//!   probe --data DIR pair --code NNNNNN [--lan-hint ID@IP:PORT]
//!   probe --data DIR [--lan-hint ID@IP:PORT] [--secs N] [steps…]
//!
//! Steps run in order once the first frame is in, 300 ms apart:
//!   --wait MS  --type TEXT  --key USAGE  --commit TEXT  --click X,Y  --move X,Y
//!   --scroll N  --clip-text TEXT  --clip-png FILE  --select-display ID  --resize WxH  --keyframe
//!   --key-table mac|pc  --disconnect
//! Other options: --h264-only, --size WxH, --dump FILE, --save-clip DIR, --no-mdns,
//!   --takeover, --name NAME

use std::collections::HashSet;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use host_desktop::opus::OpusDecoder;
use remote_core::audio::AudioPlay;
use remote_core::connect::{Client, ClientConfig, ClientEvent};
use remote_core::keys::{DeviceId, FileStore};
use remote_core::msg::{self, Button, ClipFormat, ClipItem, InputEvent, KeyRule, Msg};

#[derive(Clone, Debug)]
enum Step {
    Wait(u64),
    Type(String),
    Key(u16),
    Commit(String),
    Click(f64, f64),
    Move(f64, f64),
    Scroll(i32),
    ClipText(String),
    ClipPng(PathBuf),
    SelectDisplay(u32),
    Resize(u32, u32),
    Keyframe,
    KeyTable(Vec<KeyRule>),
    Disconnect,
}

struct Opts {
    data: PathBuf,
    name: String,
    hints: Vec<(DeviceId, SocketAddr)>,
    mdns: bool,
    secs: u64,
    h264_only: bool,
    size: (u32, u32),
    dump: Option<PathBuf>,
    save_clip: Option<PathBuf>,
    takeover: bool,
    steps: Vec<Step>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("probe: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut o = Opts {
        data: PathBuf::new(),
        name: "Probe".into(),
        hints: vec![],
        mdns: true,
        secs: 20,
        h264_only: false,
        size: (1920, 1080),
        dump: None,
        save_clip: None,
        takeover: true,
        steps: vec![],
    };
    let mut pair_code = None;
    let mut it = std::env::args().skip(1);
    let num = |s: &str| -> Result<Vec<f64>> { s.split(',').map(|p| p.trim().parse::<f64>().context("number")).collect() };
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "pair" => {}
            "--code" => pair_code = Some(val()?),
            "--data" => o.data = val()?.into(),
            "--name" => o.name = val()?,
            "--lan-hint" => {
                let v = val()?;
                let (id, addr) = v.split_once('@').context("--lan-hint ID@IP:PORT")?;
                o.hints.push((id.parse().map_err(|e| anyhow::anyhow!("{e:?}"))?, addr.parse()?));
            }
            "--no-mdns" => o.mdns = false,
            "--secs" => o.secs = val()?.parse()?,
            "--h264-only" => o.h264_only = true,
            "--size" => {
                let v: Vec<u32> = val()?.split('x').map(|p| p.parse().unwrap_or(0)).collect();
                o.size = (v[0], v[1]);
            }
            "--dump" => o.dump = Some(val()?.into()),
            "--save-clip" => o.save_clip = Some(val()?.into()),
            "--no-takeover" => o.takeover = false,
            "--wait" => o.steps.push(Step::Wait(val()?.parse()?)),
            "--type" => o.steps.push(Step::Type(val()?)),
            "--key" => {
                let v = val()?;
                o.steps.push(Step::Key(u16::from_str_radix(v.trim_start_matches("0x"), 16)?));
            }
            "--commit" => o.steps.push(Step::Commit(val()?)),
            "--click" => {
                let v = num(&val()?)?;
                o.steps.push(Step::Click(v[0], v[1]));
            }
            "--move" => {
                let v = num(&val()?)?;
                o.steps.push(Step::Move(v[0], v[1]));
            }
            "--scroll" => o.steps.push(Step::Scroll(val()?.parse()?)),
            "--clip-text" => o.steps.push(Step::ClipText(val()?)),
            "--clip-png" => o.steps.push(Step::ClipPng(val()?.into())),
            "--select-display" => o.steps.push(Step::SelectDisplay(val()?.parse()?)),
            "--resize" => {
                let v = val()?;
                let (w, h) = v.split_once('x').context("--resize WxH")?;
                o.steps.push(Step::Resize(w.parse()?, h.parse()?));
            }
            "--keyframe" => o.steps.push(Step::Keyframe),
            "--key-table" => {
                let t = match val()?.as_str() {
                    "mac" => KeyRule::mac_defaults(),
                    _ => vec![],
                };
                o.steps.push(Step::KeyTable(t));
            }
            "--disconnect" => o.steps.push(Step::Disconnect),
            _ => bail!("unknown option {a}"),
        }
    }
    if o.data.as_os_str().is_empty() {
        bail!("--data DIR is required");
    }
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    rt.block_on(async move {
        std::fs::create_dir_all(&o.data)?;
        let mut cfg = ClientConfig::new(Arc::new(FileStore::new(&o.data)), o.name.clone());
        cfg.rendezvous = None;
        cfg.mdns = o.mdns;
        cfg.lan_hints = o.hints.clone();
        let (client, events) = Client::start(cfg).await?;
        match pair_code {
            Some(code) => pair(&client, &code, o.hints.first().copied()).await,
            None => session(client, events, o).await,
        }
    })
}

/// Pairs over the LAN: with the host given by `--lan-hint` (no mDNS needed), else the
/// one host mDNS finds with a pairing window open.
async fn pair(client: &Client, code: &str, hint: Option<(DeviceId, SocketAddr)>) -> Result<()> {
    if let Some((host, addr)) = hint {
        let t = remote_core::connect::PairingTarget { host, name: String::new(), lan: Some((vec![addr.ip()], addr.port())), rendezvous: None };
        let d = client.pair(&t, code).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
        println!("paired {} {} {:?}", d.id, d.id.code(), d.name);
        client.shutdown().await;
        return Ok(());
    }
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        let t = client.discover_pairing(Duration::from_secs(2)).await;
        if let Some(t) = t.first() {
            eprintln!("probe: pairing with {:?} ({})", t.name, t.host.code());
            let d = client.pair(t, code).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
            println!("paired {} {} {:?}", d.id, d.id.code(), d.name);
            client.shutdown().await;
            return Ok(());
        }
    }
    bail!("no host with an open pairing window")
}

#[derive(Default)]
struct Counts {
    frames: u64,
    keyframes: u64,
    refreshes: u64,
    lost: u64,
    undecodable: u64,
    bytes: u64,
    first_frame: Option<Instant>,
    last_frame: Option<Instant>,
    shapes: HashSet<u64>,
    shape_events: u64,
    positions: u64,
    /// The last pointer position the host reported (stream pixels) and whether it shows.
    last_pos: Option<(f32, f32, bool)>,
    audio_packets: u64,
    audio_conceal: u64,
    audio_sq: f64,
    audio_n: u64,
    secure: Vec<(f64, bool)>,
    clips: Vec<String>,
    welcome: String,
    formats: Vec<String>,
    host_fps: Vec<f32>,
}

/// US-layout HID usage (and Shift) for an ASCII character.
fn ascii_usage(c: char) -> Option<(u16, bool)> {
    let lower = "abcdefghijklmnopqrstuvwxyz";
    if let Some(i) = lower.find(c) {
        return Some((0x04 + i as u16, false));
    }
    if let Some(i) = lower.to_uppercase().find(c) {
        return Some((0x04 + i as u16, true));
    }
    let digits = "1234567890";
    if let Some(i) = digits.find(c) {
        return Some((0x1E + i as u16, false));
    }
    let shifted_digits = "!@#$%^&*()";
    if let Some(i) = shifted_digits.find(c) {
        return Some((0x1E + i as u16, true));
    }
    let plain = [(' ', 0x2C), ('\n', 0x28), ('\t', 0x2B), ('-', 0x2D), ('=', 0x2E), ('[', 0x2F), (']', 0x30), ('\\', 0x31), (';', 0x33), ('\'', 0x34), ('`', 0x35), (',', 0x36), ('.', 0x37), ('/', 0x38)];
    let shifted = [('_', 0x2D), ('+', 0x2E), ('{', 0x2F), ('}', 0x30), ('|', 0x31), (':', 0x33), ('"', 0x34), ('~', 0x35), ('<', 0x36), ('>', 0x37), ('?', 0x38)];
    plain.iter().find(|(k, _)| *k == c).map(|&(_, u)| (u, false)).or_else(|| shifted.iter().find(|(k, _)| *k == c).map(|&(_, u)| (u, true)))
}

async fn session(client: Client, mut events: tokio::sync::mpsc::UnboundedReceiver<ClientEvent>, o: Opts) -> Result<()> {
    let host = match client.paired().as_slice() {
        [d] => d.id,
        [] => bail!("not paired: run `probe --data DIR pair --code NNNNNN` first"),
        many => many.last().unwrap().id,
    };
    let hello = msg::Hello {
        version: msg::VERSION,
        min_version: msg::MIN_VERSION,
        name: String::new(),
        display: msg::ClientDisplay {
            width: o.size.0,
            height: o.size.1,
            pixels_per_point: 1.0,
            refresh_mhz: 60_000,
            fullscreen: true,
            display_width: o.size.0,
            display_height: o.size.1,
        },
        decode: msg::DecodeSupport { hevc: !o.h264_only, h264: true, hevc_444: false, av1: false, max_width: 0, max_height: 0 },
        settings: msg::StreamSettings::default(),
        client_os: msg::Os::Linux,
        client_os_flavor: String::new(),
    };
    let t0 = Instant::now();
    eprintln!("probe: connecting to {}", host.code());
    client.connect(host, hello, o.takeover).await.map_err(|e| anyhow::anyhow!("connect: {e:?}"))?;
    client.send(Msg::KeyTable(vec![]));
    let mut c = Counts::default();
    let mut dump = match &o.dump {
        Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(p)?)),
        None => None,
    };
    let mut dec = OpusDecoder::new().context("opus decoder")?;
    let mut audio_tick = tokio::time::interval(Duration::from_millis(10));
    let end = tokio::time::Instant::now() + Duration::from_secs(o.secs);
    let mut steps = o.steps.clone().into_iter();
    let mut next_step: Option<tokio::time::Instant> = None;
    let mut clip_serial = 1u32;
    let mut offered: Option<(u32, Vec<(ClipFormat, Vec<u8>)>)> = None;
    let mut incoming: Option<(u32, Vec<(ClipFormat, u64, Vec<u8>)>)> = None;
    let since = |t0: Instant| t0.elapsed().as_secs_f64();
    loop {
        let step_at = next_step.unwrap_or_else(|| end + Duration::from_secs(3600));
        tokio::select! {
            _ = tokio::time::sleep_until(end) => break,
            _ = audio_tick.tick() => {
                if let Some(buf) = client.audio_buffer() {
                    let out = buf.lock().unwrap().pop(Instant::now());
                    if let Some(out) = out {
                        match out.play {
                            AudioPlay::Packet(p) | AudioPlay::Fec(p) => {
                                c.audio_packets += 1;
                                if let Some(pcm) = dec.decode(&p) {
                                    c.audio_sq += pcm.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>();
                                    c.audio_n += pcm.len() as u64;
                                }
                            }
                            AudioPlay::Conceal => c.audio_conceal += 1,
                        }
                    }
                }
            }
            _ = tokio::time::sleep_until(step_at) => {
                match steps.next() {
                    None => next_step = None,
                    Some(s) => {
                        eprintln!("probe: {:.2} s: {s:?}", since(t0));
                        let mut gap = 300;
                        match s {
                            Step::Wait(ms) => gap = ms,
                            Step::Type(text) => {
                                for ch in text.chars() {
                                    let Some((u, shift)) = ascii_usage(ch) else { continue };
                                    if shift { client.send_input(&InputEvent::Key { usage: 0xE1, down: true }); }
                                    client.send_input(&InputEvent::Key { usage: u, down: true });
                                    tokio::time::sleep(Duration::from_millis(15)).await;
                                    client.send_input(&InputEvent::Key { usage: u, down: false });
                                    if shift { client.send_input(&InputEvent::Key { usage: 0xE1, down: false }); }
                                    tokio::time::sleep(Duration::from_millis(15)).await;
                                }
                            }
                            Step::Key(u) => {
                                client.send_input(&InputEvent::Key { usage: u, down: true });
                                tokio::time::sleep(Duration::from_millis(30)).await;
                                client.send_input(&InputEvent::Key { usage: u, down: false });
                            }
                            Step::Commit(text) => {
                                for e in InputEvent::commit_chunks(&text) { client.send_input(&e); }
                            }
                            Step::Move(x, y) => { client.send_input(&InputEvent::PointerAbs { x, y }); }
                            Step::Click(x, y) => {
                                client.send_input(&InputEvent::PointerAbs { x, y });
                                tokio::time::sleep(Duration::from_millis(30)).await;
                                client.send_input(&InputEvent::Button { button: Button::Left, down: true });
                                tokio::time::sleep(Duration::from_millis(40)).await;
                                client.send_input(&InputEvent::Button { button: Button::Left, down: false });
                            }
                            Step::Scroll(n) => {
                                for _ in 0..n.abs() {
                                    let v = n.signum();
                                    client.send_input(&InputEvent::Axis { source: msg::AxisSource::Wheel, axis: msg::Axis::Vertical, value: 15.0 * v as f64, value120: 120 * v, stop: false, more: false });
                                    tokio::time::sleep(Duration::from_millis(40)).await;
                                }
                            }
                            Step::ClipText(t) => {
                                let items = vec![(ClipFormat::Text, t.into_bytes())];
                                offer(&client, clip_serial, &items);
                                offered = Some((clip_serial, items));
                                clip_serial += 1;
                            }
                            Step::ClipPng(p) => {
                                let items = vec![(ClipFormat::Png, std::fs::read(&p)?)];
                                offer(&client, clip_serial, &items);
                                offered = Some((clip_serial, items));
                                clip_serial += 1;
                            }
                            Step::SelectDisplay(id) => client.send(Msg::SelectDisplay(id)),
                            Step::Resize(w, h) => client.send(Msg::Resize(msg::ClientDisplay {
                width: w,
                height: h,
                pixels_per_point: 1.0,
                refresh_mhz: 60_000,
                fullscreen: false,
                display_width: o.size.0,
                display_height: o.size.1,
            })),
            Step::Keyframe => client.send(Msg::KeyframeRequest),
                            Step::KeyTable(t) => client.send(Msg::KeyTable(t)),
                            Step::Disconnect => {
                                client.disconnect();
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                break;
                            }
                        }
                        next_step = Some(tokio::time::Instant::now() + Duration::from_millis(gap));
                    }
                }
            }
            ev = events.recv() => {
                let Some(ev) = ev else { break };
                match ev {
                    ClientEvent::Connected { route } => eprintln!("probe: {:.2} s: connected {:?}", since(t0), route.map(|r| (r.kind, r.remote))),
                    ClientEvent::VideoFrame(f) => {
                        let now = Instant::now();
                        if c.first_frame.is_none() {
                            c.first_frame = Some(now);
                            eprintln!("probe: {:.2} s: first frame ({} bytes, key {})", since(t0), f.data.len(), f.keyframe);
                            next_step = Some(tokio::time::Instant::now() + Duration::from_millis(1000));
                        }
                        c.last_frame = Some(now);
                        c.frames += 1;
                        c.keyframes += f.keyframe as u64;
                        c.bytes += f.data.len() as u64;
                        if !f.decodable { c.undecodable += 1; }
                        if f.meta.len() >= 2 && f.meta[0] == 1 && f.meta[1] == 2 { c.refreshes += 1; }
                        if let Some(d) = &mut dump { d.write_all(&f.data)?; }
                    }
                    ClientEvent::VideoFrameLost { .. } => c.lost += 1,
                    ClientEvent::CursorShape { hash, image } => {
                        c.shape_events += 1;
                        if c.shapes.insert(hash) {
                            eprintln!("probe: {:.2} s: cursor shape {}x{} hot {},{}", since(t0), image.width, image.height, image.hot_x, image.hot_y);
                        }
                    }
                    ClientEvent::CursorPosition { x, y, visible, .. } => {
                        c.positions += 1;
                        // Each settled position, for checking injected moves come back.
                        if c.last_pos.is_none_or(|(px, py, pv)| (px - x).abs() > 2.0 || (py - y).abs() > 2.0 || pv != visible) {
                            eprintln!("probe: {:.2} s: pointer at {x:.0},{y:.0}{}", since(t0), if visible { "" } else { " (hidden)" });
                        }
                        c.last_pos = Some((x, y, visible));
                    }
                    ClientEvent::Control(m) => match m {
                        Msg::Welcome(w) => {
                            c.welcome = format!("{}x{} {:?} host {:?} {:?} displays {:?} current {}", w.format.width, w.format.height, w.format.codec, w.host_os, w.display_mode, w.displays.iter().map(|d| (d.id, d.name.clone(), d.width, d.height, d.primary)).collect::<Vec<_>>(), w.current_display);
                            eprintln!("probe: {:.2} s: welcome {}", since(t0), c.welcome);
                        }
                        Msg::Format(f) => {
                            let s = format!("{}x{} {:?}", f.width, f.height, f.codec);
                            eprintln!("probe: {:.2} s: format {s}", since(t0));
                            c.formats.push(s);
                        }
                        Msg::SecureScreen(on) => {
                            eprintln!("probe: {:.2} s: secure screen {on}", since(t0));
                            c.secure.push((since(t0), on));
                        }
                        Msg::Stats(s) => if let Some(h) = s.host { c.host_fps.push(h.fps); },
                        Msg::ClipboardOffer(off) => {
                            let mut items = vec![];
                            for it in off.items {
                                match it.inline {
                                    Some(d) => items.push((it.format, it.size, d)),
                                    None => {
                                        client.send(Msg::ClipboardRequest { serial: off.serial, format: it.format.clone() });
                                        items.push((it.format, it.size, vec![]));
                                    }
                                }
                            }
                            incoming = Some((off.serial, items));
                        }
                        Msg::ClipboardData(d) => {
                            if let Some((s, items)) = &mut incoming && *s == d.serial
                                && let Some((_, _, b)) = items.iter_mut().find(|(f, _, _)| *f == d.format)
                                && b.len() as u64 == d.offset
                            {
                                b.extend_from_slice(&d.bytes);
                            }
                        }
                        Msg::ClipboardRequest { serial, format } => {
                            if let Some((s, items)) = &offered && *s == serial
                                && let Some((_, data)) = items.iter().find(|(f, _)| *f == format)
                            {
                                for m in msg::ClipboardData::chunks(serial, &format, data) { client.send(m); }
                            }
                        }
                        Msg::Disconnect(r) => eprintln!("probe: the host disconnected: {r:?}"),
                        _ => {}
                    },
                    ClientEvent::Reconnecting { since: s } => eprintln!("probe: {:.2} s: reconnecting (silent {:.2} s)", since(t0), s.as_secs_f64()),
                    ClientEvent::Disconnected { reason } => {
                        eprintln!("probe: {:.2} s: disconnected: {reason:?}", since(t0));
                        break;
                    }
                    _ => {}
                }
                // A clipboard offer complete: save it.
                if incoming.as_ref().is_some_and(|(_, items)| items.iter().all(|(_, size, b)| b.len() as u64 >= *size)) {
                    let (serial, items) = incoming.take().unwrap();
                    for (f, _, b) in items {
                        let desc = match &f {
                            ClipFormat::Text => format!("text {:?}", String::from_utf8_lossy(&b)),
                            f => format!("{f:?} {} bytes", b.len()),
                        };
                        eprintln!("probe: {:.2} s: host clipboard {serial}: {desc}", since(t0));
                        c.clips.push(desc);
                        if let Some(dir) = &o.save_clip {
                            std::fs::create_dir_all(dir)?;
                            let name = match f { ClipFormat::Text => "text.txt", ClipFormat::Html => "html.html", ClipFormat::Png => "image.png", _ => "other.bin" };
                            std::fs::write(dir.join(format!("{serial}-{name}")), &b)?;
                        }
                    }
                }
            }
        }
    }
    client.disconnect();
    tokio::time::sleep(Duration::from_millis(300)).await;
    client.shutdown().await;
    if let Some(d) = &mut dump {
        d.flush()?;
    }
    let span = match (c.first_frame, c.last_frame) {
        (Some(a), Some(b)) if b > a => b.duration_since(a).as_secs_f64(),
        _ => 0.0,
    };
    let rms = if c.audio_n > 0 { (c.audio_sq / c.audio_n as f64).sqrt() } else { 0.0 };
    let mut hf = c.host_fps.clone();
    hf.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("welcome: {}", c.welcome);
    println!("formats: {:?}", c.formats);
    println!(
        "video: {} frames ({} keyframes, {} refreshes, {} lost, {} undecodable), {:.1} fps over {:.1} s, {:.2} Mbit/s",
        c.frames,
        c.keyframes,
        c.refreshes,
        c.lost,
        c.undecodable,
        if span > 0.0 { c.frames as f64 / span } else { 0.0 },
        span,
        if span > 0.0 { c.bytes as f64 * 8.0 / span / 1e6 } else { 0.0 }
    );
    println!("host fps per second: p50 {:.1}, max {:.1}", hf.get(hf.len() / 2).copied().unwrap_or(0.0), hf.last().copied().unwrap_or(0.0));
    println!("cursor: {} shapes ({} shape messages), {} positions, last {:?}", c.shapes.len(), c.shape_events, c.positions, c.last_pos);
    println!("audio: {} packets, {} concealed, RMS {:.4} ({:.1} dBFS)", c.audio_packets, c.audio_conceal, rms, 20.0 * rms.max(1e-9).log10());
    println!("secure screen: {:?}", c.secure);
    println!("host clipboard: {:?}", c.clips);
    Ok(())
}

fn offer(client: &Client, serial: u32, items: &[(ClipFormat, Vec<u8>)]) {
    let items = items
        .iter()
        .map(|(f, d)| ClipItem { format: f.clone(), size: d.len() as u64, inline: (d.len() <= msg::CLIPBOARD_CHUNK).then(|| d.clone()) })
        .collect();
    client.send(Msg::ClipboardOffer(msg::ClipboardOffer { serial, items, concealed: false }));
}
