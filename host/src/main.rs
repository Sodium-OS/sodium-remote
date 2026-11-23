// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! remoted: Remote's host daemon.
//!
//! - `remoted run`: the daemon: the session lifecycle, the
//!   display manager, input, the IME bridge, audio, clipboard, the device key and
//!   the `org.sodiumos.Remote` D-Bus API.
//! - `remoted capture`: the milestone 0 video path: capture (plugin socket or
//!   ext-image-copy-capture) -> Vulkan DMA-BUF import -> compute colour conversion
//!   -> NVENC through CUDA interop -> an elementary stream on disk, with per-stage
//!   timings. See remote/docs/reference/video-pipeline.md.
//! - `remoted id`: this host's device ID and its code (creates `device.key` on first use).
//!
//! Everything lives in the `remoted` library (src/lib.rs); this is the command line.


use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    time::Instant,
};

use anyhow::{Context, Result, bail};

use remoted::{
    capture::{self, CaptureSource, monotonic_ns, plugin::PluginSource, wayland::WaylandSource},
    daemon, gpu,
    gpu::{Gpu, OutFormat, SrcImage},
    keys, nvenc,
    nvenc::{Cuda, EncParams, Encoder, ExtMem},
};

const USAGE: &str = "\
remoted run [options]       the daemon: see `remoted run --help`

remoted id                  print this host's device ID and its code (XXXX-XXXX)

remoted capture [options]   capture an output, encode it, write the stream

  --source wayland|plugin   ext-image-copy-capture (default) or the sodiumwm plugin socket
  --output NAME             output to capture (default: the first; REMOTE-1 for the plugin)
  --socket PATH             plugin socket (default $XDG_RUNTIME_DIR/sodiumos/remote-capture.sock)
  --frames N                frames to encode (default 300)
  --codec hevc|av1          default hevc
  --chroma 420|444          default 420 (444 is HEVC only)
  --input yuv|argb          yuv: our compute shader converts (default); argb: NVENC converts
  --kbps N                  CBR target (default 50000)
  --fps N                   rate-control frame rate; match the output's refresh (default 60)
  --split on|off            forced 2-way split-frame encoding (default on)
  --out FILE                elementary stream: Annex-B HEVC or AV1 OBUs
  --stats FILE              per-frame timings as CSV
  --dump-source DIR         also write every Kth captured frame (raw BGRx) for quality checks
  --dump-every K            default 10
";

#[derive(Debug)]
struct Opts {
    plugin: bool,
    output: Option<String>,
    socket: PathBuf,
    frames: u64,
    av1: bool,
    chroma444: bool,
    argb: bool,
    kbps: u32,
    fps: u32,
    split: bool,
    out: Option<PathBuf>,
    stats: Option<PathBuf>,
    dump_dir: Option<PathBuf>,
    dump_every: u64,
}

fn parse(args: &[String]) -> Result<Opts> {
    let mut o = Opts {
        plugin: false,
        output: None,
        socket: capture::plugin::default_socket(),
        frames: 300,
        av1: false,
        chroma444: false,
        argb: false,
        kbps: 50_000,
        fps: 60,
        split: true,
        out: None,
        stats: None,
        dump_dir: None,
        dump_every: 10,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--source" => o.plugin = match val()?.as_str() {
                "plugin" => true,
                "wayland" => false,
                v => bail!("unknown source {v}"),
            },
            "--output" => o.output = Some(val()?),
            "--socket" => o.socket = val()?.into(),
            "--frames" => o.frames = val()?.parse()?,
            "--codec" => o.av1 = match val()?.as_str() {
                "av1" => true,
                "hevc" => false,
                v => bail!("unknown codec {v}"),
            },
            "--chroma" => o.chroma444 = val()? == "444",
            "--input" => o.argb = val()? == "argb",
            "--kbps" => o.kbps = val()?.parse()?,
            "--fps" => o.fps = val()?.parse()?,
            "--split" => o.split = val()? != "off",
            "--out" => o.out = Some(val()?.into()),
            "--stats" => o.stats = Some(val()?.into()),
            "--dump-source" => o.dump_dir = Some(val()?.into()),
            "--dump-every" => o.dump_every = val()?.parse()?,
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            _ => bail!("unknown option {a}\n\n{USAGE}"),
        }
    }
    if o.av1 && o.chroma444 {
        bail!("AV1 on NVENC is 4:2:0 only");
    }
    Ok(o)
}

/// Per-frame measurements, all in milliseconds.
struct Sample {
    seq: u64,
    capture: f64,  // compositor done -> remoted has the frame
    record: f64,   // record and submit the GPU pass
    convert: f64,  // GPU pass (waits on the acquire fence first)
    encode: f64,   // nvEncEncodePicture -> bitstream ready
    total: f64,    // compositor done -> bitstream ready
    interval: f64, // since the previous frame arrived
    bytes: usize,
    pic_type: i32,
    qp: u32,
    damage_pct: f64, // share of the frame the source reported as damaged
}

