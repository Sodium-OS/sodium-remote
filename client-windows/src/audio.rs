// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Audio out: the core's jitter buffer (`Client::audio_buffer`, 10 ms pops) →
//! Opus (the static libopus; stereo, 48 kHz) → a WASAPI shared-mode render
//! stream on the default device, event driven, with a ~30 ms buffer.
//!
//! The level (RMS of what was handed to the device, dBFS) is logged to stderr
//! about every 2 s and, with `--audio-log FILE`, to a file once a second, so
//! an unattended run can check that sound arrived.

use crate::platform::AudioOptions;
use remote_core::audio::AudioPlay;
use remote_engine::App;
use std::ffi::c_void;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
    eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

unsafe extern "C" {
    fn opus_decoder_create(fs: i32, channels: i32, error: *mut i32) -> *mut c_void;
    fn opus_decode_float(d: *mut c_void, data: *const u8, len: i32, pcm: *mut f32, frame_size: i32, fec: i32) -> i32;
    fn opus_decoder_destroy(d: *mut c_void);
}

const RATE: u32 = 48_000;
/// 10 ms, the jitter buffer's pop.
const FRAME: usize = 480;

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
        let r = unsafe { opus_decode_float(self.0, p, n, out.as_mut_ptr(), FRAME as i32, fec as i32) };
        r == FRAME as i32
    }
}

impl Drop for Opus {
    fn drop(&mut self) {
        unsafe { opus_decoder_destroy(self.0) }
    }
}

pub fn start(app: &Arc<App>, opts: &AudioOptions) {
    if !opts.enabled {
        return;
    }
    let (app, log) = (app.clone(), opts.log.clone());
    let _ = std::thread::Builder::new().name("audio".into()).spawn(move || {
        if let Err(e) = run(&app, log) {
            eprintln!("remote-client: audio: {e}");
        }
    });
}

fn run(app: &Arc<App>, log: Option<std::path::PathBuf>) -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let ac: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let fmt = WAVEFORMATEX {
            wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
            nChannels: 2,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        ac.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            300_000, // 30 ms in 100 ns units
            0,
            &fmt,
            None,
        )?;
        let event: HANDLE = CreateEventW(None, false, false, None)?;
        ac.SetEventHandle(event)?;
        let render: IAudioRenderClient = ac.GetService()?;
        let total = ac.GetBufferSize()? as usize;
        // Start with silence in the buffer, then run.
        let first = render.GetBuffer(total as u32)?;
        std::ptr::write_bytes(first, 0, total * 8);
        render.ReleaseBuffer(total as u32, 0)?;
        ac.Start()?;
        eprintln!("remote-client: audio: WASAPI shared, {RATE} Hz stereo float, buffer {total} frames");

        let mut opus = Opus::new().ok_or_else(|| windows::core::Error::from_hresult(windows::core::HRESULT(-1)))?;
        let mut logf = log.and_then(|p| std::fs::File::create(p).ok());
        let mut pcm = [0f32; FRAME * 2];
        let (mut sum_sq, mut n, mut frames_played, mut loud) = (0f64, 0u64, 0u64, 0u64);
        let (mut last_log, mut last_file) = (Instant::now(), Instant::now());
        let started = Instant::now();
        while !app.quitting() {
            WaitForSingleObject(event, 100);
            let pad = ac.GetCurrentPadding()? as usize;
            let mut avail = total.saturating_sub(pad);
            while avail >= FRAME {
                let got = app.client.audio_buffer().and_then(|b| b.lock().unwrap().pop(Instant::now()));
                let ok = match got {
                    Some(a) => match &a.play {
                        AudioPlay::Packet(d) => opus.decode(Some(d), false, &mut pcm),
                        AudioPlay::Fec(d) => opus.decode(Some(d), true, &mut pcm),
                        AudioPlay::Conceal => opus.decode(None, false, &mut pcm),
                    },
                    None => false,
                };
                if !ok {
                    pcm.fill(0.0);
                } else {
                    frames_played += 1;
                }
                let e: f64 = pcm.iter().map(|&s| (s as f64) * (s as f64)).sum();
                sum_sq += e;
                n += pcm.len() as u64;
                if e > 0.0 {
                    loud += 1;
                }
                let dst = render.GetBuffer(FRAME as u32)?;
                std::ptr::copy_nonoverlapping(pcm.as_ptr() as *const u8, dst, FRAME * 8);
                render.ReleaseBuffer(FRAME as u32, 0)?;
                avail -= FRAME;
            }
            let rms = if n > 0 { (sum_sq / n as f64).sqrt() } else { 0.0 };
            let db = if rms > 0.0 { 20.0 * rms.log10() } else { -120.0 };
            if let Some(f) = logf.as_mut()
                && last_file.elapsed() >= Duration::from_secs(1)
            {
                let _ = writeln!(f, "{:.1},{:.2},{},{}", started.elapsed().as_secs_f64(), db, frames_played, loud);
                let _ = f.flush();
                last_file = Instant::now();
            }
            if last_log.elapsed() >= Duration::from_secs(2) {
                if frames_played > 0 || loud > 0 {
                    eprintln!("remote-client: audio: {frames_played} frames decoded, {loud} non-silent, level {db:.1} dBFS");
                }
                last_log = Instant::now();
                sum_sq = 0.0;
                n = 0;
            }
        }
        let _ = ac.Stop();
    }
    Ok(())
}
