// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The automatic click-to-photon test. The client clicks on a test
//! surface on the host that flips a marker on every press; the first decoded
//! frame whose marker has flipped is the answer, and its present time (when
//! there is a window) closes the loop. All on the client's own clock.

use crate::stats::{FrameRec, percentile};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct ClickConfig {
    pub count: u32,
    /// The marker in stream pixels: x, y, w, h. The luma is read from its middle half.
    pub marker: (u32, u32, u32, u32),
    /// Random gap between clicks, so clicks sample every phase of the frame grid.
    pub gap_ms: (u64, u64),
    pub timeout_ms: u64,
    /// No clicks for this long first, to measure idle bandwidth.
    pub idle_ms: u64,
    /// Quit once every click is answered (otherwise keep running until --secs or a signal).
    pub quit_when_done: bool,
}

#[derive(Clone, Debug)]
pub struct ClickResult {
    pub id: u32,
    /// Button-down handed to the input channel (client session µs).
    pub t_click: i64,
    pub frame: Option<FrameRec>,
    pub present: Option<i64>,
}

struct Pending {
    id: u32,
    t_click: i64,
    expect: bool,
}

struct Inner {
    marker: Option<bool>,
    luma: f64,
    pending: Option<Pending>,
    awaiting: Vec<ClickResult>,
    done: Vec<ClickResult>,
    sent: u32,
    csv: BufWriter<File>,
    epoch_mono_ns: u64,
}

pub enum Action {
    Wait,
    Click { id: u32 },
    Finished,
}

pub struct ClickTest {
    pub cfg: ClickConfig,
    windowed: bool,
    inner: Mutex<Inner>,
}

impl ClickTest {
    pub fn new(cfg: ClickConfig, windowed: bool, csv_path: &Path, epoch_mono_ns: u64) -> std::io::Result<ClickTest> {
        let mut csv = BufWriter::new(File::create(csv_path)?);
        writeln!(csv, "# epoch_mono_ns={epoch_mono_ns}")?;
        writeln!(
            csv,
            "click,t_click_us,frame_id,host_rendered_us,host_captured_us,host_encoded_us,send_us,first_pkt_us,last_pkt_us,decoded_us,ready_us,present_us,input_to_ready_ms,input_to_present_ms"
        )?;
        let inner = Inner {
            marker: None,
            luma: f64::NAN,
            pending: None,
            awaiting: Vec::new(),
            done: Vec::new(),
            sent: 0,
            csv,
            epoch_mono_ns,
        };
        Ok(ClickTest { cfg, windowed, inner: Mutex::new(inner) })
    }

    /// The point to click, in stream pixels.
    pub fn target(&self) -> (f64, f64) {
        let (x, y, w, h) = self.cfg.marker;
        (x as f64 + w as f64 / 2.0, y as f64 + h as f64 / 2.0)
    }

    /// The luma probe: the marker's middle half.
    pub fn probe(&self) -> (u32, u32, u32, u32) {
        let (x, y, w, h) = self.cfg.marker;
        (x + w / 4, y + h / 4, (w / 2).max(1), (h / 2).max(1))
    }

    /// Decode thread: a picture came out; `luma` is the probe's mean.
    pub fn on_picture(&self, rec: &FrameRec, luma: f64) {
        let mut i = self.inner.lock().unwrap();
        let state = luma > 127.5;
        i.luma = luma;
        i.marker = Some(state);
        if i.pending.as_ref().is_some_and(|p| p.expect == state) {
            let p = i.pending.take().unwrap();
            let r = ClickResult { id: p.id, t_click: p.t_click, frame: Some(rec.clone()), present: None };
            if self.windowed {
                i.awaiting.push(r);
            } else {
                Self::finish(&mut i, r);
            }
        }
    }

    /// Window thread: `frame_id` was presented. A flip shows in every later frame too,
    /// so a click whose own frame was skipped takes the first presented one after it.
    pub fn on_present(&self, frame_id: u32, present_us: i64) {
        let mut i = self.inner.lock().unwrap();
        let (ready, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut i.awaiting)
            .into_iter()
            .partition(|r| r.frame.as_ref().is_some_and(|f| remote_core::wire::seq32_newer(frame_id, f.frame_id) || frame_id == f.frame_id));
        i.awaiting = keep;
        for mut r in ready {
            r.present = Some(present_us);
            Self::finish(&mut i, r);
        }
    }

    fn finish(i: &mut Inner, r: ClickResult) {
        let f = r.frame.clone().unwrap_or_default();
        let ms = |a: i64, b: Option<i64>| b.map_or(String::new(), |b| format!("{:.3}", (b - a) as f64 / 1e3));
        let o = |v: Option<i64>| v.map_or(String::new(), |v| v.to_string());
        let has = r.frame.is_some();
        let _ = writeln!(
            i.csv,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            r.id,
            r.t_click,
            if has { f.frame_id.to_string() } else { String::new() },
            o(f.host_rendered),
            o(f.host_captured),
            o(f.host_encoded),
            o(f.send),
            if has { f.first_pkt.to_string() } else { String::new() },
            if has { f.last_pkt.to_string() } else { String::new() },
            if has { f.decoded.to_string() } else { String::new() },
            if has { f.ready.to_string() } else { String::new() },
            o(r.present),
            ms(r.t_click, has.then_some(f.ready)),
            ms(r.t_click, r.present),
        );
        let _ = i.csv.flush();
        i.done.push(r);
    }

