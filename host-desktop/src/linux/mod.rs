// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The generic Linux backend (`docs/reference/host-linux.md`): any desktop whose
//! xdg-desktop-portal implements RemoteDesktop and ScreenCast (GNOME, KDE Plasma 6).
//!
//! - `portal`: the portal session on a thread of its own (`ashpd`): the sharing dialog
//!   and its restore token, the PipeWire remote, ConnectToEIS, the Notify methods, the
//!   clipboard, the session's end.
//! - `capture`: the ScreenCast node over PipeWire (`remote-linux-media`): SHM or
//!   DMA-BUF pictures, damage, the pointer from SPA_META_Cursor.
//! - `input`: libei through `reis` (ConnectToEIS), else the portal's Notify methods.
//! - `clipboard`: the portal Clipboard: text, HTML, PNG.
//! - audio: the default sink's monitor (`remote-linux-media`).
//! - encoders: `encode::Probe` with `Candidates::linux()`: VA-API, NVENC, libx264,
//!   libopenh264, OpenH264.
//!
//! What it reports: one display (the one chosen in the dialog), `display_mode`
//! Physical, no live resize, no secure screens, no foreground app, no access before
//! login. The portal session opens when a device connects (`check_ready`) and closes
//! when its session ends; stopping the share at the computer ends the session.

pub mod capture;
pub mod clipboard;
pub mod input;
pub mod lock;
pub mod portal;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use remote_core::msg::Os;

use crate::backend::*;
use crate::encode::{Candidates, Probe};

/// The names this backend adds of its own (the rest come
/// from `remote_core::brand` and `remote_core::dirs`).
pub mod names {
    /// The portal's restore token, in the data directory.
    pub const RESTORE_TOKEN: &str = "portal-restore-token";
    /// The PipeWire node that captures the default output, and the name given to EIS.
    pub const CLIENT_NAME: &str = remote_core::brand::BIN_HOST;
}

/// the host half, `$XDG_DATA_HOME/sodium-remote/host`, without the first-start
/// migration: the pre-rename `sodiumos/remote` is remoted's (the SodiumOS host's)
/// identity, never this host's.
pub fn default_data_dir() -> Result<PathBuf> {
    Ok(remote_core::dirs::data_dir(remote_core::dirs::Half::Host)?)
}

/// The sharing permission saved in a data directory, for when no host runs (the pane's
/// state while "Allow remote control" is off): a restore token means "remembered".
pub fn stored_consent(data: &std::path::Path) -> Consent {
    if data.join(names::RESTORE_TOKEN).exists() { Consent::Remembered } else { Consent::Ask }
}

/// Forgets the saved permission in a data directory (no host running).
pub fn forget_stored_consent(data: &std::path::Path) {
    let _ = std::fs::remove_file(data.join(names::RESTORE_TOKEN));
}

/// The lock file in the data directory: held while a host serves from it.
pub const LOCK_FILE: &str = "host.lock";

/// Another host (the app's "This computer", or `sodium-remote-host`) already serves from this data directory.
/// The apps recognise it with `downcast_ref` and show their own translated line.
#[derive(Debug)]
pub struct DataDirInUse(pub PathBuf);

impl std::fmt::Display for DataDirInUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "another Sodium Remote host is already sharing this computer from {}: quit it (the app's \"This computer\" switch, or the sodium-remote-host program) and try again",
            self.0.display()
        )
    }
}

impl std::error::Error for DataDirInUse {}

/// An exclusive `flock` on `LOCK_FILE` in the data directory, for as long as it lives (the kernel drops it when the
/// process ends, however it ends). Two hosts on one directory would fight over the restore token and the pairing list.
#[derive(Debug)]
pub struct DataLock(#[allow(dead_code)] std::fs::File);

impl DataLock {
    pub fn take(data: &std::path::Path) -> Result<DataLock> {
        use rustix::fs::{FlockOperation, flock};
        std::fs::create_dir_all(data).with_context(|| format!("creating {}", data.display()))?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(data.join(LOCK_FILE))
            .with_context(|| format!("opening {}", data.join(LOCK_FILE).display()))?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(DataLock(file)),
            Err(rustix::io::Errno::WOULDBLOCK) => Err(DataDirInUse(data.to_path_buf()).into()),
            Err(e) => Err(anyhow::Error::new(e).context(format!("locking {}", data.join(LOCK_FILE).display()))),
        }
    }
}

/// After the last paired device is unpaired (no host running): the saved permission goes with it, so the next device
/// to pair is asked again. True when it was forgotten.
pub fn forget_stored_consent_if_unpaired(data: &std::path::Path) -> bool {
    let store = remote_core::keys::FileStore::new(data.to_path_buf());
    if !remote_core::keys::Pairings::load(&store).devices.is_empty() {
        return false;
    }
    forget_stored_consent(data);
    true
}

pub struct PortalBackend {
    probe: Probe,
    portal: Arc<portal::Portal>,
    _lock: Option<DataLock>,
}

impl PortalBackend {
    /// `data` holds the restore token and is locked for this host (`DataDirInUse` when another host has it).
    /// `encoder` limits the encoders to one (tests).
    pub fn new(data: PathBuf, encoder: Option<&str>) -> Result<PortalBackend> {
        let lock = DataLock::take(&data)?;
        let mut b = Self::new_unlocked(data, encoder)?;
        b._lock = Some(lock);
        Ok(b)
    }

