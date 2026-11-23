// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Opus through the C shim: 48 kHz stereo, 10 ms frames, restricted low delay at
//! 128 kbit/s, the same stream remoted sends (host/src/audio, core/src/audio.rs).

use std::ffi::c_int;

#[repr(C)]
struct EncRaw {
    _p: [u8; 0],
}
#[repr(C)]
struct DecRaw {
    _p: [u8; 0],
}

unsafe extern "C" {
    fn hd_opus_enc_new(bitrate: c_int) -> *mut EncRaw;
    fn hd_opus_encode(o: *mut EncRaw, pcm: *const f32, out: *mut u8, cap: c_int) -> c_int;
    fn hd_opus_enc_free(o: *mut EncRaw);
    fn hd_opus_dec_new() -> *mut DecRaw;
    fn hd_opus_decode(o: *mut DecRaw, data: *const u8, len: c_int, pcm: *mut f32, max_samples: c_int) -> c_int;
    fn hd_opus_dec_free(o: *mut DecRaw);
}

/// remoted's rate for desktop audio.
pub const BITRATE: i32 = 128_000;
/// Samples per channel in one packet (10 ms).
pub const FRAME: usize = 480;

pub struct OpusEncoder(*mut EncRaw);
unsafe impl Send for OpusEncoder {}

impl OpusEncoder {
    pub fn new() -> Option<OpusEncoder> {
        let p = unsafe { hd_opus_enc_new(BITRATE) };
        (!p.is_null()).then_some(OpusEncoder(p))
    }

    /// 10 ms of interleaved stereo → one packet.
    pub fn encode(&mut self, pcm: &[f32; FRAME * 2]) -> Option<Vec<u8>> {
        let mut out = vec![0u8; 1500];
        let n = unsafe { hd_opus_encode(self.0, pcm.as_ptr(), out.as_mut_ptr(), out.len() as c_int) };
        (n > 0).then(|| {
            out.truncate(n as usize);
            out
        })
    }
}

impl Drop for OpusEncoder {
    fn drop(&mut self) {
        unsafe { hd_opus_enc_free(self.0) }
    }
}

pub struct OpusDecoder(*mut DecRaw);
unsafe impl Send for OpusDecoder {}

impl OpusDecoder {
    pub fn new() -> Option<OpusDecoder> {
        let p = unsafe { hd_opus_dec_new() };
        (!p.is_null()).then_some(OpusDecoder(p))
    }

    /// One packet → interleaved stereo samples.
    pub fn decode(&mut self, packet: &[u8]) -> Option<Vec<f32>> {
        let mut pcm = vec![0f32; 5760 * 2];
        let n = unsafe { hd_opus_decode(self.0, packet.as_ptr(), packet.len() as c_int, pcm.as_mut_ptr(), 5760) };
        (n > 0).then(|| {
            pcm.truncate(n as usize * 2);
            pcm
        })
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        unsafe { hd_opus_dec_free(self.0) }
    }
}
