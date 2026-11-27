// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Wire format of the rendezvous API.
//!
//! Every frame is one JSON object in a WebSocket text message, with its kind in `"t"`.
//! Binary values travel as base64url without padding. The server never decodes the
//! meaning of `blob` fields: signalling and pairing payloads are opaque to it. The
//! 6-digit pairing code never reaches the server in any form.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const PROTOCOL_VERSION: u32 = 1;

/// Domain-separation prefixes for everything a device signs.
pub const AUTH_CONTEXT: &[u8] = b"remote-rendezvous/v1/auth\0";
pub const PAIRED_LIST_CONTEXT: &[u8] = b"remote-rendezvous/v1/paired-list\0";

/// Limits the server enforces (documented in the API doc).
pub const MAX_FRAME: usize = 64 * 1024;
pub const MAX_SIGNAL_BLOB: usize = 16 * 1024;
pub const MAX_PAIR_BLOB: usize = 4 * 1024;
/// Pairing messages per window, all attempts and both directions together.
pub const MAX_PAIR_MSGS: u32 = 32;
/// SPAKE2 attempts (joins) per pairing window; the host also counts failures itself.
pub const MAX_PAIR_ATTEMPTS: u32 = 3;
pub const MAX_PAIR_NAME: usize = 64;
pub const MAX_DISCOVERED: usize = 8;
pub const MAX_PAIRED_CLIENTS: usize = 64;
pub const MAX_PAIRED_LIST_BYTES: usize = 8 * 1024;
pub const MAX_PRESENCE_QUERY: usize = 32;
pub const MAX_SESSIONS_PER_CLIENT: usize = 8;
pub const MAX_PAIRINGS_PER_HOST: usize = 1;

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn unb64(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s).ok()
}

/// The bytes a device signs to answer the server's challenge.
pub fn auth_message(server: &str, role: Role, nonce: &[u8]) -> Vec<u8> {
    let mut m = AUTH_CONTEXT.to_vec();
    m.extend_from_slice(server.as_bytes());
    m.push(0);
    m.extend_from_slice(role.as_str().as_bytes());
    m.push(0);
    m.extend_from_slice(nonce);
    m
}

/// The bytes a host signs for its paired-client list (`payload` is the exact JSON it uploads).
pub fn paired_list_message(payload: &[u8]) -> Vec<u8> {
    let mut m = PAIRED_LIST_CONTEXT.to_vec();
    m.extend_from_slice(payload);
    m
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Host,
    Client,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Host => "host",
            Role::Client => "client",
        }
    }
}

/// A device's ID is its Ed25519 public key (32 bytes, base64url, 43 characters).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(pub [u8; 32]);

impl DeviceId {
    pub fn parse(s: &str) -> Option<Self> {
        if s.len() != 43 {
            return None;
        }
        let bytes: [u8; 32] = unb64(s)?.try_into().ok()?;
        VerifyingKey::from_bytes(&bytes).ok()?;
        Some(Self(bytes))
    }

    /// Strict Ed25519 verification of `sig` (base64url, 64 bytes) over `msg`.
    pub fn verify(&self, msg: &[u8], sig: &str) -> bool {
        let Some(sig) = unb64(sig).and_then(|b| <[u8; 64]>::try_from(b).ok()) else {
            return false;
        };
        let Ok(key) = VerifyingKey::from_bytes(&self.0) else {
            return false;
        };
        key.verify_strict(msg, &Signature::from_bytes(&sig)).is_ok()
    }

    /// A short form for logs.
    pub fn short(&self) -> String {
        self.to_string()[..12].to_string()
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&b64(&self.0))
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.short())
    }
}

impl Serialize for DeviceId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DeviceId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        DeviceId::parse(&s).ok_or_else(|| serde::de::Error::custom("not a device ID"))
    }
}

/// The first frame a device sends, answering `challenge`.
#[derive(Debug, Deserialize)]
pub struct Auth {
    pub t: String,
    pub device: String,
    pub role: Role,
    pub sig: String,
    /// An enrolment token, used only when this authentication registers the device.
    #[serde(default)]
    pub token: Option<String>,
}

/// A request after authentication. Every request carries a numeric `id`, which the
/// reply echoes; it is read separately so it isn't part of this enum.
#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Request {
    PairedList { list: String, sig: String },
    GetPairedList {},
    Presence { hosts: Vec<String> },
    Connect {
        host: String,
        #[serde(default)]
        node: Option<u16>,
    },
    Signal { session: String, blob: String },
    Close { session: String },
    Routes { session: String },
    PairOpen { name: String },
    PairDiscover {},
    PairJoin { pairing: String },
    PairMsg { pairing: String, blob: String },
    PairFail { pairing: String },
    PairClose { pairing: String },
    TurnCredentials {},
    Unregister {},
    Sleep {},
}

