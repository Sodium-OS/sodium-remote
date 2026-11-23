// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! vinput: synthetic pointer input for the client's nested Hyprland, to drive
//! the reference client's window in tests (keys go through `wtype`). Refuses
//! unless $WAYLAND_DISPLAY belongs to a nested Hyprland (`--config`).
//!
//!   vinput move X Y          absolute, over the first output
//!   vinput wheel N           N wheel clicks down (negative: up), value120
//!   vinput finger DY         a two-finger scroll of DY px in 10 steps, then axis_stop
//!   vinput click             left button down and up
//!   vinput keys CODE...      press and release evdev keycodes, "us" keymap
//!   vinput rel DX DY         relative motion

use anyhow::{Context, Result, bail};
use std::os::fd::AsRawFd;
use std::time::Duration;
use wayland_client::protocol::wl_pointer::{Axis, AxisSource, ButtonState};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_output::WlOutput, wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

#[derive(Default)]
struct St {
    size: (i32, i32),
}
impl Dispatch<WlRegistry, GlobalListContents> for St {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as wayland_client::Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlOutput, ()> for St {
    fn event(s: &mut Self, _: &WlOutput, ev: wayland_client::protocol::wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wayland_client::protocol::wl_output::Event::Mode { width, height, .. } = ev {
            s.size = (width, height);
        }
    }
}
delegate_noop!(St: ignore WlSeat);
delegate_noop!(St: ZwlrVirtualPointerManagerV1);
delegate_noop!(St: ZwlrVirtualPointerV1);
delegate_noop!(St: ZwpVirtualKeyboardManagerV1);
delegate_noop!(St: ZwpVirtualKeyboardV1);

fn ms() -> u32 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
    (t.tv_sec as u64 * 1000 + t.tv_nsec as u64 / 1_000_000) as u32
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let conn = Connection::connect_to_env()?;
    // Only ever into a nested Hyprland.
    let fd = conn.backend().poll_fd().as_raw_fd();
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) };
    let cmdline = std::fs::read(format!("/proc/{}/cmdline", cred.pid)).unwrap_or_default();
    let comm = std::fs::read_to_string(format!("/proc/{}/comm", cred.pid)).unwrap_or_default();
    if comm.trim() != "Hyprland" || !cmdline.split(|&b| b == 0).any(|a| a == b"--config") {
        bail!("refusing: the compositor (pid {}) is not a nested Hyprland", cred.pid);
    }
    let (globals, mut q) = registry_queue_init::<St>(&conn)?;
    let qh = q.handle();
    let mut st = St::default();
    let _o: WlOutput = globals.bind(&qh, 1..=4, ())?;
    let seat: WlSeat = globals.bind(&qh, 1..=8, ())?;
    let pm: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).context("no virtual pointer manager")?;
    q.roundtrip(&mut st)?;
    let p = pm.create_virtual_pointer(Some(&seat), &qh, ());
    let num = |i: usize| -> Result<f64> { args.get(i).context("missing number")?.parse::<f64>().context("number") };
    match args.first().map(String::as_str) {
        Some("move") => {
            p.motion_absolute(ms(), num(1)? as u32, num(2)? as u32, st.size.0.max(1) as u32, st.size.1.max(1) as u32);
            p.frame();
        }
        Some("wheel") => {
            let n = num(1)? as i32;
            p.axis_source(AxisSource::Wheel);
            p.axis_discrete(ms(), Axis::VerticalScroll, 15.0 * n as f64, n);
            p.frame();
        }
        Some("finger") => {
            let dy = num(1)?;
            for _ in 0..10 {
                p.axis_source(AxisSource::Finger);
                p.axis(ms(), Axis::VerticalScroll, dy / 10.0);
                p.frame();
                conn.flush()?;
                std::thread::sleep(Duration::from_millis(8));
            }
            p.axis_source(AxisSource::Finger);
            p.axis_stop(ms(), Axis::VerticalScroll);
            p.frame();
        }
        Some("keys") => {
            // A virtual keyboard with the plain "us" keymap, so evdev codes mean
            // physical keys (wtype uploads its own keymap with made-up codes).
            let km: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).context("no virtual keyboard manager")?;
            let kb = km.create_virtual_keyboard(&seat, &qh, ());
            let keymap = std::process::Command::new("xkbcli").args(["compile-keymap", "--layout", "us"]).output()?.stdout;
            let fd = unsafe { libc::memfd_create(c"keymap".as_ptr(), libc::MFD_CLOEXEC) };
            let mut f = std::fs::File::from(unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd) });
            std::io::Write::write_all(&mut f, &keymap)?;
            std::io::Write::write_all(&mut f, &[0])?;
            kb.keymap(1, std::os::fd::AsFd::as_fd(&f), keymap.len() as u32 + 1);
            q.roundtrip(&mut st)?;
            for code in &args[1..] {
                let code: u32 = code.parse().context("evdev keycode")?;
                kb.key(ms(), code, 1);
                conn.flush()?;
                std::thread::sleep(Duration::from_millis(15));
                kb.key(ms(), code, 0);
                conn.flush()?;
                std::thread::sleep(Duration::from_millis(15));
            }
        }
        Some("rel") => {
            p.motion(ms(), num(1)?, num(2)?);
            p.frame();
        }
        Some("click") => {
            p.button(ms(), 0x110, ButtonState::Pressed);
            p.frame();
            conn.flush()?;
            std::thread::sleep(Duration::from_millis(30));
            p.button(ms(), 0x110, ButtonState::Released);
            p.frame();
        }
        _ => bail!("usage: vinput move X Y | rel DX DY | wheel N | finger DY | click | keys CODE..."),
    }
    q.roundtrip(&mut st)?;
    Ok(())
}
