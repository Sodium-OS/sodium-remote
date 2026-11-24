// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Decoding through FFmpeg (the C shim in csrc/): NVDEC via CUDA, VA-API, or
//! software (FFmpeg's HEVC decoder, dav1d for AV1). A picture is a reference to
//! FFmpeg's frame, handed to the presenter without a copy: in system memory, or
//! with `keep_cuda`, an NVDEC picture left on the GPU for CUDA → GL (`GlInterop`),
//! or on Windows a D3D11VA picture left in its decoder texture on the presenter's
//! own device (`d3d11_share_device`, `Picture::d3d11`).

use crate::proto::Codec;
use anyhow::{Result, bail};
use std::ffi::{CStr, c_char, c_int};
use std::ptr;

#[repr(C)]
struct RcDec {
    _p: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RcFrame {
    width: c_int,
    height: c_int,
    format: c_int,
    full_range: c_int,
    matrix: c_int,
    hw: c_int,
    data: [*const u8; 3],
    linesize: [c_int; 3],
    transfer_ms: f64,
    reference: *mut std::ffi::c_void,
    cuda: c_int,
    cuctx: *mut std::ffi::c_void,
    d3d11: c_int,
    texture: *mut std::ffi::c_void,
    slice: c_int,
    tex_width: c_int,
    tex_height: c_int,
}

#[repr(C)]
struct RcGl {
    _p: [u8; 0],
}

unsafe extern "C" {
    fn rc_dec_keep_cuda(d: *mut RcDec, on: c_int);
    fn rc_frame_luma(f: *const RcFrame, x: c_int, y: c_int, w: c_int, h: c_int, mean: *mut f64, err: *mut c_char, errlen: c_int) -> c_int;
    fn rc_gl_open(cuctx: *mut std::ffi::c_void, out: *mut *mut RcGl, err: *mut c_char, errlen: c_int) -> c_int;
    fn rc_gl_register(g: *mut RcGl, plane: c_int, tex: u32, err: *mut c_char, errlen: c_int) -> c_int;
    fn rc_gl_copy(g: *mut RcGl, f: *const RcFrame, err: *mut c_char, errlen: c_int) -> c_int;
    fn rc_gl_close(g: *mut RcGl);
}

#[cfg(windows)]
unsafe extern "C" {
    fn rc_d3d11_set_device(device: *mut std::ffi::c_void) -> c_int;
    fn rc_d3d11_lock();
    fn rc_d3d11_unlock();
    fn rc_d3d11_zero_copy(on: c_int);
}

/// Windows zero copy: D3D11VA decoders opened from now on decode on `device` (the
/// presenter's `ID3D11Device`, made with video support; null stops it), into NV12
/// textures the presenter samples. With `Decoder::keep_cuda(true)` their pictures
/// come back as `Picture::d3d11` instead of being copied to system memory.
///
/// # Safety
/// `device` is null or a live `ID3D11Device` (the shim takes its own reference).
#[cfg(windows)]
pub unsafe fn d3d11_share_device(device: *mut std::ffi::c_void) {
    unsafe {
        rc_d3d11_set_device(device);
    }
}

/// Turns the zero-copy pictures off (false) for every decoder, e.g. when the
/// presenter could not use the textures: pictures come in system memory again.
#[cfg(windows)]
pub fn d3d11_zero_copy(on: bool) {
    unsafe { rc_d3d11_zero_copy(on as c_int) }
}

/// The lock FFmpeg takes around its use of the shared device's immediate
/// context; the presenter holds it while it draws from a decoder texture.
#[cfg(windows)]
pub struct D3d11Lock(());

#[cfg(windows)]
impl D3d11Lock {
    pub fn take() -> D3d11Lock {
        unsafe { rc_d3d11_lock() };
        D3d11Lock(())
    }
}

#[cfg(windows)]
impl Drop for D3d11Lock {
    fn drop(&mut self) {
        unsafe { rc_d3d11_unlock() }
    }
}

unsafe extern "C" {
    fn rc_dec_open(codec: c_int, hw: c_int, out: *mut *mut RcDec, backend: *mut c_char, blen: c_int, err: *mut c_char, errlen: c_int) -> c_int;
    fn rc_dec_decode(d: *mut RcDec, data: *const u8, size: c_int, got: *mut c_int, err: *mut c_char, errlen: c_int) -> c_int;
    fn rc_dec_frame(d: *mut RcDec, out: *mut RcFrame, err: *mut c_char, errlen: c_int) -> c_int;
    fn rc_frame_free(r: *mut std::ffi::c_void);
    fn rc_dec_close(d: *mut RcDec);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwPref {
    Auto,
    Cuda,
    Vaapi,
    /// Windows: Direct3D 11 video decode (d3d11va), any GPU vendor.
    D3d11,
    Software,
}

impl HwPref {
    pub fn parse(s: &str) -> Option<HwPref> {
        Some(match s {
            "auto" => HwPref::Auto,
            "cuda" | "nvdec" => HwPref::Cuda,
            "vaapi" => HwPref::Vaapi,
            "d3d11" | "d3d11va" => HwPref::D3d11,
            "sw" | "software" => HwPref::Software,
            _ => return None,
        })
    }
    fn raw(self) -> c_int {
        match self {
            HwPref::Auto => 0,
            HwPref::Cuda => 1,
            HwPref::Vaapi => 2,
            HwPref::Software => 3,
            HwPref::D3d11 => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixFmt {
    /// Y plane, then interleaved CbCr at half size.
    Nv12,
    /// Three planes, chroma at half size.
    Yuv420p,
    /// Three planes, all full size.
    Yuv444p,
}

struct ErrBuf([c_char; 256]);

impl ErrBuf {
    fn new() -> Self {
        ErrBuf([0; 256])
    }
    fn msg(&self) -> String {
        unsafe { CStr::from_ptr(self.0.as_ptr()) }.to_string_lossy().into_owned()
    }
}

pub struct Decoder {
    raw: *mut RcDec,
    pub codec: Codec,
    /// What actually decodes, e.g. "hevc+cuda" or "libdav1d".
    pub backend: String,
}

// The decoder is used from one thread at a time.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn open(codec: Codec, pref: HwPref) -> Result<Decoder> {
        let mut raw = ptr::null_mut();
        let mut backend = [0 as c_char; 64];
        let mut e = ErrBuf::new();
        let c = match codec {
            Codec::Hevc => 0,
            Codec::Av1 => 1,
            Codec::H264 => 2,
        };
        let r = unsafe { rc_dec_open(c, pref.raw(), &mut raw, backend.as_mut_ptr(), 64, e.0.as_mut_ptr(), 256) };
        if r < 0 {
            bail!("opening the {codec:?} decoder: {}", e.msg());
        }
        let backend = unsafe { CStr::from_ptr(backend.as_ptr()) }.to_string_lossy().into_owned();
        Ok(Decoder { raw, codec, backend })
    }

    /// Keep NVDEC pictures on the GPU (`Picture::cuda`) instead of copying them down.
    pub fn keep_cuda(&mut self, on: bool) {
        unsafe { rc_dec_keep_cuda(self.raw, on as c_int) }
    }

    /// Decodes one access unit; true when a picture came out.
    pub fn decode(&mut self, data: &[u8]) -> Result<bool> {
        let mut got = 0;
        let mut e = ErrBuf::new();
        let r = unsafe { rc_dec_decode(self.raw, data.as_ptr(), data.len() as c_int, &mut got, e.0.as_mut_ptr(), 256) };
        if r < 0 {
            bail!("{}", e.msg());
        }
        Ok(got != 0)
    }

    /// The newest picture, in system memory (a hardware picture is copied down
    /// first). Returns it and that transfer's time in ms.
    pub fn picture(&mut self) -> Result<(Picture, f64)> {
        let mut f = RcFrame {
            width: 0,
            height: 0,
            format: 0,
            full_range: 0,
            matrix: 0,
            hw: 0,
            data: [ptr::null(); 3],
            linesize: [0; 3],
            transfer_ms: 0.0,
            reference: ptr::null_mut(),
            cuda: 0,
            cuctx: ptr::null_mut(),
            d3d11: 0,
            texture: ptr::null_mut(),
            slice: 0,
            tex_width: 0,
            tex_height: 0,
        };
        let mut e = ErrBuf::new();
        if unsafe { rc_dec_frame(self.raw, &mut f, e.0.as_mut_ptr(), 256) } < 0 {
            bail!("{}", e.msg());
        }
        let mut p = Picture {
            raw: f,
            cuda: f.cuda != 0,
            reference: f.reference,
            width: f.width as u32,
            height: f.height as u32,
            format: PixFmt::Nv12,
            full_range: f.full_range != 0,
            bt601: f.matrix == 5 || f.matrix == 6, // AVCOL_SPC_BT470BG, SMPTE170M
            hw: f.hw != 0,
            d3d11: (f.d3d11 != 0).then_some(D3d11Slice {
                texture: f.texture,
                slice: f.slice as u32,
                tex_width: f.tex_width as u32,
                tex_height: f.tex_height as u32,
            }),
            data: [ptr::null(); 3],
            strides: [0; 3],
            lens: [0; 3],
        };
        p.format = match f.format {
            1 => PixFmt::Nv12,
            2 => PixFmt::Yuv420p,
            3 => PixFmt::Yuv444p,
            x => bail!("unknown picture format {x}"),
        };
        if p.d3d11.is_some() {
            // A decoder texture slice: nothing in system memory.
            if !self.backend.contains("zero-copy") {
                self.backend = match self.backend.strip_suffix(" (shared device)") {
                    Some(b) => format!("{b} (shared device, zero-copy)"),
                    None => format!("{} (zero-copy)", self.backend),
                };
            }
            return Ok((p, 0.0));
        }
        let (w, h) = (p.width as usize, p.height as usize);
        let (cw, ch) = match p.format {
            PixFmt::Nv12 => (w.div_ceil(2) * 2, h.div_ceil(2)),
            PixFmt::Yuv420p => (w.div_ceil(2), h.div_ceil(2)),
            PixFmt::Yuv444p => (w, h),
        };
        let planes = if p.format == PixFmt::Nv12 { 2 } else { 3 };
        for i in 0..planes {
            let (row, rows) = if i == 0 { (w, h) } else { (cw, ch) };
            if f.data[i].is_null() || (f.linesize[i] as isize) < row as isize || rows == 0 {
                bail!("plane {i} has no data");
            }
            let ls = f.linesize[i] as usize;
            p.data[i] = f.data[i];
            p.strides[i] = ls;
            p.lens[i] = ls * (rows - 1) + row;
        }
        // FFmpeg falls back to software by itself when the hardware setup fails
        // (e.g. NVDEC wants at least 144 lines): say so in the label.
        if !p.hw && self.backend.contains('+') {
            self.backend = format!("{} (software: hardware setup failed)", self.backend.split('+').next().unwrap_or(""));
        }
        Ok((p, f.transfer_ms))
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { rc_dec_close(self.raw) }
    }
}

/// A decoded picture: FFmpeg's frame, kept alive until dropped. In system
/// memory, or on the GPU when `cuda` (then only `GlInterop` and `mean_luma` read it).
pub struct Picture {
    raw: RcFrame,
    pub cuda: bool,
    reference: *mut std::ffi::c_void,
    pub width: u32,
    pub height: u32,
    pub format: PixFmt,
    pub full_range: bool,
    pub bt601: bool,
    pub hw: bool,
    /// Windows zero copy: the picture is this slice of a decoder texture on the
    /// presenter's device (no planes in system memory).
    pub d3d11: Option<D3d11Slice>,
    data: [*const u8; 3],
    /// Bytes per row of each plane (FFmpeg's linesize; two bytes per sample pair for NV12's CbCr).
    pub strides: [usize; 3],
    lens: [usize; 3],
}

// The frame is refcounted and only read; it may be freed on any thread.
unsafe impl Send for Picture {}

/// A picture left in its D3D11 decoder texture: an NV12 texture array on the
/// device given to `d3d11_share_device`, padded beyond the picture's size.
#[derive(Clone, Copy, Debug)]
pub struct D3d11Slice {
    /// The `ID3D11Texture2D`, kept alive by the `Picture`.
    pub texture: *mut std::ffi::c_void,
    pub slice: u32,
    pub tex_width: u32,
    pub tex_height: u32,
}

impl Drop for Picture {
    fn drop(&mut self) {
        unsafe { rc_frame_free(self.reference) }
    }
}

impl Picture {
    /// Plane `i` (empty when the format has fewer planes).
    pub fn plane(&self, i: usize) -> &[u8] {
        if self.data[i].is_null() || self.cuda || self.d3d11.is_some() {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.data[i], self.lens[i]) }
    }

    /// Mean luma (0–255) over a rectangle, clamped to the picture. For a CUDA or
    /// D3D11 picture only the rectangle is copied down.
    pub fn mean_luma(&self, x: u32, y: u32, w: u32, h: u32) -> Option<f64> {
        let x1 = (x + w).min(self.width);
        let y1 = (y + h).min(self.height);
        if x >= x1 || y >= y1 {
            return None;
        }
        let mut mean = 0.0;
        let mut e = ErrBuf::new();
        let r = unsafe {
            rc_frame_luma(&self.raw, x as c_int, y as c_int, (x1 - x) as c_int, (y1 - y) as c_int, &mut mean, e.0.as_mut_ptr(), 256)
        };
        (r == 0).then_some(mean)
    }
}

/// CUDA → GL: copies a CUDA picture's planes straight into GL textures. Use it
/// on the thread whose GL context is current.
pub struct GlInterop {
    raw: *mut RcGl,
    cuctx: *mut std::ffi::c_void,
}

impl GlInterop {
    pub fn open(p: &Picture) -> Result<GlInterop> {
        let mut raw = ptr::null_mut();
        let mut e = ErrBuf::new();
        if unsafe { rc_gl_open(p.raw.cuctx, &mut raw, e.0.as_mut_ptr(), 256) } < 0 {
            bail!("CUDA–GL interop: {}", e.msg());
        }
        Ok(GlInterop { raw, cuctx: p.raw.cuctx })
    }
    pub fn same_context(&self, p: &Picture) -> bool {
        self.cuctx == p.raw.cuctx
    }
    /// (Re)register plane `i`'s texture after allocating its storage.
    pub fn register(&mut self, plane: usize, tex: u32) -> Result<()> {
        let mut e = ErrBuf::new();
        if unsafe { rc_gl_register(self.raw, plane as c_int, tex, e.0.as_mut_ptr(), 256) } < 0 {
            bail!("{}", e.msg());
        }
        Ok(())
    }
    pub fn copy(&mut self, p: &Picture) -> Result<()> {
        let mut e = ErrBuf::new();
        if unsafe { rc_gl_copy(self.raw, &p.raw, e.0.as_mut_ptr(), 256) } < 0 {
            bail!("{}", e.msg());
        }
        Ok(())
    }
}

impl Drop for GlInterop {
    fn drop(&mut self) {
        unsafe { rc_gl_close(self.raw) }
    }
}

/// What this machine decodes, by opening each decoder (hardware first, with
/// the software fallback `pref` allows), never assumed. `hevc_444` follows
/// `hevc` (the software decoder does 4:4:4; a hardware one that refuses falls
/// back by itself).
pub fn probe_support(pref: HwPref) -> remote_core::msg::DecodeSupport {
    let ok = |c| Decoder::open(c, pref).is_ok();
    let hevc = ok(Codec::Hevc);
    remote_core::msg::DecodeSupport { hevc, h264: ok(Codec::H264), hevc_444: hevc, av1: ok(Codec::Av1), max_width: 0, max_height: 0 }
}

/// Guess the codec from the bitstream when the host didn't say: AV1 streams
/// begin with a temporal delimiter OBU, HEVC Annex-B with a start code.
pub fn sniff_codec(data: &[u8]) -> Codec {
    // (H.264 is never guessed: the host names it in `Format`.)
    if data.len() >= 2 && data[0] == 0x12 && data[1] == 0x00 { Codec::Av1 } else { Codec::Hevc }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One white 320×192 frame (NVDEC wants at least 144 lines) from ffmpeg's own encoders, as an elementary stream.
    fn stream(dir: &std::path::Path, name: &str, args: &[&str]) -> Option<Vec<u8>> {
        let out = dir.join(name);
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "color=c=white:s=320x192", "-frames:v", "1"])
            .args(args)
            .arg(&out)
            .status()
            .ok()?
            .success();
        ok.then(|| std::fs::read(&out).ok()).flatten()
    }

