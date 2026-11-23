// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Device identity and storage.
//!
//! - **The device key** is Ed25519. Its public half, base64url without padding,
//!   is the device ID everywhere: on the rendezvous, in pairing, in the paired
//!   lists.
//! - **The Noise static key** is the X25519 key derived from it (RFC 7748's
//!   birational map, libsodium's `ed25519_sk_to_curve25519`), so one pinned ID
//!   also pins the Noise IK handshake.
//! - **Storage** is a [`Store`] of named blobs: [`FileStore`] on hosts (the
//!   contract file `device.key`, PKCS#8 PEM, mode 0600), the Keychain on the Mac
//!   through FFI callbacks.

use crate::crypto::StaticKeypair;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey, spki::der::pem::LineEnding};
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;

/// The name of the device key in a [`Store`] (and the host's file name).
pub const DEVICE_KEY: &str = "device.key";
/// The paired devices and the paired-list sequence.
pub const PAIRED: &str = "paired.json";

/// A device ID: the Ed25519 public key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(pub [u8; 32]);

impl DeviceId {
    /// The X25519 key the device uses in Noise.
    pub fn noise_public(&self) -> Option<[u8; 32]> {
        Some(VerifyingKey::from_bytes(&self.0).ok()?.to_montgomery().to_bytes())
    }

    pub fn verify(&self, msg: &[u8], sig: &[u8]) -> bool {
        let (Ok(k), Ok(s)) = (VerifyingKey::from_bytes(&self.0), ed25519_dalek::Signature::from_slice(sig)) else {
            return false;
        };
        k.verify(msg, &s).is_ok()
    }

    /// The first 12 characters, for logs (as the rendezvous does).
    pub fn short(&self) -> String {
        self.to_string()[..12].to_string()
    }

    /// The code people compare by eye: the first 40 bits of SHA-256
    /// over the raw public key, as 8 Crockford base32 characters, `XXXX-XXXX`.
    /// A fingerprint only: copying always copies the full ID.
    pub fn code(&self) -> String {
        const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let h = Sha256::digest(self.0);
        let v = u64::from_be_bytes([0, 0, 0, h[0], h[1], h[2], h[3], h[4]]);
        let c: String = (0..8).map(|i| ALPHABET[((v >> (35 - 5 * i)) & 31) as usize] as char).collect();
        format!("{}-{}", &c[..4], &c[4..])
    }

    /// Does `typed` name this device's code? Case-insensitive, hyphen optional,
    /// with Crockford's reading of I/L as 1 and O as 0.
    pub fn code_matches(&self, typed: &str) -> bool {
        let norm = |s: &str| -> String {
            s.chars()
                .filter(|c| *c != '-' && !c.is_whitespace())
                .map(|c| match c.to_ascii_uppercase() {
                    'I' | 'L' => '1',
                    'O' => '0',
                    c => c,
                })
                .collect()
        };
        norm(typed) == norm(&self.code())
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&URL_SAFE_NO_PAD.encode(self.0))
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.short())
    }
}

impl FromStr for DeviceId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let b = URL_SAFE_NO_PAD.decode(s).map_err(|e| format!("not a device ID: {e}"))?;
        let a: [u8; 32] = b.try_into().map_err(|_| "a device ID is 32 bytes".to_string())?;
        VerifyingKey::from_bytes(&a).map_err(|_| "not an Ed25519 public key".to_string())?;
        Ok(DeviceId(a))
    }
}

impl Serialize for DeviceId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DeviceId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

/// The device's Ed25519 key.
#[derive(Clone)]
pub struct DeviceKey(SigningKey);

impl DeviceKey {
    pub fn generate() -> Self {
        DeviceKey(SigningKey::from_bytes(&rand::random::<[u8; 32]>()))
    }

    pub fn from_seed(seed: [u8; 32]) -> Self {
        DeviceKey(SigningKey::from_bytes(&seed))
    }

    /// PKCS#8 PEM (`-----BEGIN PRIVATE KEY-----`), as `openssl genpkey -algorithm ed25519` writes.
    pub fn from_pem(pem: &str) -> Result<Self, String> {
        SigningKey::from_pkcs8_pem(pem).map(DeviceKey).map_err(|e| format!("not an Ed25519 PKCS#8 PEM key: {e}"))
    }

