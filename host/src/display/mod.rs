// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The display manager: REMOTE-1 at the client's size, the
//! desk monitor mirroring it (or a privacy screen, or the privacy screen with the
//! panel off), every workspace moved there and back, floating windows placed
//! proportionally, and an exact restore.
//!
//! The order follows what was measured:
//! - connect: snapshot → REMOTE-1's rule (mode, refresh, snapped scale, a fixed place
//!   right of the desks) → create it → focus it → move every workspace → the desk's
//!   home mode (a mirror rule that carries the desk's own mode, position, scale and
//!   the rest) → focus REMOTE-1 again (Hyprland keeps focus on the mirror, and new
//!   windows then fail to map) → floating windows placed by sodiumwm's rules
//!   (`fitWindows`), or proportionally without it.
//! - disconnect: the desk back on and un-mirrored with its own values → workspaces
//!   back → each monitor's shown space back → floating windows to their snapshot
//!   rects (windows opened meanwhile placed proportionally) → REMOTE-1 removed.
//!
//! The snapshot is also written to `$XDG_RUNTIME_DIR/sodiumos/remoted-display-<sig>.json`
//! before anything changes, so a remoted that crashed mid-session can still put
//! everything back when it starts again.

pub mod hypr;
pub mod layout;
pub mod scale;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use hypr::{Hypr, lua_str};
use layout::{Monitor, Rect, Snapshot, Window, place};
pub use scale::UiSize;

pub const OUTPUT: &str = "REMOTE-1";

/// What the desk monitor does while a device is connected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HomeMode {
    /// It mirrors REMOTE-1, letterboxed (the default).
    #[default]
    Mirror,
    /// It shows the shell's "In use remotely" layer and local input is ignored.
    Privacy,
    /// Privacy with the panel powered down (DPMS): not mirroring, local
    /// input ignored, the privacy screen mapped (it lights the panel at a desk key).
    Off,
}

impl HomeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            HomeMode::Mirror => "mirror",
            HomeMode::Privacy => "privacy",
            HomeMode::Off => "off",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mirror" => Some(HomeMode::Mirror),
            "privacy" => Some(HomeMode::Privacy),
            "off" => Some(HomeMode::Off),
            _ => None,
        }
    }
}

/// What the client asks for.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Pixels: the window's backing size (windowed) or the display's (full screen).
    pub width: u32,
    pub height: u32,
    /// mHz, e.g. 60000.
    pub refresh_mhz: u32,
    pub ui: UiSize,
    /// The client's own UI ratio: panel pixels ÷ "looks like" points (1.741 on the 13" Air).
    pub ui_ratio: f64,
    /// The size the stream would have in full screen (the client's display, or
    /// a fixed size itself): the scale list comes from it. 0: the
    /// requested size.
    pub display_width: u32,
    pub display_height: u32,
}

/// REMOTE-1 for a request: the scale from the full-screen size first, then the
/// size rounded down to that scale's pixel period.
pub fn plan(req: &Request, desk_scale: f64) -> Applied {
    let (w, h) = (req.width.clamp(320, 7680), req.height.clamp(200, 4320));
    let (dw, dh) = match (req.display_width, req.display_height) {
        (0, _) | (_, 0) => (w, h),
        (dw, dh) => (dw.clamp(320, 7680), dh.clamp(200, 4320)),
    };
    let refresh_mhz = if req.refresh_mhz == 0 { 60_000 } else { req.refresh_mhz.clamp(24_000, 240_000) };
    let s = scale::choose(dw, dh, req.ui, req.ui_ratio, desk_scale);
    let (width, height) = scale::fit(w, h, s);
    Applied { width, height, refresh_mhz, scale: s }
}

/// The UI size menu's list (`Welcome.scales`): the full-screen size's scales.
pub fn scales_for(req: &Request) -> Vec<f64> {
    match (req.display_width, req.display_height) {
        (0, _) | (_, 0) => scale::valid_scales(req.width, req.height),
        (dw, dh) => scale::valid_scales(dw, dh),
    }
}

/// What REMOTE-1 became.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    pub width: u32,
    pub height: u32,
    pub refresh_mhz: u32,
    pub scale: f64,
}

/// A desk monitor's own rule values, so the mirror rule and its undo carry them all
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeskRule {
    pub name: String,
    pub fields: String,
}

