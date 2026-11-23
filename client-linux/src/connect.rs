// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The Linux side of the clocks: Wayland's event times are CLOCK_MONOTONIC, so
//! they become `Instant`s the engine can stamp with; and the default-route
//! check for the connection timeline's "Waiting for the network…".

pub use remote_engine::connect::*;
use std::time::{Duration, Instant};

/// A Wayland event time (ms, CLOCK_MONOTONIC on Hyprland, wrapping) as an
/// `Instant` (Rust's is CLOCK_MONOTONIC too). None when it isn't plausibly
/// that clock (in the future, or older than 10 s).
pub fn wayland_time(ms: u32) -> Option<Instant> {
    let now_ns = mono_ns();
    let now_ms = (now_ns / 1_000_000) as u32;
    let age_ms = now_ms.wrapping_sub(ms) as i32;
    if !(0..=10_000).contains(&age_ms) {
        return None;
    }
    // The event happened within its millisecond: at most 1 ms early.
    let event_ns = (now_ns / 1_000_000 - age_ms as u64) * 1_000_000;
    Instant::now().checked_sub(Duration::from_nanos(now_ns - event_ns))
}

/// A µs timestamp of the same clock (zwp_relative_pointer's `utime`).
pub fn wayland_time_us(us: u64) -> Option<Instant> {
    let age_us = (mono_ns() / 1000).checked_sub(us)?;
    (age_us <= 10_000_000).then(|| Instant::now().checked_sub(Duration::from_micros(age_us))).flatten()
}

/// Linux has a default route (IPv4 or IPv6): "Waiting for the network…"
/// otherwise.
pub fn has_default_route() -> bool {
    let v4 = std::fs::read_to_string("/proc/net/route").unwrap_or_default();
    // Iface Destination Gateway Flags …: destination 0, flag RTF_UP (1).
    let v4 = v4.lines().skip(1).any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        f.len() > 3 && f[1] == "00000000" && u32::from_str_radix(f[3], 16).is_ok_and(|fl| fl & 1 != 0)
    });
    let v6 = std::fs::read_to_string("/proc/net/ipv6_route").unwrap_or_default();
    // dest dest_len src src_len next_hop metric refcnt use flags iface: ::/0,
    // up, not the kernel's unreachable entry on lo.
    let v6 = v6.lines().any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        f.len() > 9
            && f[0].bytes().all(|b| b == b'0')
            && f[1] == "00"
            && u32::from_str_radix(f[8], 16).is_ok_and(|fl| fl & 1 != 0)
            && f[9] != "lo"
    });
    v4 || v6
}
