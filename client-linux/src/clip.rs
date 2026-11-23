// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The engine's `ClipboardBackend` on the Wayland clipboard (`remote-wlclip`:
//! ext-data-control-v1 on its own connection, so it works whether or not the
//! stream window has focus, like the Windows client's).
//!
//! A new selection made by another program bumps the sequence and is kept as
//! read; the engine's `ClipSync` offers it to the host. A write from the host
//! becomes our own selection, which `remote-wlclip` never reports back, so the
//! sequence stays put and nothing echoes.

use crate::msg::ClipFormat;
use remote_engine::clipboard::ClipboardBackend;
use remote_wlclip::{ClipEntry, Clipboard};
use std::sync::{Arc, Mutex};

const TEXT: &str = "text/plain;charset=utf-8";
const HTML: &str = "text/html";
const PNG: &str = "image/png";

#[derive(Default)]
struct Seen {
    seq: u32,
    items: Vec<(ClipFormat, Vec<u8>)>,
    concealed: bool,
}

pub struct WlClipboard {
    cb: Clipboard,
    seen: Arc<Mutex<Seen>>,
}

fn from_mime(m: &str) -> Option<ClipFormat> {
    match m {
        TEXT => Some(ClipFormat::Text),
        HTML => Some(ClipFormat::Html),
        PNG => Some(ClipFormat::Png),
        _ => None,
    }
}

fn to_mime(f: &ClipFormat) -> Option<&'static str> {
    match f {
        ClipFormat::Text => Some(TEXT),
        ClipFormat::Html => Some(HTML),
        ClipFormat::Png => Some(PNG),
        _ => None,
    }
}

impl WlClipboard {
    /// Connects to `$WAYLAND_DISPLAY`; fails when the compositor has no ext-data-control-v1.
    pub fn start() -> anyhow::Result<WlClipboard> {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let s2 = seen.clone();
        let cb = Clipboard::start(move |e: ClipEntry| {
            let mut s = s2.lock().unwrap();
            s.seq = s.seq.wrapping_add(1);
            s.concealed = e.concealed;
            s.items = e.items.into_iter().filter_map(|(m, d)| Some((from_mime(&m)?, d))).collect();
        })?;
        Ok(WlClipboard { cb, seen })
    }
}

impl ClipboardBackend for WlClipboard {
    fn sequence(&self) -> u32 {
        self.seen.lock().unwrap().seq
    }

    fn read(&self) -> Vec<(ClipFormat, Vec<u8>)> {
        self.seen.lock().unwrap().items.clone()
    }

    fn concealed(&self) -> bool {
        self.seen.lock().unwrap().concealed
    }

    fn write(&self, items: Vec<(ClipFormat, Vec<u8>)>, concealed: bool) -> u32 {
        let entry = ClipEntry { items: items.iter().filter_map(|(f, d)| Some((to_mime(f)?.to_string(), d.clone()))).collect(), concealed };
        let mut s = self.seen.lock().unwrap();
        // A concealed entry is held only by remote-wlclip, which wipes it when it loses the selection.
        s.items = if concealed { vec![] } else { items };
        s.concealed = concealed;
        if !entry.items.is_empty() {
            self.cb.set(entry);
        }
        s.seq
    }
}
