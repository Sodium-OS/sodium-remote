// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Enrolment tokens: `sodium-remote-server token new` mints
//! one; a device sends it in `auth` when it registers. Hosts need one when the
//! server runs `--enrol token` (the default); clients may send one (it binds an
//! account), and a bad one is refused either way.
//!
//! **Storage:** one file per token in `<state dir>/tokens/<id>.json`, so the CLI
//! and a running server never write the same file: the CLI only creates and
//! deletes, the server only updates a token it redeems. Only the token's SHA-256
//! is stored; `id` is the first 12 hex digits of it, for `list` and `revoke`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::proto::b64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenRec {
    /// SHA-256 of the token, hex.
    pub hash: String,
    /// Registrations left; `None`: unlimited.
    pub uses_left: Option<u32>,
    /// Unix seconds after which it is refused; `None`: never.
    pub expires: Option<u64>,
    /// The account devices registered with it belong to (hosted services).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub created: u64,
}

impl TokenRec {
    pub fn id(&self) -> &str {
        &self.hash[..12]
    }
}

/// Why a token was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenError {
    /// Not a token this server minted (or revoked).
    Invalid,
    /// Past its expiry, or used up.
    Expired,
}

fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn path_for(dir: &Path, hash: &str) -> PathBuf {
    dir.join(format!("{}.json", &hash[..12]))
}

fn write_atomic(path: &Path, rec: &TokenRec) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    f.write_all(&serde_json::to_vec(rec)?)?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Mint a token. `uses`: `None` for unlimited. Returns the token (shown once) and its record.
pub fn mint(dir: &Path, uses: Option<u32>, expires: Option<u64>, account: Option<String>, now: u64) -> anyhow::Result<(String, TokenRec)> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut b = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut b);
    let token = b64(&b);
    let rec = TokenRec { hash: hash(&token), uses_left: uses, expires, account, created: now };
    write_atomic(&path_for(dir, &rec.hash), &rec)?;
    Ok((token, rec))
}

/// Every token's record, oldest first.
pub fn list(dir: &Path) -> anyhow::Result<Vec<TokenRec>> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    for e in entries {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "json")
            && let Ok(rec) = serde_json::from_slice::<TokenRec>(&fs::read(&p)?)
        {
            out.push(rec);
        }
    }
    out.sort_by_key(|r| r.created);
    Ok(out)
}

/// Delete a token by its ID (or a unique prefix of it).
pub fn revoke(dir: &Path, id: &str) -> anyhow::Result<TokenRec> {
    let found: Vec<TokenRec> = list(dir)?.into_iter().filter(|r| !id.is_empty() && r.hash.starts_with(id)).collect();
    match found.as_slice() {
        [rec] => {
            fs::remove_file(path_for(dir, &rec.hash))?;
            Ok(rec.clone())
        }
        [] => bail!("no token with ID {id}"),
        _ => bail!("{id} matches more than one token; give more of the ID"),
    }
}

/// Use a token for one registration. Returns its account.
pub fn redeem(dir: &Path, token: &str, now: u64) -> Result<Option<String>, TokenError> {
    let token = token.trim();
    if token.len() < 16 {
        return Err(TokenError::Invalid);
    }
    let h = hash(token);
    let path = path_for(dir, &h);
    let mut rec: TokenRec = fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(|r: &TokenRec| r.hash == h)
        .ok_or(TokenError::Invalid)?;
    if rec.expires.is_some_and(|e| now >= e) || rec.uses_left == Some(0) {
        return Err(TokenError::Expired);
    }
    if let Some(n) = &mut rec.uses_left {
        *n -= 1;
        if let Err(e) = write_atomic(&path, &rec) {
            tracing::error!(error = %e, "saving a redeemed token failed; refusing it");
            return Err(TokenError::Invalid);
        }
    }
    Ok(rec.account)
}

/// `30m`, `24h`, `7d`, `90s`, or `never`.
pub fn parse_duration(s: &str) -> anyhow::Result<Option<u64>> {
    if s == "never" {
        return Ok(None);
    }
    let (n, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = n.parse().with_context(|| format!("{s:?} is not a duration like 30m, 24h or 7d"))?;
    let mul = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => bail!("{s:?} is not a duration like 30m, 24h or 7d"),
    };
    Ok(Some(n * mul))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30m").unwrap(), Some(1800));
        assert_eq!(parse_duration("7d").unwrap(), Some(7 * 86400));
        assert_eq!(parse_duration("never").unwrap(), None);
        assert!(parse_duration("7").is_err());
        assert!(parse_duration("d").is_err());
    }
}
