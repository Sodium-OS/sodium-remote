// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The frame as a grid of encoder cells (HEVC CTBs, 32×32; AV1 superblocks, 64×64):
//! which cells changed and when, the QP delta map that follows from it, and the
//! motion-hint field for window moves and scrolls.
//!
//! - **Change-driven:** cells that changed in this frame are coded at the rate
//!   controller's QP; every other cell gets a large positive delta, so the encoder
//!   codes it as skip and an unchanged area costs almost nothing.
//! - **Build to lossless, adaptive:** a cell is *done* once it has been
//!   coded at a frame QP at or below `done_qp`. A cell that isn't done and stopped
//!   changing `settle` ago is refined: coded again at the rate controller's QP (delta
//!   `refine_delta`, 0 by default, so NVENC's rate control keeps the frame inside its
//!   budget), once per frame interval, until a pass comes out at `done_qp` or better,
//!   or after `max_passes`. On a fast link a change is usually done at once (typing:
//!   QP 19) and costs no refinement; a dense page on a slow link takes several passes.
//! - **Readable and lossless times:** a cell is *readable* once it was coded
//!   at `readable_qp` (the luma bar, 34) or better. A change (the cells one frame
//!   changed) is readable when all its cells are, lossless when all are done;
//!   [`Grid::commit`] says when that happens and how long after the change's own frame.
//! - **Slow links:** on a measured bottleneck the due cells of a refinement frame
//!   get QP offsets that grow with their distance to the caret and the pointer ([`Pick`]):
//!   the rate controller spends the frame's budget on the nearest first, and the farther
//!   ones follow as those finish. Every cell goes to the luma bar before any goes on to
//!   lossless. Under a user cap no cell is coded finer than the bar ([`RefineConfig::bar_floor`]).

use std::collections::HashMap;

use crate::capture::Rect;

#[derive(Clone, Debug)]
pub struct RefineConfig {
    /// How long a cell must stay unchanged before it is refined.
    pub settle_ns: u64,
    /// A cell coded at a frame QP at or below this needs no (more) refinement
    /// (HEVC QP; AV1 qindex is 4×).
    pub done_qp: u32,
    /// The luma bar: a cell coded at or below this is readable (Y SSIM about 0.99 on
    /// text). Under a user-set bandwidth cap refinement stops here instead of at `done_qp`.
    pub readable_qp: u32,
    /// Refinement passes at most, per change; 0: one second's worth (the stream rate).
    pub max_passes: u8,
    /// QP delta for the cells a frame refines (0: the rate controller's QP).
    pub refine_delta: i8,
    /// QP delta for cells a frame doesn't touch (unchanged: coded as skip).
    pub skip_delta: i8,
    /// When ordering applies (a measured bottleneck, or a target under 40 Mbit/s), every cell goes to the luma bar (nearest the focus first)
    /// before any goes on to lossless. Off: strictly by distance, so the cells near the
    /// caret are lossless first and the far ones readable later.
    pub readable_first: bool,
    /// When ordering applies: the QP offset of the nearest due cell is minus this and the
    /// farthest's is plus this, between them by rank. The rate controller sets the frame's base
    /// QP for its budget, so this decides how much more of it the near cells get (2× per 6 QP).
    pub order_spread: u8,
    /// Under a user cap a cell is never coded finer than `readable_qp`: with a `pick` its QP is
    /// `max(base + offset, readable_qp)`, so the nearest cells get the bar and the farthest the coarse
    /// end, and the bit-neutral shift is worked out over those clamped offsets. The base is the one the
    /// last frame came out at (the encoder's rate control sets it, so this frame's isn't known yet).
    pub bar_floor: bool,
}

impl Default for RefineConfig {
    fn default() -> Self {
        RefineConfig {
            settle_ns: 150_000_000,
            done_qp: 20,
            readable_qp: 34,
            max_passes: 0,
            refine_delta: 0,
            skip_delta: 30,
            readable_first: true,
            order_spread: 12,
            bar_floor: false,
        }
    }
}

impl RefineConfig {
    /// The rules for one frame: under a user-set bandwidth cap (`RateInput::user_cap`)
    /// refinement stops at the luma bar, and no cell is coded finer than it (`bar_floor`).
    pub fn for_rate(&self, user_cap: bool) -> RefineConfig {
        let mut c = self.clone();
        if user_cap {
            c.done_qp = c.done_qp.max(c.readable_qp);
            c.bar_floor = true;
        }
        c
    }
}

/// What a frame's QP map does, cell by cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellRole {
    Skip,
    Changed,
    Refine,
}

/// Where the user is looking, in buffer pixels: the focused text field's caret and the
/// pointer. Refinement on a measured bottleneck starts at the nearest of the two.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Focus {
    pub caret: Option<Rect>,
    pub pointer: Option<(i32, i32)>,
}

impl Focus {
    pub fn is_empty(&self) -> bool {
        self.caret.is_none() && self.pointer.is_none()
    }

    /// Squared distance from (x, y) to the nearest focus point; the caret is a rect
    /// (0 inside it). `i64::MAX` with no focus.
    fn dist2(&self, x: i64, y: i64) -> i64 {
        let mut best = i64::MAX;
        if let Some(r) = self.caret {
            let dx = (r.x as i64 - x).max(0).max(x - (r.x as i64 + r.w.max(0) as i64));
            let dy = (r.y as i64 - y).max(0).max(y - (r.y as i64 + r.h.max(0) as i64));
            best = best.min(dx * dx + dy * dy);
        }
        if let Some((px, py)) = self.pointer {
            let (dx, dy) = (px as i64 - x, py as i64 - y);
            best = best.min(dx * dx + dy * dy);
        }
        best
    }
}

