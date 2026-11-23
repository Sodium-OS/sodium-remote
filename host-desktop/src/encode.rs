// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Encoders through the C shim (`csrc/hd_shim.c`): FFmpeg's (the hardware HEVC and H.264
//! encoders, Media Foundation's H.264, VA-API, libx264, libopenh264) and OpenH264 used
//! directly, its library loaded at run time (`softvideo`). Shared by the Windows, Linux and
//! fake backends; each gives its own candidate list. On Linux a `*_vaapi` encoder also takes
//! the portal's DMA-BUF pictures (`hd_enc_encode_drm`).
//!
//! Every candidate is set up for low latency: no B-frames, no lookahead, an endless
//! GOP with keyframes on request only, CBR with a one-frame VBV (FFmpeg) or OpenH264's
//! bitrate mode without frame skipping. OpenH264 changes its rate per frame; FFmpeg's
//! wrappers fix it at open (its NVENC wrapper even forces an IDR on every change), so
//! for those the media loop re-opens on a lasting change (`policy::RateFollower`).

use std::ffi::{CStr, CString, c_char, c_int};
use std::sync::{Arc, OnceLock};

use anyhow::{Result, bail};
use remote_core::msg::Codec;

use crate::backend::{EncodedFrame, Encoder, EncoderCaps, EncoderConfig, Picture};
use crate::softvideo::SoftwareVideo;

#[repr(C)]
struct HdEnc {
    _p: [u8; 0],
}

#[repr(C)]
struct HdEncConfig {
    name: *const c_char,
    width: c_int,
    height: c_int,
    fps: c_int,
    bitrate: i64,
    threads: c_int,
}

unsafe extern "C" {
    fn hd_enc_open(cfg: *const HdEncConfig, out: *mut *mut HdEnc, err: *mut c_char, errlen: c_int) -> c_int;
    fn hd_enc_codec(e: *const HdEnc) -> c_int;
    fn hd_enc_dynamic_rate(e: *const HdEnc) -> c_int;
    #[allow(clippy::too_many_arguments)]
    fn hd_enc_encode(
        e: *mut HdEnc,
        bgra: *const u8,
        w: c_int,
        h: c_int,
        stride: c_int,
        force_key: c_int,
        bitrate: i64,
        out: *mut *const u8,
        out_len: *mut c_int,
        is_key: *mut c_int,
        convert_ms: *mut f64,
        encode_ms: *mut f64,
        err: *mut c_char,
        errlen: c_int,
    ) -> c_int;
    fn hd_enc_close(e: *mut HdEnc);
    fn hd_log_quiet(quiet: c_int);
    #[cfg(all(target_os = "linux", feature = "linux-portal"))]
    fn hd_enc_encode_drm(
        e: *mut HdEnc,
        fourcc: u32,
        modifier: u64,
        nplanes: c_int,
        fds: *const c_int,
        offsets: *const u32,
        pitches: *const u32,
        w: c_int,
        h: c_int,
        force_key: c_int,
        out: *mut *const u8,
        out_len: *mut c_int,
        is_key: *mut c_int,
        convert_ms: *mut f64,
        encode_ms: *mut f64,
        err: *mut c_char,
        errlen: c_int,
    ) -> c_int;
}

/// The software H.264 candidate: OpenH264 through its run-time loaded library.
pub const SOFT: &str = "openh264";

/// The encoders a backend tries, best first, per codec.
#[derive(Clone, Debug)]
pub struct Candidates {
    /// Hardware HEVC encoders only.
    pub hevc: Vec<&'static str>,
    pub h264: Vec<&'static str>,
}

impl Candidates {
    /// The Windows order: the hardware encoders
    /// (nvenc, amf, qsv), then Media Foundation's GPU MFT, then OpenH264 from Cisco's DLL
    /// (`softvideo`: downloaded on first need, never shipped).
    pub fn windows() -> Candidates {
        Candidates {
            hevc: vec!["hevc_nvenc", "hevc_amf", "hevc_qsv"],
            h264: vec!["h264_nvenc", "h264_amf", "h264_qsv", "h264_mf", SOFT],
        }
    }

