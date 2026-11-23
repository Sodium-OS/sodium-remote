// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Audio (design): while a device is connected, a virtual
//! PipeWire sink "Remote" is the default output, so the desk speakers go quiet
//! without a mute, and what plays into it goes to the client as Opus. A virtual
//! source "Remote microphone" carries the client's mic and is the default input
//! while it is on. Afterwards the previous defaults, and the previous default
//! sink's volume, come back exactly as they were.
//!
//! The PipeWire and Opus work is the C shim `remote/linux-media/csrc/audio_shim.c`. WirePlumber's
//! "default" metadata decides the defaults: `default.configured.audio.sink` is
//! what a user (or remoted) chose, `default.audio.sink` what is in effect. remoted
//! saves the configured value (or its absence) and the effective node's channel
//! volumes and mute as exact floats, and puts back exactly that.

use std::{
    ffi::{CStr, CString, c_char, c_int},
    path::PathBuf,
    sync::Arc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub const SINK_NODE: &str = "sodiumos-remote";
pub const SINK_DESC: &str = "Remote";
pub const MIC_NODE: &str = "sodiumos-remote-mic";
pub const MIC_DESC: &str = "Remote microphone";
/// Opus at 10 ms frames, 48 kHz stereo.
pub const SINK_BITRATE: i32 = 128_000;

// The shim lives in remote-linux-media, shared with the generic Linux host.
use remote_linux_media::audio::ffi::*;

fn check(r: c_int, what: &str) -> Result<c_int> {
    if r < 0 {
        bail!("{what}: {}", std::io::Error::from_raw_os_error(-r));
    }
    Ok(r)
}

/// A node's volume: per-channel linear volumes and mute, exactly as PipeWire holds them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Volume {
    pub channels: Vec<f32>,
    pub mute: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Sink,
    Source,
}

impl Kind {
    fn configured_key(self) -> &'static str {
        match self {
            Kind::Sink => "default.configured.audio.sink",
            Kind::Source => "default.configured.audio.source",
        }
    }
    fn effective_key(self) -> &'static str {
        match self {
            Kind::Sink => "default.audio.sink",
            Kind::Source => "default.audio.source",
        }
    }
}

/// A default device as it was before the session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub kind: Kind,
    /// The configured default's raw JSON, or None if nothing was configured.
    pub configured: Option<String>,
    /// The node in effect, and its volume.
    pub node: Option<String>,
    pub volume: Option<Volume>,
}

/// What a restore did, for the log and the tests.
#[derive(Debug, Default, PartialEq)]
pub struct Restored {
    pub configured_back: bool,
    pub volume_was_changed: bool,
    pub volume_back: bool,
}

/// A connection to PipeWire.
pub struct Audio {
    ra: *mut RaAudio,
}

// The shim locks PipeWire's thread loop around every call.
unsafe impl Send for Audio {}
unsafe impl Sync for Audio {}

fn cstr(s: &str) -> CString {
    CString::new(s).expect("no NUL in names")
}

fn name_of(json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(json).ok()?["name"].as_str().map(str::to_string)
}

impl Audio {
    pub fn open() -> Result<Arc<Self>> {
        let mut err = 0;
        let ra = unsafe { ra_open(&mut err) };
        if ra.is_null() {
            check(err.min(-1), "connecting to PipeWire")?;
        }
        Ok(Arc::new(Audio { ra }))
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        let mut buf = vec![0 as c_char; 2048];
        let n = check(unsafe { ra_meta_get(self.ra, cstr(key).as_ptr(), buf.as_mut_ptr(), buf.len() as c_int) }, key)?;
        if n == 0 {
            return Ok(None);
        }
        Ok(Some(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()))
    }

    pub fn meta_set(&self, key: &str, json: Option<&str>) -> Result<()> {
        let v = json.map(cstr);
        check(unsafe { ra_meta_set(self.ra, cstr(key).as_ptr(), v.as_ref().map_or(std::ptr::null(), |c| c.as_ptr())) }, key)?;
        Ok(())
    }

    pub fn volume(&self, node: &str) -> Result<Option<Volume>> {
        let mut v = [0f32; 64];
        let mut mute = 0;
        let n = unsafe { ra_node_volume_get(self.ra, cstr(node).as_ptr(), v.as_mut_ptr(), 64, &mut mute) };
        if n == -libc::ENOENT || n == -libc::ENODATA {
            return Ok(None);
        }
        let n = check(n, node)? as usize;
        Ok(Some(Volume { channels: v[..n].to_vec(), mute: mute != 0 }))
    }

    pub fn set_volume(&self, node: &str, vol: &Volume) -> Result<()> {
        check(
            unsafe { ra_node_volume_set(self.ra, cstr(node).as_ptr(), vol.channels.as_ptr(), vol.channels.len() as c_int, vol.mute as c_int) },
            node,
        )?;
        Ok(())
    }

    pub fn node_exists(&self, node: &str) -> bool {
        unsafe { ra_node_exists(self.ra, cstr(node).as_ptr()) == 1 }
    }

