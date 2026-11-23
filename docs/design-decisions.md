# Design decisions

The choices that shape Sodium Remote, and what they cost. Outcomes only: each is a decision that is in the code today.

## Protocol and security

- **Its own UDP protocol, not WebRTC or a screen-sharing standard.** Media is hardware-encoded video over UDP with forward error correction, selective retransmission and frame-by-frame rate control. This keeps latency low on imperfect networks, and lets the cursor, input and audio have channels tuned for them. The price is that Sodium Remote only talks to Sodium Remote.
- **Sans-IO core.** The session, crypto and path layers are state machines that take packets and a time and return packets and events. They can be tested without a network and driven from Rust, Swift or a test harness.
- **ICE with our own route layer.** A full ICE agent that keeps checking after a pair is chosen, so the route can change live, plus TURN over UDP, TCP and TLS. On top of it, routes are scored by round-trip time, relay legs, loss and throughput, and the active route is judged faster than ICE's own timers. Media fails over in about 0.7 seconds.
- **Noise IK with pinned keys.** The client already knows the host's key from pairing, so a session starts in one round trip and a host accepts only keys it has pinned. Keys are independent of the network path.
- **Pairing by a code that never leaves the device.** A six-digit code through SPAKE2, one use, five minutes, three tries. A server can't see or brute-force it.
- **The server is untrusted.** It routes sealed messages and sees metadata. The host's signed list of paired clients, not the server's opinion, authorizes a client.
- **One key per device, for everything.** The Ed25519 device key authenticates to servers, and an X25519 key derived from it runs the handshake and sealing, so the ID you pinned is what the session proves.
- **No telemetry, no updater.** You update yourself, and the software contacts only what you configure.

## Servers and networks

- **Nothing is compiled in.** No default server and no bundled CA. A device uses a server only when its configuration or a link names one; without one it works on the LAN and by address and says so.
- **Web PKI by default, a pinned CA optionally.** A pinned CA replaces trust entirely, and is never fetched from the server it checks.
- **You can run the whole thing.** The server and its deploy scripts are open source and generic: no built-in names, ports are parameters, TLS from a public CA or a private one.
- **Enrolment by token by default.** A host registers with a short-lived token or an allowlist, so a public server isn't a free relay.
- **Additive API.** The rendezvous API only grows. Control versions are negotiated by range, and every release speaks the previous one too ([compatibility](compatibility.md)).
- **A hosted relay service is planned and separate.** The software has no billing or account system; hooks for quotas exist and are off. See [LICENSING.md](../LICENSING.md).

## Video and input

- **Send only what changed; sharpen when still.** Damage-driven frames keep an idle desktop near zero bandwidth. When an area stops moving, refinement frames bring text to exact, nearest the caret and pointer first.
- **The cursor is local.** Only its shape and position cross the network.
- **Quality gives way before smoothness.** Under a bandwidth cap or loss, the encoder lowers quality, not the frame rate.
- **HEVC 4:4:4 where the client decodes it in hardware, 4:2:0 otherwise,** with H.264 as a fallback and AV1 as an option. Codecs are probed, never assumed.
- **A SodiumOS host makes a display of the right size.** It creates a display at the client's window size and refresh rate, so nothing is scaled. Windows and macOS hosts stream their real displays and scale to fit; they don't create one.
- **Keys are USB HID usages on the wire.** The host maps them, with a per-client key table (for example ⌘ to Ctrl between a Mac and a PC), so shortcuts feel native.
- **Input uses the user's own input method** on a client that has one: the text that results is sent, not keystrokes.

## Platforms and code

- **A Qt app on Sodium UI for Linux and Windows, a native app for macOS.** The Qt app shares one codebase. The Mac app is native Swift because it must feel like a Mac app and use VideoToolbox and Metal.
- **The host is a library plus thin platform backends** for Windows and macOS. SodiumOS keeps its own deeper host (compositor plug-in, virtual display, input method).
- **Portal-based host for other Linux desktops:** the desktop's own screen-sharing portals, PipeWire, libei and the portal clipboard, so the desktop asks the user to allow it. GNOME and KDE Plasma 6 on Wayland in 0.1.
- **Keys:** Keychain on macOS, DPAPI on Windows, a file with restricted permissions (mode 0600) on Linux.
- **Every interface string is translated** into ten languages, and layouts size to their content ([translating](translating.md)).

## License

- **AGPL-3.0-only, with a commercial license** for those who can't meet the AGPL's conditions. Contributors sign a CLA that makes the commercial license possible. Documentation is CC-BY-SA-4.0. See [LICENSING.md](../LICENSING.md).
- **Builds don't link GPL-only code,** and the dependencies are permissive, apart from MPL-2.0 for the UniFFI crates in the Mac app's library.
- **The names and the app icon are trademarks** ([TRADEMARKS.md](../TRADEMARKS.md)).