    /// The generic Linux order (docs/reference/host-linux.md): VA-API (DMA-BUF straight in),
    /// then NVENC (a CPU copy), then the system FFmpeg's software H.264, libx264 then
    /// libopenh264, and last OpenH264 itself, the system library loaded at run time (`softvideo`).
    pub fn linux() -> Candidates {
        Candidates {
            hevc: vec!["hevc_vaapi", "hevc_nvenc"],
            h264: vec!["h264_vaapi", "h264_nvenc", "libx264", "libopenh264", SOFT],
        }
    }

    /// One encoder by name (`--encoder`), for tests.
    pub fn only(name: &str) -> Candidates {
        let name: &'static str = Box::leak(name.to_string().into_boxed_str());
        if name.starts_with("hevc") {
            Candidates { hevc: vec![name], h264: vec![] }
        } else {
            Candidates { hevc: vec![], h264: vec![name] }
        }
    }

    fn of(&self, codec: Codec) -> &[&'static str] {
        match codec {
            Codec::H264 => &self.h264,
            _ => &self.hevc,
        }
    }
}

pub struct ShimEncoder {
    e: *mut HdEnc,
    name: String,
    codec: Codec,
    dynamic: bool,
}

// The shim's encoder is used from one thread at a time (the media thread).
unsafe impl Send for ShimEncoder {}

impl ShimEncoder {
    pub fn open(name: &str, cfg: &EncoderConfig) -> Result<ShimEncoder> {
        let cname = CString::new(name)?;
        let c = HdEncConfig {
            name: cname.as_ptr(),
            width: cfg.width as c_int,
            height: cfg.height as c_int,
            fps: cfg.fps as c_int,
            bitrate: cfg.bitrate_bps as i64,
            threads: 0,
        };
        let mut err = [0 as c_char; 256];
        let mut e = std::ptr::null_mut();
        let r = unsafe { hd_enc_open(&c, &mut e, err.as_mut_ptr(), err.len() as c_int) };
        if r < 0 || e.is_null() {
            bail!("{}", unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy());
        }
        let codec = if unsafe { hd_enc_codec(e) } == 0 { Codec::Hevc420 } else { Codec::H264 };
        let dynamic = unsafe { hd_enc_dynamic_rate(e) } != 0;
        Ok(ShimEncoder { e, name: name.to_string(), codec, dynamic })
    }
}

impl Drop for ShimEncoder {
    fn drop(&mut self) {
        unsafe { hd_enc_close(self.e) }
    }
}

impl Encoder for ShimEncoder {
    fn name(&self) -> &str {
        &self.name
    }
    fn codec(&self) -> Codec {
        self.codec
    }
    fn dynamic_rate(&self) -> bool {
        self.dynamic
    }
    fn encode(&mut self, picture: &dyn Picture, keyframe: bool, bitrate_bps: u64) -> Result<EncodedFrame> {
        let (w, h) = picture.size();
        #[cfg(all(target_os = "linux", feature = "linux-portal"))]
        if let Some(d) = picture.as_any().downcast_ref::<crate::linux::capture::DmaPicture>() {
            return self.encode_drm(d, keyframe);
        }
        let Some((data, stride)) = picture.bgra() else { bail!("{}: the picture isn't in system memory", self.name) };
        let mut out = std::ptr::null();
        let (mut len, mut key) = (0, 0);
        let (mut convert_ms, mut encode_ms) = (0.0, 0.0);
        let mut err = [0 as c_char; 256];
        let r = unsafe {
            hd_enc_encode(
                self.e,
                data.as_ptr(),
                w as c_int,
                h as c_int,
                stride as c_int,
                keyframe as c_int,
                bitrate_bps as i64,
                &mut out,
                &mut len,
                &mut key,
                &mut convert_ms,
                &mut encode_ms,
                err.as_mut_ptr(),
                err.len() as c_int,
            )
        };
        if r < 0 {
            bail!("{}", unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy());
        }
        let data = if len > 0 { unsafe { std::slice::from_raw_parts(out, len as usize) }.to_vec() } else { vec![] };
        Ok(EncodedFrame {
            data,
            keyframe: key != 0,
            convert_us: (convert_ms * 1000.0) as u32,
            encode_us: (encode_ms * 1000.0) as u32,
        })
    }
}