    pub fn to_pem(&self) -> String {
        self.0.to_pkcs8_pem(LineEnding::LF).expect("PKCS#8 encoding").to_string()
    }

    pub fn id(&self) -> DeviceId {
        DeviceId(self.0.verifying_key().to_bytes())
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.0.sign(msg).to_bytes()
    }

    /// The X25519 key pair for Noise, derived from this key.
    pub fn noise_keypair(&self) -> StaticKeypair {
        StaticKeypair {
            private: self.0.to_scalar_bytes().to_vec(),
            public: self.0.verifying_key().to_montgomery().to_bytes(),
        }
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceKey({})", self.id().short())
    }
}

/// Named blobs: the device key and the paired devices.
pub trait Store: Send + Sync {
    fn load(&self, name: &str) -> Option<Vec<u8>>;
    fn save(&self, name: &str, data: &[u8]) -> io::Result<()>;
    fn remove(&self, name: &str) -> io::Result<()>;
}

/// A directory of files, each mode 0600, the directory 0700 (on Windows: a folder under the
/// per-user `%LOCALAPPDATA%\SodiumRemote`, protected by the profile's inherited ACL; no
/// explicit ACL).
pub struct FileStore {
    pub dir: PathBuf,
    /// Seals `device.key` at rest (DPAPI on Windows, none elsewhere): `keyprotect`.
    protector: Option<std::sync::Arc<dyn crate::keyprotect::Protector>>,
}

impl FileStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        FileStore { dir: dir.into(), protector: crate::keyprotect::platform() }
    }

    /// A store that seals `device.key` with `protector` (tests pass a fake; `new` picks the platform's).
    pub fn with_protector(dir: impl Into<PathBuf>, protector: Option<std::sync::Arc<dyn crate::keyprotect::Protector>>) -> Self {
        FileStore { dir: dir.into(), protector }
    }

    fn write(&self, name: &str, data: &[u8]) -> io::Result<()> {
        use std::io::Write;
        #[cfg(unix)]
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let mut db = std::fs::DirBuilder::new();
        db.recursive(true);
        #[cfg(unix)]
        db.mode(0o700);
        db.create(&self.dir)?;
        // Write a temporary file, then rename: a crash never leaves half a key.
        let tmp = self.dir.join(format!(".{name}.tmp"));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, self.dir.join(name))
    }

    /// The client half of an app that is both client and host: `<parent>/client`. Its sibling
    /// [`FileStore::host_in`] shares no file with it (each holds its own `device.key` and `paired.json`).
    pub fn client_in(parent: impl AsRef<std::path::Path>) -> Self {
        FileStore::new(parent.as_ref().join("client"))
    }

    /// The host half: `<parent>/host`.
    pub fn host_in(parent: impl AsRef<std::path::Path>) -> Self {
        FileStore::new(parent.as_ref().join("host"))
    }

    /// The host half of the data directory (`dirs`): `~/.local/share/sodium-remote/host`
    /// (copied there from the pre-rename `~/.local/share/sodiumos/remote` on first start)
    /// or `%LOCALAPPDATA%\SodiumRemote\host`. Its `device.key` is the host's identity at
    /// the rendezvous.
    pub fn host_default() -> io::Result<Self> {
        // Windows: the per-user %LOCALAPPDATA%. Its ACL is inherited from the user's
        // profile (the user, SYSTEM and administrators only), which stands in for
        // 0600/0700; the files get no explicit ACL of their own.
        Ok(FileStore::new(crate::dirs::data_dir_migrated(crate::dirs::Half::Host)?))
    }

    /// The client half: `$SODIUM_REMOTE_DATA` (or the old `$SODIUMOS_REMOTE_DATA`) when
    /// set, else `~/.local/share/sodium-remote/client` (copied from the pre-rename
    /// `~/.local/share/sodiumos/remote-client` on first start) or
    /// `%LOCALAPPDATA%\SodiumRemote\client`.
    pub fn client_default() -> io::Result<Self> {
        if let Some(d) = crate::dirs::data_override() {
            return Ok(FileStore::new(d));
        }
        Ok(FileStore::new(crate::dirs::data_dir_migrated(crate::dirs::Half::Client)?))
    }
}

