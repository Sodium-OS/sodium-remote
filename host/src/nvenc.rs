// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! NVENC through CUDA interop (Route A), over the C shim in csrc/.

use std::{
    ffi::{CStr, c_char, c_int},
    os::fd::{IntoRawFd, OwnedFd},
    ptr,
};

use anyhow::{Result, bail};

#[repr(C)]
struct RdCuda {
    _p: [u8; 0],
}
#[repr(C)]
struct RdExtMem {
    _p: [u8; 0],
}
#[repr(C)]
struct RdEnc {
    _p: [u8; 0],
}

pub const CODEC_HEVC: c_int = 0;
pub const CODEC_AV1: c_int = 1;
pub const INPUT_NV12: c_int = 0;
pub const INPUT_YUV444: c_int = 1;
pub const INPUT_ARGB: c_int = 2;
pub const SPLIT_TWO_FORCED: c_int = 2;
pub const SPLIT_OFF: c_int = 15;

#[derive(Clone, Copy, Debug)]
pub struct EncParams {
    pub codec: c_int,
    pub input: c_int,
    pub chroma444: c_int,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub kbps: u32,
    pub vbv_pct: u32,
    pub split: c_int,
}

/// The shim's RdEncParams: EncParams plus the milestone 1 switch (QP delta maps,
/// ME hints, on-demand intra refresh).
#[repr(C)]
struct RdEncParams {
    p: EncParamsC,
    m1: c_int,
}

#[repr(C)]
struct EncParamsC {
    codec: c_int,
    input: c_int,
    chroma444: c_int,
    width: u32,
    height: u32,
    fps: u32,
    kbps: u32,
    vbv_pct: u32,
    split: c_int,
}

#[repr(C)]
struct RdPicOpts {
    force_idr: c_int,
    force_ir_frames: u32,
    qp_delta: *const i8,
    qp_delta_size: u32,
    mv_xy: *const i16,
    mv_valid: *const u8,
    n_mv: u32,
}

/// Per-picture options for [`Encoder::encode_with`].
#[derive(Default)]
pub struct PicOpts<'a> {
    pub force_idr: bool,
    /// On-demand intra refresh spread over this many frames (0: none).
    pub intra_refresh: u32,
    /// QP delta per CTB/SB in raster order (see [`Encoder::grid`]).
    pub qp_delta: Option<&'a [i8]>,
    /// Motion hints per block in raster order: (x, y) integer pixels from the block to
    /// its match in the previous frame, and whether the block has one.
    pub mv: Option<(&'a [[i16; 2]], &'a [u8])>,
}

#[repr(C)]
struct RdPacket {
    data: *const u8,
    size: u32,
    pic_type: c_int,
    avg_qp: u32,
    submit_to_ready_ms: f64,
}

unsafe extern "C" {
    fn rd_cuda_open(uuid: *const u8, out: *mut *mut RdCuda, err: *mut c_char, errlen: c_int) -> c_int;
    fn rd_cuda_close(cu: *mut RdCuda);
    fn rd_cuda_import(
        cu: *mut RdCuda,
        fd: c_int,
        size: u64,
        dedicated: c_int,
        out: *mut *mut RdExtMem,
        devptr: *mut u64,
        err: *mut c_char,
        errlen: c_int,
    ) -> c_int;
    fn rd_cuda_release(cu: *mut RdCuda, m: *mut RdExtMem);
    fn rd_enc_open(cu: *mut RdCuda, p: *const RdEncParams, out: *mut *mut RdEnc, err: *mut c_char, errlen: c_int) -> c_int;
    fn rd_enc_register(e: *mut RdEnc, devptr: u64, pitch: u32, slot: *mut c_int, err: *mut c_char, errlen: c_int) -> c_int;
    fn rd_enc_encode(
        e: *mut RdEnc,
        slot: c_int,
        pts: u64,
        force_idr: c_int,
        out: *mut RdPacket,
        err: *mut c_char,
        errlen: c_int,
    ) -> c_int;
    fn rd_enc_encode2(
        e: *mut RdEnc,
        slot: c_int,
        pts: u64,
        o: *const RdPicOpts,
        out: *mut RdPacket,
        err: *mut c_char,
        errlen: c_int,
    ) -> c_int;
    fn rd_enc_grid(e: *mut RdEnc, qp_cell: *mut u32, mv_block: *mut u32);
    fn rd_enc_set_bitrate(e: *mut RdEnc, kbps: u32, err: *mut c_char, errlen: c_int) -> c_int;
    fn rd_enc_set_rate(e: *mut RdEnc, kbps: u32, vbv_pct: u32, split: c_int, max_qp: u32, err: *mut c_char, errlen: c_int) -> c_int;
    fn rd_enc_invalidate(e: *mut RdEnc, pts: u64, err: *mut c_char, errlen: c_int) -> c_int;
    fn rd_enc_close(e: *mut RdEnc);
}

