// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The host's small rules, pure so they are tested on any OS: the codec, the
//! stream size, the refresh after stillness and the encoder's rate,
//! and pointer coordinates.

use std::time::{Duration, Instant};

use remote_core::msg::{Codec, CodecChoice, DecodeSupport};

use crate::backend::EncoderCaps;

/// The codec for this client: HEVC when the host has a hardware
/// HEVC encoder and the client decodes it, else H.264 (`DecodeSupport::use_h264`).
/// The client's "HEVC" choice wins when both sides have HEVC; AV1 isn't encoded here
///, so that choice falls back to the automatic one. None: no codec
/// in common.
pub fn choose_codec(caps: EncoderCaps, decode: &DecodeSupport, choice: CodecChoice) -> Option<Codec> {
    let hevc = caps.hevc && decode.hevc;
    let h264 = caps.h264 && decode.h264;
    if choice == CodecChoice::Hevc420 && hevc {
        return Some(Codec::Hevc420);
    }
    if DecodeSupport::use_h264(Some(*decode), caps.hevc, caps.h264) {
        return Some(Codec::H264);
    }
    if hevc {
        Some(Codec::Hevc420)
    } else if h264 {
        Some(Codec::H264)
    } else {
        None
    }
}

/// The stream's size for a display: native pixels, scaled down
/// only to fit the client's decoder (`max_width`/`max_height`, 0 = no limit), aspect
/// kept, even (4:2:0).
pub fn fit(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let mut s = 1.0f64;
    if max_width > 0 && width > max_width {
        s = s.min(max_width as f64 / width as f64);
    }
    if max_height > 0 && height > max_height {
        s = s.min(max_height as f64 / height as f64);
    }
    let even = |v: f64| ((v.floor() as u32) & !1).max(2);
    (even(width as f64 * s), even(height as f64 * s))
}

/// How long the picture must stay still before its one high-quality refresh.
pub const REFRESH_AFTER: Duration = Duration::from_millis(150);

/// "One high-quality refresh about 150 ms after the last change": a change (any frame
/// sent for damage, or a keyframe) arms it; it fires once; the next change re-arms it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Refresh {
    last_change: Option<Instant>,
    done: bool,
}

impl Refresh {
    pub fn changed(&mut self, now: Instant) {
        self.last_change = Some(now);
        self.done = false;
    }

    /// When the refresh is due; None when there is nothing to refresh.
    pub fn due(&self) -> Option<Instant> {
        if self.done { None } else { self.last_change.map(|t| t + REFRESH_AFTER) }
    }

    /// True once, when the refresh is due at `now`.
    pub fn take(&mut self, now: Instant) -> bool {
        if self.due().is_some_and(|d| now >= d) {
            self.done = true;
            true
        } else {
            false
        }
    }
}

/// For an encoder whose rate is fixed at open: when to open a new one at core's target.
/// A drop to under 60% of the encoder's rate acts at once (the link can't carry it);
/// any other change of more than a third (up or down) acts once it has lasted 2 s.
/// Each re-open costs a keyframe, so never more than one per 500 ms.
#[derive(Clone, Copy, Debug)]
pub struct RateFollower {
    pub current: u64,
    off_since: Option<Instant>,
    last: Option<Instant>,
}

impl RateFollower {
    pub fn new(current: u64) -> Self {
        RateFollower { current, off_since: None, last: None }
    }

    /// Whether to re-open at `target` now. When true, `current` is `target`.
    pub fn reopen(&mut self, target: u64, now: Instant) -> bool {
        let r = target as f64 / self.current.max(1) as f64;
        let off = !(0.75..=1.33).contains(&r);
        if !off {
            self.off_since = None;
            return false;
        }
        let since = *self.off_since.get_or_insert(now);
        let urgent = r < 0.6;
        let lasted = now.duration_since(since) >= Duration::from_secs(2);
        let spaced = self.last.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(500));
        if (urgent || lasted) && spaced {
            self.current = target;
            self.off_since = None;
            self.last = Some(now);
            return true;
        }
        false
    }
}

/// The shortest gap between two keyframes the client or core asked for (
/// review): a loss burst becomes one keyframe, not a storm.
pub const KEYFRAME_GAP: Duration = Duration::from_millis(250);

/// Requested keyframes (the client's `KeyframeRequest`, core's `RefHint`s), coalesced
///: at most one per 250 ms, a request inside that gap waits
/// for its end, and a hint naming a frame older than the last keyframe sent is stale
/// (that keyframe already replaced it) and dropped. Keyframes the stream itself needs
/// (its start, a new encoder, the end of a secure screen) don't go through the gate,
/// but each one sent resets it.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyframeGate {
    /// When the last keyframe went out, and its frame id in this connection.
    last: Option<(Instant, Option<u32>)>,
    pending: bool,
}

impl KeyframeGate {
    /// The client asked for a keyframe, or core says it has nothing to predict from.
    pub fn request(&mut self) {
        self.pending = true;
    }

