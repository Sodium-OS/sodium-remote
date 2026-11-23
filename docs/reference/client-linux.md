# The Linux client

`remote/client-linux` (crate `remote-client`) is Remote's test client for Linux. It pairs and connects through `remote_core::connect`, decodes, presents in a Wayland window (or headless), draws the cursor locally, sends input, reports its vsync grid for the phase lock, and runs the click-to-photon test. It exists so the whole system can be tested end to end, unattended, in nested Hyprland, until the Mac client can be built.

- **The split:** the platform-independent parts live in `remote/client-engine` (crate `remote-engine`: connect and pair, the session loop, decode and the FFmpeg shim `csrc/decode_shim.c`, stats, the click test, the timeline, layout, clipboard sync, the scripted mode); `client-linux` is the Linux platform layer on it (`window/`, `hid.rs`, the Wayland clocks and the default-route check in `connect.rs`, the eventfd waker; `Linux` implements `remote_engine::Platform`). Flags, exit codes, file formats and behaviour are unchanged, except `--codec auto|hevc|h264`, and `Hello.decode` now comes from a probe of the decoders instead of fixed `true`s. Paths below that say `decode.rs`, `stats.rs`, `latency.rs`, `status.rs`, `proto.rs`, `client.rs` or `window/layout.rs` are now in `client-engine/src/` (`layout.rs` beside them); see `docs/client-windows.md` for the engine and its API.
- **The harness:** `tests/e2e/` (the click test surface, `vinput`, `run.sh`, `analyze.py`). The host is the real `remoted`; the stand-in host is gone.
- **Status:** pairs and connects on the LAN path against core's test host (`connect_demo`); end-to-end runs against the real host are in `tests/e2e/`. See "Results" and "Missing".

## Running it

```sh
cd remote && cargo build --release -p remote-client

# Pair: the host shows a 6-digit code (Settings › Remote, or remoted's pairing window).
remote-client --data DIR pair [--code NNNNNN] [--host ID] [--no-rendezvous] [--no-mdns] [--timeout S]

# Connect (the default command): the only paired host, or --host.
remote-client --data DIR [connect] [--host ID] [--headless] [--fullscreen] [--takeover]
              [--no-rendezvous] [--no-mdns] [--relay-only] [--bind IP]... [--lan-hint ID@IP:PORT]...
              [--decoder auto|cuda|vaapi|sw] [--stats-dir DIR] [--secs N] [--fps N] [--max-mbps N]
              [--size WxH] [--fixed] [--ppp F] [--no-hud] [--title T] [--input-log FILE] [--no-zero-copy]
              [--no-audio | --audio] [--audio-log FILE] [--no-clipboard | --clipboard] [--script FILE]
              [--clicks N [--marker X,Y,W,H] [--click-gap MIN,MAX] [--idle-ms N] [--stay]]
```