#[cfg(all(target_os = "linux", feature = "linux-portal"))]
impl ShimEncoder {
    /// A DMA-BUF picture straight into VA-API. When the import fails, DMA-BUF is switched
    /// off for good (`linux::capture::dmabuf_failed`): the capture re-opens on SHM and this
    /// frame is skipped, so the session goes on.
    fn encode_drm(&mut self, d: &crate::linux::capture::DmaPicture, keyframe: bool) -> Result<EncodedFrame> {
        let b = &d.buf;
        let fds: Vec<c_int> = b.planes.iter().map(|p| p.fd).collect();
        let offsets: Vec<u32> = b.planes.iter().map(|p| p.offset).collect();
        let pitches: Vec<u32> = b.planes.iter().map(|p| p.stride).collect();
        let mut out = std::ptr::null();
        let (mut len, mut key) = (0, 0);
        let (mut convert_ms, mut encode_ms) = (0.0, 0.0);
        let mut err = [0 as c_char; 256];
        let r = unsafe {
            hd_enc_encode_drm(
                self.e,
                b.drm_format,
                b.modifier,
                fds.len() as c_int,
                fds.as_ptr(),
                offsets.as_ptr(),
                pitches.as_ptr(),
                b.width as c_int,
                b.height as c_int,
                keyframe as c_int,
                &mut out,
                &mut len,
                &mut key,
                &mut convert_ms,
                &mut encode_ms,
                err.as_mut_ptr(),
                err.len() as c_int,
            )
        };
        if r < 0 {
            let e = unsafe { CStr::from_ptr(err.as_ptr()) }.to_string_lossy().into_owned();
            crate::linux::capture::dmabuf_failed(&e);
            return Ok(EncodedFrame { data: vec![], keyframe: false, convert_us: 0, encode_us: 0 });
        }
        let data = if len > 0 { unsafe { std::slice::from_raw_parts(out, len as usize) }.to_vec() } else { vec![] };
        Ok(EncodedFrame { data, keyframe: key != 0, convert_us: (convert_ms * 1000.0) as u32, encode_us: (encode_ms * 1000.0) as u32 })
    }
}

/// Which candidates work on this machine: each hardware one is opened once and encodes one frame.
/// The software one (OpenH264) is judged each time it is asked about: it works once its library is
/// loaded and the setting "software video" is on, which can become true while the host runs.
pub struct Probe {
    pub cands: Candidates,
    ok: OnceLock<Vec<&'static str>>,
    soft: Arc<SoftwareVideo>,
    soft_ok: OnceLock<bool>,
}

impl Probe {
    /// With the platform's software video: Cisco's DLL on Windows, the system's library elsewhere.
    pub fn new(cands: Candidates) -> Probe {
        Probe::with_software(cands, SoftwareVideo::platform_default())
    }

    pub fn with_software(cands: Candidates, soft: Arc<SoftwareVideo>) -> Probe {
        Probe { cands, ok: OnceLock::new(), soft, soft_ok: OnceLock::new() }
    }

    fn working(&self) -> &[&'static str] {
        self.ok.get_or_init(|| {
            // Absent hardware encoders explain themselves at length: our one line will do.
            unsafe { hd_log_quiet(1) };
            let mut ok = vec![];
            let pic = crate::backend::CpuPicture::new(640, 360);
            for name in self.cands.hevc.iter().chain(&self.cands.h264).filter(|n| **n != SOFT) {
                let cfg = EncoderConfig { codec: Codec::H264, width: 640, height: 360, fps: 30, bitrate_bps: 2_000_000 };
                match ShimEncoder::open(name, &cfg).and_then(|mut e| e.encode(&pic, true, cfg.bitrate_bps)) {
                    Ok(_) => {
                        eprintln!("sodium-remote-host: encoder {name}: works");
                        ok.push(*name);
                    }
                    Err(e) => eprintln!("sodium-remote-host: encoder {name}: {e:#}"),
                }
            }
            unsafe { hd_log_quiet(0) };
            if self.cands.h264.contains(&SOFT) {
                // No hardware H.264 encoder: software video may be needed (and, on Windows, fetched).
                let hw = self.cands.h264.iter().any(|n| *n != SOFT && ok.contains(n));
                self.soft.note_hardware(hw);
            }
            ok
        })
    }