/// How a refinement frame orders its due cells on a measured bottleneck.
#[derive(Clone, Copy, Debug)]
pub struct Pick {
    pub focus: Focus,
    /// Cells that aren't readable yet come before those that are (`RefineConfig::readable_first`).
    pub readable_first: bool,
}

/// The base QP falls this much (in QP) from one refinement frame to the next while a page is refined
/// (pagecap, 10 Mbit/s: 1 to 4 a frame, about 2), so the floor of [`RefineConfig::bar_floor`] takes the
/// next frame's base to be this far under the last one's. With the last one's own, the cells at the floor
/// came out about 2 QP finer than the bar and the page took 7 % longer and 13 % more bytes than
/// unordered; at 2 it is the same, at 3 or 4 the caret's box is later.
const BASE_DRIFT: i32 = 2;

/// A frame that orders its cells skips the rest this far under QP 51 when the base is at the
/// bar the frame aims for, and by at least `MIN_PICK_SKIP`.
const SKIP_MARGIN: i32 = 3;
const MIN_PICK_SKIP: i32 = 10;

/// What a frame's QP map came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapInfo {
    /// Cells refined in this frame.
    pub refined: usize,
    /// Their bounding box.
    pub bbox: Option<Rect>,
}

/// How a change ended. Ordered by how far it is from the stop: a change
/// whose cells ended differently reports the worst.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ending {
    /// Every cell was coded at the stop QP.
    #[default]
    Stop,
    /// A pass coded nothing: the cell is converged at the QP the link affords.
    Parked,
    /// Out of passes.
    Cap,
}

impl Ending {
    pub fn as_str(self) -> &'static str {
        match self {
            Ending::Stop => "stop",
            Ending::Parked => "parked",
            Ending::Cap => "cap",
        }
    }
}

/// How long a change took to become readable, or lossless, or to settle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Took {
    /// From the change's own frame (`t_rendered`) to the bitstream being ready.
    pub ms: f64,
    /// At least one refinement pass was needed (a change coded well at once costs none).
    pub refined: bool,
    pub end: Ending,
}

/// Changes that finished in one frame. With several, the longest.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settled {
    pub readable: Option<Took>,
    /// Changes that ended at the stop only.
    pub lossless: Option<Took>,
    /// Changes that ended parked or at the cap (`end` says which).
    pub settled: Option<Took>,
}

fn longest(a: Option<Took>, b: Took) -> Option<Took> {
    Some(match a {
        Some(a) if a.ms >= b.ms => a,
        _ => b,
    })
}

/// What [`Grid::commit`] found out about the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Committed {
    /// The frame's base QP (codec units), recovered from NVENC's average.
    pub base_qp: i32,
    pub settled: Settled,
}

/// The cells one frame changed, counted until they are readable and lossless.
#[derive(Default)]
struct Change {
    /// Cells not yet done / not yet readable.
    undone: u32,
    unreadable: u32,
    /// A refinement pass touched it.
    refined: bool,
    /// The worst ending of its cells so far.
    end: Ending,
}

pub struct Grid {
    pub cell: u32,
    pub cols: u32,
    pub rows: u32,
    width: u32,
    height: u32,
    /// The frame time (CLOCK_MONOTONIC ns) of the cell's last change; 0: never changed.
    /// It names the change the cell belongs to.
    changed_ns: Vec<u64>,
    /// What the cell's settle time counts from: `changed_ns`, or earlier once the
    /// compositor said the animation settled.
    due_ns: Vec<u64>,
    /// Refinement passes since the last change.
    passes: Vec<u8>,
    /// Coded at `readable_qp` or better since the last change.
    readable: Vec<bool>,
    /// Coded at `done_qp` or better since the last change (or out of passes).
    done: Vec<bool>,
    /// What the frame being built does with each cell.
    role: Vec<CellRole>,
    /// The changes with cells still to finish, by their `changed_ns`.
    changes: HashMap<u64, Change>,
    /// The base QP (codec units) the last committed frame came out at: what `RefineConfig::bar_floor`
    /// takes for the next frame's.
    last_base: Option<i32>,
}

impl Grid {
    pub fn new(width: u32, height: u32, cell: u32) -> Self {
        let cols = width.div_ceil(cell);
        let rows = height.div_ceil(cell);
        let n = (cols * rows) as usize;
        Grid {
            cell,
            cols,
            rows,
            width,
            height,
            changed_ns: vec![0; n],
            due_ns: vec![0; n],
            passes: vec![0; n],
            readable: vec![true; n],
            done: vec![true; n],
            role: vec![CellRole::Skip; n],
            changes: HashMap::new(),
            last_base: None,
        }
    }

    pub fn len(&self) -> usize {
        self.changed_ns.len()
    }

    /// The cells a rect touches, clipped to the frame: (x0, y0, x1, y1) exclusive.
    fn cells_of(&self, r: &Rect) -> Option<(u32, u32, u32, u32)> {
        let x0 = r.x.max(0) as i64;
        let y0 = r.y.max(0) as i64;
        let x1 = (r.x as i64 + r.w as i64).min(self.width as i64);
        let y1 = (r.y as i64 + r.h as i64).min(self.height as i64);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let c = self.cell as i64;
        Some(((x0 / c) as u32, (y0 / c) as u32, ((x1 + c - 1) / c) as u32, ((y1 + c - 1) / c) as u32))
    }

