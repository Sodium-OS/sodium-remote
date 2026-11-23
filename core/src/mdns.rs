// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! LAN discovery (mDNS `_sodium-remote._tcp`, `brand::MDNS_SERVICE`; `_remote._tcp`
//! before protocol v6) and the host's LAN
//! listener, so a LAN connection or pairing needs no server at all.
//!
//! - **Advertised:** one instance of `_sodium-remote._tcp.local.` per host, named after
//!   its display name. The SRV port is the host's LAN listener (TCP). TXT:
//!   `v=1`, `id=<device ID>`, `name=<display name>`, `pair=1|0` (a pairing
//!   window is open).
//! - **The LAN listener** carries what the rendezvous would: pairing messages
//!   and sealed signalling. Frames are a `u32` big-endian length and a postcard
//!   `LanFrame`. The first frame says which (`Pair` or `Signal { client }`).
//!   One pairing attempt or one signalling session per TCP connection.

use crate::keys::DeviceId;
use crate::msg;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const SERVICE: &str = crate::brand::MDNS_SERVICE;
/// Frames on the LAN listener are small: pairing and signalling only.
pub const MAX_FRAME: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct LanHost {
    pub id: DeviceId,
    pub name: String,
    pub addrs: Vec<IpAddr>,
    /// The LAN listener's TCP port.
    pub port: u16,
    pub pairing: bool,
}

/// The host's mDNS announcement.
pub struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
    id: DeviceId,
    name: String,
    port: u16,
}

impl Advertiser {
    pub fn start(id: DeviceId, name: &str, port: u16, pairing: bool) -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
        let mut a = Advertiser { daemon, fullname: String::new(), id, name: name.to_string(), port };
        a.register(pairing)?;
        Ok(a)
    }

    fn register(&mut self, pairing: bool) -> Result<(), String> {
        let id = self.id.to_string();
        // Instance names are unique per host: the display name plus the start
        // of the ID (two hosts may share a name).
        let instance = format!("{} {}", truncate(&self.name, 48), &id[..8]);
        let host_name = format!("sodium-remote-{}.local.", id[..12].to_ascii_lowercase().replace(['_', '-'], "x"));
        let props = [("v", "1"), ("id", id.as_str()), ("name", truncate(&self.name, 64)), ("pair", if pairing { "1" } else { "0" })];
        let info = ServiceInfo::new(SERVICE, &instance, &host_name, "", self.port, &props[..])
            .map_err(|e| e.to_string())?
            .enable_addr_auto();
        self.fullname = info.get_fullname().to_string();
        self.daemon.register(info).map_err(|e| e.to_string())
    }

    /// Re-announce with the pairing flag changed.
    pub fn set_pairing(&mut self, pairing: bool) -> Result<(), String> {
        // Registering the same name again replaces the TXT record.
        self.register(pairing)
    }

    /// Announce under a new display name. The name is part of the instance name,
    /// so the old instance is withdrawn first (a goodbye on the wire: browsers
    /// drop it) and the new one registered, in that order on the daemon's thread.
    pub fn rename(&mut self, name: &str, pairing: bool) -> Result<(), String> {
        if name == self.name {
            return Ok(());
        }
        if !self.fullname.is_empty() {
            let _ = self.daemon.unregister(&self.fullname);
        }
        self.name = name.to_string();
        self.register(pairing)
    }

    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        if let Ok(rx) = self.daemon.unregister(&self.fullname) {
            let _ = rx.recv_timeout(Duration::from_millis(500));
        }
        let _ = self.daemon.shutdown();
    }
}

