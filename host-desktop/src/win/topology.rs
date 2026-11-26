// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The display configuration for the matched display (`vdisplay::Topology`), through
//! `QueryDisplayConfig` and `SetDisplayConfig`. Nothing is ever saved to Windows' display
//! database (no `SDC_SAVE_TO_DATABASE`): if the host dies, Windows goes back to the
//! database's own configuration by itself.
//!
//! What worked when the driver was first tried in a Windows 11 VM: the virtual
//! target's active path alone, its source moved to 0,0, the mode array cut down to that
//! path's two entries, `SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES`.
//! Without the move and the compaction Windows answers 87 (invalid parameter).

use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ALL_PATHS, QDC_ONLY_ACTIVE_PATHS, QUERY_DISPLAY_CONFIG_FLAGS,
    QueryDisplayConfig, SDC_ALLOW_CHANGES, SDC_APPLY, SDC_TOPOLOGY_EXTEND, SDC_USE_DATABASE_CURRENT, SDC_USE_SUPPLIED_DISPLAY_CONFIG,
    SetDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, HWND, LPARAM, LUID, POINT, POINTL, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    DISPLAYCONFIG_PATH_ACTIVE, DISPLAYCONFIG_PATH_MODE_IDX_INVALID, GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY,
    MONITORINFO, MonitorFromPoint, MonitorFromRect,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, IsIconic, IsWindowVisible, IsZoomed, SW_SHOWMAXIMIZED,
    SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPlacement, SetWindowPos, WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};
use windows::core::BOOL;

use super::display::{number, wide};
use crate::vdisplay::{Monitor, Topology};

/// The monitor name SudoVDA puts in the virtual monitor's EDID (`sudovda::DEVICE_NAME`).
pub const MONITOR_NAME: &str = "Sodium Remote";

/// The saved topology's header: "SRTP", then the version.
const MAGIC: &[u8; 5] = b"SRTP1";

pub struct WinTopology {
    /// Targets Windows didn't make active by itself, already extended to once.
    kicked: Mutex<Vec<(u64, u32)>>,
}

impl WinTopology {
    pub fn new() -> WinTopology {
        WinTopology { kicked: Mutex::new(vec![]) }
    }
}

impl Default for WinTopology {
    fn default() -> Self {
        Self::new()
    }
}

pub fn luid(l: LUID) -> u64 {
    ((l.HighPart as u32 as u64) << 32) | l.LowPart as u64
}

fn query(flags: QUERY_DISPLAY_CONFIG_FLAGS) -> Result<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>)> {
    for _ in 0..4 {
        let (mut np, mut nm) = (0u32, 0u32);
        let r = unsafe { GetDisplayConfigBufferSizes(flags, &mut np, &mut nm) };
        if r != ERROR_SUCCESS {
            bail!("GetDisplayConfigBufferSizes: {}", r.0);
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        let r = unsafe { QueryDisplayConfig(flags, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None) };
        if r == ERROR_INSUFFICIENT_BUFFER {
            continue; // the configuration changed in between
        }
        if r != ERROR_SUCCESS {
            bail!("QueryDisplayConfig: {}", r.0);
        }
        paths.truncate(np as usize);
        modes.truncate(nm as usize);
        return Ok((paths, modes));
    }
    bail!("QueryDisplayConfig: the configuration keeps changing")
}

fn source_gdi(p: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
    src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
    src.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
    src.header.adapterId = p.sourceInfo.adapterId;
    src.header.id = p.sourceInfo.id;
    (unsafe { DisplayConfigGetDeviceInfo(&mut src.header) } == 0).then(|| wide(&src.viewGdiDeviceName))
}

fn target_name(p: &DISPLAYCONFIG_PATH_INFO) -> String {
    let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
    tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
    tgt.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
    tgt.header.adapterId = p.targetInfo.adapterId;
    tgt.header.id = p.targetInfo.id;
    if unsafe { DisplayConfigGetDeviceInfo(&mut tgt.header) } == 0 { wide(&tgt.monitorFriendlyDeviceName) } else { String::new() }
}

/// The monitor's path: its target id on the adapter SudoVDA reported, else (should the
/// LUIDs differ) its target id on a monitor named ours.
fn find<'a>(paths: &'a [DISPLAYCONFIG_PATH_INFO], m: &Monitor) -> Option<&'a DISPLAYCONFIG_PATH_INFO> {
    paths.iter().find(|p| p.targetInfo.id == m.target && luid(p.targetInfo.adapterId) == m.adapter).or_else(|| {
        paths.iter().find(|p| p.targetInfo.id == m.target && target_name(p) == MONITOR_NAME)
    })
}

