// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! `remote-rendezvous`: the rendezvous service for Remote.
//!
//! Devices authenticate with their Ed25519 keys; hosts keep a WebSocket open (presence)
//! and upload a signed list of the clients they've paired; clients ask for a host by ID
//! and exchange opaque, end-to-end-encrypted signalling with it; a host opens a 5-minute
//! pairing window that unpaired clients discover (the 6-digit code never reaches the
//! server) and that carries opaque SPAKE2 messages; and authorised devices get
//! short-lived coturn credentials. The API is in remote/docs/reference/rendezvous-api.md.

pub mod hooks;
pub mod hub;
pub mod limits;
pub mod proto;
pub mod server;
pub mod store;
pub mod tokens;
pub mod turn;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use hooks::{Denial, Hooks, NoHooks};
pub use hub::Hub;

/// Where the source of this server is offered (AGPL-3.0 section 13): `--version` and
/// the challenge's `source`. Mirrors `remote_core::brand::SOURCE_URL` (a test checks).
pub const SOURCE_URL: &str = "https://github.com/sodium-os/sodium-remote";
/// What `--version` prints after the name.
pub const VERSION_TEXT: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\nSodium Remote's rendezvous server, licensed AGPL-3.0-only.\nSource code: https://github.com/sodium-os/sodium-remote"
);

/// Who may register as a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Enrol {
    /// Any device may register as a host.
    Open,
    /// A new host registers with an enrolment token (`sodium-remote-server token new`);
    /// hosts on the allowlist, if one is given, need none.
    #[default]
    Token,
    /// Only hosts on the allowlist, checked at every authentication.
    Allowlist,
}

/// One relay of a region-labelled list handed out with TURN credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayRegion {
    pub region: String,
    pub urls: Vec<String>,
}

/// A message from the operator, given at `auth_ok` (`{code, text, url}`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Notice {
    pub code: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone)]
pub struct Config {
    /// The name devices bind their auth signature to (e.g. "relay.example.com"):
    /// the host name in the URL they connect to. Sent in the challenge.
    pub server_name: String,
    /// Other names devices may reach this server by (a cluster node's own name
    /// after a redirect, another DNS name for the same server). A signature
    /// bound to any of them, or to `server_name`, is accepted.
    pub server_aliases: Vec<String>,
    /// Where state.json lives.
    pub state_dir: PathBuf,
    /// Who may register as a host.
    pub enrol: Enrol,
    /// Host device IDs on the allowlist (one per line; re-read on SIGHUP).
    pub hosts_allow: Option<PathBuf>,
    /// coturn's `static-auth-secret`.
    pub turn_secret: Vec<u8>,
    pub turn_ttl: Duration,
    /// STUN/TURN URLs handed out with credentials.
    pub turn_urls: Vec<String>,
    /// Relays in several regions (optional): clients take the nearest by RTT.
    pub relays: Vec<RelayRegion>,
    /// Send new registrations to this server instead (`redirect`), e.g. a cluster's
    /// node. Clients follow it once, only to the configured host or a subdomain of it.
    pub redirect: Option<String>,
    /// The operator's message at `auth_ok`.
    pub notice: Option<Notice>,
    /// Proxies whose `X-Real-IP` header names the real client address. Empty: the header is ignored.
    pub trusted_proxies: Vec<std::net::IpAddr>,
    /// Quota and account hooks; `NoHooks` (everything allowed) by default.
    pub hooks: Arc<dyn Hooks>,
    pub pairing_ttl: Duration,
    /// How long a host that sent a sleep notice shows as asleep (24 h); then offline.
    pub asleep_ttl: Duration,
    /// A client that no host has paired and that has been idle this long is forgotten (30 days). Zero keeps them forever.
    pub client_idle_ttl: Duration,
}

impl Config {
    /// A configuration with the defaults: token enrolment, 10-minute TURN
    /// credentials, 5-minute pairing windows, 24 hours asleep, no hooks.
    pub fn new(server_name: impl Into<String>, state_dir: impl Into<PathBuf>, turn_secret: Vec<u8>) -> Config {
        Config {
            server_name: server_name.into(),
            server_aliases: vec![],
            state_dir: state_dir.into(),
            enrol: Enrol::Token,
            hosts_allow: None,
            turn_secret,
            turn_ttl: Duration::from_secs(600),
            turn_urls: vec![],
            relays: vec![],
            redirect: None,
            notice: None,
            trusted_proxies: vec![],
            hooks: Arc::new(NoHooks),
            pairing_ttl: Duration::from_secs(300),
            asleep_ttl: Duration::from_secs(24 * 3600),
            client_idle_ttl: Duration::from_secs(30 * 24 * 3600),
        }
    }

    /// Every name a device may have signed for: each configured name as given and
    /// as a URL's host reads (lowercase, no trailing dot).
    pub fn server_names(&self) -> impl Iterator<Item = String> + '_ {
        std::iter::once(&self.server_name)
            .chain(&self.server_aliases)
            .flat_map(|n| [n.clone(), n.trim_end_matches('.').to_ascii_lowercase()])
    }

    /// Where enrolment tokens are kept.
    pub fn tokens_dir(&self) -> PathBuf {
        self.state_dir.join("tokens")
    }
}
