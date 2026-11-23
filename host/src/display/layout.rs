// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! What the display manager reads from Hyprland and puts back: every monitor,
//! workspace and window, and where floating windows go when their monitor changes.
//!
//! Hyprland moves floating windows with their workspace but keeps their absolute
//! position (a 700×500 window at 1100,600 stayed there on a 1600×1040 REMOTE-1,
//! half off-screen). The design wants them placed "as a proportion of the usable area",
//! so remoted does it: each window keeps its place in the free space around it (a
//! window touching the right edge still touches it), and a window larger than the
//! new area is shrunk to fit.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::hypr::Hypr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, r: &Rect) -> bool {
        r.x >= self.x && r.y >= self.y && r.x + r.w <= self.x + self.w && r.y + r.h <= self.y + self.h
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Monitor {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub refresh: f64,
    pub scale: f64,
    pub transform: i64,
    /// left, top, right, bottom, in logical px (bar, dock).
    pub reserved: [i32; 4],
    pub active_workspace: String,
    pub special_workspace: String,
    pub focused: bool,
    pub mirror_of: String,
    pub dpms: bool,
    pub disabled: bool,
    pub vrr: bool,
    pub format: String,
    pub cm: String,
}

impl Monitor {
    fn from_json(m: &Value) -> Result<Self> {
        let s = |k: &str| m[k].as_str().unwrap_or_default().to_string();
        let i = |k: &str| m[k].as_i64().unwrap_or_default();
        let mut reserved = [0; 4];
        if let Some(r) = m["reserved"].as_array() {
            for (n, v) in r.iter().take(4).enumerate() {
                reserved[n] = v.as_i64().unwrap_or_default() as i32;
            }
        }
        Ok(Monitor {
            name: m["name"].as_str().context("monitor without a name")?.to_string(),
            x: i("x") as i32,
            y: i("y") as i32,
            width: i("width") as u32,
            height: i("height") as u32,
            refresh: m["refreshRate"].as_f64().unwrap_or(60.0),
            scale: m["scale"].as_f64().unwrap_or(1.0),
            transform: i("transform"),
            reserved,
            active_workspace: m["activeWorkspace"]["name"].as_str().unwrap_or_default().to_string(),
            special_workspace: m["specialWorkspace"]["name"].as_str().unwrap_or_default().to_string(),
            focused: m["focused"].as_bool().unwrap_or(false),
            mirror_of: s("mirrorOf"),
            dpms: m["dpmsStatus"].as_bool().unwrap_or(true),
            disabled: m["disabled"].as_bool().unwrap_or(false),
            vrr: m["vrr"].as_bool().unwrap_or(false),
            format: s("currentFormat"),
            cm: s("colorManagementPreset"),
        })
    }

    pub fn mirroring(&self) -> bool {
        !self.mirror_of.is_empty() && self.mirror_of != "none"
    }

    /// The monitor in global logical coordinates.
    pub fn rect(&self) -> Rect {
        let (pw, ph) = if self.transform % 2 == 1 { (self.height, self.width) } else { (self.width, self.height) };
        Rect { x: self.x, y: self.y, w: (pw as f64 / self.scale).round() as i32, h: (ph as f64 / self.scale).round() as i32 }
    }

