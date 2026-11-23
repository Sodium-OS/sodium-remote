// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! A portal ScreenCast node over PipeWire (`csrc/lm_video.c`).

use std::os::fd::{IntoRawFd, OwnedFd};
use std::sync::Arc;
use std::time::Duration;

const MAX_DAMAGE: usize = 32;
const MAX_PLANES: usize = 4;
/// The largest pointer bitmap taken, pixels per side.
pub const CURSOR_MAX: usize = 256;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[repr(C)]
struct LmFrame {
    frame: i32,
    width: i32,
    height: i32,
    damage_n: i32,
    damage: [Rect; MAX_DAMAGE],
    dmabuf: i32,
    drm_format: u32,
    modifier: u64,
    planes: i32,
    fd: [i32; MAX_PLANES],
    offset: [u32; MAX_PLANES],
    stride: [u32; MAX_PLANES],
    buffer: *mut std::ffi::c_void,
    cursor: i32,
    cursor_visible: i32,
    cursor_x: i32,
    cursor_y: i32,
    shape: i32,
    shape_w: i32,
    shape_h: i32,
    hot_x: i32,
    hot_y: i32,
}

#[repr(C)]
struct LmVideo {
    _p: [u8; 0],
}

unsafe extern "C" {
    fn lm_video_open(pw_fd: i32, node: u32, fps: i32, dmabuf: i32, err: *mut i32) -> *mut LmVideo;
    fn lm_video_size(v: *mut LmVideo, w: *mut i32, h: *mut i32, dmabuf: *mut i32) -> i32;
    fn lm_video_next(v: *mut LmVideo, timeout_ms: i32, dst: *mut u8, dst_cap: i32, shape: *mut u8, shape_cap: i32, out: *mut LmFrame) -> i32;
    fn lm_video_release(v: *mut LmVideo, buffer: *mut std::ffi::c_void);
    fn lm_video_close(v: *mut LmVideo);
}

struct Handle(*mut LmVideo);
// The shim locks the stream's thread loop around every call.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { lm_video_close(self.0) }
    }
}

/// One DMA-BUF plane.
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub fd: i32,
    pub offset: u32,
    pub stride: u32,
}

/// A DMA-BUF picture, held from the producer until dropped. The fds belong to the
/// buffer: import them (`dup` if needed) before the drop.
pub struct DmaBuf {
    v: Arc<Handle>,
    buffer: *mut std::ffi::c_void,
    pub width: u32,
    pub height: u32,
    /// DRM fourcc (XR24, AR24, XB24, AB24).
    pub drm_format: u32,
    pub modifier: u64,
    pub planes: Vec<Plane>,
}

unsafe impl Send for DmaBuf {}
unsafe impl Sync for DmaBuf {}

impl Drop for DmaBuf {
    fn drop(&mut self) {
        unsafe { lm_video_release(self.v.0, self.buffer) }
    }
}

/// A new picture.
pub enum Picture {
    /// BGRx rows, `width * 4` bytes each, in the buffer passed to `next`.
    Cpu { width: u32, height: u32 },
    DmaBuf(DmaBuf),
}

/// The pointer's shape: premultiplied BGRA.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub width: u32,
    pub height: u32,
    pub hot_x: i32,
    pub hot_y: i32,
    pub pixels: Vec<u8>,
}

/// What one `next` saw.
#[derive(Default)]
pub struct Update {
    pub picture: Option<Picture>,
    /// What changed, stream pixels; empty means the whole picture.
    pub damage: Vec<Rect>,
    /// The hotspot (stream pixels) and whether the pointer shows.
    pub pointer: Option<(i32, i32, bool)>,
    pub shape: Option<Shape>,
}

/// Why `next` stopped.
#[derive(Debug)]
pub enum NextError {
    /// The node went away: the user stopped sharing, or the portal session ended.
    Ended,
    Io(std::io::Error),
}