impl Store for FileStore {
    fn load(&self, name: &str) -> Option<Vec<u8>> {
        use crate::keyprotect::{Kind, classify, wrap};
        let bytes = std::fs::read(self.dir.join(name)).ok()?;
        let Some(p) = self.protector.as_ref().filter(|_| name == DEVICE_KEY) else { return Some(bytes) };
        match classify(&bytes) {
            // A blob that does not open (another user, another computer) comes back as it is, so
            // the caller fails on it: never `None`, which would make a new key and lose the identity.
            Kind::Protected(blob) => Some(p.unprotect(blob).unwrap_or(bytes)),
            Kind::Plain => {
                // A key from before DPAPI: seal it now. The key, so the identity, is the same; if the
                // rewrite fails the plain file stays and the next start tries again.
                if let Err(e) = wrap(p.as_ref(), &bytes).and_then(|w| self.write(name, &w)) {
                    eprintln!("sodium-remote: could not protect {name}: {e}");
                }
                Some(bytes)
            }
        }
    }

    fn save(&self, name: &str, data: &[u8]) -> io::Result<()> {
        match self.protector.as_ref().filter(|_| name == DEVICE_KEY) {
            Some(p) => self.write(name, &crate::keyprotect::wrap(p.as_ref(), data)?),
            None => self.write(name, data),
        }
    }

    fn remove(&self, name: &str) -> io::Result<()> {
        match std::fs::remove_file(self.dir.join(name)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
}

/// An in-memory store, for tests and throwaway devices.
#[derive(Default)]
pub struct MemoryStore(Mutex<std::collections::HashMap<String, Vec<u8>>>);

impl Store for MemoryStore {
    fn load(&self, name: &str) -> Option<Vec<u8>> {
        self.0.lock().unwrap().get(name).cloned()
    }
    fn save(&self, name: &str, data: &[u8]) -> io::Result<()> {
        self.0.lock().unwrap().insert(name.into(), data.to_vec());
        Ok(())
    }
    fn remove(&self, name: &str) -> io::Result<()> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

/// The device key from the store, created (and saved) on first use.
pub fn load_or_create_device_key(store: &dyn Store) -> io::Result<DeviceKey> {
    if let Some(b) = store.load(DEVICE_KEY) {
        if matches!(crate::keyprotect::classify(&b), crate::keyprotect::Kind::Protected(_)) {
            return Err(io::Error::other("device.key is protected for another Windows user or computer"));
        }
        let pem = String::from_utf8(b).map_err(|_| io::Error::other("device.key is not PEM text"))?;
        return DeviceKey::from_pem(&pem).map_err(io::Error::other);
    }
    let key = DeviceKey::generate();
    store.save(DEVICE_KEY, key.to_pem().as_bytes())?;
    Ok(key)
}

/// A device on the other side of a pairing.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PairedDevice {
    pub id: DeviceId,
    pub name: String,
    /// Unix seconds.
    pub paired_at: u64,
    pub last_connected: Option<u64>,
    /// The desk renamed this device (`rename_paired`): its own name at `Hello` no longer replaces `name`.
    #[serde(default)]
    pub named_at_desk: bool,
    /// Clients: where this host's LAN listener was (`IP:PORT`) when it was paired over the
    /// LAN, so a bare-address connect finds its ID and a connect knows where to dial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan_addr: Option<String>,
}

/// What `paired.json` holds.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Pairings {
    /// Hosts: the `seq` of the last paired list uploaded (it must only grow).
    #[serde(default)]
    pub seq: u64,
    pub devices: Vec<PairedDevice>,
}

impl Pairings {
    pub fn load(store: &dyn Store) -> Pairings {
        store.load(PAIRED).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, store: &dyn Store) -> io::Result<()> {
        store.save(PAIRED, &serde_json::to_vec_pretty(self).expect("json"))
    }

    pub fn get(&self, id: &DeviceId) -> Option<&PairedDevice> {
        self.devices.iter().find(|d| d.id == *id)
    }

    /// Add or replace (re-pairing the same device keeps one entry).
    pub fn upsert(&mut self, dev: PairedDevice) {
        self.devices.retain(|d| d.id != dev.id);
        self.devices.push(dev);
    }

