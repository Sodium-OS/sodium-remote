# host-desktop on the Mac

The macOS backend of `remote/host-desktop`, and its UniFFI surface for Sodium Remote.app's "This computer" pane. The session, media policy and protocol are the shared ones in [host-desktop.md](host-desktop.md); this file covers what is Mac-only.

**Checking:** `host-desktop/build-swift.sh check-mac` type-checks the backend for `aarch64-apple-darwin` from Linux, with and without `ffi`. The checklist at the end is for running it on a Mac.

- **Pure Rust.** The backend is built on the objc2 framework crates (0.3), with no Objective-C or Swift sources. The backend uses objc2 bindings rather than an Objective-C shim, so it type-checks off the Mac.
- **No FFmpeg.** The only C is Opus (`csrc/hd_opus.c`, built against a static libopus).

## Layout

| File | What |
|---|---|
| `src/mac/mod.rs` | `MacBackend`: the `Backend` trait over the pieces below. With no Screen Recording it opens nothing, and `check_ready` turns the client away with a reason. |
| `src/mac/main_thread.rs` | `on_main` (work on the main dispatch queue, with a bounded wait) and `run_main_loop` for the program. |
| `src/mac/display.rs` | Active displays: ID, backing pixels, bounds in points, scale, refresh, NSScreen name. |
| `src/mac/capture.rs` | ScreenCaptureKit video: `ScCapture`, the stream output class, frame info, the lock-screen check. |
| `src/mac/cursor.rs` | The pointer's position (CGEvent) and shape (NSCursor). |
| `src/mac/vt.rs` | VideoToolbox HEVC and H.264, the encoder probe, Annex B. |
| `src/mac/input.rs` | `CGEventPost`: keys, flags, pointer, buttons, scroll phases, pinch, text. |
| `src/mac/audio.rs` | ScreenCaptureKit system audio, 48 kHz stereo. |
| `src/mac/clipboard.rs` | NSPasteboard polling and writing. |
| `src/mac/desktop.rs` | The frontmost bundle ID, the terminal list, the computer's name. |
| `src/mac/permissions.rs` | Screen Recording and Accessibility. |
| `src/mac/power.rs` | The display-awake assertion during a session. |
| `src/hid.rs` (`hid_to_mac`), `src/policy.rs` (`mac_point`, `mac_pixel`) | The key code table and the pixel/point maths, unit-tested on any OS. |
| `src/ffi.rs`, `uniffi.toml` | The app's interface (feature `ffi`). |
| `build-swift.sh`, `build-opus-mac.sh` | The darwin check, the Swift bindings, the Mac's static library, static Opus. |
| `tests/mac/host.sh` | Build, sign and run the host on the Mac over SSH. |

## How it runs

- **In Sodium Remote.app (the product).** The app links the host and runs it in-process, so Screen Recording and Accessibility belong to the app's bundle ID and signature. There is no separate host process and no Swift host code.
- **As a program (tests).** `sodium-remote-host --backend mac` (the default on macOS) runs it on its own; then its permissions belong to the binary. Over SSH it must be started in the GUI session (`tests/mac/host.sh`), because an SSH process has no window server and would get sshd's permissions.
- **The main thread.** AppKit wants NSScreen on the main thread, and NSCursor's current cursor and NSWorkspace's frontmost app are kept current by the main run loop. So the backend reads them there through `on_main`, which waits at most 50–500 ms and never forever: the app's main thread may itself be waiting on the host, for example while switching it off.
  - In the app, AppKit runs that loop.
  - The program runs `run_main_loop` (NSApplication with the Prohibited policy, so no Dock icon) on its main thread, and the session on another thread.

## The backend

### Displays

- **ID:** the `CGDirectDisplayID`.
- **Size:** the current mode's backing pixels, which is what macOS renders. A 13″ Air "looking like" 1470 × 956 is 2940 × 1912.
- **Scale and position:** the scale is pixels over the points of `CGDisplayBounds`, and events are posted in those points.
- **Refresh:** 0 from CoreGraphics (a built-in panel) counts as 60 Hz.
- **Names:** NSScreen's `localizedName`, or "" when it can't be read, so that clients say "Display %1".
- **Order:** the main display (`CGMainDisplayID`) is first and primary.

### Capture

One `SCStream` per display, over an `SCContentFilter` of the whole display:

