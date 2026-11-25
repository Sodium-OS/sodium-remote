# Remote: the Windows client and the client engine

`remote/client-windows` (bin `remote-client.exe`) is Sodium Remote's test client for Windows, the way `client-linux` is for Linux: a CLI that pairs, connects, shows the stream in a Win32 window and sends input. Both are thin platform layers on **`remote/client-engine`** (crate `remote-engine`), the stream engine that the Qt app links later.

- **Status:** builds with mingw; connect, pairing, decode, the scripted mode and the frame dump are tested headless under wine against the nested host; the window, input, clipboard and audio are tested in the Windows VM .

## Build and run

```sh
rustup target add x86_64-pc-windows-gnu           # once
remote/packaging/windows/build-deps.sh            # once: FFmpeg, the OpenH264 headers, Opus (about 5 minutes)
remote/packaging/windows/stage-client.sh --build  # cross-builds and gathers remote/build/win-client/
```

`stage-client.sh` copies `remote-client.exe` with `avcodec-63.dll`, `avutil-61.dll`, `swscale-10.dll` and `libwinpthread-1.dll` (everything else is Windows' own: `d3d11`, `dxgi`, `d3dcompiler_47`, `winmm`, WASAPI) and the licenses. `packaging/windows/cargo-win.sh build --release -p client-windows` is the same as `. env.sh && cargo build --release --target x86_64-pc-windows-gnu -p client-windows`. Copy the folder into the VM, or run `remote-client.exe --headless` under wine.

```
remote-client.exe --data DIR pair --code NNNNNN [--host ID] [--no-rendezvous]
remote-client.exe --data DIR [connect] [--host ID] [--lan-hint ID@IP:PORT]... [--headless]
                  [--decoder auto|d3d11|sw] [--codec auto|hevc|h264] [--size WxH] [--fixed] [--fullscreen]
                  [--no-zero-copy] [--no-vsync]
                  [--stats-dir DIR] [--secs N] [--clicks N ...] [--input-log FILE] [--no-hud]
                  [--script FILE] [--dump-frames DIR [--dump-every N]] [--no-audio] [--audio-log FILE]
```

The flags are the Linux client's (`docs/client-linux.md`), minus the Wayland-only ones (`--decoder cuda|vaapi`), plus the ones below. `--no-zero-copy` copies each decoded picture to system memory and uploads it (the path before zero copy, for comparing). `--no-vsync` presents without waiting for the vsync (`DXGI_PRESENT_ALLOW_TEARING`, sync interval 0) where DXGI allows tearing; it tears only where Windows flips the window independently (full screen), and is off by default. `--data`'s folder holds the same `device.key` and `paired.json`; the device name defaults to "Windows client". Exit codes are the Linux client's (0 user, 3 ended otherwise, 4 never connected, pair: 0 / 2 / 3 / 4).

**Unattended tests**:

- `--script FILE` sends timed inputs through the keyboard-and-mouse path, starting one second after the first picture. One step per line, `<ms> <action>`: `move X Y` (stream pixels), `click [left|right|middle|back|forward] [X Y]`, `down`/`up` (button edges), `key ctrl+a` (a chord; `key enter up` for one edge; names in `client-engine/src/script.rs`, or `0xNN` for a HID usage), `text Hello` (committed text), `scroll DX DY` (notches, positive is down), `clipboard TEXT` (sets the local clipboard; the sync offers it to the host), `quit`. Every step is logged to stderr.
- `--dump-frames DIR --dump-every N` writes every Nth decoded picture as `frame-<id>.png` (CPU conversion with the presenters' matrix and range rules; works headless).
- `--audio-log FILE` writes `seconds,dBFS,frames decoded,non-silent frames` once a second; the same level is on stderr every 2 s.
- `--secs`, `--stats-dir` and the click test are the Linux client's.

## The engine (`client-engine`)

`remote-engine` holds everything that is not a window, an input device, a cursor, a clipboard or a speaker, and builds for Linux and for `x86_64-pc-windows-gnu`:

| Module | What |
|---|---|
| `client` | `run(Options, Arc<dyn Platform>)` and `pair(...)`: the session loop (Hello, `KeyTable`, events), the decode thread, the timeline, the click-test scheduler, stats, exit codes |
| `connect` | `NetOptions`, `start` (the core's `Client` with a `FileStore`), `pick_host`, `SessionClock` (`mono_ns` is CLOCK_MONOTONIC on Unix, a process clock elsewhere) |
| `decode` + `csrc/decode_shim.c` | the FFmpeg 9 shim: HEVC, H.264, AV1; NVDEC and VA-API (Linux) or `d3d11va` (Windows) with a software fallback; `probe_support` for `DecodeSupport` |
| `clipboard` | `ClipboardBackend` (the platform's clipboard) and `ClipSync` (offer, request, chunks, assembly, no echo of its own writes) |
| `script`, `dump` | the scripted mode and the PNG dump |
| `layout` | where the stream goes in the window and where the pointer goes in the stream (1:1 within 64 px, else aspect fit) |
| `latency`, `stats`, `status`, `proto` | the click-to-photon harness, stats files and HUD text, the connection timeline, frame metadata |

**The platform trait** (`remote_engine::Platform`): `client_os()`, `adjust_decode_support()`, `key_table(host_os)`, `has_default_route()`, `waker()`, `clipboard()`, `run_window(app, rx, opts)` and `start_services(app)`. The window runs on the main thread, shows `app.latest` (the decoder's newest picture; latest frame wins), publishes `app.display` (what `Hello` and `Resize` describe), feeds input into `app.send_input` and plays the `WinMsg`s (stream, cursor shape and position, pointer lock, secure screen). An app that wants more than the CLI's `run` drives `connect::start` and `App` itself. `App::host` has `Welcome` (OS, display mode, `displays`, `current_display`) and `App::select_display(id)` sends `SelectDisplay`.

**Decode support is probed, never assumed**: at start the engine opens each decoder (hardware first, then the software fallback) and says what worked in `Hello.decode`. `--codec auto|hevc|h264` (`client::CodecChoice`) clears the other bits (AV1 only in Automatic). On Windows `av1` is true only when the GPU lists the AV1 VLD profile (`d3d::hardware_av1`; there is no software AV1).

## The Windows layer (`client-windows`)

- **Window** (`win.rs`): a Win32 window, per-monitor DPI aware (real pixels; `pixels_per_point` = DPI / 96), `ClientDisplay` from the client area, the monitor's size and refresh. `Resize` on every change unless `--fixed`, `--size` or `live_resize` is false (then it fits by aspect and never sends it). `--fullscreen` is a borderless window over the monitor.
- **Present** (`d3d.rs`): a D3D11 device (hardware, else WARP: the VM), a **flip-model swapchain** (`FLIP_DISCARD`, two buffers, `FRAME_LATENCY_WAITABLE_OBJECT`, latency 1). The loop waits on the waitable object and the wake event together with the message queue, then takes the newest picture (older ones are never shown and count as skipped), draws it straight from its decoder texture (zero copy, below) or uploads it (NV12: R8 + R8G8; 3 planes: R8 each; `UpdateSubresource`), draws and presents with sync interval 1 (`--no-vsync`: 0 with `ALLOW_TEARING`). The pixel shader converts Y′CbCr → RGB (BT.709 or 601, full or limited) and is compiled at run time with `D3DCompile` from `d3dcompiler_47.dll` (loaded dynamically). The HUD and the secure-screen notice are GDI text panels uploaded as premultiplied BGRA overlays. Present times come from `IDXGISwapChain::GetFrameStatistics` (QPC → client µs); when DXGI gives none for 100 ms (WARP, occluded) a frame is stamped at its present call plus a refresh. They feed `Stats::on_present`, the receiver report, the click test and `Msg::Vsync` (`LeadStats`, once a second).
- **Decode:** `d3d11va` through the shim, else software. **Which GPU** (`d3d::decode_adapter`): the first hardware adapter, in DXGI's order (the one driving the primary display first), whose D3D11 video device lists a decoder profile; stderr says `decoding on GPU N (name)`. So a PC whose display GPU decodes keeps it, and one whose display adapter has no video decoder (a virtual display, a display-only GPU) decodes on the next one that has. The shared device, the decoder's own device (`rc_d3d11_set_adapter`, FFmpeg's adapter index) and the AV1 check all use that adapter; with no such adapter the default one is used as before. Presenting from a GPU other than the display's makes Windows copy each frame across adapters. **Zero copy** (default in the CLI and the Qt app): before the session starts, `d3d::share_with_decoder` makes the hardware device with video support and gives it to the shim (`rc_d3d11_set_device`, multithread-protected, one lock shared with FFmpeg). The D3D11VA decoder decodes on that device; in `get_format` the shim asks FFmpeg for its frame parameters and adds `D3D11_BIND_SHADER_RESOURCE` (NV12 only), and with the keep flag (`Options::zero_copy`) a picture comes back as its texture and array slice (`Picture::d3d11`), never copied to system memory. `Gfx::show` makes an R8 (luma) and an R8G8 (CbCr) `Texture2DArray` view of that slice (cached per slice), samples it in `ps_nv12_array` with the coordinates scaled to the picture inside the padded texture, and keeps the picture (and so its decoder slot) until a newer one is drawn. The decoder and the presenter share the device's immediate context, so the draw is ordered after the decode with no copy and no fence. **Fallbacks:** no hardware device with video support, a driver that refuses shader-readable decoder textures, a format other than NV12 (4:4:4), software decode, headless or `--dump-frames`, or `--no-zero-copy`: pictures are copied and uploaded as before. If the views can't be made, zero copy is turned off for the session (`d3d11_zero_copy(false)`) and the next pictures come in system memory. The decoder's label says which: `hevc+d3d11va (shared device, zero-copy)`. The click test reads its probe through a small staging copy of just that rectangle. The Windows client never uses CUDA/GL interop.
- **Present path timing** (`win.rs`): per shown frame, the time to hand the picture to the GPU (the upload, or making the views), ready → drawn (the present call returned), drawn → on screen (DXGI frame statistics) and last packet → on screen. p50/p95 go to stderr every 5 s as `present path zero-copy: …` or `present path copy: …`, and on the HUD. The per-frame stats CSV has the same stamps (`decoded_us`, `ready_us`, `transfer_ms`, `commit_us`, `present_us`). Run once with and once with `--no-zero-copy` on the same machine to compare.
- **Cursor:** `msg::CursorImage` becomes an HCURSOR (`CreateIconIndirect`, 32-bit premultiplied alpha, bilinear-scaled like the video, at most 256 px); `WM_SETCURSOR` applies it; hidden under `PointerLock::Locked`.
- **Input:** `WM_MOUSEMOVE` → `PointerAbs` in stream pixels (`layout::to_stream`); under `Locked` the cursor is clipped to one pixel at the window's centre and **Raw Input** (`WM_INPUT`) gives `PointerRel` (scaled to stream pixels, unaccelerated); `Confined` clips to the window. Buttons incl. X1/X2 (Back, Forward) with mouse capture; the wheel (`WHEEL_DELTA` 120 → `value120`, hi-res deltas pass as they are, sign flipped to the wire's: positive is down) and the horizontal wheel. Keys are HID usages from scan codes (`scancode.rs`: set 1 plus the extended flag), repeats dropped. While the window is the foreground window a **`WH_KEYBOARD_LL` hook** takes every key, so Win, Alt+Tab and Alt+Esc reach the host (without the hook, `WM_KEYDOWN` is used). Held keys and buttons are released on focus loss, `ReleaseAll` at exit. Ctrl+Alt+Del and the secure desktop are the OS's.
- **Key table:** `Hello.client_os = Windows`; when `Welcome` names the host's OS the client sends `KeyTable(default_rules(Windows, host_os))` (empty, so not sent again, for a Windows or SodiumOS host; Ctrl/Alt/Win remaps for a Mac host).
- **Clipboard** (`clip.rs`): `CF_UNICODETEXT` (UTF-8 and LF on the wire, CRLF locally) and PNG both ways ("PNG" when an app offers it, else `CF_DIB` converted; on receipt both "PNG" and `CF_DIB` are set). Concealed items set the history and cloud-sync opt-out formats. A message-only window owns the clipboard; `GetClipboardSequenceNumber` is polled every 100 ms.
- **Audio** (`audio.rs`): the core's jitter buffer (`Client::audio_buffer`, 10 ms pops) → Opus (static libopus, stereo 48 kHz; FEC and concealment as the buffer says) → a **WASAPI shared-mode** render stream, event driven, 30 ms buffer, float32. The level is logged. No device means no audio and a log line; the video is unaffected.
- **`SecureScreen(true)`** draws a plain notice over the video with this wording ("<host> is showing a secure screen", the body naming lock, sign-in and UAC on a Windows host, the lock screen on a Mac) and the engine logs it. PROVISIONAL, with the HUD and the window chrome, and English only (the designed, translated notice comes with the Qt app).

## Limits and follow-ups

- Zero copy and `--no-vsync` are built and checked on CI but not yet measured on real hardware: compare `present path` lines with and without `--no-zero-copy`, and with `--no-vsync` in full screen. With `--no-vsync` the vsync reports to the host still go out; whether the host's phase lock should change for it is open.
- No `has_default_route` on Windows (the timeline's "Waiting for the network…" never shows); no network-change notification (`Client::network_changed`).
- No software AV1 (no dav1d in the FFmpeg build, by design); no IME preedit (committed text only, `ImeCommit` from `text` steps and, later, the Qt app).
- The host-side remote cursor is not drawn when the pointer is outside the window (Linux draws it).
- The HUD, notice and window chrome are provisional; the strings are English only.
- The host's displays list is stored (`App::host`) and `select_display` exists; the picker is the Qt app's (no picker with one display, never a name or picker for a Virtual host).
- Wine can run `--headless`, not the D3D11 window against a real desktop, and has no WASAPI device: the window, hook, cursor, clipboard and audio are only exercised in the VM.