    fn wait_node(&self, node: &str, present: bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.node_exists(node) != present {
            if Instant::now() > deadline {
                bail!("PipeWire node {node} never {}", if present { "appeared" } else { "went away" });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    /// Reads a default before the session changes it.
    pub fn save(&self, kind: Kind) -> Result<Saved> {
        let configured = self.meta_get(kind.configured_key())?;
        let node = self.meta_get(kind.effective_key())?.as_deref().and_then(name_of);
        let volume = match &node {
            Some(n) => self.volume(n)?,
            None => None,
        };
        Ok(Saved { kind, configured, node, volume })
    }

    fn make_default(&self, kind: Kind, node: &str) -> Result<()> {
        self.meta_set(kind.configured_key(), Some(&serde_json::json!({ "name": node }).to_string()))
    }

    /// Puts back the configured default (or its absence), then the old default's
    /// volume if anything changed it. Call it after the virtual node is gone, so
    /// nothing moves streams afterwards.
    pub fn restore(&self, saved: &Saved) -> Result<Restored> {
        let mut out = Restored::default();
        if self.meta_get(saved.kind.configured_key())? != saved.configured {
            self.meta_set(saved.kind.configured_key(), saved.configured.as_deref())?;
            out.configured_back = true;
        }
        if let (Some(node), Some(vol)) = (&saved.node, &saved.volume)
            && let Some(now) = self.volume(node)?
            && &now != vol
        {
            out.volume_was_changed = true;
            self.set_volume(node, vol)?;
            out.volume_back = self.volume(node)?.as_ref() == Some(vol);
        }
        Ok(out)
    }

    /// The "Remote" sink, made the default output. Its Opus packets go to `on_packet`
    /// from a thread of its own until [`Sink::stop`].
    pub fn start_sink(self: &Arc<Self>, on_packet: impl FnMut(Vec<u8>) + Send + 'static) -> Result<Sink> {
        let saved = self.save(Kind::Sink)?;
        check(unsafe { ra_sink_start(self.ra, cstr(SINK_NODE).as_ptr(), cstr(SINK_DESC).as_ptr(), SINK_BITRATE) }, "starting the Remote sink")?;
        let me = self.clone();
        let mut on_packet = on_packet;
        let reader = std::thread::Builder::new().name("remoted-audio-out".into()).spawn(move || {
            let mut buf = vec![0u8; 1500];
            loop {
                let n = unsafe { ra_sink_read(me.ra, buf.as_mut_ptr(), buf.len() as c_int, 200) };
                if n < 0 {
                    break;
                }
                if n > 0 {
                    on_packet(buf[..n as usize].to_vec());
                }
            }
        })?;
        let sink = Sink { audio: self.clone(), saved, reader: Some(reader) };
        self.wait_node(SINK_NODE, true)?;
        self.make_default(Kind::Sink, SINK_NODE)?;
        Ok(sink)
    }

    /// The "Remote microphone" source, made the default input.
    pub fn start_mic(self: &Arc<Self>) -> Result<Mic> {
        let saved = self.save(Kind::Source)?;
        check(unsafe { ra_mic_start(self.ra, cstr(MIC_NODE).as_ptr(), cstr(MIC_DESC).as_ptr()) }, "starting the Remote microphone")?;
        let mic = Mic { audio: self.clone(), saved };
        self.wait_node(MIC_NODE, true)?;
        self.make_default(Kind::Source, MIC_NODE)?;
        Ok(mic)
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        unsafe { ra_close(self.ra) };
    }
}

pub struct Sink {
    audio: Arc<Audio>,
    pub saved: Saved,
    reader: Option<JoinHandle<()>>,
}

impl Sink {
    /// Defaults back first (streams move to the old sink), then the sink goes, then
    /// the volume check.
    pub fn stop(mut self) -> Result<Restored> {
        let a = &self.audio;
        let configured = a.meta_set(Kind::Sink.configured_key(), self.saved.configured.as_deref());
        unsafe { ra_sink_stop(a.ra) };
        if let Some(t) = self.reader.take() {
            let _ = t.join();
        }
        unsafe { ra_sink_free(a.ra) };
        configured?;
        a.wait_node(SINK_NODE, false)?;
        let mut r = a.restore(&self.saved)?;
        r.configured_back = true;
        Ok(r)
    }
}

pub struct Mic {
    audio: Arc<Audio>,
    pub saved: Saved,
}

impl Mic {
    pub fn write(&self, opus: &[u8]) -> Result<()> {
        check(unsafe { ra_mic_write(self.audio.ra, opus.as_ptr(), opus.len() as c_int) }, "decoding mic audio")?;
        Ok(())
    }

    pub fn stop(self) -> Result<Restored> {
        let a = &self.audio;
        let configured = a.meta_set(Kind::Source.configured_key(), self.saved.configured.as_deref());
        unsafe { ra_mic_stop(a.ra) };
        configured?;
        a.wait_node(MIC_NODE, false)?;
        let mut r = a.restore(&self.saved)?;
        r.configured_back = true;
        Ok(r)
    }
}

/// The saved defaults, kept in the runtime dir while a session holds them, so a
/// remoted that crashed mid-session puts them back when it starts again.
pub fn state_file() -> PathBuf {
    crate::runtime_dir().join("remoted-audio.json")
}

pub fn save_state(saved: &[Saved]) -> Result<()> {
    let path = state_file();
    std::fs::create_dir_all(path.parent().unwrap())?;
    crate::keys::write_private(&path, &serde_json::to_vec(saved)?, 0o600)
}

pub fn clear_state() {
    let _ = std::fs::remove_file(state_file());
}

/// After a crash: the virtual nodes died with the old process; put the defaults back.
pub fn recover(audio: &Audio) -> Result<usize> {
    let Ok(bytes) = std::fs::read(state_file()) else { return Ok(0) };
    let saved: Vec<Saved> = serde_json::from_slice(&bytes).context("reading the audio state file")?;
    for s in &saved {
        audio.restore(s)?;
    }
    clear_state();
    Ok(saved.len())
}