    /// The cell changes at `t_ns`: it leaves the change it was still finishing (one with
    /// no cell left was superseded and is never reported) and joins this one.
    fn change(&mut self, i: usize, t_ns: u64) {
        if !self.done[i] {
            let t = self.changed_ns[i];
            if let Some(c) = self.changes.get_mut(&t) {
                c.undone -= 1;
                c.unreadable -= !self.readable[i] as u32;
                if c.undone == 0 {
                    self.changes.remove(&t);
                }
            }
        }
        self.changed_ns[i] = t_ns;
        self.due_ns[i] = t_ns;
        self.passes[i] = 0;
        self.readable[i] = false;
        self.done[i] = false;
        self.role[i] = CellRole::Changed;
        let c = self.changes.entry(t_ns).or_default();
        c.undone += 1;
        c.unreadable += 1;
    }

    /// Starts a new frame: every cell is skip until marked.
    pub fn begin_frame(&mut self) {
        self.role.iter_mut().for_each(|r| *r = CellRole::Skip);
    }

    /// Cells marked changed in the frame being built.
    pub fn changed_count(&self) -> usize {
        self.role.iter().filter(|&&r| r == CellRole::Changed).count()
    }

    /// Marks the cells under `rects` as changed at `t_ns` (in the frame being built).
    pub fn mark_changed(&mut self, rects: &[Rect], t_ns: u64) {
        for r in rects {
            if let Some((x0, y0, x1, y1)) = self.cells_of(r) {
                for y in y0..y1 {
                    for x in x0..x1 {
                        self.change((y * self.cols + x) as usize, t_ns);
                    }
                }
            }
        }
    }

    /// Marks the cells whose flag is set (the GPU's change detection) as changed at `t_ns`.
    pub fn mark_flagged(&mut self, flags: &[u32], t_ns: u64) {
        for i in 0..self.len().min(flags.len()) {
            if flags[i] != 0 {
                self.change(i, t_ns);
            }
        }
    }

    /// Everything changed (a keyframe, an intra refresh).
    pub fn mark_all(&mut self, t_ns: u64) {
        for i in 0..self.len() {
            self.change(i, t_ns);
        }
    }

