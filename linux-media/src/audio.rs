// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! PipeWire audio: the shim's C API (remoted builds its sink, microphone, metadata and
//! volume handling on it) and the default sink's monitor (the generic Linux host).

use std::ffi::{CString, c_int};
use std::time::Duration;

/// The audio shim's functions (`csrc/audio_shim.h`). Return values: 0 or a count, or a
/// negative errno.
pub mod ffi {
    use std::ffi::{c_char, c_int};

    #[repr(C)]
    pub struct RaAudio {
        _p: [u8; 0],
    }

    unsafe extern "C" {
        pub fn ra_open(err: *mut c_int) -> *mut RaAudio;
        pub fn ra_close(ra: *mut RaAudio);
        pub fn ra_meta_get(ra: *mut RaAudio, key: *const c_char, out: *mut c_char, cap: c_int) -> c_int;
        pub fn ra_meta_set(ra: *mut RaAudio, key: *const c_char, json: *const c_char) -> c_int;
        pub fn ra_node_volume_get(ra: *mut RaAudio, name: *const c_char, vols: *mut f32, max: c_int, mute: *mut c_int) -> c_int;
        pub fn ra_node_volume_set(ra: *mut RaAudio, name: *const c_char, vols: *const f32, n: c_int, mute: c_int) -> c_int;
        pub fn ra_node_exists(ra: *mut RaAudio, name: *const c_char) -> c_int;
        pub fn ra_sink_start(ra: *mut RaAudio, name: *const c_char, desc: *const c_char, bitrate: c_int) -> c_int;
        pub fn ra_sink_read(ra: *mut RaAudio, out: *mut u8, cap: c_int, timeout_ms: c_int) -> c_int;
        pub fn ra_sink_stop(ra: *mut RaAudio);
        pub fn ra_sink_free(ra: *mut RaAudio);
        pub fn ra_mic_start(ra: *mut RaAudio, name: *const c_char, desc: *const c_char) -> c_int;
        pub fn ra_mic_write(ra: *mut RaAudio, opus: *const u8, len: c_int) -> c_int;
        pub fn ra_mic_stop(ra: *mut RaAudio);
        pub fn ra_monitor_start(ra: *mut RaAudio, name: *const c_char) -> c_int;
        pub fn ra_monitor_read(ra: *mut RaAudio, out: *mut f32, frames: c_int, timeout_ms: c_int) -> c_int;
        pub fn ra_monitor_stop(ra: *mut RaAudio);
    }
}

/// What plays on the default output: a capture stream WirePlumber links to the default
/// sink's monitor (and moves when the default changes). 48 kHz, stereo, f32.
pub struct Monitor {
    ra: *mut ffi::RaAudio,
}

// The shim locks PipeWire's thread loop around every call; the ring has one reader.
unsafe impl Send for Monitor {}

impl Monitor {
    /// Connects to the user's PipeWire and starts the stream. `node_name` names it in
    /// `pw-top` and `wpctl status`.
    pub fn open(node_name: &str) -> std::io::Result<Monitor> {
        let mut err = 0;
        let ra = unsafe { ffi::ra_open(&mut err) };
        if ra.is_null() {
            return Err(crate::errno(err.min(-1)));
        }
        let name = CString::new(node_name).unwrap_or_default();
        let r = unsafe { ffi::ra_monitor_start(ra, name.as_ptr()) };
        if r < 0 {
            unsafe { ffi::ra_close(ra) };
            return Err(crate::errno(r));
        }
        Ok(Monitor { ra })
    }

    /// Fills `out` (interleaved stereo) when that much arrived within `timeout`: true;
    /// false on a timeout (nothing playing: PipeWire stops a monitor with no source).
    pub fn read(&mut self, out: &mut [f32], timeout: Duration) -> std::io::Result<bool> {
        let frames = (out.len() / 2) as c_int;
        let r = unsafe { ffi::ra_monitor_read(self.ra, out.as_mut_ptr(), frames, timeout.as_millis() as c_int) };
        if r < 0 {
            return Err(crate::errno(r));
        }
        Ok(r > 0)
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        unsafe {
            ffi::ra_monitor_stop(self.ra);
            ffi::ra_close(self.ra);
        }
    }
}
