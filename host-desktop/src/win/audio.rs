// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! WASAPI loopback of the default render endpoint → 48 kHz stereo f32, 10 ms at a time.
//!
//! Windows can hand back whatever the speakers are playing as a capture stream. We ask
//! for 48 kHz stereo float and let Windows convert; if the device refuses, we capture in
//! its own mix format and convert here (`to_stereo` and `Resampler`). Loopback delivers no
//! packets while nothing plays, so `read` returns false after its timeout, and a stale
//! tail is dropped so it doesn't play later. When the default output device changes, or
//! the device goes away (AUDCLNT_E_DEVICE_INVALIDATED), `read` fails and the caller opens
//! a new `Loopback` on the new default.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioCaptureClient,
    IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::core::GUID;

use crate::backend::AudioSource;

const RATE: u32 = 48_000;
/// Samples in 10 ms of stereo.
const CHUNK: usize = 960;
/// Never hold more than this much (200 ms) of unread audio: older audio is dropped.
const MAX_BUFFERED: usize = 19_200;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const SUBTYPE_PCM_DATA1: u32 = 1;
const SUBTYPE_FLOAT_DATA1: u32 = 3;
/// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT.
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const SPEAKER_FRONT_LEFT_RIGHT: u32 = 3;

/// How the device's samples are laid out.
#[derive(Clone, Copy, Debug)]
struct Layout {
    rate: u32,
    channels: usize,
    /// Bits in one sample's container: 16, 24 or 32.
    bits: usize,
    float: bool,
}

/// Uninitialises COM on drop, if we were the ones to initialise it.
struct Com(bool);

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

pub struct Loopback {
    // Fields drop in order: the interfaces go before COM is uninitialised.
    client: IAudioClient,
    capture: IAudioCaptureClient,
    enumerator: IMMDeviceEnumerator,
    device_id: String,
    layout: Layout,
    resampler: Resampler,
    queue: VecDeque<f32>,
    last_device_check: Instant,
    _com: Com,
}

// The COM objects are used only on the thread that opened them (the audio thread owns the
// Loopback from open to drop), which is all `Send` is needed for here.
unsafe impl Send for Loopback {}

fn device_id(dev: &IMMDevice) -> Result<String> {
    unsafe {
        let p = dev.GetId()?;
        let s = p.to_string();
        CoTaskMemFree(Some(p.0 as *const _));
        Ok(s?)
    }
}

fn default_id(enumerator: &IMMDeviceEnumerator) -> Result<String> {
    let dev = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }?;
    device_id(&dev)
}

/// Reads the layout out of a WAVEFORMATEX that may really be a WAVEFORMATEXTENSIBLE.
unsafe fn layout_of(f: *const WAVEFORMATEX) -> Result<Layout> {
    unsafe {
        let w = f.read_unaligned();
        let tag = w.wFormatTag;
        let channels = w.nChannels;
        let mut float = tag == WAVE_FORMAT_IEEE_FLOAT;
        if tag == WAVE_FORMAT_EXTENSIBLE {
            let x = (f as *const WAVEFORMATEXTENSIBLE).read_unaligned();
            let data1 = x.SubFormat.data1;
            match data1 {
                SUBTYPE_FLOAT_DATA1 => float = true,
                SUBTYPE_PCM_DATA1 => float = false,
                _ => bail!("unsupported sample subtype {data1:#x}"),
            }
        } else if tag != WAVE_FORMAT_PCM && !float {
            bail!("unsupported format tag {tag}");
        }
        let bits = w.wBitsPerSample as usize;
        let ok = if float { bits == 32 } else { matches!(bits, 16 | 24 | 32) };
        if channels == 0 || !ok {
            bail!("unsupported format: {channels} channels, {bits} bits");
        }
        Ok(Layout { rate: w.nSamplesPerSec, channels: channels as usize, bits, float })
    }
}

/// The 48 kHz stereo float format we'd like Windows to give us.
fn wanted_format() -> WAVEFORMATEXTENSIBLE {
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE,
            nChannels: 2,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 22,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: 32 },
        dwChannelMask: SPEAKER_FRONT_LEFT_RIGHT,
        SubFormat: SUBTYPE_IEEE_FLOAT,
    }
}

/// Converts `frames` frames of device samples to stereo f32 frames: the first two
/// channels (mono is duplicated), integers scaled to -1..1.
fn to_stereo(data: &[u8], frames: usize, l: &Layout, out: &mut Vec<[f32; 2]>) {
    let bytes = l.bits / 8;
    let sample = |i: usize| -> f32 {
        let b = &data[i * bytes..(i + 1) * bytes];
        match (l.float, l.bits) {
            (true, _) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            (false, 16) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            (false, 24) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            _ => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        }
    };
    for f in 0..frames {
        let left = sample(f * l.channels);
        let right = if l.channels > 1 { sample(f * l.channels + 1) } else { left };
        out.push([left, right]);
    }
}

