// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The UniFFI surface for Remote.app's "This computer" pane (11;
//! docs/reference/host-mac.md "The app's interface"). The host runs in-process, so the Screen
//! Recording and Accessibility permissions belong to the app. Built with the `ffi`
//! feature, which also turns on remote-core's: one static library holds both crates'
//! scaffolding, and both crates' Swift files go into the app's one `RemoteCore`
//! module (`build-swift.sh`).
//!
//! - `ThisComputer` owns a tokio runtime and, while "Allow remote control of this
//!   computer" is on, a `session::Session` on the Mac backend. Its methods never block
//!   the caller for long; the async ones are Swift `async`.
//! - Events come through `ThisComputerListener.onEvent`, on a Rust thread: hop to your
//!   own queue. AppKit's main thread must not wait on these calls synchronously while
//!   a session runs (the backend reads NSScreen and NSCursor there).
//! - Its files (device key, paired devices, `host.txt`) live in `data_dir`, apart from
//!   the client's: this Mac is one device as a client and another as a host.
//! - Names avoid remote-core's (`Device`, `RemoteEvent`, …): the two share a module.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use remote_core::keys::{DeviceId, FileStore, Store};
use remote_core::msg::Os;

use crate::backend::Backend;
use crate::session::{self, Note, Options, PairedClient, Session, Status};

/// Receives every event of the host, on a Rust thread.
#[uniffi::export(with_foreign)]
pub trait ThisComputerListener: Send + Sync {
    fn on_event(&self, event: ThisComputerEvent);
}

/// Where the app keeps the host's key and pairings, when not as files in the data folder: the Mac app's
/// Keychain (`device.key`). The same shape as core's `KeyStore`, but this crate's own trait, so the bindings
/// don't need a foreign trait across the two crates.
#[uniffi::export(with_foreign)]
pub trait HostKeyStore: Send + Sync {
    fn load(&self, name: String) -> Option<Vec<u8>>;
    /// False if it couldn't be saved.
    fn save(&self, name: String, data: Vec<u8>) -> bool;
    fn remove(&self, name: String);
}

struct HostStoreAdapter(Arc<dyn HostKeyStore>);

impl Store for HostStoreAdapter {
    fn load(&self, name: &str) -> Option<Vec<u8>> {
        self.0.load(name.to_string())
    }
    fn save(&self, name: &str, data: &[u8]) -> std::io::Result<()> {
        if self.0.save(name.to_string(), data.to_vec()) { Ok(()) } else { Err(std::io::Error::other("HostKeyStore.save failed")) }
    }
    fn remove(&self, name: &str) -> std::io::Result<()> {
        self.0.remove(name.to_string());
        Ok(())
    }
}

/// Who is controlling this Mac.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct ControllingDevice {
    /// The device ID (base64url).
    pub id: String,
    pub name: String,
    pub os: Os,
    /// Its os-release `ID`, lowercase; empty off Linux.
    #[uniffi(default = "")]
    pub os_flavor: String,
    /// When the session started, Unix seconds.
    pub since: u64,
    /// The connection was lost; the session is held for it to come back.
    pub reconnecting: bool,
    /// Input can't be injected (Accessibility isn't allowed): show the "View only" chip
    /// beside the device, and a card that leads to Accessibility. Follows an Allow
    /// at the next status change (a connect, the lock screen, a reconnect).
    pub view_only: bool,
}

/// What the pane, the menu-bar item and the notification show.
#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum ThisComputerState {
    /// "Allow remote control of this computer" is off.
    Off,
    /// On, nobody connected.
    Waiting,
    BeingControlled { by: ControllingDevice },
    /// The lock screen is up: the controlling device sees a notice until it goes.
    SecureScreen { by: ControllingDevice },
}

/// An open pairing window's code.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct PairingCode {
    /// Six digits.
    pub code: String,
    pub expires_in_secs: u64,
}

/// A device allowed to control this Mac.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct AllowedDevice {
    /// The device ID (base64url): what `revoke` takes.
    pub id: String,
    /// Its short code, `XXXX-XXXX`.
    pub code: String,
    pub name: String,
    /// The OS of its last connection; None until it has connected.
    pub os: Option<Os>,
    /// Its os-release `ID`, lowercase; empty off Linux.
    #[uniffi(default = "")]
    pub os_flavor: String,
    /// Unix seconds.
    pub paired_at: u64,
    pub last_used: Option<u64>,
}