    /// Animation settled over `rect`: its cells are due for refinement now rather than
    /// `settle` after their last change (the compositor knows they won't change).
    pub fn settle_now(&mut self, rect: &Rect, settle_ns: u64, now_ns: u64) {
        if let Some((x0, y0, x1, y1)) = self.cells_of(rect) {
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * self.cols + x) as usize;
                    if !self.done[i] {
                        self.due_ns[i] = self.due_ns[i].min(now_ns.saturating_sub(settle_ns));
                    }
                }
            }
        }
    }

    /// The changed cells of the frame being built as rects (runs of cells per row,
    /// merged down where a run repeats), in pixels, clipped to the frame.
    pub fn changed_rects(&self) -> Vec<Rect> {
        let c = self.cell as i32;
        let mut out: Vec<Rect> = Vec::new();
        let mut above: Vec<(u32, u32, usize)> = Vec::new();
        let changed = |x: u32, y: u32| self.role[(y * self.cols + x) as usize] == CellRole::Changed;
        for y in 0..self.rows {
            let mut here = Vec::new();
            let mut x = 0;
            while x < self.cols {
                if !changed(x, y) {
                    x += 1;
                    continue;
                }
                let x0 = x;
                while x < self.cols && changed(x, y) {
                    x += 1;
                }
                let i = match above.iter().find(|r| r.0 == x0 && r.1 == x) {
                    Some(&(_, _, i)) => {
                        out[i].h += c;
                        i
                    }
                    None => {
                        out.push(Rect { x: x0 as i32 * c, y: y as i32 * c, w: (x - x0) as i32 * c, h: c });
                        out.len() - 1
                    }
                };
                here.push((x0, x, i));
            }
            above = here;
        }
        for r in &mut out {
            r.w = r.w.min(self.width as i32 - r.x);
            r.h = r.h.min(self.height as i32 - r.y);
        }
        out
    }

    fn due(&self, i: usize, now_ns: u64, cfg: &RefineConfig) -> bool {
        !self.done[i] && self.role[i] != CellRole::Changed && self.due_ns[i] + cfg.settle_ns <= now_ns
    }

    /// When the next refinement is due (CLOCK_MONOTONIC ns), if any.
    pub fn next_due(&self, cfg: &RefineConfig) -> Option<u64> {
        (0..self.len()).filter(|&i| !self.done[i]).map(|i| self.due_ns[i] + cfg.settle_ns).min()
    }

    /// The cell's centre in pixels, inside the frame.
    fn centre(&self, i: usize) -> (i64, i64) {
        let (x, y) = (i as u32 % self.cols, i as u32 / self.cols);
        let c = self.cell as i64;
        ((x as i64 * c + c / 2).min(self.width as i64 - 1), (y as i64 * c + c / 2).min(self.height as i64 - 1))
    }

    /// Puts `due` in the order `pick` asks for: cells that aren't readable yet first (if
    /// asked to), each group nearest to the focus first.
    fn order(&self, due: &mut [usize], pick: &Pick) {
        due.sort_by_key(|&i| {
            let (x, y) = self.centre(i);
            (self.readable[i] && pick.readable_first, pick.focus.dist2(x, y))
        });
    }

    /// Builds the QP delta map for the frame being built into `map` (one i8 per cell,
    /// raster order; `scale` multiplies deltas: 1 for HEVC QP, 4 for AV1 qindex).
    /// With `refine`, due cells are refined in this frame. With a `pick` their deltas run
    /// from `-order_spread` for the first in its order to `+order_spread` for the last.
    /// Call [`Grid::commit`] once the frame is encoded.
    ///
    /// With a `pick` the skipped cells' delta is no more than puts them just under the
    /// ceiling (QP 51) when the base is at the bar the frame aims for: when few cells carry
    /// the frame's QP and every skipped cell sits at the ceiling, NVENC's average QP says
    /// nothing about the base.
    pub fn qp_map(&mut self, map: &mut Vec<i8>, refine: bool, now_ns: u64, cfg: &RefineConfig, scale: i32, pick: Option<&Pick>) -> MapInfo {
        map.clear();
        map.resize(self.len(), 0);
        let clamp = |v: i32| v.clamp(-127, 127) as i8;
        let mut due: Vec<usize> = if refine { (0..self.len()).filter(|&i| self.due(i, now_ns, cfg)).collect() } else { Vec::new() };
        if let Some(p) = pick {
            self.order(&mut due, p);
        }
        let aim = if due.iter().any(|&i| !self.readable[i]) { cfg.readable_qp } else { cfg.done_qp };
        let skip = if pick.is_some() {
            cfg.skip_delta.min((51 - SKIP_MARGIN - aim as i32).clamp(MIN_PICK_SKIP, 30) as i8)
        } else {
            cfg.skip_delta
        };
        let (mut bx0, mut by0, mut bx1, mut by1) = (u32::MAX, u32::MAX, 0, 0);
        let n = due.len();
        let spread = if pick.is_some() { cfg.order_spread as i32 } else { 0 };
        let by_rank: Vec<i32> =
            (0..n).map(|k| if n > 1 { (2 * spread * k as i32 + (n as i32 - 1) / 2) / (n as i32 - 1) - spread } else { 0 }).collect();
        // Under a user cap no cell is coded finer than the bar: its delta is at least the bar minus the
        // base, which is the last frame's (in QP units, rounded down: a finer cell, never a coarser one).
        let floor = self
            .last_base
            .filter(|_| pick.is_some() && cfg.bar_floor)
            .map(|b| (cfg.readable_qp as i32 * scale - (b - BASE_DRIFT * scale)).div_euclid(scale) - cfg.refine_delta as i32);
        let neutral = neutral_shift(&by_rank, floor);
        for (k, &i) in due.iter().enumerate() {
            self.role[i] = CellRole::Refine;
            let d = by_rank[k] + neutral;
            map[i] = clamp((cfg.refine_delta as i32 + floor.map_or(d, |f| d.max(f))) * scale);
            let (x, y) = (i as u32 % self.cols, i as u32 / self.cols);
            (bx0, by0, bx1, by1) = (bx0.min(x), by0.min(y), bx1.max(x + 1), by1.max(y + 1));
        }
        for i in 0..self.len() {
            match self.role[i] {
                CellRole::Changed => map[i] = 0,
                CellRole::Skip => map[i] = clamp(skip as i32 * scale),
                CellRole::Refine => {}
            }
        }
        let bbox = (n > 0).then(|| {
            let c = self.cell as i32;
            let x = bx0 as i32 * c;
            let y = by0 as i32 * c;
            Rect { x, y, w: (bx1 as i32 * c).min(self.width as i32) - x, h: (by1 as i32 * c).min(self.height as i32) - y }
        });
        MapInfo { refined: n, bbox }
    }

    /// The frame built by [`Grid::qp_map`] into `map` was encoded with NVENC's average QP
    /// `avg_qp` (codec units; `scale` 1 for HEVC, 4 for AV1) and its bitstream was ready
    /// at `t_enc_ns`. Changed and refined cells coded at `readable_qp` or better are
    /// readable, at `done_qp` or better done; refined cells count a pass. Says which
    /// changes became readable or lossless with this frame. `empty`: the frame coded nothing
    /// (its slice is the empty slice's size): the cells it refined are parked.
    pub fn commit(&mut self, avg_qp: u32, map: &[i8], cfg: &RefineConfig, scale: i32, t_enc_ns: u64, empty: bool) -> Committed {
        let max = if scale == 1 { 51 } else { 255 };
        let base = base_qp(avg_qp, map, max);
        self.last_base = Some(base);
        let (readable_qp, done_qp) = (cfg.readable_qp as i32 * scale, cfg.done_qp as i32 * scale);
        let mut out = Settled::default();
        for i in 0..self.len() {
            // each cell was coded at the base QP plus its own delta
            let q = (base + map.get(i).copied().unwrap_or(0) as i32).clamp(0, max);
            let refined = match self.role[i] {
                CellRole::Changed => false,
                CellRole::Refine => true,
                CellRole::Skip => continue,
            };
            let t = self.changed_ns[i];
            if refined {
                self.passes[i] = self.passes[i].saturating_add(1);
                if let Some(c) = self.changes.get_mut(&t) {
                    c.refined = true;
                }
            }
            let ms = t_enc_ns.saturating_sub(t) as f64 / 1e6;
            let parked = refined && empty;
            if !parked && !self.readable[i] && q <= readable_qp {
                self.readable[i] = true;
                if let Some(c) = self.changes.get_mut(&t) {
                    c.unreadable -= 1;
                    if c.unreadable == 0 {
                        out.readable = longest(out.readable, Took { ms, refined: c.refined, end: Ending::Stop });
                    }
                }
            }
            let end = if parked {
                Some(Ending::Parked)
            } else if q <= done_qp {
                Some(Ending::Stop)
            } else if refined && self.passes[i] >= cfg.max_passes {
                Some(Ending::Cap)
            } else {
                None
            };
            if let Some(end) = end {
                self.done[i] = true;
                if let Some(c) = self.changes.get_mut(&t) {
                    c.undone -= 1;
                    c.end = c.end.max(end);
                    if c.undone == 0 {
                        let took = Took { ms, refined: c.refined, end: c.end };
                        if c.end == Ending::Stop {
                            out.lossless = longest(out.lossless, took);
                        } else {
                            out.settled = longest(out.settled, took);
                        }
                        self.changes.remove(&t);
                    }
                }
            }
        }
        Committed { base_qp: base, settled: out }
    }

    #[cfg(test)]
    fn roles(&self) -> Vec<CellRole> {
        self.role.clone()
    }
}

