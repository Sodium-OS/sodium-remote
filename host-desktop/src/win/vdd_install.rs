// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Installing and removing the matched display's driver. Runs in an elevated copy of the
//! app (`SodiumRemote.exe --matched-display install|uninstall`, started with one UAC prompt
//! when the setting is turned on or off), never in the host itself.
//!
//! **Install**: every file in the driver folder ([`vdd_pin::DIR`]) must match its SHA-256 pin
//! first. Then our certificate goes into the local machine's Root and TrustedPublisher
//! stores (and nowhere else), the driver package is staged (`SetupCopyOEMInf`), the device
//! node is created the way Nefarius' nefconc does it (`SetupDiCreateDeviceInfo`, the
//! hardware ID, `DIF_REGISTERDEVICE`; this Windows' pnputil has no `/add-device`), and the
//! driver installed on it (`UpdateDriverForPlugAndPlayDevices`). What was made is written to
//! [`KEY`], so removal touches only that. A SudoVDA device that another program installed
//! (Apollo's, say) is left alone: installing stops instead.
//!
//! **Uninstall**: the device node removed (`DiUninstallDevice`), the staged package deleted
//! (`SetupUninstallOEMInf`), the certificate deleted from both stores by its thumbprint, the
//! key deleted. No test-signing mode, ever.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DICD_GENERATE_ID, DIF_REGISTERDEVICE, DIF_REMOVE, DiUninstallDevice, GUID_DEVCLASS_DISPLAY, HDEVINFO, INSTALLFLAG_FORCE, SP_COPY_STYLE,
    SP_DEVINFO_DATA, SPDRP_HARDWAREID, SPOST_PATH, SUOI_FORCEDELETE, SETUP_DI_GET_CLASS_DEVS_FLAGS, SetupCopyOEMInfW, SetupDiCallClassInstaller,
    SetupDiCreateDeviceInfoList, SetupDiCreateDeviceInfoW, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW, SetupDiSetDeviceRegistryPropertyW, SetupUninstallOEMInfW,
    UpdateDriverForPlugAndPlayDevicesW,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND};
use windows::Win32::Security::Cryptography::{
    CERT_FIND_SHA1_HASH, CERT_OPEN_STORE_FLAGS, CERT_QUERY_ENCODING_TYPE, CERT_STORE_ADD_REPLACE_EXISTING, CERT_STORE_PROV_SYSTEM_W,
    CERT_SYSTEM_STORE_LOCAL_MACHINE, CRYPT_INTEGER_BLOB, CertAddEncodedCertificateToStore, CertCloseStore, CertDeleteCertificateFromStore,
    CertFindCertificateInStore, CertOpenStore, HCERTSTORE, PKCS_7_ASN_ENCODING, X509_ASN_ENCODING,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegGetValueW, RegSetValueExW,
};
use windows::core::{PCWSTR, w};

use crate::vdd_pin;

/// What the installer made, under `HKEY_LOCAL_MACHINE`: values `Device` (the device
/// instance ID), `DriverInf` (the staged package's `oemNN.inf`) and `Certificate` (the
/// thumbprint, hex).
pub const KEY: &str = r"SOFTWARE\Sodium Remote\Matched display";

fn wstr(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The driver folder next to this program.
pub fn driver_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding this program")?;
    Ok(exe.parent().context("this program's folder")?.join(vdd_pin::DIR))
}

/// Every file is there and matches its pin.
fn check_files(dir: &Path) -> Result<[u8; 20]> {
    let (files, thumb) = vdd_pin::pinned().map_err(anyhow::Error::msg)?;
    for (name, want) in files {
        let path = dir.join(name);
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let got: [u8; 32] = Sha256::digest(&bytes).into();
        if got != want {
            bail!("{} doesn't match the pinned checksum (got {}, want {})", path.display(), hexs(&got), hexs(&want));
        }
    }
    Ok(thumb)
}

