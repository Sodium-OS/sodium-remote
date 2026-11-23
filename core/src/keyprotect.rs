// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Protecting the device key at rest.
//!
//! On Windows `FileStore` writes `device.key` through DPAPI (`CryptProtectData`, current-user
//! scope): the file holds [`MAGIC`] followed by the DPAPI blob, so only the same Windows user on the
//! same machine can read the key back. A plain PKCS#8 PEM key from an earlier build is read, then
//! rewritten protected (temp file and rename), and the identity does not change.
//!
//! The mechanism is a [`Protector`], so the logic (magic, migration, failure handling) is the same
//! everywhere and tests use a fake one. Linux and the Mac pass none: nothing changes there
//! (the Mac's Keychain is its own store).

use std::io;

/// Starts every protected `device.key`. A PEM key starts with `-----BEGIN`, never with this.
pub const MAGIC: &[u8] = b"SODIUM-REMOTE-DPAPI-1\n";

/// Seals and opens a blob for this user on this machine.
pub trait Protector: Send + Sync {
    fn protect(&self, plain: &[u8]) -> io::Result<Vec<u8>>;
    fn unprotect(&self, blob: &[u8]) -> io::Result<Vec<u8>>;
}

/// What a file holds, by its first bytes.
#[derive(Debug, PartialEq, Eq)]
pub enum Kind<'a> {
    /// `MAGIC`, then this protected blob.
    Protected(&'a [u8]),
    /// Anything else (a plain PEM).
    Plain,
}

pub fn classify(bytes: &[u8]) -> Kind<'_> {
    match bytes.strip_prefix(MAGIC) {
        Some(blob) => Kind::Protected(blob),
        None => Kind::Plain,
    }
}

/// `MAGIC` + the protected blob.
pub fn wrap(p: &dyn Protector, plain: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&p.protect(plain)?);
    Ok(out)
}

/// The platform's protector: DPAPI on Windows, none elsewhere.
pub fn platform() -> Option<std::sync::Arc<dyn Protector>> {
    #[cfg(windows)]
    {
        Some(std::sync::Arc::new(Dpapi))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Windows DPAPI, current-user scope, no UI. A fixed entropy string ties the blob to this
/// application, so another program of the same user does not open it by accident.
#[cfg(windows)]
pub struct Dpapi;

#[cfg(windows)]
const ENTROPY: &[u8] = b"Sodium Remote device key";

#[cfg(windows)]
impl Protector for Dpapi {
    fn protect(&self, plain: &[u8]) -> io::Result<Vec<u8>> {
        use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData};
        let input = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
        let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe { CryptProtectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }
            .map_err(|e| io::Error::other(format!("DPAPI CryptProtectData: {e}")))?;
        Ok(take(out))
    }

    fn unprotect(&self, blob: &[u8]) -> io::Result<Vec<u8>> {
        use windows::Win32::Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData};
        let input = CRYPT_INTEGER_BLOB { cbData: blob.len() as u32, pbData: blob.as_ptr() as *mut u8 };
        let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe { CryptUnprotectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out) }
            .map_err(|e| io::Error::other(format!("DPAPI CryptUnprotectData: {e}")))?;
        Ok(take(out))
    }
}

/// Copies a DPAPI-allocated blob out, wipes it and frees it.
#[cfg(windows)]
fn take(blob: windows::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB) -> Vec<u8> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    let slice = unsafe { std::slice::from_raw_parts_mut(blob.pbData, blob.cbData as usize) };
    let v = slice.to_vec();
    slice.fill(0);
    unsafe { LocalFree(Some(HLOCAL(blob.pbData as *mut _))) };
    v
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A stand-in that is not DPAPI: reversible, and it refuses an empty blob.
    pub struct Fake;
    impl Protector for Fake {
        fn protect(&self, plain: &[u8]) -> io::Result<Vec<u8>> {
            Ok(plain.iter().rev().map(|b| b ^ 0x5a).collect())
        }
        fn unprotect(&self, blob: &[u8]) -> io::Result<Vec<u8>> {
            if blob.is_empty() {
                return Err(io::Error::other("not ours"));
            }
            Ok(blob.iter().rev().map(|b| b ^ 0x5a).collect())
        }
    }

    /// The real DPAPI (runs under wine or on Windows only).
    #[cfg(windows)]
    #[test]
    fn dpapi_round_trips_and_hides_the_key() {
        let key = crate::keys::DeviceKey::generate();
        let dir = std::env::temp_dir().join(format!("remote-dpapi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let plain = crate::keys::FileStore::with_protector(&dir, None);
        crate::keys::Store::save(&plain, crate::keys::DEVICE_KEY, key.to_pem().as_bytes()).unwrap();
        // The default store seals the old plain key at its first load, and the ID stays.
        let store = crate::keys::FileStore::new(&dir);
        assert_eq!(crate::keys::load_or_create_device_key(&store).unwrap().id(), key.id());
        let on_disk = std::fs::read(dir.join(crate::keys::DEVICE_KEY)).unwrap();
        assert!(on_disk.starts_with(MAGIC) && !String::from_utf8_lossy(&on_disk).contains("PRIVATE KEY"));
        assert_eq!(crate::keys::load_or_create_device_key(&store).unwrap().id(), key.id());
        assert!(crate::keys::load_or_create_device_key(&plain).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn magic_is_recognised_and_a_pem_is_not() {
        let w = wrap(&Fake, b"secret").unwrap();
        assert!(matches!(classify(&w), Kind::Protected(b) if Fake.unprotect(b).unwrap() == b"secret"));
        assert_eq!(classify(b"-----BEGIN PRIVATE KEY-----\n"), Kind::Plain);
        assert_eq!(classify(b""), Kind::Plain);
    }
}
