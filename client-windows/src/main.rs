// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! remote-client.exe: Sodium Remote's Windows client (CLI and test client, like
//! the Linux one). See docs/reference/client-windows.md.



#[cfg(not(windows))]
fn main() {
    eprintln!("remote-client for Windows: build it with `. remote/packaging/windows/env.sh && cargo build --target x86_64-pc-windows-gnu -p client-windows`");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    cli::main();
}

#[cfg(windows)]
mod cli {
    use anyhow::{Context, Result, bail};
    use remote_engine::WindowOptions;
    use remote_engine::client::{self, CodecChoice, Options};
    use remote_engine::connect::{NetOptions, parse_lan_hint};
    use remote_engine::decode::HwPref;
    use remote_engine::latency::ClickConfig;
    use std::path::PathBuf;
    use std::time::Duration;

    const USAGE: &str = "\
remote-client --data DIR pair [--code NNNNNN] [--host ID] [network options] [--timeout S]
remote-client --data DIR [connect] [--host ID] [network options] [stream options]

  --data DIR             this device's key and paired.json (created if missing)
  --name NAME            this device's name, as the host shows it (default \"Windows client\")
  --host ID              the host: its full ID or its code (XXXX-XXXX); default the only one

  pair:
  --code NNNNNN          the code the host shows (default $REMOTE_PAIR_CODE)
  --timeout S            how long to look for a host with an open pairing window (default 15)
  exit 0 paired, 2 wrong code, 3 the window closed, 4 failed or no host

