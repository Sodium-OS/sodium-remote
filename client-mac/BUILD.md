# Sodium Remote for Mac: building and testing

The app is `build/Sodium Remote.app` (bundle ID `com.sodiumos.remote`). The build makes one static library, `libremote_ffi.a`, with the Mac host in it (`host-desktop/build-swift.sh`). The first-compile and first-run checklist is [docs/reference/mac-app.md](../docs/reference/mac-app.md); this file is the path from a clean Mac to a running app, then what to check after the first run.

> **Test builds.** Every build for testing is made with `./build-app.sh --test-id`: bundle ID `com.sodiumos.remote.dev`, name "Sodium Remote (test)", output `build/Sodium Remote (test).app`. It keeps its data in `~/Library/Application Support/com.sodiumos.remote.dev` (the host's in its `Host` folder), its Keychain items under that ID, its settings in `~/Library/Preferences/com.sodiumos.remote.dev.plist` and its stats in `~/Library/Logs/com.sodiumos.remote.dev`, so it never touches an installed Sodium Remote. Read `Sodium Remote.app` and `com.sodiumos.remote` below as `Sodium Remote (test).app` and `com.sodiumos.remote.dev` for a test build. `--install` refuses to run.

Everything here runs in a terminal except where macOS insists on a click (permissions, the Keychain).

## 1. From a clean Mac (once)

1. **macOS 14 or later** (`CAMetalDisplayLink` needs it).
2. **Command Line Tools** (Swift 5.9+, the macOS SDK, `codesign`). Over SSH, without the GUI installer:
   ```sh
   touch /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress
   softwareupdate -l | grep -o 'Command Line Tools for Xcode-[0-9.]*' | tail -1 | xargs -I{} softwareupdate -i "{}" --verbose
   rm /tmp/.com.apple.dt.CommandLineTools.installondemand.in-progress
   swift --version          # 5.9 or later
   ```
   Full Xcode is not needed. Metal shaders are compiled at run time, because the Command Line Tools have no Metal compiler.
3. **Rust** (the core is built for `aarch64-apple-darwin`, the Mac's own target):
   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   source ~/.cargo/env
   rustup target add aarch64-apple-darwin
   ```
4. **The repository:**
   ```sh
   git clone https://github.com/sodium-os/sodium-remote.git && cd sodium-remote
   ```

## 2. Build

```sh
cd client-mac             # in the repository's root
./build-app.sh            # the library + bindings, swift build, build/Sodium Remote.app, ad-hoc signed
./build-app.sh --test     # the same, plus `swift test` (RemoteKit: layout, timeline, key table)
./build-app.sh --skip-rust   # Swift-only changes: reuse Generated/
```

The script runs `host-desktop/build-swift.sh mac`: it builds `libremote_ffi.a` (host-desktop with remote-core's surface) and generates `RemoteCore.swift`, `SodiumRemoteHost.swift` and their headers from that library, so the bindings always match it. It puts them in `Generated/`, checks that every string in `App/Localizable.xcstrings` has all ten languages, runs `swift build -c release` and assembles `build/Sodium Remote.app` from `App/Info.plist`, with the notices file in it (below). Then it signs with `codesign -s -`. The first Rust build takes a few minutes. Nothing it writes is committed (`Generated/`, `.build/`, `build/`).

- **`swift test` says `no such module 'XCTest'`:** some Command Line Tools releases ship without XCTest. Install Xcode (only for the tests), or skip them: the app doesn't need them.
- **Stable permissions across rebuilds (optional):** an ad-hoc signature changes with every build. macOS may then ask again for Accessibility, and the Keychain asks again to let Sodium Remote read its key. A self-signed identity fixes both:
  1. Create it once at the Mac: Keychain Access › Certificate Assistant › Create a Certificate…, name `Sodium Remote Dev`, type Code Signing.
  2. Build with it: `REMOTE_SIGN_IDENTITY="Sodium Remote Dev" ./build-app.sh`.
- **The notices file (a release):** About › Third-party notices opens `THIRD-PARTY-NOTICES.txt` from the app's Resources: the acknowledgements (`App/Acknowledgements.md`) and cargo-about's list of the Rust libraries. Make the list first, from the repository's root:
  ```sh
  cargo about generate --manifest-path host-desktop/Cargo.toml about.hbs > build/THIRD-PARTY-NOTICES-mac.md
  ```
  (`REMOTE_NOTICES=<file>` names another place; without the file `build-app.sh` warns and the notices hold the acknowledgements only.)

## 3. First run

```sh
open "build/Sodium Remote.app"                         # needs you logged in at the Mac
"build/Sodium Remote.app/Contents/MacOS/Remote"        # the same, with its log in this terminal
log stream --predicate 'subsystem == "com.sodiumos.remote"'   # or the unified log
```

- **Gatekeeper:** an app built on this Mac carries no quarantine flag, so it opens without a prompt. If it was copied from elsewhere, clear the flag with `xattr -dr com.apple.quarantine "build/Sodium Remote.app"`, or right-click › Open once.
- **The welcome (design 2)** has two rows. Each **Allow…** triggers macOS's own prompt:
  - **Accessibility:** System Settings opens at Privacy & Security › Accessibility; switch Sodium Remote on. It's needed for ⌘Tab, ⌘` and ⌘Space (and in full screen every system key) to reach the computer. Without it they stay on the Mac and a toast says so. After an ad-hoc rebuild, switch it off and on again (or remove and re-add the app).
  - **Local network:** the mDNS browse makes macOS ask; allow it. If it was refused, change it in Privacy & Security › Local Network. Without it, Sodium Remote can't find computers on the network by itself; connect by address.
  - The microphone isn't asked: it isn't used in 0.1.
- **The Keychain** asks once per build to let Sodium Remote use "Sodium Remote device key" (and "Sodium Remote host key" once this Mac is a host): choose Always Allow. Those items are this Mac's Ed25519 identities as a client and as a host. Deleting one makes a new device that has to be paired again.
- **Pair:** on the computer, open its pairing window (Sodium Remote › Pair a device, or Settings › Remote on SodiumOS). In Sodium Remote, choose Pair and type the 6 digits. The code never leaves the Mac.
- **If mDNS doesn't find the computer on the LAN:** core's mdns-sd shares port 5353 with macOS's mDNSResponder, which is untested. Give the core the host's LAN listener directly, then restart Remote:
  ```sh
  defaults write com.sodiumos.remote RemoteLanHints -array '<HOST-ID>@<IP>:<port>'
  ```
  The ID is the full 43-character one (Copy ID on the card). The port is the LAN listener's (47311 by default; the host's log or its page says).
- **Files:**
  - Settings: `~/Library/Preferences/com.sodiumos.remote.plist`.
  - `paired.json` and the cards' thumbnails: `~/Library/Application Support/com.sodiumos.remote/` (the host's in its `Host` folder; the keys are in the Keychain).
  - Stats: `~/Library/Logs/com.sodiumos.remote/session-*` (Help › Show Stats Files).

