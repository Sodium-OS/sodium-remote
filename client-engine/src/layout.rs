// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Where the stream goes in the window, and where the pointer goes in the
//! stream.
//!
//! The host rounds REMOTE-1 down to a multiple of its scale's pixel period, so
//! the stream can be up to p−1 px smaller than the window on each axis. Then it
//! is drawn 1:1, centred in whole pixels with the odd pixel to the right and
//! bottom; the band is the window's background. Anything else (fixed
//! resolution, a size still in flight) is fitted with its aspect kept.

/// Within this many pixels per axis, the stream is drawn 1:1 (the largest
/// pixel period, 32 at 2.133, is well inside it).
pub const ONE_TO_ONE_BAND: u32 = 64;

/// The stream's rect in the window's buffer pixels: x, y, w, h.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Where a `stream`-sized picture goes in a `window` of buffer pixels.
pub fn place(window: (u32, u32), stream: (u32, u32)) -> Placement {
    let (bw, bh) = window;
    let (sw, sh) = stream;
    if sw == 0 || sh == 0 {
        return Placement { x: 0.0, y: 0.0, w: bw as f64, h: bh as f64 };
    }
    if sw <= bw && sh <= bh && bw - sw <= ONE_TO_ONE_BAND && bh - sh <= ONE_TO_ONE_BAND {
        // Whole pixels; the odd one goes right and bottom.
        return Placement { x: ((bw - sw) / 2) as f64, y: ((bh - sh) / 2) as f64, w: sw as f64, h: sh as f64 };
    }
    let k = (bw as f64 / sw as f64).min(bh as f64 / sh as f64);
    let (w, h) = (sw as f64 * k, sh as f64 * k);
    Placement { x: ((bw as f64 - w) / 2.0).floor(), y: ((bh as f64 - h) / 2.0).floor(), w, h }
}

/// A point in buffer pixels as stream pixels; a point in the band clamps to
/// the nearest stream edge.
pub fn to_stream(p: Placement, stream: (u32, u32), bx: f64, by: f64) -> (f64, f64) {
    let (sw, sh) = (stream.0 as f64, stream.1 as f64);
    let x = ((bx - p.x) * (sw / p.w)).clamp(0.0, (sw - 1.0).max(0.0));
    let y = ((by - p.y) * (sh / p.h)).clamp(0.0, (sh - 1.0).max(0.0));
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_to_one_with_an_odd_band_and_the_pointer_in_it() {
        // 2237×1349 window, stream rounded down to 2232×1344 (1.6's period 8):
        // 5 px each way, 2 left/top and 3 right/bottom.
        let p = place((2237, 1349), (2232, 1344));
        assert_eq!(p, Placement { x: 2.0, y: 2.0, w: 2232.0, h: 1344.0 });
        assert_eq!(to_stream(p, (2232, 1344), 102.5, 52.0), (100.5, 50.0));
        // In the band: clamped to the nearest edge.
        assert_eq!(to_stream(p, (2232, 1344), 0.0, 1.0), (0.0, 0.0));
        assert_eq!(to_stream(p, (2232, 1344), 2236.0, 700.0), (2231.0, 698.0));
        assert_eq!(to_stream(p, (2232, 1344), 1000.0, 1348.0), (998.0, 1343.0));
        // The band limit: 64 px is still 1:1, 65 is fitted.
        assert_eq!(place((2296, 1408), (2232, 1344)).w, 2232.0);
        assert!(place((2297, 1409), (2232, 1344)).w > 2232.0);
        // A fixed resolution in a bigger window: aspect fit.
        assert_eq!(place((2560, 1664), (1280, 832)), Placement { x: 0.0, y: 0.0, w: 2560.0, h: 1664.0 });
    }
}