- **Format:** native pixels, `420v` (NV12, video range), BT.709, sRGB.
- **Cursor:** `showsCursor` off; the pointer goes as shape and position.
- **Queue:** `queueDepth` 5.
- **Rate:** `minimumFrameInterval` of 1/fps, at the stream's rate (the lower of the client's and the display's).

The frames:

- **New pictures:** only `SCFrameStatus.complete` samples carry one; `idle` means nothing changed, so no frame is sent.
- **Damage:** the dirty rects come from the frame info and travel as the frame's damage. A newer frame replaces a waiting one in a one-slot mailbox, and their rects merge.
- **When the whole picture goes:** the first frame, or a frame with no dirty rects.
- **Ends:** the stream ends when ScreenCaptureKit stops it (`didStopWithError`), or when a once-a-second check sees the display's size change; the media loop then opens a new one, as it does for Windows' access loss.
- **Debug:** `SODIUM_REMOTE_HOST_DEBUG=1` logs the first frames' status and rects.

### Pointer

- **Position:** `CGEventGetLocation` of a fresh event, every 16 ms, mapped to the display's pixels. Off this display the pointer is hidden.
- **Shape:** `NSCursor.currentSystemCursor` every 50 ms, drawn at the display's scale into premultiplied BGRA (at most 256 px), with the hot spot scaled the same; it is sent when its pixels change.
  - `currentSystemCursor` is deprecated but is the only public way to read the shape. When it returns nothing, the arrow stands in.
  - `CGCursorIsVisible` (also deprecated) hides the pointer while typing, where it still answers.

### Encode (VideoToolbox, used directly)

**Codecs and settings:**

- **HEVC Main** requires the hardware encoder (HEVC is offered only with hardware). **H.264 Constrained High** prefers hardware.
- **Low latency:**
  - Not `EnableLowLatencyRateControl`: on the M3 Air it doubled the encode time at 2940x1912 (HEVC 23.6 against 10.9 ms at 60 fps) and overshot the rate more.
  - `RealTime`, no frame reordering, `PrioritizeEncodingSpeedOverQuality`.
  - `MaxKeyFrameInterval` and its duration both 0: no periodic keyframes. A keyframe is forced only when asked (`ForceKeyFrame`).
- **Rate:** `AverageBitRate` and `DataRateLimits` (1.5 × the target over 1 s) follow core's target. They are set again only when the target moves by more than 10%.
- **Colour:** BT.709 primaries, transfer and matrix in the VUI, the same as the Windows encoders.

**Pictures:**

- **Zero copy:** the ScreenCaptureKit buffer (IOSurface NV12) is encoded as it is.
- **Other sizes:** when the stream is scaled down to the client's maximum, a `VTPixelTransferSession` scales it into a buffer from the session's own pool.
- **CPU pictures:** BGRA pictures (the fake backend) are copied into a pixel buffer and converted the same way.

**Out:** each `encode` completes its frame (`CompleteFrames`). The length-prefixed NAL units become Annex B, with the parameter sets in front of every keyframe.

**Probe:** at start, each codec opens and encodes one 640 × 360 frame. `--encoder videotoolbox_hevc|videotoolbox_h264` keeps only one.

### Input (needs Accessibility)

All input is posted with `CGEventPost` at the HID tap, from a `HIDSystemState` event source.

**Keys:**

- **Key codes:** `hid::hid_to_mac`, the inverse of the Mac client's key table. PC-only keys:
  - Print Screen, Scroll Lock and Pause become F13, F14 and F15;
  - Insert becomes Help, Num Lock becomes keypad Clear, and the Application key becomes the context-menu key;
  - LANG1 and LANG2 become Kana and Eisu.
- **Modifiers:** modifier keys are `FlagsChanged` events. Every event carries the held modifiers' flags, including the left/right device bits.
- **Hardware flags:** arrows add NumericPad and Fn; function and navigation keys add Fn.
- **Caps Lock** toggles.
- **IME commits** go as Unicode strings, 20 UTF-16 units per event, with no modifier flags; a newline is Return.

**Pointer:**

- **Absolute position:** stream pixels map to global points (`policy::mac_point`, clamped to the display).
- **Drags:** a move with a button held is that button's drag.
- **Relative motion** (pointer lock) moves from the current location and carries the deltas.

**Buttons:**

- Left, right, middle (other 2), back (3), forward (4), and any other button.
- Click counts come from `NSEvent.doubleClickInterval` within 4 points, which AppKit needs for double and triple clicks.

**Scroll:**

