// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The displays: CoreGraphics' active displays (no permission needed), each with
//!
//! - its `CGDirectDisplayID` as the protocol's id (stable while it stays connected);
//! - its size in pixels: the current mode's backing pixels (`CGDisplayModeGetPixelWidth`,
//!   2940 × 1912 for a 13″ Air that "looks like" 1470 × 956), which is what macOS renders
//!   and what the stream carries at native size;
//! - its bounds in global points (`CGDisplayBounds`), where events are posted, and the
//!   scale between the two;
//! - the refresh rate (0 from CoreGraphics means a built-in panel: 60 Hz, or 120 on a
//!   ProMotion panel when the mode says so);
//! - NSScreen's `localizedName` ("Built-in Retina Display", "DELL U2723QE"), read on the
//!   main thread; "" when it can't be read, so clients say "Display %1".

use std::time::Duration;

use objc2_app_kit::NSScreen;
use objc2_core_graphics::{CGDirectDisplayID, CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGGetActiveDisplayList, CGMainDisplayID};
use objc2_foundation::{NSNumber, NSString};
use remote_core::msg;

use super::main_thread::on_main;
use crate::backend::Display;

/// One display, with what capture and input need.
#[derive(Clone, Debug, PartialEq)]
pub struct MacDisplay {
    pub display: Display,
    /// Global display coordinates, points: x, y, width, height.
    pub bounds: (f64, f64, f64, f64),
}

/// NSScreen's names by display ID.
fn names() -> Vec<(u32, String)> {
    on_main(Duration::from_millis(500), |mtm| {
        let mut v = vec![];
        for screen in NSScreen::screens(mtm).iter() {
            let desc = screen.deviceDescription();
            let key = NSString::from_str("NSScreenNumber");
            let Some(n) = desc.objectForKey(&key) else { continue };
            let Ok(n) = n.downcast::<NSNumber>() else { continue };
            v.push((n.unsignedIntValue(), screen.localizedName().to_string()));
        }
        v
    })
    .unwrap_or_default()
}

pub fn ids() -> Vec<CGDirectDisplayID> {
    let mut ids = [0 as CGDirectDisplayID; 16];
    let mut n = 0u32;
    if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) } != objc2_core_graphics::CGError::Success {
        return vec![];
    }
    ids[..n as usize].to_vec()
}

/// One display's geometry, without its name.
pub fn geometry(id: CGDirectDisplayID) -> Option<MacDisplay> {
    let b = CGDisplayBounds(id);
    let mode = CGDisplayCopyDisplayMode(id)?;
    let (pw, ph) = (CGDisplayMode::pixel_width(Some(&mode)) as u32, CGDisplayMode::pixel_height(Some(&mode)) as u32);
    if pw == 0 || ph == 0 || b.size.width <= 0.0 {
        return None;
    }
    let hz = CGDisplayMode::refresh_rate(Some(&mode));
    let hz = if hz < 1.0 { 60.0 } else { hz };
    Some(MacDisplay {
        display: Display {
            info: msg::DisplayInfo {
                id,
                name: String::new(),
                width: pw,
                height: ph,
                scale: (pw as f64 / b.size.width) as f32,
                primary: id == CGMainDisplayID(),
            },
            refresh_mhz: (hz * 1000.0).round() as u32,
        },
        bounds: (b.origin.x, b.origin.y, b.size.width, b.size.height),
    })
}

/// Every active display, the main one first, with names.
pub fn outputs() -> Vec<MacDisplay> {
    let names = names();
    let mut v: Vec<MacDisplay> = ids()
        .into_iter()
        .filter_map(|id| {
            let mut d = geometry(id)?;
            d.display.info.name = names.iter().find(|(n, _)| *n == id).map(|(_, s)| s.clone()).unwrap_or_default();
            Some(d)
        })
        .collect();
    v.sort_by_key(|d| !d.display.info.primary);
    v
}

pub fn list() -> Vec<Display> {
    outputs().into_iter().map(|d| d.display).collect()
}
