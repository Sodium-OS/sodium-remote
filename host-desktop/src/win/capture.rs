// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! DXGI Desktop Duplication of one output.
//!
//! - **Frames**: `AcquireNextFrame` gives the desktop image with its move and dirty
//!   rectangles. The changed rows are copied into a BGRA picture in system memory (what
//!   the encoders convert from): straight from the desktop image when DXGI keeps it in
//!   system memory (`MapDesktopSurface`; the Basic Display Adapter, where going through
//!   a staging texture costs 20 ms a frame), else through a staging texture. A move's
//!   destination counts as damage: the image already has the moved pixels.
//! - **The pointer**: its position (DXGI gives the shape's top-left; we send the
//!   hotspot) and visibility on this output, and its shape when it changes, converted
//!   to premultiplied BGRA. Monochrome and masked shapes' inverting pixels can't be
//!   drawn by a client: they become black with a white outline (the text cursor).
//! - **Access loss** (a mode change, a full-screen app, the secure desktop) drops the
//!   duplication; it is re-created every 250 ms. While the input desktop isn't the
//!   user's (lock, login, UAC, Ctrl+Alt+Del), the capture reports `secure`, and again
//!   when it's back. A changed display size ends this capture, so the media loop opens
//!   a new one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{E_ACCESSDENIED, HMODULE, POINT, RECT};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_9_3, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_INVALID_CALL, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_MOVE_RECT,
    DXGI_OUTDUPL_POINTER_SHAPE_INFO, IDXGIOutput1, IDXGIOutput5, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use super::desktop::input_desktop_is_default;
use super::display::{Output, outputs};
use crate::backend::{Capture, CaptureUpdate, CpuPicture, CursorShape, Picture, PointerState, Rect};

const SHAPE_MONOCHROME: u32 = 1;
const SHAPE_COLOR: u32 = 2;
const SHAPE_MASKED_COLOR: u32 = 4;

pub struct DuplCapture {
    display: u32,
    out: Output,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    dupl: Option<IDXGIOutputDuplication>,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    /// The desktop image is in system memory: map it directly.
    sysmem: bool,
    pic: Arc<CpuPicture>,
    /// The next frame is sent whole (a new duplication).
    whole: bool,
    secure: bool,
    /// Report `secure` on the next call (a capture opened while the secure desktop is up).
    announce: bool,
    hotspot: POINT,
    last_retry: Option<Instant>,
    last_check: Instant,
    meta: Vec<u8>,
    shape: Vec<u8>,
    /// `SODIUM_REMOTE_HOST_DEBUG`: acquire and copy timings every 2 s.
    debug: Option<Debug>,
}

#[derive(Default)]
struct Debug {
    since: Option<Instant>,
    frames: u32,
    accumulated: u32,
    wait: Duration,
    copy: Duration,
}

// The D3D11 objects are used from the capture thread only.
unsafe impl Send for DuplCapture {}

impl DuplCapture {
    pub fn open(display: u32) -> Result<DuplCapture> {
        let out = outputs().into_iter().find(|o| o.display.info.id == display).with_context(|| format!("no display {display}"))?;
        let (mut device, mut context) = (None, None);
        let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_9_3];
        unsafe {
            D3D11CreateDevice(
                &out.adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("D3D11CreateDevice")?;
        }
        let (device, context) = (device.context("no D3D11 device")?, context.context("no D3D11 context")?);
        let (w, h) = (out.rect.w as u32, out.rect.h as u32);
        let mut c = DuplCapture {
            display,
            out,
            device,
            context,
            dupl: None,
            staging: None,
            sysmem: false,
            pic: Arc::new(CpuPicture::new(w, h)),
            whole: true,
            secure: false,
            announce: false,
            hotspot: POINT::default(),
            last_retry: None,
            last_check: Instant::now(),
            meta: vec![],
            shape: vec![],
            debug: std::env::var_os("SODIUM_REMOTE_HOST_DEBUG").map(|_| Debug::default()),
        };
        match c.duplicate() {
            Ok(()) => {}
            // A secure desktop is up: report it and keep trying (`next`).
            Err(e) if !input_desktop_is_default() => {
                eprintln!("sodium-remote-host: capture of display {display} waits for the secure screen to go ({e:#})");
                c.secure = true;
                c.announce = true;
            }
            Err(e) => return Err(e),
        }
        Ok(c)
    }