fn truncate(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// A running mDNS browse; `hosts()` is what's on the LAN now.
pub struct Browser {
    daemon: ServiceDaemon,
    hosts: Arc<Mutex<HashMap<String, LanHost>>>,
}

impl Browser {
    pub fn start() -> Result<Self, String> {
        let daemon = ServiceDaemon::new().map_err(|e| e.to_string())?;
        let rx = daemon.browse(SERVICE).map_err(|e| e.to_string())?;
        let hosts: Arc<Mutex<HashMap<String, LanHost>>> = Default::default();
        let h = hosts.clone();
        std::thread::Builder::new()
            .name("remote-mdns".into())
            .spawn(move || {
                while let Ok(ev) = rx.recv() {
                    match ev {
                        ServiceEvent::ServiceResolved(s) => {
                            let prop = |k: &str| s.get_property_val_str(k).map(String::from);
                            let Some(id) = prop("id").and_then(|i| i.parse::<DeviceId>().ok()) else { continue };
                            if prop("v").as_deref() != Some("1") {
                                continue;
                            }
                            // Link-local IPv6 needs a scope we don't carry: skip it.
                            let mut addrs: Vec<IpAddr> = s
                                .get_addresses()
                                .iter()
                                .map(|a| a.to_ip_addr())
                                .filter(|a| !matches!(a, IpAddr::V6(v) if (v.segments()[0] & 0xffc0) == 0xfe80))
                                .collect();
                            let mut hosts = h.lock().unwrap();
                            // Resolutions can arrive with a subset of the
                            // addresses: keep the union for the same instance.
                            if let Some(old) = hosts.get(s.get_fullname()) {
                                if old.id == id && old.port == s.get_port() {
                                    addrs.extend(old.addrs.iter().filter(|a| !addrs.contains(a)).copied().collect::<Vec<_>>());
                                }
                            }
                            addrs.sort_by_key(|a| (!a.is_ipv4(), *a));
                            hosts.insert(
                                s.get_fullname().to_string(),
                                LanHost {
                                    id,
                                    name: prop("name").unwrap_or_default(),
                                    addrs,
                                    port: s.get_port(),
                                    pairing: prop("pair").as_deref() == Some("1"),
                                },
                            );
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            h.lock().unwrap().remove(&fullname);
                        }
                        ServiceEvent::SearchStopped(_) => break,
                        _ => {}
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Browser { daemon, hosts })
    }

    pub fn hosts(&self) -> Vec<LanHost> {
        self.hosts.lock().unwrap().values().cloned().collect()
    }

    pub fn find(&self, id: &DeviceId) -> Option<LanHost> {
        self.hosts.lock().unwrap().values().find(|h| h.id == *id && !h.addrs.is_empty()).cloned()
    }

    /// Wait up to `timeout` for a host that satisfies `pred`.
    pub async fn wait_for(&self, timeout: Duration, pred: impl Fn(&LanHost) -> bool) -> Option<LanHost> {
        let end = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(h) = self.hosts().into_iter().find(&pred) {
                return Some(h);
            }
            if tokio::time::Instant::now() >= end {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(SERVICE);
        let _ = self.daemon.shutdown();
    }
}

// ---------------------------------------------------------------------------
// The LAN listener's framing

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum LanFrame {
    /// First frame of a pairing connection.
    PairHello,
    /// First frame of a signalling connection: who is calling (the blobs are
    /// sealed by that device, so a lie gets nothing opened).
    SignalHello { client: [u8; 32] },
    /// A pairing message (opaque, as `pair_msg` blobs).
    Pair(Vec<u8>),
    /// A sealed signalling blob (as `signal` blobs).
    Signal(Vec<u8>),
    /// The host's pairing outcome for this attempt when it isn't in a message.
    PairClosed,
}

pub async fn write_frame<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, f: &LanFrame) -> std::io::Result<()> {
    let b = msg::encode(f);
    let mut out = (b.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(&b);
    w.write_all(&out).await?;
    w.flush().await
}

/// The next frame, or `None` at EOF or on a malformed one.
pub async fn read_frame<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> Option<LanFrame> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await.ok()?;
    let n = u32::from_be_bytes(len) as usize;
    if n > MAX_FRAME {
        return None;
    }
    let mut b = vec![0u8; n];
    r.read_exact(&mut b).await.ok()?;
    msg::decode(&b)
}
