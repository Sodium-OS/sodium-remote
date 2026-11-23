# Architecture

How Sodium Remote is put together: the parts, and how a frame gets from one computer to the other. For the formats, see [the reference](reference/).

## The shape

Every computer runs the same **core**, a Rust library that does the networking, the encryption and the session. Around it each platform adds a **host** (capture, encode, inject input) and a **client** (decode, present, send input). A computer can run both. A **server** introduces computers to each other across the internet and relays when it must.

```
 Computer A (the one you sit at)                      Computer B (the one you control)
┌──────────────────────────────┐                     ┌──────────────────────────────────┐
│ client app                    │                     │ host                              │
│  Qt app (Linux, Windows)      │                     │  capture   what is on screen      │
│  or Swift app (macOS)         │                     │  encode    HEVC / H.264 (hardware)│
│  - decode (hardware first)    │                     │  inject    keyboard, pointer      │
│  - present, draw the cursor   │                     │  clipboard, sound, displays       │
│  - send input, clipboard      │                     │                                   │
├──────────────────────────────┤                     ├──────────────────────────────────┤
│ remote-core (session)         │                     │ remote-core (session)             │
│  channels · FEC · pacing ·    │ <== encrypted ==>   │  channels · FEC · pacing ·        │
│  congestion control           │     packets         │  congestion control               │
│  Noise IK + ChaCha20-Poly1305 │   (UDP, or TCP/TLS  │  Noise IK + ChaCha20-Poly1305     │
│  ICE: direct, punched, relay  │    through a relay) │  ICE: direct, punched, relay      │
└───────────────┬──────────────┘                     └───────────────┬──────────────────┘
                │ sealed signalling, pairing                           │
                └────────────┬───────────────────────┬─────────────────┘
                             │  LAN: mDNS + TCP      │
                             ▼  or a server          ▼
                  ┌──────────────────────────────────────────┐
                  │ sodium-remote-server (rendezvous API v1) │  introduces, never reads
                  │ coturn (STUN / TURN relay)               │  forwards encrypted packets
                  └──────────────────────────────────────────┘
```

## The crates and components

The Rust workspace is at the repository root (`Cargo.toml`).

| Directory | Crate or target | What it is |
|---|---|---|
| `core/` | `remote-core` | The heart. Sans-IO state machines for the session (channels, FEC, NACK, pacing, congestion control, clock sync), crypto (Noise IK, packet sealing, replay window), ICE and route choice, pairing (SPAKE2), the rendezvous client, mDNS, key storage, server configuration, and the `connect` API that hosts and clients use. Also a UniFFI surface for Swift. `brand.rs` holds every public name and path |
| `client-engine/` | `remote-engine` | The stream engine every client shares: connect and pair, the session loop, decode (FFmpeg through a C shim: HEVC, H.264, AV1; NVDEC, VA-API, D3D11VA or software), the click-to-photon test, clipboard sync, layout |
| `client-linux/` | `remote-client` | The Linux platform layer: a Wayland window, EGL presentation, input, audio. Also a command-line test client |
| `client-windows/` | `client-windows` | The Windows platform layer: a Win32 window, Direct3D 11 presentation, Raw Input, WASAPI audio, clipboard. Also a command-line test client |
| `client-qt/` | `remote-qt` | The engine behind the Qt app, through a C interface. A browse client (list, presence, pairing) and the session |
| `host-desktop/` | `host-desktop`, program `sodium-remote-host` | The host for computers that aren't SodiumOS: a Windows backend (DXGI capture, hardware encoders, SendInput, WASAPI) and a macOS backend (ScreenCaptureKit, VideoToolbox, CGEvent); its UniFFI surface drives the Mac app's "This computer" page |
| `host/` | `remoted` | The SodiumOS host: the session lifecycle, virtual display, input method, audio, clipboard, and the `org.sodiumos.Remote` D-Bus API. It takes frames from the window manager's capture plug-in and encodes with NVENC. Built only where it applies |
| `wlclip/` | `remote-wlclip` | A Wayland clipboard for the Linux hosts and clients |
| `fcitx5-remote/` | C++ add-on | The input-method engine a SodiumOS host drives while a session runs |
| `relay/` | `remote-rendezvous`, program `sodium-remote-server` | The rendezvous server, and `deploy/`, the scripts and templates that set it up with coturn |
| `app/` | Qt/QML app | "Sodium Remote" for Linux and Windows, on Sodium UI (vendored in `sodium-ui/`). Its engine is `client-qt` |
| `client-mac/` | Swift app | The macOS app. Links one static library, `libremote_ffi.a`, made from `host-desktop` and `core` |
| `packaging/` | | Windows build scripts, and the Arch and Mac packaging |
| `i18n/` | | Translation catalogs for the app (`remote`) and for Sodium UI (`sodiumui`), and their tools |
| `tests/e2e/`, `tools/` | | End-to-end test beds, the VM helpers, `spdx-headers.py` |

