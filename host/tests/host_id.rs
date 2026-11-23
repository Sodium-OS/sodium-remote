// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! `deploy.sh host-id` prints the same ID and short code as `remoted id` for one key
//! (the code is the first 40 bits of SHA-256 over the raw public key, Crockford base32:
//! `DeviceId::code()` in remote-core, redone in bash).

use std::process::Command;

#[test]
fn deploy_sh_host_id_matches_remoted_id() {
    let dir = std::env::temp_dir().join(format!("remoted-host-id-{}", std::process::id()));
    let remoted = Command::new(env!("CARGO_BIN_EXE_remoted"))
        .arg("id")
        .env("REMOTED_DATA_DIR", &dir)
        .output()
        .expect("remoted id");
    assert!(remoted.status.success(), "{}", String::from_utf8_lossy(&remoted.stderr));
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../relay/deploy/deploy.sh");
    let deploy = Command::new("bash")
        .args([script, "host-id"])
        .env("SODIUM_REMOTE_HOST_KEY", dir.join("device.key"))
        .output()
        .expect("deploy.sh host-id");
    assert!(deploy.status.success(), "{}", String::from_utf8_lossy(&deploy.stderr));
    let (a, b) = (String::from_utf8(remoted.stdout).unwrap(), String::from_utf8(deploy.stdout).unwrap());
    assert_eq!(a.lines().count(), 2, "{a:?}");
    assert_eq!(a, b);
    std::fs::remove_dir_all(&dir).ok();
}