    fn duplicate(&mut self) -> Result<()> {
        let dupl = unsafe {
            // DuplicateOutput1 gives BGRA even on an HDR desktop; Windows 10 1703 and later.
            match self.out.output.cast::<IDXGIOutput5>() {
                Ok(o5) => o5.DuplicateOutput1(&self.device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM]),
                Err(_) => self.out.output.cast::<IDXGIOutput1>()?.DuplicateOutput(&self.device),
            }
        };
        let dupl = dupl.map_err(|e| {
            let why = if e.code() == E_ACCESSDENIED { " (no access: a secure desktop, or not the interactive session)" } else { "" };
            anyhow::anyhow!("DuplicateOutput: {e}{why}")
        })?;
        let desc = unsafe { dupl.GetDesc() };
        let (w, h) = (desc.ModeDesc.Width, desc.ModeDesc.Height);
        if (w, h) != (self.pic.width, self.pic.height) {
            bail!("display {} is now {w}x{h}", self.display);
        }
        self.sysmem = desc.DesktopImageInSystemMemory.as_bool();
        self.dupl = Some(dupl);
        self.whole = true;
        Ok(())
    }

    /// Without a duplication: try again (every 250 ms), and follow the secure desktop.
    fn recover(&mut self, timeout: Duration, u: &mut CaptureUpdate) -> Result<()> {
        if self.last_retry.is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
            std::thread::sleep(timeout.min(Duration::from_millis(50)));
            return Ok(());
        }
        self.last_retry = Some(Instant::now());
        match self.duplicate() {
            Ok(()) => {
                self.secure = false;
                u.secure = Some(false);
                Ok(())
            }
            Err(e) => {
                let secure = !input_desktop_is_default();
                if secure != self.secure {
                    self.secure = secure;
                    u.secure = Some(secure);
                }
                if secure {
                    Ok(())
                } else if format!("{e:#}").contains("is now") {
                    Err(e)
                } else {
                    // Another app holds the output for a moment (a full-screen switch).
                    std::thread::sleep(timeout.min(Duration::from_millis(50)));
                    Ok(())
                }
            }
        }
    }

    fn staging(&mut self, w: u32, h: u32) -> Result<ID3D11Texture2D> {
        if let Some((t, sw, sh)) = &self.staging
            && (*sw, *sh) == (w, h)
        {
            return Ok(t.clone());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut t = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut t)) }.context("staging texture")?;
        let t = t.context("no staging texture")?;
        self.staging = Some((t.clone(), w, h));
        Ok(t)
    }

    /// The changed rectangles of the frame just acquired (None: take it whole).
    fn rects(&mut self, dupl: &IDXGIOutputDuplication, info: &DXGI_OUTDUPL_FRAME_INFO) -> Option<Vec<Rect>> {
        let size = info.TotalMetadataBufferSize as usize;
        if size == 0 {
            return Some(vec![]);
        }
        self.meta.resize(size, 0);
        let mut out = vec![];
        unsafe {
            let mut need = 0u32;
            dupl.GetFrameMoveRects(size as u32, self.meta.as_mut_ptr().cast(), &mut need).ok()?;
            let moves = std::slice::from_raw_parts(self.meta.as_ptr().cast::<DXGI_OUTDUPL_MOVE_RECT>(), need as usize / size_of::<DXGI_OUTDUPL_MOVE_RECT>());
            out.extend(moves.iter().map(|m| rect(&m.DestinationRect)));
            let mut need = 0u32;
            dupl.GetFrameDirtyRects(size as u32, self.meta.as_mut_ptr().cast(), &mut need).ok()?;
            let dirty = std::slice::from_raw_parts(self.meta.as_ptr().cast::<RECT>(), need as usize / size_of::<RECT>());
            out.extend(dirty.iter().map(rect));
        }
        Some(out)
    }

    /// Copies the acquired image's changed rows into the picture.
    fn copy(&mut self, dupl: &IDXGIOutputDuplication, res: &IDXGIResource, rects: &[Rect], whole: bool) -> Result<()> {
        if self.sysmem {
            match unsafe { dupl.MapDesktopSurface() } {
                Ok(m) => {
                    self.copy_rows(m.pBits, m.Pitch as usize, rects, whole);
                    let _ = unsafe { dupl.UnMapDesktopSurface() };
                    return Ok(());
                }
                Err(_) => self.sysmem = false,
            }
        }
        let tex: ID3D11Texture2D = res.cast()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { tex.GetDesc(&mut desc) };
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            bail!("the desktop image is {:?}, not BGRA", desc.Format);
        }
        if (desc.Width, desc.Height) != (self.pic.width, self.pic.height) {
            bail!("display {} is now {}x{}", self.display, desc.Width, desc.Height);
        }
        let staging = self.staging(desc.Width, desc.Height)?;
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context.CopyResource(&staging, &tex);
            self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).context("Map")?;
        }
        self.copy_rows(m.pData as *const u8, m.RowPitch as usize, rects, whole);
        unsafe { self.context.Unmap(&staging, 0) };
        Ok(())
    }

    /// The rows under `rects` (or all of them) from a mapped desktop image.
    fn copy_rows(&mut self, src: *const u8, pitch: usize, rects: &[Rect], whole: bool) {
        let pic = Arc::make_mut(&mut self.pic);
        let (w, h) = (pic.width as i32, pic.height as i32);
        let full = [Rect { x: 0, y: 0, w, h }];
        let rects = if whole { &full[..] } else { rects };
        for r in rects {
            let x0 = r.x.clamp(0, w);
            let x1 = (r.x + r.w).clamp(0, w);
            let (y0, y1) = (r.y.clamp(0, h), (r.y + r.h).clamp(0, h));
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let bytes = (x1 - x0) as usize * 4;
            for y in y0..y1 {
                let s = unsafe { std::slice::from_raw_parts(src.add(y as usize * pitch + x0 as usize * 4), bytes) };
                let d = y as usize * pic.stride + x0 as usize * 4;
                pic.data[d..d + bytes].copy_from_slice(s);
            }
        }
    }

    fn pointer_shape(&mut self, dupl: &IDXGIOutputDuplication, size: u32) -> Option<CursorShape> {
        self.shape.resize(size as usize, 0);
        let mut need = 0u32;
        let mut info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        unsafe { dupl.GetFramePointerShape(size, self.shape.as_mut_ptr().cast(), &mut need, &mut info) }.ok()?;
        self.hotspot = info.HotSpot;
        convert_shape(&self.shape, &info)
    }
}

