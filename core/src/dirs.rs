// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Where Sodium Remote keeps its files, and the first-start
//! migration from the directories used before the rename.
//!
//! | | Linux | Windows |
//! |---|---|---|
//! | Data (host half) | `$XDG_DATA_HOME/sodium-remote/host` | `%LOCALAPPDATA%\SodiumRemote\host` |
//! | Data (client half) | `$XDG_DATA_HOME/sodium-remote/client` | `%LOCALAPPDATA%\SodiumRemote\client` |
//! | Configuration | `$XDG_CONFIG_HOME/sodium-remote/`, then `/etc/sodium-remote/` | `%APPDATA%\SodiumRemote\` |
//!
//! `$XDG_DATA_HOME` defaults to `~/.local/share` and `$XDG_CONFIG_HOME` to
//! `~/.config`. The Mac app keeps its data and preferences in its container and
//! passes them in; other Unix builds use the Linux layout.
//!
//! **Migration:** before the rename the host kept `~/.local/share/sodiumos/remote`
//! and the app `~/.local/share/sodiumos/remote-client`. When the new directory
//! doesn't exist and the old one does, the old one's files are copied across
//! (`device.key` and `paired.json` among them, so pairings survive). The old
//! directory is left as it was, so an older build still running on the machine
//! keeps working until it is replaced; a `MIGRATED` text file in it names the new one.

use crate::brand;
use std::io;
use std::path::{Path, PathBuf};

/// Which half of the data directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Half {
    Host,
    Client,
}

impl Half {
    fn subdir(self) -> &'static str {
        match self {
            Half::Host => brand::HOST_SUBDIR,
            Half::Client => brand::CLIENT_SUBDIR,
        }
    }

    #[cfg(not(windows))]
    fn old_subdir(self) -> &'static str {
        match self {
            Half::Host => brand::OLD_LINUX_HOST_DIR,
            Half::Client => brand::OLD_LINUX_CLIENT_DIR,
        }
    }
}

#[cfg(not(windows))]
fn home() -> io::Result<PathBuf> {
    std::env::var_os("HOME").filter(|v| !v.is_empty()).map(PathBuf::from).ok_or_else(|| io::Error::other("HOME is not set"))
}

#[cfg(not(windows))]
fn xdg(var: &str, fallback: &str) -> io::Result<PathBuf> {
    match std::env::var_os(var).filter(|v| !v.is_empty()) {
        Some(d) => Ok(PathBuf::from(d)),
        None => Ok(home()?.join(fallback)),
    }
}

#[cfg(windows)]
fn known(var: &str) -> io::Result<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).ok_or_else(|| io::Error::other(format!("{var} is not set")))
}

/// The base of both data halves: `~/.local/share/sodium-remote` or
/// `%LOCALAPPDATA%\SodiumRemote`.
pub fn data_base() -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(known("LOCALAPPDATA")?.join(brand::WINDOWS_DIR_NAME))
    }
    #[cfg(not(windows))]
    {
        Ok(xdg("XDG_DATA_HOME", ".local/share")?.join(brand::LINUX_DIR_NAME))
    }
}

/// One half of the data directory, without migrating anything.
pub fn data_dir(half: Half) -> io::Result<PathBuf> {
    Ok(data_base()?.join(half.subdir()))
}

/// One half of the data directory, after the first-start migration from the
/// pre-rename directory (Linux; nothing to migrate on Windows).
pub fn data_dir_migrated(half: Half) -> io::Result<PathBuf> {
    let new = data_dir(half)?;
    #[cfg(not(windows))]
    {
        let old = xdg("XDG_DATA_HOME", ".local/share")?.join(half.old_subdir());
        match migrate(&old, &new) {
            Ok(true) => tracing::info!("copied {} to {}", old.display(), new.display()),
            Ok(false) => {}
            Err(e) => tracing::warn!("couldn't copy {} to {}: {e}", old.display(), new.display()),
        }
    }
    Ok(new)
}

/// The app's data directory as an override says: `$SODIUM_REMOTE_DATA`, else the
/// old `$SODIUMOS_REMOTE_DATA`. `None`: use [`data_dir_migrated`].
pub fn data_override() -> Option<PathBuf> {
    [brand::ENV_DATA, brand::OLD_ENV_DATA]
        .iter()
        .find_map(|v| std::env::var_os(v).filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

/// Copies `old` to `new` when `new` doesn't exist and `old` is a directory.
/// Returns whether it copied. Files keep their permissions; `new` is made 0700.
/// A half-done copy is never left as `new`: it is built beside it and renamed.
pub fn migrate(old: &Path, new: &Path) -> io::Result<bool> {
    if new.exists() || !old.is_dir() {
        return Ok(false);
    }
    let parent = new.parent().ok_or_else(|| io::Error::other("no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let name = new.file_name().and_then(|n| n.to_str()).unwrap_or("data");
    let tmp = parent.join(format!(".{name}.migrating"));
    let _ = std::fs::remove_dir_all(&tmp);
    copy_dir(old, &tmp)?;
    std::fs::rename(&tmp, new)?;
    // A note in the old directory, for anyone who finds it; nothing deletes it.
    let _ = std::fs::write(old.join("MIGRATED"), format!("Copied to {}\n", new.display()));
    Ok(true)
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    let mut db = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        db.mode(0o700);
    }
    db.create(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else if ty.is_file() {
            // std::fs::copy keeps the permission bits (0600 for the key).
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

/// The configuration directories to look in, first match wins: the user's, then
/// the system's (Linux); `%APPDATA%\SodiumRemote` (Windows).
pub fn config_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(windows)]
    {
        if let Ok(d) = known("APPDATA") {
            v.push(d.join(brand::WINDOWS_DIR_NAME));
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(d) = xdg("XDG_CONFIG_HOME", ".config") {
            v.push(d.join(brand::LINUX_DIR_NAME));
        }
        v.push(PathBuf::from(brand::LINUX_SYSTEM_CONFIG_DIR));
    }
    v
}

/// The user's own configuration directory (where an imported link is saved).
pub fn user_config_dir() -> Option<PathBuf> {
    config_dirs().into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("remote-dirs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn migration_copies_the_key_and_pairings_once_and_keeps_the_old_dir() {
        let root = scratch("migrate");
        let old = root.join("sodiumos/remote");
        let new = root.join("sodium-remote/host");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("device.key"), b"KEY").unwrap();
        std::fs::write(old.join("paired.json"), b"{\"seq\":3}").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(old.join("device.key"), std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        assert!(migrate(&old, &new).unwrap());
        assert_eq!(std::fs::read(new.join("device.key")).unwrap(), b"KEY");
        assert_eq!(std::fs::read(new.join("paired.json")).unwrap(), b"{\"seq\":3}");
        assert!(old.join("device.key").exists(), "the old directory stays for older builds");
        assert!(std::fs::read_to_string(old.join("MIGRATED")).unwrap().contains(new.to_str().unwrap()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(new.join("device.key")).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&new).unwrap().permissions().mode() & 0o777, 0o700);
        }

        // Never again once the new directory exists: it is the one in use.
        std::fs::write(old.join("device.key"), b"OTHER").unwrap();
        assert!(!migrate(&old, &new).unwrap());
        assert_eq!(std::fs::read(new.join("device.key")).unwrap(), b"KEY");
        // Nothing to do without an old directory.
        assert!(!migrate(&root.join("absent"), &root.join("fresh")).unwrap());
        assert!(!root.join("fresh").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
