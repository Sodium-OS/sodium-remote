// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The foreground app (the key table's `Terminal` and `Class` conditions; cross-platform
//! md 7: the foreground window's process image name, with a built-in terminal list),
//! the input desktop (a secure desktop is anything but "Default"), and the computer's
//! name.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, GetUserObjectInformationW, OpenInputDesktop, UOI_NAME};
use windows::Win32::System::SystemInformation::{ComputerNamePhysicalDnsHostname, GetComputerNameExW};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::PWSTR;

use crate::backend::Foreground;

/// Process image names that are terminals: Ctrl stays Control there, and Mac rules
/// marked `Terminal` apply (copy is Ctrl+Shift+C).
pub const TERMINALS: &[&str] = &[
    "WindowsTerminal.exe",
    "OpenConsole.exe",
    "conhost.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "wezterm-gui.exe",
    "alacritty.exe",
    "mintty.exe",
    "kitty.exe",
    "putty.exe",
    "ConEmu64.exe",
    "ConEmu.exe",
    "Tabby.exe",
    "Hyper.exe",
    "Termius.exe",
    "MobaXterm.exe",
];

pub fn computer_name() -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let mut n = buf.len() as u32;
        if GetComputerNameExW(ComputerNamePhysicalDnsHostname, Some(PWSTR(buf.as_mut_ptr())), &mut n).is_ok() && n > 0 {
            return String::from_utf16_lossy(&buf[..n as usize]);
        }
    }
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".into())
}

/// The foreground window's process image name ("WINWORD.EXE"), cached for 200 ms:
/// every key goes through the table, and a window rarely changes between two keys.
pub fn foreground() -> Foreground {
    static CACHE: Mutex<Option<(Instant, isize, Foreground)>> = Mutex::new(None);
    let hwnd = unsafe { GetForegroundWindow() };
    let key = hwnd.0 as isize;
    if let Some((t, h, f)) = CACHE.lock().unwrap().as_ref()
        && *h == key
        && t.elapsed() < Duration::from_millis(200)
    {
        return f.clone();
    }
    let id = image_name(hwnd).unwrap_or_default();
    let terminal = TERMINALS.iter().any(|t| t.eq_ignore_ascii_case(&id));
    let f = Foreground { id, terminal };
    *CACHE.lock().unwrap() = Some((Instant::now(), key, f.clone()));
    f
}

fn image_name(hwnd: windows::Win32::Foundation::HWND) -> Option<String> {
    if hwnd.0.is_null() {
        return None;
    }
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let h: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        r.ok()?;
        let path = String::from_utf16_lossy(&buf[..n as usize]);
        Some(path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string())
    }
}

/// The input desktop is the user's ("Default"): false while a lock, login, UAC or
/// Ctrl+Alt+Del screen is up (Winlogon's desktop, which a user process can't open).
pub fn input_desktop_is_default() -> bool {
    unsafe {
        let Ok(desk) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) else { return false };
        let mut buf = [0u16; 64];
        let mut need = 0u32;
        let ok = GetUserObjectInformationW(HANDLE(desk.0), UOI_NAME, Some(buf.as_mut_ptr().cast()), (buf.len() * 2) as u32, Some(&mut need)).is_ok();
        let _ = CloseDesktop(desk);
        if !ok {
            return false;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n]).eq_ignore_ascii_case("Default")
    }
}
