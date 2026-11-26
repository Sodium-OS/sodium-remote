# Protocol (wire v3, control schema v6)

The protocol between `remoted` and a client, implemented in `remote/core` (the `remote-core` crate). It covers the ICE path, the Noise session and every channel over it. This file also records the ICE library choice and the network-emulator results.

- **Changing it:** change the wire format only by bumping the prologue's version (section 3.2) and noting the change at the bottom.
- **Versions (from v6):** every release speaks control schema N and N−1. The client sends the range it speaks (`min_version`..=`version`) in its `Offer` and again in `Hello`; the host takes the highest version both speak (`msg::negotiate`) and puts it in `Welcome.version`. With no common version it refuses with `VersionMismatch { host, client }`, where `host > client` means "update this device" and anything else "update the computer" (`DisconnectReason::update_side`). Details under v6 in Changes.
- **Byte order:** big-endian everywhere. Varints are unsigned LEB128.
- **Code map:** `path.rs` (candidates, ICE, TURN, route choice), `crypto.rs` (Noise IK, packet AEAD, replay window, rekey), `session.rs` (all channels), `video.rs` (packetizer, FEC, NACK), `pacer.rs`, `feedback.rs`, `cc.rs`, `clock.rs`, `input.rs`, `reliable.rs`, `mtu.rs`, `alloc.rs` (utilisation by content, section 7.1), `health.rs` (liveness and hedging, section 7.2), `train.rs` (probe trains), `audio.rs` (jitter buffer), `driver.rs` (the tokio driver).

## 1. Layers

```
 app (remoted / client)   frames, input, control, cursor, audio            Session API (sans-IO)
 session                  channels, pacing, FEC, NACK, feedback, CC, clock  session.rs
 crypto                   Noise IK → ChaCha20-Poly1305 + counter + replay   crypto.rs
 path                     ICE (is), STUN, TURN UDP/TCP/TLS, route choice    path.rs (sans-IO)
 driver                   UDP sockets, TCP streams, timers, channels        driver.rs (tokio)
```

- **Sans-IO:** everything except `driver.rs` is a state machine. It takes datagrams and an `Instant`, and returns datagrams, events and its next deadline (`handle_datagram` / `handle_timeout` / `poll_transmit` / `poll_event` / `poll_timeout`). After every call in, the caller drains `poll_transmit`.
- **Path-agnostic session:** the session never sees addresses. Packets are authenticated end to end and bound to no address, so the path can change under a running session with no new handshake and no keyframe. The session is only told `path_changed()`, which restarts MTU probing and the delay baseline.
- **For UniFFI later:** the public surface is plain data (`Vec<u8>`, numbers, small enums). The Swift wrapper will own the clock and pass `Instant`s in; nothing in the core reads a clock or spawns a thread.

## 2. ICE choice

### 2.1 Candidates evaluated

Each was read at the source level (crates.io sources; libjuice through the C sources vendored in `libjuice-sys` 0.9.7).

