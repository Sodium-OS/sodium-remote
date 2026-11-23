// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! media-ipc: reads the plugin socket. Buffers, frames and hints go to the encoder
//! thread; the cursor, caret and pointer lock go straight to the sink, so they never
//! wait behind an encode.

use std::{
    collections::HashSet,
    fs::File,
    os::unix::fs::FileExt,
    sync::{Arc, atomic::Ordering, mpsc::Sender},
};

use anyhow::{Result, ensure};

use super::{CursorImage, CursorShape, EventSink, MediaEvent, Shared, encode::ToEnc};
use crate::capture::{
    Rect,
    plugin::{self, Reader},
};

/// One HINT (capture-ipc v3, 40 bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub kind: u32,
    pub detail: u32,
    pub rect: Rect,
    pub dx: i32,
    pub dy: i32,
    pub seq: u64,
}

pub const HINT_ANIM_START: u32 = 1;
pub const HINT_ANIM_SETTLE: u32 = 2;
pub const HINT_WINDOW_MOVE: u32 = 3;
pub const HINT_SCROLL: u32 = 4;

fn rect(rd: &mut Reader) -> Result<Rect> {
    Ok(Rect { x: rd.i32()?, y: rd.i32()?, w: rd.i32()?, h: rd.i32()? })
}

pub fn parse_hint(p: &[u8]) -> Result<Hint> {
    let mut rd = Reader::new(p);
    Ok(Hint { kind: rd.u32()?, detail: rd.u32()?, rect: rect(&mut rd)?, dx: rd.i32()?, dy: rd.i32()?, seq: rd.u64()? })
}

pub(super) fn run(shared: Arc<Shared>, tx: Sender<ToEnc>, sink: EventSink) {
    let why = match read_loop(&shared, &tx, &sink) {
        Ok(why) => why,
        Err(e) => format!("{e:#}"),
    };
    let _ = tx.send(ToEnc::Stop);
    if !shared.stopping.load(Ordering::SeqCst) {
        sink(MediaEvent::Stopped(why));
    }
}

fn read_loop(shared: &Shared, tx: &Sender<ToEnc>, sink: &EventSink) -> Result<String> {
    let mut shapes: HashSet<u64> = HashSet::new();
    let mut shape = 0u64;
    let mut animating = false;
    loop {
        let p = plugin::recv(&shared.sock)?;
        match p.kind {
            plugin::BUFFERS => {
                let (g, bufs) = plugin::parse_buffers(p)?;
                let _ = tx.send(ToEnc::Buffers(g, bufs));
            }
            plugin::FRAME => {
                let _ = tx.send(ToEnc::Frame(plugin::parse_frame(p)?));
            }
            plugin::HINT => {
                let h = parse_hint(&p.payload)?;
                // core's latency-critical flag wants this at once (Session::set_animation)
                let anim = match h.kind {
                    HINT_ANIM_START => Some(true),
                    HINT_ANIM_SETTLE => Some(false),
                    _ => None,
                };
                if let Some(a) = anim.filter(|&a| a != animating) {
                    animating = a;
                    sink(MediaEvent::Animation(a));
                }
                let _ = tx.send(ToEnc::Hint(h));
            }
            plugin::CURSOR_SHAPE => {
                let mut rd = Reader::new(&p.payload);
                let (hash, w, h, hx, hy) = (rd.u64()?, rd.u32()?, rd.u32()?, rd.i32()?, rd.i32()?);
                shape = hash;
                if shapes.contains(&hash) {
                    continue; // the plugin dedups too, but a START sends it again
                }
                ensure!(w <= 512 && h <= 512, "cursor {w}x{h} too big");
                let fd = p.fds.into_iter().next();
                let Some(fd) = fd else { continue };
                // The memfd's offset is shared with the plugin's copy: read at 0.
                let mut px = vec![0u8; (w * h * 4) as usize];
                File::from(fd).read_exact_at(&mut px, 0)?;
                // the plugin's ARGB8888, premultiplied, as it is: core's CursorImage
                shapes.insert(hash);
                let image = CursorImage { width: w, height: h, hot_x: hx, hot_y: hy, scale: 1.0, pixels: px };
                sink(MediaEvent::CursorShape(CursorShape { hash, image }));
            }
            plugin::CURSOR_POS => {
                let mut rd = Reader::new(&p.payload);
                let (x, y, visible, _pad, t_ns) = (rd.i32()?, rd.i32()?, rd.u32()?, rd.u32()?, rd.u64()?);
                shared.focus.lock().unwrap().pointer = (visible != 0).then_some((x, y));
                sink(MediaEvent::CursorPos { x, y, visible: visible != 0, shape, t_ns });
            }
            plugin::CARET => {
                let mut rd = Reader::new(&p.payload);
                let (visible, purpose, r, t_ns) = (rd.u32()?, rd.u32()?, rect(&mut rd)?, rd.u64()?);
                shared.focus.lock().unwrap().caret = (visible != 0).then_some(r);
                sink(MediaEvent::Caret { visible: visible != 0, purpose, rect: r, t_ns });
            }
            plugin::POINTER_LOCK => {
                let mut rd = Reader::new(&p.payload);
                let (state, _pad) = (rd.u32()?, rd.u32()?);
                let r = rect(&mut rd).unwrap_or_default(); // a v3 draft plugin sent 8 bytes
                sink(MediaEvent::PointerLock { state, rect: r });
            }
            plugin::STATS => {
                let mut rd = Reader::new(&p.payload);
                let (frames, nb, nc) = (rd.u64()?, rd.u64()?, rd.u64()?);
                shared.stats.lock().unwrap().plugin = (frames, nb, nc);
                sink(MediaEvent::PluginStats { frames, dropped_no_buffer: nb, dropped_no_client: nc });
            }
            plugin::STOPPED => {
                let reason = Reader::new(&p.payload).u32()?;
                return Ok(format!("the plugin stopped capturing: {} ({reason})", plugin::stop_reason(reason)));
            }
            plugin::HELLO_ACK => {}
            k => anyhow::bail!("unknown message {k} from the plugin"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_has_the_v3_layout() {
        let mut b = Vec::new();
        for v in [3u32, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for v in [10i32, 20, 300, 200, 8, -4] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&77u64.to_le_bytes());
        assert_eq!(b.len(), 40);
        let h = parse_hint(&b).unwrap();
        assert_eq!(h, Hint { kind: 3, detail: 0, rect: Rect { x: 10, y: 20, w: 300, h: 200 }, dx: 8, dy: -4, seq: 77 });
    }
}