// ---- the registry key ------------------------------------------------------------------

fn reg_get(name: &str) -> Option<String> {
    let (key, name) = (wstr(KEY), wstr(name));
    let mut buf = vec![0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()), PCWSTR(name.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len))
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n])).filter(|s| !s.is_empty())
}

fn reg_set(values: &[(&str, &str)]) -> Result<()> {
    let key = wstr(KEY);
    let mut h = HKEY::default();
    let r = unsafe { RegCreateKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_READ | KEY_WRITE, None, &mut h, None) };
    if r != ERROR_SUCCESS {
        bail!("creating HKLM\\{KEY}: {}", r.0);
    }
    for (name, value) in values {
        let v = wstr(value);
        let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2) };
        let r = unsafe { RegSetValueExW(h, PCWSTR(wstr(name).as_ptr()), None, REG_SZ, Some(bytes)) };
        if r != ERROR_SUCCESS {
            let _ = unsafe { RegCloseKey(h) };
            bail!("writing {name} in HKLM\\{KEY}: {}", r.0);
        }
    }
    let _ = unsafe { RegCloseKey(h) };
    Ok(())
}

// ---- devices ---------------------------------------------------------------------------

/// The Display-class devices whose hardware IDs include ours, present or not: (instance ID, set, data).
struct Devices {
    set: HDEVINFO,
    found: Vec<(String, SP_DEVINFO_DATA)>,
}

impl Drop for Devices {
    fn drop(&mut self) {
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.set) };
    }
}

fn ours() -> Result<Devices> {
    let set = unsafe { SetupDiGetClassDevsW(Some(&GUID_DEVCLASS_DISPLAY), PCWSTR::null(), None, SETUP_DI_GET_CLASS_DEVS_FLAGS(0)) }.context("SetupDiGetClassDevs")?;
    let mut d = Devices { set, found: vec![] };
    let want = vdd_pin::HARDWARE_ID.to_ascii_lowercase();
    for i in 0.. {
        let mut dev = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
        if unsafe { SetupDiEnumDeviceInfo(set, i, &mut dev) }.is_err() {
            break;
        }
        let mut buf = vec![0u8; 2048];
        if unsafe { SetupDiGetDeviceRegistryPropertyW(set, &dev, SPDRP_HARDWAREID, None, Some(&mut buf), None) }.is_err() {
            continue;
        }
        let wide: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let matches = wide.split(|&c| c == 0).any(|id| String::from_utf16_lossy(id).to_ascii_lowercase() == want);
        if matches {
            let mut id = vec![0u16; 512];
            if unsafe { SetupDiGetDeviceInstanceIdW(set, &dev, Some(&mut id), None) }.is_ok() {
                let n = id.iter().position(|&c| c == 0).unwrap_or(id.len());
                d.found.push((String::from_utf16_lossy(&id[..n]), dev));
            }
        }
    }
    Ok(d)
}

/// A new root-enumerated Display device with our hardware ID; its instance ID.
fn create_device() -> Result<String> {
    unsafe {
        let set = SetupDiCreateDeviceInfoList(Some(&GUID_DEVCLASS_DISPLAY), None).context("SetupDiCreateDeviceInfoList")?;
        let r = (|| {
            let mut dev = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            SetupDiCreateDeviceInfoW(set, w!("Display"), &GUID_DEVCLASS_DISPLAY, PCWSTR::null(), None, DICD_GENERATE_ID, Some(&mut dev))
                .context("SetupDiCreateDeviceInfo")?;
            // REG_MULTI_SZ: the ID, then an empty string.
            let ids: Vec<u16> = vdd_pin::HARDWARE_ID.encode_utf16().chain([0, 0]).collect();
            let bytes = std::slice::from_raw_parts(ids.as_ptr() as *const u8, ids.len() * 2);
            SetupDiSetDeviceRegistryPropertyW(set, &mut dev, SPDRP_HARDWAREID, Some(bytes)).context("setting the hardware ID")?;
            SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set, Some(&dev)).context("registering the device")?;
            let mut id = vec![0u16; 512];
            let id = match SetupDiGetDeviceInstanceIdW(set, &dev, Some(&mut id), None) {
                Ok(()) => String::from_utf16_lossy(&id[..id.iter().position(|&c| c == 0).unwrap_or(id.len())]),
                Err(e) => {
                    let _ = SetupDiCallClassInstaller(DIF_REMOVE, set, Some(&dev));
                    return Err(e).context("reading the new device's ID");
                }
            };
            Ok(id)
        })();
        let _ = SetupDiDestroyDeviceInfoList(set);
        r
    }
}

