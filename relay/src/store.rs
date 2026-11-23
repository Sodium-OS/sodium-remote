// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The little state that outlives a restart: registered devices and each host's
//! signed paired-client list. One JSON file, replaced atomically on every change.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::proto::{DeviceId, Role};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRec {
    pub role: Role,
    pub first_seen: u64,
    pub last_seen: u64,
    /// When a host sent its sleep notice; cleared when it authenticates again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asleep_since: Option<u64>,
    /// The account of the token it registered with, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// A host's paired-client list exactly as it signed it, plus the parsed fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedRec {
    pub seq: u64,
    /// base64url of the signed payload bytes.
    pub list: String,
    pub sig: String,
    pub clients: Vec<DeviceId>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StoreData {
    pub devices: BTreeMap<DeviceId, DeviceRec>,
    pub paired: BTreeMap<DeviceId, PairedRec>,
}

pub struct Store {
    path: PathBuf,
    pub data: StoreData,
}

impl Store {
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join("state.json");
        let data = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StoreData::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Self { path, data })
    }

    /// Writes the state to a temporary file, syncs it and renames it over the old one.
    pub fn save(&self) {
        if let Err(e) = self.try_save() {
            tracing::error!(error = %e, "saving state failed");
        }
    }

    fn try_save(&self) -> anyhow::Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec(&self.data)?)?;
        f.sync_all()?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}
