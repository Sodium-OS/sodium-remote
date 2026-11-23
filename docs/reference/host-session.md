# Host session: what `remoted run` does on this computer

The host session: the session lifecycle, the display manager, input, the IME bridge, audio, clipboard, the device key and the `org.sodiumos.Remote` D-Bus API that Settings › Remote and the shell's privacy pill use. The video path is the media module's (`video-pipeline.md`); the network and pairing are remote-core's (`protocol.md`, `session-api.md`).

- **Code:** `remote/host/src/`, all in the `remoted` library (`lib.rs`); `main.rs` is the command line. `daemon.rs` (`remoted run`), `stream.rs` (the join to core and media, below), `session/` (the actor, the lifecycle state machine, settings and history), `display/` (Hyprland's socket, the scale rule, layout), `input/` (the key table, focus), `ime/`, `audio/` (with the audio shim in `remote/linux-media/csrc/audio_shim.c`, shared with the generic Linux host), `clipboard/`, `keys/`, `dbus/`, `media/`.
- **Unit:** `remote/host/systemd/sodiumos-remoted.service`; D-Bus activation `remote/host/dbus/org.sodiumos.Remote.service` (both linked by `remote/host/install.links`, which `scripts/install.sh` reads; linking starts and enables nothing).
- **Tests:** `cargo test -p remoted` (28 unit tests); the host-session scripts and the whole-stack tests (`tests/e2e/`: `run.sh`, `netchange.sh`), described in section 11.

## 0. One daemon

`remoted run` is one process: the session actor, remote-core's `connect::Host` (pairing, the rendezvous with `device.key`, mDNS and the LAN listener, ICE and the protocol) and the media pipeline (which owns the plugin socket). `stream.rs` joins them:

| From | To | What |
|---|---|---|
| core `HostEvent` | the actor | `ClientConnected` → `Cmd::Connect` (takeover already decided by core) with the display request from `Hello` and the path for the notification; `ClientLost` → `Lost`; `HoldExpired` and `ClientDisconnected` (not `TakenOver`) → `Bye`; `Resize`/`Settings` → `Resize`; `HomeMonitor`; clipboard offers, requests and chunks; IME preedit and commit; the pairing window's outcomes (`PairFailed`, `Paired`, `PairClosed`); route and stats → `Stats`. |
| core `HostEvent::Input` | media | Not through the actor: keys through the key table (`input::KeyMapper`, with the focused class from `activewindowv2`), everything as INPUT_* on `Media::input`, with the client's event time on CLOCK_MONOTONIC. |
| core `Control` | media | `KeyTable`, `KeyframeRequest` (an IDR), `Vsync` (the phase lock → FRAME_CLOCK). |
| the actor | core | `Accepted` → `Msg::Welcome` (format, `scales` from the display's size, home mode, locked, took over, resumed); `Display` → `Msg::Format`; IME → `Caret` / `FocusChanged` / `ImeReset`; clipboard → `ClipboardOffer` (inline up to 64 KiB, the rest on request); audio → `send_audio`; pointer lock; `Ended` → `disconnect(reason)` (`EndedAtComputer` included). |
| media | core | Frames with their reference mapped to core's frame ids and the host stamps appended to the meta (video-pipeline.md); cursor shapes as `msg::CursorImage` with REMOTE-1's scale, positions; animation hints. Core's reference hints → `predict_from` / `request_keyframe`. |
| the session | media | Media starts once REMOTE-1 is up (Welcome), restarts on a codec or rate change, and stops before REMOTE-1 goes. LOCAL_INPUT (capture-ipc v4): 2 in Privacy and Off, 0 in Mirror. The route demand (0.2 bit/pixel/frame at the mode, or the user's cap) goes to core at every mode change. |

**Pairing and `paired.json`:** core runs SPAKE2 against the code remoted makes and shows (`open_pairing_with`), stores the device and uploads the signed list; remoted reads `paired.json` and renames and removes through core. Three wrong codes still replace the code at once with the critical notification.


## 1. Running it

```
remoted run [--no-display] [--no-audio] [--no-clipboard] [--no-ime] [--no-rendezvous] [--rendezvous URL]
            [--no-lan] [--chroma auto|420|444] [--socket PATH] [--log-dir DIR] [--relay-only] [--bind IP]... [--dev]
remoted id          # this host's device ID and its code (creates device.key on first use)
```

`--socket` is the plugin's capture socket (tests: a nested Hyprland's `SODIUMWM_REMOTE_SOCKET`); `--log-dir` writes `host-frames.csv` and `host-input.csv` for the end-to-end harness; `--chroma` overrides Auto (4:4:4 when the client decodes it).

| Variable | Meaning |
|---|---|
| `HYPRLAND_INSTANCE_SIGNATURE`, `WAYLAND_DISPLAY` | The compositor. Hyprland imports both into the user manager, so the unit needs nothing more. |
| `REMOTED_DATA_DIR` | Tests: where `device.key`, `paired.json`, `settings.json` and `recent.json` live. Default `$XDG_DATA_HOME/sodium-remote/host`, i.e. `~/.local/share/sodium-remote/host` (the relay's `deploy.sh` reads `device.key` there). |
| `REMOTED_RUNTIME_DIR` | Tests: the runtime files (display snapshot, saved audio defaults). Default `$XDG_RUNTIME_DIR/sodiumos`. |
| `REMOTED_HOLD_SECS` | Tests: the hold of a lost session (default 120), in the actor and in core. |

`--dev` adds `org.sodiumos.Remote.Dev` (section 10), a stand-in client for the host-session tests. `REMOTED_PAIRING_SECS` (tests) shortens the pairing window from 300 s, to reach the expired state.

**Applying it live** (with the user present: it creates an output and moves windows):
1. `cargo build --release -p remoted` in `remote/`, and the plugin with `compositor/plugin/build.sh` (capture IPC v4).
2. `scripts/install.sh` links `remote/host/systemd/sodiumos-remoted.service` into `~/.config/systemd/user` and `remote/host/dbus/org.sodiumos.Remote.service` into `~/.local/share/dbus-1/services/` (`remote/host/install.links`); then `systemctl --user daemon-reload` and `systemctl --user enable --now sodiumos-remoted`. Settings › Remote starts it through the activation file when it opens, so the page works before the enable.
3. Enrol the host with the rendezvous: `remote/relay/deploy/deploy.sh allow-host` (it reads `device.key`).
4. Nothing changes on the desktop until a paired device connects.

## 2. The session lifecycle

One actor (`session::Host`) owns every change a connection makes. `session::core::SessionCore` holds the rules, tested on their own:

| Situation | What happens |
|---|---|
| Nobody connected | Start: REMOTE-1, IME `Activate`, the Remote sink, clipboard sync; the client hears `Accepted`. Notification "MacBook Air connected" (or "… connected while locked"). |
| Another device while one is **live** | It hears `InUse { by }` and asks "In use by X. Take over?". Confirmed, it takes over: the old client hears `Ended(TakenOver { by })`, input is released, REMOTE-1 follows the new size. One notification: "Mac mini took over". |
| Another device while one is **held** | Taken over silently; the new client's `Accepted` carries `took_over_from` (its "Took over from X" toast). |
| The same device again | Resumed: the display is resized if needed, nothing else changes. |
| The connection is **lost** | Held for 2 minutes: REMOTE-1, the layout and the audio route stay (a blip doesn't bounce sound to the desk speakers); IME `Deactivate` and INPUT_RESET (the desk can type). State `reconnecting`. |
| The hold runs out, the client leaves, `Disconnect`, `EndAtDesk`, the device is removed, Remote is turned off, SIGTERM | The session ends: the client hears `Ended`; clipboard stops, IME `Deactivate`, INPUT_RESET, the mic goes. **A Privacy or Off session that ends any way but `EndAtDesk` then puts the desk under the session lock**: `Lock()` on logind's session (the user's graphical one, `User.Display`; skipped when `LockedHint` is already true), the shell locks at once and sets `LockedHint` once every output is secure, and remoted waits for it (a 50 ms poll; `Lock()` again every 2 s, no cap; 3 s at most once it is stopping: SIGTERM, a logout). Journal: "locking the desk before putting the desktop back", "the lock is up after N ms". Then LOCAL_INPUT 0, the stream stops, the sink goes and the old defaults come back, the desktop is put back, then "MacBook Air disconnected · After 1 h 4 min. Windows are back where they were." `State` and `Session` stay as they were until then. Mirror, and `EndAtDesk`, never lock. A takeover is not an end (it keeps the session's mode). |

Refusals (`Refused { reason }`): `off`, `not_paired`, `not_ready` (no encoder), `locked` (locked and "Connect while locked" off), `display` (REMOTE-1 couldn't come up; nothing is left moved).

**With core-connect** (`remote_core::connect::Host`, session-api.md §6) the network side of these rules lives in core: it answers `Busy` itself and runs its own hold. The adapter maps its events onto the actor's commands:

| core `HostEvent` | remoted `Cmd` |
|---|---|
| `ClientConnected { client, hello, took_over_from, resumed }` | `Connect` (`takeover: true`: core already asked); answer with `Msg::Welcome` from `HostEvent::Accepted` (+ `display::scale::valid_scales` as `Welcome.scales`) |
| `ClientLost { client }` | `Lost` |
| `HoldExpired { client }`, `ClientDisconnected { client, reason }` | `Bye` (core's hold and remoted's both run 120 s; core's event ends it) |
| `Input(InputEvent)` | `Input` (keys, pointer, scroll, gestures, `ReleaseAll`), `Preedit`, `Commit` |
| `Control(Msg::Resize / HomeMonitor / ClipboardOffer …)` | `Resize`, `ClientHomeMode`, `Clipboard`, mic on/off |
| `Stats`, `Route` | `Stats` (path, transport, RTT, bitrate, fps for Settings and the pill) |
| `PairingAttempt / PairingFailed / Paired / PairingClosed` | `PairingAttempt` sets `Pairing.state` to `attempt` (15 s at most: a device that leaves mid-run sends nothing more); `PairAttempt`'s outcomes (section 8), `PairingFailed`, `Paired` and `PairingClosed` clear it |
| `HostEvent::Ended(reason)` (remoted → client) | `host.disconnect(DisconnectReason::…)`: `TakenOver{by}`, `User` (Disconnected by <Name>), `Revoked` (removed), `HostShutdown` (Remote off). "Ended at the computer" has no `DisconnectReason` yet (open question 5). |

## 3. The display manager

All through Hyprland's command socket (`display::hypr`: `j/monitors all`, `j/workspaces`, `j/clients`, `/eval <lua>`, `/output create headless`, `/output remove`). One dispatcher per `eval`; a reply starting `error` or `warning` is a failure.

**Connect:**
1. Snapshot every monitor, workspace and window; write it to `remoted-display-<sig>.json` in the runtime dir **before** anything changes.
2. REMOTE-1's rule: `mode = "<w>x<h>@<hz>"`, `position = "auto-right"`, the snapped scale (below); then `output create headless REMOTE-1`; wait until it runs at that mode and scale.
3. Focus REMOTE-1, move every workspace there (by id; named and special ones by selector).
4. The desk's home mode (below). Every mirror rule carries the desk's **own** mode, position, scale, transform, VRR, bit depth and colour preset (a rule with just `mirror` brings the output back at `preferred`/`auto`). remoted then **waits until Hyprland reports the mirror** before going on.
5. Focus REMOTE-1 again (Hyprland leaves focus on the mirror, and new windows then fail to map), and show the workspace the desk showed (REMOTE-1 comes up with a fresh empty one: found in this run; the client would otherwise see an empty desktop).
6. Floating windows: placed proportionally (Hyprland keeps their absolute position). Each keeps its place in the free space around it within the usable area (monitor minus the bar's and dock's reserved edges), so a window touching the right edge still touches it; a window larger than the area shrinks to fit. Tiled and fullscreen windows re-layout by themselves.

**Scale**: Hyprland accepts only k/120 with a whole-pixel logical size. remoted lists the valid scales for the size (1.0 to 3.0) and picks the nearest to the client's UI ratio **by ratio, ties to the smaller UI**; `disable_scale_checks` stays off. Larger and Smaller step along the list; "Same as desk" snaps the desk's scale the same way. 2560×1664 at 1.741 → **1.6**; Larger 2.0; Smaller 1.333; 2880×1864 at 1.684 → 1.6; 2240×1400 → 1.75. The list keeps only scales whose pixel period is 8 or less; the Mac streams its backing pixels, so 2940×1848 at 2.0 → **2.0**, Larger 2.333, Smaller 1.75. See open question 1 for window sizes with no fractional scale.

**Live resize:** every `Resize` restarts a 150 ms timer; when it fires, REMOTE-1's rule changes and floating windows on it follow from the old usable area to the new. The client hears `Display`. In the test three resizes inside 150 ms gave exactly one change.

**Home modes** (per session; the default is a setting):

| Mode | The desk monitor | How |
|---|---|---|
| Mirror | Mirrors REMOTE-1, letterboxed | the full mirror rule |
| Privacy | On, not mirroring, no windows (the shell draws "In use remotely" there) | the rule with `mirror = ""`; focus stays on REMOTE-1 |
| Off | The privacy screen with the panel off: not mirroring, local input ignored (LOCAL_INPUT 2); the first desk key lights it, 30 s after the last it goes off again (the shell's privacy screen does both) | Privacy's rule, then `hl.dsp.dpms({ action = "off", monitor = … })`, at the start of Off and on a change to Off only (the desk's rule doesn't follow REMOTE-1, so nothing re-asserts after a resize) |

**Disconnect** (for Privacy and Off only once the desk is under the session lock, section 2): DPMS on and the un-mirror rule with the desk's own values (waiting until it applies) → every workspace back to its monitor (ones made during the session to the desk that had focus) → each monitor shows the workspace it showed → floating windows to their exact snapshot rects; windows opened during the session placed proportionally → `output remove REMOTE-1` → focus back. In every test the result was **identical** to the snapshot (windows, workspaces, and the desk's mode, position, scale, mirror and DPMS state).

**A crashed remoted:** the next start finds the runtime snapshot and REMOTE-1 still up, and puts everything back before serving (tested with SIGKILL). A Privacy or Off leftover (the snapshot keeps the mode) goes back under the session lock first, as at a session's end; normally the shell has locked already, when remoted left the bus. SIGTERM ends the session properly first.

## 4. Input

`input::to_plugin` turns the client's events (remote-core's `msg::InputEvent` shape) into capture-ipc v3's INPUT_* messages (the media module's `media::InputEvent` shape), and the media module writes them on the one plugin connection it owns (`Media::input`, through the `PluginLink` trait):
- keys: USB HID usage (page 7, already through the client's Mac or PC table) → evdev, as the kernel's `hid_keyboard` table maps a USB keyboard; the host keeps its own xkb layout;
- buttons: Left/Right/Middle → `BTN_LEFT/RIGHT/MIDDLE`, Back/Forward → `BTN_SIDE/EXTRA`, others as given;
- scroll: source, axis, `value`, `value120`, `stop`, `more`; gestures: kind 1–3, phase 0–3;
- `ReleaseAll`, a lost connection and every session end → INPUT_RESET.

Times: `t_ns` is the client's event time mapped to this host's `CLOCK_MONOTONIC` (0 = now). Core's `InputEvent` carries no time and no `more` flag yet (open question 6).

## 5. The IME bridge

`ime::ImeBridge` drives the fcitx5 add-on's `org.sodiumos.Remote.Ime` (ime-and-dnd.md §1): `Activate` on connect and on a resume, `Deactivate` on a lost connection and at the end; the client's marked text → `SetPreedit(text, cursor)`, its insertText → `Commit(text)`. Back to the client: the add-on's `CaretRect`, `FocusChanged(focused, password)` and `Reset`, and the plugin's CARET (from the media module's `MediaEvent::Caret`: purpose 8 password and 9 PIN mean secure input). If fcitx5 restarts mid-session, remoted activates the add-on again when its bus name returns. Without the add-on (not installed live yet) the calls fail quietly and the session carries on.

## 6. Audio

`audio_shim.c` (in `remote/linux-media`, the `remote-linux-media` crate shared with the generic Linux host; PipeWire and libopus, in C because SPA pods are inline functions and macros):
- **"Remote" sink:** a PipeWire stream with `media.class = Audio/Sink` (what module-loopback makes), `node.name = sodiumos-remote`, 48 kHz stereo F32. WirePlumber links apps into it like any sink. Its mix becomes 10 ms Opus packets (128 kbit/s, restricted low delay) on a reader thread, sent to the current client (`HostEvent::Audio`; with core, `host.send_audio`). Silence sends nothing: an idle sink isn't scheduled.
- **"Remote microphone":** `media.class = Audio/Source`, `sodiumos-remote-mic`, 48 kHz mono, fed by the client's Opus packets (decoded into a 200 ms queue; underruns play silence). Created when the client turns its mic on (with core: from `host.mic_buffer()`), removed when it turns it off or the session ends.
- **Defaults:** WirePlumber's `default` metadata. Before the sink appears remoted saves `default.configured.audio.sink` **or its absence**, the node in effect (`default.audio.sink`) and that node's channel volumes and mute as exact floats (read through the node's Props, not a 6-decimal CLI). It sets the configured default to the Remote sink. At the end: configured default back (or removed), the sink destroyed, then the old node's volume compared and put back if anything changed it. The same for the default source and the mic.
- **After a crash** the saved defaults are in `remoted-audio.json` in the runtime dir; the next start restores them.

## 7. Clipboard

`clipboard::Clipboard` runs on its own thread with its own Wayland connection and ext-data-control-v1:
- **Host → client:** each new selection that isn't remoted's own is read in text (`text/plain;charset=utf-8`, from any of the usual text types), `text/html` and `image/png` (2 s and 32 MiB per type) and sent as `HostEvent::Clipboard` (core: `Msg::ClipboardOffer`).
- **Client → host:** the entry becomes the selection; text is offered under every common text type.
- **Sensitive:** a selection offering `x-kde-passwordManagerHint` = `secret` (KeePassXC and other Qt password managers) is sent `concealed`; a concealed entry from the client is offered with that hint, so the shell's history skips it. remoted keeps clipboard data only in memory while it owns the selection and wipes it when it loses it; nothing is written to disk or logged (Debug prints types and sizes only).
- Its own offers carry `application/x-sodium-remote`, so they are never echoed back. The primary selection is ignored.

## 8. Keys, pairing and the rendezvous

- **`device.key`:** Ed25519, PKCS#8 PEM v1 (exactly what `openssl genpkey -algorithm ed25519` writes), mode 0600 in a 0700 dir, made on first start; a key readable by others is refused. The device ID (base64url public key) matches what `openssl pkey -pubout` and `deploy.sh host-id` derive (checked in the nested test).
- **`paired.json`:** remote-core's `keys::Pairings` layout (`seq`; `id`, `name`, `paired_at`, `last_connected`, `named_at_desk`), plus `kind`. Core takes the device's own `Hello.name` into `name` at every connection unless `named_at_desk` is set (the desk renamed it); `RenameDevice` with `""` clears the flag and keeps the name until the next connection. `seq` only grows, and never falls below the wall clock, so a lost file can't replay an old list.
- **The paired list** for the rendezvous is signed here (`DeviceKey::sign_paired_list`, rendezvous-api.md §5) and handed to the `Rendezvous` trait after every pairing and removal.
- **Pairing windows:** `OpenPairing` makes a 6-digit code (uniform, from `getrandom`), 5 minutes, 3 wrong tries; the third closes pairing (`state: attempts_used`), with the critical notification "Pairing closed" when the dialog isn't open. No new code opens by itself: only the person at the computer opens pairing again, so three guesses are all a stranger gets. Success: the device is stored, the list uploaded, the window closed, "MacBook Air is paired" when the dialog isn't open. The SPAKE2 run itself is core's (`pairing::HostWindow` does the same window rules); with core, remoted shows core's code and maps its pairing events.
- **The rendezvous** sits behind the `Rendezvous` trait (`paired_list`, `pair_open`, `pair_close`, `sleep_notice`, `state`). `NoRendezvous` logs until core's `connect::Host` is wired in (it does registration, reconnection, the list upload and `sleep_notice`/`wake_notice` itself).

**Wake-on-LAN groundwork (design: WoL):**
- **The sleep notice:** remoted holds a logind *delay* inhibitor; on `PrepareForSleep(true)` it tells the rendezvous (`Cmd::Sleep(true)` → `sleep_notice`), then lets the inhibitor go; on wake it takes it again. The relay has no `sleep` request yet (session-api.md open question 1).
- **The root step (not applied; with the user present):** the wired NIC is `eno3` (Intel I226-V, `igc`; its PCI wakeup is already `enabled`). `ethtool` isn't installed.
  1. `sudo pacman -S ethtool`; check `sudo ethtool eno3 | grep Wake-on` lists `g`.
  2. Turn it on now: `sudo ethtool -s eno3 wol g`.
  3. Keep it across boots, with a systemd link file `/etc/systemd/network/50-wol-eno3.link`:
     ```
     [Match]
     MACAddress=c8:7f:54:0b:29:01
     [Link]
     NamePolicy=kernel database onboard slot path
     WakeOnLan=magic
     ```
     (or `nmcli connection modify <wired connection> 802-3-ethernet.wake-on-lan magic`, since NetworkManager manages the link).
  4. In the firmware setup, enable "Power On By PCI-E" (ASUS boards), or WoL won't wake from S5.

## 9. The D-Bus API: `org.sodiumos.Remote`

Bus name and interface `org.sodiumos.Remote`, object `/org/sodiumos/Remote`, session bus. Every property emits `PropertiesChanged`; writable ones apply at once (Settings has no Apply). Shaped from the RemoteSettings (SR1–SR9) and RemoteShell (SB1–SB8) boards.

**Status**

| Property | Type | Meaning |
|---|---|---|
| `ApiVersion` | u | This API's version, 2 (1: before the public release). Later versions only add. The reference is `host/dbus/org.sodiumos.Remote.xml` (and `.Dev.xml`); a test diffs it against the live interface (`UPDATE_DBUS_XML=1` rewrites it for an additive change) |
| `State` | s | `off`, `ready`, `not_ready` (no encoder), `connected`, `reconnecting` (a lost session is held; the pill's dot pulses) |
| `DeviceId` | s | This host's ID (43 base64url characters) |
| `DeviceCode` | s | The same as the code people compare by eye, `XXXX-XXXX`; `DeviceId` is what Copy copies |
| `Session` | a{sv} | Empty when nobody is connected. `device`, `name`, `since` (t, Unix), `path` (`lan`, `direct`, `vpn`, `tailscale`, `relay`), `network` (the label of the `networks.toml` network a `vpn` path is in; absent otherwise), `transport` (`udp`, `tcp`, `tls`), `rtt_ms`, `bitrate_kbps`, `fps`, `width`, `height`, `refresh_mhz`, `scale`, `held` (b), `held_until` (t), `locked_at_connect` (b), `home_mode` (this session's), `mic` (b) |
| `PairedDevices` | aa{sv} | `id`, `name`, `kind` (`mac`…), `paired_at`, `last_seen` (t, 0 = never), `connected` (b); `name` is the device's own name from its last connection unless the desk renamed it |
| `Pairing` | a{sv} | Empty when no window. `code`, `expires_at` (t), `attempts_left` (u), `state` (`open`, `attempt`: a device is running SPAKE2 against the code right now, `attempts_used`: three wrong codes closed it, `expired`, `paired`), `paired_name` |
| `RecentConnections` | aa{sv} | 30 days, newest first: `device`, `name`, `start`, `end` (0 = now), `path`, `transport`, `rtt_ms` |
| `Relays` | aa{sv} | `host` (the configured server's name), `state` (`unknown` until core is wired, `online`, `unreachable`, `not_enrolled`), `rtt_ms`, `last_ok` |
| `Capture` | s | `plugin`, `fallback` (ext-image-copy-capture: information) or `unknown` |
| `EncoderReady` | b | False: devices can't connect ("Not ready") |

**Settings** (read-write; persisted in `settings.json`)

| Property | Type | Default |
|---|---|---|
| `Enabled` | b | true. Off stops connections and ends a session; pairings stay. |
| `Name` | s | What paired devices call this computer: the one you set, else the computer's own name (hostnamed's PrettyHostname, else the hostname: `archlinux` on this machine), **followed live** (remoted watches hostname1's `PropertiesChanged`). Setting `""` goes back to following it (the reset; hostnamed's own `PrettyHostname = ""` means "none" the same way, so there is no Reset method). It re-announces over mDNS under the new name at once; a pairing window that is open keeps the name it was opened with. |
| `MdnsVisible` | b | true. Live: off withdraws the mDNS announcement (the LAN listener keeps running, so a device that knows the address still connects), on announces again. |
| `ConnectWhileLocked` | b | true |
| `HomeMode` | s | `mirror` (`privacy`, `off`): the default for new sessions |
| `Notify` | b | true |
| `PairingDialogOpen` | b | Settings sets it while the pairing dialog shows, so "is paired" and "Pairing closed" notify only when it doesn't |

**Methods**

| Method | Does |
|---|---|
| `Disconnect()` | Ends the session; the device sees "Disconnected by <Name>" (Settings, the pill, the notification's button) |
| `EndAtDesk()` | The privacy screen (or the lock's desk form during a Privacy or Off session) accepted the password at the desk; the device sees "Ended at the computer", and the desktop comes back unlocked (the one end that doesn't lock) |
| `SetSessionHomeMode(s)` | The desk monitor's mode for this session only (the pill's segmented control) |
| `RenameDevice(s id, s name)` | 1–64 bytes; `""` follows the device's own name again |
| `RemoveDevice(s id)` | Deletes the pinned key and uploads the new list; a connected device is dropped at once |
| `OpenPairing() → (s code, t expires_at)` | Opens a window; again = "New code" |
| `CancelPairing()`, `ClearRecent()` | |

Errors are `org.freedesktop.DBus.Error.Failed` with a sentence (`InvalidArgs` for a bad mode).

**Notifications** (org.freedesktop.Notifications, app "Remote", icon `sodiumshell-remote`): "<Device> connected · The desk monitor mirrors it." [Disconnect]; "<Device> connected while locked · The lock screen is showing there too. Unlock happens in the stream."; "<New> took over · <Old> was disconnected. One device streams at a time." [Disconnect]; "<Device> disconnected · After 1 h 4 min. Windows are back where they were."; "<Device> is paired · It can connect from anywhere now." [Remote settings]; "Pairing closed · Someone typed a wrong code 3 times, so pairing closed. To pair a device, open pairing again." [Pair a device], urgency critical, sent even with Notify off. The Privacy and Off connect bodies are PROVISIONAL (not on the board). Actions: Disconnect calls `Disconnect`; the others open `settings://remote` and `settings://remote/pair`.

## 10. The test-only Dev interface

`org.sodiumos.Remote.Dev` at `/org/sodiumos/Remote/Dev` (only with `--dev`): `Connect(s device, u w, u h, u refresh_mhz, s ui, d ui_ratio, b takeover)`, `Lose(s)`, `Bye(s)`, `Resize(s, u, u, u, d)`, `Preedit(s, s, i)`, `Commit(s, s)`, `Clipboard(s, s text, b concealed)`, `Mic(s, b)`, `HomeMode(s, s)`, `Pair(s code, s device, s name) → b` (stands in for SPAKE2), `Stats(s device, s path, s transport, u rtt_ms, u bitrate_kbps, d fps)` (the transport's measurements: the path and RTT in `Session`), `Media(s capture, b encoder_ready)` (what the media module reports), `Relay(s state, u rtt_ms)` (what the relay row shows: `unknown`, `online`, `unreachable`, `not_enrolled`), `Events(s device) → (as, t)` (what that device heard, and its audio packet count).

## 11. Tests and evidence

| Script | Where | Checks |
|---|---|---|
| `test-nested.sh` | one nested Hyprland (its window is the desk), a private bus standing in for the session and system buses, a fake notification server, Qt test windows | 53: pairing and the code replacement; connect (2560×1664@60 at 1.6, the desk mirroring with its own values, every window on REMOTE-1, focus, the desk's workspace shown, the floating window inside); a window mapping while mirrored; clipboard both ways with the password-manager hint (a Qt stand-in copies like KeePassXC); debounced live resize; the three home modes; takeover asked and silent; exact restores after Disconnect, the hold, SIGTERM and SIGKILL; rename, remove, refusal; notifications' wording; no clipboard text in the log |
| `test-audio.sh` | a private PipeWire (stock config plus null "Desk speakers" and "Desk mic") and WirePlumber (`--profile policy`, no device monitors), its own runtime, state and bus | 14: the Remote sink is the default, 100 Opus packets for a 1 s tone, the mic source is the default input, the configured default and the desk's exact volume (0.25/0.5, changed to 0.75 muted mid-session) come back, an unset default stays unset, crash recovery, and the live PipeWire's defaults and volumes unchanged |
| `test-ime.sh` | the input-method test bed (nested Hyprland with REMOTE-1, private bus, its own fcitx5 with the add-on) | 12: Activate puts GTK and Qt fields on `remote`, inline preedit, commit 你好, CaretRect and FocusChanged reach the client, a lost session gives Rime back and a resume takes it, disconnect restores Rime and keyboard-us |

## 12. What the sodiumwm plugin (and the shell) must add

| For | What |
|---|---|
| **Privacy screen: local input ignored** | A switch remoted can flip (a capture-ipc message, e.g. 71 `LOCAL_INPUT { u32 mode }`: 0 normal, 1 ignore the physical keyboard and pointer, keeping the plugin's own `sodiumos-remote-*` devices; 2 = 1 but keyboard events still go to the shell's privacy layer while it has keyboard focus, for the password). Back to 0 3 s after the socket closes (: the shell locks meanwhile, and with the lock up mode 2 sends desk keys to the desk's lock form), so a crashed remoted can't lock the desk out. Built (capture-ipc.md). |
| **Off: injected input must not wake the desk** | With `misc:mouse_move_enables_dpms` or `key_press_enables_dpms` on (both off live today), the Mac's input would power DP-2 up. The plugin's devices should not count as DPMS activity. |
| **Floating placement** | Optional: a way to ask sodiumwm's placement rules ("clamped by sodiumwm's placement rules"); remoted clamps to the usable area for now. |
| **Shell (built)** | `shell/services/Remote.qml` (a view of `State` and `Session`, with `Disconnect`, `SetSessionHomeMode` and `EndAtDesk` through `helpers/remote-watch.py`); the Bar's privacy pill with "Remote · <device>" and its popover (`modules/Bar/PrivacyPill.qml`, `RemoteSection.qml`); the privacy screen, its own Quickshell process (`shell/privacy.qml`, `modules/Remote/PrivacyScreen.qml`) started by `modules/Remote` while `Session.home_mode` is `privacy` or `off`, a layer named `sodiumshell-privacy` with keyboard interactivity none, whose password calls `EndAtDesk()` and then leaves with the unlock motion (in Off it also lights the panel at a desk key); the lock's chip "Connected from <device>", a full form on REMOTE-1 with its own PAM attempt, and a desk unlock during a Privacy or Off session calling `EndAtDesk()` first (`modules/Lock`); `Sodiumwm.remoteAnimation(seconds)` for the shell's own animations. Wave 2's A and B 7, 8, 11 (built): the lock answers logind's `Lock` at once during a Privacy or Off session and sets `LockedHint` once every output is secure; it locks on its own when remoted leaves the bus mid-session, or when such a session ends with no lock up and not by the desk password; the privacy screen waits for the lock before it quits; three quick privacy-screen failures fall back to Off with a notification; a desk password that remoted doesn't answer in 2.5 s restarts remoted, and the lock opens once `State` is ready; the pill's dot turns amber for 5 s on `remotelocalinput`, and wrong desk passwords post one notification with the count. Settings › Remote is the Settings app's. Details: `shell/modules/Remote/README.md`. |

## 13. Open questions

All eight were answered in (1 → item 1, 2 → 2, 3 → 3, 4 → 4, 5 → `DisconnectReason::EndedAtComputer`, 6 → msg v2's event time and `Axis.more`, 7 → item 5, 8 → item 6) and are implemented (section 0). Kept for the record:

1. **Window sizes with no fractional scale.** Hyprland takes only k/120 with a whole-pixel logical size. A windowed client size like 2236×1349 (the RemoteSettings board's own example) has **no** valid scale but 1.0 in 1.0–3.0 (checked in nested: asking for 1.6 gave 1.0), so "the nearest valid scale" makes the UI 1.6× smaller whenever the window has an odd size, and live resize jumps between 1.6 and 1.0. Options: (a) REMOTE-1 a few pixels smaller than the window (each side rounded down to a multiple of the scale's denominator, 8 px at 1.6), the client letterboxing ≤ 7 px; (b) the client snapping its window size to such multiples; (c) `disable_scale_checks` (decided against); (d) accept 1.0. remoted implements the decided rule only.
2. **Audio during the hold.** remoted keeps the Remote sink as the default while a lost session is held (2 minutes of silence at the desk rather than sound bouncing between the speakers and the Mac on a blip). The IME add-on is deactivated and input released during the hold, so the desk can type.
3. **The short device ID.** Settings shows "7QK4-XM2P"; remoted exposes the full 43-character ID. Its short form (and its derivation) isn't specified.
4. **The Privacy and Off connect notifications** have no fixed wording yet (current text: "The desk monitor shows the privacy screen." / "The desk monitor is off.").
5. **"Ended at the computer"** needs a `DisconnectReason` in remote-core's `msg` (today: `User`, `TakenOver`, `HostShutdown`, `HostSleeping`, `Revoked`, …).
6. **Core's `InputEvent`** has no event time and no axis `more` flag; capture-ipc v3 has both (kinetic scrolling and double clicks keep the client's spacing; both axes of a trackpad event in one `wl_pointer.frame`).
7. **Cmd+C in a terminal** needs the focused app, which only the host knows; the client applies the key table. Either the host reports the focused window's class to the client, or the host applies that one rule.
8. **Settings storage.** remoted keeps Settings › Remote's values in `settings.json` next to `device.key` and serves them over D-Bus; the design names them as keys (`remoteEnabled`, …) as if Settings stored them. One program should hold them; this assumes remoted.

## 14. Settings › Remote over this API

The page (`apps/settings`: `src/remote.cpp`, `qml/rows/Remote*.qml`, `qml/RemotePairDialog.qml`; README there) is a view, like the Bluetooth page over BlueZ: it reads `GetAll` and follows `PropertiesChanged` (debounced), writes through `Properties.Set` and the methods above, and keeps no value of its own.

- **Starting it:** opening the page with remoted missing calls `StartServiceByName("org.sodiumos.Remote")` (the activation file, which starts the user unit). If that fails, the page says "Remote isn't running" with **Start it** (`systemctl --user start sodiumos-remoted`) and **Show log** (the unit's journal in the viewer). What it says comes from the unit (`systemctl --user show sodiumos-remoted -p Result,NRestarts`): `Result=start-limit-hit` reads "remoted stopped 3 times in a minute, so it's paused. Starting it again usually helps.", `exit-code` or `signal` "remoted stopped. Starting it again usually helps.", anything else "The Remote service stopped, so devices can't connect. Starting it again usually helps." The sidebar's amber dot shows while the relay is `unreachable` or `not_enrolled`, the encoder isn't ready, or the user unit is enabled (`systemctl --user is-enabled`) and remoted isn't running; it survives a Settings restart, and a Remote that was never installed has no dot.
- **The card:** `State` (off, ready, not_ready, connected, reconnecting) with `Session`; its switch is `Enabled`, and turning it off while connected asks first. A held session (`Session.held`) reads "Connection lost · Windows return in 1:42", counting down live to `held_until` (the host holds, the device retries: "Reconnecting" is the device's word).
- **This computer:** `Name`, `DeviceCode` (the row opens to `DeviceId`; Copy copies the full ID), `MdnsVisible`.
- **Pairing:** the dialog calls `OpenPairing` and shows `Pairing` (`open`, `attempt`: "A device is pairing…" for at least 600 ms so a half-second SPAKE2 run never flashes, a wrong try amber, `attempts_used` (closed after three wrong codes, with New code), `expired`, `paired`); it sets `PairingDialogOpen` while it is on screen and calls `CancelPairing` when it closes. Not drawn: the board's QR code (the code alone finds the computer). The board's "Try again" and "Details" stay out: core retries the relay by itself.
- **Relay:** `Relays[0].state`; `not_enrolled` shows the ID and `deploy.sh allow-host <ID>` with Copy (the allowlist is one of the server's enrolment modes; see `rendezvous-api.md`).
- **Deep links:** `settings://remote` and `settings://remote/pair` (a notification's `xdg-open`) reach Settings through its `x-scheme-handler/settings` entry (`org.sodiumos.Settings.desktop`: `MimeType=x-scheme-handler/settings;`, `Exec=… %u`). After the first install run `update-desktop-database ~/.local/share/applications` (or `xdg-mime default org.sodiumos.Settings.desktop x-scheme-handler/settings`) once so `xdg-open` finds it.