/// The GDI name (`\\.\DISPLAYn`) of `m` while it is active.
pub fn gdi_of(m: &Monitor) -> Option<String> {
    let (paths, _) = query(QDC_ONLY_ACTIVE_PATHS).ok()?;
    find(&paths, m).and_then(source_gdi)
}

fn as_bytes<T>(v: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn from_bytes<T: Default + Clone>(b: &[u8], n: usize) -> Option<Vec<T>> {
    let size = size_of::<T>();
    if b.len() != n * size {
        return None;
    }
    let mut v = vec![T::default(); n];
    unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), v.as_mut_ptr() as *mut u8, b.len()) };
    Some(v)
}

fn encode(paths: &[DISPLAYCONFIG_PATH_INFO], modes: &[DISPLAYCONFIG_MODE_INFO]) -> Vec<u8> {
    let mut b = MAGIC.to_vec();
    b.extend_from_slice(&(paths.len() as u32).to_le_bytes());
    b.extend_from_slice(&(modes.len() as u32).to_le_bytes());
    b.extend_from_slice(as_bytes(paths));
    b.extend_from_slice(as_bytes(modes));
    b
}

fn decode(b: &[u8]) -> Option<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>)> {
    let rest = b.strip_prefix(&MAGIC[..])?;
    let np = u32::from_le_bytes(rest.get(0..4)?.try_into().ok()?) as usize;
    let nm = u32::from_le_bytes(rest.get(4..8)?.try_into().ok()?) as usize;
    let split = 8 + np.checked_mul(size_of::<DISPLAYCONFIG_PATH_INFO>())?;
    let paths = from_bytes(rest.get(8..split)?, np)?;
    let modes = from_bytes(rest.get(split..)?, nm)?;
    (np > 0).then_some((paths, modes))
}

/// The active paths as (source adapter, source, target adapter, target), sorted.
fn shape(paths: &[DISPLAYCONFIG_PATH_INFO]) -> Vec<(u64, u32, u64, u32)> {
    let mut v: Vec<_> = paths.iter().map(|p| (luid(p.sourceInfo.adapterId), p.sourceInfo.id, luid(p.targetInfo.adapterId), p.targetInfo.id)).collect();
    v.sort_unstable();
    v
}

impl Topology for WinTopology {
    fn display_of(&self, m: &Monitor) -> Option<u32> {
        let (paths, _) = query(QDC_ONLY_ACTIVE_PATHS).ok()?;
        if let Some(p) = find(&paths, m) {
            return source_gdi(p).and_then(|g| number(&g));
        }
        // Windows extends the desktop to a new monitor by itself; when it hasn't (a
        // computer set to show one screen only), extend once, without saving.
        let (all, _) = query(QDC_ALL_PATHS).ok()?;
        if find(&all, m).is_some() {
            let mut k = self.kicked.lock().unwrap();
            if !k.contains(&(m.adapter, m.target)) {
                k.push((m.adapter, m.target));
                let r = unsafe { SetDisplayConfig(None, None, SDC_APPLY | SDC_TOPOLOGY_EXTEND) };
                eprintln!("sodium-remote-host: matched display: the virtual display wasn't active; extended the desktop to it ({r})");
            }
        }
        None
    }

    fn save(&self) -> Result<Vec<u8>> {
        let (paths, modes) = query(QDC_ONLY_ACTIVE_PATHS)?;
        Ok(encode(&paths, &modes))
    }

