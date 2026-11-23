// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The host's identity and the devices it trusts.
//!
//! - `device.key`: the host's Ed25519 key, PKCS#8 PEM (v1, exactly what
//!   `openssl genpkey -algorithm ed25519` writes), mode 0600, in the data dir. The
//!   relay's `deploy.sh host-id` / `allow-host` read this file; the device ID is the public key in base64url without padding.
//! - `paired.json`: the paired devices (their pinned public keys are their IDs), and
//!   the paired-list sequence number the rendezvous requires to grow across restarts.
//!   The layout is remote-core's `keys::Pairings` (`seq`, and `id`, `name`,
//!   `paired_at`, `last_connected` per device), so core-connect's `FileStore` and
//!   this module read each other's file; `kind` is an extra field core ignores.
//!
//! The data dir is the host half of core's `dirs`: `~/.local/share/sodium-remote/host/`
//! (core's `FileStore::host_default`, and where `deploy.sh host-id` looks), copied from
//! the pre-rename `~/.local/share/sodiumos/remote/` on first start;
//! `$REMOTED_DATA_DIR` overrides it for tests.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{
    Signer, SigningKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey, KeypairBytes, spki::der::pem::LineEnding},
};
use serde::{Deserialize, Serialize};

pub const KEY_FILE: &str = "device.key";
pub const PAIRED_FILE: &str = "paired.json";

/// Where remoted keeps what must survive a reboot.
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("REMOTED_DATA_DIR") {
        return d.into();
    }
    remote_core::dirs::data_dir_migrated(remote_core::dirs::Half::Host)
        .unwrap_or_else(|_| PathBuf::from("/").join(remote_core::brand::LINUX_DIR_NAME).join(remote_core::brand::HOST_SUBDIR))
}

fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Writes `bytes` to `path` atomically (temp file, fsync, rename) with `mode`.
pub fn write_private(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))?;
    Ok(())
}

