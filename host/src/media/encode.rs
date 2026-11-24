// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! media-enc: Vulkan import and conversion, NVENC, and the per-frame decisions:
//! change detection → QP map, refinement frames, motion hints, rate and split,
//! reference invalidation.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        atomic::Ordering,
        mpsc::{Receiver, RecvTimeoutError, SyncSender},
    },
    time::Duration,
};

use anyhow::{Context, Result};

use super::{
    Codec, EncodedFrame, EventSink, FrameClass, FrameInfo, INTERACTIVE_AFTER_INPUT_NS, MediaConfig, MediaEvent, RateInput,
    Shared,
    grid::{Grid, Motion, Pick, RefineConfig, Settled, motion_field},
    ipc::{HINT_ANIM_SETTLE, HINT_ANIM_START, HINT_SCROLL, HINT_WINDOW_MOVE, Hint},
    rate::{self, RatePlan, Steady},
    wire,
};
use crate::{
    capture::{Dmabuf, Frame, Rect, monotonic_ns, plugin},
    gpu::{Gpu, OutBuffer, OutFormat, Readback, SrcImage},
    nvenc::{self, Cuda, EncParams, Encoder, ExtMem, PicOpts},
};

pub(crate) enum ToEnc {
    Buffers(u32, Vec<Dmabuf>),
    Frame(Frame),
    Hint(Hint),
    PredictFrom(u64),
    Keyframe,
    Stop,
}

/// NVENC's DPB (maxNumRefFramesInDPB in the shim): a reference this recent is used
/// through invalidation; an older one gets an IDR (integration.md 2: NVDEC drops every
/// frame of an intra refresh whose references are missing, so a refresh never recovers).
const DPB: u64 = 8;
/// The intra refresh's length. The code stays, unused: nothing starts a refresh now.
const IR_FRAMES: u32 = 8;
/// An animation that never says it settled stops counting as one after this.
const ANIM_TIMEOUT_NS: u64 = 3_000_000_000;
const HISTORY: usize = 32;

/// How a loss is repaired (integration.md 2).
#[derive(Debug, PartialEq, Eq)]
enum Recovery {
    /// The next frame predicts from the client's last good frame, which NVENC still holds.
    Invalidate,
    /// That frame is from before the stream's keyframe or has left the DPB: a keyframe.
    Idr,
}

/// What to do for a client that has frame `p` and nothing usable after it, with frames
/// up to `end - 1` sent and the stream's keyframe at `last_idr_pts`.
fn recovery(p: u64, end: u64, last_idr_pts: u64) -> Recovery {
    if p < last_idr_pts || end - p > DPB { Recovery::Idr } else { Recovery::Invalidate }
}

/// The bytes of a bitstream's slice NAL units (HEVC) or frame / tile-group OBUs (AV1), parameter
/// sets and headers excluded. NVENC's output for a frame with nothing to code is those and a
/// slice of a few dozen bytes whatever the frame's size (CABAC packs the skip flags), so this
/// is what tells an empty refinement pass from one that coded something.
fn slice_bytes(data: &[u8], av1: bool) -> usize {
    if av1 {
        // low-overhead OBUs: header (type, extension flag, has-size flag), [extension], [leb128 size]
        let (mut i, mut n) = (0, 0);
        while i < data.len() {
            let h = data[i];
            let (ty, ext, has_size) = ((h >> 3) & 0xf, (h >> 2) & 1 != 0, (h >> 1) & 1 != 0);
            let mut j = i + 1 + ext as usize;
            let end = if has_size {
                let (mut size, mut shift) = (0usize, 0);
                while j < data.len() {
                    let b = data[j];
                    j += 1;
                    size |= ((b & 0x7f) as usize) << shift;
                    shift += 7;
                    if b & 0x80 == 0 || shift > 28 {
                        break;
                    }
                }
                (j + size).min(data.len())
            } else {
                data.len()
            };
            if ty == 4 || ty == 6 {
                n += end - i;
            }
            i = end.max(i + 1);
        }
        return n;
    }
    // Annex B: start codes 00 00 01; a NAL ends at the next one (less the zero that may lead a
    // four-byte code). VCL NAL types are below 32.
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut n = 0;
    for (k, &b) in starts.iter().enumerate() {
        let mut end = starts.get(k + 1).map_or(data.len(), |&e| e - 3);
        while end > b && data[end - 1] == 0 {
            end -= 1;
        }
        if b < data.len() && (data[b] >> 1) & 0x3f < 32 {
            n += end - b;
        }
    }
    n
}