    /// Scheduler: what to do now (`now_us` on the session clock).
    pub fn next(&self, now_us: i64) -> Action {
        let mut i = self.inner.lock().unwrap();
        let timeout = self.cfg.timeout_ms as i64 * 1000;
        if i.pending.as_ref().is_some_and(|p| now_us - p.t_click > timeout) {
            let p = i.pending.take().unwrap();
            eprintln!("remote-client: click {} got no flipped frame in {} ms", p.id, self.cfg.timeout_ms);
            Self::finish(&mut i, ClickResult { id: p.id, t_click: p.t_click, frame: None, present: None });
        }
        let stale: Vec<ClickResult> = {
            let (old, keep): (Vec<_>, Vec<_>) =
                std::mem::take(&mut i.awaiting).into_iter().partition(|r| now_us - r.t_click > timeout);
            i.awaiting = keep;
            old
        };
        for r in stale {
            Self::finish(&mut i, r);
        }
        if i.sent >= self.cfg.count {
            return if i.pending.is_none() && i.awaiting.is_empty() { Action::Finished } else { Action::Wait };
        }
        if i.pending.is_some() || i.marker.is_none() {
            return Action::Wait;
        }
        Action::Click { id: i.sent }
    }

    /// The button-down for click `id` has just been handed to the input channel.
    pub fn clicked(&self, id: u32, t_click: i64) {
        let mut i = self.inner.lock().unwrap();
        let expect = !i.marker.unwrap_or(false);
        i.pending = Some(Pending { id, t_click, expect });
        i.sent += 1;
    }

    pub fn status(&self) -> String {
        let i = self.inner.lock().unwrap();
        let mut p: Vec<f64> = i.done.iter().filter_map(|r| r.present.map(|p| (p - r.t_click) as f64 / 1e3)).collect();
        let mut d: Vec<f64> = i.done.iter().filter_map(|r| r.frame.as_ref().map(|f| (f.ready - r.t_click) as f64 / 1e3)).collect();
        format!(
            "click test {}/{} · input→decoded p50 {:.1} · input→present p50 {:.1} ms · marker {:.0}",
            i.done.len(),
            self.cfg.count,
            percentile(&mut d, 0.5),
            percentile(&mut p, 0.5),
            i.luma
        )
    }

    /// The summary in the shape of the targets table, plus the per-stage
    /// breakdown of the click frames.
    pub fn summary(&self, context: &str, rows: &[(String, String, String)]) -> String {
        let i = self.inner.lock().unwrap();
        let n = i.done.len();
        let got: Vec<&ClickResult> = i.done.iter().filter(|r| r.frame.is_some()).collect();
        let col = |f: &dyn Fn(&ClickResult) -> Option<f64>| -> Vec<f64> { got.iter().filter_map(|r| f(r)).collect() };
        let pp = |mut v: Vec<f64>| -> String {
            if v.is_empty() {
                return "n/a".into();
            }
            let n = v.len();
            format!("{:.1} / {:.1} ms (n={n})", percentile(&mut v, 0.5), percentile(&mut v, 0.95))
        };
        let fr = |r: &ClickResult| r.frame.clone().unwrap();
        let to_present = col(&|r| r.present.map(|p| (p - r.t_click) as f64 / 1e3));
        let to_ready = col(&|r| Some((fr(r).ready - r.t_click) as f64 / 1e3));
        let mut s = String::new();
        s.push_str(&format!("# Click-to-photon, {context}\n\n"));
        s.push_str(&format!("epoch_mono_ns {} · clicks {n}, answered {}\n\n", i.epoch_mono_ns, got.len()));
        s.push_str("| Measure | Target | This run |\n|---|---|---|\n");
        s.push_str(&format!(
            "| Input → present, p50 / p95 | ≤ 35 / ≤ 50 ms at 60 Hz; ≤ 25 ms p50 on the home LAN | {} |\n",
            pp(to_present)
        ));
        s.push_str(&format!("| Input → decode done, p50 / p95 | (no target; present minus the vsync wait) | {} |\n", pp(to_ready)));
        for (a, b, c) in rows {
            s.push_str(&format!("| {a} | {b} | {c} |\n"));
        }
        s.push_str("\n**Where the time goes (click frames, p50 / p95):**\n\n| Stage | ms |\n|---|---|\n");
        let stage = |name: &str, f: &dyn Fn(&ClickResult) -> Option<f64>| format!("| {name} | {} |\n", pp(col(f)));
        s.push_str(&stage("click sent → host frame rendered (input, frame wait, app + compositor)", &|r| {
            fr(r).host_rendered.map(|h| (h - r.t_click) as f64 / 1e3)
        }));
        s.push_str(&stage("rendered → captured", &|r| {
            let f = fr(r);
            Some((f.host_captured? - f.host_rendered?) as f64 / 1e3)
        }));
        s.push_str(&stage("captured → encoded", &|r| {
            let f = fr(r);
            Some((f.host_encoded? - f.host_captured?) as f64 / 1e3)
        }));
        s.push_str(&stage("encoded → sent", &|r| {
            let f = fr(r);
            Some((f.send? - f.host_encoded?) as f64 / 1e3)
        }));
        s.push_str(&stage("sent → last packet (pacing + network)", &|r| {
            let f = fr(r);
            Some((f.last_pkt - f.send?) as f64 / 1e3)
        }));
        s.push_str(&stage("last packet → decoded", &|r| {
            let f = fr(r);
            Some((f.decoded - f.last_pkt) as f64 / 1e3)
        }));
        s.push_str(&stage("decoded → in system memory for the presenter", &|r| {
            let f = fr(r);
            Some((f.ready - f.decoded) as f64 / 1e3)
        }));
        s.push_str(&stage("ready → presented (draw, commit, compositor)", &|r| r.present.map(|p| (p - fr(r).ready) as f64 / 1e3)));
        s
    }
}
