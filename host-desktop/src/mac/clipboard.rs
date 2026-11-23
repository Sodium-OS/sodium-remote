// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The general pasteboard: text, HTML and PNG both ways, through the session's
//! offer/request/data flow (as the Windows backend's).
//!
//! macOS has no change notification for the pasteboard, so a thread polls its
//! `changeCount` every 250 ms and reads a new entry: `public.utf8-plain-text`,
//! `public.html` (as it is), and `public.png`, or `public.tiff` converted to PNG
//! (what screenshots and many apps put there). Our own writes are recognised by the
//! change count they leave and not reported back. An entry marked with
//! `org.nspasteboard.ConcealedType` (password managers, nspasteboard.org) goes out
//! concealed, and a concealed entry from the client is written with that marker.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF};
use objc2_foundation::{NSData, NSDictionary, NSString};
use remote_core::msg::ClipFormat;

use crate::backend::{ClipEntry, ClipSink, Clipboard};

/// Anything bigger than this is not sent or taken.
const MAX_BYTES: usize = remote_core::msg::CLIPBOARD_MAX;
const CONCEALED: &str = "org.nspasteboard.ConcealedType";

struct MacClipboard {
    /// The change count right after our own `set`; also serialises pasteboard use.
    ours: Mutex<isize>,
}

fn data(pb: &NSPasteboard, kind: &NSString) -> Option<Vec<u8>> {
    let d = pb.dataForType(kind)?;
    (d.len() <= MAX_BYTES).then(|| d.to_vec())
}

/// The pasteboard now, in protocol formats.
fn read(pb: &NSPasteboard) -> Option<ClipEntry> {
    let types = pb.types().map(|t| t.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap_or_default();
    let mut items = vec![];
    if let Some(s) = pb.stringForType(unsafe { NSPasteboardTypeString })
        && s.len() <= MAX_BYTES
    {
        items.push((ClipFormat::Text, s.to_string().into_bytes()));
    }
    if let Some(h) = data(pb, unsafe { NSPasteboardTypeHTML }) {
        items.push((ClipFormat::Html, h));
    }
    if let Some(p) = data(pb, unsafe { NSPasteboardTypePNG }) {
        items.push((ClipFormat::Png, p));
    } else if let Some(t) = pb.dataForType(unsafe { NSPasteboardTypeTIFF })
        && let Some(rep) = NSBitmapImageRep::imageRepWithData(&t)
        && let Some(png) = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
        && png.len() <= MAX_BYTES
    {
        items.push((ClipFormat::Png, png.to_vec()));
    }
    (!items.is_empty()).then(|| ClipEntry { items, concealed: types.iter().any(|t| t == CONCEALED) })
}

impl Clipboard for MacClipboard {
    fn get(&self) -> Option<ClipEntry> {
        let _g = self.ours.lock().unwrap();
        autoreleasepool(|_| read(&NSPasteboard::generalPasteboard()))
    }

    fn set(&self, entry: ClipEntry) {
        let mut ours = self.ours.lock().unwrap();
        autoreleasepool(|_| {
            let pb = NSPasteboard::generalPasteboard();
            pb.clearContents();
            for (format, bytes) in &entry.items {
                match format {
                    ClipFormat::Text => {
                        let s = NSString::from_str(&String::from_utf8_lossy(bytes));
                        pb.setString_forType(&s, unsafe { NSPasteboardTypeString });
                    }
                    ClipFormat::Html => {
                        pb.setData_forType(Some(&NSData::with_bytes(bytes)), unsafe { NSPasteboardTypeHTML });
                    }
                    ClipFormat::Png => {
                        pb.setData_forType(Some(&NSData::with_bytes(bytes)), unsafe { NSPasteboardTypePNG });
                    }
                    _ => {}
                }
            }
            if entry.concealed {
                pb.setData_forType(Some(&NSData::new()), &NSString::from_str(CONCEALED));
            }
            *ours = pb.changeCount();
        });
    }
}

pub fn start(on_change: ClipSink) -> Result<Arc<dyn Clipboard>> {
    let initial = autoreleasepool(|_| NSPasteboard::generalPasteboard().changeCount());
    let clip = Arc::new(MacClipboard { ours: Mutex::new(initial) });
    let c = clip.clone();
    std::thread::Builder::new().name("clipboard".into()).spawn(move || {
        let mut last = initial;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let entry = {
                let ours = c.ours.lock().unwrap();
                autoreleasepool(|_| {
                    let pb = NSPasteboard::generalPasteboard();
                    let n = pb.changeCount();
                    if n == last {
                        return None;
                    }
                    last = n;
                    if n == *ours {
                        return None; // our own write
                    }
                    read(&pb)
                })
            };
            if let Some(e) = entry {
                on_change(e);
            }
        }
    })?;
    Ok(clip)
}
