# Video pipeline

Remote's host video path on this host: RTX 5070 Ti (Blackwell, 2 NVENC engines), driver 615.78, NVENC API 13.1, Vulkan 1.4.351.

- **Milestone 1** (below first): the `media` module, the pipeline's public API for `remoted`'s session, and its measurements.
- **Early measurements** (further down): the capture daemon and NVENC probes.

## Milestone 1: the `media` API

`remote/host/src/media/` owns everything between the plugin socket and an encoded frame: the capture-ipc connection (both directions), Vulkan import and conversion, NVENC, damage, refinement, motion hints, the per-frame rate and split rules, loss recovery and the cursor channel. The session (`main.rs`, `session`) owns the network and calls it.

**Crate layout.** The host crate is now a library plus binaries: `src/lib.rs` exports `capture`, `gpu`, `nvenc` and `media`. `main.rs` should `use remoted::media::*` (and may drop its own `mod capture; mod gpu; mod nvenc;` lines, which compile a second copy). `src/bin/remoted-media.rs` is the media test and measurement tool; it is not installed.

### Types and calls

```rust
use remoted::media::*;

let media = Media::start(MediaConfig {
    socket: capture::plugin::default_socket(), // $XDG_RUNTIME_DIR/sodiumos/remote-capture.sock
    output: "REMOTE-1".into(),
    codec: Codec::Hevc,          // or Codec::Av1 (4:2:0 only)
    chroma444: true,             // HEVC only; false = NV12
    fps: 60,                     // the stream rate, = REMOTE-1's refresh
    start_bps: 8_000_000,        // until the first set_rate
    ..MediaConfig::default()     // refine: 150 ms settle, 2 passes; no dumps
}, Arc::new(move |ev: MediaEvent| { tx.send(ev).ok(); }))?;   // the sink: never block in it
```

