// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Short-lived TURN credentials in coturn's `use-auth-secret` (TURN REST API) form:
//! username `<expiry unix time>:<device ID>`, password base64(HMAC-SHA1(secret, username)).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use sha1::Sha1;

use crate::proto::DeviceId;

/// The credentials, and when they expire (unix time).
///
/// The expiry is rounded to steps of half the TTL, so a device asking again
/// within the same step gets the same username back. A device then holds at
/// most two live usernames, which keeps coturn's per-user quota meaningful.
/// The credentials stay valid for more than half and at most all of `ttl`.
pub fn credentials(secret: &[u8], device: &DeviceId, now: u64, ttl: u64) -> (String, String, u64) {
    let step = (ttl / 2).max(1);
    let expires = (now / step + 2) * step;
    let username = format!("{}:{}", expires, device);
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC takes any key length");
    mac.update(username.as_bytes());
    let password = STANDARD.encode(mac.finalize().into_bytes());
    (username, password, expires)
}