- **Wheels** scroll in lines (`value120` / 120).
- **Trackpad scrolling** (`Finger` source) goes in pixels, continuous, with the scroll phase: began, changed, and ended on `stop`.
- **Axes:** both axes of one pointer frame go in one event.
- **Direction:** Wayland's sign (positive is down/right) becomes CoreGraphics' (positive is up/left).

**Pinch**: a posted magnify gesture event, with Cmd + scroll as the fallback.
- **Magnify (the default):** an `NSEventTypeGesture` CGEvent (type 29) created with `CGEventCreate`, with the undocumented fields public open-source projects use and no private symbols: the gesture subtype (field 110 = 8, magnify), the incremental magnification (field 113, a double: 0.1 is +10%, as `NSEvent.magnification`), and the phase (field 132: 1 began, 2 changed, 4 ended). It is located at the pointer and posted at the session tap. Each `Update` sends `scale / last_scale - 1`; `End`, `Cancel` and `ReleaseAll` send the ended phase.
- **Fallback:** Cmd + scroll wheel, a line for about every 22% of scale (five per unit of ln(scale)), as Windows' Ctrl + wheel. Switch with `SODIUM_REMOTE_PINCH=cmd-scroll` in the host's environment, with no rebuild.
- **Which is active:** the host logs `pinch as magnify gesture events …` or `pinch as Cmd + scroll …` at the first pinch of a run.
- **Not verified:** the field numbers, the phase values and the tap are from memory of public projects and have never run on a Mac. If the checklist's pinch step fails, try the HID tap (`CGEventTapLocation::HIDEventTap`) before falling back; if nothing zooms, set the switch and record Cmd + scroll as debt.

**Release all:** releases every key and button held, and clears the flags.

### Keys

- **Key table:** the client's table is evaluated by core's `KeyMapper`.
- **Defaults:** a PC keyboard on the Mac uses `pc_to_mac()`, including these additions:
  - **In terminals:** Ctrl+Shift+C/V become Cmd+C/V.
  - **Outside terminals:**
    - Ctrl+Left/Right become Option+Left/Right;
    - Home/End become Cmd+Left/Right;
    - Ctrl+Home/End become Cmd+Up/Down.
  - Each family has its own unit test.
- **Foreground app:** the frontmost app's bundle ID, cached 200 ms.
- **The built-in terminal list:**
  - Terminal, iTerm2, Ghostty, kitty and Alacritty;
  - WezTerm and Warp;
  - Hyper, Tabby, Rio, Termius and Contour.

### Audio

A second `SCStream`, over the main display:

- **Settings:** `capturesAudio` on, `excludesCurrentProcessAudio` on, 48 kHz stereo, and a 2 × 2 picture at 1 fps that is ignored.
- **Samples:** Float32, planar or interleaved, interleaved into a queue that the Opus loop takes 10 ms at a time.
- **Stalls:** past 200 ms the oldest samples are dropped, so a stall never plays late.
- **Permission:** the same Screen Recording permission; no other is asked.

### Clipboard

- **Polling:** macOS has no pasteboard notification, so a thread polls `changeCount` every 250 ms.
- **Formats:**
  - `public.utf8-plain-text`;
  - `public.html`;
  - `public.png`, or `public.tiff` converted to PNG.
- **Our own writes** are recognised by their change count.
- **Concealed entries:** an entry with `org.nspasteboard.ConcealedType` (password managers) goes out concealed, and a concealed entry from the client is written with that marker.

### Secure screen

- **Detection:** the lock screen, or another user on the console (fast user switching), from `CGSessionCopyCurrentDictionary` every 250 ms.
- **What happens:** the host sends `SecureScreen(true)`, sends no pictures, and keeps the session. On return it sends `SecureScreen(false)` and a keyframe.
- **Input:** none is posted while the screen is secure.

### Power

While a session is live, an IOKit `PreventUserIdleDisplaySleep` assertion is held. `pmset -g assertions` lists it as "Sodium Remote session".

### Welcome

- **Fields:**
  - `host_os` Mac and `display_mode` Physical;
  - `live_resize` false, with no UI scales;
  - the displays as above, and the current one.
- **Display choice:** the primary display unless the client chose another.
- **Downscaling:** the picture is the display's backing pixels, scaled down only to fit the client's `max_width` and `max_height`.
- **Name:** the Sharing name (`SCDynamicStoreCopyComputerName`), unless renamed.

## Permissions

