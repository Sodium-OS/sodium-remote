// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The matched display: while a client is connected, a virtual monitor the size of its
//! window is this computer's only active display (Windows hosts, with the setting
//! "Match the display to the connecting computer" on and the driver installed).
//!
//! Platform-free. The driver is behind [`VirtualDisplay`] (Windows: SudoVDA, `win::sudovda`)
//! and the OS's display configuration behind [`Topology`] (Windows: `SetDisplayConfig`,
//! `win::topology`); `fake::FakeDisplays` has both for tests. Mac and Linux have neither,
//! so nothing here runs there.
//!
//! [`Matched`] runs one matched display:
//! - **Start**: the active topology is read and written to the journal ([`JOURNAL`] in the
//!   host's data folder) before anything changes; the monitor is created at the client's
//!   window size ([`matched_mode`]); once Windows shows it (within [`APPEAR_WITHIN`], else
//!   the caller falls back to the physical display and scaling), it becomes the only
//!   active display, at 0,0, without saving to the OS's display database. The physical
//!   displays are off meanwhile; Windows moves the windows, the host never does.
//! - **Alive**: a thread pings the driver every second. SudoVDA removes its monitors about
//!   3 s after the last ping, so a host that crashes or is killed takes its monitor with
//!   it, and Windows goes back to the saved (physical) topology by itself.
//! - **New size**: in place when the driver can; otherwise a new monitor at the new mode
//!   becomes the only display, then the old one is removed.
//! - **End**: the saved topology is applied again, the monitor removed, windows left on no
//!   active monitor moved to the primary, the journal deleted.
//!
//! A journal left behind (a crash, a sign-out) is handled by [`recover`] at the next start.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The `settings.json` key of "Match the display to the connecting computer" (default off).
pub const SETTING: &str = "matched_display";
/// The topology journal, in the host's data folder.
pub const JOURNAL: &str = "matched-display.json";
/// How long a new virtual monitor may take to show up as a display before the session
/// falls back to the physical display and scaling.
pub const APPEAR_WITHIN: Duration = Duration::from_secs(3);
/// How long the client's window must stay the same size before the monitor changes mode;
/// scaling covers the drag until then.
pub const MODE_QUIET: Duration = Duration::from_millis(400);
/// A lost connection keeps the matched display this long for the client to come back.
pub const LOST_GRACE: Duration = Duration::from_secs(15);
/// The smallest matched mode, and the highest refresh.
pub const MIN_SIZE: (u32, u32) = (800, 600);
pub const MAX_HZ: u32 = 120;
/// No side longer than this (the encoders' limit).
const MAX_SIDE: u32 = 8192;
/// How often the driver is pinged (SudoVDA's watchdog: 3 s by default).
const PING_EVERY: Duration = Duration::from_secs(1);
/// After a change of the only display, how long the new display id may take to show.
const SETTLE: Duration = Duration::from_secs(1);
/// After a restore, how long Windows gets to move its windows back before the rescue.
const RESCUE_AFTER: Duration = Duration::from_millis(300);

/// A virtual monitor's mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub hz: u32,
}

/// The mode for a client's window (physical pixels; 0 unknown), its refresh (mHz; 0
/// unknown: 60 Hz) and its decoder's largest picture (0: no limit): even, at least
/// 800×600, at most the decoder's size, at most 120 Hz.
pub fn matched_mode(window: (u32, u32), refresh_mhz: u32, decoder_max: (u32, u32)) -> Mode {
    let side = |v: u32, min: u32, max: u32| {
        let max = if max == 0 { MAX_SIDE } else { max.min(MAX_SIDE) };
        v.min(max).max(min) & !1
    };
    let hz = refresh_mhz.saturating_add(500) / 1000;
    Mode {
        width: side(window.0, MIN_SIZE.0, decoder_max.0),
        height: side(window.1, MIN_SIZE.1, decoder_max.1),
        hz: if hz == 0 { 60 } else { hz.min(MAX_HZ) },
    }
}