/// A frame that refined `refined` cells is empty when its slice is the empty slice's size
/// (measured for the stream) plus 16 bytes and half a byte a refined cell: the mixed QP deltas of a
/// refinement map cost a little even when nothing is coded. Any real coding of a cell costs far more.
/// A wrong "empty" leaves cells one pass softer until their next change; a wrong "not empty" costs
/// one more small frame.
fn is_empty(slice: usize, empty_slice: usize, refined: usize) -> bool {
    slice <= empty_slice + 16 + refined / 2
}

struct Encoding {
    enc: Encoder,
    /// The slice bytes of a frame with nothing to code, measured at start for this stream's cell count.
    empty_slice: usize,
    outs: Vec<(OutBuffer, ExtMem, i32)>,
    /// Per-cell change flags from the conversion shader.
    flags: Readback,
    width: u32,
    height: u32,
    grid: Grid,
    qp_scale: i32,
    mv_block: u32,
    applied: RatePlan,
    /// The rate controller's QP ceiling in force (0: none), the one the last capture frame had,
    /// and that frame.
    max_qp: u32,
    held: u32,
    last: Option<Steady>,
}

struct State {
    cfg: MediaConfig,
    shared: Arc<Shared>,
    sink: EventSink,
    gpu: Gpu,
    cuda: Cuda,
    generation: Option<u32>,
    images: Vec<SrcImage>,
    enc: Option<Encoding>,
    interval_ns: u64,
    /// The output slot holding the newest converted capture, and that capture.
    cur_slot: Option<usize>,
    cur_rendered_ns: u64,
    cur_seq: u64,
    last_seq: Option<u64>,
    next_pts: u64,
    last_encode_ns: u64,
    /// No refinement frame before this (the interval's budget was used up).
    refine_wait_ns: u64,
    /// At most one refinement frame per frame interval.
    last_refine_ns: u64,
    force_idr: bool,
    last_idr_pts: u64,
    start_ir: bool,
    ir_left: u32,
    /// The frames [a, b) invalidated last.
    invalid_range: Option<(u64, u64)>,
    /// The next frame predicts from this one (reference invalidation).
    reference: Option<u64>,
    invalidated: u32,
    /// What each recent frame changed, to send it again if the client lost it.
    history: VecDeque<(u64, Vec<Rect>)>,
    redamage: Vec<Rect>,
    pending: Vec<Frame>,
    motion: Vec<Motion>,
    motion_seq: Option<u64>,
    anim_since: Option<u64>,
    settles: Vec<(u64, Rect)>,
    qpmap: Vec<i8>,
    mv: Vec<[i16; 2]>,
    mv_valid: Vec<u8>,
    dump: Option<(PathBuf, Readback)>,
    /// The capture whose input was dumped (once, at its first refinement frame).
    dumped_seq: Option<u64>,
    /// (bitstream ready, bytes) of the last frames, for the interval's refinement budget.
    sent: VecDeque<(u64, usize)>,
}

