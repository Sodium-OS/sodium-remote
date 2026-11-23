// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Per-frame timestamps, the session's stats file and the HUD text.
//!
//! Every time is on the client's clock (µs since its epoch, just before
//! `connect`; `SessionClock` maps it to CLOCK_MONOTONIC). The core's session
//! times are converted to it per connection (`Live::offset_us`), and the host's
//! stamps with the clock-sync offset carried by each frame (`send_us` against
//! `send_local_us`).

use crate::connect::SessionClock;
use crate::proto::{Codec, HostStamps, StreamInfo};
use remote_core::SessionStats;
use remote_core::connect::{PathKind, RouteInfo};
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One frame's life on the client, plus the host's stages when it sent them.
#[derive(Clone, Debug, Default)]
pub struct FrameRec {
    pub frame_id: u32,
    pub keyframe: bool,
    pub bytes: usize,
    pub host_rendered: Option<i64>,
    pub host_captured: Option<i64>,
    pub host_converted: Option<i64>,
    pub host_encoded: Option<i64>,
    pub send: Option<i64>,
    pub first_pkt: i64,
    pub last_pkt: i64,
    /// Handed to the decoder.
    pub decode_start: i64,
    /// The decoder gave the picture back.
    pub decoded: i64,
    /// The picture is in system memory, ready for the presenter.
    pub ready: i64,
    pub transfer_ms: f64,
    pub commit: Option<i64>,
    pub present: Option<i64>,
    /// Never shown: a newer frame won, or the compositor discarded it.
    pub skipped: bool,
}

impl FrameRec {
    /// Host stamps on our clock. `offset_us` is host minus client.
    pub fn set_host(&mut self, s: &HostStamps, offset_us: i64) {
        let conv = |v: u64| (v != 0).then(|| v as i64 - offset_us);
        self.host_rendered = conv(s.rendered_us);
        self.host_captured = conv(s.captured_us);
        self.host_converted = conv(s.converted_us);
        self.host_encoded = conv(s.encoded_us);
    }
}

