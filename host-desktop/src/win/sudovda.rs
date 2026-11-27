// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Sodium Remote Display, the virtual display driver (SudoMaker's SudoVDA, MIT, with our
//! patches; we build and sign it ourselves: `packaging/windows/sudovda/`), as
//! `vdisplay::VirtualDisplay`, through the control IOCTLs of its header
//! `Common/Include/sudovda-ioctl.h`. The structures here are our own, laid out as the
//! driver expects.
//!
//! - the device is found by its interface GUID, `{53f76209-42ea-44fd-897e-8607b15fa9be}`
//!   (ours: another program's SudoVDA has a different one and is never opened);
//! - ADD (width, height, refresh in Hz, the monitor's GUID, an EDID name and serial)
//!   returns the adapter's LUID and the target id; the monitor is there at once;
//! - REMOVE takes the GUID;
//! - UPDATE_MODE (protocol 0.3, our patch) gives a monitor a new preferred mode and target
//!   modes in place;
//! - any IOCTL but GET_WATCHDOG resets the driver's watchdog (`HKLM\SOFTWARE\Sodium Remote\
//!   Display driver` `watchdog`, 3 s by default): at zero it removes every monitor.
//!   `vdisplay::Matched` pings every second, so the monitor goes with the host.
//!
//! A resize first asks for the mode in place (UPDATE_MODE when the driver has it), then
//! switches with `ChangeDisplaySettingsEx` if Windows lists the mode; otherwise
//! `vdisplay::Matched::set_mode` falls back to a new monitor.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, LUID};
use windows::Win32::Graphics::Gdi::{
    CDS_TYPE, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_SUCCESSFUL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH,
    ENUM_DISPLAY_SETTINGS_FLAGS, ENUM_DISPLAY_SETTINGS_MODE, EnumDisplaySettingsExW,
};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::{GUID, PCWSTR};

use super::topology::{MONITOR_NAME, gdi_of, luid};
use crate::vdisplay::{Key, Mode, Monitor, VirtualDisplay};

/// Sodium Remote Display's device interface.
pub const INTERFACE: GUID = GUID::from_u128(0x53f76209_42ea_44fd_897e_8607b15fa9be);

/// `CTL_CODE(FILE_DEVICE_UNKNOWN, function, METHOD_BUFFERED, FILE_ANY_ACCESS)`.
const fn ctl(function: u32) -> u32 {
    (0x22 << 16) | (function << 2)
}
const IOCTL_ADD: u32 = ctl(0x800);
const IOCTL_REMOVE: u32 = ctl(0x801);
const IOCTL_GET_WATCHDOG: u32 = ctl(0x803);
const IOCTL_UPDATE_MODE: u32 = ctl(0x804);
const IOCTL_PING: u32 = ctl(0x888);
const IOCTL_PROTOCOL: u32 = ctl(0x8FF);

#[repr(C)]
struct AddParams {
    width: u32,
    height: u32,
    refresh: u32,
    guid: GUID,
    name: [u8; 14],
    serial: [u8; 14],
}

#[repr(C)]
#[derive(Default)]
struct AddOut {
    adapter: LUID,
    target: u32,
}

#[repr(C)]
struct RemoveParams {
    guid: GUID,
}

#[repr(C)]
struct UpdateModeParams {
    guid: GUID,
    width: u32,
    height: u32,
    refresh: u32,
}

#[repr(C)]
#[derive(Default)]
struct Protocol {
    major: u8,
    minor: u8,
    incremental: u8,
    test: u8,
}

#[repr(C)]
#[derive(Default)]
struct Watchdog {
    timeout: u32,
    countdown: u32,
}

pub struct SudoVda {
    handle: HANDLE,
    /// The driver has UPDATE_MODE (protocol 0.3 or later).
    in_place: bool,
}

// The handle is a synchronous device handle; DeviceIoControl may be called from any thread.
unsafe impl Send for SudoVda {}
unsafe impl Sync for SudoVda {}

fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// The first Sodium Remote Display device interface's path.
fn device_path() -> Result<Vec<u16>> {
    unsafe {
        let set = SetupDiGetClassDevsW(Some(&INTERFACE), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE)
            .context("SetupDiGetClassDevs")?;
        let r = (|| {
            let mut di = SP_DEVICE_INTERFACE_DATA { cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
            SetupDiEnumDeviceInterfaces(set, None, &INTERFACE, 0, &mut di).context("the virtual display driver isn't installed")?;
            let mut need = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(set, &di, None, 0, Some(&mut need), None);
            if need < size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
                bail!("SetupDiGetDeviceInterfaceDetail: no size");
            }
            // u32 storage keeps the detail structure aligned.
            let mut buf = vec![0u32; (need as usize).div_ceil(4)];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            SetupDiGetDeviceInterfaceDetailW(set, &di, Some(detail), need, None, None).context("SetupDiGetDeviceInterfaceDetail")?;
            let p = (*detail).DevicePath.as_ptr();
            let n = (0..).take_while(|&i| *p.add(i) != 0).count();
            Ok(std::slice::from_raw_parts(p, n).iter().copied().chain([0]).collect())
        })();
        let _ = SetupDiDestroyDeviceInfoList(set);
        r
    }
}