impl DeskRule {
    fn of(m: &Monitor) -> Self {
        let mut f = format!(
            "mode = \"{}x{}@{:.3}\", position = \"{}x{}\", scale = {}, transform = {}",
            m.width, m.height, m.refresh, m.x, m.y, lua_str(&m.scale.to_string()), m.transform
        );
        if m.vrr {
            f.push_str(", vrr = 1");
        }
        if m.format.contains("2101010") {
            f.push_str(", bitdepth = 10");
        }
        if !m.cm.is_empty() {
            f.push_str(&format!(", cm = {}", lua_str(&m.cm)));
        }
        DeskRule { name: m.name.clone(), fields: f }
    }

    fn lua(&self, mirror: &str) -> String {
        format!("hl.monitor({{ output = {}, {}, mirror = {} }})", lua_str(&self.name), self.fields, lua_str(mirror))
    }
}

/// Everything needed to put the desktop back, kept while a session holds REMOTE-1.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Held {
    pub snapshot: Snapshot,
    pub desks: Vec<DeskRule>,
    pub applied: Applied,
    pub mode: HomeMode,
    /// The session's device, for a restarted remoted.
    pub device: String,
}

pub struct DisplayManager {
    pub hypr: Hypr,
    state_path: PathBuf,
    held: Option<Held>,
    /// sodiumwm placed the windows on REMOTE-1 (it then refits them at a resize too).
    wm: bool,
}

fn state_path() -> PathBuf {
    crate::runtime_dir().join("remoted-display.json")
}

impl DisplayManager {
    pub fn new(hypr: Hypr) -> Self {
        let mut state_path = state_path();
        // One file per Hyprland instance, so a nested test never meets the live one's.
        state_path.set_file_name(format!("remoted-display-{}.json", short(&hypr.signature)));
        DisplayManager { hypr, state_path, held: None, wm: false }
    }

    pub fn held(&self) -> Option<&Held> {
        self.held.as_ref()
    }

    /// A previous remoted's session, if it crashed while connected and REMOTE-1 is still up.
    pub fn adopt_leftover(&mut self) -> Result<Option<String>> {
        let Ok(bytes) = std::fs::read(&self.state_path) else { return Ok(None) };
        let held: Held = serde_json::from_slice(&bytes).context("reading the display state file")?;
        let now = Snapshot::read(&self.hypr)?;
        if now.monitor(OUTPUT).is_none() {
            // REMOTE-1 is gone (Hyprland restarted): nothing to restore.
            let _ = std::fs::remove_file(&self.state_path);
            return Ok(None);
        }
        let device = held.device.clone();
        self.held = Some(held);
        self.wm = self.hypr.sodiumwm(&fit_call(&[])).is_ok();
        Ok(Some(device))
    }

    fn save(&self) -> Result<()> {
        if let Some(h) = &self.held {
            std::fs::create_dir_all(self.state_path.parent().unwrap())?;
            crate::keys::write_private(&self.state_path, &serde_json::to_vec(h)?, 0o600)?;
        }
        Ok(())
    }

    fn plan(&self, req: &Request, desk_scale: f64) -> Applied {
        plan(req, desk_scale)
    }

    /// REMOTE-1's rule, at a fixed place: with "auto-right" Hyprland moves it to
    /// 0,0 once the desk mirrors (and back when it stops), after remoted placed
    /// the windows, and drags every window on it along by the desk's width.
    fn remote_rule(&self, a: &Applied, at: (i32, i32)) -> String {
        format!(
            "hl.monitor({{ output = {}, mode = \"{}x{}@{:.3}\", position = \"{}x{}\", scale = \"{}\" }})",
            lua_str(OUTPUT),
            a.width,
            a.height,
            a.refresh_mhz as f64 / 1000.0,
            at.0,
            at.1,
            scale::rule_value(a.scale)
        )
    }

