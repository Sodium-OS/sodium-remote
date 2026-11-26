// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! `settings.json` in the host's data folder: the app's host settings ("software video",
//! "Match the display to the connecting computer"), one JSON object. Each writer changes
//! only its own key.

use std::path::Path;

use serde_json::{Value, json};

/// The whole object (empty when the file is missing or isn't an object).
pub fn read(data: &Path) -> Value {
    std::fs::read(data.join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}))
}

/// Sets one key, keeping the others (written to a temporary file, then renamed).
pub fn write(data: &Path, key: &str, value: Value) {
    let mut s = read(data);
    s[key] = value;
    let _ = std::fs::create_dir_all(data);
    let tmp = data.join(".settings.json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(&s).expect("json")).and_then(|_| std::fs::rename(&tmp, data.join("settings.json"))).is_err() {
        eprintln!("sodium-remote-host: could not write {}", data.join("settings.json").display());
    }
}