/// A virtual monitor's own identity (SudoVDA: its GUID). A new one for every monitor.
pub type Key = u128;

pub fn new_key() -> Key {
    static N: AtomicU64 = AtomicU64::new(0);
    let mut h = Sha256::new();
    h.update(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos().to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    h.update(N.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    u128::from_le_bytes(h.finalize()[..16].try_into().expect("16 bytes"))
}

/// A virtual monitor the driver made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    pub key: Key,
    /// Where the OS shows it: the display target's id and its adapter (Windows: the LUID).
    pub target: u32,
    pub adapter: u64,
    pub mode: Mode,
}

/// The driver.
pub trait VirtualDisplay: Send + Sync {
    /// Adds a monitor at `mode`.
    fn create(&self, key: Key, mode: Mode) -> Result<Monitor>;
    /// Changes `m`'s mode in place, without saving it, when the driver can (SudoVDA: only to
    /// a mode the monitor lists). False: it can't, and [`Matched`] replaces the monitor.
    fn set_mode(&self, m: &Monitor, mode: Mode) -> Result<bool>;
    /// Removes a monitor (one that is already gone is no error).
    fn remove(&self, key: Key);
    /// Keeps the monitors alive: they go when the host stops calling this.
    fn keep_alive(&self) -> Result<()>;
}

/// The OS's display configuration.
pub trait Topology: Send + Sync {
    /// The display id (`backend::Display`'s) of `m` while it is an active display.
    fn display_of(&self, m: &Monitor) -> Option<u32>;
    /// The active topology now, as bytes for the journal.
    fn save(&self) -> Result<Vec<u8>>;
    /// Makes `m` the only active display, primary at 0,0, without saving to the OS's database.
    fn only(&self, m: &Monitor) -> Result<()>;
    /// Applies a saved topology without saving to the OS's database; bytes it can't apply
    /// (none, or from before a restart) apply the database's own configuration instead.
    fn restore(&self, saved: &[u8]) -> Result<()>;
    /// Moves visible windows that are on no active monitor to the primary.
    fn rescue_windows(&self);
}

/// The journal: the monitors to remove and the topology to apply again after a crash.
#[derive(Clone, Debug, PartialEq)]
pub struct Journal {
    pub monitors: Vec<Key>,
    pub topology: Vec<u8>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

impl Journal {
    /// Written whole and renamed into place, and flushed: it must outlive a crash.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let v = json!({
            "version": 1,
            "monitors": self.monitors.iter().map(|k| format!("{k:032x}")).collect::<Vec<_>>(),
            "topology": hex(&self.topology),
        });
        let tmp = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(&v).expect("json"))?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path)
    }

    pub fn read(path: &Path) -> Option<Journal> {
        let v: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        let monitors = v.get("monitors")?.as_array()?.iter().map(|k| u128::from_str_radix(k.as_str()?, 16).ok()).collect::<Option<Vec<_>>>()?;
        Some(Journal { monitors, topology: unhex(v.get("topology")?.as_str()?)? })
    }
}

/// At startup: a journal left by a host that stopped with a matched display up (a crash, a
/// sign-out). Removes the monitors it names (when the driver answers), applies the saved
/// topology again, moves stranded windows to the primary and deletes the journal. True
/// when there was one.
pub fn recover(journal: &Path, driver: Option<&dyn VirtualDisplay>, topo: &dyn Topology) -> bool {
    if !journal.exists() {
        return false;
    }
    let j = Journal::read(journal);
    eprintln!("sodium-remote-host: matched display: the host stopped while it was up; restoring the displays");
    if let (Some(j), Some(d)) = (&j, driver) {
        for k in &j.monitors {
            d.remove(*k);
        }
    }
    // An unreadable journal: the database's own configuration.
    if let Err(e) = topo.restore(j.as_ref().map_or(&[][..], |j| &j.topology)) {
        eprintln!("sodium-remote-host: matched display: restoring the displays: {e:#}");
    }
    std::thread::sleep(RESCUE_AFTER);
    topo.rescue_windows();
    let _ = std::fs::remove_file(journal);
    true
}