/// Stage name and its endpoints (ms between them).
type Stage = (&'static str, fn(&FrameRec) -> Option<(i64, i64)>);

pub const STAGES: &[Stage] = &[
    ("host render→capture", |f| Some((f.host_rendered?, f.host_captured?))),
    ("host capture→encoded", |f| Some((f.host_captured?, f.host_encoded?))),
    ("host encoded→sent", |f| Some((f.host_encoded?, f.send?))),
    ("network (sent→last packet)", |f| Some((f.send?, f.last_pkt))),
    ("decode", |f| Some((f.last_pkt, f.decoded))),
    ("copy to presenter", |f| Some((f.decoded, f.ready))),
    ("wait for present", |f| Some((f.ready, f.present?))),
    ("host render→decoded", |f| Some((f.host_rendered?, f.ready))),
    ("host render→present", |f| Some((f.host_rendered?, f.present?))),
];

fn opt(v: Option<i64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

pub fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[derive(Default)]
struct Inner {
    pending: BTreeMap<u32, FrameRec>,
    csv: Option<BufWriter<File>>,
    stages: Vec<Vec<f64>>,
    frames_decoded: u64,
    frames_presented: u64,
    frames_skipped: u64,
    frames_lost: u64,
    bytes: u64,
    first_frame_us: Option<i64>,
    connected_us: Option<i64>,
    connect_start_us: Option<i64>,
    backend: String,
    stream: Option<StreamInfo>,
    route: String,
    session: SessionStats,
    /// When `session` arrived (client µs).
    session_at: i64,
    /// (ready µs, bytes) of the last second, for the HUD.
    recent: VecDeque<(i64, usize)>,
    recent_present: VecDeque<i64>,
    /// The last second's frames, for per-stage HUD medians.
    recent_recs: VecDeque<FrameRec>,
    present_intervals: Vec<f64>,
    last_present: Option<i64>,
    refresh_ns: u32,
    decode_errors: u64,
    frames_undecodable: u64,
    host: Option<crate::msg::HostStats>,
}

/// What `Stats::hud_numbers` returns.
#[derive(Clone, Debug)]
pub struct HudNumbers {
    pub rtt_ms: f64,
    pub loss_pct: f64,
    pub mbps: f64,
    pub fps: u32,
    pub presented: u32,
    pub decoder: String,
    pub stream: Option<StreamInfo>,
}

pub struct Stats {
    inner: Mutex<Inner>,
    pub clock: SessionClock,
    pub windowed: bool,
    dir: PathBuf,
    pub session_name: String,
}

const CSV_HEADER: &str = "frame_id,keyframe,bytes,host_rendered_us,host_captured_us,host_converted_us,host_encoded_us,send_us,first_pkt_us,last_pkt_us,decode_start_us,decoded_us,ready_us,transfer_ms,commit_us,present_us,skipped";

pub fn route_name(r: &RouteInfo) -> String {
    match &r.kind {
        PathKind::Relay { transport } => format!("relay {transport:?}").to_lowercase(),
        k => crate::status::kind_name(k).into(),
    }
}

impl Stats {
    pub fn new(dir: &Path, clock: SessionClock, windowed: bool) -> std::io::Result<Stats> {
        std::fs::create_dir_all(dir)?;
        let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        let session_name = format!("session-{unix}");
        let mut csv = BufWriter::new(File::create(dir.join(format!("{session_name}-frames.csv")))?);
        writeln!(csv, "# epoch_mono_ns={} (add us*1000 for CLOCK_MONOTONIC)", clock.epoch_mono_ns)?;
        writeln!(csv, "{CSV_HEADER}")?;
        let inner = Inner { csv: Some(csv), stages: vec![Vec::new(); STAGES.len()], route: "none".into(), ..Default::default() };
        Ok(Stats { inner: Mutex::new(inner), clock, windowed, dir: dir.to_path_buf(), session_name })
    }

    pub fn path(&self, suffix: &str) -> PathBuf {
        self.dir.join(format!("{}-{suffix}", self.session_name))
    }

    /// `Client::connect` is being called (the connect times count from here).
    pub fn on_connect_start(&self) {
        let mut i = self.inner.lock().unwrap();
        i.connect_start_us.get_or_insert(self.clock.now_us() as i64);
    }

    /// A connection came up. Frames of an earlier one never shown are skipped
    /// (frame ids start over with every session).
    pub fn on_connected(&self) {
        let mut i = self.inner.lock().unwrap();
        i.connected_us.get_or_insert(self.clock.now_us() as i64);
        for (_, mut r) in std::mem::take(&mut i.pending) {
            r.skipped = r.present.is_none();
            Self::finish_rec(&mut i, r);
        }
    }

    pub fn on_route(&self, r: &RouteInfo) {
        self.inner.lock().unwrap().route = route_name(r);
    }

    pub fn on_session_stats(&self, s: SessionStats) {
        let mut i = self.inner.lock().unwrap();
        i.session = s;
        i.session_at = self.clock.now_us() as i64;
    }

    /// Bytes received on the session so far (every channel) and when the core
    /// counted them (client µs; about once a second).
    pub fn bytes_received(&self) -> (u64, i64) {
        let i = self.inner.lock().unwrap();
        (i.session.bytes_received, i.session_at)
    }

    /// (connected, first frame ready), µs after `connect` was called.
    pub fn connect_times(&self) -> (Option<i64>, Option<i64>) {
        let i = self.inner.lock().unwrap();
        let t0 = i.connect_start_us.unwrap_or(0);
        (i.connected_us.map(|t| t - t0), i.first_frame_us.map(|t| t - t0))
    }

    /// A frame the network couldn't make decodable (its chain was broken by a loss).
    pub fn on_undecodable(&self) {
        self.inner.lock().unwrap().frames_undecodable += 1;
    }

    pub fn on_host_stats(&self, h: crate::msg::HostStats) {
        self.inner.lock().unwrap().host = Some(h);
    }

    /// The last second's numbers for the host (`msg::ClientStats`).
    pub fn client_stats(&self) -> Option<crate::msg::ClientStats> {
        let i = self.inner.lock().unwrap();
        let med = |n: usize| {
            let mut v: Vec<f64> = i.recent_recs.iter().filter_map(|r| STAGES[n].1(r)).map(|(a, b)| (b - a) as f64).collect();
            percentile(&mut v, 0.5)
        };
        let decode = med(4) + med(5).max(0.0);
        let present = med(6);
        (!i.recent.is_empty()).then(|| crate::msg::ClientStats {
            decode_us: if decode.is_finite() { decode as u32 } else { 0 },
            present_us: if present.is_finite() { present as u32 } else { 0 },
            fps: if self.windowed { i.recent_present.len() } else { i.recent.len() } as f32,
            frames_late: 0,
            frames_dropped: (i.frames_skipped + i.frames_lost + i.frames_undecodable) as u32,
        })
    }

    pub fn on_lost(&self) {
        self.inner.lock().unwrap().frames_lost += 1;
    }

    pub fn on_decode_error(&self) {
        self.inner.lock().unwrap().decode_errors += 1;
    }

    pub fn set_decoder(&self, backend: &str, stream: Option<StreamInfo>) {
        let mut i = self.inner.lock().unwrap();
        i.backend = backend.to_string();
        if stream.is_some() {
            i.stream = stream;
        }
    }

    /// A frame is decoded and handed to the presenter (or, headless, done).
    pub fn on_decoded(&self, rec: FrameRec) {
        let mut i = self.inner.lock().unwrap();
        i.frames_decoded += 1;
        i.bytes += rec.bytes as u64;
        i.first_frame_us.get_or_insert(rec.ready);
        i.recent.push_back((rec.ready, rec.bytes));
        while i.recent.front().is_some_and(|r| rec.ready - r.0 > 1_000_000) {
            i.recent.pop_front();
        }
        if self.windowed {
            i.pending.insert(rec.frame_id, rec);
        } else {
            Self::finish_rec(&mut i, rec);
        }
    }

    /// The presenter committed this frame; older frames it never showed are skipped.
    pub fn on_commit(&self, frame_id: u32, commit_us: i64) {
        let mut i = self.inner.lock().unwrap();
        let older: Vec<u32> = i.pending.range(..frame_id).filter(|(_, r)| r.commit.is_none()).map(|(k, _)| *k).collect();
        for k in older {
            if let Some(mut r) = i.pending.remove(&k) {
                r.skipped = true;
                Self::finish_rec(&mut i, r);
            }
        }
        if let Some(r) = i.pending.get_mut(&frame_id) {
            r.commit = Some(commit_us);
        }
    }

    /// Returns (decode µs, present µs) for core's receiver report (last packet →
    /// ready, ready → presented) and the frame's ready time (client µs).
    pub fn on_present(&self, frame_id: Option<u32>, present_us: i64, refresh_ns: u32) -> Option<(u32, u32, i64)> {
        let mut i = self.inner.lock().unwrap();
        i.refresh_ns = refresh_ns;
        let id = frame_id?;
        if let Some(mut r) = i.pending.remove(&id) {
            r.present = Some(present_us);
            if let Some(l) = i.last_present {
                let d = (present_us - l) as f64 / 1e3;
                i.present_intervals.push(d);
            }
            i.last_present = Some(present_us);
            i.frames_presented += 1;
            i.recent_present.push_back(present_us);
            while i.recent_present.front().is_some_and(|t| present_us - t > 1_000_000) {
                i.recent_present.pop_front();
            }
            let report = ((r.ready - r.last_pkt).max(0) as u32, (present_us - r.ready).max(0) as u32, r.ready);
            Self::finish_rec(&mut i, r);
            return Some(report);
        }
        None
    }

    pub fn on_discard(&self, frame_id: Option<u32>) {
        let mut i = self.inner.lock().unwrap();
        if let Some(mut r) = frame_id.and_then(|id| i.pending.remove(&id)) {
            r.skipped = true;
            Self::finish_rec(&mut i, r);
        }
    }

    fn finish_rec(i: &mut Inner, r: FrameRec) {
        if r.skipped {
            i.frames_skipped += 1;
        }
        for (n, (_, f)) in STAGES.iter().enumerate() {
            if let Some((a, b)) = f(&r) {
                i.stages[n].push((b - a) as f64 / 1e3);
            }
        }
        if let Some(csv) = i.csv.as_mut() {
            let _ = writeln!(
                csv,
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{:.3},{},{},{}",
                r.frame_id,
                r.keyframe as u8,
                r.bytes,
                opt(r.host_rendered),
                opt(r.host_captured),
                opt(r.host_converted),
                opt(r.host_encoded),
                opt(r.send),
                r.first_pkt,
                r.last_pkt,
                r.decode_start,
                r.decoded,
                r.ready,
                r.transfer_ms,
                opt(r.commit),
                opt(r.present),
                r.skipped as u8
            );
        }
        let now = r.ready;
        i.recent_recs.push_back(r);
        while i.recent_recs.front().is_some_and(|f| now - f.ready > 1_000_000) {
            i.recent_recs.pop_front();
        }
    }

    /// The HUD's lines (PROVISIONAL: the Mac's stats HUD is designed in
    /// the Mac app's; this is the reference client's plain text version).
    /// The HUD's numbers for an app that draws its own: round-trip ms, loss %, received Mbit/s, decoded and
    /// presented frames in the last second, and the decoder's name.
    pub fn hud_numbers(&self) -> HudNumbers {
        let i = self.inner.lock().unwrap();
        HudNumbers {
            rtt_ms: i.session.rtt_ms,
            loss_pct: i.session.loss * 100.0,
            mbps: i.recent.iter().map(|r| r.1).sum::<usize>() as f64 * 8.0 / 1e6,
            fps: i.recent.len() as u32,
            presented: i.recent_present.len() as u32,
            decoder: i.backend.clone(),
            stream: i.stream,
        }
    }

    pub fn hud_lines(&self, extra: &[String]) -> Vec<String> {
        let i = self.inner.lock().unwrap();
        let s = &i.session;
        let mut lines = Vec::new();
        lines.push(format!(
            "Remote · {} · RTT {:.1} ms · loss {:.2}% · FEC {:.0}% · MTU {}",
            i.route,
            s.rtt_ms,
            s.loss * 100.0,
            s.fec_ratio * 100.0,
            s.mtu
        ));
        let codec = i.stream.map_or("?".to_string(), |st| {
            format!(
                "{} {} {}×{}@{:.0}",
                match st.codec {
                    Codec::Hevc => "HEVC",
                    Codec::Av1 => "AV1",
                    Codec::H264 => "H.264",
                },
                if st.chroma444 { "4:4:4" } else { "4:2:0" },
                st.width,
                st.height,
                st.refresh_mhz as f64 / 1000.0
            )
        });
        let mbps = i.recent.iter().map(|r| r.1).sum::<usize>() as f64 * 8.0 / 1e6;
        lines.push(format!(
            "{codec} · {} · {} fps decoded · {} presented · {mbps:.1} Mbit/s",
            if i.backend.is_empty() { "no decoder" } else { &i.backend },
            i.recent.len(),
            i.recent_present.len(),
        ));
        let med = |n: usize| {
            let mut v: Vec<f64> = i.recent_recs.iter().filter_map(|r| STAGES[n].1(r)).map(|(a, b)| (b - a) as f64 / 1e3).collect();
            percentile(&mut v, 0.5)
        };
        let f = |v: f64| if v.is_finite() { format!("{v:.1}") } else { "–".into() };
        lines.push(format!(
            "host {} + {} + {} · net {} · decode {} + {} · present {} ms",
            f(med(0)),
            f(med(1)),
            f(med(2)),
            f(med(3)),
            f(med(4)),
            f(med(5)),
            f(med(6))
        ));
        lines.push(format!(
            "render→present {} ms · lost {} · undecodable {} · skipped {} · errors {}",
            f(med(8)),
            i.frames_lost,
            i.frames_undecodable,
            i.frames_skipped,
            i.decode_errors
        ));
        if let Some(h) = &i.host {
            lines.push(format!(
                "host says: capture {:.1} · convert {:.1} · encode {:.1} · send {:.1} ms · {:.0} fps · {:.1} Mbit/s · QP {}",
                h.capture_us as f64 / 1e3,
                h.convert_us as f64 / 1e3,
                h.encode_us as f64 / 1e3,
                h.send_us as f64 / 1e3,
                h.fps,
                h.bitrate_bps as f64 / 1e6,
                h.qp
            ));
            if h.settled_ms > 0 {
                // the latest change ended parked or at the cap, not at the stop
                lines.push(format!(
                    "text: readable {} ms · settled {:.1} s ({})",
                    h.readable_ms,
                    h.settled_ms as f64 / 1e3,
                    if h.settled_end == 2 { "cap" } else { "parked" }
                ));
            } else if h.readable_ms > 0 {
                lines.push(format!("text: readable {} ms · lossless {} ms", h.readable_ms, h.lossless_ms));
            }
            if h.target_bps > 0 {
                let mut l = format!("link: target {:.1} Mbit/s", h.target_bps as f64 / 1e6);
                if h.c_est_bps > 0 {
                    l.push_str(&format!(" · C_est {:.1} Mbit/s", h.c_est_bps as f64 / 1e6));
                }
                if h.probe_ceiling_bps > 0 {
                    l.push_str(&format!(" · ceiling {:.1} Mbit/s", h.probe_ceiling_bps as f64 / 1e6));
                }
                lines.push(l);
            }
        }
        lines.extend(extra.iter().cloned());
        lines
    }

    /// Flushes the per-frame CSV and writes the session summary (JSON). Returns its text.
    pub fn finish(&self, extra: &[(String, String)]) -> String {
        let mut i = self.inner.lock().unwrap();
        let pending: Vec<FrameRec> = std::mem::take(&mut i.pending).into_values().collect();
        for mut r in pending {
            r.skipped = r.present.is_none();
            Self::finish_rec(&mut i, r);
        }
        if let Some(mut c) = i.csv.take() {
            let _ = c.flush();
        }
        let mut stages = Vec::new();
        for (n, (name, _)) in STAGES.iter().enumerate() {
            let mut v = i.stages[n].clone();
            if v.is_empty() {
                continue;
            }
            stages.push(format!(
                "\"{name}\": {{\"n\": {}, \"p50\": {:.3}, \"p95\": {:.3}, \"max\": {:.3}}}",
                v.len(),
                percentile(&mut v, 0.5),
                percentile(&mut v, 0.95),
                percentile(&mut v, 1.0)
            ));
        }
        let mut iv = i.present_intervals.clone();
        let refresh_ms = i.refresh_ns as f64 / 1e6;
        let late = if refresh_ms > 0.0 { iv.iter().filter(|&&d| d > 1.5 * refresh_ms).count() } else { 0 };
        let s = &i.session;
        let (ht, hc, hp) = i.host.as_ref().map_or((0, 0, 0), |h| (h.target_bps, h.c_est_bps, h.probe_ceiling_bps));
        let mut out = format!(
            "{{\n  \"session\": \"{}\",\n  \"epoch_mono_ns\": {},\n  \"decoder\": \"{}\",\n  \"stream\": \"{:?}\",\n  \"route\": \"{}\",\n  \"connect_start_us\": {},\n  \"connected_us\": {},\n  \"first_frame_us\": {},\n  \"frames_decoded\": {},\n  \"frames_presented\": {},\n  \"frames_skipped\": {},\n  \"frames_lost\": {},\n  \"decode_errors\": {},\n  \"bytes\": {},\n  \"rtt_ms\": {:.3},\n  \"loss\": {:.5},\n  \"mtu\": {},\n  \"present_interval_ms_p50\": {:.3},\n  \"present_intervals_over_1.5_refresh\": {},\n  \"refresh_ms\": {:.3},\n  \"host_target_bps\": {},\n  \"host_c_est_bps\": {},\n  \"host_probe_ceiling_bps\": {},\n  \"stages_ms\": {{\n    {}\n  }}",
            self.session_name,
            self.clock.epoch_mono_ns,
            i.backend,
            i.stream,
            i.route,
            i.connect_start_us.map_or("null".into(), |v| v.to_string()),
            i.connected_us.map_or("null".into(), |v| v.to_string()),
            i.first_frame_us.map_or("null".into(), |v| v.to_string()),
            i.frames_decoded,
            i.frames_presented,
            i.frames_skipped,
            i.frames_lost,
            i.decode_errors,
            i.bytes,
            s.rtt_ms,
            s.loss,
            s.mtu,
            percentile(&mut iv, 0.5),
            late,
            refresh_ms,
            ht,
            hc,
            hp,
            stages.join(",\n    ")
        );
        for (k, v) in extra {
            out.push_str(&format!(",\n  \"{k}\": {v}"));
        }
        out.push_str("\n}\n");
        let _ = std::fs::write(self.path("summary.json"), &out);
        out
    }

    /// Stage percentiles for the latency summary: (name, p50, p95, n).
    pub fn stage_table(&self) -> Vec<(&'static str, f64, f64, usize)> {
        let i = self.inner.lock().unwrap();
        STAGES
            .iter()
            .enumerate()
            .map(|(n, (name, _))| {
                let mut v = i.stages[n].clone();
                (*name, percentile(&mut v, 0.5), percentile(&mut v, 0.95), v.len())
            })
            .collect()
    }
}
