// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Capture sources: where frames come from, as DMA-BUFs.
//!
//! - [`plugin::PluginSource`]: the sodiumwm plugin hook (docs/reference/capture-ipc.md v1),
//!   exact damage, explicit-sync fences, the main path.
//! - [`wayland::WaylandSource`]: ext-image-copy-capture-v1, the fallback when the
//!   plugin isn't loaded (Hyprland sends full damage there).
//!
//! Both hand out frames that point into a small ring of buffers. The pipeline
//! imports each buffer into Vulkan once per `generation` and gives a frame back
//! with [`CaptureSource::release`] once its compute pass has read it.

pub mod gbm;
pub mod plugin;
pub mod wayland;

use std::os::fd::OwnedFd;

use anyhow::Result;

/// `DRM_FORMAT_XRGB8888` ('XR24'), the contract's default.
pub const DRM_FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
pub const DRM_FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
pub const DRM_FORMAT_XBGR8888: u32 = u32::from_le_bytes(*b"XB24");
pub const DRM_FORMAT_ABGR8888: u32 = u32::from_le_bytes(*b"AB24");

#[derive(Debug)]
pub struct DmabufPlane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

/// One buffer of the ring, as a DMA-BUF.
#[derive(Debug)]
pub struct Dmabuf {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub modifier: u64,
    pub planes: Vec<DmabufPlane>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A captured frame: which ring buffer holds it, and when it was made.
#[derive(Debug)]
pub struct Frame {
    pub generation: u32,
    pub buffer: u32,
    pub seq: u64,
    /// CLOCK_MONOTONIC ns when the compositor finished the frame (render or present).
    pub t_rendered_ns: u64,
    /// CLOCK_MONOTONIC ns when remoted had the frame in hand.
    pub t_ready_ns: u64,
    pub damage: Vec<Rect>,
    /// sync_file to wait on before reading the buffer, if any.
    pub acquire: Option<OwnedFd>,
}

pub trait CaptureSource {
    /// The current generation and its buffers. Changes after a resize or format change.
    fn buffers(&self) -> (u32, &[Dmabuf]);
    /// Blocks until the next frame is ready.
    fn next_frame(&mut self) -> Result<Frame>;
    /// The pipeline is done reading this frame's buffer.
    fn release(&mut self, frame: &Frame) -> Result<()>;
}

pub fn monotonic_ns() -> u64 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}
