// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The ScreenCast node as a `Capture`: SHM pictures
//! as BGRx `CpuPicture`s, DMA-BUF pictures as `DmaPicture`s for a VA-API encoder,
//! damage from SPA_META_VideoDamage, the pointer from SPA_META_Cursor.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use remote_linux_media::video::{self, DmaBuf, NextError, VideoCapture};

use super::portal::Portal;
use crate::backend::{Capture, CaptureUpdate, CpuPicture, CursorShape, Picture, PointerState, Rect, SharingEnded};

static DMABUF_OFF: AtomicBool = AtomicBool::new(false);

/// A DMA-BUF import failed: captures re-open on shared memory from now on.
pub fn dmabuf_failed(why: &str) {
    if !DMABUF_OFF.swap(true, Ordering::Relaxed) {
        eprintln!("sodium-remote-host: DMA-BUF import failed ({why}); capturing to shared memory from now on");
    }
}

pub fn dmabuf_is_off() -> bool {
    DMABUF_OFF.load(Ordering::Relaxed)
}

/// A DMA-BUF picture, held from the compositor until the encoder is done with it.
pub struct DmaPicture {
    pub buf: DmaBuf,
}

impl Picture for DmaPicture {
    fn size(&self) -> (u32, u32) {
        (self.buf.width, self.buf.height)
    }
    fn bgra(&self) -> Option<(&[u8], usize)> {
        None
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct PortalCapture {
    portal: Arc<Portal>,
    generation: u64,
    video: VideoCapture,
    dmabuf: bool,
    opened: Instant,
}

impl PortalCapture {
    pub fn open(portal: &Arc<Portal>, fps: u32, dmabuf: bool) -> Result<PortalCapture> {
        let Some(a) = portal.active() else { return Err(SharingEnded.into()) };
        if portal.ended(a.generation) {
            return Err(SharingEnded.into());
        }
        let video = match portal.take_prepared(a.generation, dmabuf) {
            Some(v) => v,
            None => VideoCapture::open(portal.remote()?, a.node, fps, dmabuf)?,
        };
        Ok(PortalCapture { portal: portal.clone(), generation: a.generation, video, dmabuf, opened: Instant::now() })
    }
}

impl Capture for PortalCapture {
    fn next(&mut self, timeout: Duration) -> Result<CaptureUpdate> {
        if self.portal.ended(self.generation) {
            return Err(SharingEnded.into());
        }
        if self.dmabuf && dmabuf_is_off() {
            bail!("re-opening the capture on shared memory");
        }
        let mut dst = Vec::new();
        if let Ok(Some((w, h, false))) = self.video.size() {
            dst = vec![0u8; w as usize * h as usize * 4];
        }
        let u = match self.video.next(timeout, &mut dst) {
            Ok(u) => u,
            Err(NextError::Ended) => {
                // The node went away. The portal's Closed usually comes with it; a
                // stream that never started is an error worth a retry instead.
                if self.portal.ended(self.generation) || self.opened.elapsed() > Duration::from_secs(2) {
                    return Err(SharingEnded.into());
                }
                bail!("the PipeWire stream ended");
            }
            Err(NextError::Io(e)) => return Err(e.into()),
        };
        let mut out = CaptureUpdate::default();
        if let Some(p) = u.picture {
            let pic: Arc<dyn Picture> = match p {
                video::Picture::Cpu { width, height } => {
                    dst.truncate(width as usize * height as usize * 4);
                    Arc::new(CpuPicture { width, height, stride: width as usize * 4, data: dst })
                }
                video::Picture::DmaBuf(buf) => Arc::new(DmaPicture { buf }),
            };
            let rects = u.damage.iter().map(|r| Rect { x: r.x, y: r.y, w: r.w, h: r.h }).filter(|r| !r.is_empty()).collect();
            out.frame = Some((pic, rects));
            out.at = Some(Instant::now());
        }
        if let Some((x, y, visible)) = u.pointer {
            out.pointer = Some(PointerState { x, y, visible });
        }
        if let Some(s) = u.shape {
            out.shape = Some(CursorShape { width: s.width, height: s.height, hot_x: s.hot_x, hot_y: s.hot_y, pixels: s.pixels });
        }
        Ok(out)
    }
}