fn rect(r: &RECT) -> Rect {
    Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
}

impl Capture for DuplCapture {
    fn next(&mut self, timeout: Duration) -> Result<CaptureUpdate> {
        let mut u = CaptureUpdate::default();
        if std::mem::take(&mut self.announce) {
            u.secure = Some(self.secure);
        }
        // A secure desktop doesn't always cost the duplication at once: look once a second.
        if self.dupl.is_some() && self.last_check.elapsed() > Duration::from_secs(1) {
            self.last_check = Instant::now();
            if !input_desktop_is_default() {
                self.dupl = None;
                self.secure = true;
                u.secure = Some(true);
                return Ok(u);
            }
        }
        if self.dupl.is_none() {
            self.recover(timeout, &mut u)?;
            if self.dupl.is_none() {
                return Ok(u);
            }
        }
        let dupl = self.dupl.clone().unwrap();
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        let t0 = Instant::now();
        let acquired = unsafe { dupl.AcquireNextFrame(timeout.as_millis() as u32, &mut info, &mut res) };
        if let Some(d) = &mut self.debug {
            d.wait += t0.elapsed();
        }
        match acquired {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(u),
            // The secure desktop shows up as ACCESS_LOST, or as INVALID_CALL (a UAC prompt).
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST || e.code() == DXGI_ERROR_INVALID_CALL || !input_desktop_is_default() => {
                self.dupl = None;
                self.last_retry = None;
                if !input_desktop_is_default() {
                    self.secure = true;
                    u.secure = Some(true);
                }
                return Ok(u);
            }
            Err(e) => bail!("AcquireNextFrame: {e}"),
        }
        let r = (|| -> Result<()> {
            if info.LastPresentTime != 0 || self.whole {
                if let Some(res) = &res {
                    let whole = self.whole;
                    let rects = self.rects(&dupl, &info);
                    let whole = whole || rects.is_none();
                    let rects = rects.unwrap_or_default();
                    if whole || !rects.is_empty() {
                        let t = Instant::now();
                        self.copy(&dupl, res, &rects, whole)?;
                        if let Some(d) = &mut self.debug {
                            d.copy += t.elapsed();
                            d.frames += 1;
                            d.accumulated += info.AccumulatedFrames;
                        }
                        self.whole = false;
                        u.frame = Some((self.pic.clone() as Arc<dyn Picture>, if whole { vec![] } else { rects }));
                        u.at = Some(Instant::now());
                    }
                }
            }
            if info.LastMouseUpdateTime != 0 {
                if info.PointerShapeBufferSize > 0 {
                    u.shape = self.pointer_shape(&dupl, info.PointerShapeBufferSize);
                }
                let p = info.PointerPosition;
                u.pointer = Some(PointerState { x: p.Position.x + self.hotspot.x, y: p.Position.y + self.hotspot.y, visible: p.Visible.as_bool() });
            }
            Ok(())
        })();
        let _ = unsafe { dupl.ReleaseFrame() };
        if let Some(d) = &mut self.debug {
            let since = *d.since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_secs(2) {
                let n = d.frames.max(1);
                eprintln!(
                    "sodium-remote-host: capture debug: {:.1} frames/s, {:.2} presents per frame, wait {:.1} ms, copy {:.1} ms per frame ({})",
                    d.frames as f64 / since.elapsed().as_secs_f64(),
                    d.accumulated as f64 / n as f64,
                    d.wait.as_secs_f64() * 1000.0 / n as f64,
                    d.copy.as_secs_f64() * 1000.0 / n as f64,
                    if self.sysmem { "desktop image in system memory" } else { "staging texture" }
                );
                *d = Debug { since: Some(Instant::now()), ..Debug::default() };
            }
        }
        r?;
        Ok(u)
    }
}