struct Err([c_char; 512]);
impl Err {
    fn new() -> Self {
        Err([0; 512])
    }
    fn ptr(&mut self) -> *mut c_char {
        self.0.as_mut_ptr()
    }
    fn check(&self, r: c_int) -> Result<()> {
        if r != 0 {
            bail!("{}", unsafe { CStr::from_ptr(self.0.as_ptr()) }.to_string_lossy());
        }
        Ok(())
    }
}

pub struct Cuda(*mut RdCuda);

impl Cuda {
    /// The CUDA device with this UUID (the Vulkan device's).
    pub fn open(uuid: &[u8; 16]) -> Result<Self> {
        let mut e = Err::new();
        let mut p = ptr::null_mut();
        let r = unsafe { rd_cuda_open(uuid.as_ptr(), &mut p, e.ptr(), 512) };
        e.check(r)?;
        Ok(Cuda(p))
    }

    /// Imports Vulkan memory exported as an opaque fd; returns the CUDA pointer.
    pub fn import(&self, fd: OwnedFd, size: u64) -> Result<ExtMem> {
        let mut e = Err::new();
        let (mut m, mut dev) = (ptr::null_mut(), 0u64);
        let raw = fd.into_raw_fd();
        let r = unsafe { rd_cuda_import(self.0, raw, size, 1, &mut m, &mut dev, e.ptr(), 512) };
        if r != 0 {
            unsafe { libc::close(raw) }; // CUDA owns the fd only on success
        }
        e.check(r)?;
        Ok(ExtMem { m, devptr: dev, cuda: self.0 })
    }
}

impl Drop for Cuda {
    fn drop(&mut self) {
        unsafe { rd_cuda_close(self.0) }
    }
}

pub struct ExtMem {
    m: *mut RdExtMem,
    pub devptr: u64,
    cuda: *mut RdCuda,
}

impl Drop for ExtMem {
    fn drop(&mut self) {
        unsafe { rd_cuda_release(self.cuda, self.m) }
    }
}

pub struct Packet<'a> {
    pub data: &'a [u8],
    pub pic_type: i32,
    pub avg_qp: u32,
    pub ms: f64,
}


pub struct Encoder(*mut RdEnc);

// The encoder and CUDA context are used from one thread at a time (media's encoder
// thread owns them); the shim pushes its own context around every call.
unsafe impl Send for Encoder {}
unsafe impl Send for Cuda {}
unsafe impl Send for ExtMem {}

impl Encoder {
    pub fn open(cuda: &Cuda, p: &EncParams) -> Result<Self> {
        Self::open_with(cuda, p, false)
    }

    /// `m1`: QP delta maps, ME hints and on-demand intra refresh (media's encoder).
    pub fn open_with(cuda: &Cuda, p: &EncParams, m1: bool) -> Result<Self> {
        let mut e = Err::new();
        let mut enc = ptr::null_mut();
        let c = RdEncParams {
            p: EncParamsC {
                codec: p.codec,
                input: p.input,
                chroma444: p.chroma444,
                width: p.width,
                height: p.height,
                fps: p.fps,
                kbps: p.kbps,
                vbv_pct: p.vbv_pct,
                split: p.split,
            },
            m1: m1 as c_int,
        };
        let r = unsafe { rd_enc_open(cuda.0, &c, &mut enc, e.ptr(), 512) };
        e.check(r)?;
        Ok(Encoder(enc))
    }