/// The shift that makes the cost of the cells' `offsets` average that of no offsets at all. Bits
/// grow 2× per 6 QP down, so offsets that average 0 cost more than none, and the rate controller,
/// which sizes the frame's QP for no offsets, overshoots: they are shifted up by the QP that
/// makes their cost average the same. With a `floor` (the least a cell's offset can be) the cost is
/// that of the clamped offsets `max(offset + shift, floor)`, which are cheaper, so the shift comes
/// out lower: the cells above the floor get finer to spend what the clamped ones no longer do.
fn neutral_shift(offsets: &[i32], floor: Option<i32>) -> i32 {
    if offsets.is_empty() {
        return 0;
    }
    let n = offsets.len() as f64;
    let cost = |s: i32| {
        offsets.iter().map(|&o| 2f64.powf(-(floor.map_or(o + s, |f| (o + s).max(f))) as f64 / 6.0)).sum::<f64>() / n
    };
    let start = (6.0 * (offsets.iter().map(|&o| 2f64.powf(-o as f64 / 6.0)).sum::<f64>() / n).log2()).round() as i32;
    if floor.is_none() {
        return start;
    }
    // the cost falls as the shift grows: lower it while that gets it nearer to none
    let off = |s: i32| cost(s).ln().abs();
    let mut s = start;
    while s > start - 24 && off(s - 1) < off(s) {
        s -= 1;
    }
    s
}

/// The rate controller's base QP for a frame encoded with the delta `map`, from NVENC's
/// `frameAvgQP`, which averages every CTB's QP after its delta (measured: a
/// keystroke with every other CTB at +30 reports about base + 30). Each CTB's QP is
/// `clamp(base + delta, 0, max)`. Pessimistic: the largest base whose average is still
/// under `avg_qp + 1` (NVENC rounds or truncates). When almost every CTB is clamped the
/// average says little about the rest, and this answers high (so: refine).
pub fn base_qp(avg_qp: u32, map: &[i8], max: i32) -> i32 {
    if map.is_empty() {
        return avg_qp as i32;
    }
    let mean = |b: i32| map.iter().map(|&d| (b + d as i32).clamp(0, max) as i64).sum::<i64>() as f64 / map.len() as f64;
    let limit = avg_qp as f64 + 1.0;
    let (mut lo, mut hi) = (-max - 128, max); // mean is non-decreasing in b
    if mean(hi) < limit {
        return max;
    }
    while lo < hi {
        let mid = (lo + hi + 1).div_euclid(2);
        if mean(mid) < limit { lo = mid } else { hi = mid - 1 }
    }
    lo.clamp(0, max)
}

/// One motion hint (capture-ipc HINT kinds 3 and 4): the pixels at `rect` in this
/// frame were at `rect − (dx, dy)` in the previous one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    pub rect: Rect,
    pub dx: i32,
    pub dy: i32,
}

