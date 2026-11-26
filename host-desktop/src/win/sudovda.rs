// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! SudoVDA, the virtual display driver (SudoMaker's, MIT; we build and sign it ourselves:
//! `packaging/windows/sudovda/`), as `vdisplay::VirtualDisplay`, through the control
//! IOCTLs of its header `Common/Include/sudovda-ioctl.h` (protocol 0.2):
//!
//! - the device is found by its interface GUID, `{e5bcc234-1e0c-418a-a0d4-ef8b7501414d}`;
//! - ADD (width, height, refresh in Hz, the monitor's GUID, an EDID name and serial)
//!   returns the adapter's LUID and the target id; the monitor is there at once;
//! - REMOVE takes the GUID;
//! - any IOCTL but GET_WATCHDOG resets the driver's watchdog (`HKLM\SOFTWARE\SudoMaker\SudoVDA`
//!   `watchdog`, 3 s by default): at zero it removes every monitor. `vdisplay::Matched`
//!   pings every second, so the monitor goes with the host.
//!
//! There is no IOCTL to change a monitor's mode. A monitor lists the mode it was made with
//! plus some variants and presets, and `ChangeDisplaySettingsEx` reaches those in place;
//! any other size is a new monitor (`vdisplay::Matched::set_mode`).

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

/// SudoVDA's device interface.
pub const INTERFACE: GUID = GUID::from_u128(0xe5bcc234_1e0c_418a_a0d4_ef8b7501414d);

/// `CTL_CODE(FILE_DEVICE_UNKNOWN, function, METHOD_BUFFERED, FILE_ANY_ACCESS)`.
const fn ctl(function: u32) -> u32 {
    (0x22 << 16) | (function << 2)
}
const IOCTL_ADD: u32 = ctl(0x800);
const IOCTL_REMOVE: u32 = ctl(0x801);
const IOCTL_GET_WATCHDOG: u32 = ctl(0x803);
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
}

// The handle is a synchronous device handle; DeviceIoControl may be called from any thread.
unsafe impl Send for SudoVda {}
unsafe impl Sync for SudoVda {}

fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// The first SudoVDA device interface's path.
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
        let d = SudoVda { handle };
        let mut v = Protocol::default();
        d.ioctl(IOCTL_PROTOCOL, None::<&()>, Some(&mut v)).context("the virtual display driver doesn't answer")?;
        if v.major != 0 || v.minor < 2 {
            bail!("the virtual display driver speaks protocol {}.{}.{}; this host needs 0.2", v.major, v.minor, v.incremental);
        }
        let mut w = Watchdog::default();
        let _ = d.ioctl(IOCTL_GET_WATCHDOG, None::<&()>, Some(&mut w));
        eprintln!("sodium-remote-host: SudoVDA protocol {}.{}.{}{}, watchdog {} s", v.major, v.minor, v.incremental, if v.test != 0 { " (test build)" } else { "" }, w.timeout);
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
        let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
        let mut listed = false;
        for i in 0.. {
            if !unsafe { EnumDisplaySettingsExW(PCWSTR(name.as_ptr()), ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm, ENUM_DISPLAY_SETTINGS_FLAGS(0)) }.as_bool() {
                break;
            }
            if (dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency) == (mode.width, mode.height, mode.hz) {
                listed = true;
                break;
            }
        }
        if !listed {
            return Ok(false);
        }
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