    /// Without the lock: for `--list` and `--forget-consent`, which only look at the directory.
    pub fn new_unlocked(data: PathBuf, encoder: Option<&str>) -> Result<PortalBackend> {
        let cands = encoder.map(Candidates::only).unwrap_or_else(Candidates::linux);
        Ok(PortalBackend { probe: Probe::new(cands), portal: portal::Portal::start(data)?, _lock: None })
    }

    /// The candidates that work here, best first (`--list`).
    pub fn working_encoders(&self) -> Vec<&'static str> {
        self.probe.works()
    }

    /// DMA-BUF only when a VA-API encoder works (it alone takes DMA-BUF pictures), and
    /// not when switched off (`SODIUM_REMOTE_DMABUF=0`) or after a failed import.
    fn dmabuf(&self) -> bool {
        std::env::var("SODIUM_REMOTE_DMABUF").as_deref() != Ok("0")
            && !capture::dmabuf_is_off()
            && self.probe.works().iter().any(|n| n.ends_with("_vaapi"))
    }
}

/// This computer's name: the pretty hostname (`hostnamectl`'s), else the hostname.
fn computer_name() -> String {
    let pretty = std::fs::read_to_string("/etc/machine-info").ok().and_then(|t| {
        t.lines().find_map(|l| l.strip_prefix("PRETTY_HOSTNAME=").map(|v| v.trim().trim_matches('"').to_string()))
    });
    pretty
        .filter(|n| !n.is_empty())
        .or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok().map(|s| s.trim().to_string()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Linux".into())
}

impl Backend for PortalBackend {
    fn os(&self) -> Os {
        Os::Linux
    }
    fn os_flavor(&self) -> String {
        remote_core::os_release::flavor()
    }
    fn computer_name(&self) -> String {
        computer_name()
    }
    fn displays(&self) -> Vec<Display> {
        self.portal.display().into_iter().collect()
    }
    fn open_capture(&self, _display: u32, fps: u32) -> Result<Box<dyn Capture>> {
        Ok(Box::new(capture::PortalCapture::open(&self.portal, fps, self.dmabuf())?))
    }
    fn encoder_caps(&self) -> EncoderCaps {
        self.probe.caps()
    }
    fn open_encoder(&self, cfg: &EncoderConfig) -> Result<Box<dyn Encoder>> {
        self.probe.open(cfg)
    }
    fn input(&self) -> Result<Box<dyn Input>> {
        Ok(Box::new(input::PortalInput::new(self.portal.clone())))
    }
    fn audio(&self) -> Result<Box<dyn AudioSource>> {
        Ok(Box::new(MonitorSource(remote_linux_media::audio::Monitor::open(names::CLIENT_NAME).context("PipeWire")?)))
    }
    fn clipboard(&self, on_change: ClipSink) -> Result<Arc<dyn Clipboard>> {
        Ok(clipboard::PortalClipboard::start(self.portal.clone(), on_change))
    }
    fn foreground(&self) -> Foreground {
        // No portal tells which app has focus: the Terminal and Class key rules are off.
        Foreground::default()
    }
    fn check_ready(&self) -> Result<(), NotReady> {
        // Probe first (VA-API decides DMA-BUF), then the portal session: the dialog
        // the first time, the restore token after that.
        let dmabuf = self.dmabuf();
        self.portal.ensure_started(dmabuf)
    }
    fn session_active(&self, on: bool) {
        if !on {
            self.portal.stop();
        }
    }
    fn consent(&self) -> Option<Consent> {
        Some(self.portal.consent())
    }
    fn view_only(&self) -> bool {
        self.portal.active().is_some_and(|a| a.view_only)
    }
    fn locked(&self) -> bool {
        self.portal.locked()
    }
    fn forget_consent(&self) {
        self.portal.forget_consent();
    }
}

/// The default output's monitor, 10 ms at a time.
struct MonitorSource(remote_linux_media::audio::Monitor);

impl AudioSource for MonitorSource {
    fn read(&mut self, out: &mut [f32; 960], timeout: Duration) -> Result<bool> {
        Ok(self.0.read(out, timeout)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_host_on_one_data_dir_is_refused_until_the_first_ends() {
        let d = std::env::temp_dir().join(format!("sr-lock-{}", std::process::id()));
        let first = DataLock::take(&d).unwrap();
        let err = DataLock::take(&d).unwrap_err();
        assert!(err.downcast_ref::<DataDirInUse>().is_some(), "{err:#}");
        assert!(err.to_string().contains("already sharing"));
        drop(first);
        DataLock::take(&d).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_restore_token_goes_when_the_last_device_is_unpaired() {
        use remote_core::keys::{DeviceKey, FileStore, PairedDevice, Pairings};
        let d = std::env::temp_dir().join(format!("sr-token-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let store = FileStore::new(d.clone());
        let dev = PairedDevice { id: DeviceKey::generate().id(), name: "Laptop".into(), paired_at: 1, last_connected: None, named_at_desk: false, lan_addr: None };
        let mut p = Pairings::default();
        p.upsert(dev.clone());
        p.save(&store).unwrap();
        std::fs::write(d.join(names::RESTORE_TOKEN), "token").unwrap();
        assert!(!forget_stored_consent_if_unpaired(&d), "a device is still paired");
        assert_eq!(stored_consent(&d), Consent::Remembered);
        p.remove(&dev.id);
        p.save(&store).unwrap();
        assert!(forget_stored_consent_if_unpaired(&d));
        assert_eq!(stored_consent(&d), Consent::Ask);
        let _ = std::fs::remove_dir_all(&d);
    }
}