    /// Where windows may go: the monitor minus the bar's and dock's reserved edges.
    pub fn usable(&self) -> Rect {
        let r = self.rect();
        let [l, t, rr, b] = self.reserved;
        Rect { x: r.x + l, y: r.y + t, w: (r.w - l - rr).max(1), h: (r.h - t - b).max(1) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: i64,
    pub name: String,
    pub monitor: String,
}

impl Workspace {
    /// How `hl.dsp.workspace.move` names it, as a Lua expression: a normal workspace by its id
    /// (a number; 0.57.1's Lua dispatcher finds a named one, "Space 1", neither by "name:Space 1"
    /// nor by "Space 1"), a special one by its full "special:…" name.
    pub fn lua_ref(&self) -> String {
        if self.id > 0 {
            self.id.to_string()
        } else {
            super::hypr::lua_str(&self.name)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub address: String,
    pub class: String,
    pub workspace: String,
    pub monitor: Option<String>,
    pub rect: Rect,
    pub floating: bool,
    pub fullscreen: i64,
    pub pinned: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub monitors: Vec<Monitor>,
    pub workspaces: Vec<Workspace>,
    pub windows: Vec<Window>,
}

impl Snapshot {
    pub fn read(h: &Hypr) -> Result<Self> {
        // "all": a mirroring output is left out of the plain list.
        let mons = h.json("monitors all")?;
        let monitors =
            mons.as_array().context("monitors: not a list")?.iter().map(Monitor::from_json).collect::<Result<Vec<_>>>()?;
        let ids: Vec<(i64, String)> = mons
            .as_array()
            .unwrap()
            .iter()
            .map(|m| (m["id"].as_i64().unwrap_or(-1), m["name"].as_str().unwrap_or_default().to_string()))
            .collect();
        let workspaces = h
            .json("workspaces")?
            .as_array()
            .context("workspaces: not a list")?
            .iter()
            .map(|w| Workspace {
                id: w["id"].as_i64().unwrap_or_default(),
                name: w["name"].as_str().unwrap_or_default().to_string(),
                monitor: w["monitor"].as_str().unwrap_or_default().to_string(),
            })
            .collect();
        let windows = h
            .json("clients")?
            .as_array()
            .context("clients: not a list")?
            .iter()
            .filter(|c| c["mapped"].as_bool().unwrap_or(true))
            .map(|c| {
                let pair = |k: &str| (c[k][0].as_i64().unwrap_or_default() as i32, c[k][1].as_i64().unwrap_or_default() as i32);
                let ((x, y), (w, hh)) = (pair("at"), pair("size"));
                let mon = c["monitor"].as_i64().unwrap_or(-1);
                Window {
                    address: c["address"].as_str().unwrap_or_default().to_string(),
                    class: c["class"].as_str().unwrap_or_default().to_string(),
                    workspace: c["workspace"]["name"].as_str().unwrap_or_default().to_string(),
                    monitor: ids.iter().find(|(id, _)| *id == mon).map(|(_, n)| n.clone()),
                    rect: Rect { x, y, w, h: hh },
                    floating: c["floating"].as_bool().unwrap_or(false),
                    fullscreen: c["fullscreen"].as_i64().unwrap_or_default(),
                    pinned: c["pinned"].as_bool().unwrap_or(false),
                }
            })
            .collect();
        Ok(Snapshot { monitors, workspaces, windows })
    }

    pub fn monitor(&self, name: &str) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.name == name)
    }

    pub fn workspace(&self, name: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.name == name)
    }

    pub fn window(&self, address: &str) -> Option<&Window> {
        self.windows.iter().find(|w| w.address == address)
    }

    pub fn focused(&self) -> Option<&Monitor> {
        self.monitors.iter().find(|m| m.focused)
    }
}

/// Where along `free` px of free space an edge at `off` sits, 0 to 1.
fn frac(off: i32, free: i32) -> f64 {
    if free <= 0 { 0.5 } else { (off as f64 / free as f64).clamp(0.0, 1.0) }
}

/// A floating window's rect moved from area `from` to area `to`.
pub fn place(r: Rect, from: Rect, to: Rect) -> Rect {
    let w = r.w.min(to.w);
    let h = r.h.min(to.h);
    let fx = frac(r.x - from.x, from.w - r.w);
    let fy = frac(r.y - from.y, from.h - r.h);
    Rect { x: to.x + (fx * (to.w - w) as f64).round() as i32, y: to.y + (fy * (to.h - h) as f64).round() as i32, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_windows_keep_their_place_and_fit() {
        // The case: 700×500 at 1100,600 on a 1920×1200 desk, onto 1600×1040 at 0,0.
        let desk = Rect { x: 0, y: 0, w: 1920, h: 1200 };
        let remote = Rect { x: 0, y: 0, w: 1600, h: 1040 };
        let r = place(Rect { x: 1100, y: 600, w: 700, h: 500 }, desk, remote);
        assert!(remote.contains(&r));
        assert_eq!((r.w, r.h), (700, 500));
        // 1100 of 1220 free px → 0.9016 of 900 = 811; 600 of 700 → 0.857 of 540 = 463.
        assert_eq!((r.x, r.y), (811, 463));
        // Right-edge docked stays docked, and back again is exact.
        let docked = Rect { x: 1220, y: 0, w: 700, h: 500 };
        let there = place(docked, desk, remote);
        assert_eq!(there.x + there.w, 1600);
        assert_eq!(place(there, remote, desk), docked);
        // Too big: shrunk to the area.
        let big = place(Rect { x: 10, y: 10, w: 1900, h: 1190 }, desk, remote);
        assert_eq!(big, remote);
    }

    #[test]
    fn a_workspace_is_moved_by_its_id_and_a_special_one_by_its_name() {
        let named = Workspace { id: 1, name: "Space 1".into(), monitor: "DP-2".into() };
        let special = Workspace { id: 0, name: "special:minimized".into(), monitor: "DP-2".into() };
        assert_eq!(named.lua_ref(), "1");
        assert_eq!(special.lua_ref(), "\"special:minimized\"");
    }
}