| | webrtc-rs `webrtc-ice` 0.17.2 | webrtc-rs `rtc-ice` 0.21.1 | str0m's `is` 0.12.0 | `rice-proto` 0.4.3 (librice) | libjuice 1.7.3 (C, FFI) |
|---|---|---|---|---|---|
| Full agent (RFC 8445) | yes | yes | **yes** (lite is an option, off by default) | yes | "simplified" full agent, one component |
| TURN allocations | UDP only (TCP/TLS are a commented-out TODO in `agent_gather.rs`) | external (`rtc-turn`) | external: we pair it with `turn-client-proto` (UDP, TCP, TLS) | built in: UDP, TCP, TLS | UDP only ("Only UDP is supported as transport protocol") |
| Checks after nomination; live switch | no: nomination is final (pion's design) | no renomination | **yes: "nomination is not final"**. Every pair is rechecked at least every `max_rto`, and `NominatedSend` is re-emitted when a better pair succeeds | no: the checklist goes `Completed` and stops | no (keepalives on the selected pair every 15 s) |
| Consent freshness | keepalives only | yes (RFC 7675) | continuous checks on all pairs; a dead pair is dropped only after its retransmits run out (and is kept 15 s after the peer's last check), so we add our own liveness (2.3) | not implemented (RFC 7675 unchecked in its own docs) | 15 s STUN keepalive |
| Sans-IO | no (tokio inside) | yes | **yes** | yes | no (its own thread and sockets) |
| License | MIT/Apache | MIT/Apache | MIT/Apache | MIT/Apache | LGPL-2.1 |

### 2.2 Decision: `is` + `turn-client-proto`, with our own route layer

- **`is` meets the hardest requirements:** a full agent, sans-IO, and renomination without an ICE restart. These are the ones nobody else has.
- **TURN:** `is` leaves TURN to the caller, and `turn-client-proto` (sans-IO, MIT/Apache, the TURN half of librice) supplies UDP, TCP and TLS-over-TCP allocations.
- **Server-reflexive candidates** come from a 30-line STUN binding client in `path.rs`.
- **Rejected:**
  - `rice-proto` has the best TURN support but stops checking once a pair is nominated, so it can't switch paths live.
  - `webrtc-ice` and libjuice can't do TURN over TCP or TLS, which is the fallback behind restrictive firewalls.
  - `rtc-ice` has no renomination.
  - libjuice is also LGPL and not sans-IO.

**Upstream bugs found and worked around** (worth reporting upstream; neither affects UDP):
1. **`turn-client-rustls` 0.1.2** calls `read_tls` once per received chunk and drops whatever rustls didn't take. It also reads at most 2048 bytes of plaintext per call. Large TCP reads therefore corrupt the stream (`DecryptError`). We don't use the crate: `path.rs` wraps its own `rustls::ClientConnection` around `TurnClientTcp`, with complete read and flush loops.
2. **`turn-types` 0.7.3** neither pads ChannelData to 4 bytes over TCP/TLS nor skips the server's padding (RFC 8656 section 12.5). The stream desyncs as soon as a channel is bound. We bind channels only on UDP relays; stream relays use Send/Data indications, which are 4-aligned (36 bytes of overhead instead of 4, on the last-resort path only).

### 2.3 What `path.rs` adds on top of ICE

- **Routes.** Every check that gets an integrity-checked answer proves a route (local base, remote address). The answer is timed, so every route has an RTT and a last-heard time. Our transaction ids are matched to their responses.
- **Liveness (consent).**
  - ICE rechecks every pair at least every 500 ms (`max_rto`, `initial_rto` 100 ms, `Ta` 20 ms).
  - A route that hasn't been answered for 1.5 s is dead.
  - **The active route** is judged faster by the session (section 7.2): suspect at 200 ms without an answer, dead at 700 ms. Measured failover is now 0.69–0.70 s with video on the relay after about 0.2 s (section 5.4); it was 1.2 s with a 1.2 s stall.
- **Choice.**
  - Media takes the first live route at once (aggressive).
  - It moves to another live route when that route's score is at least 25% and 1 ms better.
  - **Same relay:** routes through a relay at the same address (on either leg) share its bottleneck, so throughput can't tell them apart: such a rival takes the active route's throughput penalty, and RTT, loss and class decide.
  - **Score** (lower is better), in milliseconds:
    - the route's minimum RTT over the last 10 s;
    - a penalty for each relay leg, ours and the peer's: UDP relay +3, TCP relay +25, TLS relay +30;
    - **loss:** 200 × (the loss rate over the last 20 s − 2%), once 20 samples exist: each 1% above 2% costs 2 ms. Below 2% FEC absorbs it (as in section 3.8), and a brief outage that loses a check or two doesn't make a route look lossy. Samples are ICE checks sent after the route first worked (each answered or not within 2 s; a run of lost checks counts once, and checks lost while the session suspected the route don't count: that is an outage, which liveness handles) and probe-train packets;
    - **throughput**, only on the side that sends video (the host): 16.7 × max(0, demand / capacity − 1). Demand is 50 Mbps, or the user's bandwidth cap if lower. A route at 25 Mbps costs 16.7 ms; at 10 Mbps, 67 ms; at 50 Mbps or more, nothing.
  - **Capacity** comes from two places:
    - the active route: the highest C_est (section 3.8) of the last 10 s while the controller holds one; no bottleneck means no penalty. The window matters: C_est dips while the controller recovers or shares the link, and a train elsewhere reads the link's full rate;
    - every other live route that could win: a **probe train** (`train.rs`). 12 padded 1200-byte packets go back to back on that route; the receiver times their spread and reports it (`TRAIN_REPORT`), and the bottleneck's rate is bytes after the first ÷ spread. Train packets are left out of congestion control.
    - A capacity the controller measured outranks trains for 60 s, since a train measures the link's capacity and the controller what is actually free.
    - **Trains only where a route could win, by the switching test itself:** a route is measured only if its score without a throughput penalty is at least 25 % and 1 ms under the active route's score (`reselect`'s margin and `SWITCH_MIN_GAIN`), and it isn't a TCP/TLS relay while a UDP route works. One train at a time, at least 1 s apart, each route every 30 s. In the emulator, trains to every relay route at once overflowed the client's 10 Mbps access link, which all routes share, and the controller cut on the loss.
    - **A shared access link:** a train remembers the active route's C_est when it ran. If the active route now measures more than 10% below that (or has a bottleneck where it had none), the drop may be on an access link both routes share: the train is stale. Its route can't win on throughput until a fresh train runs, and stale routes are re-measured in one burst at most every 30 s. Without this, a bandwidth drop on the client's own link moved the stream to a relay whose train predated the drop. Re-measuring on every swing of C_est instead kept trains crossing the congested link, and utilisation fell to 40%.
  - **Measured first:** while throughput matters, media leaves a working route only for a route that has a capacity measurement. Trains start as soon as a route could win, so this costs one train's round trip.
  - **Settling** (added after live runs saw relays flap against a real relay server):
    - routes rank by class: direct, through UDP relays, through a TCP or TLS relay;
    - a working direct or UDP-relay route is never left for a TCP/TLS relay; only its death moves media there;
    - a better class is taken as soon as it scores better;
    - a switch within or down a class waits until 2 s after the last switch (a guessed, provisional RTT doesn't hold it up).
  - We use the windowed minimum, not the smoothed RTT, because queueing on a shared bottleneck inflates every route at once. Comparing smoothed RTTs made the path flap during a bandwidth drop (seen and fixed in the emulator).
- **Failover (section 7.2).** The session judges the active route by its answers. On *suspect*, `on_suspect` rechecks every pair at once (`recreate_candidate_pairs`) and picks the **hedge route**: the best live route to a different remote address if there is one, else the best other route. On *dead*, `fail_selected` marks the route failed and media moves to the best live route. A failed route comes back when an ICE check on it is answered again.
- **Network changes.** With the driver bound to `0.0.0.0`, it re-reads the IPv4 interfaces every second (or on `Command::NetworkChanged`, for NWPathMonitor). A new address gets a socket, a host candidate, STUN queries and TURN allocations; a vanished one loses its candidates, routes, allocations and TURN streams (`remove_local`), which reconnect from the new address. The session never notices beyond `path_changed`.
  - **Telling the peer:** the driver emits `Notice::NetworkChanged { added, removed }`, then a `Notice::LocalCandidate` per new candidate. It also sends each candidate gathered while connected **in-band**, as reliable control kind 251 (`Session::send_candidate`). The peer's session hands it to its path layer (`PathSignal::RemoteCandidate`), which adds it like a signalled one.
  - In-band works whenever any route survives. It also worked when every route died: the client's checks from its new address reached the host's relayed candidate, because its public IP didn't change. When the public IP changes, the peer's TURN permissions don't cover it, so the app should also carry the new candidates over rendezvous or LAN signalling; `NetworkChanged` is the cue to reopen it. `Command::RemoteCandidate` accepts candidates at any time.
- **Relay transport hint.** A relayed candidate doesn't say how its owner reaches the TURN server. Ours encode it in the ICE local preference: 16000 for UDP, 8000 for TCP and 4000 for TLS, less 2 per candidate. The peer reads it back from the priority. Without this, a host could send video to a client's TLS allocation when its UDP allocation works.
- **Provisional route.** When no route exists yet, an integrity-checked binding request from the peer opens a provisional route on its 5-tuple, with a guessed RTT of 50 ms. The answering side can then send its handshake reply at once instead of waiting for its own check. The first real measurement replaces the guess.
- **Upgrades.** ICE never retries a failed pair. While no direct route is in use, `path.rs` re-forms all pairs every 5 s (`recreate_candidate_pairs`). A direct path that opens later is therefore found within about 5 s. Meanwhile the current route stays up.
- **Asymmetric routes are fine:** each side chooses for its own sending direction.

## 3. Wire format

### 3.1 Demultiplexing (one UDP socket for everything)

| First byte | Meaning |
|---|---|
| 0–3 | STUN (ICE checks, STUN binding, TURN control) |
| 64–79 | TURN ChannelData (UDP relays) |
| 0xC1 | Remote handshake initiation |
| 0xC2 | Remote handshake response |
| 0xC3 | Remote data packet |

Bytes 192–255 are unused by RFC 7983's STUN, DTLS, TURN and RTP ranges.

### 3.2 Handshake: Noise IK with pinned keys

- **Pattern:** `Noise_IK_25519_ChaChaPoly_BLAKE2s`.
- **Prologue:** `"SodiumOS Remote wire v3"`. Bump it on any incompatible change.
- **Roles:** the client is the initiator and already holds the host's static key (pinned at pairing). The host is the responder and accepts only static keys it has pinned.

| Message | Layout |
|---|---|
| Initiation (client → host) | `0xC1`, `u32 sender_index`, Noise message 1 (`e, es, s, ss`; payload: `u64` unix µs timestamp) |
| Response (host → client) | `0xC2`, `u32 sender_index`, `u32 receiver_index` (= the initiator's index), Noise message 2 (`e, ee, se`; empty payload) |

- **Anti-replay:** the host keeps the newest timestamp per client key and refuses an initiation that isn't newer (WireGuard's TAI64N idea).
- **Rekey payload:** a rekey's initiation payload is the timestamp then `u32 rekey_of`, the host's receiver index of the running session. The payload is encrypted (after `es, ss`), so only the paired client can name a session.
- **Retransmit:** the client sends a fresh initiation (new ephemeral key, timestamp and index) every 400 ms until a response arrives.
- **Takeover:** a valid initiation from another paired device replaces the session (by design, a new connection from another paired device takes over). The host's state machine resets and emits `Connected` with the new key.
- **Keys are path-independent:** when ICE changes the route, the session keys stay.

### 3.3 Data packets

```
0xC3 | u32 receiver_index | u64 counter | ChaCha20-Poly1305(plaintext) + 16-byte tag
```

- **Keys:** Noise's split (snow's stateless transport). The nonce is the 64-bit counter.
- **Counters** start at 0 per direction and never repeat.
- **Replay window:** 2048 counters (a WireGuard-style bitmap ring). A packet is recorded only after its tag verifies.
- **Transport-wide sequence:** the counter doubles as the transport-wide sequence number used for congestion feedback (section 3.6). Packets are sealed when they leave the pacer, so counter order is departure order.
- **Overhead:** 29 bytes per datagram (13 of header, 16 of tag).
- **Rekeying** (WireGuard's timers):
  - The client starts a fresh IK handshake inside the session 120 s after its keys were made (or after 2^60 packets), naming the session in the payload (section 3.2). It retransmits every 400 ms and keeps sending under the old keys meanwhile.
  - The host answers but keeps sending under the old keys until the first packet under the new ones arrives (key confirmation). It then switches.
  - The client switches as soon as the response arrives.
  - **Counters carry on** across the swap, because they are the transport-wide sequence (section 3.6). Each key set has its own receiver index and replay window.
  - Old keys still open stragglers for 10 s.
  - Keys older than 180 s are never used: the session ends with `Disconnected { KeyExpired }`.
  - An initiation without `rekey_of`, or naming another session, is a new session (takeover), as before.
- **Idle timeout:** nothing authenticated from the peer for 2 minutes (the host's hold) ends the session with `Event::Disconnected { IdleTimeout }`. The host then waits for a new handshake. The client stays closed until `reconnect()`.

### 3.4 Channels (first byte of the plaintext)

| # | Channel | Delivery | Direction | Messages (after the channel byte) |
|---|---|---|---|---|
| 0 | Control | Reliable ARQ | both | `0 DATA`: `u32 seq, u8 kind, u8 flags (bit 0: more fragments), bytes`. `1 ACK`: `u32 cumulative, u64 sack` (bit i = seq cumulative+2+i received). Kinds 0–239 are the application's; 250 is a cursor shape (`u64 hash`, image); 251 is an ICE candidate (SDP string) gathered mid-session. |
| 1 | Video | Unreliable, FEC, NACK | host → client | Section 3.5 |
| 2 | Feedback | Unreliable | client → host | `0 TRANSPORT` (section 3.6), then `u32 last_good` when the client has a decodable frame. `1 NACK`: `u32 frame, u16 n, n × u16 shard index`. `2 FRAME_LOST`: `u32 frame`, then `u32 last_good` when known. `3 RECEIVER_REPORT`: `u32 frame, u32 decode_us, u32 present_us`. |
| 3 | Cursor | Unreliable, latest wins | host → client | `0 POSITION`: `u32 seq, f32 x, f32 y, u8 visible, u64 shape hash`. Shapes go once per hash on Control kind 250. |
| 4 | Input | Unreliable, redundant | client → host | `0 EVENTS`: `u32 first_seq, u16 n, n × (u16 len, bytes)`, every event not yet acknowledged. `1 ACK`: `u32 last_seq` (host → client). |
| 5 | Audio | Unreliable | host → client | `u32 seq, u32 timestamp (48 kHz), u64 capture_us (sender's session clock), Opus packet` |
| 6 | Mic | Unreliable | client → host | Same as Audio |
| 7 | Probe | Unreliable | both | `0 PING`: `u64 t0`. `1 PONG`: `u64 t0, u64 t1, u64 t2`. `2 MTU_PROBE`: `u16 size, u16 id`, zero padding to `size`. `3 MTU_ACK`: `u16 size, u16 id`. `4 TRAIN`: `u32 train, u16 index, u16 count`, zero padding to 1200 bytes. `5 TRAIN_REPORT`: `u32 train, u16 count, u16 received, u32 spread_us, u32 bytes` (bytes after the first packet). |

**Notes on the channels:**
- **Control** fragments a message larger than a datagram and delivers it whole and in order. Up to 256 segments are in flight. A segment is resent after 2× RTT (at least 20 ms), with exponential backoff.
- **Input** events are opaque bytes; their encoding (HID usages, pointer, scroll phases, IME text) belongs to the host and client. A lost input packet costs nothing: the next one repeats it. With no new input, unacknowledged events are repeated every 1.25× RTT (at least every 10 ms).
- **Liveness echo:** a PING also goes out whenever nothing the peer answers (a ping, video, input events, control data) went out for 100 ms, so a working route answers at least every ~100 ms (section 7.2).
- **Audio:** received audio and mic go through an adaptive jitter buffer (`audio.rs`): the target is the 95th percentile of delay variation over 2 s plus one frame, at least 20 ms. It grows by concealing without advancing and shrinks by skipping a frame, at most once per 200 ms. A single lost packet is rebuilt with Opus in-band FEC from the next packet (`AudioPlay::Fec`); otherwise it is concealed. `capture_us` maps to the receiver's clock through clock sync, the same clock as a video frame's `send_local_us`, for A/V sync. The host reads `send_loss()` for Opus's expected loss (which sizes its FEC).
- **Clock sync:** both sides ping, every 100 ms for the first second, then every 500 ms. The offset comes from the lowest-RTT sample of the last eight (NTP's four timestamps). A received frame carries the host's send time, already converted to the client's clock, so frame latency is measured without a shared clock.

### 3.5 Video

**Packet:** `u8 channel=1 | u32 frame_id | u16 index | u16 k | u16 m | u32 frame_len | u8 flags | shard`.
- **Header size:** 16 bytes inside the encryption, 45 bytes per packet in all (3% at a 1472-byte MTU).
- **Flags:** bit 0 keyframe, bit 1 retransmission.

- **Flags:** bit 2 reference (the payload names the frame it is predicted from), bit 3 recovery point (decoding can restart here, e.g. a completed intra refresh).

**Frame payload** (what gets sharded): `u64 send_us (host clock) | u32 reference (only with flag bit 2) | u16 meta_len | meta (damage rects and other per-frame metadata, opaque here) | bitstream`.

**Padding** (the controller's probes, section 3.8): the same 16-byte header with `k = 0`, which no frame has; `frame_id` is the probe's number, `index` and `m` its packet and count, `frame_len` and `flags` 0, then zeros to the path's full datagram. A receiver counts it in transport feedback and drops it: it never reaches frame assembly, FEC, NACK or the decoder. Not a wire change: a receiver from before padding drops it too (its decoder refuses `k = 0`), so neither the prologue nor `msg::VERSION` moves.

**Packetizer:**
- The frame is cut into `k` equal shards that fill the path MTU (even lengths, zero-padded).
- `m` Reed-Solomon recovery shards are added with `reed-solomon-simd`.
- Any `k` of the `k + m` packets rebuild the frame.
- Packets `0..k` are data, `k..k+m` recovery.

**FEC ratio** (`cc.rs`):
- **Measurement:** transport-wide loss over the last 2 s (all packets, so FEC can't hide its own need).
- **Rule:** 0 below 0.1% loss; otherwise `4 × loss + 2%`, capped at 30%. A frame gets at least one recovery shard whenever the ratio is above 0.
- **Encoder budget:** the encoder target is `target / (1 + ratio)`, so FEC sits inside the congestion-controlled rate.

**NACK:**
- **Only while RTT is under half a frame interval** (8.3 ms at 60 fps). The receiver decides from its own RTT.
- **When:**
  - Once a gap is visible (a later frame has started), or once a frame's packets stop for 3 ms, the receiver asks for exactly as many missing data shards as the frame still needs.
  - It asks again after 1.5× RTT, at most twice per frame.
- **Sender:** keeps 300 ms of packets and resends them at the head of the pacer queue.

**Delivery:**
- **In order:** frames go to the decoder in order.
- **When a frame is given up:**
  - with NACK off, as soon as a later frame is complete;
  - with NACK on, once its retransmissions are exhausted;
  - in any case, after 2 frame intervals (or 3× RTT with NACK) without progress.
- **Lost frames:** the client is told `VideoFrameLost`, and the host gets `PeerLostFrame` (sent as `FRAME_LOST`) so it can invalidate the reference.

**Reference invalidation:**
- **Receiver:** a frame is *decodable* when every frame it depends on was: a keyframe or recovery point starts a chain, a lost frame breaks it, and a frame with a `reference` is decodable if that frame was. `ReceivedFrame::decodable` says so. The newest decodable frame (`last_good`) rides on every transport feedback and on `FRAME_LOST`.
- **Sender:** after a `FRAME_LOST` for a frame at or after its last recovery frame, and when a failover starts, `take_reference_hint()` returns `Reference(last_good)` once (or `Keyframe` if nothing was decodable). The encoder invalidates every frame after it, predicts the next frame from it (a keyframe if it has left the DPB), and sends that frame with `FrameOpts { reference: Some(id) }`. Losses of frames older than that recovery frame are not acted on again.

### 3.6 Transport-wide feedback

`0 TRANSPORT`: `u16 fb_seq | u64 from | u16 count | bitmap of ceil(count/8) bytes | arrival times`.

- **Coverage:** counters `from .. from+count`, with bit i set when counter `from+i` arrived.
- **Arrival times:** the first is an absolute varint in receiver µs. Each following one is a zigzag-varint delta, in counter order.
- **Cadence:** the client sends one every frame interval while packets arrive. Each report starts where the last one ended. A packet reordered behind a sent report counts as lost.

### 3.7 Pacing

- **Spread:** each frame's packets drain over half a frame interval, at `max(1.5 × target, frame bytes / (interval / 2))`.
- **Bucket:** a leaky bucket allows a burst of 2 packets or 2 ms at the pacing rate, whichever is larger. The 2 ms covers the driver's 1 ms timer.
- **Bypass:** input, cursor, feedback, control, audio and probes never wait behind video. NACKed retransmissions jump the queue.

### 3.8 Congestion control (delay-based, GCC/SCReAMv2 family)

- **Inputs:** every feedback joins arrival times with send times. One-way delay minus its windowed minimum (two 5 s buckets) is each packet's queueing delay.
- **Signal:** the *standing* queue, which is the **minimum** queueing delay over the last frame interval. Within a frame the pacer's burst always builds a little queue. What matters is whether the queue is still there when the next frame starts. GCC's trendline would also work; this signal suits a paced, frame-structured sender and is simpler to reason about.

| State | When | Action |
|---|---|---|
| Decrease (delay) | standing queue > 6 ms, and the queue is ours: we sent at least half the target in the last 100 ms, and it didn't arrive as a step of over 6 ms between two packets unless it then rose 6 ms more | target ≤ `R × max(0.5, 0.85 − queue_s / 0.5 s)`, where `R` is the receive rate over the last 100 ms (while our queue stands, arrivals measure the bottleneck); C_est = `R`. Not loaded but rising for 100 ms (and sending at least the minimum rate): the same target cut, no C_est. Otherwise nothing |
| Decrease (loss) | loss over 500 ms > 10% | target × (1 − loss / 2), at most every 200 ms |
| Increase | standing queue < 2 ms and loss < 2% | ×1.6 per second far from the last overuse rate; +10% per second within [0.8, 1.1] of it; never above 1.5 × the 250 ms receive rate + 1 Mbps, or above the last probe the link carried (app-limited senders don't creep up: they probe) |
| Hold | otherwise | |

- **Bounds:** start 8 Mbps, floor 0.5 Mbps, cap 300 Mbps or the user's bandwidth limit (`set_max_bitrate`).
- **Per frame:** the host reads `encoder_bitrate_bps()` before every frame.
- **Path change:** a path change resets the baseline, and forgets C_est.
- **Random loss:** loss below 2% doesn't slow the stream down; FEC absorbs it.
- **App-limited:** the increase never takes the target above 1.5 × the receive rate + 1 Mbps, but that cap never pulls the target *down* (an earlier version let an idle desktop drag the target to about 1 Mbps, and the stream had to ramp from there when motion came back).
  - **Probing:** while no bottleneck is measured, the probe's rate is under any refused probe's (so a target cut on that link climbs back while idle), the target under min(demand, cap) and the link quiet, the controller sends one frame's worth of padding at 1.3 × the target, paced as a frame, every 250 ms. A standing queue under 2 ms after it takes the target to that rate; 2 to 6 ms refuses it and sets `probe_ceiling`, forgotten as `C_est` is; over 6 ms is the decrease. A real frame cuts a probe in flight. Not while hedging.
    - **As built:** the probe's rate is min(1.3 × target, demand, cap), so the last step lands on the demand. "Quiet" is no video packet left the pacer in the last frame interval, none is queued, and feedback has covered the last one sent (audio, cursor and pings don't count). The standing queue after a probe is the queue it left one frame interval after its first packet: a FIFO bottleneck is busy until its last packet leaves, so it is the latest (send time + queueing delay) of its packets less that instant. An accepted probe also needs loss under 2 % (the Increase row's test); a probe with more loss and no queue is neither accepted nor refused. A cut probe, one that was frozen by a suspect route, and one with no verdict 1 s after it started have no verdict. The decrease after a probe uses the rate its packets arrived at. The host's `SessionStats` carries `probe_bps` (the last probe's rate), `probe_ceiling_bps` and the probe counts, and `msg::HostStats` carries `target_bps`, `c_est_bps` and `probe_ceiling_bps` to the client, schema version 4.
- **C_est** (the measured bottleneck, for section 7.1 and the encoder's split rule):
  - set to the receive rate at each delay overuse that is ours (the Decrease (delay) row);
  - held through idle periods;
  - raised when more than C_est gets through with no standing queue;
  - released after 10 s with no overuse while the stream used at least half of it (the link was exercised and never queued), and on a path change. The 10 s timer only runs while the standing queue is under 2 ms (`Q_LOW_US`, implemented in the).
- **Queue-free time:** how long the standing queue has stayed under 2 ms; zero at start-up.
- **Frozen while hedging** (section 7.2): feedback then mixes two routes, so it changes nothing, and thawing restores the pre-suspect target.

### 3.9 Path MTU

- **Start:** every path starts at 1200-byte datagrams.
- **Probes:** padded `MTU_PROBE`s try 1280, 1350, 1400, 1440 and 1472 bytes. An acknowledged probe raises the size; three silent ones (each 2× RTT, at least 200 ms) end the search.
- **Restarts:** on a path change and every 10 minutes.
- **Measured:** every run in section 5 reached 1472 on direct, UDP-relay and TLS-relay paths alike.

## 4. Driver

`driver::spawn_endpoint(EndpointConfig, Session)` runs one tokio task. It:
- binds one UDP socket per local address (8 MB buffers requested; the kernel caps them at `rmem_max`);
- connects TCP streams to TURN servers;
- feeds everything to `PathAgent` and `Session`;
- holds session datagrams until a route exists;
- talks to the app over channels: `Command` in; `Notice` out (candidates, route changes, session events, stats every `stats_interval`); and a `watch` of the encoder bitrate.

## 5. Results

### 5.1 Testbed

The emulator's `netns.sh` script builds seven network namespaces with veth links, created inside the namespaces, so the host's interfaces, routes and firewall are untouched:

```
 rn-host 192.0.2.60 ─ rn-gl (NAT) ─ rn-isp (NAT) 198.51.100.2 ─┐
                                                          rn-inet ─ rn-turn: coturn 4.18 at 192.0.2.10
 rn-client 198.51.100.60 ─ rn-cnat (NAT) 203.0.113.2 ──────────────┘   (UDP/TCP 3478, TLS 5349; 443 → 5349)
```

- **Home side:** double NAT, both nftables `masquerade`, which keeps the source port: endpoint-independent mapping, like the home netcheck.
- **Client NAT modes:**
  - `eim`: like the home side.
  - `symmetric`: `masquerade fully-random`, so the mapping varies by destination, as measured on a large shared network.
  - `udpblock`: symmetric, plus a firewall that drops all UDP and every TCP port except 443.
- **WAN firewalls:** every NAT router drops unsolicited WAN input, as any home or office router does.
  - Without that, the first hole-punch packet leaves a conntrack entry on the router itself, and the masquerade then remaps the real flow's port.
  - This is worth knowing for real Linux routers with open input chains.
- **coturn** runs unprivileged in its namespace with `use-auth-secret` credentials (the TURN REST scheme the rendezvous will hand out) and a test CA. TURN over TLS is reached on port 443 through an nftables redirect.

**Running it:** `scenarios.sh nat|switch|netem` drives the runs, and `analyze.py` summarises them.
- **The test program:** `remote-net` signals through a shared directory (a stand-in for the rendezvous) and pins both keys.
- **The host** streams synthetic 60 fps frames sized by `encoder_bitrate_bps()`: the first one 3× as big, as a keyframe.
- **The client** sends input at 120 Hz.
- **Frame latency** is from the host's send call to the frame's last packet arriving, on the client's clock via clock sync. It includes the pacing spread, and the bottleneck's serialization when there is one.

### 5.2 NAT setups and path switching (15 s each, 50 Mbps cap)

| Client side | Path found (both directions) | Connected / first frame | Frames lost | Frame latency p50 / p99 |
|---|---|---|---|---|
| EIM NAT (home LAN, easy hotspot) | **direct** hole punch through both NATs | 4 ms / 13 ms after the endpoints started | 0 of 899 | 7.2 / 7.8 ms |
| Symmetric NAT (a large shared network) | **UDP relay** (one relay leg each way) | 154 ms / 162 ms | 0 of 891 | 7.3 / 8.0 ms |
| UDP blocked, TCP 443 only | **TURN over TLS on 443** | 178 ms / 187 ms | 0 of 889 | 7.1 / 7.9 ms |

- **All three:** inputs arrived 1777–1798 of 1780–1801 (the last few were in flight when the run stopped), and MTU reached 1472 on every path.
- **A symmetric NAT against a port-restricted home NAT gives no direct path.** The hole punch fails because Linux conntrack filters by port.

**Live switching** (client EIM; direct UDP between the sites blocked at start, opened at about 7 s, blocked again at about 15 s):
- **Upgrade:** relay → direct 218 ms (host) and 235 ms (client) after the firewall opened. Here a 5 s recheck happened to be in progress; the worst case is about 5 s.
- **Failover:** direct → relay **1.20 s** (host) and **1.22 s** (client) after the direct path died. The session went on with no new handshake and no keyframe.
- **Cost:** a ~1.2 s video stall, 73 frames, while the dead route timed out. Section 6 has ideas for shortening it.

**Stress, no rate cap:**
- **Direct path:** it climbs to the 300 Mbps cap. Loss then appears at the receiving socket, and FEC (up to 8.5%) rebuilds it.
- **Through coturn:** the UDP relay saturates around 200–230 Mbps with 2–16% loss. The controller holds and then decreases, and FEC rises to 30%: 883 of 887 frames arrived.
- **Takeaway:** a TURN relay's capacity is a real ceiling. measure yours once it is deployed.

### 5.3 Network emulator (tc netem on the client's link, direct path)

| Scenario | Controller | Queue it keeps | FEC / NACK | Frames | Frame latency |
|---|---|---|---|---|---|
| **10 ms RTT, 1% loss each way**, 30 s | holds the 50 Mbps cap; random loss under 2% is not congestion | n/a | loss measured 1.01%; FEC 6% rebuilt 1164 shards; NACK off (RTT 10 ms > 8.3 ms) | 2 of 1799 lost (0.11%) | p50 11.5, p99 12.2 ms |
| **30 → 10 → 30 Mbps**, 10 ms RTT, 150-packet queue | start ramp 8 → 24 Mbps in 2.4 s. **30 → 10: converged in 0.55 s** (target ≤ 11 Mbps and queue < 5 ms, sustained). **10 → 30: back to 24 Mbps in 3.2 s.** Steady: 90–91% of capacity | standing queue p50 0.4–1.1 ms, p95 4.0–4.5 ms; peak 111 ms right after the drop | no loss outside the drop | 0 of 2698 lost | p50 19.4, p95 24.7, p99 29.9 ms; worst 138 ms at the drop |
| **100 ms RTT, 30 Mbps** (a far-away VPN) | start ramp 2.4 s; steady 91% of capacity | p50 0.5, p95 6.5 ms | NACK off; no loss | 0 of 1784 lost | p50 64.5, p95 71.8 ms |

**What the numbers say:**
- **Serialization dominates at the bottleneck.**
  - Frame latency at a 30 or 10 Mbps bottleneck is about propagation plus 14–15 ms. At 90% utilization a 60 fps frame takes about 0.9 of a frame interval to cross the link, whatever the queue.
  - The design budget's 2 ms for "pacing and transmit" holds only when capacity is well above the stream's rate.
  - See open question 1.
- **Tuning came from the emulator:** the deeper drain on overuse, the 100 ms rate window and the faster ramps.
  - The first version took 2.1 s to converge after the drop, lost 13 frames and needed 6.5 s to recover. The table shows the tuned version.
  - Comparing routes on smoothed RTT made the path flap during the drop; fixed with the windowed minimum (section 2.3).


### 5.4 Failover, hedging and route choice

Same testbed, plus three things in `netns.sh`: a second client path, a VPN-like `rn-vpn` (`vpn up`, `vpn-shape`); `blackout MS`; and `client-readdr`, which moves the client's address. `scenarios.sh content|hedge|vpn|regather|audio` runs them, and `analyze.py $EV m1` summarises. The host's synthetic stream follows `video_budget`. With `--content typing` it sends a latency-critical frame capped by the budget, then a refinement frame filling the interval behind it. With `--content passive`, one frame per tick (the earlier stream). The client types at 120 Hz in both.

**7.1, utilisation by content** (30 → 10 → 30 Mbps, 10 ms RTT, 150-packet queue, 45 s each):

| Steady state | Passive: interactive frames p50 / p95 | Typing: interactive frames p50 / p95 | Utilisation passive / typing | Standing queue p95 passive / typing | Loss |
|---|---|---|---|---|---|
| 30 Mbps | 19.7 / 25.0 ms | **16.4** / 20.5 ms | 91% / 92% | 4.5 / 4.0 ms | 0 |
| 10 Mbps | 17.4 / 22.6 ms | **14.4** / 17.9 ms | 79% / 82% | 4.4 / 4.7 ms | 0 |
| 30 Mbps again | 19.8 / 25.3 ms | **15.0** / 18.9 ms | 92% / 92% | 4.3 / 5.5 ms | 0 |

- **Result:** about 4 ms off the interactive frame's latency, with queue and loss unchanged, as expected (19.4 → about 15 ms).
- **Refinement frames:** p50 19.3 ms.
- **C_est** was held through all three steady states, at about 0.97 of capacity.
- **Convergence:** 30 → 10 in 0.6 s (earlier: 0.55 s); 10 → 30 back to 24 Mbps in 3.0–4.6 s (earlier: 3.2 s).
- Utilisation at 10 Mbps varied between runs (79–88%); an earlier run measured 86–90%.

**7.2, hedging** (live switching: relay, then direct at about 7 s, direct dies at about 15 s; then two 300 ms blackouts of the direct path):

| Target | Measured |
|---|---|
| First thin frame on the relay within 400 ms of the cut | **206 ms** (other runs: 192–208 ms). The longest gap between frames was 216 ms |
| Full switch within 800 ms | **698 ms** host, 700 ms client |
| At most 25 frames lost | **12** (earlier: 73) |
| No typed input lost | 2975 of 2975 |
| 300 ms blackout: duplication with no path change | Both blackouts: suspect 186–198 ms after the block on each side; no route change on either side |
| Target restored within 100 ms of the first answer | Restored **at** the first answer (the same call), to exactly the pre-suspect 50 Mbps; that answer came 39–63 ms after the path returned. 11–12 frames lost per blackout |

- While suspect, the budget is the thin 4 Mbps and every datagram goes on both routes.
- The relay → direct upgrade still takes about 0.2 s.

**Path choice by throughput** (a client with a second, VPN-like path: direct 10 ms RTT and uncapped; "VPN" 4 ms RTT):

| Run | Where the host sends video | Target | Frame latency p50 |
|---|---|---|---|
| VPN at 8 Mbps, throughput rule | direct (the VPN's train read 7.7 Mbps) | 43–50 Mbps | 10.1 ms |
| VPN at 8 Mbps, RTT and loss only (`--demand-mbps 0`, the old rule) | the VPN | 6.9 Mbps | 15.3 ms |
| VPN uncapped, throughput rule | the VPN (lower RTT, enough capacity; no train needed) | 50 Mbps | 8.9 ms |

**Re-gathering** (client bound to `auto`; at 8 s its address moves from 198.51.100.60 to .61; signalling closed after connect, so the new candidates go in-band only):
- **Video back 158 ms after the change**; 11 frames lost; one handshake for the whole run (no new session).
- **Client:** a relay route at +6 ms (TLS, then UDP at +106 ms), then direct at +296 ms.
- **Host:** gave the old route up at +643 ms. The hedge carried video meanwhile.
- The interface poll is 1 s; on the Mac, NWPathMonitor → `Command::NetworkChanged` makes it immediate.

**Audio** (10 ms packets, 10 ± 4 ms delay and 1% loss each way, 20 s):
- The jitter buffer stayed at its 20 ms target.
- Of 1956 slots: 1944 packets played, 11 rebuilt with Opus FEC from the next packet, 1 concealed; none late or dropped.
- Capture → playout p50 31 ms, against video send → complete 12.9 ms.

**Rekey** (130 s stream, EIM, 20 Mbps cap): one rekey on each side at 120 s and no new handshake. 7799 of 7800 frames arrived (0 lost), all 15599 inputs too, and no 100 ms stats row went without a new frame.

**Regressions** (the earlier scenarios, rerun):
- 1% loss: 3 of 1800 frames lost.
- 100 ms RTT: 92% of capacity.
- NAT setups: one route per side, kept; no flapping.

**Found and fixed in the emulator:**
- Trains to every relay route at once overflowed the shared access link.
- Feedback that only covered trains counted as an answer, which delayed a suspect by about 400 ms.
- ICE checks lost before a route existed, or during an outage, made the route look lossy and moved media to a relay.
- A bandwidth drop on the client's own link moved media to a relay whose train predated the drop (section 2.3).
- A `LinkStatus` snapshot read 0 bytes right after a frame (now the fresh-interval budget).

## 6. Not done yet

- **Integration:** the rendezvous and pairing (SPAKE2); mDNS LAN discovery; host candidates on `tun0` and `tailscale0` (supported, since each bound address becomes a candidate, but untested); IPv6.
- **Path choice:** birthday port prediction (only if the relay measures worse than the targets). Throughput-aware choice and re-gathering are done (section 5.4); the 50 Mbps demand and the 16.7 ms-per-multiple weight are untested on real VPNs.
- **Media:** the encoder and decoder ends of reference invalidation (the host's media code and the clients use `take_reference_hint`, `FrameOpts` and `ReceivedFrame::decodable`); Opus itself (the core carries packets and tells the decoder what to do).
- **Not measured yet:** the idle timeout (unit test only); the hedge thinning by the hedge route's measured rate (relay routes are seldom trained now, so it is usually 4 Mbps).
- **Real networks:** tuning on real Wi-Fi (AWDL) and real VPNs; the controller was tuned on netem only.
- **UniFFI:** generating the Swift bindings.

## 7. Decisions

### 7.1 Utilisation: by content, not a lower set point

- **The controller stays as it is**, at about 90% of the bottleneck; that's where it learns capacity. The allocator adds one rule, but only on a link measured as the bottleneck: the controller has seen a standing queue and holds a last-overuse receive rate, `C_est`.
- **Latency-critical frames** are capped at **0.65 × C_est × frame interval**: about 11 ms on the wire at 60 Hz (5.4 ms at 120 Hz) instead of 15. They are frames with input in the last 1 s, or with a compositor animation in flight. Expect about +2–3 QP during motion on constrained links, and nothing on the LAN.
- **Everything else fills up to the controller's target** (about 90%): build-to-lossless refinement, passive motion with no input, a still desktop.
  - Refinement packets go behind the interactive frame in the same interval.
  - The total per interval stays at or under today's target, so the standing queue stays at 4–4.5 ms (p95).
- **Unconstrained, app-limited links** (no standing queue seen): probed up to the session's demand (3.8).
- **After a suspect window (7.2):** restore the pre-suspect target directly, with no re-ramp.
- **Verify:** 30 → 10 → 30 Mbps with typing plus a window move. Frame latency p50 should go from 19.4 to about 15 ms, with no change in queue or loss.

### 7.2 Failover: hedge instead of stalling

> **Note:** the client UI timeline, the thin-rate reading, the per-session route demand and the C_est timer were refined after this section was written. Where they differ from this section, the code wins.

- **Liveness:**
  - a 100 ms authenticated ping/echo on the active route whenever nothing else went out (about 5 kbps each way); ICE's 500 ms checks on the other pairs are unchanged.
  - **Suspect** after **200 ms** unanswered (no echo, no transport feedback during video, no input ACK during input). Then recheck every pair at once.
  - **Dead** after **700 ms** (was 1.5 s): media moves to the best live route.
  - Each side judges its own sending direction.
- **While suspect, duplicate on the best other live route** (usually the Singapore relay):
  - **in full:** input, cursor, control, feedback and audio;
  - **video:** the same stream cut to a thin rate: min(target, 4 Mbps), or half the secondary's last measured rate. Frame rate is held at 60, with the auto-by-content ladder below that.
    - Every video packet goes on both routes with the same counter, so the replay window dedups.
    - The first thin frame references the last frame the client acknowledged (reference invalidation; a keyframe if it's outside the DPB). No keyframe and no second encode.
  - Nothing is duplicated continuously.
- **It stops** when the suspect route answers: duplication ends with no path change, and the pre-suspect target comes back. If the route dies instead, the thin stream is already flowing, and the controller re-baselines there.
- **Expected:** moving video on the relay 300–400 ms after the path dies (about 20 frames missing at 60 fps instead of 73), and no typed input lost.
- **The client during a stall:**
  - the last frame stays on screen (never blanked, blurred or dimmed), the cursor stays live, and unacknowledged input keeps being resent;
  - nothing shows for the first 500 ms;
  - after that, a small top-edge pill, "Reconnecting… · relay", and the path chip turns amber;
  - after 10 s with no live route, "Waiting for the network…".
  - A Disconnected card comes only at the session timeout. That is by design: "Lost" keeps retrying until the host's 2-minute hold ends.
- **Verify:**
  - the live-switching scenario: the first thin frame on the relay within 400 ms of the cut, the full switch within 800 ms, at most 25 frames lost;
  - a 300 ms blackout: duplication starts with no path change, and the target is restored within 100 ms of the first answer.

### 7.3 How the core implements 7.1 and 7.2

- **Latency-critical:** input in the last 1 s is tracked by the host's session from received input events. Animations come from `set_animation` (the plugin's HINT).
- **C_est's lifetime** (section 3.8) is an implementation choice: the design says what C_est is, not when it stops being true.
- **The thin rate** is the smallest of the target, 4 Mbps and half the hedge route's measured capacity (when a train measured it). The design's "min(target, 4 Mbps), or half the secondary's last measured rate" is read as all three bounding it.
- **Each side judges its own direction**, so a dead return path makes the other side hedge too. That is harmless: its probe is answered over the peer's hedge route and it recovers.
- **The client UI** gets `Health` (`unanswered_ms`, `silence_ms`, `hedging`), the hedge route and `no_route_ms` in `LinkStatus`. The 500 ms and 10 s thresholds stay in the UI. The design gives different stages (under 1 s, 1–30 s, Lost); which applies is an open question.

## 8. API

Rust names, all in `remote_core`.

### 8.1 Host: the encoder

| Call | What it gives |
|---|---|
| `Session::video_budget(now) -> VideoBudget`, `interval_budget(now)` | `interval_budget` is a fresh interval's budget, what `LinkStatus.budget` carries. `video_budget`: read before every frame. `encoder_bps` is the per-frame reconfigure value and `frame_bytes` the frame's cap (both FEC excluded). `latency_critical`, `capped` (the 0.65 × C_est cap applied) and `thin` (failover) say why. For a refinement frame in the same interval, read again: `interval_remaining_bytes` is what is left. |
| `Session::set_animation(bool)` | An animation is in flight (HINT start / settle). Input in the last second is tracked by the session itself. |
| `Session::latency_critical(now)` | Input in the last 1 s or an animation in flight. |
| `Session::link_state(now) -> LinkState` | `c_est_bps` (bottleneck measured), `queue_free_ms`, `queue_free_3s`, and `split_ok` = no C_est and 3 s queue-free: the encoder's split rule. It turns false at once; flip the encoder at a still moment. |
| `Session::take_reference_hint() -> Option<RefHint>` | Once after a loss or when a failover starts: `Reference(id)` (invalidate after `id`, predict from it) or `Keyframe`. |
| `Session::send_video_frame_with(now, frame, FrameOpts)` | `FrameOpts { reference: Some(id) }` for the frame that follows a hint; `recovery_point` for a completed intra refresh. |
| `Session::last_acked_frame()` | The newest frame the client reported decodable. |
| `Session::send_audio_frame(now, mic, timestamp, capture_us, opus)` | Audio with its capture time on the session clock (`now_us`). `send_audio` still works and stamps "now". |
| `Session::send_loss(now)` | Host: loss to the client over 2 s, for `OPUS_SET_PACKET_LOSS_PERC`. |

### 8.2 Liveness, hedging and path choice

| Call | What it gives |
|---|---|
| `Session::health(now) -> Health` | `state` (Healthy, Suspect, Dead), `unanswered_ms`, `silence_ms` (since anything arrived), `hedging`. |
| `Session::poll_transmit_via(now) -> (datagram, Via)` | `Via::Active` (and, while hedging, the hedge route too), `Via::ActiveOnly` (the liveness probe), `Via::Train(id)`. `poll_transmit` still works and drops the hint. |
| `Session::poll_path_signal() -> PathSignal` | `Suspect`, `Recovered`, `Dead`, `TrainReport`. The driver carries them to the path agent. |
| `Session::start_probe_train(now)`, `set_hedge_capacity(bps)` | For a sans-IO user that drives its own path layer. |
| `PathAgent::on_suspect`, `on_recovered`, `fail_selected`, `send_hedge`, `hedge_route`, `hedge_capacity` | The hedge route and failover. |
| `PathAgent::set_demand_bps`, `note_capacity`, `poll_train`, `train_started`, `send_train`, `on_train_report`, `live_routes` | Throughput-aware choice. |
| `PathAgent::remove_local(now, ip)`, `turn_streams_on(ip)` | Re-gathering when an address vanishes (`add_udp_socket` adds one). |
| `Session::send_candidate(now, sdp)`, `PathSignal::RemoteCandidate(sdp)` | In-band candidates (control kind 251); the driver does both ends. |

### 8.3 Session lifetime, audio, frames

| Call | What it gives |
|---|---|
| `Event::Disconnected { reason }` | **New variant**: `IdleTimeout` or `KeyExpired`. Code matching `Event` exhaustively needs an arm. |
| `Session::set_idle_timeout(d)`, `Session::reconnect(now)`, `Session::role()` | Default idle timeout 120 s. |
| `ReceivedFrame { reference, decodable, .. }` | **New fields** (constructed only by the core; patterns without `..` need them). |
| `Session::poll_audio(now, mic) -> Option<AudioOut>`, `audio_buffer(mic) -> Arc<Mutex<JitterBuffer>>`, `audio_stats(mic)` | The jitter buffer: call `pop` once per 10 ms. `AudioOut { timestamp, play: Packet / Fec(next) / Conceal, capture_local_us }`. `Event::Audio` is still emitted as before. |
| `SessionStats` | New fields: `c_est_bps`, `queue_free_ms`, `latency_critical`, `health`, `hedging`, `suspects`, `recoveries`, `deaths`, `rekeys`. |

### 8.4 Driver

| Item | What it gives |
|---|---|
| `EndpointHandle::status: watch::Receiver<LinkStatus>` | `health`, `route`, `hedge_route` (for "Reconnecting… · relay"), `no_route_ms`, `budget`, `link`, `ref_hint` with `ref_hint_seq` (apply each sequence number once). |
| `EndpointHandle::audio_out`, `mic_out` | The shared jitter buffers for the audio callback. |
| `Command::VideoFrameWith`, `Animation`, `CursorShape`, `Audio`, `SetMaxBitrate`, `NetworkChanged`, `Reconnect`, `SetRouteDemand` | New commands. `SetRouteDemand(bps)` sets how much throughput weighs in route choice (0: RTT and loss only). |
| `Notice::NetworkChanged { added, removed }` | **New variant** (section 2.3): local addresses came or went after the first gather. The new candidates follow as `LocalCandidate`; reopen signalling to carry them. |
| `EndpointConfig::bind` with `0.0.0.0:port` | Every IPv4 interface, watched and re-gathered. Fixed addresses work as before. |
| Receive timestamps | Taken in the socket task at `recv_from`, not when the loop gets round to the datagram. Probe trains and feedback arrival times need it. |

## Changes

- v1: first version.
- v1.1: section 7 holds the decisions on utilisation and failover.
- **v2.** A new prologue: a v1 peer can't connect.
  - **Wire:** the rekey payload (3.2, 3.3); `TRAIN` and `TRAIN_REPORT` probes; `last_good` after `TRANSPORT` and `FRAME_LOST`; audio and mic carry `u64 capture_us`; video flags 4 (reference, with a `u32` in the frame payload) and 8 (recovery point).
  - **Behaviour:** the 100 ms liveness echo, suspect at 200 ms and dead at 700 ms with hedging (7.2); the 0.65 × C_est budget for latency-critical frames (7.1); route score with loss and throughput (2.3); re-gathering; rekeying every 120 s; the 2-minute idle timeout; the jitter buffer; the app-limited cap no longer lowers the target (3.8).
  - **API:** section 8. Two changes can break existing code: the new `Event::Disconnected` variant, and the new `ReceivedFrame` fields in exhaustive patterns.
  - **Dependency:** `if-addrs` 0.13 (reading local addresses).
- **v2.1:** after live runs and the emulator. No wire change, except the new control kind 251 (a v2 peer without it ignores it as an application kind).
  - **Settling:** route classes, 2 s settling, TCP/TLS relays only on failure.
  - **Loss:** loss above 2% only, outages excluded.
  - **Trains:** 12 packets, only where a route could win, one at a time, stale after a drop on the active route; the active route's capacity is the windowed maximum of C_est.
  - **Candidates:** in-band, with `Notice::NetworkChanged`, `Command::SetRouteDemand` and `Session::interval_budget`.
  - **Answers:** feedback that only covers trains is no longer an answer.
  - Results in section 5.4.
- **v2.2:** the controller probes a quiet link up to the session's route demand with padding (3.5, 3.8). No wire change: a padding packet is a video header with `k = 0`, which an older receiver already drops. `Session::set_route_demand` (the driver passes `Command::SetRouteDemand` on to it); `SessionStats` gains the probe fields.
- **v2.3:** after a run on a large shared network. No wire change. A delay overuse counts only when the queue is ours (loaded, no step unless it rose), and only that case sets C_est; an unloaded stream's rising queue cuts the target only (3.8). Routes through the same relay don't compete on throughput (2.3). A probe may go under a refused probe's rate (3.8).
- **v3.** The prologue is `"SodiumOS Remote wire v3"`, and the label in the signalling seal's prologue (`keys::seal`) is `"SodiumOS Remote signal v2"`. Nothing else on the wire changed from v2.
  - A v2 peer can't connect. It can't open the sealed offer either, so it gets no `VersionMismatch` answer: the attempt just fails.
  - Paired keys stay valid: both ends only need the new build. The rendezvous and the relay use neither label.
- **v5 (`msg::VERSION` 5).** The control schema (`core/src/msg.rs`) changes; the transport prologue stays "wire v3". No v4 compatibility: a v4 host can't decode a v5 `Hello` and ignores it, and a v5 peer can't decode a v4 `Welcome`, so a host and its clients had to be updated together.
  - **Codec:** `Codec::H264` (8-bit 4:2:0; Annex B, one access unit per frame like HEVC), appended last. `DecodeSupport` gains `hevc` (HEVC 4:2:0 decode; every client so far) and `h264`. H.264 is used only when the client decodes it, the host encodes it, and HEVC isn't available on both sides (`DecodeSupport::use_h264`); HEVC stays preferred.
  - **Identity:** new `Os { Linux, Mac, Windows }`; `Hello.client_os` and `Welcome.host_os` pick the key table defaults (`keymap::default_rules`, below).
  - **Display:** `Welcome` gains `display_mode` (`Virtual`: the host makes REMOTE-1 and sizes it to the client; `Physical`: a real display; the stream is at most its native size, the client letterboxes), `live_resize` (false: never send `Resize`; a Physical host with it true, as `host-desktop` sends, encodes at the largest size with the display's aspect that fits the `Resize` window, never above native, and announces it with `Format`), `displays: Vec<DisplayInfo { id, name, width, height, scale, primary }>` and `current_display`. `scales` may be empty (hide the UI size control) and `home_monitor` is `Option` (None: hide that control).
  - **New messages**, appended after `Vsync`: `SelectDisplay(u32)` (client to host: show this display; the host answers with `Format`) and `SecureScreen(bool)` (host to client: a lock, login or UAC screen is up that the host can't capture or drive; show a notice; false when it is gone). remoted ignores `SelectDisplay` and never sends `SecureScreen` (its lock screen is drawn in the stream).
  - **Permissions and view-only (added to v5 before release, no version bump):**
    - `DisconnectReason::PermissionMissing { screen_recording, accessibility }`, appended last: the host's OS hasn't allowed something the stream needs (a Mac without Screen Recording). It replaces the `Error("screen-recording: …")` text match. The client shows the permission card; the flags say which permissions are missing (Accessibility alone doesn't refuse a client).
    - `DisconnectReason::ControlOff`, appended after it: "Allow remote control" was switched off on the host (a connected client, or a client that connects while it is off). It replaces the text `"off"` in `Error`; no client matches text.
    - `Welcome.view_only: bool`, appended last: input can't be injected (a Mac without Accessibility). The client shows a "View only" chip and may stop sending input. Windows, Linux and remoted always send false.
    - Pairing by a bare address is a pairing-message change, not a control-schema one: message `v: 2` (`session-api.md` section 4). A host accepts `v: 1` and `v: 2`; a client uses `v: 1` with a known ID.
    - **How the host's ID is bound by the code.** The transcript `T` already holds both SPAKE2 messages and the host's ID, and `confirm_H` is an HMAC over `T` keyed from SPAKE2's output. Without a known ID the SPAKE2 identity B is the constant `"remote host"` instead of the ID, and nothing else changes: the ID in M2 is covered by `confirm_H`, which only a peer with the code can compute, so the ID a client stores is the one authenticated by the code, never one from an unauthenticated hello. No change to the transcript was needed.
  - **Key table:** its evaluation moved from remoted into `remote-core` (`keymap.rs`: `KeyMapper`; rules + a key + the foreground app give the keys to inject as HID usages, which the host turns into evdev, scan codes or CGKeyCodes). `keymap::default_rules(client_os, host_os)` gives the starting table: Mac to Windows/Linux is `KeyRule::mac_defaults()`; Windows/Linux to Mac maps Ctrl to Cmd, Win to Control, Alt to Option and Alt+Tab to Cmd+Tab, with Ctrl staying Control in terminals; every same-family pair is empty (passthrough). The wire is unchanged: the client still sends its table in `KeyTable`.
- **v6 (`msg::VERSION` 6, `msg::MIN_VERSION` 6).** Version ranges and the public LAN name; the transport prologue stays "wire v3".
  - **Ranges:** `Hello` gains `min_version` as its second field; `Offer` gains `min_version` as its last field, after the v5 fields. `msg::negotiate(host, client)` picks the highest common version or returns the refusal; `VersionRange::from_peer` reads a pre-v6 peer as speaking exactly its `version`.
  - **Frozen layouts, so any two versions can refuse each other properly:** `Offer` keeps its v5 fields first and `min_version` right after them, and later versions only append (postcard ignores trailing bytes); `Hello` always starts `version, min_version`. A v6 host still reads a v5 `Offer` (`Signal::decode` falls back to the v5 layout) and a v5 `Hello` head (`msg::hello_version`), and a v5 host reads a v6 `Offer`'s v5 fields, so both directions get a `VersionMismatch` refusal, never a silent timeout.
  - **Offer freshness:** `Offer` gains `sent_us` (Unix µs, higher with every offer the client makes; `Offer::now_us`) after `min_version`. The host takes an offer only when it is newer than the last one it took from that client, so a recorded offer played again (on the LAN, or by a server reusing a session ID) is dropped unanswered, as the Noise IK first message's timestamp already is. An offer without it (v5, or a v6 build before it) reads as `sent_us = 0`: refused by its version, or as not fresh. The new connection then runs beside the current session and replaces it only once the client's first control message arrives under the session keys; an offer alone ends nothing.
  - **Refusal words:** the host puts its lowest version in `host` when the client is too old and its highest when it is too old itself, so `host > client` always means "update the client", which v5 clients already show. The Qt bridge adds `"update": "host" | "client"`; the Mac gets `version_mismatch_update_side()` and `protocol_min_version()`.
  - **Compatibility from here:** v6 speaks only 6 (it is the first release with ranges), so a v5 peer is refused both ways. v7 must speak 6 and 7: decode the v6 `Hello` and control schema and send `Welcome.version` 6 to a v6 client.
  - **OS flavor:** `Hello.client_os_flavor` and `Welcome.host_os_flavor` (last fields, a lowercase os-release `ID`; empty on Windows and macOS) tell the UI a SodiumOS Linux from another one ("sodiumos" shows "SodiumOS", anything else "Linux"). `Msg::decode` reads a `Hello` or `Welcome` from before them as having an empty flavor.
  - **mDNS:** the service is `_sodium-remote._tcp` (`brand::MDNS_SERVICE`; was `_remote._tcp`) and the advertised host name `sodium-remote-<id>.local.`. v5 and v6 builds don't see each other on the LAN, which matches the refusal above.
