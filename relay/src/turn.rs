// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Short-lived TURN credentials in coturn's `use-auth-secret` (TURN REST API) form:
//! username `<expiry unix time>:<device ID>`, password base64(HMAC-SHA1(secret, username)).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::proto::DeviceId;

pub fn credentials(secret: &[u8], device: &DeviceId, now: u64, ttl: u64) -> (String, String) {
    let username = format!("{}:{}", now + ttl, device);
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC takes any key length");
    mac.update(username.as_bytes());
    let password = STANDARD.encode(mac.finalize().into_bytes());
    (username, password)
}
