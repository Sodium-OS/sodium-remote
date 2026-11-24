# Session API

How `remoted`, the Linux reference client and the Mac client find, pair with and connect to each other through `remote-core` (see `rendezvous-api.md`). The transport below the session (ICE, Noise, channels, the driver) is `protocol.md`.

- **Modules** in `remote/core/src/`: `msg` (schemas), `keys`, `rendezvous`, `pairing`, `mdns`, `connect` (this API) and `ffi` (UniFFI, behind the `ffi` feature).
- **Why `connect` and not `session`:** `remote_core::session` is the protocol state machine. The API that ties pairing, signalling and ICE together is `remote_core::connect`.
- **Try it:** `cargo run -p remote-core --example connect_demo -- host|client …` drives every step (section 8).

## 1. Schemas (`msg`)

Everything above the transport is in `remote_core::msg`, encoded with serde + `postcard` (compact binary, varints):

| Where | Type | Delivery |
|---|---|---|
| Control channel, kind `msg::CONTROL_KIND` (1) | `Msg` | reliable, ordered |
| Input channel, one event each | `InputEvent` | unreliable, redundant, in order |
| Cursor shape (`Command::CursorShape`, control kind 250 on the wire) | `CursorImage` | reliable, once per hash |
| Cursor position (cursor channel) | `f32 x, y` = the hotspot in stream pixels | latest wins |
| Sealed signalling blobs (rendezvous `signal`, LAN listener) | `Signal` (`Offer`, `Busy`, `Answer`, `Candidate`, `Refused`) | sealed with `keys::seal` |

- **Versioning:** `Hello.version`, `Welcome.version` and `Offer.version` carry `msg::VERSION` (4). New messages are new enum variants at the end; an older peer skips what it can't decode (`msg::decode` → `None`). Changing an existing variant is a version bump. A host that can't serve the client's version answers `Refused`/`Disconnect(VersionMismatch)`: the client's version-mismatch card.
- **Units:** stream pixels are REMOTE-1's buffer pixels, top-left origin. Refresh rates are mHz (60 Hz = 60000).
- **Keys** travel as USB HID usages (page 7), already mapped by the client's table. Buttons are named (`Left`…`Forward`, `Other(BTN_*)`). Scroll keeps `source`, `value120` and `stop` (capture-ipc's INPUT_AXIS).
- **IME rides the input channel:** `InputEvent::ImePreedit` and `ImeCommit` travel with the keys, so a commit can never overtake, or fall behind, a key typed next to it (Enter after 你好). Keep each under `INPUT_TEXT_MAX` (512 bytes); `InputEvent::commit_chunks` splits a longer one. `Caret`, `FocusChanged { password }` and `ImeReset` come back on the control channel.
- **Clipboard:** `ClipboardOffer { serial, items, concealed }`. Items up to 64 KiB come inline; bigger ones on `ClipboardRequest` → `ClipboardData` chunks of at most 64 KiB (`ClipboardData::chunks`). `concealed` = put it on the pasteboard with `org.nspasteboard.ConcealedType` and never store it.

### Session setup

```
client                                         host
  (Offer, sealed, via rendezvous or LAN)  →
                                          ←  Busy{by} | Answer | Refused     (takeover rules, §6)
  ICE, Noise IK (pinned keys)             ↔
  Hello{version, name, display, decode, settings}  →   (sent by the core, on every (re)connect)
                                          ←  Welcome{version, name, format, scales, home_monitor,
                                                     locked, took_over_from, resumed}
  Resize / Settings / HomeMonitor         →
                                          ←  Format (after any change; the next frame is a keyframe)
                                          ←  Caret, FocusChanged, ImeReset, PointerLock, Locked,
                                             HomeMonitor, TookOver (silent takeover only)
  ClipboardOffer / Request / Data         ↔
  Stats                                   ↔  (about once a second)
  Disconnect(reason)                      ↔
```

- `Resize`: the client sends every change; the host debounces (150 ms after the last). With "Fixed resolution" the client doesn't send it.
- `UiSize::MatchMac` snaps `display.pixels_per_point` to the nearest valid scale by ratio, ties to the smaller UI. Larger and Smaller step along `Welcome.scales`.
- `StreamSettings.bandwidth_limit_kbps` caps the congestion controller (the core applies it from `Hello` and every `Settings`; 0 = Auto).