/// A device ID: an Ed25519 public key, 32 bytes, base64url without padding (43 characters).
pub fn device_id(public: &[u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(public)
}

/// Checks the shape of a device ID (43 base64url characters that decode to 32 bytes).
pub fn valid_device_id(id: &str) -> bool {
    id.len() == 43 && URL_SAFE_NO_PAD.decode(id).is_ok_and(|b| b.len() == 32)
}

pub struct DeviceKey {
    key: SigningKey,
}

impl DeviceKey {
    /// Reads `device.key` from `dir`, or makes one on first start.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        let path = dir.join(KEY_FILE);
        match fs::read_to_string(&path) {
            Ok(pem) => {
                let mode = fs::metadata(&path)?.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    bail!("{} is readable by others (mode {mode:o}); fix it with chmod 600", path.display());
                }
                let key = SigningKey::from_pkcs8_pem(&pem).with_context(|| format!("reading {}", path.display()))?;
                Ok(DeviceKey { key })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                ensure_dir(dir)?;
                let mut secret = [0u8; 32];
                rustix::rand::getrandom(&mut secret, rustix::rand::GetRandomFlags::empty())?;
                let key = SigningKey::from_bytes(&secret);
                // PKCS#8 v1 (no public key inside), as openssl writes it.
                let pem = KeypairBytes { secret_key: secret, public_key: None }.to_pkcs8_pem(LineEnding::LF)?;
                secret.fill(0);
                write_private(&path, pem.as_bytes(), 0o600)?;
                Ok(DeviceKey { key })
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn id(&self) -> String {
        device_id(self.key.verifying_key().as_bytes())
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.key.sign(msg).to_bytes()
    }

    /// For core's Noise handshake and the rendezvous client.
    #[allow(dead_code)]
    pub fn signing_key(&self) -> &SigningKey {
        &self.key
    }

    /// The rendezvous' signed paired-client list:
    /// the payload bytes and the signature over the domain-separated payload.
    pub fn sign_paired_list(&self, seq: u64, clients: &[String]) -> (Vec<u8>, [u8; 64]) {
        let payload = serde_json::to_vec(&serde_json::json!({
            "v": 1, "host": self.id(), "seq": seq, "clients": clients,
        }))
        .expect("plain JSON");
        let mut msg = b"remote-rendezvous/v1/paired-list\0".to_vec();
        msg.extend_from_slice(&payload);
        (payload, self.sign(&msg))
    }
}

/// One paired device. Its ID is its pinned public key.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    /// "mac", "windows", "linux", "web": what the device said it is when pairing.
    #[serde(default = "mac")]
    pub kind: String,
    /// Unix seconds.
    pub paired_at: u64,
    /// Unix seconds of the last connection; None = never.
    #[serde(default)]
    pub last_connected: Option<u64>,
    /// The desk renamed this device: core keeps the name over the device's own at `Hello`.
    #[serde(default)]
    pub named_at_desk: bool,
}

fn mac() -> String {
    "mac".into()
}

#[derive(Default, Serialize, Deserialize)]
struct PairedFile {
    /// The last paired-list `seq` uploaded; it must only grow.
    #[serde(default)]
    seq: u64,
    devices: Vec<PairedDevice>,
}

/// The paired devices, persisted in `paired.json` after every change.
pub struct PairedStore {
    path: PathBuf,
    file: PairedFile,
}

impl PairedStore {
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join(PAIRED_FILE);
        let file = match fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).with_context(|| format!("parsing {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => PairedFile::default(),
            Err(e) => return Err(e.into()),
        };
        Ok(PairedStore { path, file })
    }

    fn save(&self) -> Result<()> {
        ensure_dir(self.path.parent().unwrap())?;
        write_private(&self.path, &serde_json::to_vec_pretty(&self.file)?, 0o600)
    }

    pub fn devices(&self) -> &[PairedDevice] {
        &self.file.devices
    }

    pub fn get(&self, id: &str) -> Option<&PairedDevice> {
        self.file.devices.iter().find(|d| d.id == id)
    }

    /// Adds (or re-pins) a device. Returns the next list seq to upload.
    pub fn add(&mut self, dev: PairedDevice) -> Result<u64> {
        if !valid_device_id(&dev.id) {
            bail!("not a device ID: {}", dev.id);
        }
        self.file.devices.retain(|d| d.id != dev.id);
        self.file.devices.push(dev);
        self.bump()
    }

    /// Removes a device; Ok(None) if it wasn't paired, else the next list seq.
    pub fn remove(&mut self, id: &str) -> Result<Option<u64>> {
        let before = self.file.devices.len();
        self.file.devices.retain(|d| d.id != id);
        if self.file.devices.len() == before {
            return Ok(None);
        }
        self.bump().map(Some)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<bool> {
        let name = name.trim();
        if name.len() > 64 {
            bail!("a device name is 1 to 64 bytes");
        }
        let Some(d) = self.file.devices.iter_mut().find(|d| d.id == id) else { return Ok(false) };
        if name.is_empty() {
            // "" follows the device's own name again; the current one stays until it connects.
            d.named_at_desk = false;
        } else {
            d.name = name.to_string();
            d.named_at_desk = true;
        }
        self.save()?;
        Ok(true)
    }

    pub fn touch(&mut self, id: &str, now: u64) -> Result<()> {
        if let Some(d) = self.file.devices.iter_mut().find(|d| d.id == id) {
            d.last_connected = Some(now);
            self.save()?;
        }
        Ok(())
    }

    fn bump(&mut self) -> Result<u64> {
        // Never below the wall clock either, so a lost paired.json can't replay an old seq.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.file.seq = (self.file.seq + 1).max(now);
        self.save()?;
        Ok(self.file.seq)
    }

    pub fn list_seq(&self) -> u64 {
        self.file.seq
    }

    pub fn client_ids(&self) -> Vec<String> {
        self.file.devices.iter().map(|d| d.id.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("remoted-keys-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn key_is_created_once_and_read_back() {
        let dir = tmpdir("key");
        let a = DeviceKey::load_or_create(&dir).unwrap();
        let path = dir.join(KEY_FILE);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let pem = fs::read_to_string(&path).unwrap();
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----\n"));
        let b = DeviceKey::load_or_create(&dir).unwrap();
        assert_eq!(a.id(), b.id());
        assert!(valid_device_id(&a.id()));
        // A key file others can read is refused, not silently used.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(DeviceKey::load_or_create(&dir).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn paired_list_signature_verifies_and_seq_grows() {
        use ed25519_dalek::{Signature, Verifier};
        let dir = tmpdir("paired");
        let key = DeviceKey::load_or_create(&dir).unwrap();
        let mut store = PairedStore::open(&dir).unwrap();
        let id = device_id(&[7; 32]);
        let s1 = store
            .add(PairedDevice { id: id.clone(), name: "MacBook Air".into(), kind: "mac".into(), paired_at: 1, last_connected: None, named_at_desk: false })
            .unwrap();
        let s2 = store.remove(&id).unwrap().unwrap();
        assert!(s2 > s1);
        assert_eq!(PairedStore::open(&dir).unwrap().list_seq(), s2);
        let (payload, sig) = key.sign_paired_list(s2, &[id.clone()]);
        let mut msg = b"remote-rendezvous/v1/paired-list\0".to_vec();
        msg.extend_from_slice(&payload);
        key.signing_key().verifying_key().verify(&msg, &Signature::from_bytes(&sig)).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(v["host"], key.id());
        assert_eq!(v["clients"][0], id);
        fs::remove_dir_all(&dir).unwrap();
    }
}