impl SudoVda {
    /// Opens the driver's control device; an error when it isn't installed or doesn't answer.
    pub fn open() -> Result<SudoVda> {
        let path = device_path()?;
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .context("opening the virtual display driver")?;
        let mut d = SudoVda { handle, in_place: false };
        let mut v = Protocol::default();
        d.ioctl(IOCTL_PROTOCOL, None::<&()>, Some(&mut v)).context("the virtual display driver doesn't answer")?;
        if v.major != 0 || v.minor < 2 {
            bail!("the virtual display driver speaks protocol {}.{}.{}; this host needs 0.2", v.major, v.minor, v.incremental);
        }
        d.in_place = v.minor >= 3;
        let mut w = Watchdog::default();
        let _ = d.ioctl(IOCTL_GET_WATCHDOG, None::<&()>, Some(&mut w));
        eprintln!("sodium-remote-host: display driver protocol {}.{}.{}{}, watchdog {} s", v.major, v.minor, v.incremental, if v.test != 0 { " (test build)" } else { "" }, w.timeout);
        Ok(d)
    }

    fn ioctl<I, O>(&self, code: u32, input: Option<&I>, output: Option<&mut O>) -> Result<()> {
        let mut got = 0u32;
        let (out, out_len) = match output {
            Some(o) => (Some(o as *mut O as *mut std::ffi::c_void), size_of::<O>() as u32),
            None => (None, 0),
        };
        unsafe {
            DeviceIoControl(
                self.handle,
                code,
                input.map(|i| i as *const I as *const std::ffi::c_void),
                input.map_or(0, |_| size_of::<I>() as u32),
                out,
                out_len,
                Some(&mut got),
                None,
            )?;
        }
        Ok(())
    }
}

impl Drop for SudoVda {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

/// How long Windows gets to list a mode after UPDATE_MODE.
const MODES_WITHIN: Duration = Duration::from_secs(1);

/// The display's listed mode equal to `mode`, if there is one.
fn listed(name: &[u16], mode: Mode) -> Option<DEVMODEW> {
    let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    for i in 0.. {
        if !unsafe { EnumDisplaySettingsExW(PCWSTR(name.as_ptr()), ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm, ENUM_DISPLAY_SETTINGS_FLAGS(0)) }.as_bool() {
            return None;
        }
        if (dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency) == (mode.width, mode.height, mode.hz) {
            return Some(dm);
        }
    }
    None
}

/// `s` as a NUL-terminated ASCII field.
fn field(s: &str) -> [u8; 14] {
    let mut f = [0u8; 14];
    for (d, c) in f.iter_mut().zip(s.bytes().take(13)) {
        *d = c;
    }
    f
}

impl VirtualDisplay for SudoVda {
    fn create(&self, key: Key, mode: Mode) -> Result<Monitor> {
        let p = AddParams {
            width: mode.width,
            height: mode.height,
            refresh: mode.hz,
            guid: GUID::from_u128(key),
            name: field(MONITOR_NAME),
            serial: field(&format!("SR{:08X}", key as u32)),
        };
        let mut out = AddOut::default();
        self.ioctl(IOCTL_ADD, Some(&p), Some(&mut out)).with_context(|| format!("adding a {}x{} @{} Hz display", mode.width, mode.height, mode.hz))?;
        Ok(Monitor { key, target: out.target, adapter: luid(out.adapter), mode })
    }

    fn set_mode(&self, m: &Monitor, mode: Mode) -> Result<bool> {
        let Some(gdi) = gdi_of(m) else { return Ok(false) };
        let name = wstr(&gdi);
        let mut found = listed(&name, mode);
        if found.is_none() && self.in_place {
            let p = UpdateModeParams { guid: GUID::from_u128(m.key), width: mode.width, height: mode.height, refresh: mode.hz };
            match self.ioctl(IOCTL_UPDATE_MODE, Some(&p), None::<&mut ()>) {
                Ok(()) => {
                    // Windows takes the new target modes in its own time.
                    let until = Instant::now() + MODES_WITHIN;
                    while found.is_none() && Instant::now() < until {
                        std::thread::sleep(Duration::from_millis(50));
                        found = listed(&name, mode);
                    }
                    if found.is_none() {
                        eprintln!("sodium-remote-host: {}x{} @{} Hz isn't listed after an in-place update; adding a new display", mode.width, mode.height, mode.hz);
                    }
                }
                Err(e) => eprintln!("sodium-remote-host: in-place mode update: {e:#}"),
            }
        }
        let Some(mut dm) = found else { return Ok(false) };
        dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
        // No CDS_UPDATEREGISTRY: for now only, never saved.
        let r = unsafe { ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), Some(&dm), None, CDS_TYPE(0), None) };
        Ok(r == DISP_CHANGE_SUCCESSFUL)
    }

    fn remove(&self, key: Key) {
        // Not found (already gone, the watchdog's doing) is fine.
        let _ = self.ioctl(IOCTL_REMOVE, Some(&RemoveParams { guid: GUID::from_u128(key) }), None::<&mut ()>);
    }

    fn keep_alive(&self) -> Result<()> {
        self.ioctl(IOCTL_PING, None::<&()>, None::<&mut ()>)
    }
}