    fn only(&self, m: &Monitor) -> Result<()> {
        let (paths, modes) = query(QDC_ONLY_ACTIVE_PATHS)?;
        let mut p = *find(&paths, m).context("the virtual display isn't active")?;
        let mut keep: Vec<DISPLAYCONFIG_MODE_INFO> = vec![];
        unsafe {
            for idx in [&mut p.sourceInfo.Anonymous.modeInfoIdx, &mut p.targetInfo.Anonymous.modeInfoIdx] {
                if *idx == DISPLAYCONFIG_PATH_MODE_IDX_INVALID || *idx as usize >= modes.len() {
                    *idx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
                    continue;
                }
                let mut mode = modes[*idx as usize];
                if mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE {
                    mode.Anonymous.sourceMode.position = POINTL { x: 0, y: 0 };
                }
                *idx = keep.len() as u32;
                keep.push(mode);
            }
        }
        p.flags |= DISPLAYCONFIG_PATH_ACTIVE;
        let r = unsafe { SetDisplayConfig(Some(&[p]), Some(&keep), SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES) };
        if r != 0 {
            bail!("SetDisplayConfig: {r}");
        }
        Ok(())
    }

    fn restore(&self, saved: &[u8]) -> Result<()> {
        if let Some((paths, modes)) = decode(saved) {
            // Already so (Windows went back by itself after the monitor left): nothing to do.
            if query(QDC_ONLY_ACTIVE_PATHS).is_ok_and(|(now, _)| shape(&now) == shape(&paths)) {
                return Ok(());
            }
            let r = unsafe { SetDisplayConfig(Some(&paths), Some(&modes), SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES) };
            if r == 0 {
                return Ok(());
            }
            // Adapter LUIDs change at every restart, so a journal from before one can't apply.
            eprintln!("sodium-remote-host: matched display: the saved topology didn't apply ({r}); using Windows' own");
        }
        let r = unsafe { SetDisplayConfig(None, None, SDC_APPLY | SDC_USE_DATABASE_CURRENT) };
        if r != 0 {
            bail!("SetDisplayConfig (database): {r}");
        }
        Ok(())
    }

    fn rescue_windows(&self) {
        let mut hwnds: Vec<HWND> = vec![];
        unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
            unsafe { (*(l.0 as *mut Vec<HWND>)).push(h) };
            BOOL(1)
        }
        let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut hwnds as *mut _ as isize)) };
        let primary = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !unsafe { GetMonitorInfoW(primary, &mut mi) }.as_bool() {
            return;
        }
        let work = mi.rcWork;
        let (ww, wh) = (work.right - work.left, work.bottom - work.top);
        let mut moved = 0;
        for h in hwnds {
            unsafe {
                if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() || GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                    continue;
                }
                let mut cloaked = 0u32;
                if DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok() && cloaked != 0 {
                    continue;
                }
                let mut r = RECT::default();
                if GetWindowRect(h, &mut r).is_err() || r.right <= r.left || r.bottom <= r.top {
                    continue;
                }
                if !MonitorFromRect(&r, MONITOR_DEFAULTTONULL).is_invalid() {
                    continue;
                }
                let (w, hh) = ((r.right - r.left).min(ww), (r.bottom - r.top).min(wh));
                let (x, y) = (work.left + (ww - w) / 2, work.top + (wh - hh) / 2);
                if IsZoomed(h).as_bool() {
                    // Maximised: its restored place moves onto the primary, where it maximises again.
                    let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
                    if GetWindowPlacement(h, &mut wp).is_ok() {
                        wp.rcNormalPosition = RECT { left: x, top: y, right: x + w, bottom: y + hh };
                        wp.showCmd = SW_SHOWMAXIMIZED.0 as u32;
                        if SetWindowPlacement(h, &wp).is_ok() {
                            moved += 1;
                        }
                    }
                } else if SetWindowPos(h, None, x, y, w, hh, SWP_NOZORDER | SWP_NOACTIVATE).is_ok() {
                    moved += 1;
                }
            }
        }
        if moved > 0 {
            eprintln!("sodium-remote-host: matched display: moved {moved} window(s) left on no display to the primary");
        }
    }
}
