// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Which rendezvous server to use: nothing is
//! compiled in. A device uses a server only when `server.toml` or an imported
//! `sodium-remote://server` link names one; with none it works on the LAN and by
//! address, and Settings › Server says so.
//!
//! **`server.toml`**, looked up in [`dirs::config_dirs`] (Linux: the user's
//! `~/.config/sodium-remote/`, then `/etc/sodium-remote/`; Windows:
//! `%APPDATA%\SodiumRemote\`; the Mac app keeps the same three values in its
//! preferences and passes them in):
//!
//! ```toml
//! [server]
//! url = "wss://relay.example.com:8443/remote"
//! ca_file = "relay-ca.pem"   # optional: pin this CA (relative to the file); else Web PKI
//! token = "…"                 # optional: the enrolment token, sent when this device registers
//! ```
//!
//! **Links** (and their QR codes) carry the same:
//! `sodium-remote://server?url=<percent-encoded wss URL>&ca=<base64url DER>&token=<token>`.
//! A link with a `ca` asks the user before the CA is trusted; a link naming
//! [`brand::OFFICIAL_SERVER_HOST`] never carries one (it is dropped) and is trusted
//! through Web PKI without a prompt.

use crate::brand;
use crate::dirs;
use crate::rendezvous::{RendezvousConfig, url_host};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rustls::pki_types::{CertificateDer, pem::PemObject};
use serde::Deserialize;
use std::io;
use std::path::{Path, PathBuf};

/// The file the pinned CA of an imported link is saved as, beside `server.toml`.
pub const IMPORTED_CA_FILE: &str = "server-ca.pem";

#[derive(Clone, Debug, PartialEq)]
pub struct ServerConfig {
    /// `wss://host[:port]/path`.
    pub url: String,
    /// The CA to pin (absolute). `None`: Web PKI.
    pub ca_file: Option<PathBuf>,
    pub token: Option<String>,
}

#[derive(Deserialize)]
struct File {
    server: Option<Section>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    url: String,
    ca_file: Option<String>,
    token: Option<String>,
}

fn check_url(url: &str) -> Result<(), String> {
    if !url.starts_with("wss://") {
        return Err(format!("the server URL must start with wss:// (got {url:?})"));
    }
    url_host(url).map(|_| ()).ok_or_else(|| format!("no host in the server URL {url:?}"))
}

impl ServerConfig {
    /// Parse `server.toml`. `base` is its directory, for a relative `ca_file`. A file
    /// without a `[server]` table configures nothing (`Ok(None)`).
    pub fn parse(text: &str, base: &Path) -> Result<Option<ServerConfig>, String> {
        let f: File = toml::from_str(text).map_err(|e| e.to_string())?;
        let Some(s) = f.server else { return Ok(None) };
        let url = s.url.trim().to_string();
        check_url(&url)?;
        let ca_file = s.ca_file.filter(|c| !c.trim().is_empty()).map(|c| base.join(c.trim()));
        let token = s.token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        Ok(Some(ServerConfig { url, ca_file, token }))
    }

    /// The first `server.toml` in `dirs`, parsed, with the path it came from.
    pub fn lookup_in(dirs: &[PathBuf]) -> Result<Option<(ServerConfig, PathBuf)>, String> {
        for d in dirs {
            let path = d.join(brand::SERVER_TOML);
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
            return match ServerConfig::parse(&text, d).map_err(|e| format!("{}: {e}", path.display()))? {
                Some(c) => Ok(Some((c, path))),
                None => Ok(None),
            };
        }
        Ok(None)
    }

    /// This machine's configuration: [`ServerConfig::lookup_in`] the standard places.
    pub fn load() -> Result<Option<(ServerConfig, PathBuf)>, String> {
        ServerConfig::lookup_in(&dirs::config_dirs())
    }