struct Encoding {
    enc: Encoder,
    outs: Vec<(gpu::OutBuffer, ExtMem, i32)>,
    width: u32,
    height: u32,
}

fn open_encoding(gpu: &Gpu, cuda: &Cuda, o: &Opts, width: u32, height: u32) -> Result<Encoding> {
    let format = if o.argb {
        OutFormat::Argb
    } else if o.chroma444 {
        OutFormat::Yuv444
    } else {
        OutFormat::Nv12
    };
    let p = EncParams {
        codec: if o.av1 { nvenc::CODEC_AV1 } else { nvenc::CODEC_HEVC },
        input: match format {
            OutFormat::Nv12 => nvenc::INPUT_NV12,
            OutFormat::Yuv444 => nvenc::INPUT_YUV444,
            OutFormat::Argb => nvenc::INPUT_ARGB,
        },
        chroma444: o.chroma444 as i32,
        width,
        height,
        fps: o.fps,
        kbps: o.kbps,
        vbv_pct: 100,
        split: if o.split { nvenc::SPLIT_TWO_FORCED } else { nvenc::SPLIT_OFF },
    };
    let mut enc = Encoder::open(cuda, &p)?;
    let mut outs = Vec::new();
    for _ in 0..2 {
        let out = gpu.create_out(width, height, format)?;
        let mem = cuda.import(gpu.export_fd(&out)?, out.alloc_size)?;
        let slot = enc.register(mem.devptr, out.pitch)?;
        outs.push((out, mem, slot));
    }
    Ok(Encoding { enc, outs, width, height })
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((p * v.len() as f64).ceil() as usize).clamp(1, v.len()) - 1]
}