pub(super) fn run(cfg: MediaConfig, shared: Arc<Shared>, rx: Receiver<ToEnc>, sink: EventSink, ready: SyncSender<Result<()>>) {
    let mut st = match State::new(cfg, shared.clone(), sink.clone()) {
        Ok(s) => {
            let _ = ready.send(Ok(()));
            s
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    if let Err(e) = st.run(rx) {
        if !shared.stopping.load(Ordering::SeqCst) {
            sink(MediaEvent::Stopped(format!("media encoder: {e:#}")));
        }
    }
    st.cleanup();
}

impl State {
    fn new(mut cfg: MediaConfig, shared: Arc<Shared>, sink: EventSink) -> Result<Self> {
        anyhow::ensure!(!(cfg.codec == Codec::Av1 && cfg.chroma444), "AV1 on NVENC is 4:2:0 only");
        if cfg.refine.max_passes == 0 {
            cfg.refine.max_passes = cfg.fps.clamp(1, 255) as u8; // a second of refinement frames
        }
        let gpu = Gpu::new()?;
        let cuda = Cuda::open(&gpu.uuid)?;
        if let Some(d) = &cfg.dump {
            std::fs::create_dir_all(d)?;
        }
        Ok(State {
            interval_ns: 1_000_000_000 / cfg.fps.max(1) as u64,
            cfg,
            shared,
            sink,
            gpu,
            cuda,
            generation: None,
            images: Vec::new(),
            enc: None,
            cur_slot: None,
            cur_rendered_ns: 0,
            cur_seq: 0,
            last_seq: None,
            next_pts: 0,
            last_encode_ns: 0,
            refine_wait_ns: 0,
            last_refine_ns: 0,
            force_idr: true,
            last_idr_pts: 0,
            start_ir: false,
            ir_left: 0,
            reference: None,
            invalidated: 0,
            invalid_range: None,
            history: VecDeque::new(),
            redamage: Vec::new(),
            pending: Vec::new(),
            motion: Vec::new(),
            motion_seq: None,
            anim_since: None,
            settles: Vec::new(),
            qpmap: Vec::new(),
            mv: Vec::new(),
            mv_valid: Vec::new(),
            dump: None,
            dumped_seq: None,
            sent: VecDeque::new(),
        })
    }

    fn run(&mut self, rx: Receiver<ToEnc>) -> Result<()> {
        loop {
            let msg = match self.deadline() {
                None => match rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return Ok(()),
                },
                Some(d) => {
                    let now = monotonic_ns();
                    if d <= now {
                        rx.try_recv().ok()
                    } else {
                        match rx.recv_timeout(Duration::from_nanos(d - now)) {
                            Ok(m) => Some(m),
                            Err(RecvTimeoutError::Timeout) => None,
                            Err(RecvTimeoutError::Disconnected) => return Ok(()),
                        }
                    }
                }
            };
            if let Some(m) = msg {
                if !self.handle(m)? {
                    return Ok(());
                }
            }
            while let Ok(m) = rx.try_recv() {
                if !self.handle(m)? {
                    return Ok(());
                }
            }
            if !self.pending.is_empty() {
                self.encode_capture()?;
            } else if self.deadline().is_some_and(|d| d <= monotonic_ns()) {
                self.encode_refine()?;
            }
        }
    }

    /// When the loop must act without a message: now for a pending capture, else the
    /// next refinement (at most one refinement frame per half frame interval).
    fn deadline(&self) -> Option<u64> {
        if !self.pending.is_empty() {
            return Some(0);
        }
        if !self.cfg.refinement || self.cur_slot.is_none() {
            return None;
        }
        let e = self.enc.as_ref()?;
        let due = e.grid.next_due(&self.cfg.refine)?;
        Some(
            due.max(self.last_encode_ns + self.interval_ns / 2)
                .max(self.last_refine_ns + self.interval_ns)
                .max(self.refine_wait_ns),
        )
    }

    fn handle(&mut self, m: ToEnc) -> Result<bool> {
        match m {
            ToEnc::Buffers(g, bufs) => self.on_buffers(g, bufs)?,
            ToEnc::Frame(f) => {
                if Some(f.generation) == self.generation && (f.buffer as usize) < self.images.len() {
                    self.pending.push(f);
                } else {
                    self.release(&f);
                }
            }
            ToEnc::Hint(h) => self.on_hint(h),
            ToEnc::PredictFrom(pts) => self.predict_from(pts)?,
            ToEnc::Keyframe => self.force_idr = true,
            ToEnc::Stop => return Ok(false),
        }
        Ok(true)
    }

    fn release(&self, f: &Frame) {
        let _ = plugin::send(&self.shared.sock, plugin::RELEASE, &plugin::release_payload(f.generation, f.buffer), &[]);
    }

    fn on_buffers(&mut self, g: u32, bufs: Vec<Dmabuf>) -> Result<()> {
        for img in self.images.drain(..) {
            self.gpu.destroy_src(img);
        }
        for f in std::mem::take(&mut self.pending) {
            self.release(&f);
        }
        for b in &bufs {
            self.images.push(self.gpu.import(b).context("importing the plugin's buffers")?);
        }
        self.generation = Some(g);
        let (w, h) = (bufs[0].width, bufs[0].height);
        if self.enc.as_ref().is_none_or(|e| (e.width, e.height) != (w, h)) {
            self.close_encoder();
            self.open_encoder(w, h)?;
        }
        Ok(())
    }

    fn rate_input(&self) -> RateInput {
        if let Some(src) = self.shared.rate_source.lock().unwrap().clone() {
            return src();
        }
        self.shared.rate.lock().unwrap().clone().unwrap_or_else(|| RateInput::fixed(self.cfg.start_bps))
    }

    fn open_encoder(&mut self, width: u32, height: u32) -> Result<()> {
        let format = if self.cfg.chroma444 { OutFormat::Yuv444 } else { OutFormat::Nv12 };
        let plan = rate::plan(&self.rate_input(), FrameClass::Interactive, self.cfg.fps).unwrap();
        let p = EncParams {
            codec: if self.cfg.codec == Codec::Av1 { nvenc::CODEC_AV1 } else { nvenc::CODEC_HEVC },
            input: if self.cfg.chroma444 { nvenc::INPUT_YUV444 } else { nvenc::INPUT_NV12 },
            chroma444: self.cfg.chroma444 as i32,
            width,
            height,
            fps: self.cfg.fps,
            kbps: plan.kbps,
            vbv_pct: plan.vbv_pct,
            split: if plan.split { nvenc::SPLIT_TWO_FORCED } else { nvenc::SPLIT_OFF },
        };
        let mut enc = Encoder::open_with(&self.cuda, &p, true)?;
        let mut outs = Vec::new();
        for _ in 0..2 {
            let out = self.gpu.create_out(width, height, format)?;
            let mem = self.cuda.import(self.gpu.export_fd(&out)?, out.alloc_size)?;
            let slot = enc.register(mem.devptr, out.pitch)?;
            outs.push((out, mem, slot));
        }
        let (cell, mv_block) = enc.grid();
        if let Some(dir) = self.cfg.dump.clone() {
            if let Some((_, rb)) = self.dump.take() {
                self.gpu.destroy_readback(rb);
            }
            let o = &outs[0].0;
            std::fs::write(
                dir.join("layout.json"),
                format!(
                    "{{\"width\": {width}, \"height\": {height}, \"pitch\": {}, \"off_u\": {}, \"off_v\": {}, \"format\": \"{}\"}}\n",
                    o.pitch,
                    o.off_u,
                    o.off_v,
                    if self.cfg.chroma444 { "yuv444" } else { "nv12" }
                ),
            )?;
            self.dump = Some((dir, self.gpu.create_readback(o.alloc_size as usize)?));
        }
        let grid = Grid::new(width, height, cell);
        let flags = self.gpu.create_flags(grid.len())?;
        // The empty slice, measured once: an IDR of the (still unwritten) surface, then P frames
        // of the same surface with every cell skipped (+30, as the grid's unchanged cells are), so
        // there is nothing to code. The largest of six slices is the constant (a few frames, because
        // the first can still carry the IDR's residue). The stream starts with its own IDR afterwards.
        // A frame that refines cells is a little larger when nothing is coded in it (the map's
        // mixed deltas cost CABAC bits: 165 B live against 143 here, 470 B for a quarter of the
        // cells), so `is_empty` allows for the refined cells.
        let av1 = self.cfg.codec == Codec::Av1;
        let skip = vec![30i8; grid.len()];
        let slot = outs[0].2;
        enc.encode_with(slot, self.next_pts, &PicOpts { force_idr: true, ..Default::default() })?;
        let mut empty_slice = 0;
        for k in 1..=6 {
            let pkt = enc.encode_with(slot, self.next_pts + k, &PicOpts { qp_delta: Some(&skip), ..Default::default() })?;
            empty_slice = empty_slice.max(slice_bytes(pkt.data, av1));
        }
        self.shared.stats.lock().unwrap().empty_slice_bytes = empty_slice;
        self.enc = Some(Encoding {
            enc,
            empty_slice,
            outs,
            flags,
            width,
            height,
            grid,
            qp_scale: if self.cfg.codec == Codec::Av1 { 4 } else { 1 },
            mv_block,
            applied: plan,
            max_qp: 0,
            held: 0,
            last: None,
        });
        self.cur_slot = None;
        self.force_idr = true;
        self.reference = None;
        self.history.clear();
        Ok(())
    }

    fn close_encoder(&mut self) {
        if let Some(e) = self.enc.take() {
            drop(e.enc);
            for (out, mem, _) in e.outs {
                drop(mem);
                self.gpu.destroy_out(out);
            }
            self.gpu.destroy_readback(e.flags);
        }
    }

    fn cleanup(&mut self) {
        for f in std::mem::take(&mut self.pending) {
            self.release(&f);
        }
        self.close_encoder();
        for img in self.images.drain(..) {
            self.gpu.destroy_src(img);
        }
        if let Some((_, rb)) = self.dump.take() {
            self.gpu.destroy_readback(rb);
        }
    }

    fn on_hint(&mut self, h: Hint) {
        match h.kind {
            HINT_ANIM_START => {
                self.anim_since.get_or_insert(monotonic_ns());
            }
            HINT_ANIM_SETTLE => {
                self.anim_since = None;
                self.settles.push((h.seq, h.rect));
                self.apply_settles();
            }
            HINT_WINDOW_MOVE | HINT_SCROLL => {
                if h.detail == 0 || self.motion_seq != Some(h.seq) {
                    self.motion.clear();
                    self.motion_seq = Some(h.seq);
                }
                if h.rect.w > 0 && h.rect.h > 0 {
                    self.motion.push(Motion { rect: h.rect, dx: h.dx, dy: h.dy });
                }
            }
            _ => {}
        }
    }

    /// Settled animations whose final FRAME is encoded: refine their rect now.
    fn apply_settles(&mut self) {
        let Some(e) = self.enc.as_mut() else { return };
        let (now, last) = (monotonic_ns(), self.last_seq.unwrap_or(0));
        let settle_ns = self.cfg.refine.settle_ns;
        self.settles.retain(|(seq, r)| {
            if *seq <= last {
                e.grid.settle_now(r, settle_ns, now);
                false
            } else {
                true
            }
        });
    }

    /// The client has frame `p` and nothing usable after it (core's RefHint::Reference).
    fn predict_from(&mut self, mut p: u64) -> Result<()> {
        let end = self.next_pts;
        // An invalidated frame can't be a reference: go back to what the frames after
        // the last invalidation predicted from.
        if let Some((from, to)) = self.invalid_range {
            if p >= from && p < to {
                p = from - 1;
            }
        }
        if p + 1 >= end {
            return Ok(()); // nothing after it was sent
        }
        self.invalid_range = Some((p + 1, end));
        // what the lost frames changed must be coded again
        for (pts, rects) in &self.history {
            if *pts > p {
                self.redamage.extend_from_slice(rects);
            }
        }
        let Some(e) = self.enc.as_mut() else { return Ok(()) };
        match recovery(p, end, self.last_idr_pts) {
            Recovery::Idr => self.force_idr = true,
            Recovery::Invalidate => {
                for q in p + 1..end {
                    e.enc.invalidate(q)?;
                }
                self.invalidated += (end - p - 1) as u32;
                self.reference = Some(p);
                self.shared.stats.lock().unwrap().invalidations += 1;
            }
        }
        Ok(())
    }

    fn class_now(&self, now: u64, rate: &RateInput, hinted: bool) -> FrameClass {
        let input = self.shared.last_input_ns.load(Ordering::Relaxed);
        let anim = self.anim_since.is_some_and(|t| now.saturating_sub(t) < ANIM_TIMEOUT_NS);
        if rate.latency_critical || hinted || anim || (input != 0 && now.saturating_sub(input) < INTERACTIVE_AFTER_INPUT_NS)
        {
            FrameClass::Interactive
        } else {
            FrameClass::Passive
        }
    }

    /// The plan and the QP ceiling (0: none), by one reconfigure when either changed.
    fn apply_plan(&mut self, plan: RatePlan, max_qp: u32) -> Result<()> {
        let e = self.enc.as_mut().unwrap();
        if plan != e.applied || max_qp != e.max_qp {
            let split = if plan.split { nvenc::SPLIT_TWO_FORCED } else { nvenc::SPLIT_OFF };
            e.enc.set_rate(plan.kbps, plan.vbv_pct, split, max_qp)?;
            e.applied = plan;
            e.max_qp = max_qp;
            let mut s = self.shared.stats.lock().unwrap();
            (s.kbps, s.split, s.vbv_pct) = (plan.kbps, plan.split, plan.vbv_pct);
        }
        Ok(())
    }

    fn refine_cfg(&self, keep_skip: bool, user_cap: bool) -> RefineConfig {
        let mut rc = self.cfg.refine.for_rate(user_cap);
        if !keep_skip || !self.cfg.damage_qp {
            rc.skip_delta = 0;
        }
        rc
    }

    /// On a measured bottleneck the cells a refinement frame refines are ordered by their
    /// distance to the caret and the pointer.
    fn pick(&self, rate: &RateInput) -> Option<Pick> {
        (self.cfg.focus_refine && orders(rate))
            .then(|| Pick { focus: *self.shared.focus.lock().unwrap(), readable_first: self.cfg.refine.readable_first })
    }

    /// The latest settle times for the stats: changes that needed refinement only.
    fn note_settled(&self, s: &Settled) {
        let mut st = self.shared.stats.lock().unwrap();
        if let Some(t) = s.readable.filter(|t| t.refined) {
            st.readable_ms = Some(t.ms);
        }
        if let Some(t) = s.lossless.filter(|t| t.refined) {
            st.lossless_ms = Some(t.ms);
            st.settled_ms = None;
            st.settled_end = None;
        }
        if let Some(t) = s.settled.filter(|t| t.refined) {
            st.settled_ms = Some(t.ms);
            st.settled_end = Some(t.end);
        }
    }

    /// Counts down an intra refresh; true for the frame that completes it.
    fn ir_step(&mut self, starts: bool) -> bool {
        if starts {
            self.ir_left = IR_FRAMES - 1;
            false
        } else if self.ir_left > 0 {
            self.ir_left -= 1;
            self.ir_left == 0
        } else {
            false
        }
    }

    fn encode_capture(&mut self) -> Result<()> {
        let mut frames = std::mem::take(&mut self.pending);
        let mut f = frames.pop().unwrap();
        let merged = frames.len() as u32;
        let mut damage: Vec<Rect> = Vec::new();
        for old in &frames {
            damage.extend_from_slice(&old.damage);
            self.release(old);
        }
        damage.extend_from_slice(&f.damage);
        let now = monotonic_ns();

        let idr = self.force_idr;
        let ir = self.start_ir && !idr;
        let hinted = self.cfg.motion_hints
            && !idr
            && merged == 0
            && !self.motion.is_empty()
            && self.motion_seq == Some(f.seq)
            && self.last_seq.is_some_and(|s| s + 1 == f.seq);

        // Convert into the other slot, comparing with the newest frame (change detection).
        let e = self.enc.as_mut().unwrap();
        let slot = self.cur_slot.map_or(0, |s| 1 - s);
        let prev = self.cur_slot.filter(|_| !idr).map(|s| &e.outs[s].0);
        let diff = prev.map(|p| (p, &e.flags, e.grid.cell, e.grid.cols));
        let (out, _, enc_slot) = &e.outs[slot];
        let ct = self.gpu.convert_diff(&self.images[f.buffer as usize], out, f.acquire.take(), None, None, diff)?;
        let enc_slot = *enc_slot;
        let _ = plugin::send(&self.shared.sock, plugin::RELEASE, &plugin::release_payload(f.generation, f.buffer), &[]);

        // What changed: every cell on a keyframe or intra refresh; else the cells whose
        // pixels differ (an app's damage is often its whole window), plus whatever frames
        // the client lost had changed. Without a previous frame, the damage.
        let redamage = std::mem::take(&mut self.redamage);
        e.grid.begin_frame();
        if idr || ir {
            e.grid.mark_all(f.t_rendered_ns);
        } else if prev.is_some() && self.cfg.damage_qp {
            e.grid.mark_flagged(e.flags.words(), f.t_rendered_ns);
            e.grid.mark_changed(&redamage, f.t_rendered_ns);
        } else {
            e.grid.mark_changed(&damage, f.t_rendered_ns);
            e.grid.mark_changed(&redamage, f.t_rendered_ns);
        }
        let changed = e.grid.changed_count() as u32;
        if changed == 0 {
            // Damaged but identical: no frame. Both slots hold the same pixels.
            self.last_seq = Some(f.seq);
            self.motion.clear();
            self.motion_seq = None;
            self.apply_settles();
            return Ok(());
        }
        let damage = e.grid.changed_rects();

        let rate_in = self.rate_input();
        let class = self.class_now(now, &rate_in, hinted);
        let plan = rate::plan(&rate_in, class, self.cfg.fps).unwrap();
        // no ceiling on a keyframe or while an intra refresh goes on (its frames carry the refresh)
        let e = self.enc.as_ref().unwrap();
        let max_qp = if idr || ir || self.ir_left > 0 {
            0
        } else {
            rate::max_qp(e.last.as_ref(), e.held != 0, changed, now, self.interval_ns, &plan, self.cfg.fps, e.qp_scale)
        };
        self.apply_plan(plan, max_qp)?;
        let fold = class == FrameClass::Passive && self.cfg.refinement && !idr;
        let rc = self.refine_cfg(!idr && !ir && self.ir_left == 0, rate_in.user_cap);
        let pick = if fold { self.pick(&rate_in) } else { None };
        let e = self.enc.as_mut().unwrap();
        let map = e.grid.qp_map(&mut self.qpmap, fold, now, &rc, e.qp_scale, pick.as_ref());
        let hinted_blocks = if hinted
            && motion_field(e.width, e.height, e.mv_block, &self.motion, &mut self.mv, &mut self.mv_valid)
        {
            self.mv_valid.iter().filter(|&&v| v != 0).count() as u32
        } else {
            0
        };

        let pts = self.next_pts;
        let opts = PicOpts {
            force_idr: idr,
            intra_refresh: if ir { IR_FRAMES } else { 0 },
            qp_delta: Some(&self.qpmap),
            mv: (hinted_blocks > 0).then_some((&self.mv[..], &self.mv_valid[..])),
        };
        let pkt = e.enc.encode_with(enc_slot, pts, &opts)?;
        let t_encoded_ns = monotonic_ns();
        let data = pkt.data.to_vec();
        let (pic_type, avg_qp, encode_ms) = (pkt.pic_type, pkt.avg_qp, pkt.ms);
        // a frame with changed cells has bytes: only a refinement frame can be empty
        let done = e.grid.commit(avg_qp, &self.qpmap, &rc, e.qp_scale, t_encoded_ns, false);
        e.last = Some(Steady { base_qp: done.base_qp, bytes: data.len(), changed, t_encoded_ns });
        e.held = max_qp;
        let (width, height, cells) = (e.width, e.height, e.grid.len() as u32);
        self.note_settled(&done.settled);

        let mut rects = if idr || ir { vec![Rect { x: 0, y: 0, w: width as i32, h: height as i32 }] } else { damage };
        rects.extend(map.bbox);
        if idr {
            self.last_idr_pts = pts;
            self.ir_left = 0;
            self.reference = None;
        }
        let recovery_point = !idr && self.ir_step(ir);
        self.force_idr = false;
        self.start_ir = false;
        self.cur_slot = Some(slot);
        self.cur_rendered_ns = f.t_rendered_ns;
        self.cur_seq = f.seq;
        self.last_seq = Some(f.seq);
        self.motion.clear();
        self.motion_seq = None;
        let info = FrameInfo {
            seq: f.seq,
            t_ready_ns: f.t_ready_ns,
            convert_ms: ct.gpu_ms,
            encode_ms,
            avg_qp,
            pic_type,
            changed_cells: changed,
            refined_cells: map.refined as u32,
            cells,
            hinted_blocks,
            merged,
            kbps: plan.kbps,
            split: plan.split,
            intra_refresh: ir,
            invalidated: std::mem::take(&mut self.invalidated),
            base_qp: done.base_qp,
            readable_ms: done.settled.readable.map(|t| t.ms),
            lossless_ms: done.settled.lossless.map(|t| t.ms),
            settled_ms: done.settled.settled.map(|t| t.ms),
            settled_end: done.settled.settled.map(|t| t.end),
        };
        let reference = self.reference.take();
        self.emit(pts, data, idr, reference, recovery_point, class, rects, (width, height), f.t_rendered_ns, t_encoded_ns, info);
        self.apply_settles();
        Ok(())
    }

    fn encode_refine(&mut self) -> Result<()> {
        let now = monotonic_ns();
        let Some(slot) = self.cur_slot else { return Ok(()) };
        // core's budget is the fresh interval's (protocol.md §8.1): what this interval's
        // frames already took comes off it
        let mut rate_in = self.rate_input();
        let sent: usize =
            self.sent.iter().filter(|(t, _)| now.saturating_sub(*t) < self.interval_ns / 2).map(|s| s.1).sum();
        rate_in.interval_remaining_bytes = rate_in.interval_remaining_bytes.saturating_sub(sent);
        let Some(plan) = rate::plan(&rate_in, FrameClass::Refine, self.cfg.fps) else {
            // this interval's budget is spent: try again in half an interval
            self.refine_wait_ns = now + self.interval_ns / 2;
            return Ok(());
        };
        let rc = self.refine_cfg(self.ir_left == 0, rate_in.user_cap);
        let pick = self.pick(&rate_in);
        let e = self.enc.as_mut().unwrap();
        e.grid.begin_frame();
        let map = e.grid.qp_map(&mut self.qpmap, true, now, &rc, e.qp_scale, pick.as_ref());
        if map.refined == 0 {
            return Ok(());
        }
        let max_qp = rate::refine_max_qp(e.last.as_ref(), e.held, now, self.interval_ns);
        self.apply_plan(plan, max_qp)?;
        let e = self.enc.as_mut().unwrap();
        let (out, _, enc_slot) = &e.outs[slot];
        if let Some((dir, rb)) = self.dump.as_ref().filter(|_| self.dumped_seq != Some(self.cur_seq)) {
            self.gpu.dump_out(out, rb)?;
            std::fs::write(dir.join(format!("refine_{:06}.yuv", self.next_pts)), rb.bytes())?;
            self.dumped_seq = Some(self.cur_seq);
        }
        let enc_slot = *enc_slot;
        let pts = self.next_pts;
        let opts = PicOpts { qp_delta: Some(&self.qpmap), ..Default::default() };
        let pkt = e.enc.encode_with(enc_slot, pts, &opts)?;
        let t_encoded_ns = monotonic_ns();
        let data = pkt.data.to_vec();
        let (pic_type, avg_qp, encode_ms) = (pkt.pic_type, pkt.avg_qp, pkt.ms);
        let empty = is_empty(slice_bytes(&data, self.cfg.codec == Codec::Av1), e.empty_slice, map.refined);
        let done = e.grid.commit(avg_qp, &self.qpmap, &rc, e.qp_scale, t_encoded_ns, empty);
        self.last_refine_ns = now;
        let (width, height, cells) = (e.width, e.height, e.grid.len() as u32);
        self.note_settled(&done.settled);
        let recovery_point = self.ir_step(false);
        let info = FrameInfo {
            seq: self.cur_seq,
            t_ready_ns: now,
            encode_ms,
            avg_qp,
            pic_type,
            refined_cells: map.refined as u32,
            cells,
            kbps: plan.kbps,
            split: plan.split,
            invalidated: std::mem::take(&mut self.invalidated),
            base_qp: done.base_qp,
            readable_ms: done.settled.readable.map(|t| t.ms),
            lossless_ms: done.settled.lossless.map(|t| t.ms),
            settled_ms: done.settled.settled.map(|t| t.ms),
            settled_end: done.settled.settled.map(|t| t.end),
            ..Default::default()
        };
        let reference = self.reference.take();
        let rects = map.bbox.into_iter().collect();
        let t_r = self.cur_rendered_ns;
        self.emit(pts, data, false, reference, recovery_point, FrameClass::Refine, rects, (width, height), t_r, t_encoded_ns, info);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn emit(
        &mut self,
        pts: u64,
        data: Vec<u8>,
        keyframe: bool,
        reference: Option<u64>,
        recovery_point: bool,
        class: FrameClass,
        rects: Vec<Rect>,
        (width, height): (u32, u32),
        t_rendered_ns: u64,
        t_encoded_ns: u64,
        info: FrameInfo,
    ) {
        self.next_pts += 1;
        if self.sent.len() == 8 {
            self.sent.pop_front();
        }
        self.sent.push_back((t_encoded_ns, data.len()));
        self.last_encode_ns = t_encoded_ns;
        let meta = wire::frame_meta(class.wire(), &rects, width, height);
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back((pts, rects));
        {
            let mut s = self.shared.stats.lock().unwrap();
            let c = class.wire() as usize;
            s.frames[c] += 1;
            s.bytes[c] += data.len() as u64;
            s.keyframes += keyframe as u64;
            s.intra_refreshes += info.intra_refresh as u64;
            s.last_encode_ms = info.encode_ms;
        }
        (self.sink)(MediaEvent::Frame(EncodedFrame {
            pts,
            data,
            keyframe,
            reference,
            recovery_point,
            class,
            meta,
            width,
            height,
            t_rendered_ns,
            t_encoded_ns,
            info,
        }));
    }
}

/// Targets under this share of the session's demand are budget-bound whether or not a
/// bottleneck was measured (a user cap, the probes' first 2 s, the thin hedge), so
/// refinement is ordered; the LAN's target sits at the demand.
const ORDER_BELOW_DEMAND: f64 = 0.6;
/// The threshold while the demand isn't known (the media harness, before the session
/// says): integration.md 7's 40 Mbit/s, 0.6 × the 65 Mbit/s demand at 2940 × 1848 at 60 Hz.
const ORDER_BELOW_BPS: u64 = 40_000_000;

/// Refinement takes the cells nearest the caret and the pointer first on a measured
/// bottleneck, or whenever the target is under 0.6 × the demand (integration.md 7).
fn orders(rate: &RateInput) -> bool {
    let below = match rate.demand_bps {
        0 => ORDER_BELOW_BPS,
        d => (ORDER_BELOW_DEMAND * d as f64) as u64,
    };
    rate.c_est_bps.is_some() || rate.encoder_bps < below
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_beyond_the_dpb_gets_an_idr() {
        assert_eq!(recovery(10, 12, 0), Recovery::Invalidate);
        assert_eq!(recovery(10, 10 + DPB, 0), Recovery::Invalidate); // the oldest frame still held
        assert_eq!(recovery(10, 10 + DPB + 1, 0), Recovery::Idr); // one more and it left
        assert_eq!(recovery(3, 6, 5), Recovery::Idr); // from before the stream's keyframe
    }

    #[test]
    fn ordering_applies_on_a_bottleneck_or_under_0_6_of_the_demand() {
        let at = |bps, c_est_bps, demand_bps| RateInput { encoder_bps: bps, c_est_bps, demand_bps, ..Default::default() };
        assert!(orders(&at(77_999_999, None, 130_000_000))); // 120 Hz at 2940 x 1848: under 78
        assert!(!orders(&at(78_000_000, None, 130_000_000)));
        assert!(!orders(&at(15_000_000, None, 25_000_000))); // 1080p's demand: a LAN never orders
        assert!(orders(&at(39_999_999, None, 0)) && !orders(&at(40_000_000, None, 0))); // demand unknown: 40
        assert!(orders(&at(130_000_000, Some(140_000_000), 130_000_000))); // measured: whatever the target
    }
}
