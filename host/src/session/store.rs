// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! What Settings › Remote changes, and the recent connections (RemoteSettings
//! defaults: Notify on, connect while locked on, Mirror by default; 30 days of
//! history, metadata only). Both live in the data dir next to `device.key`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{display::HomeMode, keys::write_private};

pub const SETTINGS_FILE: &str = "settings.json";
pub const RECENT_FILE: &str = "recent.json";
pub const RECENT_DAYS: u64 = 30;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Remote itself: off stops listening and leaves the pairings.
    pub enabled: bool,
    /// What paired devices call this computer. Empty: the computer's own name,
    /// followed live.
    pub name: String,
    /// Visible on this network (mDNS).
    pub mdns: bool,
    pub connect_while_locked: bool,
    /// The desk monitor's default mode; a device can change it for its session.
    pub home_mode: HomeMode,
    /// Notify when a device connects, disconnects or takes over.
    pub notify: bool,
}

pub fn hostname() -> String {
    rustix::system::uname().nodename().to_string_lossy().into_owned()
}

/// The computer's name as hostnamed keeps it: PRETTY_HOSTNAME in
/// /etc/machine-info, else the hostname.
pub fn computer_name() -> String {
    std::fs::read_to_string("/etc/machine-info")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| l.strip_prefix("PRETTY_HOSTNAME=").map(|v| v.trim().trim_matches('"').to_string()))
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(hostname)
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: true, name: String::new(), mdns: true, connect_while_locked: true, home_mode: HomeMode::Mirror, notify: true }
    }
}

impl Settings {
    /// The name paired devices see: the user's, or the computer's own.
    pub fn display_name(&self) -> String {
        if self.name.is_empty() { computer_name() } else { self.name.clone() }
    }

    pub fn load(dir: &Path) -> Settings {
        std::fs::read(dir.join(SETTINGS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        write_private(&dir.join(SETTINGS_FILE), &serde_json::to_vec_pretty(self)?, 0o600)
    }
}

/// One past connection: device, time and path only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub device: String,
    pub name: String,
    /// Unix seconds; `end` 0 while it is going on.
    pub start: u64,
    pub end: u64,
    /// "lan", "direct", "vpn", "tailscale", "relay" …
    pub path: String,
    /// "udp", "tcp", "tls".
    pub transport: String,
    pub rtt_ms: u32,
}

pub struct RecentLog {
    path: PathBuf,
    pub entries: Vec<Recent>,
}

impl RecentLog {
    pub fn open(dir: &Path) -> RecentLog {
        let path = dir.join(RECENT_FILE);
        let entries = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        RecentLog { path, entries }
    }

    pub fn save(&mut self, now: u64) -> Result<()> {
        let cutoff = now.saturating_sub(RECENT_DAYS * 86_400);
        self.entries.retain(|e| e.end == 0 || e.end >= cutoff);
        std::fs::create_dir_all(self.path.parent().unwrap())?;
        write_private(&self.path, &serde_json::to_vec(&self.entries)?, 0o600)
    }

    pub fn begin(&mut self, e: Recent) -> Result<()> {
        let now = e.start;
        self.entries.insert(0, e);
        self.save(now)
    }

    /// The current entry's path changed (a path switch, or the first stats).
    pub fn update_path(&mut self, path: &str, transport: &str, rtt_ms: u32) {
        if let Some(e) = self.entries.first_mut().filter(|e| e.end == 0) {
            e.path = path.into();
            e.transport = transport.into();
            e.rtt_ms = rtt_ms;
        }
    }

    pub fn finish(&mut self, now: u64) -> Result<()> {
        for e in self.entries.iter_mut().filter(|e| e.end == 0) {
            e.end = now.max(e.start);
        }
        self.save(now)
    }

    pub fn clear(&mut self, now: u64) -> Result<()> {
        self.entries.retain(|e| e.end == 0);
        self.save(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_connections_keep_30_days() {
        let dir = std::env::temp_dir().join(format!("remoted-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let day = 86_400;
        let now = 100 * day;
        let mut log = RecentLog::open(&dir);
        let e = |start, end| Recent { device: "d".into(), name: "Air".into(), start, end, path: "direct".into(), transport: "udp".into(), rtt_ms: 9 };
        log.entries = vec![e(now - 31 * day, now - 31 * day + 60), e(now - 29 * day, now - 29 * day + 60)];
        log.begin(e(now, 0)).unwrap();
        assert_eq!(log.entries.len(), 2); // the 31-day-old one is gone
        log.finish(now + 60).unwrap();
        assert_eq!(RecentLog::open(&dir).entries[0].end, now + 60);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