fn remove_device(instance: &str) -> Result<()> {
    let d = ours()?;
    for (id, dev) in &d.found {
        if id.eq_ignore_ascii_case(instance) {
            unsafe { DiUninstallDevice(HWND::default(), d.set, dev, 0, None) }.with_context(|| format!("removing {id}"))?;
        }
    }
    Ok(())
}

// ---- the certificate -------------------------------------------------------------------

fn open_store(name: PCWSTR) -> Result<HCERTSTORE> {
    unsafe {
        CertOpenStore(CERT_STORE_PROV_SYSTEM_W, CERT_QUERY_ENCODING_TYPE(0), None, CERT_OPEN_STORE_FLAGS(CERT_SYSTEM_STORE_LOCAL_MACHINE), Some(name.as_ptr() as *const _))
    }
    .context("opening a local machine certificate store")
}

fn import_cert(der: &[u8]) -> Result<()> {
    for name in [w!("Root"), w!("TrustedPublisher")] {
        let store = open_store(name)?;
        let r = unsafe { CertAddEncodedCertificateToStore(Some(store), X509_ASN_ENCODING | PKCS_7_ASN_ENCODING, der, CERT_STORE_ADD_REPLACE_EXISTING, None) };
        let _ = unsafe { CertCloseStore(Some(store), 0) };
        r.context("importing the driver's certificate")?;
    }
    Ok(())
}

fn delete_cert(thumb: &[u8]) -> Result<()> {
    let mut failed = None;
    for name in [w!("Root"), w!("TrustedPublisher")] {
        let store = open_store(name)?;
        let blob = CRYPT_INTEGER_BLOB { cbData: thumb.len() as u32, pbData: thumb.as_ptr() as *mut u8 };
        loop {
            let c = unsafe { CertFindCertificateInStore(store, X509_ASN_ENCODING | PKCS_7_ASN_ENCODING, 0, CERT_FIND_SHA1_HASH, Some(&blob as *const _ as *const _), None) };
            if c.is_null() {
                break;
            }
            // Frees the context too.
            if let Err(e) = unsafe { CertDeleteCertificateFromStore(c) } {
                failed = Some(e);
                break;
            }
        }
        let _ = unsafe { CertCloseStore(Some(store), 0) };
    }
    match failed {
        Some(e) => Err(e).context("deleting the driver's certificate"),
        None => Ok(()),
    }
}

// ---- install, uninstall ----------------------------------------------------------------

