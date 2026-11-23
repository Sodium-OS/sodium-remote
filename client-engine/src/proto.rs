// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! What the client reads on top of remote-core's channels, apart from `msg`
//! (control messages, input events and the cursor image):
//!
//! - **Frame metadata** (`ReceivedFrame::meta`): host-media's format v1
//!   (`u8 version = 1 | u8 class | u16 n | n × (u16 x, y, w, h)`), followed by
//!   tag-length-value records (`u8 tag | u16 len | body`). Tag 1 (STAMPS)
//!   carries the host's per-stage times for the latency harness: rendered,
//!   captured, converted and encoded, `u64` µs each on the host's session
//!   clock (the clock of the frame's `send_us`).
//!
//! Big-endian, like the rest of the wire.

use crate::msg;
use remote_core::wire::{Reader, Writer};

/// The codec the decoder needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Hevc,
    Av1,
    H264,
}

/// The stream as the host describes it (`Welcome.format`, `Format`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamInfo {
    pub codec: Codec,
    pub chroma444: bool,
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
}

impl From<&msg::StreamFormat> for StreamInfo {
    fn from(f: &msg::StreamFormat) -> Self {
        let (codec, chroma444) = match f.codec {
            msg::Codec::Hevc420 => (Codec::Hevc, false),
            msg::Codec::Hevc444 => (Codec::Hevc, true),
            msg::Codec::Av1 => (Codec::Av1, false),
            msg::Codec::H264 => (Codec::H264, false),
        };
        StreamInfo { codec, chroma444, width: f.width, height: f.height, refresh_mhz: f.refresh_mhz }
    }
}

/// The host's per-stage timestamps for one frame, on the host's session clock
/// (µs, the same scale as the frame's `send_us`); 0 when unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostStamps {
    /// The compositor finished the frame (render done or presentation).
    pub rendered_us: u64,
    /// The host had the captured frame.
    pub captured_us: u64,
    /// Colour conversion done.
    pub converted_us: u64,
    /// The bitstream was ready.
    pub encoded_us: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// Frame classes in host-media's metadata.
pub mod class {
    pub const PASSIVE: u8 = 0;
    pub const INTERACTIVE: u8 = 1;
    pub const REFINE: u8 = 2;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameMeta {
    pub class: u8,
    pub damage: Vec<Rect>,
    pub stamps: Option<HostStamps>,
}

mod tag {
    pub const STAMPS: u8 = 1;
}

impl FrameMeta {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(64);
        let rects: Vec<Rect> = if self.damage.len() > 32 {
            // At most 32 rects, then their bounding box (host-media's rule).
            let x0 = self.damage.iter().map(|r| r.x).min().unwrap_or(0);
            let y0 = self.damage.iter().map(|r| r.y).min().unwrap_or(0);
            let x1 = self.damage.iter().map(|r| r.x as u32 + r.w as u32).max().unwrap_or(0);
            let y1 = self.damage.iter().map(|r| r.y as u32 + r.h as u32).max().unwrap_or(0);
            vec![Rect { x: x0, y: y0, w: (x1 - x0 as u32) as u16, h: (y1 - y0 as u32) as u16 }]
        } else {
            self.damage.clone()
        };
        w.u8(1).u8(self.class).u16(rects.len() as u16);
        for r in &rects {
            w.u16(r.x).u16(r.y).u16(r.w).u16(r.h);
        }
        if let Some(s) = &self.stamps {
            w.u8(tag::STAMPS).u16(32).u64(s.rendered_us).u64(s.captured_us).u64(s.converted_us).u64(s.encoded_us);
        }
        w.buf
    }

    /// Unknown versions give an empty meta; unknown records are skipped and a
    /// truncated one ends the parse.
    pub fn decode(b: &[u8]) -> FrameMeta {
        let mut m = FrameMeta::default();
        let mut r = Reader::new(b);
        let (Some(1), Some(class), Some(n)) = (r.u8(), r.u8(), r.u16()) else { return m };
        m.class = class;
        for _ in 0..n {
            let (Some(x), Some(y), Some(w), Some(h)) = (r.u16(), r.u16(), r.u16(), r.u16()) else { return m };
            m.damage.push(Rect { x, y, w, h });
        }
        while let (Some(t), Some(len)) = (r.u8(), r.u16()) {
            let Some(body) = r.take(len as usize) else { break };
            let mut br = Reader::new(body);
            match t {
                tag::STAMPS => {
                    if let (Some(a), Some(b), Some(c), Some(d)) = (br.u64(), br.u64(), br.u64(), br.u64()) {
                        m.stamps = Some(HostStamps { rendered_us: a, captured_us: b, converted_us: c, encoded_us: d });
                    }
                }
                _ => {}
            }
        }
        m
    }
}

/// evdev BTN_* ↔ msg's buttons (BTN_SIDE and BTN_EXTRA are a mouse's back and forward).
pub fn button_from_evdev(code: u32) -> msg::Button {
    match code {
        0x110 => msg::Button::Left,
        0x111 => msg::Button::Right,
        0x112 => msg::Button::Middle,
        0x113 => msg::Button::Back,
        0x114 => msg::Button::Forward,
        c => msg::Button::Other(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_round_trip() {
        let m = FrameMeta {
            class: class::INTERACTIVE,
            damage: vec![Rect { x: 0, y: 0, w: 10, h: 20 }],
            stamps: Some(HostStamps { rendered_us: 1, captured_us: 2, converted_us: 3, encoded_us: 4 }),
        };
        assert_eq!(FrameMeta::decode(&m.encode()), m);
        // host-media's plain v1 meta (no records) and an unknown record.
        let mut plain = vec![1, 0, 0, 1, 0, 1, 0, 2, 0, 3, 0, 4];
        assert_eq!(FrameMeta::decode(&plain).damage, vec![Rect { x: 1, y: 2, w: 3, h: 4 }]);
        plain.extend([99, 0, 1, 7]);
        assert_eq!(FrameMeta::decode(&plain).damage.len(), 1);
    }
}