pub struct VideoCapture {
    v: Arc<Handle>,
    shape: Vec<u8>,
}

impl VideoCapture {
    /// Opens `node` on the PipeWire remote the portal handed out (taken over). With
    /// `dmabuf`, DMA-BUF is offered before SHM.
    pub fn open(remote: OwnedFd, node: u32, fps: u32, dmabuf: bool) -> std::io::Result<VideoCapture> {
        let mut err = 0;
        let v = unsafe { lm_video_open(remote.into_raw_fd(), node, fps as i32, dmabuf as i32, &mut err) };
        if v.is_null() {
            return Err(crate::errno(err.min(-1)));
        }
        Ok(VideoCapture { v: Arc::new(Handle(v)), shape: vec![0; CURSOR_MAX * CURSOR_MAX * 4] })
    }

    /// The negotiated size and whether it is DMA-BUF, once known.
    pub fn size(&self) -> Result<Option<(u32, u32, bool)>, NextError> {
        let (mut w, mut h, mut d) = (0, 0, 0);
        match unsafe { lm_video_size(self.v.0, &mut w, &mut h, &mut d) } {
            r if r < 0 => Err(NextError::Ended),
            0 => Ok(None),
            _ => Ok(Some((w as u32, h as u32, d != 0))),
        }
    }

    /// Waits up to `timeout` for news. A CPU picture lands in `dst` (resized to fit).
    pub fn next(&mut self, timeout: Duration, dst: &mut Vec<u8>) -> Result<Update, NextError> {
        let mut f: LmFrame = unsafe { std::mem::zeroed() };
        let mut r = self.next_raw(timeout, dst, &mut f);
        if r == -libc_enospc() {
            if let Ok(Some((w, h, _))) = self.size() {
                dst.resize(w as usize * h as usize * 4, 0);
            }
            r = self.next_raw(Duration::ZERO, dst, &mut f);
        }
        if r == -32 {
            return Err(NextError::Ended);
        }
        if r < 0 {
            return Err(NextError::Io(crate::errno(r)));
        }
        let mut u = Update::default();
        if r == 0 {
            return Ok(u);
        }
        if f.frame != 0 {
            u.damage = f.damage[..f.damage_n.clamp(0, MAX_DAMAGE as i32) as usize].to_vec();
            u.picture = Some(if f.dmabuf != 0 {
                Picture::DmaBuf(DmaBuf {
                    v: self.v.clone(),
                    buffer: f.buffer,
                    width: f.width as u32,
                    height: f.height as u32,
                    drm_format: f.drm_format,
                    modifier: f.modifier,
                    planes: (0..f.planes.clamp(0, MAX_PLANES as i32) as usize)
                        .map(|i| Plane { fd: f.fd[i], offset: f.offset[i], stride: f.stride[i] })
                        .collect(),
                })
            } else {
                Picture::Cpu { width: f.width as u32, height: f.height as u32 }
            });
        }
        if f.cursor != 0 {
            u.pointer = Some((f.cursor_x, f.cursor_y, f.cursor_visible != 0));
        }
        if f.shape != 0 {
            let n = (f.shape_w * f.shape_h * 4) as usize;
            u.shape = Some(Shape { width: f.shape_w as u32, height: f.shape_h as u32, hot_x: f.hot_x, hot_y: f.hot_y, pixels: self.shape[..n].to_vec() });
        }
        Ok(u)
    }

    fn next_raw(&mut self, timeout: Duration, dst: &mut [u8], f: &mut LmFrame) -> i32 {
        unsafe {
            lm_video_next(
                self.v.0,
                timeout.as_millis().min(i32::MAX as u128) as i32,
                dst.as_mut_ptr(),
                dst.len().min(i32::MAX as usize) as i32,
                self.shape.as_mut_ptr(),
                self.shape.len() as i32,
                f,
            )
        }
    }
}

const fn libc_enospc() -> i32 {
    28
}
