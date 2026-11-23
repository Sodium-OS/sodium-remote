// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The Win32 clipboard: text, HTML and PNG both ways.
//!
//! One thread owns a message-only window. It hears `WM_CLIPBOARDUPDATE`, reads the
//! clipboard into a `ClipEntry` and hands it to the session. Reads and writes from
//! the session are queued to that thread too: after `EmptyClipboard`, only a
//! clipboard opened by a real window can take `SetClipboardData`. A change we made
//! ourselves is recognised by its sequence number and not reported back, so two
//! linked clipboards never ping-pong. Images travel as PNG; on Windows they are
//! converted from and to the DIB formats that Paint and Office use.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use remote_core::msg::ClipFormat;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_UNICODETEXT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG, PostMessageW,
    RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLIPBOARDUPDATE,
    WNDCLASSW,
};
use windows::core::{PCWSTR, w};

use crate::backend::{ClipEntry, ClipSink, Clipboard};

/// Anything bigger than this is not sent or taken.
const MAX_BYTES: usize = remote_core::msg::CLIPBOARD_MAX;
const WM_SET: u32 = WM_APP;
const WM_GET: u32 = WM_APP + 1;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const LCS_SRGB: u32 = 0x7352_4742;
const LCS_GM_IMAGES: u32 = 4;

/// What the window procedure needs; there is one clipboard per process.
struct Shared {
    on_change: ClipSink,
    /// The sequence number right after our own `set`.
    ours: AtomicU32,
    sets: Mutex<VecDeque<ClipEntry>>,
    gets: Mutex<VecDeque<Sender<Option<ClipEntry>>>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

struct WinClipboard {
    /// The window, as an integer because `HWND` isn't `Send`.
    hwnd: isize,
}

impl WinClipboard {
    fn post(&self, msg: u32) -> bool {
        unsafe { PostMessageW(Some(HWND(self.hwnd as *mut c_void)), msg, WPARAM(0), LPARAM(0)).is_ok() }
    }
}

impl Clipboard for WinClipboard {
    fn get(&self) -> Option<ClipEntry> {
        let shared = SHARED.get()?;
        let (tx, rx) = channel();
        shared.gets.lock().ok()?.push_back(tx);
        if !self.post(WM_GET) {
            return None;
        }
        rx.recv_timeout(Duration::from_secs(2)).ok().flatten()
    }

    fn set(&self, entry: ClipEntry) {
        let Some(shared) = SHARED.get() else { return };
        if let Ok(mut q) = shared.sets.lock() {
            q.push_back(entry);
        }
        if !self.post(WM_SET) {
            log("could not queue the clipboard write");
        }
    }
}

fn log(what: &str) {
    eprintln!("sodium-remote-host: clipboard: {what}");
}

/// Starts the clipboard thread and returns once its window exists.
pub fn start(on_change: ClipSink) -> Result<Arc<dyn Clipboard>> {
    let shared = Shared {
        on_change,
        ours: AtomicU32::new(0),
        sets: Mutex::new(VecDeque::new()),
        gets: Mutex::new(VecDeque::new()),
    };
    if SHARED.set(shared).is_err() {
        bail!("the clipboard is already started");
    }
    let (tx, rx) = channel::<std::result::Result<isize, String>>();
    std::thread::Builder::new().name("clipboard".into()).spawn(move || {
        let hwnd = match unsafe { create_window() } {
            Ok(h) => h,
            Err(e) => {
                let _ = tx.send(Err(format!("{e:#}")));
                return;
            }
        };
        let _ = tx.send(Ok(hwnd.0 as isize));
        unsafe {
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    })?;
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(hwnd)) => Ok(Arc::new(WinClipboard { hwnd })),
        Ok(Err(e)) => Err(anyhow!("clipboard window: {e}")),
        Err(_) => Err(anyhow!("the clipboard thread did not start")),
    }
}

