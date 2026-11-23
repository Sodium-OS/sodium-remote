// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The client's side of `remote_core::connect`: the
//! `ClientConfig` from the command line, picking a host, and the clock helpers
//! that line the session's µs times up with CLOCK_MONOTONIC (Wayland's
//! presentation and input times, and the host's logs on one machine).

use anyhow::{Context, Result, bail};
use remote_core::connect::{Client, ClientConfig, ClientEvent};
use remote_core::keys::{DeviceId, FileStore, PairedDevice};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc::UnboundedReceiver;

/// How the client reaches hosts (shared by `pair` and `connect`).
#[derive(Clone, Debug)]
pub struct NetOptions {
    /// Device key and `paired.json` (a `keys::FileStore`).
    pub data: PathBuf,
    /// This device's name, as the host shows it.
    pub name: String,
    pub rendezvous: bool,
    pub mdns: bool,
    pub relay_only: bool,
    /// Local addresses to bind (default: every IPv4 interface).
    pub bind: Vec<IpAddr>,
    /// Known LAN listeners, for when mDNS can't be used.
    pub lan_hints: Vec<(DeviceId, SocketAddr)>,
    /// The stream rate for pacing and feedback.
    pub fps: u32,
}

/// Starts the core's client with these options.
pub async fn start(o: &NetOptions) -> Result<(Client, UnboundedReceiver<ClientEvent>)> {
    std::fs::create_dir_all(&o.data).with_context(|| format!("creating {}", o.data.display()))?;
    let mut cfg = ClientConfig::new(Arc::new(FileStore::new(&o.data)), o.name.clone());
    // The server `server.toml` names, if any (nothing is compiled in).
    cfg.rendezvous = if o.rendezvous { remote_core::server_config::ServerConfig::load_rendezvous() } else { None };
    cfg.mdns = o.mdns;
    cfg.relay_only = o.relay_only;
    if !o.bind.is_empty() {
        cfg.bind = Some(o.bind.clone());
    }
    cfg.lan_hints = o.lan_hints.clone();
    cfg.fps = o.fps;
    Ok(Client::start(cfg).await?)
}

/// `ID@IP:PORT`: a host's LAN listener.
pub fn parse_lan_hint(s: &str) -> Result<(DeviceId, SocketAddr)> {
    let (id, addr) = s.split_once('@').context("--lan-hint ID@IP:PORT")?;
    let id: DeviceId = id.parse().map_err(|e| anyhow::anyhow!("--lan-hint: bad device ID: {e:?}"))?;
    Ok((id, addr.parse().context("--lan-hint: bad IP:PORT")?))
}

/// Does `sel` name this device: its full ID, or its code (`XXXX-XXXX`)?
pub fn names(id: &DeviceId, sel: &str) -> bool {
    id.to_string() == sel || id.code_matches(sel)
}

/// The paired host to connect to: `--host`, else the only one.
pub fn pick_host(paired: &[PairedDevice], sel: Option<&str>) -> Result<DeviceId> {
    match sel {
        Some(s) => paired.iter().find(|d| names(&d.id, s)).map(|d| d.id).with_context(|| format!("no paired host {s}")),
        None => match paired {
            [] => bail!("no paired host: run `remote-client --data DIR pair` first"),
            [d] => Ok(d.id),
            _ => bail!(
                "{} paired hosts; choose one with --host: {}",
                paired.len(),
                paired.iter().map(|d| format!("{} ({})", d.id.code(), d.name)).collect::<Vec<_>>().join(", ")
            ),
        },
    }
}

/// The paired host to connect to: `--host` as an ID or code (`pick_host`), or as a bare LAN
/// address `IP[:PORT]`, which finds the ID stored for that address (never one read from the
/// network) and tells the core where to dial.
pub fn pick_host_or_address(client: &Client, sel: Option<&str>) -> Result<DeviceId> {
    if let Some(s) = sel.filter(|s| s.contains('.') || s.contains(':')) {
        let addr = remote_core::connect::parse_address(s).with_context(|| format!("--host ID, CODE or IP[:PORT], not {s:?}"))?;
        let id = client.host_at(addr).with_context(|| format!("no paired host at {addr}: pair first with `pair --address {s}`"))?;
        client.hint_lan(id, addr);
        return Ok(id);
    }
    pick_host(&client.paired(), sel)
}

/// The client's own clock: µs since `epoch`, mapped to CLOCK_MONOTONIC so its
/// stamps line up with Wayland presentation times and, on one machine, the
/// host's logs. Session times (`ReceivedFrame`'s µs fields) are converted to it
/// with `Live::offset_us`.
#[derive(Clone, Copy, Debug)]
pub struct SessionClock {
    pub epoch: Instant,
    pub epoch_mono_ns: u64,
}

impl SessionClock {
    pub fn now() -> Self {
        let epoch = Instant::now();
        SessionClock { epoch, epoch_mono_ns: mono_ns() }
    }
    pub fn us(&self, t: Instant) -> u64 {
        t.saturating_duration_since(self.epoch).as_micros() as u64
    }
    /// Like `us`, negative before the epoch.
    pub fn us_signed(&self, t: Instant) -> i64 {
        match t.checked_duration_since(self.epoch) {
            Some(d) => d.as_micros() as i64,
            None => -(self.epoch.duration_since(t).as_micros() as i64),
        }
    }
    pub fn now_us(&self) -> u64 {
        self.us(Instant::now())
    }
    /// A CLOCK_MONOTONIC time (ns) as client µs (negative before the epoch).
    pub fn mono_to_us(&self, mono_ns: u64) -> i64 {
        (mono_ns as i64 - self.epoch_mono_ns as i64) / 1000
    }
    pub fn us_to_mono_ns(&self, us: i64) -> u64 {
        (self.epoch_mono_ns as i64 + us * 1000) as u64
    }
}

/// CLOCK_MONOTONIC (ns) on Unix; on other systems, ns since the first call
/// (the clock only has to be one the client's own stamps share).
#[cfg(unix)]
pub fn mono_ns() -> u64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

#[cfg(not(unix))]
pub fn mono_ns() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1_000_000_000
}