    /// Core's hint that the client lost frames after `acked` (its last good frame).
    pub fn lost_after(&mut self, acked: u32) {
        let stale = self.last.is_some_and(|(_, id)| id.is_some_and(|k| seq_older(acked, k)));
        if !stale {
            self.pending = true;
        }
    }

    /// A keyframe went out (`id`: its frame id, when core gave one).
    pub fn sent(&mut self, id: Option<u32>, now: Instant) {
        self.last = Some((now, id));
        self.pending = false;
    }

    /// When the pending request may be sent (None: nothing pending).
    pub fn due(&self) -> Option<Instant> {
        self.pending.then(|| self.last.map_or_else(Instant::now, |(t, _)| t + KEYFRAME_GAP))
    }

    /// A requested keyframe is due at `now`.
    pub fn ready(&self, now: Instant) -> bool {
        self.due().is_some_and(|d| now >= d)
    }
}

/// `a` is older than `b` in frame ids that wrap (core's u32 ids).
pub fn seq_older(a: u32, b: u32) -> bool {
    a != b && b.wrapping_sub(a) < 1 << 31
}

/// A pixel of a display → macOS's global display coordinates (points, the main
/// display's top-left at 0,0, y down), where `CGEvent`s are posted. `bounds` is the
/// display's `CGDisplayBounds` (points); `pixels` its size in pixels (the stream's
/// display size: the mode's backing pixels, twice the points on a Retina display).
pub fn mac_point(x: f64, y: f64, bounds: (f64, f64, f64, f64), pixels: (u32, u32)) -> (f64, f64) {
    let (bx, by, bw, bh) = bounds;
    let sx = bw / pixels.0.max(1) as f64;
    let sy = bh / pixels.1.max(1) as f64;
    (bx + (x * sx).clamp(0.0, (bw - sx).max(0.0)), by + (y * sy).clamp(0.0, (bh - sy).max(0.0)))
}

/// The inverse: a global point (the pointer's location) → the display's pixels, and
/// whether it is on that display.
pub fn mac_pixel(px: f64, py: f64, bounds: (f64, f64, f64, f64), pixels: (u32, u32)) -> (i32, i32, bool) {
    let (bx, by, bw, bh) = bounds;
    let inside = px >= bx && py >= by && px < bx + bw && py < by + bh;
    let x = ((px - bx) * pixels.0 as f64 / bw.max(1.0)).floor() as i32;
    let y = ((py - by) * pixels.1 as f64 / bh.max(1.0)).floor() as i32;
    (x, y, inside)
}

/// A point in stream pixels → the display's pixels (the stream may be scaled down).
pub fn stream_to_display(x: f64, y: f64, stream: (u32, u32), display: (u32, u32)) -> (f64, f64) {
    let sx = display.0 as f64 / stream.0.max(1) as f64;
    let sy = display.1 as f64 / stream.1.max(1) as f64;
    ((x * sx).clamp(0.0, display.0 as f64 - 1.0), (y * sy).clamp(0.0, display.1 as f64 - 1.0))
}

