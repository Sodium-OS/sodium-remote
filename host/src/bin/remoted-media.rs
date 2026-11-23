// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! remoted-media: runs the media pipeline (remoted::media) against the plugin and
//! records what a session would send, for the milestone 1 measurements
//! A small stand-in for core's budget feeds it.
//! Not installed; nested Hyprland only.

use std::{
    collections::{BTreeSet, VecDeque},
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use remoted::{
    capture::monotonic_ns,
    capture::Rect,
    media::{Codec, FrameClass, InputEvent, Media, MediaConfig, MediaEvent, RateInput},
};

const USAGE: &str = "\
remoted-media [options]   capture through the plugin with media's rules, record the stream

  --socket PATH             plugin socket (default $XDG_RUNTIME_DIR/sodiumos/remote-capture.sock)
  --output NAME             default REMOTE-1
  --codec hevc|av1          default hevc
  --chroma 420|444          default 420
  --fps N                   the stream rate (REMOTE-1's refresh), default 60
  --seconds S               how long to run (default 10)
  A stand-in for core's budget (protocol.md 8.1), read before every frame:
  --kbps N                  the controller's target (default 50000)
  --bottleneck-kbps N       C_est: the link is a measured bottleneck
  --critical                every frame latency-critical (core saw input): capped at 0.65 x C_est
  --user-cap                the client set a bandwidth cap: refinement stops at the luma bar (QP 34)
  --split-ok                core's split_ok (no bottleneck, 3 s without a queue: a LAN; splits Interactive frames only)
  --thin T,D,KBPS           hedging: the thin stream from T s for D s at KBPS
  --lose PTS[,PTS...]       these frames are lost: predict_from(PTS - 1) two frames later
  media's switches:
  --no-damage-qp | --no-refine | --no-hints
  --done-qp N               a cell coded at this base QP or better needs no refinement (default 20)
  --readable-qp N           the luma bar: coded at this base QP or better is readable (default 34)
  --no-focus-refine         on a bottleneck or under 40 Mbit/s, every due cell at the same QP (no ordering by distance)
  --no-readable-first       with ordering, order strictly by distance: the cells near the caret go to
                            lossless before the far ones are readable (default: all readable first)
  --order-spread N          QP offsets from -N (nearest) to +N (farthest) by rank (default 12; 20 was tried, integration.md 6)
  --focus CX,CY,CW,CH[,PX,PY]
                            Media::set_focus 0.4 s in: the caret rect and the pointer (buffer pixels);
                            the plugin's CARET and CURSOR_POS replace them when they arrive
  --max-passes N            refinement passes per change at most (default: the stream rate, a second)
  --refine-delta N          QP delta of refined cells (default 0: the rate controller's QP)
  --skip-delta N            QP delta for unchanged cells (default 30)
  --settle-ms N             default 150
  --input-test              inject a pointer move and Shift (v3 plugin)
  --type TEXT               type TEXT through media.input (v3 plugin) from 0.3 s, a key every
  --type-gap-ms N           N ms (default 70); key times go to --events
  Output:
  --out FILE                the stream (Annex-B / OBUs)
  --lossy-out FILE          the stream a client would decode with --lose (each lost frame and
                            the ones sent before the encoder heard of it left out)
  --stats FILE              one CSV line per frame
  --events FILE             cursor, caret, pointer lock, animation and plugin stats as lines
  --dump DIR                the encoder input of each refined capture (YUV, at its first refinement frame) for SSIM
";

/// What core's Session::video_budget and link_state would say (protocol.md 8.1).
struct FakeCore {
    user_cap: bool,
    target: u64,
    c_est: Option<u64>,
    critical: bool,
    split_ok: bool,
    thin: Option<u64>,
    fps: u32,
}

impl FakeCore {
    fn budget(&self) -> RateInput {
        let mut bps = self.target;
        if let Some(t) = self.thin {
            bps = bps.min(t);
        } else if let (true, Some(c)) = (self.critical, self.c_est) {
            bps = bps.min((c as f64 * 0.65) as u64);
        }
        // the fresh interval's budget (core's LinkStatus.budget): media takes off what it sent
        RateInput {
            encoder_bps: bps,
            latency_critical: self.critical,
            thin: self.thin.is_some(),
            interval_remaining_bytes: (self.target / 8 / self.fps.max(1) as u64) as usize,
            c_est_bps: self.c_est,
            split_ok: self.split_ok && self.c_est.is_none(),
            user_cap: self.user_cap,
            demand_bps: 0, // a fixed rate: ordering at integration.md 7's 40 Mbit/s
        }
    }
}

struct Opts {
    cfg: MediaConfig,
    focus: Option<(Rect, Option<(i32, i32)>)>,
    seconds: f64,
    core: FakeCore,
    thin: Option<(f64, f64, u64)>,
    lose: BTreeSet<u64>,
    input_test: bool,
    type_text: Option<(String, f64, u64)>,
    out: Option<PathBuf>,
    lossy_out: Option<PathBuf>,
    stats: Option<PathBuf>,
    events: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Opts> {
    let mut o = Opts {
        cfg: MediaConfig::default(),
        focus: None,
        seconds: 10.0,
        core: FakeCore {
            user_cap: false,
            target: 50_000_000,
            c_est: None,
            critical: false,
            split_ok: false,
            thin: None,
            fps: 60,
        },
        thin: None,
        lose: BTreeSet::new(),
        input_test: false,
        type_text: None,
        out: None,
        lossy_out: None,
        stats: None,
        events: None,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--socket" => o.cfg.socket = val()?.into(),
            "--output" => o.cfg.output = val()?,
            "--codec" => o.cfg.codec = if val()? == "av1" { Codec::Av1 } else { Codec::Hevc },
            "--chroma" => o.cfg.chroma444 = val()? == "444",
            "--fps" => o.cfg.fps = val()?.parse()?,
            "--seconds" => o.seconds = val()?.parse()?,
            "--kbps" => o.core.target = val()?.parse::<u64>()? * 1000,
            "--bottleneck-kbps" => o.core.c_est = Some(val()?.parse::<u64>()? * 1000),
            "--critical" => o.core.critical = true,
            "--user-cap" => o.core.user_cap = true,
            "--split-ok" => o.core.split_ok = true,
            "--thin" => {
                let v: Vec<f64> = val()?.split(',').map(|s| s.parse()).collect::<Result<_, _>>()?;
                anyhow::ensure!(v.len() == 3, "--thin T,D,KBPS");
                o.thin = Some((v[0], v[1], (v[2] * 1000.0) as u64));
            }
            "--lose" => o.lose = val()?.split(',').map(|s| s.parse()).collect::<Result<_, _>>()?,
            "--no-damage-qp" => o.cfg.damage_qp = false,
            "--no-refine" => o.cfg.refinement = false,
            "--no-hints" => o.cfg.motion_hints = false,
            "--done-qp" => o.cfg.refine.done_qp = val()?.parse()?,
            "--readable-qp" => o.cfg.refine.readable_qp = val()?.parse()?,
            "--no-focus-refine" => o.cfg.focus_refine = false,
            "--no-readable-first" => o.cfg.refine.readable_first = false,
            "--order-spread" => o.cfg.refine.order_spread = val()?.parse()?,
            "--focus" => {
                let v: Vec<i32> = val()?.split(',').map(|s| s.parse()).collect::<Result<_, _>>()?;
                anyhow::ensure!(v.len() == 4 || v.len() == 6, "--focus CX,CY,CW,CH[,PX,PY]");
                o.focus = Some((Rect { x: v[0], y: v[1], w: v[2], h: v[3] }, (v.len() == 6).then(|| (v[4], v[5]))));
            }
            "--max-passes" => o.cfg.refine.max_passes = val()?.parse()?,
            "--refine-delta" => o.cfg.refine.refine_delta = val()?.parse()?,
            "--skip-delta" => o.cfg.refine.skip_delta = val()?.parse()?,
            "--settle-ms" => o.cfg.refine.settle_ns = val()?.parse::<u64>()? * 1_000_000,
            "--input-test" => o.input_test = true,
            "--type" => o.type_text = Some((val()?, 0.3, 70)),
            "--type-gap-ms" => o.type_text.as_mut().context("--type first")?.2 = val()?.parse()?,
            "--out" => o.out = Some(val()?.into()),
            "--lossy-out" => o.lossy_out = Some(val()?.into()),
            "--stats" => o.stats = Some(val()?.into()),
            "--events" => o.events = Some(val()?.into()),
            "--dump" => o.cfg.dump = Some(val()?.into()),
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            _ => bail!("unknown option {a}\n\n{USAGE}"),
        }
    }
    o.core.fps = o.cfg.fps;
    o.cfg.start_bps = o.core.target;
    Ok(o)
}

fn create(p: &Option<PathBuf>) -> Result<Option<BufWriter<File>>> {
    p.as_ref()
        .map(|p| File::create(p).with_context(|| format!("creating {}", p.display())).map(BufWriter::new))
        .transpose()
}

/// US-layout evdev code for a character, and whether it needs Shift.
fn evdev_key(c: char) -> Option<(u32, bool)> {
    const LETTERS: [u32; 26] =
        [30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44];
    Some(match c {
        'a'..='z' => (LETTERS[c as usize - 'a' as usize], false),
        'A'..='Z' => (LETTERS[c as usize - 'A' as usize], true),
        '1'..='9' => (c as u32 - '1' as u32 + 2, false),
        '0' => (11, false),
        ' ' => (57, false),
        ',' => (51, false),
        '.' => (52, false),
        '\n' => (28, false),
        _ => return None,
    })
}

fn class_name(c: FrameClass) -> &'static str {
    match c {
        FrameClass::Interactive => "interactive",
        FrameClass::Passive => "passive",
        FrameClass::Refine => "refine",
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let o = parse(&args)?;
    let (tx, rx) = mpsc::channel();
    let media = Media::start(o.cfg.clone(), Arc::new(move |ev| drop(tx.send(ev))))?;
    let st = media.stats();
    eprintln!(
        "remoted-media: plugin capture-ipc v{} on Hyprland {}, {} {} @{} Hz",
        st.plugin_version,
        st.hyprland,
        if o.cfg.codec == Codec::Av1 { "AV1" } else { "HEVC" },
        if o.cfg.chroma444 { "4:4:4" } else { "4:2:0" },
        o.cfg.fps
    );
    let mut focus = o.focus;
    let core = Arc::new(Mutex::new(o.core));
    {
        let core = core.clone();
        media.set_rate_source(Arc::new(move || core.lock().unwrap().budget()));
    }

    let mut out = create(&o.out)?;
    let mut lossy = create(&o.lossy_out)?;
    let mut stats = create(&o.stats)?;
    let mut events = create(&o.events)?;
    if let Some(f) = stats.as_mut() {
        writeln!(
            f,
            "pts,class,key,seq,t_rendered_ns,t_ready_ns,t_encoded_ns,latency_ms,convert_ms,encode_ms,bytes,qp,pic_type,changed,refined,cells,hinted,merged,kbps,split,ir,invalidated,reference,recovery,base_qp,readable_ms,lossless_ms,settled,settled_ms,rects"
        )?;
    }

    if o.input_test {
        let now = monotonic_ns();
        media.input(InputEvent::PointerAbs { x: 2400.0, y: 1500.0, t_ns: now })?;
        media.input(InputEvent::Key { keycode: 42, pressed: true, t_ns: now })?; // KEY_LEFTSHIFT
        media.input(InputEvent::Key { keycode: 42, pressed: false, t_ns: now })?;
    }

    let start = Instant::now();
    let deadline = start + Duration::from_secs_f64(o.seconds);
    let mut thin_on = false;
    let mut lose_at: VecDeque<(u64, u64)> = VecDeque::new(); // (report once this pts is encoded, lost pts)
    let mut dropping = false;
    let mut last_decoded: Option<u64> = None;
    let (mut n, mut bytes, mut by_class) = (0u64, 0u64, [0u64; 3]);
    let (mut shapes, mut cursor_pos, mut stopped) = (0, 0, None);
    // --type: key presses and releases through media.input, as a session would inject them
    let mut keys: VecDeque<(Instant, u32, bool)> = VecDeque::new();
    if let Some((text, at, gap)) = &o.type_text {
        let mut t = start + Duration::from_secs_f64(*at);
        for ch in text.chars() {
            let Some((code, shift)) = evdev_key(ch) else { continue };
            if shift {
                keys.push_back((t, 42, true));
            }
            keys.push_back((t, code, true));
            keys.push_back((t + Duration::from_millis(30), code, false));
            if shift {
                keys.push_back((t + Duration::from_millis(31), 42, false));
            }
            t += Duration::from_millis(*gap);
        }
    }
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        // after the plugin's first CARET and CURSOR_POS (visible = false), which would replace it
        if let Some((caret, pointer)) = focus.filter(|_| now - start > Duration::from_millis(400)) {
            media.set_focus(Some(caret), pointer);
            focus = None;
        }
        while keys.front().is_some_and(|k| k.0 <= now) {
            let (_, code, pressed) = keys.pop_front().unwrap();
            let t_ns = monotonic_ns();
            media.input(InputEvent::Key { keycode: code, pressed, t_ns })?;
            if let Some(w) = events.as_mut() {
                writeln!(w, "{t_ns} key {code} {}", if pressed { "down" } else { "up" })?;
            }
        }
        let wait = keys.front().map_or(Duration::from_millis(20), |k| k.0.saturating_duration_since(now)).min(Duration::from_millis(20));
        if let Some((t, d, kbps)) = o.thin {
            let s = (now - start).as_secs_f64();
            let want = s >= t && s < t + d;
            if want != thin_on {
                thin_on = want;
                core.lock().unwrap().thin = want.then_some(kbps);
                eprintln!("remoted-media: thin {} at {s:.2} s", if want { "on" } else { "off" });
            }
        }
        let ev = match rx.recv_timeout((deadline - now).min(wait)) {
            Ok(ev) => ev,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let t = monotonic_ns();
        match ev {
            MediaEvent::Frame(f) => {
                n += 1;
                bytes += f.data.len() as u64;
                by_class[f.class.wire() as usize] += f.data.len() as u64;
                if let Some(w) = out.as_mut() {
                    w.write_all(&f.data)?;
                }
                // Simulated loss: the client drops the frame and everything after it until
                // a frame that predicts from what it has (or a keyframe or recovery point).
                if o.lose.contains(&f.pts) {
                    lose_at.push_back((f.pts + 2, f.pts));
                    dropping = true;
                }
                if dropping && !o.lose.contains(&f.pts) && (f.reference.is_some() || f.keyframe || f.recovery_point) {
                    dropping = false;
                }
                if !dropping {
                    last_decoded = Some(f.pts);
                    if let Some(w) = lossy.as_mut() {
                        w.write_all(&f.data)?;
                    }
                }
                // core's RefHint::Reference(last frame the client decoded), about two frames later
                while lose_at.front().is_some_and(|&(at, _)| f.pts >= at) {
                    lose_at.pop_front();
                    if let Some(p) = last_decoded {
                        media.predict_from(p);
                    }
                }
                if let Some(w) = stats.as_mut() {
                    let i = &f.info;
                    let rects: Vec<String> = f.meta[4..]
                        .chunks_exact(8)
                        .map(|c| {
                            let v = |k: usize| u16::from_be_bytes([c[k], c[k + 1]]);
                            format!("{}:{}:{}:{}", v(0), v(2), v(4), v(6))
                        })
                        .collect();
                    writeln!(
                        w,
                        "{},{},{},{},{},{},{},{:.3},{:.3},{:.3},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                        f.pts,
                        class_name(f.class),
                        f.keyframe as u8,
                        i.seq,
                        f.t_rendered_ns,
                        i.t_ready_ns,
                        f.t_encoded_ns,
                        (f.t_encoded_ns.saturating_sub(f.t_rendered_ns)) as f64 / 1e6,
                        i.convert_ms,
                        i.encode_ms,
                        f.data.len(),
                        i.avg_qp,
                        i.pic_type,
                        i.changed_cells,
                        i.refined_cells,
                        i.cells,
                        i.hinted_blocks,
                        i.merged,
                        i.kbps,
                        i.split as u8,
                        i.intra_refresh as u8,
                        i.invalidated,
                        f.reference.map_or(-1, |r| r as i64),
                        f.recovery_point as u8,
                        i.base_qp,
                        i.readable_ms.map_or(String::new(), |v| format!("{v:.1}")),
                        i.lossless_ms.map_or(String::new(), |v| format!("{v:.1}")),
                        if i.lossless_ms.is_some() { "stop" } else { i.settled_end.map_or("", |e| e.as_str()) },
                        i.settled_ms.map_or(String::new(), |v| format!("{v:.1}")),
                        rects.join(" ")
                    )?;
                }
            }
            MediaEvent::CursorShape(s) => {
                shapes += 1;
                if let Some(w) = events.as_mut() {
                    let i = &s.image;
                    writeln!(w, "{t} shape {:016x} {}x{} hot {},{} scale {} pixels {} B", s.hash, i.width, i.height, i.hot_x, i.hot_y, i.scale, i.pixels.len())?;
                }
            }
            MediaEvent::CursorPos { x, y, visible, shape, t_ns } => {
                cursor_pos += 1;
                if let Some(w) = events.as_mut() {
                    writeln!(w, "{t} pos {x},{y} visible {visible} shape {shape:016x} t {t_ns}")?;
                }
            }
            MediaEvent::Caret { visible, purpose, rect, t_ns } => {
                if let Some(w) = events.as_mut() {
                    writeln!(w, "{t} caret visible {visible} purpose {purpose} {rect:?} t {t_ns}")?;
                }
            }
            MediaEvent::PointerLock { state, rect } => {
                if let Some(w) = events.as_mut() {
                    writeln!(w, "{t} pointer_lock {state} {rect:?}")?;
                }
            }
            MediaEvent::Animation(a) => {
                if let Some(w) = events.as_mut() {
                    writeln!(w, "{t} animation {a}")?;
                }
            }
            MediaEvent::PluginStats { frames, dropped_no_buffer, dropped_no_client } => {
                if let Some(w) = events.as_mut() {
                    writeln!(w, "{t} stats frames {frames} no_buffer {dropped_no_buffer} no_client {dropped_no_client}")?;
                }
            }
            MediaEvent::Stopped(why) => {
                stopped = Some(why);
                break;
            }
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let st = media.stats();
    media.stop();
    for w in [out.as_mut(), lossy.as_mut(), stats.as_mut(), events.as_mut()].into_iter().flatten() {
        w.flush()?;
    }
    println!(
        "frames {n} in {secs:.2} s: {:.1} kbit/s; passive {} B, interactive {} B, refine {} B; keyframes {}, invalidations {}, intra refreshes {}",
        bytes as f64 * 8.0 / secs / 1e3,
        by_class[0],
        by_class[1],
        by_class[2],
        st.keyframes,
        st.invalidations,
        st.intra_refreshes
    );
    println!("cursor: {shapes} shapes, {cursor_pos} positions; plugin stats {:?}", st.plugin);
    println!("settle: readable {:?} ms, lossless {:?} ms, settled {:?} ms {:?} (the latest change that needed refinement); empty slice {} B", st.readable_ms, st.lossless_ms, st.settled_ms, st.settled_end, st.empty_slice_bytes);
    if let Some(why) = stopped {
        bail!("media stopped: {why}");
    }
    Ok(())
}