  network:
  --no-rendezvous        LAN only (mDNS and the host's LAN listener)
  --no-mdns              don't browse mDNS
  --relay-only           share only relayed candidates (TURN)
  --bind IP              local address for ICE (repeatable; default every IPv4 interface)
  --lan-hint ID@IP:PORT  a host's LAN listener, for when mDNS can't be used (repeatable)

  connect (exit 0 when the user ends it, 3 when the session ends otherwise, 4 when it never connects):
  --headless             decode and stamp, no window
  --takeover             when the host is in use, take over
  --decoder auto|d3d11|sw   default auto (Direct3D 11 video decode, then software)
  --codec auto|hevc|h264 a forced codec (default auto)
  --stats-dir DIR        where the session's stats files go (default .)
  --secs N               quit after N seconds
  --fps N                ask for this stream rate (default: the display's)
  --max-mbps N           bandwidth cap (default: Auto)
  --size WxH             ask for exactly this stream size, never resize (default: the window;
                         headless 2560x1664)
  --fixed                fixed resolution: keep the first size and scale the picture
  --ppp F                pixels per point for the UI scale (default: the monitor's DPI; headless 1.741)
  --fullscreen           a borderless window over the monitor
  --no-zero-copy         copy each decoded picture to system memory and upload it (default: draw it
                         straight from the decoder's texture where the GPU allows)
  --no-vsync             present without waiting for the vsync where DXGI allows tearing (full screen)
  --no-hud               hide the stats HUD
  --title TEXT           window title (default \"Sodium Remote\")
  --input-log FILE       log every input event sent
  --no-audio             don't play the host's audio
  --audio-log FILE       log the audio's level (RMS) once a second

  unattended tests:
  --script FILE          timed inputs: keys, text, clicks, scroll, clipboard set (see client-engine/src/script.rs)
  --dump-frames DIR      write decoded frames there as PNG
  --dump-every N         every Nth decoded frame (default 30)

  click-to-photon test:
  --clicks N             click N times on the host's test surface, then quit
  --marker X,Y,W,H       the test surface's marker in stream pixels (default 64,64,256,256)
  --click-gap MIN,MAX    random gap between clicks in ms (default 250,450)
  --idle-ms N            then measure idle bandwidth for N ms (default 3000)
  --stay                 keep running after the clicks (until --secs or a signal)
";

    fn numbers<T: std::str::FromStr>(s: &str, n: usize) -> Result<Vec<T>> {
        let v: Vec<T> = s.split(',').map(|p| p.trim().parse::<T>().ok()).collect::<Option<_>>().context("bad number list")?;
        if v.len() != n {
            bail!("expected {n} comma-separated numbers, got {s:?}");
        }
        Ok(v)
    }

    pub fn main() {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .with_writer(std::io::stderr)
            .init();
        match run() {
            Ok(code) => std::process::exit(code),
            Err(e) => {
                eprintln!("remote-client: {e:#}");
                std::process::exit(1);
            }
        }
    }

    fn run() -> Result<i32> {
        let mut net = NetOptions {
            data: PathBuf::new(),
            name: "Windows client".into(),
            rendezvous: true,
            mdns: true,
            relay_only: false,
            bind: vec![],
            lan_hints: vec![],
            fps: 60,
        };
        let mut o = Options {
            net: net.clone(),
            host: None,
            headless: false,
            decoder: HwPref::Auto,
            stats_dir: PathBuf::from("."),
            click: None,
            secs: None,
            fps: None,
            max_mbps: None,
            takeover: false,
            input_log: None,
            zero_copy: true,
            codec: CodecChoice::Auto,
            script: None,
            dump_frames: None,
            thumb: None,
            window: WindowOptions { title: "Sodium Remote".into(), fullscreen: false, hud: true, fixed: false, size: None, ppp: None },
        };
        let mut audio = client_windows::platform::AudioOptions { enabled: true, log: None };
        let (mut dump_dir, mut dump_every) = (None, 30u32);
        let mut command = None;
        let mut code = None;
        let mut timeout = 15u64;
        let mut clicks = 0u32;
        let mut marker = (64, 64, 256, 256);
        let mut gap = (250, 450);
        let mut idle_ms = 3000;
        let mut stay = false;
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            let mut val = || it.next().with_context(|| format!("{a} needs a value"));
            match a.as_str() {
                "pair" | "connect" if command.is_none() => command = Some(a.clone()),
                "--data" => net.data = val()?.into(),
                "--name" => net.name = val()?,
                "--host" => o.host = Some(val()?),
                "--code" => code = Some(val()?),
                "--timeout" => timeout = val()?.parse()?,
                "--no-rendezvous" => net.rendezvous = false,
                "--no-mdns" => net.mdns = false,
                "--relay-only" => net.relay_only = true,
                "--bind" => net.bind.push(val()?.parse().context("--bind IP")?),
                "--lan-hint" => net.lan_hints.push(parse_lan_hint(&val()?)?),
                "--headless" => o.headless = true,
                "--takeover" => o.takeover = true,
                "--decoder" => o.decoder = HwPref::parse(&val()?).context("--decoder auto|d3d11|sw")?,
                "--codec" => o.codec = CodecChoice::parse(&val()?).context("--codec auto|hevc|h264")?,
                "--stats-dir" => o.stats_dir = val()?.into(),
                "--secs" => o.secs = Some(val()?.parse()?),
                "--fps" => o.fps = Some(val()?.parse()?),
                "--max-mbps" => o.max_mbps = Some(val()?.parse()?),
                "--size" => {
                    let v: Vec<u32> = val()?.split('x').map(|p| p.parse().unwrap_or(0)).collect();
                    if v.len() != 2 || v[0] == 0 || v[1] == 0 {
                        bail!("--size WxH");
                    }
                    o.window.size = Some((v[0], v[1]));
                }
                "--fixed" => o.window.fixed = true,
                "--ppp" => o.window.ppp = Some(val()?.parse().context("--ppp F")?),
                "--fullscreen" => o.window.fullscreen = true,
                "--no-hud" => o.window.hud = false,
                "--title" => o.window.title = val()?,
                "--clicks" => clicks = val()?.parse()?,
                "--marker" => {
                    let v = numbers::<u32>(&val()?, 4)?;
                    marker = (v[0], v[1], v[2], v[3]);
                }
                "--click-gap" => {
                    let v = numbers::<u64>(&val()?, 2)?;
                    gap = (v[0], v[1]);
                }
                "--idle-ms" => idle_ms = val()?.parse()?,
                "--stay" => stay = true,
                "--input-log" => o.input_log = Some(val()?.into()),
                "--no-audio" => audio.enabled = false,
                "--audio-log" => audio.log = Some(val()?.into()),
                "--script" => o.script = Some(val()?.into()),
                "--dump-frames" => dump_dir = Some(PathBuf::from(val()?)),
                "--dump-every" => dump_every = val()?.parse().context("--dump-every N")?,
                "--no-zero-copy" => o.zero_copy = false,
                "--no-vsync" => client_windows::d3d::set_tearing(true),
                "-h" | "--help" => {
                    print!("{USAGE}");
                    return Ok(0);
                }
                _ => bail!("unknown option {a}\n\n{USAGE}"),
            }
        }
        if net.data.as_os_str().is_empty() {
            bail!("--data DIR is required\n\n{USAGE}");
        }
        if let Some(f) = o.fps {
            net.fps = f;
        }
        if command.as_deref() == Some("pair") {
            let code = match code.or_else(|| std::env::var("REMOTE_PAIR_CODE").ok()) {
                Some(c) => c,
                None => bail!("pair needs --code NNNNNN (or $REMOTE_PAIR_CODE)"),
            };
            return client::pair(net, code, o.host, Duration::from_secs(timeout));
        }
        o.net = net;
        o.dump_frames = dump_dir.map(|d| (d, dump_every));
        if clicks > 0 {
            o.click = Some(ClickConfig { count: clicks, marker, gap_ms: gap, timeout_ms: 1500, idle_ms, quit_when_done: !stay });
        }
        // Zero copy draws from the decoder's textures: nothing to draw headless, and the frame dump reads system memory.
        if o.headless || o.dump_frames.is_some() {
            o.zero_copy = false;
        }
        // Decode on the GPU that has a video decoder, which may not be the one driving the display.
        client_windows::d3d::decode_adapter();
        if o.zero_copy && !client_windows::d3d::share_with_decoder() {
            o.zero_copy = false;
        }
        client_windows::platform::init_process();
        client::run(o, std::sync::Arc::new(client_windows::platform::Windows::new(audio)?))
    }
}