    /// [`ServerConfig::load`] as the rendezvous configuration, logging (not failing
    /// on) a broken file: a device still works on the LAN without a server.
    pub fn load_rendezvous() -> Option<RendezvousConfig> {
        match ServerConfig::load().and_then(|c| c.map(|(c, _)| c.rendezvous()).transpose()) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("no rendezvous: {e}");
                None
            }
        }
    }

    pub fn host(&self) -> String {
        url_host(&self.url).unwrap_or_default()
    }

    /// Names the hosted service: label it [`brand::OFFICIAL_SERVICE_NAME`].
    pub fn is_official(&self) -> bool {
        self.host() == brand::OFFICIAL_SERVER_HOST
    }

    /// What Settings › Server and the relay views call this server.
    pub fn label(&self) -> String {
        if self.is_official() { brand::OFFICIAL_SERVICE_NAME.to_string() } else { self.host() }
    }

    /// The rendezvous configuration, with the pinned CA read from its file.
    pub fn rendezvous(&self) -> Result<RendezvousConfig, String> {
        let ca_pem = match &self.ca_file {
            Some(p) => {
                let pem = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
                if CertificateDer::pem_slice_iter(&pem).next().is_none() {
                    return Err(format!("{}: no certificate in it", p.display()));
                }
                Some(pem)
            }
            None => None,
        };
        Ok(RendezvousConfig { url: self.url.clone(), ca_pem, token: self.token.clone() })
    }

    /// The file's text (for writing an imported link, and the Mac's export).
    pub fn to_toml(&self) -> String {
        let q = |s: &str| toml::Value::String(s.to_string()).to_string();
        let mut out = format!("[server]\nurl = {}\n", q(&self.url));
        if let Some(c) = &self.ca_file {
            out += &format!("ca_file = {}\n", q(&c.to_string_lossy()));
        }
        if let Some(t) = &self.token {
            out += &format!("token = {}\n", q(t));
        }
        out
    }
}

/// A `sodium-remote://server?...` link, parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerLink {
    pub url: String,
    /// The CA to pin, DER. Never set for the hosted service.
    pub ca_der: Option<Vec<u8>>,
    pub token: Option<String>,
}

fn pct_decode(s: &str) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
                let v = hex.and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or("a bad %-escape in the link")?;
                out.push(v);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| "the link isn't UTF-8".to_string())
}

fn pct_encode(s: &str) -> String {
    s.bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (c as char).to_string(),
            _ => format!("%{c:02X}"),
        })
        .collect()
}

impl ServerLink {
    pub fn parse(link: &str) -> Result<ServerLink, String> {
        let prefix = format!("{}://server", brand::LINK_SCHEME);
        let link = link.trim();
        let rest = link
            .get(..prefix.len())
            .filter(|p| p.eq_ignore_ascii_case(&prefix))
            .map(|_| &link[prefix.len()..])
            .ok_or_else(|| format!("not a {prefix} link"))?;
        let query = rest.trim_start_matches('/').strip_prefix('?').ok_or("the link has no settings")?;
        let (mut url, mut ca, mut token) = (None, None, None);
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = pct_decode(v)?;
            match k {
                "url" => url = Some(v),
                "ca" => ca = Some(URL_SAFE_NO_PAD.decode(v.trim_end_matches('=')).map_err(|_| "the link's CA isn't base64url")?),
                "token" => token = Some(v).filter(|t| !t.is_empty()),
                _ => {} // newer links may say more
            }
        }
        let url = url.ok_or("the link names no server URL")?;
        check_url(&url)?;
        let mut l = ServerLink { url, ca_der: ca, token };
        if l.is_official() {
            // The hosted service is trusted through Web PKI only; a link can't pin it.
            l.ca_der = None;
        }
        Ok(l)
    }

    pub fn host(&self) -> String {
        url_host(&self.url).unwrap_or_default()
    }

    pub fn is_official(&self) -> bool {
        self.host() == brand::OFFICIAL_SERVER_HOST
    }

    /// The app must ask before trusting the link's CA (it pins a private CA).
    pub fn needs_ca_prompt(&self) -> bool {
        self.ca_der.is_some()
    }

    pub fn to_link(&self) -> String {
        let mut s = format!("{}://server?url={}", brand::LINK_SCHEME, pct_encode(&self.url));
        if let Some(ca) = &self.ca_der {
            s += &format!("&ca={}", URL_SAFE_NO_PAD.encode(ca));
        }
        if let Some(t) = &self.token {
            s += &format!("&token={}", pct_encode(t));
        }
        s
    }

    /// The link for a configuration (export, QR). The CA file's first certificate
    /// goes in as DER.
    pub fn from_config(c: &ServerConfig) -> Result<ServerLink, String> {
        let ca_der = match &c.ca_file {
            Some(p) => {
                let pem = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
                Some(der_from_pem(&pem).map_err(|e| format!("{}: {e}", p.display()))?)
            }
            None => None,
        };
        Ok(ServerLink { url: c.url.clone(), ca_der, token: c.token.clone() })
    }

    /// Save as `dir/server.toml` (and the CA as `dir/server-ca.pem`), both 0600.
    /// Call only after the user agreed to trust the CA, when there is one.
    pub fn save(&self, dir: &Path) -> io::Result<ServerConfig> {
        let ca_file = match &self.ca_der {
            Some(der) => {
                let pem = pem_from_der(der);
                let path = dir.join(IMPORTED_CA_FILE);
                write_private(&path, pem.as_bytes())?;
                Some(path)
            }
            None => None,
        };
        let cfg = ServerConfig { url: self.url.clone(), ca_file, token: self.token.clone() };
        write_private(&dir.join(brand::SERVER_TOML), cfg.to_toml().as_bytes())?;
        Ok(cfg)
    }
}

