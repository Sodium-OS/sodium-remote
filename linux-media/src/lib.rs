// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Sodium Remote's PipeWire media on Linux, shared by remoted
//! (the SodiumOS host) and the generic Linux host (`host-desktop`'s `linux` backend).
//!
//! - [`audio::ffi`]: the audio shim's C API (`csrc/audio_shim.c`): the "Remote" sink and
//!   "Remote microphone" source remoted makes, WirePlumber's default metadata, node
//!   volumes, Opus.
//! - [`audio::Monitor`]: the default sink's monitor, 48 kHz stereo float, for a host
//!   that streams the computer's own output (the generic Linux host).
//! - [`video::VideoCapture`]: a portal ScreenCast node over the portal's PipeWire remote
//!   (`csrc/lm_video.c`): BGRx pictures or DMA-BUF planes, damage, the pointer.
#![cfg(target_os = "linux")]

pub mod audio;
pub mod video;

/// A negative errno from the shims as an `io::Error`.
pub(crate) fn errno(r: i32) -> std::io::Error {
    std::io::Error::from_raw_os_error(-r)
}
