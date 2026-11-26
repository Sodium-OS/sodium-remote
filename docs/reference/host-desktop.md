# host-desktop: the portable host

`remote/host-desktop` is the host for computers that aren't SodiumOS: Windows, the Mac and Linux on GNOME or KDE Plasma (the Mac in [host-mac.md](host-mac.md), Linux in [host-linux.md](host-linux.md)). It is a library plus a program, `sodium-remote-host`. SodiumOS keeps `sodiumos-remoted`, which has its own virtual display, compositor plugin and input method; `host-desktop` streams the computer's real displays instead (`display_mode: Physical`).

## Layout

| File | What |
|---|---|
| `src/backend.rs` | The backend trait and its types. No Windows or Mac types in it. |
| `src/session.rs` | The session glue on `remote_core::connect::Host`: pairing, Welcome, display choice, input, clipboard, audio, statistics. |
| `src/media.rs` | The media loop: capture, the refresh rule, rate, encode, send; cursor shape and position. |
| `src/policy.rs` | The rules with unit tests: codec choice, fit to the client's maximum, the 150 ms refresh, the encoder re-open rule, pointer mapping. |
| `src/encode.rs` + `csrc/hd_shim.c` | The encoders through a C shim: FFmpeg's (hardware HEVC and H.264, Media Foundation H.264, VA-API, libx264) and OpenH264 used directly, its library loaded at run time (`src/softvideo.rs`: Cisco's DLL on Windows, downloaded on first need; the system's on Linux); BGRA to NV12/I420 with swscale; Opus. |
| `src/hid.rs` | HID usage to Windows scan code (set 1) or virtual key, to macOS key code, and to Linux evdev code. |
| `src/fake.rs` | A fake backend for tests on any OS. |
| `src/win/` | The Windows backend (`cfg(windows)`). |
| `src/mac/` | The macOS backend (`cfg(target_os = "macos")`): [host-mac.md](host-mac.md). |
| `src/linux/` | The generic Linux backend over xdg-desktop-portal (feature `linux-portal`, on by default): [host-linux.md](host-linux.md). PipeWire capture and audio are in `remote/linux-media`. |
| `src/ffi.rs`, `uniffi.toml` | The UniFFI surface for Remote.app's "This computer" pane (feature `ffi`). |
| `csrc/hd_opus.c`, `build-opus-mac.sh`, `build-swift.sh` | Opus on its own (the Mac has no other C); the Mac's static Opus; the darwin check, Swift bindings and the Mac's static library. |
| `examples/probe.rs` | A scripted test client (pair, connect, send steps, count what comes back). |
| `tests/e2e-linux.sh` | The Linux end to end with the fake backend. |
| `tests/linux/e2e.sh` | The portal backend in the GNOME/KDE VM ([host-linux.md](host-linux.md), Testing). |
| `tests/mac/host.sh` | Build, sign and run the host on the Mac over SSH (a LaunchAgent in the GUI session). |