/// A DXGI pointer shape → premultiplied BGRA.
fn convert_shape(buf: &[u8], info: &DXGI_OUTDUPL_POINTER_SHAPE_INFO) -> Option<CursorShape> {
    let (w, pitch) = (info.Width as usize, info.Pitch as usize);
    let h = if info.Type == SHAPE_MONOCHROME { info.Height as usize / 2 } else { info.Height as usize };
    if w == 0 || h == 0 || w > 512 || h > 512 {
        return None;
    }
    let mut px = vec![0u8; w * h * 4];
    // Pixels that invert the screen: drawn black, outlined in white afterwards.
    let mut invert = vec![false; w * h];
    match info.Type {
        SHAPE_MONOCHROME => {
            let bit = |row: usize, x: usize| buf.get(row * pitch + x / 8).is_some_and(|b| b & (0x80 >> (x % 8)) != 0);
            for y in 0..h {
                for x in 0..w {
                    let (and, xor) = (bit(y, x), bit(y + h, x));
                    let i = (y * w + x) * 4;
                    match (and, xor) {
                        (false, false) => px[i..i + 4].copy_from_slice(&[0, 0, 0, 0xFF]),
                        (false, true) => px[i..i + 4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]),
                        (true, false) => {}
                        (true, true) => {
                            px[i..i + 4].copy_from_slice(&[0, 0, 0, 0xFF]);
                            invert[y * w + x] = true;
                        }
                    }
                }
            }
        }
        SHAPE_COLOR => {
            let rows: Vec<&[u8]> = (0..h).map(|y| buf.get(y * pitch..y * pitch + w * 4)).collect::<Option<_>>()?;
            // Straight alpha, unless every colour is already within its alpha.
            let premultiplied = rows.iter().all(|r| r.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
            for (y, r) in rows.iter().enumerate() {
                for (x, p) in r.chunks(4).enumerate() {
                    let a = p[3] as u32;
                    let m = |c: u8| if premultiplied { c } else { (c as u32 * a / 255) as u8 };
                    let i = (y * w + x) * 4;
                    px[i..i + 4].copy_from_slice(&[m(p[0]), m(p[1]), m(p[2]), p[3]]);
                }
            }
        }
        SHAPE_MASKED_COLOR => {
            for y in 0..h {
                let r = buf.get(y * pitch..y * pitch + w * 4)?;
                for (x, p) in r.chunks(4).enumerate() {
                    let i = (y * w + x) * 4;
                    if p[3] == 0 {
                        px[i..i + 4].copy_from_slice(&[p[0], p[1], p[2], 0xFF]);
                    } else if p[0] | p[1] | p[2] != 0 {
                        px[i..i + 4].copy_from_slice(&[0, 0, 0, 0xFF]);
                        invert[y * w + x] = true;
                    }
                }
            }
        }
        _ => return None,
    }
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            if px[i + 3] != 0 {
                continue;
            }
            let near = [(0i32, -1i32), (0, 1), (-1, 0), (1, 0)].iter().any(|(dx, dy)| {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h && invert[ny as usize * w + nx as usize]
            });
            if near {
                px[i..i + 4].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
            }
        }
    }
    Some(CursorShape { width: w as u32, height: h as u32, hot_x: info.HotSpot.x, hot_y: info.HotSpot.y, pixels: px })
}