fn capture(o: Opts) -> Result<()> {
    let mut gpu = Gpu::new()?;
    let cuda = Cuda::open(&gpu.uuid)?;
    let mut src: Box<dyn CaptureSource> = if o.plugin {
        let out = o.output.as_deref().unwrap_or("REMOTE-1");
        let s = PluginSource::connect(&o.socket, out, 3, 0)?;
        eprintln!("remoted: plugin capture of {out} (Hyprland {})", s.hyprland_version);
        Box::new(s)
    } else {
        let s = WaylandSource::new(o.output.as_deref(), 3, |f| gpu.sampleable_modifiers(f))?;
        eprintln!("remoted: ext-image-copy-capture of {} (GBM on {})", s.output_name, s.gbm_node.display());
        Box::new(s)
    };
    eprintln!("remoted: Vulkan and CUDA on {}", gpu.name);

    let mut file = match &o.out {
        Some(p) => Some(BufWriter::new(File::create(p).with_context(|| format!("creating {}", p.display()))?)),
        None => None,
    };
    let mut stats_file = match &o.stats {
        Some(p) => {
            let mut f = BufWriter::new(File::create(p)?);
            writeln!(f, "frame,seq,t_rendered_ns,t_ready_ns,capture_ms,record_ms,convert_ms,encode_ms,total_ms,interval_ms,bytes,pic_type,qp,damage_pct")?;
            Some(f)
        }
        None => None,
    };
    if let Some(d) = &o.dump_dir {
        std::fs::create_dir_all(d)?;
    }

    let mut images: Vec<SrcImage> = Vec::new();
    let mut generation = None;
    let mut import_ms = Vec::new();
    let mut encoding: Option<Encoding> = None;
    let mut readback = None;
    let mut samples = Vec::new();
    let mut last_ready = None;
    let t_start = Instant::now();

    for n in 0..o.frames {
        let mut frame = src.next_frame()?;
        let (g, bufs) = src.buffers();
        if generation != Some(g) {
            for img in images.drain(..) {
                gpu.destroy_src(img);
            }
            for b in bufs {
                let t = Instant::now();
                images.push(gpu.import(b)?);
                import_ms.push(t.elapsed().as_secs_f64() * 1e3);
            }
            generation = Some(g);
            let (w, h) = (bufs[0].width, bufs[0].height);
            eprintln!(
                "remoted: generation {g}: {} buffers {w}x{h}, format {:#x}, modifier {:#x}, {} plane(s)",
                bufs.len(),
                bufs[0].format,
                bufs[0].modifier,
                bufs[0].planes.len()
            );
            if encoding.as_ref().is_none_or(|e| (e.width, e.height) != (w, h)) {
                if let Some(e) = encoding.take() {
                    for (out, mem, _) in e.outs {
                        drop(mem);
                        gpu.destroy_out(out);
                    }
                }
                encoding = Some(open_encoding(&gpu, &cuda, &o, w, h)?);
            }
            if o.dump_dir.is_some() && readback.is_none() {
                readback = Some(gpu.create_readback((w * h * 4) as usize)?);
            }
        }
        let e = encoding.as_mut().unwrap();
        let (out, _, slot) = &e.outs[(n % 2) as usize];
        let dump = o.dump_dir.is_some() && n % o.dump_every == 0;
        let ct = gpu.convert(
            &images[frame.buffer as usize],
            out,
            frame.acquire.take(),
            if dump { readback.as_ref() } else { None },
            None,
        )?;
        src.release(&frame)?;
        if dump {
            let path = o.dump_dir.as_ref().unwrap().join(format!("src_{n:05}.bgrx"));
            std::fs::write(&path, readback.as_ref().unwrap().bytes())?;
        }
        let slot = *slot;
        let pkt = e.enc.encode(slot, n, n == 0)?;
        let t_done = monotonic_ns();
        if let Some(f) = file.as_mut() {
            f.write_all(pkt.data)?;
        }
        let ms = |a: u64, b: u64| (b.saturating_sub(a)) as f64 / 1e6;
        let s = Sample {
            seq: frame.seq,
            capture: ms(frame.t_rendered_ns, frame.t_ready_ns),
            record: ct.record_ms,
            convert: ct.gpu_ms,
            encode: pkt.ms,
            total: ms(frame.t_rendered_ns, t_done),
            interval: last_ready.map_or(f64::NAN, |l| ms(l, frame.t_ready_ns)),
            bytes: pkt.data.len(),
            pic_type: pkt.pic_type,
            qp: pkt.avg_qp,
            damage_pct: 100.0 * frame.damage.iter().map(|r| r.w as f64 * r.h as f64).sum::<f64>().min((e.width * e.height) as f64)
                / (e.width * e.height) as f64,
        };
        last_ready = Some(frame.t_ready_ns);
        if let Some(f) = stats_file.as_mut() {
            writeln!(
                f,
                "{n},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{},{},{},{:.1}",
                s.seq, frame.t_rendered_ns, frame.t_ready_ns, s.capture, s.record, s.convert, s.encode, s.total, s.interval, s.bytes, s.pic_type, s.qp, s.damage_pct
            )?;
        }
        samples.push(s);
    }
    if let Some(f) = file.as_mut() {
        f.flush()?;
    }
    if let Some(f) = stats_file.as_mut() {
        f.flush()?;
    }
    let secs = t_start.elapsed().as_secs_f64();

    // Summary (the first frame, an IDR with cold caches, is left out).
    let steady = || samples.iter().skip(1);
    let col = |f: fn(&Sample) -> f64| -> Vec<f64> { steady().map(f).filter(|v| v.is_finite()).collect() };
    println!(
        "frames {} in {:.2} s ({:.1} fps), {:.1} Mbit/s",
        samples.len(),
        secs,
        samples.len() as f64 / secs,
        samples.iter().map(|s| s.bytes).sum::<usize>() as f64 * 8.0 / secs / 1e6
    );
    println!("import (once per buffer): {:?} ms", import_ms.iter().map(|v| format!("{v:.2}")).collect::<Vec<_>>());
    println!("stage            p50 ms   p95 ms   max ms");
    for (name, f) in [
        ("capture", (|s: &Sample| s.capture) as fn(&Sample) -> f64),
        ("record+submit", |s: &Sample| s.record),
        ("convert (GPU)", |s: &Sample| s.convert),
        ("encode", |s: &Sample| s.encode),
        ("total", |s: &Sample| s.total),
        ("frame interval", |s: &Sample| s.interval),
    ] {
        let mut v = col(f);
        println!("{name:<16} {:>7.3}  {:>7.3}  {:>7.3}", pct(&mut v, 0.5), pct(&mut v, 0.95), pct(&mut v, 1.0));
    }

    for img in images {
        gpu.destroy_src(img);
    }
    if let Some(e) = encoding {
        drop(e.enc);
        for (out, mem, _) in e.outs {
            drop(mem);
            gpu.destroy_out(out);
        }
    }
    if let Some(r) = readback {
        gpu.destroy_readback(r);
    }
    drop(src);
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("capture") => capture(parse(&args[1..])?),
        Some("run") => {
            if args[1..].iter().any(|a| a == "-h" || a == "--help") {
                print!("{}", daemon::RUN_USAGE);
                return Ok(());
            }
            daemon::run(daemon::parse_run(&args[1..])?)
        }
        Some("id") => {
            let id = keys::DeviceKey::load_or_create(&keys::data_dir())?.id();
            let code = id.parse::<remote_core::keys::DeviceId>().map(|d| d.code()).unwrap_or_default();
            println!("{id}\n{code}");
            Ok(())
        }
        _ => {
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    }
}