    /// The candidates that work now, best first (`--list`, and whether DMA-BUF is worth asking for).
    pub fn works(&self) -> Vec<&'static str> {
        let mut w = self.working().to_vec();
        if self.soft_usable() {
            w.push(SOFT);
        }
        w
    }

    /// Software H.264 is a candidate, the setting allows it, its library is loaded and it encodes a frame.
    fn soft_usable(&self) -> bool {
        if !self.cands.h264.contains(&SOFT) || !self.soft.usable() {
            return false;
        }
        *self.soft_ok.get_or_init(|| {
            let cfg = EncoderConfig { codec: Codec::H264, width: 640, height: 360, fps: 30, bitrate_bps: 2_000_000 };
            let pic = crate::backend::CpuPicture::new(640, 360);
            match ShimEncoder::open(SOFT, &cfg).and_then(|mut e| e.encode(&pic, true, cfg.bitrate_bps)) {
                Ok(_) => {
                    eprintln!("sodium-remote-host: encoder {SOFT}: works");
                    true
                }
                Err(e) => {
                    eprintln!("sodium-remote-host: encoder {SOFT}: {e:#}");
                    false
                }
            }
        })
    }

    pub fn caps(&self) -> EncoderCaps {
        let w = self.working();
        EncoderCaps {
            hevc: self.cands.hevc.iter().any(|n| w.contains(n)),
            h264: self.cands.h264.iter().any(|n| w.contains(n)) || self.soft_usable(),
        }
    }

    /// The first working candidate for the codec that opens at this configuration.
    pub fn open(&self, cfg: &EncoderConfig) -> Result<Box<dyn Encoder>> {
        let mut w = self.working().to_vec();
        if cfg.codec == Codec::H264 && self.soft_usable() {
            w.push(SOFT);
        }
        first_that_opens(self.cands.of(cfg.codec), &w, cfg.codec, |name| {
            ShimEncoder::open(name, cfg).map(|e| Box::new(e) as Box<dyn Encoder>)
        })
    }
}

/// The first candidate, in order, that worked when probed and opens now: a hardware
/// encoder whose set-up fails at this size or rate falls back to the next.
fn first_that_opens<T>(cands: &[&'static str], working: &[&'static str], codec: Codec, mut open: impl FnMut(&str) -> Result<T>) -> Result<T> {
    let mut last = None;
    for name in cands.iter().filter(|n| working.contains(n)) {
        match open(name) {
            Ok(e) => return Ok(e),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("no {codec:?} encoder on this computer")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_order_is_vaapi_nvenc_then_software() {
        let c = Candidates::linux();
        assert_eq!(c.hevc, ["hevc_vaapi", "hevc_nvenc"]);
        assert_eq!(c.h264, ["h264_vaapi", "h264_nvenc", "libx264", "libopenh264", "openh264"]);
    }

    #[test]
    fn falls_back_past_missing_and_failing_hardware() {
        let c = Candidates::linux();
        // No VA-API at all (the probe failed): NVENC.
        let working = ["h264_nvenc", "libx264", "openh264"];
        assert_eq!(first_that_opens(&c.h264, &working, Codec::H264, |n| Ok(n.to_string())).unwrap(), "h264_nvenc");
        // NVENC probed fine but won't open at this size: libx264.
        let r = first_that_opens(&c.h264, &working, Codec::H264, |n| if n == "h264_nvenc" { bail!("too big") } else { Ok(n.to_string()) });
        assert_eq!(r.unwrap(), "libx264");
        // Nothing works: an error, not a panic.
        assert!(first_that_opens(&c.hevc, &working, Codec::Hevc420, |n| Ok(n.to_string())).is_err());
    }
}