/// A pixel of the virtual desktop (`left`, `top`, `width`, `height`: all displays'
/// bounding box) → `SendInput`'s `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`
/// units, 0..=65535 on each axis. Windows maps a unit `d` back to `left + d * width /
/// 65536` (rounded down); aiming at the pixel's centre lands on exactly that pixel.
pub fn virtual_desk_abs(x: i32, y: i32, left: i32, top: i32, width: i32, height: i32) -> (i32, i32) {
    let f = |p: i32, origin: i32, size: i32| {
        let size = size.max(1) as i64;
        let rel = ((p - origin) as i64).clamp(0, size - 1);
        ((2 * rel + 1) * 65536 / (2 * size)).min(65535) as i32
    };
    (f(x, left, width), f(y, top, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(hevc: bool, h264: bool) -> DecodeSupport {
        DecodeSupport { hevc, h264, hevc_444: false, av1: false, max_width: 0, max_height: 0 }
    }

    #[test]
    fn hevc_only_with_a_hardware_encoder_else_h264() {
        let gpu = EncoderCaps { hevc: true, h264: true };
        let vm = EncoderCaps { hevc: false, h264: true };
        assert_eq!(choose_codec(gpu, &d(true, true), CodecChoice::Auto), Some(Codec::Hevc420));
        assert_eq!(choose_codec(vm, &d(true, true), CodecChoice::Auto), Some(Codec::H264));
        assert_eq!(choose_codec(gpu, &d(false, true), CodecChoice::Auto), Some(Codec::H264));
        assert_eq!(choose_codec(gpu, &d(true, false), CodecChoice::Auto), Some(Codec::Hevc420));
        // AV1 asked for: not encoded here, so automatic.
        assert_eq!(choose_codec(vm, &d(true, true), CodecChoice::Av1), Some(Codec::H264));
        // Nothing in common.
        assert_eq!(choose_codec(vm, &d(true, false), CodecChoice::Auto), None);
    }

    #[test]
    fn the_stream_shrinks_only_to_fit_the_decoder() {
        assert_eq!(fit(3840, 2160, 0, 0), (3840, 2160));
        assert_eq!(fit(3840, 2160, 1920, 1080), (1920, 1080));
        assert_eq!(fit(2560, 1600, 1920, 0), (1920, 1200));
        assert_eq!(fit(1921, 1081, 4096, 4096), (1920, 1080)); // even
    }

    #[test]
    fn one_refresh_150_ms_after_the_last_change() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut r = Refresh::default();
        assert_eq!(r.due(), None);
        r.changed(t0);
        r.changed(ms(100)); // still changing: the clock restarts
        assert!(!r.take(ms(200)));
        assert_eq!(r.due(), Some(ms(250)));
        assert!(r.take(ms(250)));
        assert!(!r.take(ms(400)), "only once");
        assert_eq!(r.due(), None);
        r.changed(ms(500));
        assert!(r.take(ms(650)));
    }

    #[test]
    fn a_fixed_rate_encoder_follows_lasting_changes_and_drops_at_once() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut f = RateFollower::new(8_000_000);
        assert!(!f.reopen(9_000_000, t0), "within a third");
        assert!(!f.reopen(20_000_000, ms(100)));
        assert!(!f.reopen(20_000_000, ms(2000)));
        assert!(f.reopen(20_000_000, ms(2100)), "up after 2 s");
        assert_eq!(f.current, 20_000_000);
        assert!(!f.reopen(4_000_000, ms(2300)), "at most one per 500 ms");
        assert!(f.reopen(4_000_000, ms(2600)), "a big drop acts at once");
    }

    #[test]
    fn a_loss_burst_gives_one_keyframe_and_stale_hints_are_dropped() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut g = KeyframeGate::default();
        g.sent(Some(10), t0); // the stream's first keyframe, frame 10
        // A burst: hints for frames after the keyframe, and a request, inside 250 ms.
        g.lost_after(12);
        g.lost_after(13);
        g.request();
        assert!(!g.ready(ms(100)), "not inside the gap");
        assert_eq!(g.due(), Some(ms(250)));
        assert!(g.ready(ms(250)));
        g.sent(Some(20), ms(250)); // one keyframe answers the whole burst
        assert_eq!(g.due(), None);
        // Hints naming frames before that keyframe are stale (frame ids wrap).
        g.lost_after(15);
        assert_eq!(g.due(), None);
        let mut w = KeyframeGate::default();
        w.sent(Some(2), t0);
        w.lost_after(u32::MAX - 1);
        assert_eq!(w.due(), None, "older across the wrap");
        w.lost_after(3);
        assert_eq!(w.due(), Some(ms(250)));
    }

    #[test]
    fn mac_pixels_and_points() {
        // A Retina display: 2940x1912 pixels in 1470x956 points, the main one at 0,0.
        let main = (0.0, 0.0, 1470.0, 956.0);
        assert_eq!(mac_point(0.0, 0.0, main, (2940, 1912)), (0.0, 0.0));
        assert_eq!(mac_point(1470.0, 956.0, main, (2940, 1912)), (735.0, 478.0));
        assert_eq!(mac_point(5000.0, -1.0, main, (2940, 1912)), (1469.5, 0.0), "clamped to the display");
        // A 1x display left of it.
        let left = (-1920.0, 0.0, 1920.0, 1080.0);
        assert_eq!(mac_point(10.0, 20.0, left, (1920, 1080)), (-1910.0, 20.0));
        assert_eq!(mac_pixel(-1910.0, 20.0, left, (1920, 1080)), (10, 20, true));
        assert_eq!(mac_pixel(735.25, 478.0, main, (2940, 1912)), (1470, 956, true));
        assert!(!mac_pixel(10.0, 10.0, left, (1920, 1080)).2, "on another display");
    }

    #[test]
    fn pointer_lands_on_the_exact_pixel() {
        // Two displays side by side, the second left of the primary: x from -1920.
        let (left, top, w, h) = (-1920, 0, 3840, 1080);
        for x in [-1920, -1, 0, 1, 1919] {
            let (dx, _) = virtual_desk_abs(x, 0, left, top, w, h);
            assert!((0..=65535).contains(&dx));
            assert_eq!(left + (dx as i64 * w as i64 / 65536) as i32, x, "x {x}");
        }
        for y in 0..h {
            let (_, dy) = virtual_desk_abs(0, y, left, top, w, h);
            assert_eq!((dy as i64 * h as i64 / 65536) as i32, y);
        }
        // Stream pixels on a stream scaled to half the display.
        assert_eq!(stream_to_display(960.0, 540.0, (1920, 1080), (3840, 2160)), (1920.0, 1080.0));
        assert_eq!(stream_to_display(5000.0, -3.0, (1920, 1080), (1920, 1080)), (1919.0, 0.0));
    }
}