impl Request {
    pub fn name(&self) -> &'static str {
        match self {
            Request::PairedList { .. } => "paired_list",
            Request::GetPairedList {} => "get_paired_list",
            Request::Presence { .. } => "presence",
            Request::Connect { .. } => "connect",
            Request::Signal { .. } => "signal",
            Request::Close { .. } => "close",
            Request::Routes { .. } => "routes",
            Request::PairOpen { .. } => "pair_open",
            Request::PairDiscover {} => "pair_discover",
            Request::PairJoin { .. } => "pair_join",
            Request::PairMsg { .. } => "pair_msg",
            Request::PairFail { .. } => "pair_fail",
            Request::PairClose { .. } => "pair_close",
            Request::TurnCredentials {} => "turn_credentials",
            Request::Unregister {} => "unregister",
            Request::Sleep {} => "sleep",
        }
    }
}

/// The signed payload of a host's paired-client list. v2 names the server it is
/// for (the host name the host connected to), so it can't be replayed at another.
#[derive(Debug, Serialize, Deserialize)]
pub struct PairedListPayload {
    pub v: u32,
    pub host: DeviceId,
    #[serde(default)]
    pub server: String,
    pub seq: u64,
    pub clients: Vec<DeviceId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostState {
    Online,
    /// Sent a sleep notice before disconnecting, less than 24 hours ago.
    Asleep,
    Offline,
    NotAuthorised,
}

/// Frames the server sends without being asked.
#[derive(Debug, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Event {
    /// `source`: where this server's source code is offered (AGPL-3.0 section 13).
    Challenge { v: u32, server: String, nonce: String, source: &'static str },
    AuthOk {
        device: DeviceId,
        role: Role,
        registered: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        notice: Option<crate::Notice>,
        #[serde(skip_serializing_if = "Option::is_none")]
        service: Option<serde_json::Value>,
    },
    /// Instead of `auth_ok`, for a registration: register at this server instead.
    Redirect { url: String },
    Incoming {
        session: String,
        client: DeviceId,
        #[serde(skip_serializing_if = "Option::is_none")]
        routes: Option<serde_json::Value>,
    },
    Signal { session: String, blob: String },
    SessionClosed { session: String, reason: &'static str },
    PairJoined { pairing: String, attempt: u32 },
    PairMsg { pairing: String, blob: String },
    PairLeft { pairing: String },
    PairClosed { pairing: String, reason: &'static str },
    Presence { host: DeviceId, state: HostState, last_seen: Option<u64>, since: Option<u64> },
    Revoked { host: DeviceId },
    Replaced {},
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    BadRequest,
    AuthFailed,
    HostNotAllowed,
    RoleMismatch,
    ServerFull,
    Forbidden,
    NotAuthorised,
    HostOffline,
    NotFound,
    RateLimited,
    StaleSeq,
    BadSignature,
    TooLarge,
    TooMany,
    TokenInvalid,
    TokenExpired,
    QuotaExceeded,
    PaymentRequired,
    AccountRequired,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::AuthFailed => "auth_failed",
            ErrorCode::HostNotAllowed => "host_not_allowed",
            ErrorCode::RoleMismatch => "role_mismatch",
            ErrorCode::ServerFull => "server_full",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::NotAuthorised => "not_authorised",
            ErrorCode::HostOffline => "host_offline",
            ErrorCode::NotFound => "not_found",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::StaleSeq => "stale_seq",
            ErrorCode::BadSignature => "bad_signature",
            ErrorCode::TooLarge => "too_large",
            ErrorCode::TooMany => "too_many",
            ErrorCode::TokenInvalid => "token_invalid",
            ErrorCode::TokenExpired => "token_expired",
            ErrorCode::QuotaExceeded => "quota_exceeded",
            ErrorCode::PaymentRequired => "payment_required",
            ErrorCode::AccountRequired => "account_required",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: &'static str,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }
}

/// `{"t":"result","id":N, ...fields}`.
pub fn result_frame(id: u64, fields: serde_json::Value) -> String {
    let mut obj = match fields {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    obj.insert("t".into(), "result".into());
    obj.insert("id".into(), id.into());
    serde_json::Value::Object(obj).to_string()
}

/// `{"t":"error","id":N|null,"code":"...","message":"..."}`.
pub fn error_frame(id: Option<u64>, err: &ApiError) -> String {
    serde_json::json!({
        "t": "error",
        "id": id,
        "code": err.code.as_str(),
        "message": err.message,
    })
    .to_string()
}