Dependency direction: platform hosts and clients depend on `remote-core`, never the other way. The core has no windows, no sockets of its own beyond the driver, and no clock reads outside it, so it can be tested without a network and driven from Swift.

## One frame, end to end

On a host:

1. **Capture.** The platform's capture API reports what changed: DXGI Desktop Duplication on Windows, ScreenCaptureKit on macOS, the window manager's plug-in on SodiumOS. Only changed areas produce a frame; an idle desktop produces almost none.
2. **Encode.** A hardware encoder where there is one (NVENC, AMF, Quick Sync, VideoToolbox), at the bitrate the congestion controller allows this frame. Low-latency settings: no B-frames, no lookahead. When an area stops moving, extra frames sharpen it until text is exact.
3. **Session.** The frame is cut into packets, Reed-Solomon error correction is added in proportion to measured loss, and the **pacer** releases the packets at the controller's rate. Every packet is sealed with ChaCha20-Poly1305.
4. **Path.** ICE chose the route: direct on the local network, punched through NATs, or through a relay. The session doesn't know addresses, so the route can change under it.

On a client:

5. **Receive.** Packets are opened, checked against the replay window, and the frame is rebuilt from any enough of its packets. A missing packet is requested again if the round trip is short enough, otherwise the frame is dropped and the host is told so it can predict from a frame the client has.
6. **Decode and present.** Hardware decoding first, then software. The latest frame wins; the client draws the cursor itself from the position the host sends.
7. **Feedback.** The client reports arrival times, loss and decode and present times. The host's congestion controller uses them to set the next frame's rate.

Input goes the other way, over a channel that repeats unacknowledged events so a lost packet costs nothing. Audio, the cursor, the clipboard and control messages have their own channels. The channel list is in [the protocol reference](reference/protocol.md).

## Pairing and connecting

- **Pairing** runs SPAKE2 between the two computers, through the server or directly on the LAN, then each pins the other's key ([security](security.md)).
- **Connecting** on the LAN opens the LAN listener (TCP) to exchange sealed candidates; across the internet the same exchange goes through the server. ICE does the rest.
- The **server** keeps who is online (a host being online means its connection is open), each host's signed list of paired clients (the only thing that authorizes a client), and temporary pairing windows. It stores no content.

## The Qt app and Sodium UI

The Linux and Windows apps are one Qt/QML codebase on **Sodium UI**, the part of the SodiumOS design library the app needs, vendored into `sodium-ui/` so the repository builds on its own. Video never goes through Qt's scene graph: on Wayland the engine draws into a subsurface beneath Qt's window, on Windows into a child window with its own swap chain. Qt draws the controls above it. The macOS app is native Swift and shares only `remote-core` and the host code, through UniFFI.

## Where to read more

- [Protocol](reference/protocol.md), [rendezvous API](reference/rendezvous-api.md), [session API](reference/session-api.md)
- [Video pipeline](reference/video-pipeline.md) (SodiumOS host) and [desktop host](reference/host-desktop.md)
- [Design decisions](design-decisions.md)