/// "Match the display to the connecting computer".
pub fn wanted(data: &Path) -> bool {
    crate::settings::read(data).get(SETTING).and_then(Value::as_bool).unwrap_or(false)
}

pub fn set_wanted(data: &Path, on: bool) {
    crate::settings::write(data, SETTING, json!(on));
}

/// What a connection gets.
pub enum Choice {
    /// The setting is off, or this OS has no matched display: the physical display.
    Physical,
    /// The matched display is up.
    Virtual(Matched),
    /// The setting is on but the matched display couldn't start: the physical display
    /// and scaling for this session, and the app says so.
    Failed,
}

/// The display for a new connection: the matched display when `wanted` and the driver and
/// the topology are there and it appears in time; else the physical display.
pub fn open(
    wanted: bool,
    topo: Option<Arc<dyn Topology>>,
    driver: impl FnOnce() -> Result<Arc<dyn VirtualDisplay>>,
    journal: PathBuf,
    mode: Mode,
    within: Duration,
) -> Choice {
    let Some(topo) = topo.filter(|_| wanted) else { return Choice::Physical };
    match driver().and_then(|d| Matched::start(d, topo, journal, mode, within)) {
        Ok(m) => Choice::Virtual(m),
        Err(e) => {
            eprintln!("sodium-remote-host: matched display: {e:#}; showing the physical display instead");
            Choice::Failed
        }
    }
}