- **`--data DIR`** holds the device key (`device.key`) and `paired.json`: a `keys::FileStore`, created if missing. Required, so a test never touches `~/.local/share`.
- **`--host ID`** is the host's full ID or its code (`XXXX-XXXX`, any case, hyphen optional). `--name` sets the name the host shows (default "Linux reference client").
- **`pair`** looks for a host with an open pairing window on mDNS and the rendezvous (up to `--timeout`, default 15 s), takes the only one (or `--host`), and runs SPAKE2 with `--code` (else `$REMOTE_PAIR_CODE`). It prints `paired <ID> <code> "<name>"`. **Exit:** 0 paired, 2 wrong code, 3 the window closed, 4 failed or no host.
- **`connect`** exits 0 when the user ends it (a signal, closing the window, `--secs`, the click test finishing, or `Disconnected(User)`), 3 when the session ends otherwise (`Disconnected { reason }`: hold expired, taken over, revoked, host shutdown…), 4 when it never connects (offline, busy without `--takeover`, not paired, refused). 1 is a usage or setup error.
- **Network:** `--no-rendezvous` is LAN only (mDNS and the host's LAN listener); `--relay-only` shares only relayed candidates; `--bind` (repeatable) sets `ClientConfig.bind` (default every IPv4 interface); `--lan-hint` adds a host's LAN listener for when mDNS can't be used. `RUST_LOG=info` shows the core's own log.
- **Stream:** `--fps` asks for a rate (default the display's); `--max-mbps` becomes `StreamSettings.bandwidth_limit_kbps` (default Auto); `--size WxH` asks for exactly that size and never resizes (tests; headless default 2560×1664, the 13-inch Air); `--fixed` keeps the first size and scales the picture; `--ppp` overrides `pixels_per_point` (default the output's scale, headless 1.741, the Air's; `tests/e2e/run.sh` passes 1.741 unless given another, so every e2e run stands in for the 13" Air:); `--takeover` answers "In use by X" by connecting again with takeover.

- **Clipboard and sound:** on by default with a window, off when `--headless` (the shell a test runs from is usually the live desktop), unless `--clipboard` / `--audio`. `--audio-log FILE` writes the played level once a second (`secs,dBFS,frames decoded,non-silent`). `--script FILE` runs the engine's timed inputs (`client-engine/src/script.rs`).

**Build needs:** ffmpeg (libavcodec, libavutil), libwayland-client and -egl, libEGL, libpipewire and libopus (audio out); at run time `nvidia-utils` for NVDEC and dav1d for software AV1. Nothing new was installed.

## How it works

```
 tokio (2 workers)                     decode thread                  main thread
 ────────────────────────────────      ────────────────────────       ──────────────────────────────────
 remote_core::connect::Client:         FFmpeg (csrc/decode_shim.c)    window/: xdg_toplevel, EGL + GLES 3
 pairing, signalling, ICE, Noise  ──►  NVDEC (CUDA) → VA-API → SW  ──► latest frame wins; draw + commit
 session(): Hello, KeyTable, events    frame → picture                when the last frame callback fired;
   → decoder / window / stats          stamps: decode in/out, ready   wp_presentation feedback per commit
 timeline(): chip + pill, 10 Hz        click test: marker luma        → receiver report, Vsync lead
 click test scheduler                                                  display → Hello, Resize on configure
```

- **Connecting** (`connect.rs`, `client.rs`): `Client::start` with a `FileStore` at `--data`, then `connect(host, hello, takeover)`. The window publishes its display first (after its first configure), so `Hello` is right from the start. Right after `connect` succeeds the client sends `Msg::KeyTable(vec![])` (PC passthrough); the core sends it again after `Hello` on every reconnect. `Busy` with `--takeover` connects again with takeover.
- **Clocks:** every stats file uses the client's own clock (µs since the run started, with its CLOCK_MONOTONIC epoch on the first line). Each connection has its own session clock (`Client::session_epoch()`); on `Connected` the client records its offset (`Live`), and frames' µs fields are moved onto the client clock with it. The host's stamps go through the frame's own `send_us` against `send_local_us`.
- **Decode** (`decode.rs`, `csrc/decode_shim.c`): FFmpeg 9 through a small C shim. Low-latency settings: no frame threads, `AV_CODEC_FLAG_LOW_DELAY`, dav1d's frame delay 1. `auto` tries NVDEC through CUDA, then VA-API, then software; a hardware decoder that fails on its first frame switches the session to software. **Zero copy:** an NVDEC picture stays on the GPU and the window copies its planes CUDA → GL; `--no-zero-copy` (and VA-API or software) uploads from system memory.
- **Wire v2:** a frame goes to the decoder when `ReceivedFrame::decodable` (the core's chain: a loss breaks it until a keyframe, a recovery point or a frame referencing a good one). A lost frame no longer waits for a keyframe. A decoder reset or error (and so a fresh decoder) waits for a keyframe and sends `Msg::KeyframeRequest`, at most every 500 ms.
- **Present** (`window/`): one GLES 3 pass converts Y′CbCr to RGB (BT.709 or 601, full or limited range; NV12 or three planes). EGL's swap interval is 0; pacing comes from frame callbacks, and the newest picture wins. Every commit carries a `wp_presentation` feedback; its time is the frame's present stamp.
- **Size and letterbox** (`window/mod.rs`, `window/layout.rs`): `ClientDisplay` is the window's backing pixels (the output's mode in full screen), the output's refresh and current mode (`display_width/height`), `fullscreen` from the configure, and `pixels_per_point` from the output's scale (or `--ppp`). Every configure (or output change) that changes it sends `client.send(Msg::Resize(..))`; the host debounces, and the core keeps it for reconnects. `--fixed` and `--size` never send it. A stream at most 64 px smaller than the window on each axis is drawn 1:1, centred in whole pixels with the odd pixel to the right and bottom, the band in the window's black; a pointer in the band clamps to the nearest stream edge. Anything else (fixed resolution, a size in flight) is fitted with its aspect kept. Placement uses the picture actually shown, so a new size takes effect with its first frame.
- **Cursor:** the shape is `msg::CursorImage` (decoded by the core: raw premultiplied BGRA in stream pixels, with REMOTE-1's `scale`). It becomes the Wayland cursor, scaled like the video with `wp_viewporter`, so it moves with no stream latency. When the pointer is outside the window, the image is drawn at the host's position. Hidden on `visible = false` and under pointer lock.
- **Input:** pointer motion as absolute stream pixels, relative motion under `Msg::PointerLock(Locked)`, buttons, scroll (source, value, value120, stop, summed per `wl_pointer.frame`; when both axes come in one frame the first carries `more: true`), keys as HID usages exactly as pressed (`hid.rs`), key-ups for held keys on focus loss and `ReleaseAll` at exit. Events with a Wayland time (ms, CLOCK_MONOTONIC on Hyprland; µs for relative motion) go through `send_input_at`; a time that isn't plausibly that clock (in the future or over 10 s old) falls back to `send_input`.
- **Vsync reports**: for every frame presented, `LeadStats::add(ready, presented, refresh)` on the live session's clock, from the frame's ready time (decoded, on the presenter's side) and its `wp_presentation` time and `refresh` (the output's mode when the feedback has none). The lead is "before the presenter's deadline for that vsync"; Hyprland renders at the vsync, so here the deadline is the vsync and the gap is 0. About once a second `Msg::Vsync(report)`; one log line every tenth. A reconnect starts the stats over. Headless sends none.
- **The connection timeline** (`status.rs`): every 100 ms from `client.link_status()` (t₀ = `health.silence_ms` ago) while connected, and from `ClientEvent::Reconnecting { since }` after that. Chip: the route kind (LAN · direct · VPN · Tailscale · relay), the hedge route while hedging, the last route while there is none; amber while hedging, unhealthy, on a TCP or TLS relay, or from 1 s; red "Lost" from 30 s. Pill: none under 1 s, "Reconnecting…" 1–30 s, "Connection lost · retrying" from 30 s, and "Waiting for the network…" from 1 s whenever Linux has no default route (`/proc/net/route`, `/proc/net/ipv6_route`). Each change is logged as one line, `remote-client: link chip "LAN" amber · pill "Reconnecting…" (t0 1.02 s ago, run 9.0 s)`, and shown in the HUD. `Disconnected { reason }` is logged and ends the run.
- **Session messages:** `Welcome` and `Format` give the codec and size (logged with the scale list, `took_over_from` and `resumed`), `PointerLock`, host `Stats` into the HUD, client `Stats` every second, receiver reports per presented frame, `Disconnect(User)` through `Client::shutdown` at exit.
- **Stats** (`stats.rs`): `session-<unix>-frames.csv` (one row per frame: host stages when sent, send, first and last packet, decode in and out, ready, commit, present, skipped) and `session-<unix>-summary.json` (per-stage p50/p95/max, frames decoded/presented/skipped/lost, route, RTT, present intervals, idle bandwidth). Frames of an earlier connection never shown are closed as skipped on a reconnect (frame ids start over).
- **HUD** (a plain monospace panel, not the Mac app's design): route, RTT, loss, FEC, MTU; codec, decoder, fps, Mbit/s; per-stage medians; lost/undecodable/skipped; the host's stats; the link timeline; the click test's progress.
- **Click test** (`latency.rs`): after the first picture, the client clicks on the test surface's marker at random gaps, finds the first decoded frame whose marker luma flipped, and closes the click with that frame's present time (headless, its decode-done). Then it measures idle bandwidth between two of the core's byte counts (about a second apart, at least 2.5 s). It writes `session-*-clicks.csv` and `session-*-latency.md`.
- **Host stamps:** the frame meta's TLV record tag 1 (STAMPS: rendered, captured, converted, encoded µs on the host's session clock, the clock of `send_us`), after host-media's v1 meta (`proto.rs`).

### Clipboard, sound and the input method

The Windows client had all three; the Linux layer had none, so the SodiumOS app (`apps/remote`, which runs this crate's window embedded) had none either. Both now share them:
- **Clipboard** (`clip.rs`): `remote_engine::clipboard::ClipboardBackend` on `remote-wlclip`, the ext-data-control-v1 clipboard remoted already used (moved into its own crate, unchanged). It runs on its own thread and Wayland connection, so it syncs with or without focus, as on Windows. Text, HTML and PNG; a password manager's copy (`x-kde-passwordManagerHint`) goes to the host as concealed, and a concealed entry from the host is offered with the hint. Our own offers are marked, so a write from the host is never offered back.
- **Sound** (`audio.rs`, `csrc/audio_out.c`): the core's jitter buffer → libopus → a PipeWire playback stream (48 kHz stereo float, `node.latency` 480/48000) that pulls 10 ms at a time, as WASAPI does on Windows. The volume mixer shows it as "Remote" with the app's icon.
- **Input method** (`window/mod.rs`): text-input-v3 on the surface that has the keyboard (Qt's toplevel when embedded), enabled while keys go to the host. Re-enabling after a focus change sends a disable first: Hyprland 0.57 keeps a text input's enabled state across leave, so an enable alone wouldn't activate the input method again. The local input method (fcitx5: Mozc, Rime, …) grabs the keys at the compositor and composes in its own candidate window, placed at the last click; its committed text goes as `ImeCommit` (the host types it as Unicode), and the keys it doesn't take come back as keys. No preedit is sent. Off while the Qt app has the keyboard back (its menus and fields).
- **Testing:** `client-linux/tests/nested-io.sh [app]` (the reference client, or the app) in a nested Hyprland with a private fcitx5, against host-desktop's fake host, with sound into a private PipeWire. It composes again after a focus change, and in the app types into the main window mid-session (no key may reach the host: keys go only while the stream window has the keyboard). `REMOTE_LOG_IME=1` logs the text-input events (kinds and lengths only).

### File formats (for `analyze.py`)

Unchanged: the CSV columns, the `# epoch_mono_ns=` first line of the frames and clicks CSVs, `session-*-clicks.csv` and `session-*-latency.md`. Changed:
- The epoch is the start of the run (before the window and `connect`), not the session's; times across a reconnect stay on one clock.
- `summary.json` gains `connect_start_us`; latency.md's Connect row counts from `connect` being called.
- `route` names are `LAN`, `direct`, `VPN`, `Tailscale`, `relay udp|tcp|tls` (was `direct` / `relay udp`).
- `t_click_us` is when the button-down was handed to the core (as before, within microseconds).

## Seams now closed

| Seam | Before | Now |
|---|---|---|
| Connecting | The M0 signalling directory (`--sig`) | `remote_core::connect::Client`: rendezvous, mDNS and the LAN listener, SPAKE2 pairing, takeover, reconnect within the hold |
| Messages | A copy of core's `msg.rs` | `remote_core::msg` (v3) |
| Frame chain | Wire v1: a loss waits for a keyframe | `ReceivedFrame::decodable` |
| Keyframe request | Control kind 3 | `Msg::KeyframeRequest` |
| Cursor image | host-media's raw/PNG payload and `msg::CursorImage` | `msg::CursorImage` only |
| Size | Fixed by `--size` | `ClientDisplay` from the window and output; `Resize` on every change; 1:1 letterbox |
| Key table | none | `Msg::KeyTable(vec![])` at connect (PC passthrough) |
| Phase lock | per-frame receiver reports only | `Msg::Vsync` from `LeadStats`, about once a second |
| Input times | stamped when sent | the Wayland event time (`send_input_at`) |
| Reconnect | none | the timeline in the log and HUD |

## Results

Against core's test host (`connect_demo host --no-rdv`, LAN path only, both bound to the LAN address, one machine). Its video is dummy bytes, so decoding fails by design. Evidence: the session scratchpad's `remote/m1-integration/client/` (`demo-lan/`, `nested-window/`).

- **Pairing over mDNS + the LAN listener:** a wrong code exits 2 (the host logs `PairingFailed { attempts_left: 2 }`); the right code from `$REMOTE_PAIR_CODE` exits 0 and prints the host's ID, code and name; both sides store each other.
- **Connect:** route LAN, `connected` about 1 s after start, `Welcome` (2560×1664, scale 1.6 of [1.333, 1.6, 2.0]); the host received `Hello` with the display, then `KeyTable([])`, then `KeyframeRequest` every 500 ms while the dummy video failed to decode.
- **The timeline, with the host killed (`kill -9`):** `chip "LAN" amber` at 0.2 s (hedging), `pill "Reconnecting…"` at t₀ + 1.02 s, `Reconnecting { since: 4.02 s }` from the core, `chip "Lost" red · pill "Connection lost · retrying"` at t₀ + 30.02 s, then `Disconnected(HoldExpired)` and exit 3.
- **Windowed, one nested Hyprland** (CLIENT-1 2560×1664@60, scale 1): `Hello` carried the tiled window, 2518×1622 (display 2560×1664, 60 Hz, ppp 1), and `Welcome` answered at that size. Floating the window and resizing it to 2237×1349 sent `Resize` 2560×1664 then 2237×1349; full screen sent 2560×1664 with `fullscreen: true`. A key (HID 4 down and up), a wheel click (`value120` 120, `more: false`) and pointer motion arrived at the host; the pointer at the window's corner, in the band of the fitted 2518×1622 stream, arrived as (0.0, 1.2), clamped to the edge. SIGINT ended it with exit 0: `ReleaseAll`, then the host's `ClientDisconnected { reason: User }`. The HUD shows the link line (`floating.png`).

The earlier latency results (input → present 20.3 / 47.8 ms p50/p95 windowed, HEVC 4:2:0 on NVDEC with CUDA → GL; the chain breakdown) were measured against the stand-in host and are in this file's history (`git log -p remote/docs/client-linux.md`); runs against `remoted` replace them.

## Missing

- Zero copy for VA-API (DMA-BUF → EGLImage); only NVDEC has it.
- Fractional scale: not built, on purpose. The window renders at the integer preferred buffer scale, and `pixels_per_point` is the output's integer scale unless `--ppp` says otherwise; Linux is a test stand-in for the Air, not a target client.
- Gestures, the remote preedit (`ImePreedit`), the Busy question as UI (it's `--takeover`), the Disconnected card (the client just exits). The last two are the Mac's UI (design 5 and 7); the tests have what they need in the exits.
- An adaptive 0–1 frame jitter buffer: presentation is always latest-wins. The host's phase-lock margin covers jitter up to half a period; the hold is for more, and gets built when a path's reported `lead_p05` stays negative for 3 reports in a row (frames still missing after the margin saturated), released after 10 s with p05 at 1 ms or more.
- `Axis.more` is untested end to end: nested Hyprland's virtual pointer can't send both axes in one frame.
- The letterbox is checked by the unit test only; a real host's rounded-down stream still needs a check.

## Open questions

All three are decided:

1. **`pixels_per_point` on Linux:** the reference client is a stand-in for the Air. `run.sh` passes `--ppp 1.741` unless `--ppp` is given (headless already sends 1.741; windowed without `--ppp` stays the output's integer scale). No `wp_fractional_scale_v1`.
2. **The vsync lead's "ready":** decode-done on the presenter's side, measured against the presenter's deadline for the vsync the frame was shown at. Linux keeps `ready` and the `wp_presentation` time (on the nested headless output present is the compositor's render, so ready → present has no gap); the Mac client reports the display link update's `targetTimestamp` minus ready. The adds one check, in `analyze.py`: the plain ready → present p50 within max(3 ms, the lead's p50 − p05) of the lead p50 plus the client's deadline-to-vsync ( widened the 3 ms to the lead's spread, both printed).
3. **The Disconnected card and the Busy question** stay the Mac's UI. The reference client's exits are enough: 3 with the reason on the last log line, 4 for Busy without `--takeover`, and `--takeover` for the takeover path (a takeover test runs a second client with it and asserts the first's exit 3 "taken over").

## Integration notes

- **Present:** a new picture is committed as soon as it is ready, not after the previous commit's frame callback. Waiting for the callback put every frame of continuous motion a whole refresh late (ready → presented 21.9 ms p50 with a window animating every frame; 4.4 ms after the change; input → present 39.6 → 21.6 ms).
- **Decoder watchdog:** three decodable frames in a row that give no picture ask for a keyframe (`Msg::KeyframeRequest`). FFmpeg's NVDEC path drops every frame whose reference set names a missing picture, so an intra refresh after a loss longer than NVENC's DPB never recovered. The host now sends an IDR for that loss, so the watchdog is the safety net for decoder errors; in `netchange.sh` it no longer fires.
- **Phase lock:** `LeadStats::report` now waits for 10 frames, so a still desktop's sparse frames add up before a report.