    #[test]
    fn hevc_420_444_and_av1_decode_in_hardware_and_software() {
        let dir = std::env::temp_dir().join(format!("remote-client-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let hevc = ["-c:v", "libx265", "-x265-params", "bframes=0:log-level=error", "-f", "hevc"];
        let cases = [
            ("420.hevc", Codec::Hevc, vec!["-pix_fmt", "yuv420p"], PixFmt::Yuv420p),
            ("444.hevc", Codec::Hevc, vec!["-pix_fmt", "yuv444p"], PixFmt::Yuv444p),
            ("420.h264", Codec::H264, vec!["-pix_fmt", "yuv420p", "-c:v", "libx264", "-tune", "zerolatency", "-f", "h264"], PixFmt::Yuv420p),
            ("420.obu", Codec::Av1, vec!["-pix_fmt", "yuv420p", "-c:v", "libaom-av1", "-cpu-used", "8", "-f", "obu"], PixFmt::Yuv420p),
        ];
        for (name, codec, mut args, sw_fmt) in cases {
            if codec == Codec::Hevc {
                args.extend(hevc);
            }
            let Some(data) = stream(&dir, name, &args) else {
                eprintln!("skipping {name}: ffmpeg could not make it");
                continue;
            };
            if codec != Codec::H264 {
                assert_eq!(sniff_codec(&data), codec);
            }
            for pref in [HwPref::Auto, HwPref::Software] {
                let mut d = Decoder::open(codec, pref).unwrap();
                assert!(d.decode(&data).unwrap(), "{name} {} gave no picture", d.backend);
                let (p, _) = d.picture().unwrap();
                assert_eq!((p.width, p.height), (320, 192), "{name} {}", d.backend);
                if !p.hw {
                    assert_eq!(p.format, sw_fmt);
                }
                let y = p.mean_luma(0, 0, 320, 192).unwrap();
                if pref == HwPref::Auto && std::path::Path::new("/dev/nvidiactl").exists() {
                    assert!(p.hw, "{name}: expected NVDEC, got {}", d.backend);
                }
                assert!(y > 200.0, "{name} {}: luma {y}", d.backend);
                eprintln!("{name}: {} -> {:?} hw={} luma {y:.0}", d.backend, p.format, p.hw);
                if p.hw {
                    // The same picture left on the GPU: the probe reads it there.
                    let mut d = Decoder::open(codec, pref).unwrap();
                    d.keep_cuda(true);
                    assert!(d.decode(&data).unwrap());
                    let (p, transfer) = d.picture().unwrap();
                    assert!(p.cuda && transfer == 0.0, "{name}: not kept on the GPU");
                    assert!(p.mean_luma(0, 0, 320, 192).unwrap() > 200.0, "{name}: CUDA luma probe");
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