/// Pings the driver until dropped.
struct KeepAlive {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl KeepAlive {
    fn start(driver: Arc<dyn VirtualDisplay>) -> KeepAlive {
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let thread = std::thread::Builder::new()
            .name("vdisplay-ping".into())
            .spawn(move || {
                let mut failing = false;
                while !st.load(Ordering::Relaxed) {
                    match driver.keep_alive() {
                        Ok(()) => failing = false,
                        Err(e) if !failing => {
                            failing = true;
                            eprintln!("sodium-remote-host: matched display: ping: {e:#}");
                        }
                        Err(_) => {}
                    }
                    let until = Instant::now() + PING_EVERY;
                    while !st.load(Ordering::Relaxed) && Instant::now() < until {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            })
            .ok();
        KeepAlive { stop, thread }
    }
}

impl Drop for KeepAlive {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn wait_display(topo: &dyn Topology, m: &Monitor, until: Instant) -> Option<u32> {
    loop {
        if let Some(id) = topo.display_of(m) {
            return Some(id);
        }
        if Instant::now() >= until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One matched display, up until [`Matched::end`] (or drop).
pub struct Matched {
    driver: Arc<dyn VirtualDisplay>,
    topo: Arc<dyn Topology>,
    journal: PathBuf,
    /// The topology from before, applied again at the end.
    saved: Vec<u8>,
    monitor: Option<Monitor>,
    display: u32,
    alive: Option<KeepAlive>,
}

impl Matched {
    /// Journal, monitor, wait for it (`within`), make it the only display. Every step that
    /// fails undoes the ones before.
    pub fn start(driver: Arc<dyn VirtualDisplay>, topo: Arc<dyn Topology>, journal: PathBuf, mode: Mode, within: Duration) -> Result<Matched> {
        let saved = topo.save().context("reading the display configuration")?;
        let key = new_key();
        Journal { monitors: vec![key], topology: saved.clone() }.write(&journal).with_context(|| format!("writing {}", journal.display()))?;
        let mut m = Matched { driver, topo, journal, saved, monitor: None, display: 0, alive: None };
        let deadline = Instant::now() + within;
        let mon = m.driver.create(key, mode).context("creating the virtual display")?;
        m.monitor = Some(mon);
        m.alive = Some(KeepAlive::start(m.driver.clone()));
        wait_display(&*m.topo, &mon, deadline).ok_or_else(|| anyhow!("the virtual display didn't appear within {} s", within.as_secs_f32()))?;
        m.topo.only(&mon).context("making the virtual display the only one")?;
        m.display = wait_display(&*m.topo, &mon, Instant::now() + SETTLE).ok_or_else(|| anyhow!("the virtual display went away"))?;
        eprintln!("sodium-remote-host: matched display: display {} at {}x{} @{} Hz is the only display", m.display, mode.width, mode.height, mode.hz);
        Ok(m)
    }

    /// The virtual monitor's display id.
    pub fn display(&self) -> u32 {
        self.display
    }

    pub fn mode(&self) -> Option<Mode> {
        self.monitor.map(|m| m.mode)
    }

    /// A new mode: in place when the driver can, else a new monitor that becomes the only
    /// display before the old one goes. On failure the old monitor stays.
    pub fn set_mode(&mut self, mode: Mode, within: Duration) -> Result<()> {
        let old = self.monitor.context("no virtual display")?;
        if old.mode == mode {
            return Ok(());
        }
        if self.driver.set_mode(&old, mode)? {
            self.monitor = Some(Monitor { mode, ..old });
            self.display = wait_display(&*self.topo, &old, Instant::now() + SETTLE).unwrap_or(self.display);
            eprintln!("sodium-remote-host: matched display: now {}x{} @{} Hz", mode.width, mode.height, mode.hz);
            return Ok(());
        }
        let key = new_key();
        let journal = |keys: Vec<Key>, saved: &[u8], path: &Path| Journal { monitors: keys, topology: saved.to_vec() }.write(path);
        journal(vec![old.key, key], &self.saved, &self.journal).context("writing the display journal")?;
        let deadline = Instant::now() + within;
        let step = self.driver.create(key, mode).context("creating the virtual display").and_then(|new| {
            wait_display(&*self.topo, &new, deadline).ok_or_else(|| anyhow!("the new virtual display didn't appear"))?;
            self.topo.only(&new).context("making the new virtual display the only one")?;
            Ok(new)
        });
        let new = match step {
            Ok(n) => n,
            Err(e) => {
                // Back to the old monitor alone (its arrival may have changed the topology).
                self.driver.remove(key);
                std::thread::sleep(RESCUE_AFTER);
                let _ = self.topo.only(&old);
                let _ = journal(vec![old.key], &self.saved, &self.journal);
                return Err(e);
            }
        };
        self.driver.remove(old.key);
        self.monitor = Some(new);
        if let Err(e) = journal(vec![key], &self.saved, &self.journal) {
            eprintln!("sodium-remote-host: matched display: writing the journal: {e:#}");
        }
        // The old monitor's departure may make Windows apply its own topology: the new one alone again.
        let gone = Instant::now() + SETTLE;
        while self.topo.display_of(&old).is_some() && Instant::now() < gone {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.topo.only(&new);
        self.display = wait_display(&*self.topo, &new, Instant::now() + SETTLE).context("the new virtual display went away")?;
        eprintln!("sodium-remote-host: matched display: now display {} at {}x{} @{} Hz", self.display, mode.width, mode.height, mode.hz);
        Ok(())
    }

    /// The saved topology again, the monitor removed, stranded windows moved to the
    /// primary, the journal deleted. Also on drop.
    pub fn end(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        if let Some(mon) = self.monitor.take() {
            if let Err(e) = self.topo.restore(&self.saved) {
                eprintln!("sodium-remote-host: matched display: restoring the displays: {e:#}");
            }
            self.driver.remove(mon.key);
            self.alive = None;
            std::thread::sleep(RESCUE_AFTER);
            self.topo.rescue_windows();
            eprintln!("sodium-remote-host: matched display: removed; the displays are as they were");
        }
        let _ = std::fs::remove_file(&self.journal);
    }
}

impl Drop for Matched {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeDisplays;

    fn dir(name: &str) -> PathBuf {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../build/test-tmp/vdisplay").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn parts(f: &Arc<FakeDisplays>) -> (Arc<dyn VirtualDisplay>, Arc<dyn Topology>) {
        (f.clone(), f.clone())
    }

    #[test]
    fn the_mode_is_even_at_least_800_by_600_and_at_most_120_hz() {
        assert_eq!(matched_mode((1733, 1091), 59_940, (0, 0)), Mode { width: 1732, height: 1090, hz: 60 });
        assert_eq!(matched_mode((640, 480), 144_000, (0, 0)), Mode { width: 800, height: 600, hz: 120 });
        assert_eq!(matched_mode((0, 0), 0, (0, 0)), Mode { width: 800, height: 600, hz: 60 });
        assert_eq!(matched_mode((3840, 2160), 60_000, (1920, 1080)), Mode { width: 1920, height: 1080, hz: 60 });
    }

    #[test]
    fn a_leftover_journal_is_read_back_and_undone() {
        let d = dir("journal");
        let path = d.join(JOURNAL);
        let j = Journal { monitors: vec![0x0123_4567_89ab_cdef_0011_2233_4455_6677, 7], topology: vec![0, 1, 0xfe, 0xff] };
        j.write(&path).unwrap();
        assert_eq!(Journal::read(&path), Some(j.clone()));
        let f = FakeDisplays::new(Some(Duration::ZERO));
        assert!(recover(&path, Some(&*f), &*f));
        let s = f.state.lock().unwrap();
        assert_eq!(s.removed, j.monitors);
        assert_eq!(s.restored, vec![j.topology.clone()]);
        assert_eq!(s.rescued, 1);
        assert!(!path.exists());
    }

    #[test]
    fn the_setting_chooses_the_matched_display_and_a_new_size_replaces_the_monitor() {
        let d = dir("matched");
        let f = FakeDisplays::new(Some(Duration::from_millis(30)));
        let (drv, topo) = parts(&f);
        let mode = Mode { width: 1734, height: 1092, hz: 60 };
        // Off: the physical display, the driver isn't even opened.
        let off = open(false, Some(topo.clone()), || panic!("opened"), d.join(JOURNAL), mode, APPEAR_WITHIN);
        assert!(matches!(off, Choice::Physical));
        let Choice::Virtual(mut m) = open(true, Some(topo.clone()), || Ok(drv.clone()), d.join(JOURNAL), mode, APPEAR_WITHIN) else { panic!("not virtual") };
        let first = f.state.lock().unwrap().monitors[0].0;
        assert_eq!(f.state.lock().unwrap().only, Some(first.key));
        assert_eq!(m.display(), f.display_id(&first));
        assert_eq!(Journal::read(&d.join(JOURNAL)).unwrap().monitors, vec![first.key]);

        let bigger = Mode { width: 1920, height: 1200, hz: 60 };
        m.set_mode(bigger, APPEAR_WITHIN).unwrap();
        let (now, only) = {
            let s = f.state.lock().unwrap();
            (s.monitors.iter().map(|(m, _)| *m).collect::<Vec<_>>(), s.only)
        };
        assert_eq!(now.len(), 1);
        assert_eq!((now[0].mode, only), (bigger, Some(now[0].key)));
        assert_eq!(Journal::read(&d.join(JOURNAL)).unwrap().monitors, vec![now[0].key]);

        m.end();
        let s = f.state.lock().unwrap();
        assert!(s.monitors.is_empty());
        assert_eq!(s.restored, vec![FakeDisplays::PHYSICAL.to_vec()]);
        assert!(!d.join(JOURNAL).exists());
    }

    #[test]
    fn a_display_that_doesnt_appear_falls_back_and_undoes_everything() {
        let d = dir("timeout");
        let f = FakeDisplays::new(None);
        let (drv, topo) = parts(&f);
        let t0 = Instant::now();
        let c = open(true, Some(topo), || Ok(drv), d.join(JOURNAL), Mode { width: 800, height: 600, hz: 60 }, Duration::from_millis(200));
        assert!(matches!(c, Choice::Failed));
        assert!(t0.elapsed() >= Duration::from_millis(200));
        let s = f.state.lock().unwrap();
        assert!(s.monitors.is_empty() && s.only.is_none());
        assert_eq!(s.restored.len(), 1);
        assert!(!d.join(JOURNAL).exists());
    }
}
