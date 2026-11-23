// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Hyprland's command socket (what `hyprctl` talks to): one request per
//! connection, the reply read to EOF.
//!
//! Requests used: `j/monitors`, `j/workspaces`, `j/clients` (JSON), `/eval <lua>`
//! (0.57's Lua API: monitor rules and dispatchers), `/repl return <lua>` (sodiumwm's
//! API, which answers in JSON), `/output create headless NAME`
//! and `/output remove NAME`. A Lua error comes back as `error: ...`, a failed
//! dispatcher as `warning: ...`; both become errors here.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, bail};

#[derive(Clone, Debug)]
pub struct Hypr {
    socket: PathBuf,
    pub signature: String,
}

impl Hypr {
    /// The instance remoted runs in: `$HYPRLAND_INSTANCE_SIGNATURE`.
    pub fn from_env() -> Result<Self> {
        let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
            .context("HYPRLAND_INSTANCE_SIGNATURE is not set (import it into the user manager: docs/reference/host-session.md)")?;
        Ok(Self::with_signature(&sig))
    }

    pub fn with_signature(sig: &str) -> Self {
        let dir = crate::xdg_runtime_dir();
        Hypr { socket: PathBuf::from(dir).join("hypr").join(sig).join(".socket.sock"), signature: sig.to_string() }
    }

    pub fn request(&self, req: &str) -> Result<String> {
        let mut s = UnixStream::connect(&self.socket).with_context(|| format!("connecting to {}", self.socket.display()))?;
        s.set_read_timeout(Some(Duration::from_secs(3)))?;
        s.set_write_timeout(Some(Duration::from_secs(3)))?;
        s.write_all(req.as_bytes())?;
        let mut out = String::new();
        s.read_to_string(&mut out).with_context(|| format!("reading Hyprland's reply to {req:.60}"))?;
        Ok(out)
    }

    pub fn json(&self, what: &str) -> Result<serde_json::Value> {
        let out = self.request(&format!("j/{what}"))?;
        serde_json::from_str(&out).with_context(|| format!("Hyprland's {what}: {out:.200}"))
    }

    /// Runs Lua in the compositor. Fails on a Lua error or a dispatcher warning.
    pub fn eval(&self, lua: &str) -> Result<()> {
        let out = self.request(&format!("/eval {lua}"))?;
        let t = out.trim();
        if t.starts_with("error") || t.starts_with("warning") {
            bail!("Hyprland: {t} (for {lua})");
        }
        Ok(())
    }

    /// Calls `sodiumwm.api.<call>`: its JSON reply,
    /// or an error when sodiumwm isn't there or refused.
    pub fn sodiumwm(&self, call: &str) -> Result<serde_json::Value> {
        let out = self.request(&format!("/repl return sodiumwm.api.{call}"))?;
        let v: serde_json::Value =
            serde_json::from_str(out.trim()).with_context(|| format!("sodiumwm: {:.200} (for {call:.60})", out.trim()))?;
        if v["ok"] != serde_json::Value::Bool(true) {
            bail!("sodiumwm: {} (for {call:.60})", v["error"]);
        }
        Ok(v)
    }

    pub fn dispatch(&self, dispatcher: &str) -> Result<()> {
        self.eval(&format!("hl.dispatch({dispatcher})"))
    }

    fn ok(&self, req: &str) -> Result<()> {
        let out = self.request(req)?;
        if out.trim() != "ok" {
            bail!("Hyprland: {} (for {req})", out.trim());
        }
        Ok(())
    }

    pub fn output_create_headless(&self, name: &str) -> Result<()> {
        self.ok(&format!("/output create headless {name}"))
    }

    pub fn output_remove(&self, name: &str) -> Result<()> {
        self.ok(&format!("/output remove {name}"))
    }
}

/// A Lua string literal.
pub fn lua_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\{}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