## 2. Identity and storage (`keys`)

- **Device key:** Ed25519 (`DeviceKey`). The device ID (`DeviceId`) is the 32-byte public key, shown as base64url without padding (43 characters), as on the rendezvous.
- **One key for everything:** the X25519 key for Noise and for sealing is derived from the device key (`DeviceKey::noise_keypair()`, RFC 7748's map, as libsodium's `ed25519_sk_to_curve25519`). So the ID pinned at pairing also pins the Noise IK handshake and the signalling seals.
- **Sealing** (`keys::seal` / `keys::open`): Noise one-way `K` (`Noise_K_25519_ChaChaPoly_BLAKE2s`), both static keys known, the rendezvous session ID (or, on the LAN, `lan v2 ` and the host's per-connection nonce) in the prologue. Only the pinned peer can open a blob, it can only have come from the pinned sender, and it opens only in its own session.
- **Storage is pluggable:** `keys::Store` holds named blobs.
  - `FileStore` (hosts): a directory, files 0600, written by rename. `FileStore::host_default()` is `$XDG_DATA_HOME/sodium-remote/host` on Linux (default `~/.local/share/sodium-remote/host`) and `%LOCALAPPDATA%\SodiumRemote\host` on Windows.
    - `device.key`: **the contract file**, an Ed25519 private key in PKCS#8 PEM (what `openssl genpkey -algorithm ed25519` writes). Checked: `deploy.sh host-id` and `allow-host` read a key written by `keys` and derive the same ID.
    - `paired.json`: `{"seq": n, "devices": [{"id", "name", "paired_at", "last_connected"}]}`. On a host, `seq` is the last paired-list sequence uploaded.
  - `MemoryStore` for tests; the Mac implements `ffi::KeyStore` in Swift (the Keychain).
  - `keys::load_or_create_device_key(&store)` creates the key on first use.

## 3. Rendezvous client (`rendezvous`)

`Rendezvous::connect(&cfg, &key, role)` opens `cfg.url` (from `server.toml` or a link: `server_config`; nothing is compiled in) with TLS that trusts the platform's roots (Web PKI), or **only** the pinned CA when `cfg.ca_pem` is set. It sends `cfg.token` when the device registers, follows one `redirect` only to the configured host or a subdomain of it, answers the challenge and returns a cloneable handle, an event receiver and `registered`.

- Every request of rendezvous-api.md §5 is a typed `async fn` (`upload_paired_list`, `paired_list_seq`, `pair_open/msg/fail/close`, `signal`, `close_session`, `turn_credentials`, `unregister`, `presence`, `connect_host`, `pair_discover`, `pair_join`); errors carry the server's code (`RdvError::code()`).
- `TurnCredentials::resolve()` turns the URLs into STUN servers and TURN servers (UDP, TCP, TLS) for the path layer; `rendezvous::tls_config` is the pinned config for TURN over TLS too.
- **Sleep notice:** `sleep_notice()` sends `{"t":"sleep"}` and closes. The live server doesn't know `sleep` (it answers `bad_request`, which is tolerated), so the card shows Offline with `last_seen` until the relay adds an "asleep" state (open question 1).
- Reconnecting is the `connect` layer's job: it keeps one connection up with backoff (1 s doubling to 30 s; 30 s after `host_not_allowed`).

## 4. Pairing (`pairing`)

SPAKE2 (RustCrypto `spake2` 0.4, Ed25519 group) with the 6-digit code. The code never leaves the client, not even hashed; only SPAKE2 messages travel, as opaque blobs (`pair_msg` on the rendezvous, `LanFrame::Pair` on the LAN).

```
C → H  M1  spake_A                                              A = "remote client", B = the host's ID
H → C  M2  spake_B, host ID, host name, confirm_H = HMAC(K_h, T)
C → H  M3  ChaCha20-Poly1305(K_c; client ID, client name; aad T)
H → C  M4  HMAC(K_d, T ‖ client ID)
```

- `T` is SHA-256 over both SPAKE2 messages and the host's ID. Keys come from SPAKE2's output with distinct labels.
- A relay (or LAN neighbour) that lies about the host's ID makes the keys disagree: it looks like a wrong code.
- **Window** (`HostWindow`): the code, 5 minutes, 3 attempts (one left half-way counts, as on the server), one use. The rendezvous enforces the same as a backstop. A host's window is open on both paths at once (rendezvous and LAN), with one attempt at a time.
- **By a bare address** (`Client::pair_by_address`, M1 `v: 2`; the review): a client that has only `IP[:PORT]` (default 47311, `connect::parse_address`) doesn't know the host's ID, so B's SPAKE2 identity is the fixed string `"remote host"`. The host answers either version. The ID then comes in M2 and goes into `T` and into `confirm_H = HMAC(K_h, T)`, where `K_h` comes from SPAKE2's output, which needs the code. The client pins the ID of an M2 only after `confirm_H` verifies, so the ID it stores is one the code vouched for: a man in the middle without the code, or a host that swaps the ID in M2, fails like a wrong code. Nothing from an unauthenticated hello is ever stored. Later connections authenticate that stored ID through the Noise handshake and the signalling seals. Someone who knows the code can still pose as the host during that window, as with a known ID; the code is the secret.
- **Connecting by a bare address** (`Client::connect_by_address`, `host_at`): the paired host whose stored `lan_addr` equals the address (or the only paired host that has none) is connected to, using the stored ID. `PairedDevice.lan_addr` (`IP:PORT`, optional in `paired.json`) is stored when a pairing runs over the LAN, and seeds the client's LAN hints at start.
- **After success** both sides store each other in `paired.json`; the host uploads its signed list with `seq` = max(stored, server's) + 1 (and again on every rendezvous reconnect).

## 5. LAN (`mdns`)

- **Discovery:** `_sodium-remote._tcp.local.` (mdns-sd). Instance: the host's display name plus the start of its ID. TXT: `v=1`, `id`, `name`, `pair=1|0`. The SRV port is the host's **LAN listener**.
- **The LAN listener (TCP)** carries what the rendezvous would, so a LAN pairing or connection needs no server: frames are `u32` big-endian length + postcard `LanFrame`. A connection starts with `PairHello` (one pairing attempt) or `SignalHelloV2 { client }` (one signalling session; the host only answers paired IDs, and the blobs are sealed by that ID anyway). The host answers it with `SignalNonce { nonce }`, 16 random bytes, before any signal, and both sides seal that connection's blobs with the nonce in the context, so a blob recorded on one connection doesn't open on another. The first version's `SignalHello`, which used one context for every connection, is refused.
- The client keeps where each host was last seen on the LAN and uses it when mDNS has lost the host for a moment (a dropped link). `ClientConfig::lan_hints` adds known addresses (tests, or the Mac's own Bonjour browser).

## 6. The session API (`connect`)

All tokio. Methods take `&self` and never block; events come on an `mpsc::UnboundedReceiver`. One "brain" task per side owns the state; hot paths (video, cursor, input) go straight to the current connection's driver.

### Host

```rust
let mut cfg = HostConfig::new(Arc::new(FileStore::host_default()?), "my-linux-pc");
// cfg.rendezvous = None (LAN only); cfg.lan / cfg.mdns; cfg.hold (120 s); cfg.bind (default 0.0.0.0: every
// IPv4 interface, re-gathered on a network change); cfg.relay_only (tests; hides addresses)
let (host, mut events) = Host::start(cfg).await?;
host.id();                                   // DeviceId, for Settings › Remote ("not enrolled" shows it)
host.open_pairing().await;                   // Some(PairingWindow { code: "123456", expires_in: 300 s })
host.close_pairing();
host.paired();  host.remove_paired(id).await;   // revoke: new signed list at once, disconnects it
host.send(&Msg::Welcome(..));                // control to the current client (false if none)
host.encoder_bitrate_bps();  host.link_status();   // budget, C_est, split rule, ref hints, health (protocol.md 8.4)
host.send_video(frame);  host.send_video_with(frame, FrameOpts { .. });  host.set_animation(true);
host.send_cursor_shape(hash, CursorImage { .. });   // remembered: re-sent to every new client
host.send_cursor_position(x, y, visible, hash);
host.send_audio(ts, capture_us, opus);  host.mic_buffer();   // jitter buffer: pop 10 ms at a time
host.disconnect(DisconnectReason::User);
host.sleep_notice().await;  host.wake_notice();     // logind PrepareForSleep(true / false)
host.network_changed();  host.shutdown().await;
```

| `HostEvent` | When |
|---|---|
| `Rendezvous { online, error }` | Connected or dropped. `error` containing `host_not_allowed` is the "not enrolled" state (`deploy.sh allow-host`). |
| `PairingAttempt { attempt }`, `PairingFailed { attempts_left }`, `Paired(PairedDevice)`, `PairingClosed { reason }` | The pairing window. `reason`: `paired`, `expired`, `attempts_used`, `cancelled`. |
| `ClientConnected { client, name, hello, took_over_from, resumed }` | Handshake done and `Hello` received: set REMOTE-1 up and answer `Msg::Welcome` (copy `took_over_from` and `resumed`). |
| `ClientLost { client }` | The connection died: **the hold starts**. Keep REMOTE-1 and the layout. |
| `HoldExpired { client }` | Nobody came back in `hold` (2 minutes): put the windows back, remove REMOTE-1. |
| `ClientDisconnected { client, reason }` | A clean end, no hold: `User`, `TakenOver { by }`, `Revoked`, … |
| `Input(InputEvent)`, `Control(Msg)` | From the current client |
| `PeerLostFrame { frame_id }`, `ReceiverReport { .. }` | Reference invalidation, the HUD |
| `Route(RouteInfo)`, `Stats(SessionStats)` | Path changes (`PathKind`: Lan, Direct, Vpn, Tailscale, Relay{Udp/Tcp/Tls}); once a second |

**Takeover and hold**:
- **Another device while a client is connected right now** (or connecting): the offer gets `Busy { by }` unless it says `takeover`. The client asks "In use by X. Take over?" and offers again with `takeover`. The old client gets `TakenOver { by }` and `Disconnect(TakenOver)`; the host emits `ClientDisconnected` and then `ClientConnected { took_over_from }`. The current session runs on until the new connection's client has proved itself (its `Hello`, under the session keys), so an offer that never gets that far ends nothing.
- **A held session** (lost, within the hold) is taken over silently: the new client gets `Welcome.took_over_from` and `Msg::TookOver` (the "Took over from X" toast). No `ClientDisconnected` for the lost one.
- **The same device coming back within the hold** resumes: `ClientConnected { resumed: true }`, no question.
- **Lost** means no packet at all for 4 s (core-network's hedging and failover handle shorter cuts inside the session), or the session's own idle timeout or key expiry. Both sides then start over with fresh signalling, ICE and handshake.

### Client

```rust
let mut cfg = ClientConfig::new(store, "MacBook Air");     // cfg.rendezvous, cfg.mdns, cfg.hold, cfg.lan_hints
let (client, mut events) = Client::start(cfg).await?;
let targets = client.discover_pairing(Duration::from_secs(2)).await;   // mDNS and the rendezvous, merged by host
client.pair(&targets[0], "123456").await?;                // PairedDevice | PairError::{WrongCode, Closed, Failed}
client.paired();  client.forget(&id);  client.presence().await;        // online, on_lan, last_seen, asleep_since
client.connect(host_id, hello, false).await?;             // ConnectError::Busy { by } → ask → connect(.., true)
client.send(Msg::Resize(..));                              // also updates the Hello used on reconnect
client.send_input(&InputEvent::Key { usage: 0x04, down: true });
client.send_receiver_report(frame_id, decode_us, present_us);
client.audio_buffer();  client.send_mic(ts, None, opus);  client.link_status();  client.network_changed();
client.disconnect();
```

| `ClientEvent` | When |
|---|---|
| `Connected { route }` | Handshake done; the core has sent `Hello`. `Welcome` follows as `Control`. |
| `Control(Msg)` | `Welcome`, `Format`, `Caret`, `FocusChanged`, `TookOver`, … |
| `VideoFrame(ReceivedFrame)`, `VideoFrameLost { frame_id }` | |
| `CursorShape { hash, image }`, `CursorPosition { x, y, visible, shape }` | |
| `Route(RouteInfo)`, `Stats(SessionStats)` | |
| `Reconnecting { since }` | The connection died; retrying every 2 s. The wording follows design.md 7 from `since` (and `link_status()` for the first seconds). |
| `Disconnected { reason }` | Final: `User`, `TakenOver { by }`, `Revoked`, `HostShutdown`, `HostSleeping`, `HoldExpired` (gave up after the hold), `VersionMismatch`, … |
| `Rendezvous { online, error }`, `Presence { host, online, last_seen, asleep_since }`, `Revoked { host }` | From the rendezvous |

**Which way it connects:** through the rendezvous when it is up and the host is online there (with STUN and TURN credentials, so every candidate type); otherwise through the host's LAN listener (host candidates only). ICE then picks the path; Tailscale and VPNs are just more candidates.

### Messages added in msg v2

`msg::VERSION` is **2**: a v1 peer gets `VersionMismatch`. What changed, for the Mac client and anything else on this API:

| Item | What |
|---|---|
| `ClientDisplay.display_width/height` | The size the stream would have in full screen: the display's pixels for Match window or Match display, **the fixed size itself** for a fixed or custom resolution. The host takes the UI scale list from it, picks the scale, then rounds REMOTE-1 down to the scale's pixel period; `Welcome.scales` is that list. |
| `InputEvent::Axis.more` | The next Axis belongs to the same pointer frame (capture-ipc's bit 1). |
| Input event time | The input channel carries `(t_us, event)` (`InputEvent::encode_at` / `decode_at`), `t_us` on the client's session clock. `Client::send_input` stamps "now"; `Client::send_input_at(&ev, Instant)` (FFI: `sendInputAt(event:ageUs:)`) the platform's event time. The host gets `HostEvent::Input { event, at: Option<Instant> }` on its own clock (None before clock sync) and passes it to the plugin as `t_ns`. |
| `Msg::KeyTable(Vec<KeyRule>)` | Client → host, after every (re)connect and every edit: `Client::send(Msg::KeyTable(..))` is remembered and sent again after `Hello` on each reconnect. Keys travel exactly as pressed (Cmd = Left/Right GUI, Option = Alt, Globe = `msg::USAGE_GLOBE` 0x0100); the host applies the table with the focused app. `KeyRule { when: Any / Terminal / Class(id), from: KeyChord { mods, key }, to: Vec<KeyChord> }`, `msg::mods` bits (CTRL 1, SHIFT 2, ALT 4, GUI 8, GLOBE 16), `KeyRule::mac_defaults()` is the Mac-to-PC table. An empty table is PC passthrough. |
| `Msg::KeyframeRequest` | Client → host: the decoder was reset or is missing references; the next frame is an IDR. |
| `Msg::Vsync(VsyncReport)` | Client → host when 10 or more frames were shown since the last: `vsync_us` (a presentation time on the client's session clock), `period_ns`, and each shown frame's lead before its vsync (`lead_p50_us`, `lead_p05_us`, `frames`). Build it with `remote_core::phase::LeadStats::add(ready_us, presented_us, period_ns)` and `report()`; the host's `phase::PhaseLock` turns it into FRAME_CLOCK. |
| `DisconnectReason::EndedAtComputer` | "Ended at the computer" (the password typed on the privacy screen). |
| `DisconnectReason::HostLocked` (JSON kind `host_locked`) | "%1 is locked": the computer's screen is locked (generic Linux on GNOME: the session ended, or a new one was refused). Consent and the restore token stay. |
| `HostStats.readable_ms / lossless_ms` | The last refined change: its frame → readable (luma bar) and → lossless (every plane), ms. |
| `DeviceId::code()` | `XXXX-XXXX`: 40 bits of SHA-256 over the public key in Crockford base32; `code_matches` compares case-insensitively with the hyphen optional. |
| `ClientEvent::Reconnecting { since }` | Counts from the last packet from the host, not from the loss declared 4 s later. |
| Network change | On `Notice::NetworkChanged` the client opens a new signalling session (the rendezvous, else the host's LAN listener) and sends every local candidate on it; the host accepts a signalling session whose first message is a `Candidate` for the client it serves. Measured in namespaces: video back within 1.1 s after the client's address changed with no route left. |
| `Host::send_video_with` | Returns the frame's id in this connection's session (`Option<u32>`; ids start at 0 on every new connection), which `RefHint::Reference` and `PeerLostFrame` name. |
| `Host::session_epoch`, `Host::peer_instant(client_us)`, `Client::session_epoch` | The session clock's origin, and a client time as a host `Instant`. |
| `Host::set_route_demand(bps)` | The throughput route choice plans for, kept for the session's later connections (: 0.2 bit/pixel/frame at the mode, or the user's cap). |
| `Host::open_pairing_with(code)`, `Host::rename_paired(id, name)` | remoted shows its own code and renames devices in core's `paired.json` (core owns that file). |
| `Host::set_name(name)`, `Host::set_mdns(on)` | Settings › Remote's Name and "Visible on this network", live: the mDNS announcement is re-announced under the new instance name (`Advertiser::rename`: the old one is withdrawn first), or withdrawn and started again with the LAN listener's port. The next pairing window carries the new name; one that is open keeps the name it was opened with. |
| `EndpointHandle::epoch`, `Session::epoch` | The session clock's origin, for the above. |

### Messages added in msg v3

`msg::VERSION` is **3**: a v2 peer gets `VersionMismatch` (every side is built from one tree). What changed:

| Item | What |
|---|---|
| `CodecChoice::Hevc420` | The client's "HEVC" segment: HEVC 4:2:0 whatever `Hello.decode` says (about 1.5× fewer bytes than 4:4:4 on a page flip). It rides in `Msg::Settings`, so it applies live: remoted's `wanted_codec` honours it, `ensure_media` restarts the encoder and a new `Format` follows. `Hello.decode.hevc_444` always tells the truth. Appended after `Av1`. |
| `ClientDisplay.pixels_per_point` | For the Mac: its backing scale (2.0 on every Retina Mac), because it streams the window's backing pixels. Match Mac is then exactly 2.0. |
| `Welcome.scales`, `display::scale::valid_scales` | Keeps only scales whose pixel period is 8 or less: at 2940 × 1848 Smaller is 1.75 and Larger 2.333; the e2e's `--ppp 1.741` at 2560 × 1664 still gets 1.6. |
| `Client::set_name(name)` | The name this device shows (pairing, the connection `Offer`, every `Hello`), live. The paired list on the computer keeps the name it was paired under. |
| `Client::set_relay_only(on)` | Relay-only candidate sharing, **from the next connection** (the filter is read when a connection starts; a reconnect counts). `ClientConfig::relay_only` is the start value. |

## 7. Swift (`ffi`)

Built with `--features ffi`; `remote/core/build-swift.sh`:
- `build-swift.sh bindings <dir>` writes `RemoteCore.swift`, `RemoteCoreFFI.h` and `RemoteCoreFFI.modulemap` (UniFFI 0.32, library mode; works on Linux).
- `build-swift.sh check-mac` runs `cargo check --target aarch64-apple-darwin` (off the Mac, ring's C is stubbed: check never links).
- `build-swift.sh mac <dir>` (on the Mac) builds `libremote_core.a` for Xcode plus the bindings.

The surface (Swift names):

```swift
let core = try RemoteClient(settings: ClientSettings(name: "MacBook Air", rendezvous: true,
                                                      rendezvousUrl: "", mdns: true, fps: 60),
                            store: KeychainStore(),        // KeyStore: load/save/remove(name)
                            listener: self)                // ClientListener: onEvent(event: RemoteEvent)
core.id(); core.paired(); try core.forget(host:)
let hosts = await core.discoverPairing(timeoutMs: 2000)    // [PairingHost]
let host = try await core.pair(host: hosts[0], code: "123456")   // RemoteError.wrongCode, .pairingClosed
await core.presence()                                      // [HostPresence]
try await core.connect(host: id, hello: Hello(...), takeover: false)   // RemoteError.busy(by:)
core.sendControl(msg: .resize(ClientDisplay(...)))
core.sendInput(event: .imeCommit(text: "你好"))
core.sendReceiverReport(frameId:decodeUs:presentUs:)
core.popAudio()                                            // AudioFrame? (.packet / .fec / .conceal)
core.routes(); core.networkChanged(); core.disconnect(); core.shutdown()
```

- `RemoteEvent` mirrors `ClientEvent`: `.connected`, `.control(msg:)`, `.videoFrame(frame:)`, `.cursorShape`, `.cursorPosition`, `.routeChanged`, `.stats`, `.reconnecting(sinceMs:)`, `.disconnected(reason:)`, `.presence`, `.revoked`, `.rendezvous`.

**Added for the Mac client** (`remote/client-mac`, all additive):

| Swift | What |
|---|---|
| `LeadStats()` with `add(readyUs:presentedUs:periodNs:)`, `report() -> VsyncReport?`, `reset()` | `phase::LeadStats` for `Msg::Vsync`. The Mac passes the display link update's `targetTimestamp` (the drawing deadline) as `presentedUs`. |
| `core.sessionClockUs() -> UInt64?` | Now on the live session's clock: one read maps `CACurrentMediaTime` onto it. |
| `core.link() -> LinkInfo?` | `route`, `hedgeRoute`, `hedging`, `healthy`, `silenceMs` (t₀ of), `noRouteMs`. Route kinds are cached, so it can be polled every 100 ms. |
| `ClientSettings.lanHints: [String]` | `ID@IP:PORT` LAN listeners (`ClientConfig::lan_hints`), for when mDNS can't run on the Mac. |
| `ClientSettings.relayOnly` | `ClientConfig::relay_only`, the start value. |
| `core.setName(name:)`, `core.setRelayOnly(on:)` | `Client::set_name`, `Client::set_relay_only` (msg v3 above). |
| `VideoFrame.sendUs` | The host's send time on the host's clock, to map the frame meta's host stamps. |
| `LinkStats.bytesReceived` | For the idle-bandwidth measurement. |
| `protocolVersion()`, `usageGlobe()`, `macDefaultKeyTable()`, `deviceCode(id:)`, `deviceMatches(id:typed:)`, `imeCommitChunks(text:)` | `msg::VERSION`, `msg::USAGE_GLOBE`, `KeyRule::mac_defaults()`, `DeviceId::code()` and `code_matches`, `InputEvent::commit_chunks`. |
- `onEvent` runs on a Rust thread (video frames too): hop to your own queue.
- Every `msg` type is generated as a Swift struct or enum (`Msg`, `InputEvent`, `Hello`, `Welcome`, `ClipboardOffer`, `CursorImage`, `DisconnectReason`, …).
- The `KeyStore` keeps `device.key` (PKCS#8 PEM bytes) in the Keychain and `paired.json` anywhere.

## 8. Verifying

`connect_demo` plays a test host and a test client.
- **Live, against a deployed server:** a throwaway host key enrolled on the server; the client found the window through the rendezvous, failed one wrong code, paired, connected (direct on this machine), and exchanged Welcome, Caret, cursor, input (keys, 你好, pointer), clipboard and 240 video frames. With `--relay-only` both sides: media through coturn on UDP (TCP and TLS relays also live), 180 frames at 2 Mbit/s. Takeover: Busy, then takeover, the first client told. The sleep notice: online before, offline with `last_seen` after. Afterwards all test devices were unregistered and the host withdrawn. `relay/tests/session.rs` repeats the pairing and session part against a local server with a pinned test CA.
- **Network namespaces (no server):** mDNS discovery, LAN pairing (wrong code first), LAN connect, a 6 s link cut resumed within the hold, Busy then takeover, and a 15 s cut against a 10 s hold expiring on both sides.
- **Tests:** `cargo test -p remote-core` (msg, keys, pairing; `tests/connect.rs` pairs, connects and takes over on loopback).

## 9. Not done yet

- **Trickle after a network change:** done in the (above). The host's own network change still relies on in-band candidates (a client can't be reached through the rendezvous); 4 s of silence leads to a fresh dial.
- **Clipboard interleaving:** chunks are 64 KiB, but the control ARQ is first-in first-out, so a large image still delays later control messages behind it.
- **IPv6** candidates (the driver binds IPv4), and mDNS on macOS (mdns-sd shares port 5353 with mDNSResponder; untested there; `lan_hints` is the fallback).
- **The Mac build** itself: only type-checked for `aarch64-apple-darwin`. The client is written (`remote/client-mac`, BUILD.md) and waits for the Mac.
- **A pinned-key mismatch** can't happen in a handshake: the device ID is the key, so a reinstalled host has a new ID. There is no `ConnectError::KeyMismatch`; the Mac's "key changed" card (MC10) waits for M2's pairing discovery.

## Open questions

1. **Done:** the relay has the `sleep` request and the `asleep` presence state (585a875). Core reads it as `HostState::Asleep` with `asleep_since`, shown in `HostStatus`, `ClientEvent::Presence` and the FFI `HostPresence` / `RemoteEvent.presence`. `sleep_notice` no longer tolerates `bad_request`.
2. **Decided: `_sodium-remote._tcp`.** DNS-SD names the transport of the advertised port, and the LAN listener is TCP. Media still goes over UDP through ICE.
