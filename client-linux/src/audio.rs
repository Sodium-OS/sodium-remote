// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Audio out: the core's jitter buffer (`Client::audio_buffer`, 10 ms pops) →
//! Opus (the system's libopus; stereo, 48 kHz) → a PipeWire playback stream
//! (`csrc/audio_out.c`) that pulls, about 10 ms at a time. The Windows client
//! does the same through WASAPI (`client-windows/src/audio.rs`).
//!
//! The level (RMS of what was played, dBFS) is logged to stderr about every
//! 2 s once sound arrives and, with `--audio-log FILE`, to a file once a
//! second, so an unattended run can check that sound arrived.

use remote_core::audio::AudioPlay;
use remote_engine::App;
use std::ffi::{CString, c_void};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn opus_decoder_create(fs: i32, channels: i32, error: *mut i32) -> *mut c_void;
    fn opus_decode_float(d: *mut c_void, data: *const u8, len: i32, pcm: *mut f32, frame_size: i32, fec: i32) -> i32;
    fn opus_decoder_destroy(d: *mut c_void);
}

#[repr(C)]
struct AoStream {
    _p: [u8; 0],
}
type FillFn = unsafe extern "C" fn(user: *mut c_void, out: *mut f32, frames: i32);
unsafe extern "C" {
    fn ao_open(app_name: *const i8, app_id: *const i8, icon: *const i8, fill: FillFn, user: *mut c_void, err: *mut i32) -> *mut AoStream;
    fn ao_close(s: *mut AoStream);
}

const RATE: u32 = 48_000;
/// 10 ms, the jitter buffer's pop.
const FRAME: usize = 480;

#[derive(Clone, Default)]
pub struct AudioOptions {
    pub enabled: bool,
    pub log: Option<PathBuf>,
}

struct Opus(*mut c_void);
unsafe impl Send for Opus {}

impl Opus {
    fn new() -> Option<Opus> {
        let mut e = 0;
        let d = unsafe { opus_decoder_create(RATE as i32, 2, &mut e) };
        (e == 0 && !d.is_null()).then_some(Opus(d))
    }
    /// 10 ms of stereo float; `data` None conceals a lost frame.
    fn decode(&mut self, data: Option<&[u8]>, fec: bool, out: &mut [f32]) -> bool {
        let (p, n) = data.map_or((std::ptr::null(), 0), |d| (d.as_ptr(), d.len() as i32));
        unsafe { opus_decode_float(self.0, p, n, out.as_mut_ptr(), FRAME as i32, fec as i32) == FRAME as i32 }
    }
}

impl Drop for Opus {
    fn drop(&mut self) {
        unsafe { opus_decoder_destroy(self.0) }
    }
}

/// What the PipeWire thread pulls from.
struct Feed {
    app: Arc<App>,
    opus: Opus,
    pcm: [f32; FRAME * 2],
    /// Samples of `pcm` already played.
    used: usize,
    sum_sq: f64,
    n: u64,
    decoded: u64,
    loud: u64,
    started: Instant,
    last_log: Instant,
    last_file: Instant,
    log: Option<std::fs::File>,
}

impl Feed {
    /// The next 10 ms from the jitter buffer, silence when there is none.
    fn refill(&mut self) {
        let got = self.app.client.audio_buffer().and_then(|b| b.lock().unwrap().pop(Instant::now()));
        let ok = match got {
            Some(a) => match &a.play {
                AudioPlay::Packet(d) => self.opus.decode(Some(d), false, &mut self.pcm),
                AudioPlay::Fec(d) => self.opus.decode(Some(d), true, &mut self.pcm),
                AudioPlay::Conceal => self.opus.decode(None, false, &mut self.pcm),
            },
            None => false,
        };
        if ok {
            self.decoded += 1;
        } else {
            self.pcm.fill(0.0);
        }
        let e: f64 = self.pcm.iter().map(|&s| (s as f64) * (s as f64)).sum();
        self.sum_sq += e;
        self.n += self.pcm.len() as u64;
        if e > 0.0 {
            self.loud += 1;
        }
        self.used = 0;
    }

    fn fill(&mut self, out: &mut [f32]) {
        let mut i = 0;
        while i < out.len() {
            if self.used == self.pcm.len() {
                self.refill();
            }
            let k = (out.len() - i).min(self.pcm.len() - self.used);
            out[i..i + k].copy_from_slice(&self.pcm[self.used..self.used + k]);
            self.used += k;
            i += k;
        }
        self.report();
    }

    fn report(&mut self) {
        let rms = if self.n > 0 { (self.sum_sq / self.n as f64).sqrt() } else { 0.0 };
        let db = if rms > 0.0 { 20.0 * rms.log10() } else { -120.0 };
        if self.last_file.elapsed() >= Duration::from_secs(1)
            && let Some(f) = self.log.as_mut()
        {
            let _ = writeln!(f, "{:.1},{:.2},{},{}", self.started.elapsed().as_secs_f64(), db, self.decoded, self.loud);
            let _ = f.flush();
            self.last_file = Instant::now();
        }
        if self.last_log.elapsed() >= Duration::from_secs(2) {
            if self.decoded > 0 || self.loud > 0 {
                eprintln!("remote-client: audio: {} frames decoded, {} non-silent, level {db:.1} dBFS", self.decoded, self.loud);
            }
            self.last_log = Instant::now();
            self.sum_sq = 0.0;
            self.n = 0;
        }
    }
}

unsafe extern "C" fn fill(user: *mut c_void, out: *mut f32, frames: i32) {
    let feed = unsafe { &mut *(user as *mut Feed) };
    let out = unsafe { std::slice::from_raw_parts_mut(out, frames.max(0) as usize * 2) };
    feed.fill(out);
}

/// Plays the session's sound until the app quits, on a thread of its own.
pub fn start(app: &Arc<App>, opts: &AudioOptions) {
    if !opts.enabled {
        return;
    }
    let (app, log) = (app.clone(), opts.log.clone());
    let _ = std::thread::Builder::new().name("audio".into()).spawn(move || {
        if let Err(e) = run(app, log) {
            eprintln!("remote-client: audio: {e}");
        }
    });
}

fn run(app: Arc<App>, log: Option<PathBuf>) -> Result<(), String> {
    let opus = Opus::new().ok_or("no Opus decoder")?;
    let now = Instant::now();
    let feed = Box::into_raw(Box::new(Feed {
        app: app.clone(),
        opus,
        pcm: [0.0; FRAME * 2],
        used: FRAME * 2,
        sum_sq: 0.0,
        n: 0,
        decoded: 0,
        loud: 0,
        started: now,
        last_log: now,
        last_file: now,
        log: log.and_then(|p| std::fs::File::create(p).ok()),
    }));
    // The volume mixer shows the app as the launcher does: "Remote", with its icon.
    let (name, id, icon) = (CString::new("Remote").unwrap(), CString::new("org.sodiumos.Remote").unwrap(), CString::new("sodiumshell-remote").unwrap());
    let mut err = 0;
    let s = unsafe { ao_open(name.as_ptr(), id.as_ptr(), icon.as_ptr(), fill, feed.cast(), &mut err) };
    if s.is_null() {
        drop(unsafe { Box::from_raw(feed) });
        return Err(format!("PipeWire: {}", std::io::Error::from_raw_os_error(-err)));
    }
    eprintln!("remote-client: audio: PipeWire, {RATE} Hz stereo float, 10 ms quantum");
    while !app.quitting() {
        std::thread::sleep(Duration::from_millis(100));
    }
    unsafe {
        ao_close(s);
        drop(Box::from_raw(feed));
    }
    Ok(())
}