/// Linear-interpolation resampler to 48 kHz. It keeps its position between calls, so
/// chunk edges don't click.
struct Resampler {
    /// Input frames per output frame.
    step: f64,
    /// Input frames not yet fully used; the next output lies at `pos` within them.
    src: Vec<[f32; 2]>,
    pos: f64,
}

impl Resampler {
    fn new(rate: u32) -> Resampler {
        Resampler { step: rate as f64 / RATE as f64, src: Vec::new(), pos: 0.0 }
    }

    fn push(&mut self, input: &[[f32; 2]], out: &mut VecDeque<f32>) {
        if self.step == 1.0 {
            for f in input {
                out.extend(f);
            }
            return;
        }
        self.src.extend_from_slice(input);
        while self.pos + 1.0 < self.src.len() as f64 {
            let i = self.pos as usize;
            let t = (self.pos - i as f64) as f32;
            let (a, b) = (self.src[i], self.src[i + 1]);
            out.push_back(a[0] + (b[0] - a[0]) * t);
            out.push_back(a[1] + (b[1] - a[1]) * t);
            self.pos += self.step;
        }
        let used = (self.pos as usize).min(self.src.len());
        self.src.drain(..used);
        self.pos -= used as f64;
    }
}

impl Loopback {
    /// Opens the default output device's loopback. Call on the thread that will read.
    pub fn open() -> Result<Loopback> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            // S_FALSE: already initialised here. RPC_E_CHANGED_MODE: another mode, usable anyway.
            let com = Com(hr.is_ok());
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).context("no audio device enumerator")?;
            let dev = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).context("no default output device")?;
            let id = device_id(&dev)?;
            let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).context("can't activate the audio client")?;

            let mix = client.GetMixFormat()?;
            let mix_layout = layout_of(mix);
            eprintln!("sodium-remote-host: audio: output mix format {mix_layout:?}");
            let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            let wanted = wanted_format();
            let buffer = 2_000_000; // 200 ms in 100 ns units
            let first = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                buffer,
                0,
                &wanted as *const WAVEFORMATEXTENSIBLE as *const WAVEFORMATEX,
                None,
            );
            let opened = match first {
                Ok(()) => Ok((client, Layout { rate: RATE, channels: 2, bits: 32, float: true })),
                Err(e) => {
                    eprintln!("sodium-remote-host: audio: 48 kHz stereo float refused ({e}); converting ourselves");
                    // A failed Initialize leaves the client unusable: take a fresh one.
                    (|| -> Result<(IAudioClient, Layout)> {
                        let c: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
                        let l = mix_layout?;
                        c.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, buffer, 0, mix, None)?;
                        Ok((c, l))
                    })()
                }
            };
            CoTaskMemFree(Some(mix as *const _));
            let (client, layout) = opened.context("the output device's mix format can't be captured")?;
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Loopback {
                client,
                capture,
                enumerator,
                device_id: id,
                layout,
                resampler: Resampler::new(layout.rate),
                queue: VecDeque::new(),
                last_device_check: Instant::now(),
                _com: com,
            })
        }
    }

    /// Moves every waiting packet into the queue.
    fn drain(&mut self) -> Result<()> {
        let mut frames_buf = Vec::new();
        unsafe {
            loop {
                if self.capture.GetNextPacketSize()? == 0 {
                    return Ok(());
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize;
                frames_buf.clear();
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    frames_buf.resize(n, [0.0, 0.0]);
                } else {
                    let bytes = std::slice::from_raw_parts(data, n * self.layout.channels * self.layout.bits / 8);
                    to_stereo(bytes, n, &self.layout, &mut frames_buf);
                }
                self.capture.ReleaseBuffer(frames)?;
                self.resampler.push(&frames_buf, &mut self.queue);
            }
        }
    }
}

impl AudioSource for Loopback {
    fn read(&mut self, out: &mut [f32; 960], timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.last_device_check.elapsed() >= Duration::from_secs(1) {
                self.last_device_check = Instant::now();
                let now = default_id(&self.enumerator).map_err(|e| anyhow!("default output device lost ({e})"))?;
                if now != self.device_id {
                    bail!("the default output device changed");
                }
            }
            // Any failure here, AUDCLNT_E_DEVICE_INVALIDATED included, ends this capture.
            self.drain().context("capturing the output")?;
            if self.queue.len() > MAX_BUFFERED {
                // Fell behind: drop the oldest, keeping whole frames.
                let extra = (self.queue.len() - MAX_BUFFERED) / 2 * 2;
                self.queue.drain(..extra);
            }
            if self.queue.len() >= CHUNK {
                for (o, s) in out.iter_mut().zip(self.queue.drain(..CHUNK)) {
                    *o = s;
                }
                return Ok(true);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                // Nothing playing: a partial tail is stale by now.
                self.queue.clear();
                return Ok(false);
            }
            std::thread::sleep(left.min(Duration::from_millis(2)));
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}