| Call (all `&self`; `Media` is `Send + Sync`, share it in an `Arc`) | What it does |
|---|---|
| `Media::start(cfg, sink) -> Result<Media>` | Connects to the plugin (HELLO 4, accepts a v1–v4 plugin), STARTs `cfg.output`, spawns two threads (`media-ipc` reads the socket, `media-enc` owns Vulkan, CUDA and NVENC). Errors if the socket isn't there or the GPU or NVENC can't open. |
| `set_rate_source(Arc<dyn Fn() -> RateInput>)` | **Preferred.** Called before every frame: return core's current `VideoBudget` and `LinkState` (protocol.md §8.1), e.g. from a read of the driver's `EndpointHandle::status` watch. Must be quick and never block. |
| `set_rate(RateInput)` | The same, pushed (latest wins), for callers without a watch. Ignored once a source is set. |
| `predict_from(pts)` | Core's `RefHint::Reference(id)`, with `id` mapped back to `pts`: the client has `pts` and nothing usable after it. Every later frame is invalidated, and the next frame predicts from `pts` (it carries `reference: Some(pts)`). If `pts` has left NVENC's 8-frame DPB (or is from before the stream's keyframe), an IDR instead: NVDEC drops every frame of an intra refresh whose references are missing, so a refresh never recovers. The intra-refresh code stays, unused; no hint starts one. |
| `request_keyframe()` | An IDR next frame: core's `RefHint::Keyframe`, or a new client decoder. |
| `input(InputEvent) -> Result<()>` | Writes one INPUT_* message (capture-ipc 64–70) to the plugin **on the caller's thread**, at once. |
| `local_input(mode: u32) -> Result<bool>` | LOCAL_INPUT (capture-ipc v4): 0 desk input normal, 1 ignored, 2 ignored but desk keys reach the privacy screen. `Ok(false)`, nothing sent, to a v3 plugin. |
| `frame_clock(phase_offset_ns, refresh_mhz, grace_ns) -> Result<()>` | FRAME_CLOCK (capture-ipc 5), on the caller's thread. |
| `set_focus(caret: Option<Rect>, pointer: Option<(i32, i32)>)` | Where the user is looking, in buffer pixels. On a measured bottleneck, or a target under 0.6 × the session's demand, refinement takes the nearest cells first (below). **The session needn't call it:** the `media-ipc` thread already keeps both from the plugin's CARET (visible → the rect) and CURSOR_POS (visible → the hotspot). A call replaces both; the latest write wins, from either. |
| `stats() -> MediaStats` | Counters: frames and bytes per class, keyframes, invalidations, intra refreshes, last encode ms, split and VBV state, the latest readable and lossless times (below), the plugin's STATS and versions. |
| `stop(self)` (also on drop) | STOP, joins both threads. |

**Why media owns input too:** the plugin takes one connection, and the capture and input messages share it. `input()` doesn't go through the encoder thread, so it never waits behind an encode.

```rust
pub struct RateInput {                   // core's VideoBudget + LinkState (protocol.md §8.1) as plain data
    pub encoder_bps: u64,                // VideoBudget::encoder_bps (already capped at 0.65 × C_est, or thin)
    pub latency_critical: bool,          // VideoBudget::latency_critical
    pub thin: bool,                      // VideoBudget::thin (hedging)
    pub interval_remaining_bytes: usize, // VideoBudget::interval_remaining_bytes (refinement fills this)
    pub c_est_bps: Option<u64>,          // LinkState::c_est_bps
    pub split_ok: bool,                  // LinkState::split_ok
    pub user_cap: bool,                  // the client set a bandwidth cap (bandwidth_limit_kbps > 0): refinement stops at the luma bar
}
// RateInput::fixed(bps): no link knowledge (until the first budget, MediaConfig::start_bps is used this way)

pub enum MediaEvent {
    Frame(EncodedFrame),
    CursorShape(CursorShape),         // once per new hash: Host::send_cursor_shape(hash, image) (image.scale first)
    CursorPos { x: i32, y: i32, visible: bool, shape: u64, t_ns: u64 }, // buffer pixels: Session::send_cursor_position
    Caret { visible: bool, purpose: u32, rect: Rect, t_ns: u64 },       // capture-ipc CARET (8 password, 9 PIN)
    PointerLock { state: u32, rect: Rect },                             // 0 none, 1 locked, 2 confined
    Animation(bool),                  // the plugin's HINT anim start / settle: Session::set_animation
    PluginStats { frames: u64, dropped_no_buffer: u64, dropped_no_client: u64 },
    Stopped(String),                  // capture ended (plugin STOPPED, socket closed, GPU error); both threads exit
}

pub struct EncodedFrame {
    pub pts: u64,                // media's own frame number from 0
    pub data: Vec<u8>,           // HEVC Annex-B or AV1 OBUs (one temporal unit)
    pub keyframe: bool,          // -> OutgoingFrame.keyframe
    pub reference: Option<u64>,  // -> FrameOpts { reference: Some(frame_id of that pts) }
    pub recovery_point: bool,    // -> FrameOpts { recovery_point: true }
    pub class: FrameClass,       // Interactive | Passive | Refine
    pub meta: Vec<u8>,           // -> OutgoingFrame.meta (format below)
    pub width: u32, pub height: u32,
    pub t_rendered_ns: u64,      // the plugin's FRAME time (CLOCK_MONOTONIC); for Refine, the capture it refines
    pub t_encoded_ns: u64,       // bitstream ready
    pub info: FrameInfo,         // stage times, QP and base QP, cells changed/refined, hint blocks, rate and split used,
                                 // readable_ms / lossless_ms (Option<f64>) when this frame completes a change
}

pub struct CursorShape {         
    pub hash: u64,
    pub image: remote_core::msg::CursorImage, // width, height, hot_x, hot_y, scale: 1.0, pixels: raw premultiplied BGRA as the plugin gives it
}

pub enum InputEvent {            // capture-ipc v3, 64-70; buffer pixels, evdev codes, t_ns host CLOCK_MONOTONIC (0 = now)
    PointerAbs { x: f64, y: f64, t_ns: u64 },
    PointerRel { dx: f64, dy: f64, dx_unaccel: f64, dy_unaccel: f64, t_ns: u64 },
    Button { button: u32, pressed: bool, t_ns: u64 },
    Axis { source: u32, axis: u32, value: f64, value120: i32, stop: bool, more: bool, t_ns: u64 },
    Key { keycode: u32, pressed: bool, t_ns: u64 },
    Gesture { kind: u32, phase: u32, fingers: u32, dx: f64, dy: f64, scale: f64, rotation: f64, t_ns: u64 },
    Reset,                       // release everything this connection holds (the plugin also does it on close)
}
```

**Session duties:**
- **Budget:** `set_rate_source` with a closure that reads the driver's `LinkStatus` (`budget`, `link`) into a `RateInput`, and `user_cap` = the client's `bandwidth_limit_kbps > 0`.
- **Cursor:** on `MediaEvent::CursorShape { hash, image }` set `image.scale` to REMOTE-1's scale (media leaves 1.0) and call `Host::send_cursor_shape(hash, image)`; core encodes it with `msg::encode`. There is no PNG and no format byte any more: the client reads `msg::CursorImage`.
- **HUD and stats:** show `MediaStats::readable_ms` and `lossless_ms` (the latest page flip or other change that needed refinement, all of its cells; "readable" is luma SSIM about 0.987, three frames (50 ms) before 0.99, and "lossless" is every plane at the QP 20 stop; there is no separate caret number:), or aggregate `FrameInfo::readable_ms` / `lossless_ms` over the frames that carry them.
- **Frames:** send each `EncodedFrame` with `send_video_frame_with(now, OutgoingFrame { data, keyframe, meta }, FrameOpts { reference, recovery_point })`, after mapping `reference` (a `pts`) to core's frame id. Keep a map of `pts` ↔ frame id for the last 64 frames or so, both ways.
- **Reference hints:** on `take_reference_hint()` (or a new `ref_hint_seq` on the watch), `Reference(id)` → `predict_from(pts of id)`; `Keyframe` → `request_keyframe()`. Hedging (§7.2) comes through the same hint.
- **Animation:** forward `MediaEvent::Animation(a)` to `Session::set_animation(a)` (`Command::Animation`).
- **Resize:** change REMOTE-1's mode as usual: the plugin sends new buffers, and the next frame is a keyframe at the new size.

### What media decides per frame

- **Change detection, then damage.** The plugin's damage says a frame exists; the conversion shader then compares every converted word with the previous frame's and flags each encoder cell (32×32 for HEVC, 64×64 for AV1) that really changed. Apps often damage a whole window for a caret blink or a keystroke (Qt does); only the changed cells are coded. A damaged frame with no changed pixel produces no frame at all.
- **QP map:** changed and refined cells at the rate controller's QP (delta 0; on a measured bottleneck the refined cells carry the offsets below); unchanged cells +30, so NVENC codes them as skip.
- **Refinement (build to lossless), adaptive:** a cell is *done* once it was coded at a base QP of 20 or better. A cell that isn't done and hasn't changed for 150 ms is coded again, once per frame interval, until a pass comes out at QP 20 or better, or after a second's worth of passes, or as soon as a pass codes nothing (parked).
  - **Inside the budget:** each refinement frame gets core's fresh-interval budget less what media already sent in the interval (a frame counts for half an interval, as in core), at least 2 kB or it waits. Refined cells use delta 0 (or the bit-neutral offsets below), so NVENC's rate control keeps the frame inside its one-frame VBV (which a refinement frame always has, whatever the link:).
  - Negative refinement deltas override the rate control: the first design (two passes at −10 and −16) sent 100–157 kB refinement frames on a 9 Mbit/s link (5–8× the frame budget) and stopped at SSIM 0.94 on a dense page.
  - **The base QP:** NVENC's `frameAvgQP` averages every CTB *after* its delta. A keystroke with the rest of the frame at +30 reports 34 for a base QP of about 4. media inverts the clamped average over the map it sent, pessimistically. A change coded finely on a fast link (typing: base 4–5) is done at once and costs no refinement.
  - Refinement rides in passive frames. On its own it is a separate frame, at most one per interval and at least half an interval after any frame. An animation's settle hint makes its rect due at once.
  - **Readable and lossless times.** A cell is *readable* once it was coded at base QP 34 or better (the luma bar: Y SSIM 0.986–0.990 on dense text, measured below), *lossless* once at 20 or better, or out of passes. A *change* is the cells one captured frame changed (a page flip, a keystroke); it is readable when all its cells are, lossless when all are. A cell that changes again leaves its change, and a change with no cell left was superseded and is never reported. media reports the time from the change's own frame (`t_rendered`) to the bitstream that completed it, on that frame's `FrameInfo::readable_ms` / `lossless_ms`. `MediaStats::readable_ms` / `lossless_ms` keep the latest of the changes that needed refinement, so a keystroke coded well at once does not replace the page flip before it. The target stays every plane at SSIM ≥ 0.99, and base QP 20 stays the stop in both chroma modes. A change that ends parked or at the cap reports `settled_ms` with its ending; `lossless_ms` is the stop only. The SSIM is measured on 4:2:0 planes in either mode (a 4:4:4 stream's chroma and the source's both box-filtered 2×2 as the client's shader does; luma at full resolution).
  - **Never above the controller's target (protocol §7.1 stands, checked).** A refinement frame's rate is `min(encoder_bps, what is left of the interval)`, and `encoder_bps` is core's target or lower; refinement folded into a passive frame runs at the passive rate, the same target; an interactive frame carries none. The rate controller's one-frame VBV decides what comes out: refinement frames at a 9 Mbit/s target have p50 17.3 kB and p90 19.0 kB against an 18.75 kB budget (max 23.7 kB), the average over any second stays under 8.2 Mbit/s, and any 250 ms (15 frames) peaks at 9.8–9.9, of which the flip's own frame, 27 kB at QP 51 against an 18.75 kB budget, is a tenth. At 3 Mbit/s it is 3.1–3.3 over a second. Ordering the cells changes none of it (the same figures with `focus_refine` off).
  - **On a measured bottleneck, or a target under 0.6 × the session's demand (40 Mbit/s at 2940 × 1848 at 60 Hz; `c_est_bps` is Some, or `encoder_bps` < 0.6 × `demand_bps`).** A user cap, start-up (8 Mbit/s until the probes reach the demand, about 2 s) and the thin hedge are budget-bound with no standing queue, and the LAN's target sits well above 40 (the core's ceiling is 300 Mbit/s), so the target decides, not a measurement; cells are ranked afresh every frame, so a target crossing 40 only changes the next map (no hysteresis). The due cells of a refinement frame are put in order of their distance to the caret and the pointer (the `set_focus` points, from the cell's centre; the caret is a rect, 0 inside it), and their QP offsets run from −12 for the first to +12 for the last (`RefineConfig::order_spread`, by rank, not by pixels; 20 was tried and not kept: below). The rate controller sets the frame's base QP for its budget, so the nearest cells get the most of it (twice the bits per 6 QP) and the farthest the least, and the budget is spent in full whatever the content costs. The cells are ranked afresh every frame: as the nearest finish, the next ring moves up. Two details keep the rate controller honest:
    - *Bit-neutral offsets.* Offsets that average 0 cost more bits than none (bits double per 6 QP), and a rate controller that sizes the frame's QP for none overshoots, 45 kB against 18.75 kB in the first design. The offsets are shifted up by the QP that makes their cost average that of none (+3 for ±12; +7 for ±20).
    - *A readable skip delta.* With few cells at the base QP and the rest at +30, every skipped cell sits at the QP ceiling (51) and NVENC's average QP says nothing about the base. A frame that orders its cells skips by 51 − 3 − (the bar it aims for), at least 10: +14 in the readable phase, +28 for lossless. A due cell that gets no bits at that QP just waits.
    - Cells that are not readable yet come before cells that are, so the whole page reaches the luma bar before anything goes on to lossless (`RefineConfig::readable_first`; off: strictly by distance, which gets the cells at the caret to lossless sooner and the page to readable later: below).
    - With no focus point at all the order is raster. `MediaConfig::focus_refine = false` gives every due cell the same QP again.
    - *Rejected:* taking only as many cells a frame as its budget pays for (a learned bytes-per-cell). It worked at 9 Mbit/s but not below: the rate controller alternated 5.8 kB and 0.2 kB frames at 3 Mbit/s and used 60 % of the target.
  - **Under a user-set bandwidth cap** (`RateInput::user_cap`) refinement stops at the luma bar: `done_qp` becomes `readable_qp`, so a cell is done, and lossless_ms equals readable_ms, once it is readable.
- **Class:** Interactive when core says latency-critical, or media itself knows (input through `input()` in the last second, an animation in flight, a window-move or scroll hint for this frame); otherwise Passive. An Interactive frame core didn't know about gets the 0.65 × C_est cap here.
- **Split and VBV:** `split_ok` → forced 2-way split with a half-frame VBV on Interactive frames only; refinement and passive frames, and every frame otherwise, are unsplit with the one-frame VBV, so a pass fills its interval's budget (a half-frame VBV with the split gave 25–53 kB passes and 18 of them at 65 Mbit/s); never while thin. NVENC switches by reconfigure at the next frame (no reset, no IDR: measured below), so there is no need to wait for a still moment.
- **Motion hints:** HINT kinds 3 and 4 for exactly the next FRAME (`seq`), when no frame was skipped in between: the blocks whose centre lies in a moved rect get the vector back to where they were. Every other block, and every frame without hints, gets "no hint" (NVENC needs hints on every P frame once they're enabled).
- **Loss:** frames after the reference are invalidated and their changed cells coded again in the next frame. A reference that has left the DPB (8 frames: 133 ms at 60 Hz, 67 ms at 120 Hz) or is from before the stream's keyframe gets an IDR, about one frame's budget under the one-frame VBV (99.8 kB at 50 Mbit/s, 1.3× at 8), soft, which refinement sharpens like any page flip. The client's watchdog (three decodable frames with no picture → `KeyframeRequest`) stays as the safety net for decoder errors. A long-term reference refreshed every 500 ms would turn most of these into an invalidation (22 kB against an IDR's 50–100 kB): M2.


### Wire formats media defines (big-endian, like protocol.md)

- **Frame meta** (`OutgoingFrame.meta`): `u8 version = 1 | u8 class (0 passive, 1 interactive, 2 refine) | u16 n | n × (u16 x, u16 y, u16 w, u16 h)`: what this frame changes on screen, in buffer pixels (for a refinement frame, the region it sharpens). At most 32 rects, then their bounding box.
  - **Records after the rects** (added later): `u8 tag | u16 len | body`, skipped by a reader that doesn't know the tag. Tag 1, **host stamps**, 32 bytes: `u64 rendered, captured, converted, encoded`, µs on the host's session clock (the clock of the frame's `send_us`): the plugin's FRAME time, remoted had it, convert done, bitstream ready. `remoted`'s stream appends it to every frame (media's `wire::frame_meta` doesn't know the session clock), so a client can show and log each stage.
- **Cursor image:** not media's format. It is core's `msg::CursorImage`: `width`, `height`, `hot_x`, `hot_y`, `scale`, and the plugin's pixels as they are (ARGB8888 premultiplied, B,G,R,A in memory, `width × 4` bytes a row). The size is the output's pixels (the plugin draws the cursor at REMOTE-1's scale); media sets `scale` 1.0 and the session overwrites it with REMOTE-1's. The PNG option, the format byte and the `png` dependency are gone.

## Results

**Setup:**
- **Where:** one nested Hyprland 0.57 with REMOTE-1 at 2560×1664, scale 1.6, `look.lua` (blur, springs), and the real sodiumwm plugin built into a scratch directory.
  - capture-ipc v3 from the plugin branch (91fa6d3, exported read-only);
  - the v2 plugin in this checkout for the first runs.
- **Harness:** a nested-compositor script (not part of this repository) runs `remoted-media` (media's pipeline plus a stand-in for core's budget) for each workload. Analysis scripts summarise the runs.
- **Encoder:** HEVC 4:2:0, a 50 Mbit/s target unless noted.
- **Capture → bitstream** is the plugin's FRAME time (`t_rendered`) to the bitstream being ready.
- **GPU load:** the GPU was shared with the user's other jobs. The early runs saw 65–100 % SM from others, and stage times there were about 2× these.

**The real plugin end to end:**
- HELLO 3; v2 and v3 plugins both work.
- BUFFERS, FRAME with fences, RELEASE, STATS, CURSOR_SHAPE (memfd; PNG on the wire then, raw since) and CURSOR_POS work, and so do HINT (v3), CARET, INPUT_KEY injection through `media.input()` and STOP.
- **Found and fixed in the plugin branch** (bfe0336): a crash in `CHints::collect` on every window move.

| Workload (v3 plugin) | 60 Hz | 120 Hz |
|---|---|---|
| Capture → bitstream, typing, p50 / p95 ms | 2.8 / 4.1 | 3.4 / 4.3 |
| Key press (INPUT_KEY) → frame rendered, p50 / p95 ms | 1.5 / 3.0 | 1.8 / 5.2 |
| **Key press → bitstream ready**, p50 / p95 ms | **4.5 / 5.8** | **4.9 / 9.0** |
| Capture → bitstream, window drag, p50 / p95 ms | 4.0 / 5.3 | 3.8 / 5.2 |
| Same on a LAN (`split_ok`: 2-way split, half-frame VBV, interactive frames), p50 / p95 ms | 2.9 / 3.7 (encode 1.4) | 2.7 / 4.6 (encode 1.3) |
| **Idle**, editor window with a blinking caret, kbit/s | **4** | **4** |
| Cursor-only motion (61 CURSOR_POS, no video frames), kbit/s | 4 (the caret) | 3 |
| Typing at about 14 keys/s, kbit/s; frame size p50 | 44; 0.6 kB | 42; 0.6 kB |
| Window drag (8 px a step), Mbit/s; frame p50 with / without hints | 8.7; 21.7 / 55.3 kB | 15.5; 28.2 / 28.8 kB |
| Workspace slide; frame p50 / p95 with hints, without | 6.7 / 25.3 kB, 5.6 / 30.0 kB | 4.8 / 14.4 kB |

- **Change detection pays:** Qt damages its whole window for a caret blink or a keystroke.
  - Before it, idle cost 64 kbit/s: every blink refined the whole window.
  - With it, a blink is a 0.2–0.3 kB frame and nothing else, and a keystroke 2–4 cells.
- **Motion hints:**
  - They halve a drag at 60 Hz (but frame sizes vary between runs by ±30 % with the spring timing).
  - They change nothing at 120 Hz, where steps are 4 px and NVENC finds them itself (as the NVENC probe found at 8 px).
  - Slides are a wash.
  - A frame with no hints pays about 12 % more on large motion nobody hinted (the probe below); that is the price of keeping hints enabled.
- **Rate control:**
  - **Bottleneck** (C_est 10 Mbit/s, target 9, latency-critical): every interactive frame is encoded at 6.5 Mbit/s (0.65 × C_est); frames p50 5.4 kB, p95 11.9 kB against a 13.5 kB budget.
  - **Thin hedge** (4 Mbit/s for 1 s): applied at the next frame and lifted the same way, with no keyframe.
  - **Split:** `split_ok` switches to the 2-way split with a half-frame VBV at the next frame.
- **Reference invalidation:**
  - frames 30, 31 and 55 lost, `predict_from(last decoded)` two frames later;
  - the frames after it were invalidated (3 each time) and the next frame carries `reference`;
  - the stream as the client got it (71 of 77 frames) decodes with no decoder messages.

**Build to lossless, first design** (a dense page of small text in four colours, 2112×1600 px, replaced at once every 2.5 s; `--dump` runs for SSIM against the encoder's input). Superseded by the next section for 9 Mbit/s, where every cell of the page was refined at once and all planes were not reached in 30 passes:

| | 50 Mbit/s, 60 Hz | 50 Mbit/s, 120 Hz | 9 Mbit/s (C_est 10), 60 Hz |
|---|---|---|---|
| The flip's own frame | 99.8 kB, QP 44; SSIM Y 0.96, YUV 0.88 | 50 kB; Y 0.91, YUV 0.83 | 27 kB, QP 50; Y 0.86–0.88, YUV 0.78–0.83 |
| Refinement starts | 153 ms | 153 ms | 153 ms |
| **Y SSIM ≥ 0.99** | pass 2, **170 ms** | pass 5–6, 186–195 ms | pass 16–21, 403–488 ms |
| **YUV SSIM ≥ 0.99** | pass 6, **236 ms** | pass 13, 254 ms | not reached: 0.981–0.983 after 30 passes (bandwidth-bound) |
| Done (QP ≤ 20) | 7 passes, 254 ms, 677 kB | 15 passes, 271 ms, 720 kB | the pass cap (then 30; now a second's worth) |
| Refinement frame size | 84–105 kB (budget 104) | 46–53 kB | 15–20 kB (budget 18.75) |

- **Times without dumps:** the pass at which SSIM crosses comes from the `--dump` runs, and its time from the same pass in the runs without dumps (passes every interval). A dump stalls the encoder about 20 ms, so the dump runs' own times are longer: 287 ms to YUV 0.99 at 60 Hz.
- **HEVC 4:4:4:** done in 8 passes, 271 ms.
- **AV1:** done in 5 passes, 220 ms. Its flip frames overshoot the one-frame VBV: 139–149 kB against 104 kB.
- **Typing, editor:** keystrokes are coded at base QP 4–5 and need no refinement. SSIM ≥ 0.999 in the changed region in the frame itself.

**Text refinement on every plane and on slow links (this tree, 60 Hz, HEVC).**

- **Setup:** the same dense page. The `m1-nested.sh` workloads `pageq` (50 Mbit/s), `pagebnq` (9 Mbit/s target, C_est 10), `pagefocusq` (9 Mbit/s with a caret and the pointer at one spot, 672,512 in buffer pixels) and `pagebnallq` (`--no-focus-refine`: every due cell at the same QP, as before), each with `--dump`; `m1_quality.py` does the SSIM against the encoder's input in the stream's chroma mode, "every plane" being ffmpeg's SSIM `All`. A dump is now taken once per captured page (at its first refinement frame), so it stalls the encoder once per page and not on every pass.
- **Reading the tables:** the time is from the capture's `t_rendered` to the bitstream of the first frame whose SSIM reaches the bar, and the kB are what was sent by then. Medians over the 4 captures of a 9 s run (3 where the fourth is cut off). The first capture of a run is slower than the rest (start-up: 484 against 398 ms to readable at 9 Mbit/s). The GPU was shared: 3–26 % SM from other processes, no other NVENC sessions.

| Flip → | 50 Mbit/s 4:2:0 | 50 Mbit/s 4:4:4 | 9 Mbit/s 4:2:0 | 9 Mbit/s 4:4:4 | 9 Mbit/s 4:2:0, no ordering |
|---|---|---|---|---|---|
| **readable** (Y SSIM ≥ 0.99), ms / kB | **181** / 307 | **196** / 308 | **431** / 348 | **522** / 445 | 396 / 316 |
| **every plane ≥ 0.99**, ms / kB | **248** / 694 | never: 0.984 at QP 20 (0.98 at 283 ms / 835 kB) | **849** / 763 | never: 0.978 (0.98 at 1035 ms / 955 kB) | 864 / 739 |
| media's QP bars: readable (≤ 34) / lossless (≤ 20), ms | 183 / 267 | 196 / 296 | 398 / 1119 | 434 / 1168 | 395 / 1106 |

- **4:2:0 meets at both rates.** Every plane reaches 0.99 at 248 ms (50 Mbit/s) and 849 ms (9 Mbit/s), and media's lossless time (QP ≤ 20) agrees with it within 20 ms at 50 Mbit/s. At 9 Mbit/s the pages in this run took 1.1 s to the QP stop, 0.25 s after the SSIM crossing.
- **4:4:4 does not reach every plane at 0.99 at QP 20:** Y is 0.9988, but all planes together stop at 0.9845. The stop that does is QP 16 (`--done-qp 16`): 327 ms and 1063 kB at 50 Mbit/s (4:2:0: 694 kB); QP 18 gives 0.9878. The stop depends on the chroma mode (open question 1).
- **The same stop QP 20 in both modes, measured on the same planes.** `m1_quality.py` now box-filters a 4:4:4 stream's decoded chroma and the source's to 4:2:0 (the client shader's 2×2 filter) before the SSIM and keeps luma at full resolution, so "every plane" means the same in both modes. `pageq` in 4:4:4 at 50 Mbit/s again (4 captures): **every plane ≥ 0.99 in 262 ms / 702 kB** (4:2:0: 248 ms / 694 kB; luma 0.99 at 195 ms; every plane 0.98 at 229 ms), after the last refinement 0.9951. The full-resolution chroma alone, printed alongside as information, ends at 0.9846 (as before) and never reaches 0.99; it is the same content at twice the resolution, which low-contrast chroma scores lower at the same quantiser. `done_qp` 18 for `chroma444` was the fallback and is not needed. On a Mac client, look at coloured text at the stop on the Retina panel, and if there are chroma fringes the stop is 18 there (0.9878 at full resolution).
- **The luma bar is a little loose for dense text:** when media reports the page readable its Y SSIM is 0.987–0.988, and it reaches 0.99 about 50 ms (three frames) later; a refinement that stops at the bar (user cap) ends at Y 0.986–0.989 at 50 Mbit/s. Near the caret, where the text is densest, the page is at 0.9915 by then. The bar is QP 34 as inferred from NVENC's average, which is pessimistic by up to one QP (open question 2).
- **Ordering costs the whole page little:** Y 0.99 at 431 against 396 ms, every plane at 849 against 864 ms, and the stream rate is the same (below).

**Caret first** (9 Mbit/s, 4:2:0; caret and pointer at one spot; "box" is 400×256 px around it, 12×8 cells, "page" the 2112×1600 region):

| Flip → | ordered, readable first (default): box / page | strictly by distance: box / page | no ordering: box / page |
|---|---|---|---|
| Y SSIM ≥ 0.98, ms | **203** / 370 | 200 / 434 | 294 / 328 |
| every plane ≥ 0.98 | **443** / 768 | 317 / 785 | 647 / 631 |
| Y ≥ 0.99 | **244** / 462 | 235 / 601 | 371 / 396 |
| every plane ≥ 0.99 | **520** / 918 | 442 / 901 | 913 / 864 |
| media's QP bars for the page: readable / lossless | 420 / 1144 | 988 / 1105 | 395 / 1106 |

- **The text at the caret is sharp in about half the time:** every plane at 0.99 in 520 ms against 913 ms with no ordering, luma 0.99 in 244 against 371 ms, and luma 0.98 within 203 ms, while the page as a whole is 6–17 % later (Y 0.99 at 462 against 396 ms, every plane at 918 against 864 ms).
- **Strictly by distance** (`--no-readable-first`) is faster still at the caret (442 ms to every plane) but leaves the far text unreadable for 988 ms against 420: media's readable time for the page doubles. It stays a switch.
- **`order_spread`** (QP offsets ±N), box / page, Y 0.99 and every plane 0.99: N = 6: 298 / 440 and 606 / 884 ms; **N = 12 (default): 244 / 462 and 520 / 918**; N = 20: 202 / 466 and 467 / 912. A wider spread helps the caret and costs the page nothing measurable here, but also makes the first frames of a flip carry more bits for the rate controller to guess; 12 is what the other runs use.
  - **Re-check of the stop QP: 12 stays.** 9 Mbit/s (C_est 10, caret and pointer at one spot), 4 captures, 20 / 12 on the same day: the box at Y 0.99 in 200 / 275 ms and every plane in 464 / 505 ms; the page at Y 0.99 in 459 / 476 ms and every plane in 900 / 913 ms. The rate check passes at both: at most 7.9–8.1 Mbit/s over any second (limit 8.2) and 9.6–9.8 over any 250 ms (limit 9.9), 11.0–11.3 over 100 ms. At **3 Mbit/s** (C_est 3.3; three runs of 4 captures each, 20 / 12) the box gets to Y 0.98 a little sooner (248, 240, 256 / 269, 258, 268 ms) and to every plane at 0.98 in all four captures (972, 962, 1031 / 1130, 1112, 1129 ms, three of four captures), but **to Y 0.99 in 904, 895, 901 ms against 405, 449, 426**: more than twice as long (not traced; the box's cells may be marked readable early and then wait behind the page's unreadable cells, readable first). The page itself is the same (Y 0.98 at 828, 820, 874 / 822, 827, 820 ms), and so is the rate (worst second 3.5–3.6 Mbit/s with the start-up keyframe's refinement in it at either spread, 2.8–3.0 after the first 1.2 s, as the 3.1–3.3 before). So 20 helps at 9 Mbit/s and costs the caret's 0.99 at 3; kept at 12 (the stated fallback). It is `RefineConfig::order_spread` and `--order-spread`, one line, if the policy is revisited (open question 4).
- **At 3 Mbit/s (C_est 3.3)** the same ordering gets the box to Y 0.98 in 270 ms against 625 ms, and to Y 0.99 in 413 ms where no ordering never does; the page's Y 0.98 comes at 855 against 698 ms. No run reaches every plane at 0.98 on the page within the 2.5 s between flips. The stream rate is 3.1–3.3 Mbit/s over the worst second.

**Under a user-set bandwidth cap** (`user_cap`, refinement stops at the luma bar; 4 pages in 9 s):

| | 50 Mbit/s | 9 Mbit/s |
|---|---|---|
| Refinement kB, capped / not | 1139 / 3521 (−68 %) | 1839 / 3770 (−51 %) |
| Stops (readable = lossless), ms | 182 | 505 |
| Quality at the stop | Y 0.986–0.989, every plane 0.945 | Y ≥ 0.99 at 492 ms (3 of 4), every plane 0.956 |

**A 10 Mbit/s user cap on a fast link, with the ordering of 7** (the target is the cap, no measured bottleneck, `--user-cap`, caret and pointer at one spot, 60 Hz, 4:2:0; the `pagecap` workload; with ordering / without it, `--no-focus-refine`, which is what a cap did before):

| | ordered | unordered |
|---|---|---|
| The caret's box (12×8 cells) at Y 0.98 | **286** ms / 111 kB | 362 ms / 206 kB |
| The page at Y 0.98 | 468 ms / 333 kB | 412 ms / 268 kB |
| media's readable time for the page (all cells ≤ QP 34), median of 5 flips | 456 ms | 374 ms |
| Refinement bytes in the 9 s (5 flips and the start-up keyframe) | 1546 kB | 1238 kB |
| Worst second after the first 1.2 s | 3.2 Mbit/s | 2.6 Mbit/s |

- **The caret's box is readable before the page** (286 against 468 ms), as intended, and 76 ms sooner than unordered. At the cap's stop (luma bar, Y about 0.987) only one capture in three had the box at Y 0.99 (352 ms).
- **The page pays more than expected** ("4 %"): 22 % later and 25 % more refinement bytes than unordered (the same in the dumped run: 553 against 434 ms, 1513 against 1240 kB). Under a cap a cell stops at the bar, and the cells near the caret are coded at base − 9 or finer, below the bar by their offset: bytes the cap exists to save. The cap's saving over refining to lossless is still large (−51 % at 9 Mbit/s above). Open question 2 (answered next:).

**Under a cap, never finer than the bar** (`RefineConfig::bar_floor`, `BASE_DRIFT`). Under `user_cap` a cell's QP is `max(base + offset, readable_qp)`: the nearest cells get the bar (QP 34), the farthest the coarse end, and the bit-neutral shift is worked out over the clamped offsets (`neutral_shift`: it comes out lower, −3 against +3 with half the cells at the floor, so the others get what the clamped ones no longer spend). NVENC's rate control sets the base, so the floor is the last frame's base less `BASE_DRIFT` = 2 QP (the base falls about 2 a frame while a page is refined, 1 to 4); with the last frame's own the cells at the floor still came out finer than the bar. Same `pagecap` workload, 60 Hz, 4:2:0, target 10 Mbit/s, caret and pointer at one spot, medians of whole runs of 4 captures (3 runs before the change, 9 after, 6 unordered):

| | ordered, before | ordered, bar as floor | unordered (`--no-focus-refine`) |
|---|---|---|---|
| The caret's box (12×8 cells) at Y 0.98 | 205 ms / 101 kB | **209 ms / 101 kB** | 288 ms / 210 kB |
| The caret's box at Y 0.99 | 244 ms / 152 kB | 266 ms / 173 kB | not reached at the stop |
| media's readable time for the page (all cells ≤ QP 34) | 483 ms (+28 %) | **375 ms (−0.3 %)** | 376 ms |
| Refinement bytes in the 9 s | 1790 kB (+25 %) | **1447 kB (+0.8 %)** | 1435 kB |
| The page (region) at Y 0.98 | 386 ms / 329 kB | 365 ms / 297 kB | 335 ms / 258 kB |
| Worst second after the first 1.2 s | 3.2 Mbit/s | 2.5–2.7 | 2.5–2.6 |

- **The box is readable as soon as before** (Y 0.98 at 209 against 205 ms, the same 101 kB; 79 ms sooner than unordered), and the page, as media counts it (every cell at QP 34 or better) and in refinement bytes, is the same as unordered (−0.3 % and +0.8 %; the decision's 22 % and 25 % were these two rows). The fallback (ordering off under a cap, `pick()` without `user_cap`) is not needed.
- **What stays:** the page region's luma SSIM 0.98 comes 9 % later and after 15 % more bytes than unordered (365 against 335 ms, 297 against 258 kB): the far cells are the last to reach the bar, and the region's SSIM waits for them while unordered raises every cell together. By the other measure above it is the same; if this row is the page's measure, ordering off under a cap is `pick()`'s `&& !rate.user_cap`, one line. The box's Y 0.99 is 22 ms later than before (the near cells stop at the bar instead of past it; the cap's promise is the bar).
- **The drift** (`BASE_DRIFT` 0 / 2 / 3 / 4, media's readable ms, refinement kB): 403 / **375** / 375 / 384 and 1614 / **1438** / 1517 / 1522. At 3 and 4 the box's Y 0.99 comes later (341 and 294 ms).

**The rate** (see "Never above the controller's target"): at the 9 Mbit/s target the stream averages at most 8.2 Mbit/s over any second, ordered or not, and 9.7–9.9 over any 250 ms, of which a flip's own frame is a tenth. At 3 Mbit/s it is 3.1–3.3 over a second.

**Parking and the settle wait (2940×1848, scale 2.0, 60 Hz, HEVC, the media harness at a fixed rate, one RTX 5070 Ti).**
- **The empty slice.** `encode.rs` finds a frame's slice bytes in the bitstream (HEVC: the VCL NAL units of an Annex-B stream, types below 32, start codes and zero padding excluded; AV1: the tile-group and frame OBUs), parameter sets and SEI left out. At start, after the encoder is opened and its surfaces registered, it encodes the still-unwritten surface as an IDR, then six P frames of the same surface with every cell at +30 (nothing to code), and keeps the largest slice. **That constant is 139–141 B at 5336 cells (2940×1848, HEVC)**, not "a few dozen": the skip flags cost a bit or two a CTU. A refinement frame is empty when `slice ≤ constant + 16 + refined_cells / 2`: the mixed deltas of a refinement map cost a little even when nothing is coded (live empty frames are 137–175 B in total; a map with a quarter of the cells at delta 0 and the rest skipped codes nothing in 469 B), and any real coding of a cell costs far more. Folded refinement (a passive frame) carries changed cells and is never empty. `MediaStats::empty_slice_bytes` and `remoted-media`'s last line carry the constant.
- **What parking does** (e2e stack, 8.3 Mbit/s target, so the cells' base QP stays at 21–22 and each change still needs its one pass): a click run at 120 Hz sends **109–113 refine-class frames** where it sent 3265–3293 (the same bed back to back: 3265); the passes of an animation's trail are one empty frame per change, not up to 60. The media harness `anim` workload at 8 Mbit/s: 12 refine frames in 20 s (before: about 2400). Input → photon, 100 clicks at 120 Hz, 2940×1848 (target 8.4 Mbit/s throughout): 19.4 / 25.6 ms p50 / p95 and 20.4 / 27.4 (two runs, the second with a user job holding the GPU at 99 %), against 18.9 / 29.2 for the previous build run back to back on the same tree (16.8 / 21.3 at the pass on a quieter machine): the machine's drift, not the change, which touches nothing before the bitstream. The 30 s idle window reads 8.25 kbps (worst second 9.0) in both builds: it begins after the clicks' refinement, and the refine-class frames were the clicks' own.
- **Slow-link text is unchanged.** `pageq` at 9 000 kbit/s, 4:2:0, 2940×1848: all planes at SSIM 0.99 after 1007 ms median (slowest 1027 ms) in 3 of 4 captures, readable at 453 ms (luma 0.99); at 8 Mbit/s before the change 1130 ms, 3 of 4 (the fourth capture is the stream's first picture, which ends at the cap both times). The only parked change in the run is the 980 static background cells of a page flip's predecessor, at 716 ms (375 B for 980 cells); the flip's own 4356 cells run their passes of 17–19 kB to the cap, ending `cap` at 1.14–1.16 s as before.
- **`pageq` at 50 Mbit/s by settle wait** (medians of 4 flips to every plane at SSIM ≥ 0.99, bitstream time from `t_rendered`; the 150 ms rows are this build's baseline, the 283/270 ms of before are within 10 ms):

  | settle | 4:2:0 | 4:4:4 |
  |---|---|---|
  | 150 ms (default) | 274 ms (slowest 277) | 280 ms (slowest 290) |
  | 120 ms | 237 ms (242) | 244 ms (256) |
  | 100 ms | 223 ms (227) | 222 ms (226) |

  The time falls by about 1 ms for each ms of settle, and 4:4:4 costs 6 ms over 4:2:0 at 150 and nothing at 100.
- **Scroll-and-stop** (`m1-nested.sh scroll`; a 400-line dense page in `scroll.qml`, one step of 3 lines (51 px) every 120 ms for 2 s, then still for 3 s; 50 Mbit/s, two runs each; the steps are Down keys, because a virtual pointer's wheel never reaches a Qt window in the nested instance; the pixels are those of a wheel tick, without the plugin's scroll hints). Refine-class bytes of the run (the first page's refinement, about 195 kB, included in all):

  | settle | 4:2:0 | 4:4:4 |
  |---|---|---|
  | 150 ms | 302–304 kB (19 frames) | 399–401 kB (20) |
  | 120 ms | 1.01–1.12 MB (21) | 1.12–1.23 MB (27–28) |
  | 100 ms | 2.13–2.14 MB (28–29) | 2.67–2.74 MB (34–35) |

  At 100 ms every step is refined once before the next one lands 20 ms later: **7.0 times the refinement bytes of 150 in 4:2:0 and 6.7 times in 4:4:4**, over item 3's 20 %. **So the floor is 120 ms**, by 9's rule. Not a safe floor in itself: steps exactly 120 ms apart sit on the edge of a 120 ms settle (3.5 times the bytes at 150), and a wheel's ticks are never so even, so a settle under 150 is a trade the Mac's number has to earn. The default stays 150.

**No change elsewhere** (typing, idle, bottleneck, keys, cursor): typing 44 kbit/s, capture → bitstream 2.9 / 3.7 ms p50 / p95; idle 4 kbit/s; a 10 Mbit/s bottleneck's interactive frames 6.3 / 12.7 kB at 6.5 Mbit/s; the cursor arrives as one 38×38 image (5776 raw bytes, scale 1.0) with CURSOR_POS after it.

**Probes** (`nvenc_probe` tests `split`, `hintorder`, `hintfill`, `small`; evidence in the run's `probe/`):
- **`splitEncodeMode` changes by `nvEncReconfigureEncoder`** with no reset and no IDR, in both directions, for HEVC and AV1. Encode time follows the mode at once: 2.30 ms split, 3.19 ms unsplit, under load. So the split rule needs no still moment.
- **HEVC hint order:** raster 16×16 and z-order inside each CTB give the same result (23.8 kB against 114.8 kB without hints, left half scrolling 96 px); media uses raster.
- **Once hints are enabled at init, every P frame must carry them** ("SetupCEAHints failed" otherwise). Invalid (refidx 31) and zero vectors cost the same: +12 % on 64 px of motion nobody hinted, −18 % at 8 px.
- **A small change under +30 elsewhere:** CBR 50 Mbit/s spends 82 kB a frame on a 64×64 change without a map (it re-codes the whole static frame) and 5.5 kB with +30 on the other CTBs. Its `frameAvgQP` is 43 = base 13 + 30, which is how the average was found to include the deltas.

**In short:**

- NVENC answers yes to everything the probe asked, except emphasis maps (H.264 only) and AV1 4:4:4.
- Forced 2-way split-frame encoding halves encode time, but loosens the one-frame VBV.
- `remoted` uses **Route A: NVENC through CUDA interop**. Vulkan Video encode works here too, but it takes about 17 ms a frame against NVENC's 1.2 ms, and it can't do split-frame encoding or motion hints.
- From the compositor finishing a frame to the bitstream being ready takes 2.3–3.0 ms p50 and about 5 ms p95 at 2560×1664, at 60 and 120 Hz.

## What was built

```
 capture                  import                     convert                        encode
 ─────────────────────    ──────────────────────     ──────────────────────────     ───────────────────────────
 PluginSource             Vulkan VkImage from the    compute shader (texelFetch     cuImportExternalMemory
  (capture-ipc v2)   ─►   DMA-BUF: DRM modifier,  ─► through a UNORM view)       ─► (opaque fd) → CUDA pointer
 WaylandSource             dedicated memory,          XRGB → NV12 or YUV 4:4:4,      → nvEncRegisterResource
  (ext-image-copy-         once per ring buffer       BT.709 full range, into a      → NVENC HEVC/AV1, P1, ULL,
   capture-v1, GBM                                    buffer exported as an          CBR, 1-frame VBV, infinite
   buffers)               acquire: sync_file →        opaque fd                      GOP, split forced
                          Vulkan semaphore;                                          → Annex-B / OBU file
                          queue family FOREIGN
```

- **Capture sources** (`remote/host/src/capture/`) share one trait, `CaptureSource`: `buffers()` returns the ring and its generation, `next_frame()` blocks, and `release()` hands a buffer back.
  - `plugin.rs` is the client side of `remote/docs/capture-ipc.md`. It speaks **v2**, revision 2 (HELLO 2, STOPPED), and accepts a v1 plugin. It was tested against a fake plugin (memfd "DMA-BUFs", a fenced FRAME, RELEASE, STOP). It hasn't met the real plugin yet.
  - `wayland.rs` is the ext-image-copy-capture-v1 fallback:
    - It allocates a 3-deep ring with GBM on the compositor's DMA-BUF device. Modifiers are the compositor's offer intersected with what Vulkan can import and sample (NVIDIA block-linear `0x300000000606014` here).
    - It asks for frame N+1 before frame N is converted and encoded.
    - It exports the buffer's implicit write fence with `DMA_BUF_IOCTL_EXPORT_SYNC_FILE`, so the GPU waits on the compositor's copy instead of trusting `ready` alone.
- **Import** (`gpu.rs`) uses `VK_EXT_image_drm_format_modifier` with an explicit plane layout and `VK_EXT_external_memory_dma_buf`.
  - Each frame takes ownership from `VK_QUEUE_FAMILY_FOREIGN_EXT` in `GENERAL` (not `UNDEFINED`, which may discard the contents) and gives it back afterwards.
  - Output buffers move to `VK_QUEUE_FAMILY_EXTERNAL` for CUDA.
- **Conversion** (`shaders/rgb_to_yuv.comp`):
  - One invocation handles a 4×2 block and writes whole 32-bit words.
  - BT.709 is applied to the sRGB-encoded values, as Y′CbCr is defined; there is no linearisation.
  - 4:2:0 uses a 2×2 box filter, and the VUI says so (chroma_sample_loc 1).
  - A unit test checks both outputs against a CPU reference on a 70×38 image, edge blocks included.
- **Encode** (`csrc/nvenc_shim.c`): a small C shim over CUDA and NVENC. NVENC's structs are large, versioned bitfields, so the C compiler lays them out from ffnvcodec-headers. `libcuda.so.1` and `libnvidia-encode.so.1` are loaded at run time.
  - VUI: BT.709 primaries and matrix, sRGB transfer, full range.
  - `set_bitrate` (reconfigure, no IDR) and `invalidate` are in place for rate control, and the probe verified both.
- **Sync:** the convert pass waits on a CPU fence before `nvEncEncodePicture`. That fence wait is part of the `convert` figure below.

### Running it

```sh
cd remote && cargo build -p remoted          # needs glslc (shaderc), ffnvcodec-headers, mesa's libgbm
WAYLAND_DISPLAY=<nested socket> remoted capture --output REMOTE-1 --fps 60 --frames 600 \
    --out out.hevc --stats stats.csv [--chroma 444] [--codec av1] [--input argb] [--split off] \
    [--source plugin --socket $XDG_RUNTIME_DIR/sodiumos/remote-capture.sock] [--dump-source DIR]
```

The measurement tools behind every number here are test scripts and probes that are not part of this repository:

| File | What it does |
|---|---|
| `run.sh`, `nvenc_probe.c`, `check.py` | the NVENC probe |
| `vkvideo_probe.c` | Route B |
| `nested-capture.sh`, `nested.lua`, `anim.c` | End-to-end runs in a nested Hyprland with a headless REMOTE-1 and stub animated clients (no installed apps) |
| `quality.py` | PSNR and SSIM |
| `timing.py` | Stage tables with GPU load |

## NVENC on the RTX 5070 Ti

`nvenc_probe` uses the NVENC API directly, with CUDA device buffers registered as input, the way `remoted` feeds it.

- **Content:** a synthetic desktop (windows of text, a photo panel, a gradient) scrolling by a known vector.
- **Settings:** P1, ultra-low-latency tuning, CBR, one-frame VBV, infinite GOP, synchronous mode.
- **Latency** means `nvEncEncodePicture` until a blocking `nvEncLockBitstream` returns.
- **Decode checks** are done with ffmpeg and dav1d.

| Question | Answer | Measured |
|---|---|---|
| HEVC 4:4:4 | **yes** | p50 / p95 ms, split off → forced 2-way: 2560×1664 **3.18 / 4.08 → 1.68 / 2.10**; 2880×1864 4.16 / 5.45 → 2.21 / 3.09; 3840×2160 6.12 / 6.57 → 3.26 / 4.75 |
| HEVC 4:2:0, for reference | yes | 2560×1664 **2.03 / 2.64 → 1.07 / 1.75**; 2880×1864 2.51 / 3.30 → 1.36 / 2.03; 3840×2160 3.88 / 4.59 → 1.98 / 3.19 |
| AV1 4:2:0 | **yes** | 2560×1664 2.14 / 3.50 → 1.21 / 2.10; 2880×1864 2.72 / 3.49 → 1.60 / 2.02; 3840×2160 3.98 / 4.65 → 2.19 / 2.63 |
| AV1 4:4:4 | **no** | `nvEncInitializeEncoder`: "Invalid chroma format"; caps YUV444 = 0 |
| Forced split-frame (2 NVENCs) | **yes** (`NUM_ENCODER_ENGINES` = 2) | Encode is 1.7–1.9× faster at every size. But frames overshoot a one-frame VBV: P frames p95 2.05×, max 2.50× the per-frame budget, against 1.02× and 1.11× unsplit. A half-frame VBV with split gives p95 1.29×, max 1.60×; on content that's new every frame, max 0.98×. |
| Per-frame bitrate change (`nvEncReconfigureEncoder`, no reset, no IDR) | **yes** | 0 I/IDR frames in 61 calls; a call takes 0.015–0.03 ms. Split off, content that always wants more than its budget: 40 Mbit/s 0.99× budget, 8 Mbit/s 1.30× (QP pinned at 51, the content can't fit), back to 40: 0.98×. Alternating 10/40 every frame: 1.15× / 1.00×. AV1 tracks too, more loosely (1.59× / 0.90×). |
| Reference invalidation (`nvEncInvalidateRefFrames`) | **yes**, HEVC and AV1 | Frames 38–39 were dropped and invalidated before frame 40. The lossy stream decodes all 58 frames, and frames 40–59 are **bit-identical** to the full decode. Control without invalidation: "Could not find ref with POC 39", 20 frames lost. The recovery frame cost 22 kB against 18 kB, where an IDR is 50 kB at QP 50. |
| Intra refresh | **yes**: periodic and on demand (`forceIntraRefreshWithFrameCnt`); single-slice IR for HEVC | An 8-frame refresh spreads over 8 frames of 55–80 kB (QP 35–42) at a 62.5 kB budget, instead of one IDR. A stream cut at the refresh start decodes **bit-identical from frame 47** (8 frames later). |
| Emphasis map | **no** (H.264 only) | caps `SUPPORT_EMPHASIS_LEVEL_MAP` = 0 for HEVC and AV1; `qpMapMode = EMPHASIS` fails to initialise |
| QP delta map | **yes** (HEVC per 32×32 CTB, AV1 per 64×64 SB) | On a static frame, −12 QP on one quarter gave that region **+2.4 dB** (34.3 → 36.7) and the rest +0.0, for 14 kB more. AV1 (−48 qindex): +1.8 dB. This is enough for "build to lossless". |
| External motion-vector hints | **yes**, HEVC (16×16, 1 candidate) and AV1 (SB hints), split on or off | Constant QP 28, page scrolling 160 px/frame: HEVC **279 → 74 kB** per frame and 2.74 → 1.87 ms; 400 px: 703 → 188 kB. AV1 160 px: 365 → 64 kB. Wrong hints (0, −dy) make frames bigger, so the hints are used. At 8 px/frame NVENC finds the motion alone. |
| P1 + ULL + CBR + one-frame VBV | **yes** | Unsplit: steady P frames mean 0.66×, max 1.11× budget. A scene cut costs 0.93× (QP 49 for a frame, then it recovers). |
| ARGB input (NVENC converts) | yes, with the VUI's matrix | Decoded Y′ matches BT.709 full range. RGB PSNR is the same as our own YUV 4:4:4 (43.02 dB). 4:4:4 survives, and latency equals NV12 input (1.12 vs 1.07 ms split). |

**Other caps (HEVC / AV1):**

- 10-bit 1/1; 4:2:2 1/0
- dynamic resolution 1/1
- LTR 7/6
- sub-frame readback 1/1
- temporal filter 1/1
- weighted prediction 1/0
- full list in the evidence `s3/caps.md`

**GPU load during the probe:**

- 18–54 % SM from other processes
- 0 NVENC sessions from others (Sunshine idle)
- a re-run under 29–40 % SM reproduced the latencies within 0.15 ms

## Encode route: A (NVENC via CUDA interop)

Route B was tried with `vkvideo_probe.c`: VK_KHR_video_encode_h265 at 2560×1664, the same content, CBR 30 Mbit/s, ultra-low-latency tuning, desktop content hint.

| | Route A: NVENC via CUDA | Route B: Vulkan Video encode |
|---|---|---|
| Encode latency, HEVC 4:2:0 2560×1664 | **1.07–1.21 ms p50** (split), 2.0–2.1 ms unsplit | **16.7 ms p50**, 19.2 ms p95, CBR. Quality levels 0–4: 16.9–18.5 ms. Constant QP: 8.5–10 ms. The declared frame rate (30/60/240) changes nothing. |
| HEVC 4:4:4 / AV1 4:2:0 | yes / yes | yes / yes (caps) |
| Split-frame encoding | yes (forced 2-way) | no API |
| External motion hints | yes | no API |
| Per-frame bitrate change | yes (measured) | yes: `vkCmdControlVideoCodingKHR` with no reset (measured: 0.95× budget at 30 Mbit/s; alternating 1.08× / 0.98×) |
| Reference invalidation | yes (`nvEncInvalidateRefFrames`, measured) | yes, because the application owns the DPB: frame 60 predicted from 57 decoded bit-identical (60/60 frames) |
| Intra refresh | yes (measured) | per-picture-partition, cycle ≤ 64 (caps only) |
| QP delta map / emphasis | yes / no | yes / no (caps) |
| Bitrate ceiling | the level's | **120 Mbit/s** (caps) |
| Driver quirks found | none | CBR is rejected (`VK_ERROR_INVALID_VIDEO_STD_PARAMETERS_KHR` at `vkEndCommandBuffer`) unless every rate-control layer chains `VkVideoEncodeH265RateControlLayerInfoKHR`, which the spec makes optional. Found by diffing against FFmpeg's `hevc_vulkan` under the API-dump layer. |

**Why A:**

- **Latency.** Route B costs about 15 ms more per frame. That is the whole input-to-photon budget for encoding several times over. At 120 Hz, B can't keep up.
- **Controls.** B lacks two of the controls the design relies on: forced split and motion hints for window moves and scrolls. B's one structural advantage, an application-owned DPB, is matched by NVENC's invalidation API.
- **Cost of the interop.** It's small. The CUDA import happens once per buffer, and the per-frame Vulkan → CUDA hand-off is a fence wait.
- **Our shader, not NVENC's ARGB conversion, even though NVENC converts correctly.** In the live pipeline the shader is a little faster: convert 0.21 vs 0.37 ms p50, encode 1.94 vs 2.06 ms at 120 Hz 4:4:4, at equal quality. It also stays the place to read FP16, if the plugin ever hands that over.

## Stage timings at 2560×1664 (nested Hyprland, ext-image-copy-capture)

**Setup:**

- **Scene:** a headless REMOTE-1 at 2560×1664 in a nested Hyprland 0.57 with this checkout's `look.lua` (blur, springs).
  - Two stub clients redraw every frame (scrolling text, a bouncing box, a drifting gradient), with a third on workspace 2.
  - Workspaces slide every 1.2 s.
  - 10 s per run.
- **Encoder:** HEVC, CBR 50 Mbit/s, forced split. Here the stream averages 14–19 Mbit/s: this content is cheap.

**What each stage measures:**

- **capture:** compositor presentation time → `ready` in remoted.
- **import:** once per ring buffer: 0.4–0.6 ms (1 run: 2.0 ms), then 0 per frame.
- **convert:** submit → fence. It includes waiting on the compositor's copy fence, the acquire barrier and the compute pass.
- **encode:** NVENC submit → bitstream ready.
- **total:** compositor presentation → bitstream ready.

| Run | fps | Missed frames | capture p50 / p95 | convert p50 / p95 | encode p50 / p95 | **total p50 / p95** | GPU SM %, all / user's other processes | NVENC % |
|---|---|---|---|---|---|---|---|---|
| 60 Hz, 4:2:0 | 60.0 | 1 / 599 | 0.47 / 1.95 | 0.23 / 1.23 | 1.46 / 2.53 | **2.52 / 4.87** | 45 / 21 | 6 |
| 120 Hz, 4:2:0 | 118.5 | 11 / 1199 | 0.45 / 2.04 | 0.23 / 1.54 | 1.35 / 2.57 | **2.30 / 5.04** | 56 / 19 | 11 |
| 60 Hz, 4:4:4 | 60.0 | 1 / 599 | 0.46 / 1.97 | 0.21 / 1.65 | 2.02 / 3.18 | **2.97 / 5.68** | 42 / 23 | 9 |
| 120 Hz, 4:4:4 | 118.1 | 13 / 1199 | 0.46 / 1.87 | 0.21 / 1.25 | 1.94 / 2.97 | **2.86 / 5.29** | 55 / 17 | 18 |
| 60 Hz, 4:4:4 from ARGB | 59.8 | 1 / 599 | 0.54 / 1.95 | 0.37 / 1.51 | 2.10 / 3.69 | 3.38 / 5.91 | 48 / 22 | 9 |
| 120 Hz, 4:4:4 from ARGB | 118.8 | 12 / 1199 | 0.58 / 2.51 | 0.37 / 1.89 | 2.06 / 4.39 | 3.44 / 7.51 | 62 / 22 | 18 |
| 60 Hz, 4:2:0, split off | 60.0 | 1 / 599 | 0.47 / 1.96 | 0.25 / 1.25 | 2.50 / 3.74 | 3.55 / 5.96 | 42 / 27 | 6 |

**GPU load while measuring:**

- "User's other processes" is the card's SM load minus remoted and the nested Hyprland it captured. That covers the live session, the Claude app, the user's apps and other agents' nested instances.
- It was 17–27 % in these runs, and no other process held an NVENC session.
- With heavier load from others (53 % SM, an earlier run), the capture p95 rose to 14.6 ms and 8 % of frames were missed. The encode p95 stayed under 4 ms.
- CPU time to record and submit is 0.04 ms a frame.

**What this says about the fallback path:**

- On ext-image-copy-capture, Hyprland reports **100 % damage on every frame** (measured: 180 of 180 frames). That matches its screenshare `TODO`.
- About 1 % of frames are missed at 120 Hz, because the protocol asks for one frame at a time. The plugin path pushes frames, so it shouldn't miss them.

## Quality

The captured frames were dumped every 10th frame (`--dump-source`), and the stream was decoded with ffmpeg. Each row is the mean over frames 10–290, with the minimum in brackets. 60 Hz, CBR 50 Mbit/s target.

| Stream | Mbit/s | RGB PSNR dB | RGB SSIM | Y′ PSNR dB | Y′ SSIM |
|---|---|---|---|---|---|
| HEVC 4:2:0 (our shader) | 16.9 | 31.46 (30.60) | 0.9768 (0.9705) | 51.00 | 0.9989 (0.9979) |
| HEVC 4:4:4 (our shader) | 13.5 | **40.86** (33.10) | **0.9937** (0.9863) | 50.91 | 0.9990 (0.9982) |
| HEVC 4:4:4 (ARGB, NVENC converts) | 13.7 | 42.20 (34.49) | 0.9943 (0.9840) | 51.19 | 0.9990 (0.9977) |

- Luma reaches SSIM 0.999 in every mode, past the "visually lossless" bar of SSIM ≥ 0.99. 4:2:0 loses the colour of coloured text (RGB 31 dB), which 4:4:4 keeps (41–42 dB). This is the case for the design's "Auto: 4:4:4 if the Mac decodes it".
- Frame 0, an IDR under a one-frame VBV, is soft: Y′ 26 dB, SSIM 0.91. By frame 10 it's at 49 dB.
- The two 4:4:4 rows are separate captures of live animation, so their frames differ. On identical frames (the NVENC probe), the shader and NVENC's own conversion score the same.
- An AV1 capture (`--codec av1`) decodes cleanly with dav1d: 4:2:0, full range, BT.709.

## Missing

- **The ext-image-copy-capture fallback** isn't behind `media` yet: `media` is plugin-only. main.rs's early `capture --source wayland` still works.
- **Refinement next to a long animation:** refinement frames need half an interval free after any frame, so a page beside an animation that runs at the full frame rate is refined only once it stops. It does ride in passive frames.
- **Exact per-cell QP:** "done" and "readable" use the base QP recovered from the frame average, pessimistically (up to one QP high), and 51 when the skipped cells hide it. The slice QP in the headers doesn't give it either (ffmpeg's `trace_headers`: it differs from the inferred base by −30 to +4, tracking the first CTB's QP); NVENC's block-level output statistics would make it exact.
- **Refinement below about 5 Mbit/s:** ordering works (3 Mbit/s: the caret's box at Y 0.98 in 270 ms against 625 ms) but the page needs more than the 2.5 s of the test between flips to reach every plane, so lossless is the pass cap there ("or out of passes", about a second at 60 Hz) and media reports it as `settled_ms` with the ending `cap`, not as lossless.
- **AV1** overshoots the one-frame VBV on big changes (139–149 kB against 104 kB), as the probe saw.
- **Overlap** of the compute pass and NVENC, and **FP16 input**: still not needed (stage times above).
- **main.rs** compiles its own copy of `capture`, `gpu` and `nvenc` instead of using the library. `build.rs` links the shim into it explicitly until it switches to `remoted::...`.
- **Not measured here:** the client side (decode, present), and real networks; the budget came from a stand-in for core.

## Decided, and open

**Decided (built and measured above):** the target is every plane at SSIM ≥ 0.99 with base QP 20 as the stop; the luma crossing (base QP 34) is reported as the readable time; refinement stays under the controller's target; on a measured bottleneck, or a target under 40 Mbit/s, the cells nearest the caret and the pointer go first; under a user-set cap it stops at the luma bar. 300 ms for the whole page holds at 50 Mbit/s (248 ms in 4:2:0, 262 ms in 4:4:4 on the same planes). At 9 Mbit/s the page is readable in 0.4–0.45 s and sharp on every plane in 0.85–0.9 s; the text at the caret in 0.5 s.

**Decided:** (1) QP 20 stays in both chroma modes, measured on 4:2:0 planes; (2) a loss beyond the 8-frame DPB gets an IDR, not an intra refresh; (4) the luma bar stays at QP 34, and the HUD says what "readable" is; (5) readable-first stays, and "readable" and "lossless" are the latest change that needed refinement, all of its cells (no separate caret number); (6) `order_spread` 20 — **not kept**: the 3 Mbit/s check failed, so 12 stays (above); (7) ordering applies on a measured bottleneck or a target under 40 Mbit/s.

**Decided:** (12) `order_spread` stays 12 with no rule by target: two data points (3 and 9 Mbit/s) and an untraced cause would make a rule that encodes a symptom. **Next:** trace the 3 Mbit/s run with per-cell QP stats ( 4's precision item: NVENC's block-level statistics give each cell's QP, not the base recovered from the average), fix the cause (the box's cells marked readable early, then waiting behind the page's unreadable cells, readable first), then try 20 again at every target. (13) Ordering stays under a user cap, with no cell coded finer than the bar (measured above).

## Open questions

Questions 1–5 are decided (above). Open now:

1. **`order_spread` 20 or 12** (decided: 12 stays; a later run traces the 3 Mbit/s run with per-cell QP stats and tries 20 again at every target). 20 is better at 9 Mbit/s (the caret's box at luma 0.99 in 200 ms against 275, every plane in 464 against 505) and at 3 Mbit/s for the box's luma 0.98 and every plane at 0.98, but takes 900 ms to the box's luma 0.99 at 3 Mbit/s where 12 takes 400–450 ms (three runs each). Kept at 12 ( 6's fallback). Is the 3 Mbit/s box worth more than the 9 Mbit/s one, or is there a spread (or a rule by target) that has both?
2. **Ordering under a user cap costs the page** (decided: kept, no cell finer than the bar). On a 10 Mbit/s cap the page was readable 22 % later and cost 25 % more bytes than unordered, for the caret's box 76 ms sooner at Y 0.98; with the bar as the floor it is 0.3 % sooner and 0.8 % more bytes, the box 79 ms sooner (above). Left: the page region's luma SSIM 0.98 is 9 % later and 15 % more bytes than unordered.

(An earlier open question, split against the one-frame VBV, was decided: split by link state. NVENC switches without a reset.)
