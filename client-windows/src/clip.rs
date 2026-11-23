// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The Windows clipboard for `remote_engine::clipboard::ClipSync`: text
//! (`CF_UNICODETEXT`, UTF-8 and LF on the wire) and images ("PNG" when an app
//! offers it, else `CF_DIB`, converted), both ways.

use remote_engine::clipboard::ClipboardBackend;
use remote_engine::msg::ClipFormat;
use std::ffi::c_void;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows::core::w;

pub struct WinClipboard {
    /// A message-only window that owns what we put on the clipboard (as isize: Send).
    owner: isize,
    png: u32,
}

impl WinClipboard {
    pub fn new(owner: HWND) -> WinClipboard {
        WinClipboard { owner: owner.0 as isize, png: unsafe { RegisterClipboardFormatW(w!("PNG")) } }
    }

    fn open(&self) -> bool {
        for _ in 0..20 {
            if unsafe { OpenClipboard(Some(HWND(self.owner as *mut c_void))) }.is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }
}

fn global_bytes(h: HANDLE) -> Option<Vec<u8>> {
    let g = HGLOBAL(h.0);
    unsafe {
        let size = GlobalSize(g);
        let p = GlobalLock(g);
        if p.is_null() || size == 0 {
            return None;
        }
        let v = std::slice::from_raw_parts(p as *const u8, size).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

fn set_global(format: u32, data: &[u8]) {
    unsafe {
        let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1)) else { return };
        let p = GlobalLock(g);
        if p.is_null() {
            return;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), p as *mut u8, data.len());
        let _ = GlobalUnlock(g);
        // The clipboard owns the memory on success.
        let _ = SetClipboardData(format, Some(HANDLE(g.0)));
    }
}

fn text_from_utf16(b: &[u8]) -> String {
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    String::from_utf16_lossy(&w).replace("\r\n", "\n")
}

/// An uncompressed 24 or 32 bit DIB (`BITMAPINFOHEADER` and pixels) as PNG.
fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |o: usize| dib.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let header = u32_at(0)? as usize;
    let (w, h) = (u32_at(4)? as i32, u32_at(8)? as i32);
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?) as usize;
    let compression = u32_at(16)?;
    let clr_used = u32_at(32)? as usize;
    if header < 40 || w <= 0 || h == 0 || !(bits == 24 || bits == 32) || !(compression == 0 || compression == 3) {
        return None;
    }
    let masks = if compression == 3 && header == 40 { 12 } else { 0 };
    let off = header + masks + clr_used * 4;
    let (w, top_down, h) = (w as usize, h < 0, h.unsigned_abs() as usize);
    let stride = (w * bits).div_ceil(32) * 4;
    let mut rgba = vec![255u8; w * h * 4];
    for y in 0..h {
        let row = dib.get(off + (if top_down { y } else { h - 1 - y }) * stride..)?;
        for x in 0..w {
            let px = row.get(x * bits / 8..x * bits / 8 + bits / 8)?;
            let o = (y * w + x) * 4;
            rgba[o] = px[2];
            rgba[o + 1] = px[1];
            rgba[o + 2] = px[0];
        }
    }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().ok()?.write_image_data(&rgba).ok()?;
    Some(out)
}

/// PNG as a 32 bit bottom-up DIB, for apps that don't read "PNG".
fn png_to_dib(data: &[u8]) -> Option<Vec<u8>> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let n = info.color_type.samples();
    let mut out = Vec::with_capacity(40 + w * h * 4);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let p = &buf[(y * w + x) * n..(y * w + x) * n + n];
            let (r, g, b, a) = match n {
                1 => (p[0], p[0], p[0], 255),
                2 => (p[0], p[0], p[0], p[1]),
                3 => (p[0], p[1], p[2], 255),
                _ => (p[0], p[1], p[2], p[3]),
            };
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    Some(out)
}

impl ClipboardBackend for WinClipboard {
    fn sequence(&self) -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    fn read(&self) -> Vec<(ClipFormat, Vec<u8>)> {
        let mut items = Vec::new();
        if !self.open() {
            return items;
        }
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok()
                && let Ok(h) = GetClipboardData(CF_UNICODETEXT.0 as u32)
                && let Some(b) = global_bytes(h)
            {
                items.push((ClipFormat::Text, text_from_utf16(&b).into_bytes()));
            }
            let png = if IsClipboardFormatAvailable(self.png).is_ok() {
                GetClipboardData(self.png).ok().and_then(global_bytes)
            } else if IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok() {
                GetClipboardData(CF_DIB.0 as u32).ok().and_then(global_bytes).and_then(|d| dib_to_png(&d))
            } else {
                None
            };
            if let Some(p) = png {
                items.push((ClipFormat::Png, p));
            }
            let _ = CloseClipboard();
        }
        items
    }

    fn write(&self, items: Vec<(ClipFormat, Vec<u8>)>, concealed: bool) -> u32 {
        if !self.open() {
            return self.sequence();
        }
        unsafe {
            let _ = EmptyClipboard();
            for (f, d) in &items {
                match f {
                    ClipFormat::Text => {
                        let t = String::from_utf8_lossy(d).replace("\r\n", "\n").replace('\n', "\r\n");
                        let mut w: Vec<u8> = t.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
                        w.extend_from_slice(&[0, 0]);
                        set_global(CF_UNICODETEXT.0 as u32, &w);
                    }
                    ClipFormat::Png => {
                        set_global(self.png, d);
                        if let Some(dib) = png_to_dib(d) {
                            set_global(CF_DIB.0 as u32, &dib);
                        }
                    }
                    _ => {}
                }
            }
            if concealed {
                // Clipboard history and cloud sync skip it.
                set_global(RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")), &1u32.to_le_bytes());
                set_global(RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")), &0u32.to_le_bytes());
                set_global(RegisterClipboardFormatW(w!("CanUploadToCloudClipboard")), &0u32.to_le_bytes());
            }
            let _ = CloseClipboard();
        }
        self.sequence()
    }
}