    /// Remember where `id`'s LAN listener is, and forget the same address on any other host
    /// (an address a router handed to a different computer since). True if anything changed.
    pub fn set_lan_addr(&mut self, id: &DeviceId, addr: &str) -> bool {
        let mut changed = false;
        for d in &mut self.devices {
            if d.id == *id {
                if d.lan_addr.as_deref() != Some(addr) {
                    d.lan_addr = Some(addr.to_string());
                    changed = true;
                }
            } else if d.lan_addr.as_deref() == Some(addr) {
                d.lan_addr = None;
                changed = true;
            }
        }
        changed
    }

    pub fn remove(&mut self, id: &DeviceId) -> bool {
        let n = self.devices.len();
        self.devices.retain(|d| d.id != *id);
        n != self.devices.len()
    }
}

/// Seal a blob to a pinned peer: Noise one-way `K` (both static keys known),
/// so only `peer` can open it and it can only have come from us. `context`
/// (the rendezvous session ID, or "lan") binds it to one signalling session.
pub fn seal(key: &DeviceKey, peer: &DeviceId, context: &[u8], plaintext: &[u8]) -> Option<Vec<u8>> {
    let mut hs = snow::Builder::new(SEAL_PARAMS.parse().ok()?)
        .prologue(&seal_prologue(context))
        .ok()?
        .local_private_key(&key.noise_keypair().private)
        .ok()?
        .remote_public_key(&peer.noise_public()?)
        .ok()?
        .build_initiator()
        .ok()?;
    let mut out = vec![0u8; plaintext.len() + 128];
    let n = hs.write_message(plaintext, &mut out).ok()?;
    out.truncate(n);
    Some(out)
}

/// Open a blob sealed by `peer` to us.
pub fn open(key: &DeviceKey, peer: &DeviceId, context: &[u8], blob: &[u8]) -> Option<Vec<u8>> {
    let mut hs = snow::Builder::new(SEAL_PARAMS.parse().ok()?)
        .prologue(&seal_prologue(context))
        .ok()?
        .local_private_key(&key.noise_keypair().private)
        .ok()?
        .remote_public_key(&peer.noise_public()?)
        .ok()?
        .build_responder()
        .ok()?;
    let mut out = vec![0u8; blob.len()];
    let n = hs.read_message(blob, &mut out).ok()?;
    out.truncate(n);
    Some(out)
}

const SEAL_PARAMS: &str = "Noise_K_25519_ChaChaPoly_BLAKE2s";

fn seal_prologue(context: &[u8]) -> Vec<u8> {
    let mut p = b"SodiumOS Remote signal v2\0".to_vec();
    p.extend_from_slice(context);
    p
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_is_40_bits_of_sha256_in_crockford_base32() {
        let id = DeviceId([7; 32]);
        assert_eq!(id.code(), "9ER6-Z3JE");
        assert!(id.code_matches("9er6z3je"));
        assert!(!id.code_matches("9ER6-Z3JF"));
    }

    #[test]
    fn a_client_store_and_a_host_store_in_one_parent_share_no_file() {
        let parent = std::env::temp_dir().join(format!("remote-stores-{}", std::process::id()));
        let (c, h) = (FileStore::client_in(&parent), FileStore::host_in(&parent));
        assert_ne!(c.dir, h.dir);
        let ck = load_or_create_device_key(&c).unwrap();
        let hk = load_or_create_device_key(&h).unwrap();
        c.save("paired.json", b"client").unwrap();
        assert_ne!(ck.id(), hk.id());
        assert!(h.load("paired.json").is_none());
        assert!(!c.dir.starts_with(&h.dir) && !h.dir.starts_with(&c.dir));
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn host_key_file_matches_the_contract() {
        let dir = std::env::temp_dir().join(format!("remote-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The PEM file contract (the Windows default store seals the file: keyprotect.rs).
        let store = FileStore::with_protector(&dir, None);
        let key = load_or_create_device_key(&store).unwrap();
        let path = dir.join(DEVICE_KEY);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let pem = std::fs::read_to_string(&path).unwrap();
        assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"));
        // The same key comes back, and its ID round-trips.
        assert_eq!(load_or_create_device_key(&store).unwrap().id(), key.id());
        assert_eq!(key.id().to_string().len(), 43);
        assert_eq!(key.id().to_string().parse::<DeviceId>().unwrap(), key.id());
        // openssl reads it and derives the same ID (deploy.sh host-id does this).
        if let Ok(out) = std::process::Command::new("openssl")
            .args(["pkey", "-pubout", "-outform", "DER", "-in"])
            .arg(&path)
            .output()
        {
            if out.status.success() {
                assert_eq!(&out.stdout[out.stdout.len() - 32..], &key.id().0);
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_plain_key_is_sealed_on_first_load_and_keeps_its_identity() {
        use crate::keyprotect::{MAGIC, tests::Fake};
        let dir = std::env::temp_dir().join(format!("remote-keyprotect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // An earlier build's plain key.
        let key = DeviceKey::generate();
        FileStore::with_protector(&dir, None).save(DEVICE_KEY, key.to_pem().as_bytes()).unwrap();
        let sealed = FileStore::with_protector(&dir, Some(std::sync::Arc::new(Fake)));
        assert_eq!(load_or_create_device_key(&sealed).unwrap().id(), key.id());
        let on_disk = std::fs::read(dir.join(DEVICE_KEY)).unwrap();
        assert!(on_disk.starts_with(MAGIC) && !String::from_utf8_lossy(&on_disk).contains("BEGIN"));
        assert!(!dir.join(".device.key.tmp").exists());
        // Next start: read through the protector, same identity, file untouched.
        assert_eq!(load_or_create_device_key(&sealed).unwrap().id(), key.id());
        assert_eq!(std::fs::read(dir.join(DEVICE_KEY)).unwrap(), on_disk);
        // A new key is saved sealed; other files stay plain.
        let fresh = FileStore::with_protector(dir.join("b"), Some(std::sync::Arc::new(Fake)));
        let k2 = load_or_create_device_key(&fresh).unwrap();
        assert!(std::fs::read(dir.join("b").join(DEVICE_KEY)).unwrap().starts_with(MAGIC));
        fresh.save(PAIRED, b"{}").unwrap();
        assert_eq!(std::fs::read(dir.join("b").join(PAIRED)).unwrap(), b"{}");
        assert_eq!(load_or_create_device_key(&fresh).unwrap().id(), k2.id());
        // Without a protector (or for another user) a sealed file is an error, never a new key.
        assert!(load_or_create_device_key(&FileStore::with_protector(&dir, None)).is_err());
        assert_eq!(std::fs::read(dir.join(DEVICE_KEY)).unwrap(), on_disk);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sealed_blobs_open_only_for_the_peer_in_the_same_session() {
        let (a, b, c) = (DeviceKey::generate(), DeviceKey::generate(), DeviceKey::generate());
        let blob = seal(&a, &b.id(), b"s1", b"offer").unwrap();
        assert_eq!(open(&b, &a.id(), b"s1", &blob).as_deref(), Some(&b"offer"[..]));
        assert_eq!(open(&b, &a.id(), b"s2", &blob), None, "another session");
        assert_eq!(open(&c, &a.id(), b"s1", &blob), None, "another recipient");
        assert_eq!(open(&b, &c.id(), b"s1", &blob), None, "another sender");
    }

    #[test]
    fn the_noise_key_follows_from_the_device_id() {
        let key = DeviceKey::generate();
        let kp = key.noise_keypair();
        assert_eq!(key.id().noise_public(), Some(kp.public));
        // seal/open (ss DH) only work if the private half matches the public
        // key a peer derives from the ID: covered by the sealed-blob test.
    }
}

#[cfg(test)]
mod lan_addr_tests {
    use super::*;

    fn dev(n: u8, addr: Option<&str>) -> PairedDevice {
        PairedDevice { id: DeviceId([n; 32]), name: n.to_string(), paired_at: 1, last_connected: None, named_at_desk: false, lan_addr: addr.map(str::to_string) }
    }

    #[test]
    fn a_lan_address_moves_to_the_host_now_using_it() {
        let mut p = Pairings::default();
        p.upsert(dev(1, Some("192.0.2.5:47311")));
        p.upsert(dev(2, None));
        assert!(p.set_lan_addr(&DeviceId([2; 32]), "192.0.2.5:47311"));
        assert_eq!(p.get(&DeviceId([1; 32])).unwrap().lan_addr, None);
        assert_eq!(p.get(&DeviceId([2; 32])).unwrap().lan_addr.as_deref(), Some("192.0.2.5:47311"));
        assert!(!p.set_lan_addr(&DeviceId([2; 32]), "192.0.2.5:47311"), "nothing to change");
    }
}
