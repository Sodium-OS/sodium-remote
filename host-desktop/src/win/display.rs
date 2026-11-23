// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The displays: DXGI's outputs attached to the desktop, with Windows' own numbering
//! (`\\.\DISPLAYn` is display `n`, as Settings › Display numbers them), the monitor's
//! friendly name from the display configuration ("DELL U2720Q"; empty when Windows has
//! none, and the client then says "Display %1"), its effective DPI as the scale, its
//! refresh rate, and where it sits on the virtual desktop.

use std::collections::HashMap;

use remote_core::msg;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput};
use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW, MONITORINFO};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::PCWSTR;

use crate::backend::{Display, Rect};

/// One output, with what capture and input need.
#[derive(Clone)]
pub struct Output {
    pub display: Display,
    /// Its rectangle on the virtual desktop (physical pixels).
    pub rect: Rect,
    pub adapter: IDXGIAdapter1,
    pub output: IDXGIOutput,
}

fn wide(s: &[u16]) -> String {
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n])
}

/// `\\.\DISPLAY3` → 3.
fn number(gdi: &str) -> Option<u32> {
    gdi.trim_start_matches(r"\\.\DISPLAY").parse().ok()
}

/// GDI device names → monitor friendly names, from the active display paths.
fn friendly_names() -> HashMap<String, String> {
    let mut out = HashMap::new();
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None) != ERROR_SUCCESS {
            return out;
        }
        for p in &paths[..np as usize] {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            src.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            src.header.adapterId = p.sourceInfo.adapterId;
            src.header.id = p.sourceInfo.id;
            let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            tgt.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            tgt.header.adapterId = p.targetInfo.adapterId;
            tgt.header.id = p.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut src.header) == 0 && DisplayConfigGetDeviceInfo(&mut tgt.header) == 0 {
                let name = wide(&tgt.monitorFriendlyDeviceName);
                if !name.is_empty() {
                    out.insert(wide(&src.viewGdiDeviceName), name);
                }
            }
        }
    }
    out
}

/// Every output attached to the desktop, by display number.
pub fn outputs() -> Vec<Output> {
    let mut v = vec![];
    let names = friendly_names();
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return v };
    for a in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(a) }) else { break };
        for o in 0.. {
            let Ok(output) = (unsafe { adapter.EnumOutputs(o) }) else { break };
            let Ok(desc) = (unsafe { output.GetDesc() }) else { continue };
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }
            let gdi = wide(&desc.DeviceName);
            let r = desc.DesktopCoordinates;
            let rect = Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top };
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let primary = unsafe { GetMonitorInfoW(desc.Monitor, &mut mi) }.as_bool() && mi.dwFlags & MONITORINFOF_PRIMARY != 0;
            let (mut dx, mut dy) = (96u32, 96u32);
            let scale = if unsafe { GetDpiForMonitor(desc.Monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.is_ok() { dx as f32 / 96.0 } else { 1.0 };
            let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
            let gdi_w: Vec<u16> = gdi.encode_utf16().chain([0]).collect();
            let hz = if unsafe { EnumDisplaySettingsW(PCWSTR(gdi_w.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm) }.as_bool() {
                dm.dmDisplayFrequency
            } else {
                60
            };
            // 0 and 1 mean "the hardware's default".
            let hz = if hz <= 1 { 60 } else { hz };
            let id = number(&gdi).unwrap_or(100 + v.len() as u32);
            v.push(Output {
                display: Display {
                    info: msg::DisplayInfo {
                        id,
                        name: names.get(&gdi).cloned().unwrap_or_default(),
                        width: rect.w as u32,
                        height: rect.h as u32,
                        scale,
                        primary,
                    },
                    refresh_mhz: hz * 1000,
                },
                rect,
                adapter: adapter.clone(),
                output,
            });
        }
    }
    v.sort_by_key(|o| o.display.info.id);
    v
}

pub fn list() -> Vec<Display> {
    outputs().into_iter().map(|o| o.display).collect()
}