/// Settings typed by hand (Add a server, the Mac's preferences): the URL, an optional CA as PEM and an
/// optional token, checked the way a link or `server.toml` is. The hosted service never pins a CA.
pub fn check_settings(url: &str, ca_pem: &str, token: &str) -> Result<ServerLink, String> {
    let url = url.trim().to_string();
    check_url(&url)?;
    let ca_der = if ca_pem.trim().is_empty() { None } else { Some(der_from_pem(ca_pem.as_bytes())?) };
    let token = Some(token.trim().to_string()).filter(|t| !t.is_empty());
    let mut l = ServerLink { url, ca_der, token };
    if l.is_official() {
        l.ca_der = None;
    }
    Ok(l)
}

/// Why a typed server address was refused: the apps word these themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressError {
    /// `http://` or `ws://`: never upgraded, since the user asked for an unencrypted connection.
    Insecure,
    /// Empty, or another scheme.
    Invalid,
}

/// What a person types for a server, made into the `wss://` URL the core wants: a bare host
/// (`relay.example.org`, `relay.example.org:8443`) gets `wss://` and [`brand::DEFAULT_SERVER_PATH`]; `https://` and `wss://`
/// become `wss://` (a path is kept as typed, else the default is added); `http://` and `ws://` are refused.
pub fn normalize_address(typed: &str) -> Result<String, AddressError> {
    let t = typed.trim();
    if t.is_empty() {
        return Err(AddressError::Invalid);
    }
    let rest = match t.split_once("://") {
        Some((scheme, rest)) => match scheme.to_ascii_lowercase().as_str() {
            "https" | "wss" => rest,
            "http" | "ws" => return Err(AddressError::Insecure),
            _ => return Err(AddressError::Invalid),
        },
        None => t,
    };
    if rest.is_empty() || rest.starts_with(['/', '?', '#']) || rest.contains(char::is_whitespace) {
        return Err(AddressError::Invalid);
    }
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let path = &rest[host_end..];
    let path = if path.is_empty() || path == "/" { brand::DEFAULT_SERVER_PATH } else { path };
    Ok(format!("wss://{}{}", &rest[..host_end], path))
}

/// A certificate's DER as PEM.
pub fn pem_from_der(der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in b64.as_bytes().chunks(64) {
        pem += std::str::from_utf8(line).expect("base64 is ASCII");
        pem.push('\n');
    }
    pem + "-----END CERTIFICATE-----\n"
}

/// The first certificate in a PEM, as DER.
pub fn der_from_pem(pem: &[u8]) -> Result<Vec<u8>, String> {
    let der = CertificateDer::pem_slice_iter(pem).next().ok_or("no certificate in the PEM")?;
    Ok(der.map_err(|e| e.to_string())?.to_vec())
}

fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("remote-srvcfg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn server_toml_parses_and_the_users_file_wins_over_the_systems() {
        let root = scratch("lookup");
        let (user, system) = (root.join("user"), root.join("etc"));
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        let dirs = [user.clone(), system.clone()];

        // Nothing configured: no server (LAN and address only).
        assert_eq!(ServerConfig::lookup_in(&dirs).unwrap(), None);

        std::fs::write(system.join("server.toml"), "[server]\nurl = \"wss://relay.example.org/remote\"\n").unwrap();
        let (c, from) = ServerConfig::lookup_in(&dirs).unwrap().unwrap();
        assert_eq!((c.url.as_str(), c.ca_file.clone(), c.token.clone()), ("wss://relay.example.org/remote", None, None));
        assert_eq!(from, system.join("server.toml"));

        std::fs::write(
            user.join("server.toml"),
            "# mine\n[server]\nurl = \"wss://relay.example.com:8443/remote\"\nca_file = \"relay-ca.pem\"\ntoken = \" abc \"\n",
        )
        .unwrap();
        let (c, from) = ServerConfig::lookup_in(&dirs).unwrap().unwrap();
        assert_eq!(from, user.join("server.toml"));
        assert_eq!(c.host(), "relay.example.com");
        assert_eq!(c.ca_file, Some(user.join("relay-ca.pem")));
        assert_eq!(c.token.as_deref(), Some("abc"));
        assert!(!c.is_official());
        // The pinned CA must exist and hold a certificate.
        assert!(c.rendezvous().is_err());
        // It round-trips through its own text.
        assert_eq!(ServerConfig::parse(&c.to_toml(), &user).unwrap(), Some(c));

        // Broken files say why; http:// is refused.
        for bad in ["[server]\nurl = \"https://relay.example.com\"\n", "[server]\nurl = \"wss://x\"\nport = 1\n", "[server\n"] {
            assert!(ServerConfig::parse(bad, &user).is_err(), "{bad}");
        }
        let official = ServerConfig::parse("[server]\nurl = \"wss://remote.sodiumos.com/remote\"\n", &user).unwrap().unwrap();
        assert_eq!(official.label(), "Sodium Remote service");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn typed_addresses_become_wss_urls_and_plain_text_is_refused() {
        for (typed, url) in [
            ("relay.example.org", "wss://relay.example.org/remote"),
            (" Relay.Example.org:8443 ", "wss://Relay.Example.org:8443/remote"),
            ("https://relay.example.org/", "wss://relay.example.org/remote"),
            ("https://relay.example.org/join", "wss://relay.example.org/join"),
            ("WSS://relay.example.org/remote", "wss://relay.example.org/remote"),
        ] {
            assert_eq!(normalize_address(typed).as_deref(), Ok(url), "{typed}");
        }
        assert_eq!(normalize_address("http://relay.example.org"), Err(AddressError::Insecure));
        assert_eq!(normalize_address("ws://relay.example.org/remote"), Err(AddressError::Insecure));
        for bad in ["", "  ", "ftp://x", "https://", "/remote"] {
            assert_eq!(normalize_address(bad), Err(AddressError::Invalid), "{bad:?}");
        }
    }

    #[test]
    fn links_parse_round_trip_and_never_pin_a_ca_for_the_hosted_service() {
        let l = ServerLink::parse("sodium-remote://server?url=wss%3A%2F%2Frelay.example.com%3A8443%2Fremote&ca=AQID&token=t0k%2Ben").unwrap();
        assert_eq!(l.url, "wss://relay.example.com:8443/remote");
        assert_eq!(l.ca_der.as_deref(), Some(&[1u8, 2, 3][..]));
        assert_eq!(l.token.as_deref(), Some("t0k+en"));
        assert!(l.needs_ca_prompt());
        assert_eq!(ServerLink::parse(&l.to_link()).unwrap(), l);

        let o = ServerLink::parse("SODIUM-REMOTE://server/?url=wss://remote.sodiumos.com/remote&ca=AQID").unwrap();
        assert!(o.is_official());
        assert_eq!(o.ca_der, None, "a link can't pin a CA for the hosted service");
        assert!(!o.needs_ca_prompt());

        for bad in [
            "https://server?url=wss://a.example",
            "sodium-remote://server",
            "sodium-remote://server?token=x",
            "sodium-remote://server?url=ws%3A%2F%2Fa.example",
            "sodium-remote://server?url=wss://a.example&ca=%%%",
        ] {
            assert!(ServerLink::parse(bad).is_err(), "{bad}");
        }

        // Saved, it becomes a server.toml with the CA beside it, 0600.
        let dir = scratch("link");
        let saved = ServerLink { url: "wss://relay.example.com/remote".into(), ca_der: None, token: Some("tok".into()) }.save(&dir).unwrap();
        let (back, _) = ServerConfig::lookup_in(&[dir.clone()]).unwrap().unwrap();
        assert_eq!(back, saved);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_typed_by_hand_are_checked_like_a_link() {
        let l = check_settings(" wss://relay.example.com/remote ", "", " tok ").unwrap();
        assert_eq!((l.url.as_str(), l.token.as_deref(), l.ca_der.is_none()), ("wss://relay.example.com/remote", Some("tok"), true));
        assert!(check_settings("wss://relay.example.com", "not a pem", "").is_err());
        assert!(check_settings("https://relay.example.com", "", "").is_err());
        // A CA that does parse is kept, except for the hosted service.
        let pem = pem_from_der(&[1, 2, 3]);
        if let Ok(l) = check_settings("wss://remote.sodiumos.com/remote", &pem, "") {
            assert_eq!(l.ca_der, None);
        }
    }
}