| | Needed for | Read | Ask |
|---|---|---|---|
| **Screen Recording** ("Screen & System Audio Recording") | anything shown, and the sound | `CGPreflightScreenCaptureAccess` | `CGRequestScreenCaptureAccess`: macOS's prompt the first time only; later it just answers no. An Allow works only after the process starts again. |
| **Accessibility** | keys and the pointer | `AXIsProcessTrusted` | `AXIsProcessTrustedWithOptions` with the prompt option: macOS's dialog, which leads to System Settings. It works at once. |

What the host does about them:

- **Without Screen Recording:**
  - the host still runs, pairs and is listed;
  - a connecting client is turned away with the typed `DisconnectReason::PermissionMissing { screen_recording: true, accessibility }` (`accessibility` is true when that is missing too); clients show their "Mac without Screen Recording" card from it, never from text;
  - no capture is ever opened, so macOS's prompt never appears in the middle of a session.
- **Without Accessibility:** the session runs view-only (macOS drops the events). `Welcome.view_only` is true, so the client shows a "View only" chip; `ControllingDevice.viewOnly` (Swift) carries the same for the pane, and the log says so once per session. `Backend::view_only()` is read whenever the status is worked out, so an Allow shows at the next status change (a connect, the lock screen, a reconnect), and `Welcome` carries it at connect time only.
- **What the UIs show (Swift pane, clients):**
  - **A client, on `PermissionMissing`** (the session ends with `Ended(reason)`): the card "this Mac hasn't allowed Screen Recording for %1" (host-naming rule: `%1` is "Sodium Remote", untranslated), with the Accessibility line added when `accessibility` is true; the card's action is on the Mac, so it names where to allow it. Qt's JSON has `{"kind": "permission_missing", "screen_recording", "accessibility"}`.
  - **A client, on `Welcome.view_only`:** a "View only" chip near the host's name (Qt: `welcome.view_only`), and no input need be sent.
  - **The pane:** a "View only" chip beside the controlling device when `ControllingDevice.viewOnly`, and a card that leads to Accessibility's "Allow…" while it is set.
- **In the pane** (`ThisComputer.permissions()`):
  - each row is `allowed` or `notAllowed`;
  - after "Allow…" (`requestScreenRecording()`) has been pressed in this run without effect, Screen Recording answers `reopenToFinish`. The row then shows "Reopen to finish" with "Quit and Reopen", never "Allowed": macOS doesn't say whether the person allowed it, and only a relaunch applies it.
- **Logs and the program:**
  - the program logs one line naming what is missing and where to allow it;
  - `--permissions` prints both permissions;
  - `--ask-permissions` shows macOS's prompts for those missing.
- **Signatures:** macOS ties both permissions to the code signature. An ad hoc signature changes with every build, so tests sign with a stable self-signed identity, as the app's BUILD.md does for the Keychain.

## The app's interface (`ffi`)

`cargo … -p host-desktop --features ffi` turns on remote-core's `ffi` too, and the result is one static library holding both crates' UniFFI scaffolding and one copy of Rust's std. In Swift:

```swift
let host = try ThisComputer(dataDir: hostDir, name: savedName, listener: self)  // off
try await host.start()             // "Allow remote control of this computer" on
await host.stop()                  // off
host.isOn() -> Bool
host.state() -> ThisComputerState  // .off, .waiting, .beingControlled(by:), .secureScreen(by:)
try await host.openPairing() -> PairingCode   // code: "123456", expiresInSecs
host.closePairing()
host.devices() -> [AllowedDevice]  // id, code, name, os?, pairedAt, lastUsed?; on or off
try await host.revoke(id:)         // "Remove": it must pair again; its session ends
host.name() / host.setName(name:)  // "Name shown to other computers" ("" = the Sharing name)
host.disconnect()                  // "Disconnect"
try host.deviceId() / host.deviceCode()     // this Mac as a host ("XXXX-XXXX")
host.permissions() -> HostPermissions       // screenRecording, accessibility: .allowed / .notAllowed / .reopenToFinish
host.requestScreenRecording() / host.requestAccessibility() -> PermissionState
```

**Events.** `ThisComputerListener.onEvent(event:)` arrives on a Rust thread, so hop to the main actor. The events:

- `.state(state:)`: every change of who controls the Mac, the secure screen, a lost connection being held (`by.reconnecting`), and off;
- `.pairingOpened(pairing:)`, `.pairingAttempt`, `.pairingFailed(attemptsLeft:)` and `.pairingClosed(reason:)`;
- `.devicesChanged`: read `devices()` again.