impl From<PairedClient> for AllowedDevice {
    fn from(c: PairedClient) -> Self {
        AllowedDevice {
            id: c.device.id.to_string(),
            code: c.device.id.code(),
            name: c.device.name,
            os: c.os,
            os_flavor: c.os_flavor,
            paired_at: c.device.paired_at,
            last_used: c.device.last_connected,
        }
    }
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum ThisComputerEvent {
    State { state: ThisComputerState },
    /// A pairing window opened (also the answer to `open_pairing`).
    PairingOpened { pairing: PairingCode },
    /// A device is trying the code.
    PairingAttempt,
    PairingFailed { attempts_left: u32 },
    /// The window ended: "paired", "expired", "attempts_used", "cancelled", …
    PairingClosed { reason: String },
    /// The allowed devices changed (paired, revoked): read `devices()` again.
    DevicesChanged,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionState {
    Allowed,
    NotAllowed,
    /// Screen Recording only: asked for in this run but not in effect. An Allow works
    /// only after a relaunch, and macOS doesn't say whether the person allowed it, so
    /// the row shows "Reopen to finish" with "Quit and Reopen", never
    /// "Allowed".
    ReopenToFinish,
}

#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostPermissions {
    /// Screen & System Audio Recording: needed to show anything.
    pub screen_recording: PermissionState,
    /// Accessibility: needed for keys and the pointer.
    pub accessibility: PermissionState,
}

#[derive(uniffi::Error, Debug, Clone, PartialEq)]
pub enum ThisComputerError {
    /// "Allow remote control" is off.
    Off,
    /// Not an ID of an allowed device.
    UnknownDevice,
    Failed { message: String },
}

impl std::fmt::Display for ThisComputerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

fn failed(e: impl std::fmt::Display) -> ThisComputerError {
    ThisComputerError::Failed { message: e.to_string() }
}

fn state_of(s: &Status) -> ThisComputerState {
    match s {
        Status::Waiting => ThisComputerState::Waiting,
        Status::Controlled { client, secure, held } => {
            let by = ControllingDevice { id: client.id.to_string(), name: client.name.clone(), os: client.os, os_flavor: client.os_flavor.clone(), since: client.since, reconnecting: *held, view_only: client.view_only };
            if *secure { ThisComputerState::SecureScreen { by } } else { ThisComputerState::BeingControlled { by } }
        }
    }
}

fn permission(on: bool) -> PermissionState {
    if on { PermissionState::Allowed } else { PermissionState::NotAllowed }
}

#[cfg(target_os = "macos")]
fn screen_recording_state(on: bool) -> PermissionState {
    if !on && crate::mac::permissions::screen_recording_asked() { PermissionState::ReopenToFinish } else { permission(on) }
}

/// This computer as a host: "Allow remote control of this computer".
#[derive(uniffi::Object)]
pub struct ThisComputer {
    rt: tokio::runtime::Runtime,
    data_dir: PathBuf,
    /// The key and pairings: the app's store, else files in `data_dir`.
    store: Arc<dyn Store>,
    name: Mutex<String>,
    listener: Arc<dyn ThisComputerListener>,
    session: Mutex<Option<Session>>,
    /// The server the app configured; read by `start`.
    server: Mutex<Option<remote_core::rendezvous::RendezvousConfig>>,
}

impl ThisComputer {
    fn session(&self) -> Result<Session, ThisComputerError> {
        self.session.lock().unwrap().clone().ok_or(ThisComputerError::Off)
    }

    fn backend() -> Result<Arc<dyn Backend>, ThisComputerError> {
        #[cfg(target_os = "macos")]
        {
            Ok(Arc::new(crate::mac::MacBackend::new(None).map_err(failed)?))
        }
        // Elsewhere (the bindings' build, tests) the fake screen.
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Arc::new(crate::fake::FakeBackend::new(None, (1920, 1080), None)))
        }
    }
}

