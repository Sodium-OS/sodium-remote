# The generic Linux host

`host-desktop`'s third backend: the computer's real display on any desktop whose xdg-desktop-portal implements RemoteDesktop and ScreenCast. In 0.1 that means GNOME and KDE Plasma 6 on Wayland. It is the same program as on Windows and the Mac, `sodium-remote-host`, with the backend `portal` (cargo feature `linux-portal`, on by default; it does nothing off Linux). SodiumOS keeps `remoted`. The user-facing page is [hosts/linux.md](../hosts/linux.md).

## Layout

| Where | What |
|---|---|
| `host-desktop/src/linux/mod.rs` | `PortalBackend` (the `Backend` trait), names on disk (`names`), the default data directory |
| `host-desktop/src/linux/portal.rs` | The portal session on its own thread and tokio runtime (`ashpd`): Start, the restore token, OpenPipeWireRemote, ConnectToEIS, the Notify methods, the clipboard signals, Closed |
| `host-desktop/src/linux/capture.rs` | The ScreenCast node as a `Capture`; `DmaPicture`; the DMA-BUF kill switch |
| `host-desktop/src/linux/input.rs` | libei through `reis` (`Ei`), and `PortalInput`, which falls back to Notify |
| `host-desktop/src/linux/clipboard.rs` | The `Clipboard` over the portal's |
| `host-desktop/csrc/hd_shim.c` (`HD_VAAPI`) | VA-API: NV12 upload, and DMA-BUF import through a filter graph |
| `linux-media/` (`remote-linux-media`) | PipeWire in C: remoted's audio shim (moved here), the default sink's monitor, the ScreenCast video stream (`lm_video.c`) |
| `host-desktop/tests/linux/` | `e2e.sh` (the VM run) and `sr-testapp.py` (the guest's witness window) |
| `tools/vm/linux.sh` | The Arch VM with GNOME and KDE Plasma ([tools/vm/README.md](../../tools/vm/README.md)) |

## The portal session

- **When.** The portal session opens when a device connects (`Backend::check_ready`, run off the session's event loop because the dialog can wait for a person) and closes when that device's session ends (`session_active(false)`). Nothing is shared, and no screen-sharing indicator shows, while nobody is connected. A held session (connection lost, waiting to resume) keeps it open.
- **Start.** CreateSession; SelectDevices (keyboard and pointer, persist mode 2 "until revoked", the saved restore token); SelectSources (one monitor, cursor mode metadata, not multiple); RequestClipboard; Start. The person at the computer chooses the screen in the dialog; that is the one display the host reports.
- **Restore token.** Start returns a token; it is saved in the data directory as `portal-restore-token` (mode 0600, written through a temporary file). Tokens are single-use, so every Start saves the new one. With a token the desktop starts the session with no dialog: a device reconnects, or the host restarts, without anyone at the computer. A declined dialog deletes the token. `--forget-consent` (and the pane's "forget") deletes it too.
- **Consent** (`backend::Consent`, reported as `Note::Consent`): `Ask` (no token: the next connection shows the dialog), `Asking` (the dialog is up), `Remembered` (a token), `Declined` (the last dialog was cancelled), `Unavailable(detail)` (no portal with RemoteDesktop and ScreenCast).
- **Declined.** The device is refused with `DisconnectReason::EndedAtComputer` ("Ended at the computer"). No new reason: `PermissionMissing` is the Mac's permission card.
- **View only.** GNOME's dialog has an "Allow Remote Interaction" switch, off by default. When the session comes back without keyboard and pointer, `Backend::view_only` is true and `Welcome.view_only` says so.
- **Locked.** GNOME closes the portal session when the screen locks and refuses a new one while it is locked. On `Closed`, and on a Start that fails with a portal error other than Cancelled, the host reads logind's `LockedHint` for its own session (`XDG_SESSION_ID`, else the session of the process; zbus on the system bus, the user's graphical session when the process is in none) and, if it says unlocked, once more after about 300 ms. GNOME Shell 51 in the test VM never set the hint, so GNOME's `org.gnome.ScreenSaver.GetActive` on the session bus is read as well, and either one means locked. If locked, the device gets `DisconnectReason::HostLocked` (JSON `host_locked`: "%1 is locked", "Unlock it at that computer, then reconnect."). The consent stays Allowed and the restore token is kept. KDE keeps the session, so nothing ends there.
- **Stopping.** The person at the computer stops the share (GNOME: the top bar's orange indicator; KDE: the tray item's End). The portal sends `Closed`, the capture returns `SharingEnded`, the media loop reports `MediaEvent::Ended`, and the session ends with `EndedAtComputer`. The capture does not try again. A PipeWire stream that ends after it started counts as the same.
- **App ID.** xdg-desktop-portal names a host program by its systemd unit (`app-<ID>@<anything>.service` or `.scope`) when a desktop entry for that ID exists. The host runs as `org.sodiumos.Remote` (the Qt app's ID), so KDE shows "Sodium Remote" instead of "an unidentifiable application" and can pre-authorise it. A host started from a shell gets no ID; that works, but KDE's dialog says the app can't be identified.

## Capture

- `lm_video.c` connects to the portal's PipeWire remote (one OpenPipeWireRemote per capture: two PipeWire connections can't share a socket) and links to the stream's node.
- **Formats.** 32-bit RGB in any byte order, delivered as BGRx rows. With DMA-BUF (below) four EnumFormats with the LINEAR or implicit modifier come first, and SHM (MemFd, MemPtr) stays the fallback.
- **Metadata.** Header (corrupted buffers are skipped), Cursor (position, hotspot and a bitmap of up to 256×256; the size offered goes up to 1024×1024 because mutter's range doesn't meet a 256 cap), and VideoDamage (up to 16 regions; none, or an overflow, means the whole picture).
- **Buffers.** The newest buffer with a picture is held until the media thread takes it. An older one goes back to the compositor at once and its damage is merged into the next. Cursor-only buffers (GNOME sends them when only the pointer moves) update the pointer and go back at once.
- **Size.** The portal's size is logical. The pixel size comes from PipeWire's format: `ensure_started` opens the stream once and waits up to 3 s for it. That capture is handed to the media loop. The display's `scale` is pixels ÷ logical.
- **The pointer** is never drawn into the picture. Its position is the hotspot in stream pixels; an empty bitmap means hidden. Bitmaps are premultiplied RGBA from both compositors, so only the byte order changes.

## Encoders

`Candidates::linux()`, tried in order, each opened once at start with one frame encoded (`encode::Probe`):

| Codec | Order |
|---|---|
| HEVC (hardware only) | `hevc_vaapi`, `hevc_nvenc` |
| H.264 | `h264_vaapi`, `h264_nvenc`, `libx264`, `libopenh264`, then `openh264` (OpenH264 used directly) |

- **Fallback.** A candidate that fails to open (no VA-API device, no NVIDIA driver) is skipped at the probe. One that worked at the probe but won't open at the stream's size falls to the next (`first_that_opens`, unit-tested).
- **OpenH264 last.** The system's library, loaded at run time (`softvideo`), for distributions whose FFmpeg has neither libx264 nor libopenh264.
- **VA-API, CPU path.** BGRx → NV12 with swscale, then `av_hwframe_transfer_data` to a VA surface. CBR, `async_depth` 1. A forced keyframe is an IDR.
- **VA-API, DMA-BUF path.**
  - DMA-BUF is asked for only when a `*_vaapi` encoder passed the probe, and not with `SODIUM_REMOTE_DMABUF=0`.
  - The picture stays in its buffer (`DmaPicture`). The encoder imports it as a DRM-PRIME frame on a DRM device derived from its VAAPI device, then runs a graph `hwmap=derive_device=vaapi,scale_vaapi=format=nv12` and re-opens on the graph's surfaces (one IDR).
  - If the import fails, DMA-BUF is switched off for the process: the frame is skipped, the capture re-opens on SHM, and the encoder carries on with uploads. **Not verified**: no machine here has a VA-API encoder (see Testing).
- **NVENC** takes the CPU path (p2, ull, zerolatency, as on Windows).
- **libx264** runs `ultrafast`, `zerolatency`, `forced-idr`, CBR with a one-frame VBV.
- `SODIUM_REMOTE_VAAPI_DEVICE` picks the render node.

## Input

- **libei** (RemoteDesktop v2, ConnectToEIS) through `reis`, on a thread of its own.
  - Handshake as a sender. Bind every capability the seat offers (pointer, absolute pointer, scroll, button, keyboard, text). Start emulating each device when it resumes. Each event is followed by a frame.
  - Events read with the handshake are handled before polling the socket. mutter sends the seat in the same burst as the handshake's end.
- **Keys:** HID usage → evdev code (`hid::hid_to_evdev`, the kernel's table), so the computer's own layout applies, as with a real keyboard. Every key and button held is released on `ReleaseAll`.
- **Pointer.**
  - Absolute moves go to the region whose mapping ID is the stream's (else the one at the stream's position, else the first), scaled by the region's size over the stream's pixels.
  - Relative moves are in logical pixels.
  - Buttons are BTN_LEFT…BTN_EXTRA.
  - Wheels use `scroll_discrete` in 120ths. Finger scrolls use `scroll` in pixels, then `scroll_stop`.
  - No pinch: libei has none.
- **Text** (IME commits):
  - `ei_text.utf8` where the server offers it (neither GNOME 49 nor Plasma 6.8 does yet).
  - Otherwise each character's key on the EIS keymap, through xkbcommon: level 1 with Shift, 2 with AltGr, 3 with both. Enter and Tab are mapped directly.
  - A character on no key is dropped and logged.
- **Fallback.** No ConnectToEIS (RemoteDesktop v1), or a dead EIS connection, uses the portal's Notify methods with the same mapping. Text then goes through `NotifyKeyboardKeysym`.
- **Foreground app:** none (no portal says which app has focus), so the key table's Terminal and Class rules are off.

## Clipboard

- The portal Clipboard (RequestClipboard before Start; it works only while a session runs). Text (`text/plain;charset=utf-8`, `text/plain`, `UTF8_STRING`, `STRING`, `TEXT`), `text/html` and `image/png`.
- **Out** (the computer's clipboard to the device): `SelectionOwnerChanged` from another owner → `SelectionRead` per wanted type. The pipe is set to blocking: GNOME's is non-blocking. Read with a 3 s limit → the session's offer.
- **In** (the device's to the computer): `SetSelection` with the types, and the data written on `SelectionTransfer`, then `SelectionWriteDone`.
- A secret is marked `x-kde-passwordManagerHint: secret` both ways (the protocol's `concealed`).

## Audio

`remote-linux-media::audio::Monitor`:
- a capture stream with `stream.capture.sink=true` and AUTOCONNECT, which WirePlumber links to the default sink's monitor and moves when the default changes;
- 48 kHz stereo float, read 10 ms at a time;
- Opus as on the other hosts.

remoted's virtual sink and microphone use the same shim (`audio::ffi`), unchanged.

## Names on disk

`remote_core::brand` and `remote_core::dirs` hold the product's names; `linux::names` adds the backend's own:
- the data directory `$XDG_DATA_HOME/sodium-remote/host` (`dirs::data_dir(Half::Host)`), without the first-start migration from `sodiumos/remote`: that identity is remoted's;
- `portal-restore-token`;
- the PipeWire node and EIS client name `sodium-remote-host`;
- the app ID `org.sodiumos.Remote` lives only in the test script and the docs: the program doesn't set it.

## The pane ("This computer" on Linux)

`remote-qt` with the `linux-host` feature (off by default: the SodiumOS build hosts with remoted) builds `client-qt/src/host.rs` on Linux too. It is the Windows `rh_*` C ABI with `host_desktop::platform_backend`:
- `rh_new`, `rh_enable(on, name)`, `rh_open_pairing`, `rh_close_pairing`, `rh_disconnect`, `rh_set_name`, `rh_revoke`, `rh_paired`, `rh_self`, as on Windows;
- `host_status` events now carry `client.view_only`;
- new events: `{"ev":"host_consent","state":"ask|asking|remembered|declined|unavailable","detail"}`;
- new calls: `rh_consent` (the state, from the saved token when hosting is off), `rh_forget_consent`, `rh_default_data_dir` (`~/.local/share/sodium-remote/host`).

The standalone Qt app turns the feature on with the CMake option `REMOTE_LINUX_HOST` (default ON when Sodium UI is built from the repository, OFF when a caller such as SodiumOS's `apps/` project provides `SODIUMUI_DIR`). `app/src/hostcontroller.cpp` (`Host`) and `app/qml/ThisComputerPage.qml` are the Windows page with the desktop's permission in place of the firewall:
- `Host.consent` (`ask`, `asking`, `remembered`, `declined`, `unavailable`) shows as Not asked yet, Waiting for you, Allowed (with **Forget permission…**, `rh_forget_consent`) and Not allowed; `ask` also says that the desktop will ask when a paired device first connects;
- `unavailable` is also decided when the app starts, without a portal session: an X11 session, or no `org.freedesktop.portal.RemoteDesktop` or `ScreenCast` on the session bus. The page then says so, the switch is off and dimmed, and it never turns on;
- `client.view_only` in `host_status` adds "can only watch" to the card that names who is controlling;
- not on Linux: the tray, the notice window, start at sign-in (no XDG autostart in the app yet), the firewall and network rows, the address row.

The app then links FFmpeg (avcodec, avutil, avfilter, swscale), OpenH264, Opus, libpipewire-0.3 and libxkbcommon.

## Running

```
sodium-remote-host [--data DIR] [--pair] [--no-mdns] [--bind IP] [--encoder NAME] [--list] [--forget-consent]
```

`--list` probes the encoders and prints the consent state without opening a portal session. Run it in the user's graphical session, for example as `systemd-run --user --unit=app-org.sodiumos.Remote@host sodium-remote-host` with `~/.local/share/applications/org.sodiumos.Remote.desktop` present. `SODIUM_REMOTE_HOST_DEBUG=1` logs libei events and PipeWire buffer metadata. `SODIUM_REMOTE_NO_EIS=1` skips ConnectToEIS so the portal's Notify methods carry the input (a test switch: both desktops offer libei, so nothing else reaches that fallback).

## Testing

- **Unit** (`cargo test -p host-desktop`):
  - the Linux encoder order, and falling back past missing or failing hardware;
  - the restore token: saved 0600, reused by a new host, forgotten;
  - a capture that reports `SharingEnded` ends the capture loop instead of re-opening.
- **A development PC** (`--list`, no portal touched; Intel iGPU without a VA driver, NVIDIA GPU): VA-API fails, so NVENC is used: `hevc_nvenc, h264_nvenc, libx264, openh264`.
- **The VM**, `host-desktop/tests/linux/e2e.sh gnome|kde [--fresh]`:
  - the host runs in `arch-desktops.qcow2` (`tools/vm/linux.sh`; llvmpipe, no GPU), as `app-org.sodiumos.Remote@e2e`;
  - the probe and client-linux run on the PC over the host-only bridge (the host binds the guest's bridge address, so no route goes through the guest's NAT NIC);
  - a GTK witness window in the guest records typing and clicks and reads the clipboard; `wl-copy` sets it;
  - GNOME's first dialog is answered by QMP clicks: Allow Remote Interaction, then Share;
  - KDE is pre-authorised in the permission store (`kde-authorized` / `remote-desktop` / `org.sodiumos.Remote` = `yes`).

### Results (2026-11-23)

GNOME 49 (mutter, xdg-desktop-portal-gnome; RemoteDesktop v2, ScreenCast v5, Clipboard v1) and KDE Plasma 6.8.3 (KWin, xdg-desktop-portal-kde 6.8.3; RemoteDesktop v2, ScreenCast v6, Clipboard v1), Wayland, xdg-desktop-portal 1.22.1, PipeWire 1.6, all `--fresh`:

| Check | GNOME | KDE Plasma 6 |
|---|---|---|
| First connection | dialog, answered by QMP clicks | no dialog (pre-authorised) |
| Picture | 1920×1080 H.264, libx264, shared memory | same |
| Pointer (libei absolute) | moved to 900,600; the cursor metadata came back at 900,600 | same |
| Click | witness press at 960,512 (window coordinates) | same |
| Keyboard + IME commit (libei keys) | "Hello from Sodium Remote ok" in the witness | same |
| Clipboard device → computer | the witness read "from the probe" | same |
| Clipboard computer → device | "from the gnome guest" offered | "from the kde guest" offered |
| Audio (sink monitor) | 2403 packets in 30 s, RMS 0.121 for the guest tone | 2954 packets, RMS 0.110 |
| Reconnect | restore token, no dialog | restore token, no dialog |
| Stop sharing at the computer | top bar indicator → client "EndedAtComputer" | tray End → client "EndedAtComputer" |
| Encoder fallback | vaapi, nvenc fail; libx264 | same |

Evidence: `remote/build/e2e-linux/{gnome,kde}/` (`result.txt`, `host.log`, `probe1.log`, `client3.log`, witness files, `evidence/*.png` screenshots of the dialog, the session, the reconnect and the stop).

### The real portal in the VM (2026-11-23)

Same VM, same desktops, after the security fixes (pairing limit, offer freshness, view-only, clipboard caps): `e2e.sh gnome|kde --fresh` passes all 11 checks on both, before and after the fixes from this run. Then the standalone Qt app (`remote/app`, system Qt 6.12, `REMOTE_LINUX_HOST=ON`) ran in the guest as `app-org.sodiumos.Remote@pane`, with the standalone probe as the paired device.

| Check | GNOME 49 | KDE Plasma 6.8 |
|---|---|---|
| "This computer" pane, first connect (consent) | PASS: pane says "Waiting for you" while GNOME's dialog is up; after Share, "Allowed" and "Probe is controlling this computer" | PASS: KDE's dialog (wording in [hosts/linux.md](../hosts/linux.md)); then "Allowed" |
| Second connect, restore token | PASS: no dialog, frames at once | PASS |
| Decline | PASS: client "EndedAtComputer", pane "Not allowed", no token kept | PASS (Deny) |
| Stop at the computer | PASS: top bar indicator; pane back to idle, still "Allowed" | PASS: tray item, End |
| "Forget permission" | PASS: confirm dialog, token gone, next connect asks again | PASS |
| In-use error | PASS: `sodium-remote-host` on the app's data dir exits with the worded error; the switch with the program already running shows the red banner | PASS (program side) |
| IME text (client → computer) | PASS for characters on the keyboard layout; é, ñ, ü and CJK are dropped (mutter's EIS has no text device) | PASS for all of them, including 日本語 (KWin's `ei_text`) |
| Software encode, libx264 | PASS: 1920×1080 H.264, 0 undecodable | PASS |
| Software encode, system OpenH264 (`--encoder openh264`) | PASS | PASS |
| `libopenh264` in FFmpeg | not in Arch's FFmpeg; the direct OpenH264 path covers it | same |
| Several monitors + HiDPI | PASS: two heads, the second at scale 2 (960×540 logical at 1920,0); each monitor shared in turn; clicks land at the expected window coordinates on both | PASS after the fix below |
| Notify fallback (`SODIUM_REMOTE_NO_EIS=1`) | PASS: pointer, click, typing; é and CJK dropped | PASS: pointer, click, typing including é and CJK |
| Lock screen | session ends; connecting while locked fails with the portal's error: both now show the client's "is locked" card (`HostLocked`, see below) | the session stays up and shows the lock screen; the password can be typed from the client |
| Pairing limit (H1) | PASS: three wrong codes close pairing, the fourth attempt and the right code are refused ("Closed"), the pane says "Pairing closed" | PASS |

Findings:
- **Fixed: switch off and on hung the next dialog.** ashpd keeps one D-Bus connection per process, and zbus ties its tasks to the tokio runtime that made it. A runtime per `Portal`, dropped when the "This computer" switch went off, left the next `Portal` talking to a dead connection: the pane said "Waiting for you" and no dialog ever came. All portals of a process now share one runtime thread (`portal.rs`, `runtime()`).
- **Fixed: KDE pointer with several screens.** KDE shares the whole workspace (one stream, no mapping ID, no screen chooser) while its EIS device has one region per screen. The pointer was mapped onto the first region only (x scaled by 1920/2880). It now maps onto the box around all regions (`pick_area`, unit-tested).
- **KDE with several screens shares all of them** as one picture (the box around the screens, in logical pixels, so a scale-2 screen is shown at half its pixels). GNOME asks which monitor.
- **Lock screen.** GNOME: the portal closes the session when the screen locks and refuses a new one while locked; the host answers `HostLocked` for both (see "Locked" above). Checked in the GNOME VM: locking during a live session gives the client `HostLocked`, so does connecting to an already locked screen, the restore token stays, and after unlocking the next connection needs no dialog; stopping the share with the top bar's indicator still gives `EndedAtComputer`. KDE: nothing ends; the client sees the lock screen and can unlock it by typing the account password, so the pane's limits line differs by desktop (it is keyed on a machine-readable desktop id, `Host.desktopId`: `gnome`, `kde` or `other`).
- GNOME's first dialog is not shown while a full-screen window covers the monitor it appears on; answer it before starting other windows.
- Plasma's dialog sits on the second screen when there are two; keyboard Enter answers it.

Evidence: `remote/build/e2e-linux/v1/` (per-run logs, the probe and witness output, `evidence` screenshots of every dialog, the multi-head screenshots and the lock runs).

### Not verified

- VA-API and the DMA-BUF import: the VM has no GPU (QEMU's standard VGA, llvmpipe), and the development PC has no VA-API encoder. Only the fallback past VA-API is verified, in the VM and on that PC.
- NVENC under the portal: verified only as a working encoder on the development PC, not with a portal stream.
- `ei_text` on GNOME (mutter offers none), PNG and HTML clipboard.
- The Qt client's own IME (fcitx) on another computer: the probe's committed text stands in for it.

## Limits (0.1)

- **Desktops.**
  - GNOME and KDE Plasma 6 on Wayland only.
  - Hyprland, Sway and other wlroots compositors: their portals don't implement RemoteDesktop with ConnectToEIS in a way this backend is tested with. A wlroots path (ext-image-copy-capture, virtual pointer and keyboard) is the second phase.
  - X11: not supported.
  - On SodiumOS, remoted is the host.
- **When it works.** No access before login. Lock screen: on GNOME the portal ends the session when the screen locks (client: "is locked"), and a connection to a locked GNOME gets the same card; on KDE the session continues and shows the lock screen (the password can be typed from the client). Nothing reports a secure screen.
- **Display.** One display, chosen in the dialog (GNOME); KDE with several screens shares them all as one picture. No switching from the device, and the display is never resized; the stream follows the client's window.
- **Someone must be at the computer once.** The first connection on GNOME needs a person to answer the dialog, and to turn on Allow Remote Interaction. After that the restore token lets devices connect alone, until the token is revoked or forgotten.
- **Text.** On GNOME, typed characters must exist on the computer's keyboard layout (mutter has no `ei_text`); KDE types any character. There is no pinch zoom.
- **Speed.** The CPU path copies each frame once out of PipeWire's shared memory and converts it with swscale.
