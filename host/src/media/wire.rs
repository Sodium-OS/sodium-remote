// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The small format media defines for the stream (big-endian, like
//! remote/docs/reference/protocol.md): per-frame metadata. The cursor image is core's
//! `msg::CursorImage`, not defined here.

use crate::capture::Rect;

pub const META_VERSION: u8 = 1;
pub const MAX_META_RECTS: usize = 32;

/// `OutgoingFrame.meta`: `u8 version | u8 class | u16 n | n × (u16 x, y, w, h)`, the
/// region this frame changes on screen, in buffer pixels. More than 32 rects go as
/// their bounding box.
pub fn frame_meta(class: u8, rects: &[Rect], width: u32, height: u32) -> Vec<u8> {
    let clip = |r: &Rect| {
        let x0 = r.x.clamp(0, width as i32);
        let y0 = r.y.clamp(0, height as i32);
        let x1 = (r.x.saturating_add(r.w)).clamp(0, width as i32);
        let y1 = (r.y.saturating_add(r.h)).clamp(0, height as i32);
        (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    };
    let mut rs: Vec<Rect> = rects.iter().filter_map(clip).collect();
    if rs.len() > MAX_META_RECTS {
        let x0 = rs.iter().map(|r| r.x).min().unwrap();
        let y0 = rs.iter().map(|r| r.y).min().unwrap();
        let x1 = rs.iter().map(|r| r.x + r.w).max().unwrap();
        let y1 = rs.iter().map(|r| r.y + r.h).max().unwrap();
        rs = vec![Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }];
    }
    let mut b = Vec::with_capacity(4 + rs.len() * 8);
    b.push(META_VERSION);
    b.push(class);
    b.extend_from_slice(&(rs.len() as u16).to_be_bytes());
    for r in rs {
        for v in [r.x, r.y, r.w, r.h] {
            b.extend_from_slice(&(v as u16).to_be_bytes());
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_clips_and_folds_into_a_bounding_box() {
        let m = frame_meta(1, &[Rect { x: -5, y: 10, w: 20, h: 4 }], 100, 100);
        assert_eq!(m, [1, 1, 0, 1, 0, 0, 0, 10, 0, 15, 0, 4]);
        let many: Vec<Rect> = (0..40).map(|i| Rect { x: i * 2, y: i, w: 2, h: 1 }).collect();
        let m = frame_meta(0, &many, 1000, 1000);
        assert_eq!(m, [1, 0, 0, 1, 0, 0, 0, 0, 0, 80, 0, 40]);
    }
}