`Picture::bgra()` may return `None` for a GPU surface; such a backend brings its own `Encoder` that takes its own picture type through `Picture::as_any()` (the Mac's IOSurface pictures go to VideoToolbox that way). `Session` is the running host as a handle: the program and the Mac app's pane (`ffi`) drive the same one, and it reports status, pairing and paired-list changes as `Note`s.

## The session

- **Store and identity.** `--data DIR` holds the host key and the paired clients (`remote_core::keys::FileStore`). On Windows the default is `%LOCALAPPDATA%\SodiumRemote\host`, on Linux with the portal backend `~/.local/share/sodium-remote/host`; on the Mac `--data` is required. `host.txt` in the data dir has the host ID, LAN port and name.
- **Pairing.** With `--pair`, or when nothing is paired yet, the host opens pairing, prints `pairing code NNNNNN` when stdout is a terminal (never to a log or the journal) and writes `pairing-code.txt` (0600 in a 0700 data folder on Linux and macOS, tightened at every start; removed once a client pairs). With `--pair` it opens a new code when one expires; after three wrong codes pairing stays closed (it says so) until it is opened again.
- **Pairing by address.** A client that knows only this computer's address pairs with `IP[:PORT]` (default 47311) and the code: `remote-client --data DIR pair --address IP --code N`. The host answers the address-only exchange (M1 `v: 2`) as well as the ID one; the client learns this host's ID from the exchange, bound by the code (`protocol.md`, `session-api.md` section 4). Nothing changes in the host's command line.
- **Discovery.** The LAN listener is on (`0.0.0.0:47311`, or any free port if that's taken; `--port`, `--bind`). mDNS is on unless `--no-mdns`. The rendezvous server is off unless `--rendezvous URL`.
- **Welcome**: `host_os` Windows (or the backend's), `display_mode` Physical, `live_resize` true (`Backend::live_resize`; every backend's encoder scales to any even size), no UI scales, no home monitor, the displays with their Windows names where Windows has them (empty otherwise, so clients show "Display %1"), the current one, and `view_only` (true only when the backend can't inject input: a Mac without Accessibility; the Windows and fake backends send false).
- **Not ready** (`Backend::check_ready`): returns `NotReady::Permission { screen_recording, accessibility }`, sent to the client as `DisconnectReason::PermissionMissing`, or `NotReady::Other(text)` for the client's log. `Backend::view_only()` feeds `Welcome.view_only` and `ControllingDevice.viewOnly`.
- **Display.** The client's chosen display, else the primary. `SelectDisplay` restarts the media on another display and sends a new `Format`. The frame rate is the lower of the client's and the display's.
- **Stream size.** `policy::stream_size`: the largest even size with the display's aspect that fits the client's window (`Hello.display` width and height, then each `Resize`; physical pixels) and the decoder's `max_width`/`max_height`, never above the display's native size (the client scales up). `Resize` is debounced: the host applies the latest one after 150 ms without another. If the size is unchanged, nothing happens. If only the size changed, the media loop opens a new encoder on the running capture, sends `Format` and a keyframe, and resends the pointer shape at the new scale; a changed display, rate or codec restarts the whole stream as `SelectDisplay` does. The encoders scale the captured picture: swscale on the CPU path for every Windows encoder (NVENC, AMF, QSV, Media Foundation, OpenH264) and the Linux software and CPU-fed encoders (area averaging when shrinking, bilinear when enlarging), `scale_vaapi` for Linux DMA-BUF frames, `VTPixelTransferSession` on the Mac. Pointer positions arrive in stream pixels and are scaled to display pixels on the host (`policy::stream_to_display`), so they stay right at any stream size; the pointer shape and damage rectangles are scaled to stream pixels.
- **Codec**: HEVC when the host has a working hardware HEVC encoder and the client decodes HEVC, else H.264 (`DecodeSupport::use_h264`). A forced codec in the client's settings restarts the media.
- **Input.** Keys go through `keymap::KeyMapper` with the foreground program (its image name, and whether it is on the terminal list) when the client sent a non-empty key table; otherwise they pass through. IME commits arrive as Unicode text. Pointer positions are in stream pixels and map to the display. `ReleaseAll` on connect, on disconnect and when the client asks.
- **Clipboard.** Text, HTML and PNG both ways, as remoted does it (offer, request, chunks). An entry a password manager marks with `ExcludeClipboardContentFromMonitorProcessing` goes out with the offer's `concealed` flag; a concealed entry coming in is written with that format and `CanIncludeInClipboardHistory` 0.
- **Audio.** The default output device's loopback, 48 kHz stereo, Opus 10 ms frames at 128 kbit/s.
- **Lost and ended sessions.** `ClientLost` pauses the media; the core's hold keeps the session for a resume. `HoldExpired` or a disconnect ends it.

## Media policy

- **Frames only on damage.** The capture reports damaged rectangles; no damage means no frame. The rectangles travel in the frame's metadata (as remoted's), with the capture, convert and encode times.
- **Rate.** Each frame is encoded at the core's congestion-controlled target (`link_status().budget.encoder_bps`), starting at 8 Mbit/s.
  - OpenH264 takes a new rate on any frame at no cost.
  - FFmpeg's wrappers fix the rate at open, and NVENC's even forces an IDR on every change. So those re-open (a keyframe) only on a lasting change: below 0.6 of the current rate at once; outside 0.75–1.33 for 2 s; never sooner than 500 ms after the last re-open (`policy::RateFollower`).
- **One refresh.** 150 ms after the last change, one frame of the unchanged screen at up to three times the target (what the link has left this interval) sharpens what the rate-limited frames blurred.
- **Keyframes on request only**: at the start, when the client asks, when the core reports a lost reference (the FFmpeg encoders can't predict from an older frame), after a re-open, and when the secure screen ends. No QP maps, no split encodes.
  - Lost-reference hints are coalesced: at most one keyframe per 250 ms for them, and a hint about a frame older than the last keyframe sent is dropped, so a loss burst gives one keyframe, not a storm.

## The Windows backend

- **Process.** A per-user program in the interactive session, per-monitor DPI aware (v2) so coordinates are physical pixels, with a 1 ms timer resolution so the pacer's 1 ms sleeps are 1 ms.
- **Displays.** DXGI outputs attached to the desktop; the ID is the `n` of `\\.\DISPLAYn`; names from `DisplayConfigGetDeviceInfo` (the monitor's friendly name); scale from `GetDpiForMonitor`; refresh from `EnumDisplaySettingsW`.
- **Capture.** DXGI Desktop Duplication (`DuplicateOutput1` for BGRA, else `DuplicateOutput`) on the output's own adapter, with the move and dirty rectangles, and the pointer's shape, hot spot and position. Only the changed rows are copied: straight from the desktop image when DXGI keeps it in system memory (`MapDesktopSurface`, as on the Basic Display Adapter), else through a staging texture. `DXGI_ERROR_ACCESS_LOST` or `DXGI_ERROR_INVALID_CALL` (mode change, full-screen switch, secure desktop) drops the duplication and recreates it every 250 ms. `SODIUM_REMOTE_HOST_DEBUG=1` logs the capture's frames per second, presents per frame and wait and copy times every 2 s. Monochrome and masked-colour cursors are converted to premultiplied BGRA; inverting pixels become black with a white outline.
- **Secure screen.** When the input desktop isn't `Default` (lock screen, sign-in, UAC on the secure desktop), duplication fails (`ACCESS_LOST` for the lock, `INVALID_CALL` for a UAC prompt) and can't be recreated: the host sends `SecureScreen(true)` and keeps the session; when it is back, `SecureScreen(false)` and a keyframe. A capture opened while the secure desktop is up reports it at once.
- **Encoders**, tried in order once at start (each opens and encodes one frame): `hevc_nvenc`, `hevc_amf`, `hevc_qsv` for HEVC; `h264_nvenc`, `h264_amf`, `h264_qsv`, `h264_mf` (Media Foundation, the GPU's own MFT only: `hw_encoding`) then OpenH264 for H.264. OpenH264 is Cisco's prebuilt DLL, fetched on first need into `%LOCALAPPDATA%\SodiumRemote\codecs\` and pinned by SHA-256 (`src/openh264_pin.rs`, `packaging/windows/pin-openh264.sh`); the setting "software video" (`settings.json`, default on) and the `codec` events are in `client-qt/src/host.rs`. Microsoft's software MFT is skipped: in the VM it answered each forced keyframe with seven IDRs of 450 KB and added an IDR every 60 frames, and its rate is fixed at open; OpenH264 changes rate per frame and sends one IDR.
  - Low latency throughout: no B-frames, no lookahead, an endless GOP, CBR with a one-frame VBV.
  - NVENC: p2, ull, zerolatency. AMF: ultralowlatency. QSV: veryfast, async depth 1. MF: display remoting scenario.
  - OpenH264: screen content real time, bitrate mode with frame skip.
- **Input.** `SendInput`: HID usages become set-1 scan codes (extended keys flagged; Pause, Num Lock and the Korean keys as virtual keys); the pointer is absolute over the virtual desktop or relative; buttons include back and forward; the wheel takes `value120` (hi-res) and horizontal scrolling; text is `KEYEVENTF_UNICODE`; a pinch is Ctrl + wheel. Windows keeps a program from driving an elevated window (UIPI): run the host elevated to control those.
- **Audio.** WASAPI loopback on the default render device, converted to 48 kHz stereo float; a default-device change re-opens it.
- **Clipboard.** A message-only window with a clipboard format listener: `CF_UNICODETEXT`, `HTML Format` (the CF_HTML fragment), `PNG`, and `CF_DIB`/`CF_DIBV5` converted to PNG when there is no PNG. Our own writes are skipped by sequence number.
- **Foreground program.** The foreground window's process image name (`notepad.exe`), cached 200 ms; terminals are `WindowsTerminal.exe`, `OpenConsole.exe`, `conhost.exe`, `cmd.exe`, `powershell.exe`, `pwsh.exe`, `wezterm-gui.exe`, `alacritty.exe`, `mintty.exe`, `kitty.exe`, `putty.exe`, ConEmu, Tabby, Hyper, Termius and MobaXterm.
- **Firewall.** The test VM has a rule for the program (`netsh advfirewall firewall add rule name="Sodium Remote host (test)" dir=in action=allow program=...`); the installer will add the real one.

## Building

- Linux: `cargo build -p host-desktop` in `remote/` (needs FFmpeg with libavfilter, OpenH264, Opus, libpipewire-0.3 and libxkbcommon through pkg-config). `--no-default-features` leaves out the portal backend and builds the fake one only.
- Windows: `remote/packaging/windows/build-deps.sh` once, then `remote/packaging/windows/cargo.sh build --release --target x86_64-pc-windows-gnu -p host-desktop --bins --examples`. Ship `sodium-remote-host.exe` with `avcodec-63.dll`, `avutil-61.dll`, `swscale-10.dll`, `libopenh264.dll` and `libwinpthread-1.dll` (staged in `remote/build/win-host/`).

## Running

```
sodium-remote-host [--data DIR] [--name NAME] [--port N] [--pair] [--no-mdns] [--rendezvous URL]
                   [--bind ADDR] [--secs N] [--backend windows|mac|portal|fake] [--encoder NAME] [--list]
                   [--permissions] [--ask-permissions] [--forget-consent]
```

`--list` prints the displays and which encoders work. `--permissions` and `--ask-permissions` (macOS) print or ask for Screen Recording and Accessibility. `--forget-consent` (Linux) deletes the portal's restore token, so the next connection asks at the computer again. `--backend fake` (any OS) draws a test screen; `--fake-size WxH` and `--input-log FILE` go with it.

## Testing

- **Unit tests** (`cargo test -p host-desktop`): HID scan code and Mac key code spot checks, codec choice, the 150 ms refresh, the re-open rule, the keyframe gate, pointer mapping (Windows and Mac), fit.
- **The Mac off the Mac:** `host-desktop/build-swift.sh check-mac` type-checks the macOS backend, with and without `ffi`, for `aarch64-apple-darwin` on Linux.
- **Linux end to end:** `host-desktop/tests/e2e-linux.sh [hevc_nvenc|openh264] [SECS]`. A fake host on a random port with mDNS off, the probe pairs over the LAN hint, client-linux runs headless for SECS seconds, then the probe types, commits IME text, clicks, scrolls, exchanges clipboard text and switches displays. Output in `remote/build/e2e/<encoder>/`.
- **Linux, GNOME and KDE Plasma:** `host-desktop/tests/linux/e2e.sh gnome|kde [--fresh]` in the Arch VM; see [host-linux.md](host-linux.md).
- **Windows (pair B):** the Windows 11 VM (`remote/tools/vm/`; take its lock first). The host must run in the interactive session, so it starts from a scheduled task with an interactive token, never from SSH (session 0 has no displays). The helpers are in `host-desktop/tests/vm/`:
  - copy the staged package, the helpers and `make-media.py`'s `tone.wav` and `clip.png` to `C:\remote-test\host-desktop\`, add the firewall rule, and run `setup.ps1` once over SSH: it registers `SRHD-*` tasks that run unelevated in session 1 (`host`, `list`, `scroll`, `anim`, `notepad`, `tone`, `cliptext`, `clippng`, `clipget`, `uac`, `lock`);
  - start one with `win.sh ssh 'schtasks /run /tn SRHD-host'`; the host's output is in `host.out` and `host.log` there, its ID in `data\host.txt`, the code in `data\pairing-code.txt`;
  - from this PC: `probe --data DIR pair --code N --lan-hint ID@192.0.2.10:47311`, then client-linux (`remote-client --data DIR connect --headless --no-rendezvous --no-mdns --lan-hint ...`) and probe scripts;
  - `killmine.ps1 [-Host_]` stops the helpers (and the host), nothing else.
  - The lock screen can't be left without the account's password: the check ends with a reboot (automatic logon).

## Pair B

client-linux and the probe on this PC, the host in the Windows 11 VM (Basic Display Adapter, WARP, no GPU, 6 vCPUs), over the host-only bridge with a LAN hint. All passed. client-linux (headless) gave the picture rows; typing, IME commit, clicks, scroll, clipboard and audio were the probe's (`examples/probe.rs`), since client-linux had no clipboard, sound or IME at that point (`client-linux.md`):

| Check | Result |
|---|---|
| Pair, connect | Paired by code over the LAN hint; Welcome: Windows, Physical, display 1 "" 1920x1080 (the adapter has no monitor name), H.264 |
| Capture | DXGI duplication works on the Basic Display Adapter (no WGC needed); desktop image in system memory |
| Encoder | No hardware HEVC or MFT: OpenH264, 1920x1080 at 60 fps requested, per-frame rate, 7–8 ms encode, 2 ms convert |
| 1080p, full screen scrolling at 60 Hz | client-linux 20 s: 561 decoded, 0 lost, 28 fps, 1 keyframe; host render→decoded p50 11.1 ms, p95 22.5 ms. The cap is the VM: reading a 1080p desktop image costs 20 ms (DWM presents 60/s, two per captured frame) |
| Console scrolling | 29 fps (what the console draws) |
| Typing, Enter, IME commit | "Hello from Linux, pair B!" and "日本語 ok" in Notepad (screenshots) |
| Clicks | dismissed Notepad's tip, placed the caret |
| Scroll | wheel events injected (no visible effect in an empty Notepad) |
| Cursor | arrow and I-beam shapes with hot spots, positions follow |
| Clipboard | text with Ü/日本 both ways; PNG Windows→client byte-identical; PNG client→Windows lands as PNG + DIBV5 + DIB |
| Audio | 1178 packets in 10 s, RMS 0.2232 for a 0.2236 tone |
| Disconnect, reconnect | clean end ("left: User"), next connect gets a keyframe at once |
| Secure screen | UAC prompt: true at 4.9 s, false at 9.7 s after it closed; lock screen: true, session kept |

## Limits

- One display at a time; a second client takes over.
- No virtual display, no UI scales, no home monitor. The display itself is never resized; only the stream follows the client's window.
- Programs running elevated can't be driven unless the host is elevated (UIPI). The secure desktop is never captured or driven.
- The CPU path copies each frame out of the GPU and converts and scales it with swscale (a window smaller than the display makes that work and the encoder's smaller). Scaling on the GPU is part of the same next step: the D3D11 video processor takes the desktop texture to an NV12 texture at the stream size, so the copy to system memory and swscale both go. A GPU path (D3D11 video processor into an NV12 texture straight into NVENC/AMF/QSV) is the next step for 4K at 60 fps.
- WGC (Windows.Graphics.Capture) isn't built: duplication works on the Basic Display Adapter, so it was never needed as a fallback.
- In the VM, full-screen changes stop at about 28 fps: the desktop image reads at about 400 MB/s there (plain or streaming loads alike). Hardware with a GPU wasn't available to measure.
- Not verified without hardware: NVENC, AMF, QSV and the GPU MFTs on Windows, several displays, display names, DPI scales other than 1.
- The mouse pointer is sent as shape and position, never drawn into the picture.