## 4. VideoToolbox and the host's HEVC

```sh
build/s6-probe Tools/samples/yuv444.hevc --loops 30 --csv build/probe-444.csv
build/s6-probe Tools/samples/nv12.hevc   --loops 30 --csv build/probe-420.csv
```

- **What it prints:** whether a decoder that *requires* hardware was created, `UsingHardwareAcceleratedVideoDecoder`, the output pixel format, and decode time per frame (p50, p95, max; the first loop is warm-up).
- **Exit codes:** 0 clean, 2 decode errors.
- **The samples:** 4 frames each (an IDR and three P-frames), **2940 × 1848** (the Mac's backing pixels, §7) at 100 Mbit/s: `yuv444.hevc` 0.90 MB and `nv12.hevc` 0.87 MB. They come from an NVENC encode with the host's settings: Main 4:4:4 (RExt) and Main 4:2:0, BT.709 full range.
  - They were made at 2940 × 1848 on 2026-11-05. Before that the tree held 2560 × 1664 samples (0.88 and 0.85 MB); `Tools/make-s6-samples.sh OUT 4 2560x1664` makes those again for a comparison.
- **Remaking them on the host:** `Tools/make-s6-samples.sh [OUT] [FRAMES] [WxH]` (WxH defaults to 2940x1848; 10 frames for the whole run).
- **The app does the same check at launch.** `HardwareProbe` uses the 4:4:4 stream's parameter sets, and the result goes into `Hello.decode.hevc_444`. That is what makes Auto pick 4:4:4.
  - The app's log prints `HEVC 4:4:4 hardware decode: true/false`.
  - Settings › Quality says which one Auto uses.
- **Record:** hardware yes/no for each file, and p50/p95 at 2940 × 1848 (and at 2560 × 1664 with samples remade at that size, for the comparison).

## 5. Click-to-photon

1. On the computer the Mac streams, run a full-screen test surface with a marker at 64,64 (256 × 256) that flips colour on every click and logs the flip times (you need your own test tool for this; none ships with Sodium Remote).
2. On the Mac, connect, preferably full screen, then run Help › Run the Latency Test…. It clicks the marker 100 times at random gaps, finds each flip in the decoded frames, and closes the loop with the frame's `presentedTime`.
3. The results go to `~/Library/Logs/com.sodiumos.remote/`:
   - `session-<t>-clicks.csv` and `session-<t>-latency.md`: the table and the per-stage breakdown.
   - `session-<t>-frames.csv`: client-linux's columns, plus `deadline_us,target_present_us`.
   - `session-<t>-summary.json`: includes `lead_check`.
4. **The first check:** in `summary.json`, `lead_check.pass` should be true. That means ready → present p50 is within 3 ms of the lead p50 plus deadline → vsync. If they're a whole period apart, the Mac shows every frame one refresh late.

## 6. What to check after the first run

1. `./build-app.sh --test` completes (see §8 if the build stops).
2. **The decode probe (§4):** both samples decode, and 4:4:4 hardware is yes or no.
3. **The app opens:** the welcome, both rows updating after Allow…, then the empty list with "This Mac: … · ID XXXX-XXXX".
   - The name is the Sharing computer name, as System Settings › General › Sharing has it. Rename the Mac there, relaunch: the log says `this Mac was renamed`, and the list follows.
4. **Pairing:** a wrong code first (red ring), then the right one: "… is paired", and a card appears.
5. **Connect on the LAN:**
   - The connecting card shows only if it takes over 600 ms.
   - First frame: under 1.5 s (the log says `first frame: connected … ms, first frame … ms`).
   - The chip reads "LAN · N ms".
   - The bar is the unified toolbar's height (52 pt on macOS 14 and 15): measure it. A 24 pt chip sits 14 pt from each edge.
6. **Sharpness and size** (the stream is the window's backing pixels):
   - The Welcome in full screen says 2940 × 1848 at scale 2.0 (p = 2); Settings › Display says the same for "Match the display".
   - Text is crisp: a terminal in the stream at the refinement stop is as sharp as the Mac's own terminal beside it. The stream sits 1:1 in the window's backing, so the one scale is the one macOS gives every window.
   - The HUD's Size is the window's backing pixels, rounded down to the scale's period.
   - A window drag steps by 1 pt at scale 2.0 (p = 2, backing scale 2; 4 pt at 1.6).
   - The band is at most p−1 px (1 px at 2.0), in the dark letterbox colour.
   - UI size: Smaller is 1.75 and Larger 2.333 (p = 7); the list never offers a scale with p over 8.
   - Settings › Quality: switch Auto → HEVC → AV1 while connected. The HUD's codec line follows within a second, with one keyframe and no reconnect. HEVC reads "HEVC 4:2:0" even when this Mac decodes 4:4:4.
   - The decode probe at 2940 × 1848 (§4).
7. **Keys:**
   - Typing.
   - ⌘C / ⌘V in kitty: copy and paste through the host's table.
   - ⌘← / ⌘→: Home and End.
   - Globe acts as Win (set Globe to "Do nothing" in macOS Keyboard settings).
   - In full screen with Accessibility allowed, every key goes to the computer and ⌘ is Win: ⌘Tab arrives as Win+Tab, ⌘ alone opens the launcher, ⌃← stays on the computer. ⌘Esc leaves full screen without opening the launcher; ⌘⇧Esc disconnects.
   - ⌃⌥⌘K releases the keyboard; a click takes it back. After ⌃⌥⌘K, ⌘Tab switches Mac apps again.
8. **The Mac's input method:**
   - Pinyin preedit shows inline in a host app, with the candidate window at the caret, and commits land.
   - At the lock screen's password field: secure input on, the IME off.
9. **The trackpad:**
   - Scroll with momentum, with no double fling: in a GTK4 window, Firefox and kitty, one fling and nothing extra at the tail; a touch stops it; a slow lift with no momentum gets its stop within 50 ms. If a toolkit flings from the tail anyway, end the finger scroll when a momentum event falls under 1 px and drop the rest; never send a stop between `ended` and the momentum's `began`.
   - Pinch.
   - A 3-finger swipe switches the computer's spaces (where it has them). Set macOS's "Swipe between full-screen apps" to four fingers first, or macOS takes it.
   - **The swipe constant** (`k` in `StreamView.touchesMoved`, 1.5 to start): a swipe across about a third of the trackpad moves one space. Set `k` to what gives that.
10. **The clipboard** both ways, including an image. **Audio** plays (Opus): if `AVAudioConverter` refuses the packets ("audio: no Opus decoder" in the log), libopus in the core is the fix, not a workaround in Swift. A Wi-Fi blip is a gap of silence, not a crash.
11. **The network:**
    - Turn Wi-Fi off for 5 s: the chip turns amber, the pill reads "Waiting for the network…", and the session resumes after Wi-Fi comes back.
    - Stop the host program: "Reconnecting…", red "Lost" after 30 s, then the Disconnected card at the hold's end.
12. **Full screen:** no toolbar or glass bar; the stream fills everything below the camera housing, with no band above or below it. Pointing into the black row shows our menu under macOS's menu bar.
13. **The click-to-photon test (§5):** p50 ≤ 25 ms on the home LAN; record it.
14. **The link in the HUD**:
    - After a page flip the Text line reads "readable N ms · lossless N ms · target 65 Mb/s" (130 at 120 Hz). The target is there 3 s after connect on the home LAN and unchanged after the flip. The stream pill's Mb/s stays the received rate.
    - The Link line ("C_est 19 Mb/s · ceiling 23 Mb/s") shows each part only while the host holds it, and is absent on the home LAN. On a relayed path `summary.json` has `host_target_bps` about 18 000 000, `host_c_est_bps` about 19 000 000 and `host_probe_ceiling_bps` set. Item 3's text figure is read from the same Text line.

## 7. How it's built

```
Package.swift            SwiftPM: RemoteCoreFFI (generated C headers) + RemoteCore (generated Swift, linked with
                         Generated/lib/libremote_ffi.a), RemoteKit, RemoteUI, Remote (the app), S6Probe (the decode probe), RemoteKitTests
build-app.sh             library → bindings → string check → swift build → Sodium Remote.app → codesign
App/Info.plist           bundle id com.sodiumos.remote (`__BUNDLE_ID__` and `__NAME__` are filled in by build-app.sh; `--test-id` makes com.sodiumos.remote.dev); Local Network, Bonjour (_sodium-remote._tcp), Microphone usage strings
Sources/RemoteKit/       pure Swift, no AppKit, no core: StreamLayout, ReconnectTimeline, the key table
                         and Mac key codes → HID usages, Annex-B/HEVC/AV1 parsing, FrameMeta, StatsRecorder, ClickTest, HostLine
Sources/RemoteUI/        SwiftUI, Graphite Glass (Theme, Components), the models (AppModel, StreamModel, PairingModel,
                         RemoteSettings), the host list, welcome, pairing, settings and key table, the stream's
                         toolbar, toasts, pill, cards, HUD and top-edge menu
Sources/Remote/          AppKit: AppDelegate (seven menus), AppController (core, hosts, pairing, settings),
                         StreamSession (connect flow, events, timeline, input routing, latency test),
                         StreamWindowController, StreamView (input, NSTextInputClient), VideoDecoder (VideoToolbox),
                         Presenter (Metal, CAMetalDisplayLink), MediaPipeline, AudioPlayer, Clipboard, Platform
                         (permissions, NWPathMonitor, the system-key event tap, cursors), SecureInput, Store, CoreBridge
Tools/S6Probe/           the decode probe CLI;  Tools/samples/  the two HEVC samples;  Tools/make-s6-samples.sh
Tests/RemoteKitTests/    layout and increments, the timeline's thresholds, key codes and the key table
```

**The hot path:** the core's listener thread → `VideoDecoder` (its own queue, synchronous VideoToolbox) → `CVMetalTextureCache` textures → `Presenter`.
- The presenter runs `CAMetalDisplayLink` on its own thread, with `preferredFrameLatency` 1. The latest frame wins.
- Presented frames go back to `MediaPipeline`, which does three things:
  - writes the stats CSV;
  - sends a receiver report per frame;
  - adds `LeadStats(ready, deadline)` and sends `Msg::Vsync` about once a second.
- No step of this path touches the main thread.

**Sizes:**
- The stream is in the window's backing pixels: points × `backingScaleFactor`, which is 2 on every Retina Mac whatever "looks like" it is set to. macOS composites every window at that size and scales it once to the panel, so a stream in backing pixels gets the one scale every Mac app gets (a stream in panel pixels would go through two filters). On the 13″ Air at the default 1470 × 956 pt, full screen is 2940 × 1848.
- The drawable is `convertToBacking(bounds)`, and the stream is placed 1:1 in it.
- `pixels_per_point` is the backing scale (2.0), so Match Mac is exactly 2.0 on the host (p = 2); `display_width/height` is the size full screen would stream, below the camera housing.
- `resizeIncrements` are p ÷ the backing scale: 1 pt at 2.0, 4 pt at 1.6.
- The scale list drops periods over 8 (host and `StreamLayout.validScales`): at 2940 × 1848 Smaller is 1.75 and Larger 2.333, both p = 7.
- The cost is 32 % more pixels than the panel's (2940 × 1848 against 2560 × 1608), so about 32 % more bits at the same quality and more encode and decode time; the demand at 60 Hz is 65 Mbit/s (0.2 bit/pixel/frame). "Smaller" or a fixed resolution cuts the pixels on a slow link.

## 8. If the build stops

The Mac app has had little compile time on real toolchains. These are the likeliest places for a first error.

- **Concurrency (Swift 5 mode, tools 5.9):**
  - UI types are `@MainActor`, and background callbacks hop to the main thread with `onMain { }` or `MainActor.assumeIsolated`.
  - A few `Binding(get:set:)` closures read `@MainActor` models. If the SDK marks them `@Sendable`, expect warnings; on an error, wrap the body in `MainActor.assumeIsolated`.
- **Name clashes:** RemoteCore's generated types include `Path`, `Button`, `Axis`, `Rect` and `Stats`. Files that import RemoteCore never import SwiftUI, and Carbon is only in `SecureInput.swift`. A clash shows up as "ambiguous for type lookup"; fix it with `RemoteCore.X`.
- **APIs most likely to differ between SDK versions:**
  - `VTSessionCopyProperty`'s out parameter (`&hw`).
  - `CVBufferCopyAttachment` (macOS 12+).
  - The block form of `VTDecompressionSessionDecodeFrame` (trailing closure).
  - The delegate signature for `CAMetalDisplayLink.Update`.
  - `AVAudioCompressedBuffer` for Opus (`byteLength`, `packetDescriptions`).
  - `CGEvent.tapCreate` with a global C callback.
  - `NSToolbarItem.view` holding an `NSHostingView`.
  - `ImageRenderer.nsImage` for the Dock icon.
  - SwiftUI `Menu` with `.menuStyle(.borderlessButton)` (deprecated, still available).
  - `.onKeyPress` and `.focusEffectDisabled` (macOS 14).
  - `NSView.convertToBacking(bounds)` for the drawable and the requested size (`StreamSession.backingSize`): it should be points × the window's backing scale, whole at 2×. Check the HUD's Size against the window.
  - `NSScreen.safeAreaInsets.top` for the camera housing's row (2940 × 1848 in full screen on the 13″ Air).
  - The generated names the Mac calls: `RemoteClient.setName(name:)`, `RemoteClient.setRelayOnly(on:)` and `CodecChoice.hevc420`.
- **Run-time things to watch:**
  - The toolbar's titlebar height: the stream view is pinned to `contentLayoutGuide`.
  - The overlays' hosting views must never take clicks while hidden.
  - Apple's Opus decoder: if `AVAudioConverter` refuses Opus, the log says `audio: no Opus decoder` and the stream has no sound.

## 9. Known limits

- **The microphone** and file drag and drop are not in 0.1. The title bar has no mic button.
- **The Paths list** in the HUD shows only the route in use and the hedge route. The core doesn't expose ICE's other candidates.
- **The connecting card** has no candidate chips, for the same reason.
- **A changed computer key** can't be detected: the device ID is the key (the Noise static is derived from the Ed25519 key), so a computer whose key changed has a new ID. The Mac never dials the old ID at the new host; the old card goes "Offline" for good, and a reinstalled host appears as a new card. Remove the old one by hand. "Couldn’t connect" stays for Timeout and Failed.
- **"Full screen beside the camera"** is stored but not applied. macOS keeps full-screen windows below the housing.
- **Swipe units:** trackpad points × 1.5 is a starting value, not yet measured on a range of Macs (§6.9).
- **The device name on the computer's list:** the Mac's name is the Sharing name, read at every launch and given to the core (`RemoteClient.setName` when it changed since the last launch), so pairing, connection offers and `Hello` carry it. The computer's paired list follows it at the next connection unless the desk renamed the device (an empty name set at the computer hands it back).
- **Design choices in 0.1:** momentum scrolling is macOS's own; there is no microphone button; the key tap takes ⌘Tab, ⌘` and ⌘Space in a window, and every system key except ⌘Esc and ⌘⇧Esc in full screen; a lost 10 ms of Opus plays as silence. Planned later: libopus in the core for FEC and concealment, Mission Control's keys as an option, the microphone.