`ControllingDevice` has the device's `id`, `name`, `os` (core's `Os`) and `since` (Unix seconds), for the menu-bar item and the "being controlled" notification.

**Errors.** `ThisComputerError` is one of:

- `.off`, when the call needs the host on;
- `.unknownDevice`;
- `.failed(message:)`.

**Threading.** The async calls run on the host's own tokio runtime, and the sync calls return at once. Don't block the main thread waiting on `stop()`: the backend reads NSScreen and NSCursor there. `stop()` and the release of the object both stop a running session.

**Storage.**

- `dataDir` holds the host's paired devices (through the app's store), `client-os.txt` (each paired device's OS, as its last connection said) and `host.txt`.
- It is apart from the client's files: as a host this Mac is another device. The app uses `~/Library/Application Support/com.sodiumos.remote/Host`.
- The key is in the Keychain (its own item, account `host/device.key`, service `com.sodiumos.remote`): the app passes a `HostKeyStore` to `ThisComputer(dataDir:name:listener:keyStore:)`, and a key file left in `dataDir` by an earlier build is imported once and deleted. Run as a program (`sodium-remote-host`), the key is a PEM file in the data folder (`FileStore`).

**The app keeps the settings.** The switch, the name and "Start at login" are the app's own settings: it calls `start()` at launch when the switch is on, and passes the saved name.

### Linking it into Remote.app

The app's Swift sources and `client-mac/` scripts were not changed in this step. The changes the app needs:

- **Library:**
  - `host-desktop/build-swift.sh mac <dir>` builds `libremote_ffi.a` (host-desktop + remote-core) and the bindings; `build-app.sh` calls it.
  - Link `sodium_remote` instead of `remote_core`. Both together would duplicate core's symbols.
- **Swift files:**
  - The bindings are `RemoteCore.swift` and `SodiumRemoteHost.swift` (each crate's `uniffi.toml`), plus `RemoteCoreFFI.h` and `SodiumRemoteHostFFI.h`, with one `module.modulemap` for the `RemoteCoreFFI` C module that lists both headers.
  - Put both Swift files in `Generated/RemoteCore/`, so they compile into the one `RemoteCore` target.
  - `SodiumRemoteHost.swift` uses core's `Os` converter from `RemoteCore.swift`. That works only within one Swift module.
  - The two files' public names don't collide (checked on the generated files).
- **Frameworks:** add to the `RemoteCore` target's linker settings, beside today's Security, SystemConfiguration, CoreFoundation, iconv and resolv:
  - ScreenCaptureKit, VideoToolbox, CoreMedia, CoreVideo, CoreGraphics, IOSurface;
  - AppKit, Foundation, IOKit, ApplicationServices.
  - `build-swift.sh mac` writes rustc's own list to `native-static-libs.txt`.
- **Info.plist:** none is needed for Screen Recording or Accessibility. macOS names the app in its prompts.
  - If the app is ever sandboxed, it needs the `com.apple.security.network.server` entitlement for the LAN listener.
  - The first incoming connection may raise the Application Firewall prompt.
- **Generate on Linux:** `host-desktop/build-swift.sh bindings remote/build/swift-host`. `client-mac/Generated/` is ignored by git, so the bindings are not checked in.

## Opus

- **Source:** `build-opus-mac.sh` downloads opus 1.6.1 (pinned SHA-256) and builds a static `libopus.a` for arm64 (`MACOSX_DEPLOYMENT_TARGET` 14.0) into `remote/build/mac-deps/opus`.
- **Linking:** `build.rs` compiles `hd_opus.c` against it and links it statically. `SODIUM_REMOTE_OPUS` points elsewhere.
- **Off the Mac,** the C is compiled by a stub, so it only type-checks.

## Building

- **Off the Mac:** `host-desktop/build-swift.sh check-mac` checks the bins, examples and the library with `ffi`, for `aarch64-apple-darwin`.
  - A stub C compiler stands in for the missing SDK, so this is a check, never a build.
  - It doesn't check framework symbols that exist only in newer SDKs (the crates declare them all), the Opus link, or anything at run time.
- **On the Mac:**
  - Program: `host-desktop/tests/mac/host.sh build` (Opus, release, signed).
  - App library: `host-desktop/build-swift.sh mac <dir>`.

## First run on the Mac (checklist)

The user has to be at the Mac for the Allow clicks. Everything else can go over SSH.

1. **Separate checkout:** `git clone` the repository into a fresh folder just for testing.
   - Never replace or rebuild an installed Sodium Remote from it; the host runs as `sodium-remote-host` from the checkout's `target/release`.
   - `rustup target add aarch64-apple-darwin` if needed.
2. **Signing identity** (once, at the Mac): Keychain Access › Certificate Assistant › Create a Certificate…, named "Sodium Remote Dev", type Code Signing. Without it, every build needs the permissions granted again.
3. **Build:** `remote/host-desktop/tests/mac/host.sh build`. Check it links (the Opus and framework paths are untested until now).
4. **Permissions and displays:**
   - `host.sh run --permissions` should show both missing.
   - `host.sh run --ask-permissions`; the user clicks Allow / Open System Settings and turns on `sodium-remote-host` under Screen & System Audio Recording and Accessibility.
   - `host.sh run --permissions` should now show both allowed.
   - `host.sh run --list` should show the displays (names, pixels, scale) and "hardware HEVC true, H.264 true".
5. **Start:** `host.sh start --pair`. Read the device ID, port and pairing code from `host.log` and `build/mac-host/data/host.txt`.
6. **Pair D, this PC's client-linux → the Mac:**
   - `remote-client --data DIR pair --code N --lan-hint ID@<mac-ip>:47311`, then `connect`.
   - If the Mac isn't on this PC's LAN, use `--rendezvous` on both sides; the allowlist entry is added by whoever runs the server.
   - Check, with screenshots over SSH (`screencapture`):
     - the picture at native pixels, frames only on change (idle desktop → no frames), and HEVC chosen;
     - the cursor shape and position (arrow, I-beam, resize);
     - typing in TextEdit, including the `pc_to_mac` rules (Ctrl+C/V, Ctrl+arrows, Home/End) and in Terminal (Ctrl+C stays Control, Ctrl+Shift+C/V copies and pastes);
     - IME commit (日本語), clicks, double clicks, drags;
     - wheel and trackpad-phase scrolling in Safari, and pinch;
     - the clipboard both ways (text, HTML, PNG, and a screenshot's TIFF arriving as PNG);
     - audio (the RMS of a test tone);
     - a display switch, if there is a second display;
     - Disconnect and reconnect;
     - the lock screen (Ctrl+Cmd+Q) giving the secure notice, with the session kept; this needs the user to unlock;
     - `pmset -g assertions` during a session;
     - click-to-photon latency.
7. **Pair E, the Windows VM client → the Mac:** the same checks from the VM. The VM's host-only bridge can't reach the Mac directly; it needs the rendezvous path or a route (open question).
8. **No Screen Recording:** turn Screen Recording off, restart the host and connect. The client must be turned away with `PermissionMissing` (the permission card), and no prompt may appear. Then allow Screen Recording but not Accessibility: the stream must come with `Welcome.view_only` (the client's "View only" chip, the pane's chip), and typing must do nothing.
8a. **Pinch (magnify gesture):** from the Mac client or a pinch-capable client, pinch in Safari (page zoom), Chrome (page zoom), Preview (document zoom) and Maps (map zoom). The log must say `pinch as magnify gesture events`. Each should zoom smoothly in and out and stop at the end of the gesture. If one doesn't, start the host with `SODIUM_REMOTE_PINCH=cmd-scroll`, repeat in the same four apps and record which work. Record which works.
9. **Stop:** `host.sh stop`. Remove the test grants in System Settings afterwards if the user wants.

## Limits and open items

- **Not verified on a Mac:**
  - all of the above;
  - whether `currentSystemCursor` still returns shapes on macOS 15 and later;
  - the scroll phases' feel in apps;
  - whether pinch as a magnify gesture (the undocumented fields) zooms in Safari, Chrome, Preview and Maps, and whether Cmd + scroll zooms where the gesture doesn't.
- **One display at a time.** A second client takes over.
- **No virtual display or resize**.
- **No login-window access** (next milestone).
- **The host key is in the Keychain** when the host runs inside the app; run as a program it is a file in the data folder.
- **Pinch** is a magnify gesture event by default, with Cmd + scroll behind `SODIUM_REMOTE_PINCH=cmd-scroll`; neither has run on a Mac.
- **No Swift yet.** The pane's switch, pairing UI, menu-bar item and notification are the app's.