    /// Waits until REMOTE-1 runs at `a` (Hyprland applies rules on its own loop).
    fn wait_applied(&self, a: &Applied) -> Result<Snapshot> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let s = Snapshot::read(&self.hypr)?;
            if let Some(m) = s.monitor(OUTPUT)
                && m.width == a.width
                && m.height == a.height
                && (m.scale - a.scale).abs() < 1e-3
            {
                return Ok(s);
            }
            if Instant::now() > deadline {
                let got = s.monitor(OUTPUT).map(|m| format!("{}x{} at {}", m.width, m.height, m.scale));
                bail!("{OUTPUT} didn't come up as {}x{} at {} (got {:?})", a.width, a.height, a.scale, got);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Hyprland applies a monitor rule on its own loop: wait until `desk` is (or
    /// isn't) mirroring, so focus moves only afterwards (it would land on the mirror).
    fn wait_mirror(&self, desk: &str, on: bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let s = Snapshot::read(&self.hypr)?;
            if s.monitor(desk).is_some_and(|m| m.mirroring() == on) {
                return Ok(());
            }
            if Instant::now() > deadline {
                bail!("{desk} didn't {} mirroring", if on { "start" } else { "stop" });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn focus_remote(&self) -> Result<()> {
        self.hypr.dispatch(&format!("hl.dsp.focus({{ monitor = {} }})", lua_str(OUTPUT)))
    }

    /// Connect: REMOTE-1 up, everything moved onto it, the desk in `mode`.
    pub fn connect(&mut self, req: &Request, mode: HomeMode, device: &str) -> Result<Applied> {
        if self.held.is_some() {
            return self.resize(req);
        }
        let mut snap = Snapshot::read(&self.hypr)?;
        if snap.monitor(OUTPUT).is_some() {
            // A REMOTE-1 nobody holds (a remoted killed before it saved): start clean.
            self.hypr.output_remove(OUTPUT)?;
            std::thread::sleep(Duration::from_millis(100));
            snap = Snapshot::read(&self.hypr)?;
        }
        let desks: Vec<DeskRule> =
            snap.monitors.iter().filter(|m| !m.disabled && !m.mirroring()).map(DeskRule::of).collect();
        if desks.is_empty() {
            bail!("no desk monitor to move from");
        }
        let desk_scale = snap.focused().map_or(1.0, |m| m.scale);
        let applied = self.plan(req, desk_scale);
        // Right of every monitor, so it never overlaps one and never has to move.
        let right = snap.monitors.iter().filter(|m| !m.disabled && !m.mirroring()).map(|m| m.rect().x + m.rect().w).max();
        let at = (right.unwrap_or(0), 0);
        self.held = Some(Held { snapshot: snap.clone(), desks, applied, mode: HomeMode::Mirror, device: device.into() });
        self.save()?;

        let result = (|| -> Result<()> {
            self.hypr.eval(&self.remote_rule(&applied, at))?;
            self.hypr.output_create_headless(OUTPUT)?;
            self.wait_applied(&applied)?;
            self.focus_remote()?;
            let mut ws = snap.workspaces.clone();
            ws.sort_by_key(|w| w.id);
            for w in ws.iter().filter(|w| w.monitor != OUTPUT) {
                self.hypr.dispatch(&format!(
                    "hl.dsp.workspace.move({{ workspace = {}, monitor = {} }})",
                    w.lua_ref(),
                    lua_str(OUTPUT)
                ))?;
            }
            self.apply_mode(None, mode)?;
            self.focus_remote()?;
            // REMOTE-1 came up with a fresh empty workspace: show what the desk showed. By id, as
            // the moves above: Hyprland 0.57.1's Lua focus doesn't find "Space 1" by its name and
            // makes a new workspace of that name, outside sodiumwm's spaces, where windows open
            // tiled and can't be dragged.
            if let Some(ws) = snap.focused().and_then(|m| snap.workspace(&m.active_workspace)) {
                self.hypr.dispatch(&format!("hl.dsp.focus({{ workspace = {} }})", ws.lua_ref()))?;
            }
            // Floating windows: from their old monitor's usable area to REMOTE-1's, by
            // sodiumwm's own rules when it runs (title bars, margins, snapped zones, and
            // a memory that makes a live resize exact both ways); else proportionally.
            let now = Snapshot::read(&self.hypr)?;
            let moved: Vec<Fit> = now
                .windows
                .iter()
                .filter(|w| w.floating && w.fullscreen == 0)
                .filter_map(|w| {
                    let old = snap.window(&w.address)?;
                    let from = old.monitor.as_deref().and_then(|m| snap.monitor(m))?;
                    (from.name != OUTPUT).then(|| Fit { window: w.clone(), at: old.rect, from: from.usable() })
                })
                .collect();
            match self.hypr.sodiumwm(&fit_call(&moved)) {
                Ok(_) => self.wm = true,
                Err(e) => {
                    eprintln!("remoted: display: placing the windows without sodiumwm ({e:#})");
                    self.wm = false;
                    let to = now.monitor(OUTPUT).context("REMOTE-1 vanished")?.usable();
                    for f in &moved {
                        self.move_window(&f.window, place(f.at, f.from, to))?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            // Never leave the desk half-moved.
            let _ = self.disconnect();
            return Err(e);
        }
        Ok(applied)
    }

    fn move_window(&self, w: &Window, r: Rect) -> Result<()> {
        let sel = lua_str(&format!("address:{}", w.address));
        let resized = (w.rect.w, w.rect.h) != (r.w, r.h);
        if resized {
            self.hypr.dispatch(&format!("hl.dsp.window.resize({{ x = {}, y = {}, window = {sel} }})", r.w, r.h))?;
        }
        // A resize can move it too (it keeps the window's centre), so a move follows one.
        if resized || (w.rect.x, w.rect.y) != (r.x, r.y) {
            self.hypr.dispatch(&format!("hl.dsp.window.move({{ x = {}, y = {}, window = {sel} }})", r.x, r.y))?;
        }
        Ok(())
    }

    /// Floating windows on REMOTE-1 follow it from `from` to its usable area now
    /// (sodiumwm, when it placed them, refits them itself at the size change).
    fn follow_remote(&self, from: Rect) -> Result<()> {
        if self.wm {
            return Ok(());
        }
        let now = Snapshot::read(&self.hypr)?;
        let to = now.monitor(OUTPUT).context("REMOTE-1 vanished")?.usable();
        if to == from {
            return Ok(());
        }
        for w in now.windows.iter().filter(|w| w.floating && w.fullscreen == 0 && w.monitor.as_deref() == Some(OUTPUT)) {
            // Hyprland may already have moved it with the monitor: only the ones outside.
            if !to.contains(&w.rect) {
                self.move_window(w, place(w.rect, from, to))?;
            }
        }
        Ok(())
    }

    /// Live resize (the caller debounces it by 150 ms).
    pub fn resize(&mut self, req: &Request) -> Result<Applied> {
        let held = self.held.as_ref().context("no session holds REMOTE-1")?;
        let desk_scale = held.snapshot.focused().map_or(1.0, |m| m.scale);
        let applied = self.plan(req, desk_scale);
        if applied == held.applied {
            return Ok(applied);
        }
        let remote = Snapshot::read(&self.hypr)?.monitor(OUTPUT).context("REMOTE-1 vanished")?.clone();
        let before = remote.usable();
        self.hypr.eval(&self.remote_rule(&applied, (remote.x, remote.y)))?;
        self.wait_applied(&applied)?;
        self.held.as_mut().unwrap().applied = applied;
        self.save()?;
        // (Off doesn't mirror REMOTE-1, so its desk rule doesn't change here: nothing to re-assert.)
        self.follow_remote(before)?;
        Ok(applied)
    }

    /// The desk monitor's mode, for this session.
    pub fn set_mode(&mut self, mode: HomeMode) -> Result<()> {
        let from = self.held.as_ref().context("no session")?.mode;
        if from == mode {
            return Ok(());
        }
        let before = Snapshot::read(&self.hypr)?.monitor(OUTPUT).context("REMOTE-1 vanished")?.usable();
        self.apply_mode(Some(from), mode)?;
        self.focus_remote()?;
        self.follow_remote(before)
    }

    fn dpms(&self, output: &str, on: bool) -> Result<()> {
        self.hypr.dispatch(&format!(
            "hl.dsp.dpms({{ action = \"{}\", monitor = {} }})",
            if on { "on" } else { "off" },
            lua_str(output)
        ))
    }

    /// Puts the desk monitors in `to`. `from` is their state now: None while they
    /// are still untouched (at connect). Only Mirror mirrors; Off is Privacy's rule
    /// plus DPMS off, asserted here at its start and on a change to it only.
    fn apply_mode(&mut self, from: Option<HomeMode>, to: HomeMode) -> Result<()> {
        let desks = self.held.as_ref().unwrap().desks.clone();
        let mirrored = |m: Option<HomeMode>| m == Some(HomeMode::Mirror);
        for d in &desks {
            if from == Some(HomeMode::Off) && to != HomeMode::Off {
                self.dpms(&d.name, true)?;
            }
            if mirrored(Some(to)) && !mirrored(from) {
                self.hypr.eval(&d.lua(OUTPUT))?;
                self.wait_mirror(&d.name, true)?;
            } else if !mirrored(Some(to)) && mirrored(from) {
                self.hypr.eval(&d.lua(""))?;
                self.wait_mirror(&d.name, false)?;
            }
        }
        if to == HomeMode::Off && from != Some(HomeMode::Off) {
            // After a rule: Hyprland powers up an output it reconfigures.
            std::thread::sleep(Duration::from_millis(100));
            for d in &desks {
                self.dpms(&d.name, false)?;
            }
        }
        self.held.as_mut().unwrap().mode = to;
        self.save()
    }

    /// Disconnect: everything back where it was, REMOTE-1 removed.
    pub fn disconnect(&mut self) -> Result<()> {
        let Some(held) = self.held.clone() else { return Ok(()) };
        let snap = &held.snapshot;
        let mut errors = Vec::new();
        let mut step = |r: Result<()>| {
            if let Err(e) = r {
                errors.push(format!("{e:#}"));
            }
        };
        let remote_area = Snapshot::read(&self.hypr).ok().and_then(|s| s.monitor(OUTPUT).map(|m| m.usable()));
        // 1. The desk on, with its own values and no mirror.
        for d in &held.desks {
            if held.mode == HomeMode::Off {
                step(self.dpms(&d.name, true));
            }
            step(self.hypr.eval(&d.lua("")));
            if held.mode == HomeMode::Mirror {
                step(self.wait_mirror(&d.name, false));
            }
        }
        // 2. Every workspace back to its monitor; ones made during the session to the focused desk.
        let home = snap.focused().map(|m| m.name.clone()).unwrap_or_else(|| held.desks[0].name.clone());
        let now = Snapshot::read(&self.hypr)?;
        let mut ws = now.workspaces.clone();
        ws.sort_by_key(|w| w.id);
        for w in ws.iter() {
            let target = snap.workspace(&w.name).map_or(home.as_str(), |o| o.monitor.as_str());
            if target != w.monitor && target != OUTPUT {
                step(self.hypr.dispatch(&format!(
                    "hl.dsp.workspace.move({{ workspace = {}, monitor = {} }})",
                    w.lua_ref(),
                    lua_str(target)
                )));
            }
        }
        // 3. Each monitor shows what it showed (a disabled one shows nothing and can't take focus).
        for m in snap.monitors.iter().filter(|m| m.name != OUTPUT && !m.disabled) {
            step(self.hypr.dispatch(&format!("hl.dsp.focus({{ monitor = {} }})", lua_str(&m.name))));
            if let Some(ws) = snap.workspace(&m.active_workspace) {
                // by id, as on connect
                step(self.hypr.dispatch(&format!("hl.dsp.focus({{ workspace = {} }})", ws.lua_ref())));
            }
        }
        // 4. Floating windows to their snapshot rects; new ones placed proportionally.
        let now = Snapshot::read(&self.hypr)?;
        for w in now.windows.iter().filter(|w| w.floating && w.fullscreen == 0) {
            match snap.window(&w.address) {
                Some(old) if old.floating && old.fullscreen == 0 => step(self.move_window(w, old.rect)),
                Some(_) => {}
                None => {
                    let (Some(from), Some(m)) = (remote_area, w.monitor.as_deref().and_then(|m| now.monitor(m))) else { continue };
                    if m.name != OUTPUT {
                        step(self.move_window(w, place(w.rect, from, m.usable())));
                    }
                }
            }
        }
        // 5. REMOTE-1 goes, and focus returns.
        if now.monitor(OUTPUT).is_some() {
            step(self.hypr.output_remove(OUTPUT));
        }
        step(self.hypr.dispatch(&format!("hl.dsp.focus({{ monitor = {} }})", lua_str(&home))));
        self.held = None;
        let _ = std::fs::remove_file(&self.state_path);
        if !errors.is_empty() {
            bail!("restoring the desktop: {}", errors.join("; "));
        }
        Ok(())
    }
}

/// A floating window moved onto REMOTE-1: where it was (`at`) on its old monitor's
/// usable area (`from`), both global logical px.
struct Fit {
    window: Window,
    at: Rect,
    from: Rect,
}

/// `sodiumwm.api.fitWindows` for these windows (refit.lua R.fit), as Lua.
fn fit_call(moved: &[Fit]) -> String {
    let rect = |r: &Rect| format!("{{ x = {}, y = {}, w = {}, h = {} }}", r.x, r.y, r.w, r.h);
    let list: Vec<String> = moved
        .iter()
        .map(|f| format!("{{ address = {}, at = {}, from = {} }}", lua_str(&f.window.address), rect(&f.at), rect(&f.from)))
        .collect();
    format!("fitWindows({}, {{ {} }})", lua_str(OUTPUT), list.join(", "))
}

fn short(sig: &str) -> String {
    sig.rsplit('_').next().unwrap_or(sig).to_string()
}