    /// Registers an input surface (a CUDA pointer with a row pitch); returns its slot.
    pub fn register(&mut self, devptr: u64, pitch: u32) -> Result<i32> {
        let mut e = Err::new();
        let mut slot = 0;
        let r = unsafe { rd_enc_register(self.0, devptr, pitch, &mut slot, e.ptr(), 512) };
        e.check(r)?;
        Ok(slot)
    }

    /// Encodes one surface; blocks until the bitstream is ready.
    pub fn encode(&mut self, slot: i32, pts: u64, force_idr: bool) -> Result<Packet<'_>> {
        let mut e = Err::new();
        let mut p = RdPacket { data: ptr::null(), size: 0, pic_type: 0, avg_qp: 0, submit_to_ready_ms: 0.0 };
        let r = unsafe { rd_enc_encode(self.0, slot, pts, force_idr as c_int, &mut p, e.ptr(), 512) };
        e.check(r)?;
        Ok(Packet {
            data: unsafe { std::slice::from_raw_parts(p.data, p.size as usize) },
            pic_type: p.pic_type,
            avg_qp: p.avg_qp,
            ms: p.submit_to_ready_ms,
        })
    }

    /// Encodes one surface with per-picture options (QP delta map, motion hints,
    /// intra refresh); blocks until the bitstream is ready.
    pub fn encode_with(&mut self, slot: i32, pts: u64, o: &PicOpts) -> Result<Packet<'_>> {
        let mut e = Err::new();
        let mut p = RdPacket { data: ptr::null(), size: 0, pic_type: 0, avg_qp: 0, submit_to_ready_ms: 0.0 };
        let (mv_xy, mv_valid, n_mv) = match o.mv {
            Some((xy, valid)) => {
                assert_eq!(xy.len(), valid.len());
                (xy.as_ptr() as *const i16, valid.as_ptr(), xy.len() as u32)
            }
            None => (ptr::null(), ptr::null(), 0),
        };
        let ro = RdPicOpts {
            force_idr: o.force_idr as c_int,
            force_ir_frames: o.intra_refresh,
            qp_delta: o.qp_delta.map_or(ptr::null(), |m| m.as_ptr()),
            qp_delta_size: o.qp_delta.map_or(0, |m| m.len() as u32),
            mv_xy,
            mv_valid,
            n_mv,
        };
        let r = unsafe { rd_enc_encode2(self.0, slot, pts, &ro, &mut p, e.ptr(), 512) };
        e.check(r)?;
        Ok(Packet {
            data: unsafe { std::slice::from_raw_parts(p.data, p.size as usize) },
            pic_type: p.pic_type,
            avg_qp: p.avg_qp,
            ms: p.submit_to_ready_ms,
        })
    }

    /// (QP map cell, motion hint block) in pixels: (32, 16) for HEVC, (64, 64) for AV1.
    pub fn grid(&self) -> (u32, u32) {
        let (mut q, mut m) = (0, 0);
        unsafe { rd_enc_grid(self.0, &mut q, &mut m) };
        (q, m)
    }

    /// Bitrate, VBV (percent of one frame's budget), split mode and the rate controller's QP
    /// ceiling (before the QP delta map, codec units; 0: none), by one reconfigure: no reset,
    /// no IDR (takes effect on the next frame).
    pub fn set_rate(&mut self, kbps: u32, vbv_pct: u32, split: c_int, max_qp: u32) -> Result<()> {
        let mut e = Err::new();
        let r = unsafe { rd_enc_set_rate(self.0, kbps, vbv_pct, split, max_qp, e.ptr(), 512) };
        e.check(r)
    }

    /// Per-frame rate control. Verified: no IDR, takes effect on the next frame.
    #[allow(dead_code)]
    pub fn set_bitrate(&mut self, kbps: u32) -> Result<()> {
        let mut e = Err::new();
        let r = unsafe { rd_enc_set_bitrate(self.0, kbps, e.ptr(), 512) };
        e.check(r)
    }

    /// Loss recovery. Verified: the next frame predicts from an older one.
    pub fn invalidate(&mut self, pts: u64) -> Result<()> {
        let mut e = Err::new();
        let r = unsafe { rd_enc_invalidate(self.0, pts, e.ptr(), 512) };
        e.check(r)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { rd_enc_close(self.0) }
    }
}