/// The motion-hint field: one vector per `block`×`block` block, in raster order,
/// from the block to where its pixels were in the previous frame, i.e. `(−dx, −dy)`
/// for blocks whose centre lies in a moved rect. Later hints win where rects overlap.
/// Returns false when no block got a hint.
pub fn motion_field(width: u32, height: u32, block: u32, hints: &[Motion], mv: &mut Vec<[i16; 2]>, valid: &mut Vec<u8>) -> bool {
    let cols = width.div_ceil(block);
    let rows = height.div_ceil(block);
    let n = (cols * rows) as usize;
    mv.clear();
    mv.resize(n, [0, 0]);
    valid.clear();
    valid.resize(n, 0);
    let mut any = false;
    for h in hints {
        if h.dx == 0 && h.dy == 0 || h.rect.w <= 0 || h.rect.h <= 0 {
            continue;
        }
        let b = block as i64;
        let (x0, y0) = (h.rect.x as i64, h.rect.y as i64);
        let (x1, y1) = (x0 + h.rect.w as i64, y0 + h.rect.h as i64);
        // blocks whose centre is inside the rect
        let bx0 = ((x0 - b / 2).max(0) + b - 1) / b;
        let by0 = ((y0 - b / 2).max(0) + b - 1) / b;
        let bx1 = ((x1 - b / 2 + b - 1) / b).clamp(0, cols as i64);
        let by1 = ((y1 - b / 2 + b - 1) / b).clamp(0, rows as i64);
        let v = [(-h.dx).clamp(-2047, 2047) as i16, (-h.dy).clamp(-511, 511) as i16];
        for y in by0..by1 {
            for x in bx0..bx1 {
                let i = (y * cols as i64 + x) as usize;
                mv[i] = v;
                valid[i] = 1;
                any = true;
            }
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    /// What NVENC reports for a frame coded at base QP `b` with `map`.
    fn avg(b: i32, map: &[i8]) -> u32 {
        (map.iter().map(|&d| (b + d as i32).clamp(0, 51)).sum::<i32>() as f64 / map.len() as f64).round() as u32
    }

    /// Commits the frame in `map` as coded at base QP `b`, ready at `t_ms`.
    fn commit(g: &mut Grid, b: i32, map: &[i8], cfg: &RefineConfig, t_ms: u64) -> Settled {
        g.commit(avg(b, map), map, cfg, 1, t_ms * MS, false).settled
    }

    #[test]
    fn a_coarse_change_is_refined_until_a_pass_reaches_done_qp() {
        let cfg = RefineConfig { max_passes: 60, ..Default::default() }; // done at QP 20
        let mut g = Grid::new(256, 128, 32); // 8 x 4 cells
        let mut map = Vec::new();

        // A change at t = 1000 ms in cell (1,1), coded coarsely (QP 40).
        g.begin_frame();
        g.mark_changed(&[Rect { x: 40, y: 40, w: 10, h: 20 }], 1000 * MS);
        assert_eq!(g.qp_map(&mut map, true, 1000 * MS, &cfg, 1, None).refined, 0);
        assert_eq!((map[9], map[0]), (0, 30));
        assert_eq!(commit(&mut g, 40, &map, &cfg, 1003), Settled::default());

        // Not due before the settle time; due at 1150 ms.
        assert_eq!(g.next_due(&cfg), Some(1150 * MS));
        g.begin_frame();
        assert_eq!(g.qp_map(&mut map, true, 1149 * MS, &cfg, 1, None).refined, 0);
        g.begin_frame();
        let m = g.qp_map(&mut map, true, 1150 * MS, &cfg, 1, None);
        assert_eq!((m.refined, map[9], m.bbox), (1, 0, Some(Rect { x: 32, y: 32, w: 32, h: 32 })));
        // A pass at QP 31: the average can't tell 31 from 50 with the rest skipped at
        // the ceiling, so it is another pass.
        assert_eq!(commit(&mut g, 31, &map, &cfg, 1153), Settled::default());

        g.begin_frame();
        assert_eq!(g.qp_map(&mut map, true, 1167 * MS, &cfg, 1, None).refined, 1);
        // QP 20: readable and lossless together, 170 ms after the change, a refined one.
        let s = commit(&mut g, 20, &map, &cfg, 1170);
        assert_eq!((s.readable, s.lossless), (Some(Took { ms: 170.0, refined: true, end: Ending::Stop }), Some(Took { ms: 170.0, refined: true, end: Ending::Stop })));
        assert_eq!(g.next_due(&cfg), None);
        g.begin_frame();
        assert_eq!(g.qp_map(&mut map, true, 2000 * MS, &cfg, 1, None).refined, 0);
    }

    #[test]
    fn a_fine_change_needs_no_refinement_and_passes_are_capped() {
        let cfg = RefineConfig { max_passes: 2, ..Default::default() };
        let mut g = Grid::new(64, 32, 32);
        let mut map = Vec::new();
        g.begin_frame();
        g.mark_changed(&[Rect { x: 0, y: 0, w: 8, h: 8 }], 1);
        g.qp_map(&mut map, true, 1, &cfg, 1, None);
        // typing on a fast link: reported as 34. Coded well at once: both times are the
        // frame's own, and nothing was refined.
        let s = commit(&mut g, 4, &map, &cfg, 4);
        assert_eq!(s.readable.map(|t| t.refined), Some(false));
        assert_eq!(s.lossless.map(|t| t.refined), Some(false));
        assert_eq!(g.next_due(&cfg), None);

        g.begin_frame();
        g.mark_changed(&[Rect { x: 40, y: 0, w: 8, h: 8 }], 1);
        g.qp_map(&mut map, true, 1, &cfg, 1, None);
        commit(&mut g, 50, &map, &cfg, 4);
        for _ in 0..2 {
            g.begin_frame();
            assert_eq!(g.qp_map(&mut map, true, 1000 * MS, &cfg, 1, None).refined, 1);
            commit(&mut g, 45, &map, &cfg, 1000);
        }
        assert_eq!(g.next_due(&cfg), None);
    }

    /// A coarse change in cell (0,0) at t = 1 s, coded at QP 40, then a refinement pass at 1150 ms.
    fn coarse_then_pass(g: &mut Grid, cfg: &RefineConfig, map: &mut Vec<i8>) {
        g.begin_frame();
        g.mark_changed(&[Rect { x: 0, y: 0, w: 8, h: 8 }], 1000 * MS);
        g.qp_map(map, true, 1000 * MS, cfg, 1, None);
        commit(g, 40, map, cfg, 1003);
        g.begin_frame();
        assert_eq!(g.qp_map(map, true, 1150 * MS, cfg, 1, None).refined, 1);
    }

    #[test]
    fn a_refined_cell_whose_frame_was_empty_is_parked_not_lossless() {
        let cfg = RefineConfig { max_passes: 60, ..Default::default() };
        let (mut g, mut map) = (Grid::new(64, 32, 32), Vec::new());
        coarse_then_pass(&mut g, &cfg, &mut map);
        // the pass coded nothing: its average QP says nothing, and the empty flag decides
        let s = g.commit(avg(51, &map), &map, &cfg, 1, 1153 * MS, true).settled;
        assert_eq!(s.lossless, None);
        assert_eq!(s.settled, Some(Took { ms: 153.0, refined: true, end: Ending::Parked }));
        assert_eq!(g.next_due(&cfg), None);
        // it reopens on its next change
        g.begin_frame();
        g.mark_changed(&[Rect { x: 0, y: 0, w: 8, h: 8 }], 2000 * MS);
        assert_eq!(g.next_due(&cfg), Some(2150 * MS));
    }

    #[test]
    fn a_frame_with_bytes_parks_nothing() {
        let cfg = RefineConfig { max_passes: 60, ..Default::default() };
        let (mut g, mut map) = (Grid::new(64, 32, 32), Vec::new());
        coarse_then_pass(&mut g, &cfg, &mut map);
        let s = commit(&mut g, 31, &map, &cfg, 1153); // readable, not at the stop, and not parked
        assert_eq!((s.lossless, s.settled), (None, None));
        assert_eq!(g.next_due(&cfg), Some(1150 * MS));
    }

    #[test]
    fn a_cell_at_the_stop_reports_stop() {
        let cfg = RefineConfig { max_passes: 60, ..Default::default() };
        let (mut g, mut map) = (Grid::new(64, 32, 32), Vec::new());
        coarse_then_pass(&mut g, &cfg, &mut map);
        let s = commit(&mut g, 20, &map, &cfg, 1153);
        assert_eq!(s.lossless.map(|t| t.end), Some(Ending::Stop));
        assert_eq!(s.settled, None);
    }

    #[test]
    fn a_cell_that_keeps_changing_is_never_refined_and_edges_clip() {
        let cfg = RefineConfig::default();
        let mut g = Grid::new(100, 70, 32); // 4 x 3 cells, the last ones partial
        let mut map = Vec::new();
        for t in 0..20u64 {
            g.begin_frame();
            g.mark_changed(&[Rect { x: 90, y: 60, w: 50, h: 50 }], (1 + t * 17) * MS);
            assert_eq!(g.qp_map(&mut map, true, (1 + t * 17) * MS, &cfg, 1, None).refined, 0);
            assert_eq!(commit(&mut g, 40, &map, &cfg, 2 + t * 17), Settled::default());
        }
        // (90,60) 50x50 clipped to the 100x70 frame touches cells (2..4, 1..3)
        let roles = g.roles();
        let changed: Vec<usize> = (0..roles.len()).filter(|&i| roles[i] == CellRole::Changed).collect();
        assert_eq!(changed, [6, 7, 10, 11]);
        assert_eq!(g.changed_rects(), [Rect { x: 64, y: 32, w: 36, h: 38 }]);
        // every change before the last was superseded, none reported
        assert_eq!(g.changes.len(), 1);
    }

    #[test]
    fn base_qp_inverts_the_average_nvenc_reports() {
        // a keystroke: 4 cells changed, the other 4156 at +30
        let mut map = vec![30i8; 4160];
        map[..4].fill(0);
        assert_eq!(avg(4, &map), 34);
        assert_eq!(base_qp(34, &map, 51), 5); // pessimistic by at most one (rounding)
        // a page flip: every cell changed
        assert_eq!(base_qp(46, &vec![0i8; 4160], 51), 46);
        // a small change coded coarsely: the clamped skip cells hide it, so assume the worst
        assert_eq!(base_qp(avg(30, &map), &map, 51), 51);
    }

    #[test]
    fn a_slow_link_gives_the_nearest_cells_the_lowest_qp_and_every_cell_the_bar_before_lossless() {
        let cfg = RefineConfig { max_passes: 60, ..Default::default() }; // spread 12
        let mut g = Grid::new(256, 128, 32); // 8 x 4 cells
        let mut map = Vec::new();
        g.begin_frame();
        g.mark_all(1000 * MS);
        g.qp_map(&mut map, true, 1000 * MS, &cfg, 1, None);
        commit(&mut g, 50, &map, &cfg, 1003); // the flip, coded coarsely

        // The pointer is in cell (6,1) = 14, the caret at the left edge of row 3, cell (0,3) = 24.
        let focus = Focus { caret: Some(Rect { x: 0, y: 100, w: 0, h: 20 }), pointer: Some((200, 48)) };
        let pick = Pick { focus, readable_first: true };
        g.begin_frame();
        let m = g.qp_map(&mut map, true, 1150 * MS, &cfg, 1, Some(&pick));
        assert_eq!(m.refined, 32);
        // by distance: 14, then 24, then 13 ...: from -12 for the nearest to +12 for the farthest,
        // all shifted by +3 so that their cost averages that of no offsets at all
        assert_eq!((map[14], map[24], map[13]), (-9, -8, -7));
        let far = (0..32).max_by_key(|&i| map[i]).unwrap();
        assert_eq!(map[far], 15);
        assert_eq!(far, 2); // cell (2,0), far from both

        // The base comes out at QP 38 every time: the cells whose offset is -4 or less (QP 34 or
        // better, give or take the inversion of the average) are readable, the others are not.
        assert_eq!(commit(&mut g, 38, &map, &cfg, 1153), Settled::default());
        let readable_now: Vec<usize> = (0..32).filter(|&i| g.readable[i]).collect();
        assert!((6..=10).contains(&readable_now.len()), "{readable_now:?}");
        assert!(readable_now.contains(&14) && readable_now.contains(&24) && !readable_now.contains(&far));

        // Next frame: the cells that aren't readable come first, whatever their distance to the
        // focus compared with the readable ones; the nearest of them is at -9 again.
        g.begin_frame();
        g.qp_map(&mut map, true, 1167 * MS, &cfg, 1, Some(&pick));
        let first = (0..32).min_by_key(|&i| map[i]).unwrap();
        assert!(!readable_now.contains(&first), "an unreadable cell goes first: {first}");
        assert_eq!(map[first], -9);
        assert!(readable_now.iter().all(|&i| map[i] > 0), "the readable ones come last");

        // A few frames more make the whole change readable, once its last cell is.
        let mut t = 1170;
        let settled = loop {
            let s = commit(&mut g, 38, &map, &cfg, t);
            if s.readable.is_some() || t > 1170 + 8 * 17 {
                break s;
            }
            t += 17;
            g.begin_frame();
            g.qp_map(&mut map, true, t * MS, &cfg, 1, Some(&pick));
        };
        assert_eq!(settled.readable.map(|t| t.ms), Some((t - 1000) as f64));
        assert_eq!(settled.lossless, None);

        // A frame that refines only 3 cells skips the rest just under the ceiling: 51 - 3 - 20 = 28.
        for i in 0..32 {
            g.done[i] = i >= 3;
        }
        g.begin_frame();
        let m = g.qp_map(&mut map, true, 2000 * MS, &cfg, 1, Some(&pick));
        assert_eq!((m.refined, map[10]), (3, 28));
    }

    #[test]
    fn under_a_user_cap_refinement_stops_at_the_luma_bar() {
        let base = RefineConfig { max_passes: 60, ..Default::default() };
        let (capped, free) = (base.for_rate(true), base.for_rate(false));
        assert_eq!((capped.done_qp, free.done_qp), (34, 20));
        for (cfg, lossless) in [(&capped, true), (&free, false)] {
            let mut g = Grid::new(64, 32, 32);
            let mut map = Vec::new();
            g.begin_frame();
            g.mark_all(0);
            g.qp_map(&mut map, true, 0, cfg, 1, None);
            commit(&mut g, 46, &map, cfg, 3);
            g.begin_frame();
            assert_eq!(g.qp_map(&mut map, true, 150 * MS, cfg, 1, None).refined, 2);
            // a pass at QP 30: readable under both, done only under the cap
            let s = commit(&mut g, 30, &map, cfg, 160);
            assert_eq!(s.readable.map(|t| t.ms), Some(160.0));
            assert_eq!(s.lossless.is_some(), lossless);
            assert_eq!(g.next_due(cfg).is_none(), lossless);
        }
    }

    #[test]
    fn under_a_user_cap_ordering_never_codes_a_cell_finer_than_the_bar() {
        let cfg = RefineConfig { max_passes: 60, ..Default::default() }.for_rate(true); // the bar is QP 34, spread 12
        let mut g = Grid::new(256, 128, 32); // 8 x 4 cells
        let mut map = Vec::new();
        g.begin_frame();
        g.mark_all(1000 * MS);
        g.qp_map(&mut map, true, 1000 * MS, &cfg, 1, None);
        commit(&mut g, 50, &map, &cfg, 1003);
        let focus = Focus { caret: Some(Rect { x: 0, y: 100, w: 0, h: 20 }), pointer: Some((200, 48)) };
        let pick = Pick { focus, readable_first: true };
        let cost = |map: &[i8]| map.iter().map(|&d| 2f64.powf(-d as f64 / 6.0)).sum::<f64>() / map.len() as f64;

        // Without a base to go by (nothing has come out yet): the offsets of an unclamped frame, -9 to 15.
        g.last_base = None;
        g.begin_frame();
        g.qp_map(&mut map, true, 1150 * MS, &cfg, 1, Some(&pick));
        assert_eq!((map.iter().min(), map.iter().max()), (Some(&-9), Some(&15)));

        // The last frame came out at base 38, so this one's is taken to be 36: no cell goes below
        // 34 = 36 - 2. The cells that would (the nearest half) sit at the bar, and the shift gives way so
        // that the others get what they saved: the farthest ends at 9 and not 15, and the cost is still
        // that of no offsets.
        g.last_base = Some(38);
        g.begin_frame();
        g.qp_map(&mut map, true, 1150 * MS, &cfg, 1, Some(&pick));
        assert_eq!(map.iter().min(), Some(&-2));
        assert_eq!((map[14], map[24], map[13], map[2]), (-2, -2, -2, 9));
        assert!((0.95..1.05).contains(&cost(&map)), "{}", cost(&map));
        // A base already near the bar (22, taken as 20): every cell is held at it; ordering has nothing left to order.
        g.last_base = Some(22);
        g.begin_frame();
        g.qp_map(&mut map, true, 1150 * MS, &cfg, 1, Some(&pick));
        assert!(map.iter().all(|&d| d == 14), "{map:?}");
        // The same frame without the cap: not clamped.
        g.begin_frame();
        g.qp_map(&mut map, true, 1150 * MS, &RefineConfig { max_passes: 60, ..Default::default() }, 1, Some(&pick));
        assert_eq!(map.iter().min(), Some(&-9));
    }

    #[test]
    fn window_move_hints_cover_the_blocks_inside_the_rect() {
        let (mut mv, mut valid) = (Vec::new(), Vec::new());
        // A window at (32,16) 64x32 that moved by (+8, -4): its blocks point back by (-8, +4).
        let hint = Motion { rect: Rect { x: 32, y: 16, w: 64, h: 32 }, dx: 8, dy: -4 };
        assert!(motion_field(128, 64, 16, &[hint], &mut mv, &mut valid));
        let cols = 8;
        let inside: Vec<usize> = (0..valid.len()).filter(|&i| valid[i] == 1).collect();
        let expect: Vec<usize> = (1..3).flat_map(|y| (2..6).map(move |x| y * cols + x)).collect();
        assert_eq!(inside, expect);
        assert_eq!(mv[expect[0]], [-8, 4]);
        // no motion, no hints
        assert!(!motion_field(128, 64, 16, &[Motion { dx: 0, dy: 0, ..hint }], &mut mv, &mut valid));
    }
}
