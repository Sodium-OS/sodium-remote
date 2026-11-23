// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The pointer, sent beside the picture (ScreenCaptureKit's `showsCursor` is off):
//!
//! - **Position:** the global location of a fresh `CGEvent` (points, any thread), mapped
//!   to the display's pixels (`policy::mac_pixel`); hidden when it is on another display
//!   or `CGCursorIsVisible` says it is hidden (as while typing).
//! - **Shape:** `NSCursor.currentSystemCursor` (the cursor on screen, whichever app set
//!   it), read on the main thread: its image drawn at the display's scale into
//!   premultiplied BGRA, its hot spot scaled the same. Sent when its pixels change.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use objc2_app_kit::NSCursor;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
#[allow(deprecated)]
use objc2_core_graphics::CGCursorIsVisible;
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGEvent, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo};

use super::display::MacDisplay;
use super::main_thread::on_main;
use crate::backend::{CursorShape, PointerState};
use crate::policy::mac_pixel;

/// The pointer's global location, points.
pub fn location() -> CGPoint {
    let ev = CGEvent::new(None);
    CGEvent::location(ev.as_deref())
}

/// The current system cursor at `scale`, premultiplied BGRA. `currentSystemCursor` is
/// deprecated (Apple points at ScreenCaptureKit's `showsCursor`, which draws it into
/// the picture) but is still the only public way to the shape; when it gives nothing
/// the arrow stands in.
#[allow(deprecated)]
pub fn shape(scale: f64) -> Option<CursorShape> {
    on_main(Duration::from_millis(50), move |_| {
        let cursor = NSCursor::currentSystemCursor().unwrap_or_else(NSCursor::arrowCursor);
        let image = cursor.image();
        let size = image.size();
        let hot = cursor.hotSpot();
        let mut rect = objc2_foundation::NSRect::new(objc2_foundation::NSPoint::new(0.0, 0.0), size);
        let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        let w = ((size.width * scale).round() as usize).clamp(1, 256);
        let h = ((size.height * scale).round() as usize).clamp(1, 256);
        let pixels = draw(&cg, w, h)?;
        Some(CursorShape {
            width: w as u32,
            height: h as u32,
            hot_x: ((hot.x * scale).round() as i32).clamp(0, w as i32 - 1),
            hot_y: ((hot.y * scale).round() as i32).clamp(0, h as i32 - 1),
            pixels,
        })
    })
    .flatten()
}

/// A CGImage drawn into w × h premultiplied BGRA (B, G, R, A in memory).
fn draw(image: &CGImage, w: usize, h: usize) -> Option<Vec<u8>> {
    let mut pixels = vec![0u8; w * h * 4];
    let space = CGColorSpace::new_device_rgb()?;
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    let ctx = unsafe { CGBitmapContextCreate(pixels.as_mut_ptr().cast(), w, h, 8, w * 4, Some(&space), info) }?;
    CGContext::draw_image(Some(&ctx), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)), Some(image));
    drop(ctx);
    Some(pixels)
}

/// The pointer's state between polls.
#[derive(Default)]
pub struct CursorPoll {
    pos: Option<PointerState>,
    pos_at: Option<Instant>,
    shape_at: Option<Instant>,
    hash: u64,
}

impl CursorPoll {
    /// What changed: the position (every 16 ms at most), the shape (every 50 ms).
    /// `CGCursorIsVisible` is deprecated and may say "visible" always: then the pointer
    /// shows whenever it is on this display.
    #[allow(deprecated)]
    pub fn poll(&mut self, d: &MacDisplay) -> (Option<PointerState>, Option<CursorShape>) {
        let now = Instant::now();
        let mut out = (None, None);
        if self.pos_at.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(16)) {
            self.pos_at = Some(now);
            let p = location();
            let px = (d.display.info.width, d.display.info.height);
            let (x, y, inside) = mac_pixel(p.x, p.y, d.bounds, px);
            let state = PointerState { x, y, visible: inside && CGCursorIsVisible() };
            if self.pos != Some(state) {
                self.pos = Some(state);
                out.0 = Some(state);
            }
        }
        if self.shape_at.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(50)) {
            self.shape_at = Some(now);
            if let Some(s) = shape(d.display.info.scale as f64) {
                let mut h = DefaultHasher::new();
                (s.width, s.height, s.hot_x, s.hot_y, &s.pixels).hash(&mut h);
                let h = h.finish();
                if h != self.hash {
                    self.hash = h;
                    out.1 = Some(s);
                }
            }
        }
        out
    }
}