/// Installs the driver from `dir` (the folder next to the program when None). Elevated.
pub fn install(dir: Option<&Path>) -> Result<()> {
    let dir = match dir {
        Some(d) => d.to_path_buf(),
        None => driver_dir()?,
    };
    let thumb = check_files(&dir)?;
    let mine = reg_get("Device");
    {
        let d = ours()?;
        if let Some((other, _)) = d.found.iter().find(|(id, _)| mine.as_deref().is_none_or(|m| !id.eq_ignore_ascii_case(m))) {
            bail!("another program's SudoVDA device is installed ({other}); not touching it");
        }
    }
    let der = std::fs::read(dir.join(vdd_pin::CER))?;
    import_cert(&der)?;
    let inf = dir.join(vdd_pin::INF);
    let inf_w = wstr(&inf.to_string_lossy());
    // Staged first, so its published name (oemNN.inf) is known for the uninstall.
    let mut oem = vec![0u16; 260];
    let mut part = windows::core::PWSTR::null();
    unsafe { SetupCopyOEMInfW(PCWSTR(inf_w.as_ptr()), PCWSTR::null(), SPOST_PATH, SP_COPY_STYLE(0), Some(&mut oem), None, Some(&mut part)) }
        .context("staging the driver package")?;
    let oem_name = if part.is_null() { String::new() } else { unsafe { part.to_string() }.unwrap_or_default() };
    let (device, created) = match mine.filter(|m| ours().is_ok_and(|d| d.found.iter().any(|(id, _)| id.eq_ignore_ascii_case(m)))) {
        Some(m) => (m, false),
        None => (create_device()?, true),
    };
    // Recorded before the driver goes on it, so a failure below can still be undone by turning the setting off.
    reg_set(&[("Device", &device), ("DriverInf", &oem_name), ("Certificate", &hexs(&thumb))])?;
    let hwid = wstr(vdd_pin::HARDWARE_ID);
    let mut reboot = windows::core::BOOL(0);
    if let Err(e) = unsafe { UpdateDriverForPlugAndPlayDevicesW(None, PCWSTR(hwid.as_ptr()), PCWSTR(inf_w.as_ptr()), INSTALLFLAG_FORCE, Some(&mut reboot)) } {
        if created {
            let _ = remove_device(&device);
        }
        return Err(e).context("installing the driver on the device");
    }
    // The control device answers once the driver has started.
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        match super::sudovda::SudoVda::open() {
            Ok(_) => break,
            Err(e) if Instant::now() >= until => return Err(e).context("the driver didn't start"),
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
    eprintln!("sodium-remote: the virtual display driver is installed ({device}, {oem_name}){}", if reboot.as_bool() { "; Windows asks for a restart" } else { "" });
    Ok(())
}

/// Removes what [`install`] made: the device, the staged package, the certificate, the key.
/// Every step is tried; the first failure is returned. Elevated.
pub fn uninstall() -> Result<()> {
    let mut first: Option<anyhow::Error> = None;
    let mut note = |r: Result<()>| {
        if let Err(e) = r {
            eprintln!("sodium-remote: {e:#}");
            first.get_or_insert(e);
        }
    };
    if let Some(device) = reg_get("Device") {
        note(remove_device(&device));
    }
    if let Some(oem) = reg_get("DriverInf") {
        let ok = unsafe { SetupUninstallOEMInfW(PCWSTR(wstr(&oem).as_ptr()), SUOI_FORCEDELETE, None) }.as_bool();
        note(if ok { Ok(()) } else { Err(anyhow::anyhow!("deleting the driver package {oem}: {}", std::io::Error::last_os_error())) });
    }
    // The thumbprint the install recorded, and this build's.
    let mut thumbs: Vec<Vec<u8>> = vec![];
    if let Some(t) = reg_get("Certificate") {
        thumbs.push((0..t.len() / 2).filter_map(|i| u8::from_str_radix(&t[2 * i..2 * i + 2], 16).ok()).collect());
    }
    if let Ok((_, t)) = vdd_pin::pinned() {
        thumbs.push(t.to_vec());
    }
    for t in thumbs.iter().filter(|t| t.len() == 20) {
        note(delete_cert(t));
    }
    let key = wstr(KEY);
    let r = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(key.as_ptr())) };
    if r != ERROR_SUCCESS && r.0 != 2 {
        note(Err(anyhow::anyhow!("deleting HKLM\\{KEY}: {}", r.0)));
    }
    match first {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Whether the driver is installed by us (the key names a device that exists).
pub fn installed() -> bool {
    reg_get("Device").is_some_and(|m| ours().is_ok_and(|d| d.found.iter().any(|(id, _)| id.eq_ignore_ascii_case(&m))))
}