#[uniffi::export]
impl ThisComputer {
    /// The host, off. `data_dir`: its own folder (Application Support/Remote/Host);
    /// `name`: "Name shown to other computers" (empty: the Sharing name). `key_store`: where the device key
    /// and the pairings live (the Keychain; nil: files in `data_dir`).
    #[uniffi::constructor]
    pub fn new(
        data_dir: String,
        name: String,
        listener: Arc<dyn ThisComputerListener>,
        key_store: Option<Arc<dyn HostKeyStore>>,
    ) -> Result<Arc<ThisComputer>, ThisComputerError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("sodium-remote-host")
            .enable_all()
            .build()
            .map_err(failed)?;
        let data_dir = PathBuf::from(data_dir);
        std::fs::create_dir_all(&data_dir).map_err(failed)?;
        let store: Arc<dyn Store> = match key_store {
            Some(k) => Arc::new(HostStoreAdapter(k)),
            None => Arc::new(FileStore::new(data_dir.clone())),
        };
        Ok(Arc::new(ThisComputer { rt, data_dir, store, name: Mutex::new(name), listener, session: Mutex::new(None), server: Mutex::new(None) }))
    }

    /// "Allow remote control of this computer" on: listen on the LAN (port 47311, or any
    /// free one) and announce this Mac over mDNS. Nothing is captured until a paired
    /// device connects.
    pub async fn start(&self) -> Result<(), ThisComputerError> {
        if self.session.lock().unwrap().is_some() {
            return Ok(());
        }
        let name = self.name.lock().unwrap().clone();
        let o = Options {
            data: self.data_dir.clone(),
            store: Some(session::StoreRef(self.store.clone())),
            name: (!name.is_empty()).then_some(name),
            port: session::DEFAULT_PORT,
            pair: false,
            mdns: true,
            rendezvous: false,
            server: self.server.lock().unwrap().clone(),
            bind: None,
            secs: None,
        };
        let listener = self.listener.clone();
        let notes: session::Notes = Arc::new(move |n: Note| {
            let ev = match n {
                Note::Status(s) => ThisComputerEvent::State { state: state_of(&s) },
                Note::PairingOpen { code, expires_in } => ThisComputerEvent::PairingOpened { pairing: PairingCode { code, expires_in_secs: expires_in.as_secs() } },
                Note::PairingAttempt => ThisComputerEvent::PairingAttempt,
                Note::PairingFailed { attempts_left } => ThisComputerEvent::PairingFailed { attempts_left },
                Note::PairingClosed { reason } => ThisComputerEvent::PairingClosed { reason },
                Note::Paired(_) | Note::PairedChanged => ThisComputerEvent::DevicesChanged,
                // The Mac asks through Screen Recording and Accessibility, not per session.
                Note::Consent(_) | Note::MatchedDisplayFailed => return,
            };
            listener.on_event(ev);
        });
        let session = self
            .rt
            .spawn(async move {
                let backend = Self::backend()?;
                Session::start(backend, o, Some(notes)).await.map_err(|e| failed(format!("{e:#}")))
            })
            .await
            .map_err(failed)??;
        *self.session.lock().unwrap() = Some(session);
        Ok(())
    }

    /// The server this computer registers with (the app's Settings › Server): read at the next `start`. An empty
    /// `url` is none (LAN and address only); `ca_pem` and `token` may be empty.
    pub fn set_server(&self, url: String, ca_pem: String, token: String) {
        let cfg = (!url.is_empty()).then(|| remote_core::rendezvous::RendezvousConfig {
            url,
            ca_pem: (!ca_pem.is_empty()).then(|| ca_pem.into_bytes()),
            token: (!token.is_empty()).then_some(token),
        });
        *self.server.lock().unwrap() = cfg;
    }

    /// Off: the session (if any) ends, the listener closes, mDNS stops.
    pub async fn stop(&self) {
        let Some(s) = self.session.lock().unwrap().take() else { return };
        let _ = self.rt.spawn(async move { s.shutdown().await }).await;
        self.listener.on_event(ThisComputerEvent::State { state: ThisComputerState::Off });
    }

    /// The LAN port the host is listening on (the "IP:PORT" on the pane); nil while off.
    pub fn lan_port(&self) -> Option<u16> {
        self.session.lock().unwrap().as_ref().and_then(|s| s.lan_port())
    }

    pub fn is_on(&self) -> bool {
        self.session.lock().unwrap().is_some()
    }

    pub fn state(&self) -> ThisComputerState {
        self.session.lock().unwrap().as_ref().map_or(ThisComputerState::Off, |s| state_of(&s.status()))
    }

    /// This Mac's host ID (base64url), made on first use.
    pub fn device_id(&self) -> Result<String, ThisComputerError> {
        let key = remote_core::keys::load_or_create_device_key(self.store.as_ref()).map_err(failed)?;
        Ok(key.id().to_string())
    }

    /// The same as a short code, `XXXX-XXXX`, as devices show it.
    pub fn device_code(&self) -> Result<String, ThisComputerError> {
        let key = remote_core::keys::load_or_create_device_key(self.store.as_ref()).map_err(failed)?;
        Ok(key.id().code())
    }

    /// "Name shown to other computers" (empty: the Sharing name).
    pub fn name(&self) -> String {
        self.name.lock().unwrap().clone()
    }

    /// Renames this Mac for other computers: at once while on (mDNS, pairing, the next
    /// connection), else from the next start. The app keeps the setting.
    pub fn set_name(&self, name: String) {
        *self.name.lock().unwrap() = name.clone();
        if let Some(s) = self.session.lock().unwrap().as_ref() {
            let shown = if name.is_empty() { s.computer_name() } else { name };
            s.set_name(&shown);
        }
    }

    /// Open a pairing window: a new 6-digit code (any older window closes).
    pub async fn open_pairing(&self) -> Result<PairingCode, ThisComputerError> {
        let s = self.session()?;
        let w = self.rt.spawn(async move { s.open_pairing().await }).await.map_err(failed)?;
        let w = w.ok_or_else(|| failed("no pairing window"))?;
        Ok(PairingCode { code: w.code, expires_in_secs: w.expires_in.as_secs() })
    }

    pub fn close_pairing(&self) {
        if let Ok(s) = self.session() {
            s.close_pairing();
        }
    }

    /// The devices allowed to control this Mac, on or off.
    pub fn devices(&self) -> Vec<AllowedDevice> {
        let list = match self.session() {
            Ok(s) => s.paired(),
            Err(_) => session::stored_paired(&self.data_dir, self.store.as_ref()),
        };
        list.into_iter().map(AllowedDevice::from).collect()
    }

    /// Remove a device: it has to pair again; a session it has ends.
    pub async fn revoke(&self, id: String) -> Result<(), ThisComputerError> {
        let id: DeviceId = id.parse().map_err(|_| ThisComputerError::UnknownDevice)?;
        let removed = match self.session() {
            Ok(s) => self.rt.spawn(async move { s.revoke(id).await }).await.map_err(failed)?,
            Err(_) => {
                let r = session::stored_revoke(self.store.as_ref(), &id).map_err(failed)?;
                if r {
                    self.listener.on_event(ThisComputerEvent::DevicesChanged);
                }
                r
            }
        };
        if removed { Ok(()) } else { Err(ThisComputerError::UnknownDevice) }
    }

    /// "Disconnect": end the current session (the device may connect again).
    pub fn disconnect(&self) {
        if let Ok(s) = self.session() {
            s.disconnect();
        }
    }

    pub fn permissions(&self) -> HostPermissions {
        #[cfg(target_os = "macos")]
        {
            use crate::mac::permissions as p;
            HostPermissions { screen_recording: screen_recording_state(p::screen_recording()), accessibility: permission(p::accessibility()) }
        }
        #[cfg(not(target_os = "macos"))]
        HostPermissions { screen_recording: permission(true), accessibility: permission(true) }
    }

    /// Screen Recording's "Allow…": macOS's own prompt the first time, nothing after
    /// (then open System Settings). Not allowed yet, it answers `ReopenToFinish` from
    /// now on: an Allow works after a relaunch.
    pub fn request_screen_recording(&self) -> PermissionState {
        #[cfg(target_os = "macos")]
        {
            screen_recording_state(crate::mac::permissions::request_screen_recording())
        }
        #[cfg(not(target_os = "macos"))]
        permission(true)
    }

    /// Accessibility's "Allow…": macOS's dialog, which leads to System Settings.
    pub fn request_accessibility(&self) -> PermissionState {
        #[cfg(target_os = "macos")]
        {
            permission(crate::mac::permissions::request_accessibility())
        }
        #[cfg(not(target_os = "macos"))]
        permission(true)
    }
}

impl Drop for ThisComputer {
    /// Released while on: the session stops first (blocking, unless released inside a
    /// runtime, where dropping `rt` ends its tasks anyway).
    fn drop(&mut self) {
        if let Some(s) = self.session.get_mut().unwrap().take()
            && tokio::runtime::Handle::try_current().is_err()
        {
            self.rt.block_on(s.shutdown());
        }
    }
}