unsafe fn create_window() -> Result<HWND> {
    unsafe {
        let hinst = HINSTANCE(GetModuleHandleW(None)?.0);
        let class = w!("SodiumRemoteClipboard");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            bail!("RegisterClassW failed");
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(hinst),
            None,
        )?;
        AddClipboardFormatListener(hwnd)?;
        Ok(hwnd)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE | WM_SET | WM_GET => {
            // A panic must not unwind into Windows.
            let r = catch_unwind(AssertUnwindSafe(|| handle(hwnd, msg)));
            if r.is_err() {
                log("internal error");
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn handle(hwnd: HWND, msg: u32) {
    let Some(shared) = SHARED.get() else { return };
    match msg {
        WM_CLIPBOARDUPDATE => {
            if unsafe { GetClipboardSequenceNumber() } == shared.ours.load(Ordering::SeqCst) {
                return;
            }
            if let Some(entry) = read_clipboard(hwnd) {
                if !entry.items.is_empty() {
                    (shared.on_change)(entry);
                }
            }
        }
        WM_SET => loop {
            let next = shared.sets.lock().ok().and_then(|mut q| q.pop_front());
            let Some(entry) = next else { break };
            if let Err(e) = write_clipboard(hwnd, &entry) {
                log(&format!("write failed: {e:#}"));
            }
            shared.ours.store(unsafe { GetClipboardSequenceNumber() }, Ordering::SeqCst);
        },
        _ => loop {
            let next = shared.gets.lock().ok().and_then(|mut q| q.pop_front());
            let Some(tx) = next else { break };
            let _ = tx.send(read_clipboard(hwnd).filter(|e| !e.items.is_empty()));
        },
    }
}

/// Other apps hold the clipboard briefly, so opening is retried.
fn open(hwnd: HWND) -> bool {
    for _ in 0..10 {
        if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn format(name: PCWSTR) -> u32 {
    unsafe { RegisterClipboardFormatW(name) }
}

fn available(fmt: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(fmt) }.is_ok()
}

// ---- reading ----

/// The clipboard's contents; the clipboard is closed again before this returns.
fn read_clipboard(hwnd: HWND) -> Option<ClipEntry> {
    if !open(hwnd) {
        log("could not open the clipboard to read it");
        return None;
    }
    let entry = read_open();
    let _ = unsafe { CloseClipboard() };
    Some(entry)
}

fn read_open() -> ClipEntry {
    let mut entry = ClipEntry::default();
    entry.concealed = available(format(w!("ExcludeClipboardContentFromMonitorProcessing")));

    if let Some(b) = get_bytes(CF_UNICODETEXT.0 as u32) {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        let text = String::from_utf16_lossy(&units);
        if !text.is_empty() {
            entry.items.push((ClipFormat::Text, text.into_bytes()));
        }
    }
    if let Some(b) = get_bytes(format(w!("HTML Format"))) {
        let html = cf_html_fragment(&b);
        if !html.is_empty() {
            entry.items.push((ClipFormat::Html, html));
        }
    }
    let png = get_bytes(format(w!("PNG")))
        .filter(|b| b.starts_with(&[0x89, b'P', b'N', b'G']))
        .or_else(|| {
            get_bytes(CF_DIBV5.0 as u32)
                .or_else(|| get_bytes(CF_DIB.0 as u32))
                .and_then(|d| dib_to_png(&d))
        });
    if let Some(png) = png {
        entry.items.push((ClipFormat::Png, png));
    }
    entry
}

/// A copy of one clipboard format's bytes (the clipboard must be open).
fn get_bytes(fmt: u32) -> Option<Vec<u8>> {
    if !available(fmt) {
        return None;
    }
    unsafe {
        let h: HANDLE = GetClipboardData(fmt).ok()?;
        if h.is_invalid() {
            return None;
        }
        let g = HGLOBAL(h.0);
        let size = GlobalSize(g);
        if size == 0 || size > MAX_BYTES {
            return None;
        }
        let p = GlobalLock(g) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, size).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

/// The number after `key:` in a CF_HTML header.
fn header_num(data: &[u8], key: &str) -> Option<usize> {
    let head = &data[..data.len().min(1024)];
    let at = head.windows(key.len()).position(|w| w == key.as_bytes())? + key.len();
    let digits: Vec<u8> = head[at..].iter().copied().take_while(|b| b.is_ascii_digit()).collect();
    std::str::from_utf8(&digits).ok()?.parse().ok()
}

/// The fragment of a CF_HTML block, else its whole HTML.
fn cf_html_fragment(data: &[u8]) -> Vec<u8> {
    let range = |s: &str, e: &str| {
        let (s, e) = (header_num(data, s)?, header_num(data, e)?);
        (s <= e && e <= data.len()).then(|| data[s..e].to_vec())
    };
    range("StartFragment:", "EndFragment:")
        .or_else(|| range("StartHTML:", "EndHTML:"))
        .unwrap_or_default()
}

/// A DIB (header, optional masks, pixels) as an RGBA8 PNG. 24- and 32-bit only.
fn dib_to_png(d: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let hdr = u32_at(0)? as usize;
    let width = u32_at(4)? as i32;
    let height = u32_at(8)? as i32;
    let bpp = u16::from_le_bytes([*d.get(14)?, *d.get(15)?]);
    let comp = u32_at(16)?;
    let clr_used = u32_at(32)? as usize;
    if hdr < 40 || width <= 0 || height == 0 || !matches!(bpp, 24 | 32) || !matches!(comp, BI_RGB | BI_BITFIELDS) {
        return None;
    }
    if comp == BI_BITFIELDS && bpp != 32 {
        return None;
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    if w.checked_mul(h)? * 4 > MAX_BYTES {
        return None;
    }
    // The masks: in the header from V4 on, else three DWORDs after it.
    let mut off = hdr;
    let (mr, mg, mb, ma) = if comp == BI_BITFIELDS {
        if hdr >= 108 {
            (u32_at(40)?, u32_at(44)?, u32_at(48)?, u32_at(52)?)
        } else {
            off += 12;
            (u32_at(hdr)?, u32_at(hdr + 4)?, u32_at(hdr + 8)?, 0)
        }
    } else {
        (0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0)
    };
    off += clr_used * 4;
    let stride = (w * bpp as usize).div_ceil(32) * 4;
    let pixels = d.get(off..off.checked_add(stride.checked_mul(h)?)?)?;

    // One channel of a pixel, scaled to 8 bits.
    let chan = |px: u32, mask: u32| -> u8 {
        if mask == 0 {
            return 0;
        }
        let shift = mask.trailing_zeros();
        let max = mask >> shift;
        let v = (px & mask) >> shift;
        if max == 255 { v as u8 } else { (v as u64 * 255 / max as u64) as u8 }
    };
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        // Positive height is bottom-up: the first row in the data is the last on screen.
        let src_y = if height > 0 { h - 1 - y } else { y };
        let row = &pixels[src_y * stride..][..stride];
        for x in 0..w {
            let o = (y * w + x) * 4;
            if bpp == 24 {
                rgba[o..o + 4].copy_from_slice(&[row[x * 3 + 2], row[x * 3 + 1], row[x * 3], 255]);
            } else {
                let b = &row[x * 4..x * 4 + 4];
                if comp == BI_BITFIELDS {
                    let px = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                    let a = if ma == 0 { 255 } else { chan(px, ma) };
                    rgba[o..o + 4].copy_from_slice(&[chan(px, mr), chan(px, mg), chan(px, mb), a]);
                } else {
                    rgba[o..o + 4].copy_from_slice(&[b[2], b[1], b[0], b[3]]);
                }
            }
        }
    }
    // Plain 32-bit BI_RGB often leaves the fourth byte zero: then it is padding, not alpha.
    if bpp == 32 && comp == BI_RGB && rgba.chunks_exact(4).all(|p| p[3] == 0) {
        rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    }

    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().ok()?;
    writer.write_image_data(&rgba).ok()?;
    writer.finish().ok()?;
    Some(out)
}

// ---- writing ----

fn write_clipboard(hwnd: HWND, entry: &ClipEntry) -> Result<()> {
    if !open(hwnd) {
        bail!("could not open the clipboard");
    }
    let r = write_open(entry);
    let _ = unsafe { CloseClipboard() };
    r
}

fn write_open(entry: &ClipEntry) -> Result<()> {
    unsafe { EmptyClipboard()? };
    for (fmt, data) in &entry.items {
        if data.len() > MAX_BYTES {
            continue;
        }
        let r = match fmt {
            ClipFormat::Text => set_text(data),
            ClipFormat::Html => set_data(format(w!("HTML Format")), &cf_html(data)),
            ClipFormat::Png => set_png(data),
            _ => Ok(()),
        };
        if let Err(e) = r {
            log(&format!("could not set {fmt:?}: {e:#}"));
        }
    }
    if entry.concealed {
        let _ = set_data(format(w!("ExcludeClipboardContentFromMonitorProcessing")), &[0; 4]);
        let _ = set_data(format(w!("CanIncludeInClipboardHistory")), &0u32.to_le_bytes());
        let _ = set_data(format(w!("CanUploadToCloudClipboard")), &0u32.to_le_bytes());
    }
    Ok(())
}

/// Puts a copy of `bytes` on the open clipboard as `fmt`.
fn set_data(fmt: u32, bytes: &[u8]) -> Result<()> {
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let p = GlobalLock(g) as *mut u8;
        if p.is_null() {
            let _ = GlobalFree(Some(g));
            bail!("GlobalLock failed");
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(g);
        // On success the clipboard owns the memory; on failure it is ours to free.
        if let Err(e) = SetClipboardData(fmt, Some(HANDLE(g.0))) {
            let _ = GlobalFree(Some(g));
            return Err(e.into());
        }
    }
    Ok(())
}

fn set_text(utf8: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(utf8).replace("\r\n", "\n").replace('\n', "\r\n");
    let mut bytes: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    bytes.extend_from_slice(&[0, 0]);
    set_data(CF_UNICODETEXT.0 as u32, &bytes)
}

/// A CF_HTML block around `fragment`, with its byte offsets.
fn cf_html(fragment: &[u8]) -> Vec<u8> {
    const PRE: &str = "<html><body><!--StartFragment-->";
    const POST: &str = "<!--EndFragment--></body></html>";
    let header = |a: usize, b: usize, c: usize, d: usize| {
        format!(
            "Version:0.9\r\nStartHTML:{a:010}\r\nEndHTML:{b:010}\r\nStartFragment:{c:010}\r\nEndFragment:{d:010}\r\n"
        )
    };
    let hlen = header(0, 0, 0, 0).len();
    let start_frag = hlen + PRE.len();
    let end_frag = start_frag + fragment.len();
    let end_html = end_frag + POST.len();
    let mut out = header(hlen, end_html, start_frag, end_frag).into_bytes();
    out.extend_from_slice(PRE.as_bytes());
    out.extend_from_slice(fragment);
    out.extend_from_slice(POST.as_bytes());
    out.push(0);
    out
}

fn set_png(png: &[u8]) -> Result<()> {
    set_data(format(w!("PNG")), png)?;
    // Windows makes CF_DIB and CF_BITMAP from this, so Paint and Office can paste.
    match png_to_dib(png) {
        Some(dib) => set_data(CF_DIBV5.0 as u32, &dib),
        None => Ok(()),
    }
}

/// A PNG as a bottom-up 32-bit BGRA CF_DIBV5 block.
fn png_to_dib(data: &[u8]) -> Option<Vec<u8>> {
    let mut dec = png::Decoder::new(Cursor::new(data));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    if w == 0 || h == 0 || w.checked_mul(h)? * 4 > MAX_BYTES {
        return None;
    }
    let src = &buf[..info.buffer_size()];
    let rgba = |i: usize| -> [u8; 4] {
        match info.color_type {
            png::ColorType::Rgba => [src[i * 4], src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3]],
            png::ColorType::Rgb => [src[i * 3], src[i * 3 + 1], src[i * 3 + 2], 255],
            png::ColorType::GrayscaleAlpha => [src[i * 2], src[i * 2], src[i * 2], src[i * 2 + 1]],
            _ => [src[i], src[i], src[i], 255],
        }
    };

    let mut out = Vec::with_capacity(124 + w * h * 4);
    let mut put = |v: u32| out.extend_from_slice(&v.to_le_bytes());
    put(124); // bV5Size
    put(w as u32);
    put(h as u32); // positive: bottom-up
    put(1 | (32 << 16)); // planes, bit count
    put(BI_BITFIELDS);
    put((w * h * 4) as u32); // bV5SizeImage
    put(0); // x pixels per metre
    put(0); // y pixels per metre
    put(0); // colours used
    put(0); // colours important
    put(0x00FF_0000); // red mask
    put(0x0000_FF00); // green mask
    put(0x0000_00FF); // blue mask
    put(0xFF00_0000); // alpha mask
    put(LCS_SRGB);
    for _ in 0..9 {
        put(0); // endpoints
    }
    put(0); // gamma red
    put(0); // gamma green
    put(0); // gamma blue
    put(LCS_GM_IMAGES);
    put(0); // profile data
    put(0); // profile size
    put(0); // reserved
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, a] = rgba(y * w + x);
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    Some(out)
}
