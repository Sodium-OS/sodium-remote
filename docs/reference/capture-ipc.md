# Capture IPC: sodiumwm plugin ↔ remoted (v4)

The contract between the plugin's `remote/` module (C++, inside Hyprland) and `remoted` (Rust). They are separate processes so that a crash in either leaves the desktop running. This is the SodiumOS host's contract with its compositor; the plugin lives in the SodiumOS compositor tree and is not part of this repository. The canonical wire structs are `host/capture-ipc/capture_ipc.h`; a test checks its layout against this document and `capture::plugin`'s constants.

- **Changing it:** change it only by bumping `version` and noting what changed at the bottom, so `remoted` can follow.
- **Rule of thumb:** the plugin never blocks Hyprland's render loop on `remoted`. If the daemon is slow, gone or out of buffers, the plugin drops that frame and counts the drop.

## Transport

- **Socket:** a Unix `SOCK_SEQPACKET` socket at `$XDG_RUNTIME_DIR/sodiumos/remote-capture.sock`. The plugin listens and `remoted` connects. `$SODIUMWM_REMOTE_SOCKET` (in Hyprland's environment) overrides the path; tests use it so a nested Hyprland never takes the live session's socket. A path another live listener owns is left alone.
- **Connections:** one at a time. A second one is refused (closed at once).
- **Peer check:** the plugin checks `SO_PEERCRED` and accepts only the same UID.
- **Message format:** each message is one packet: an 8-byte header `{u32 type, u32 length}`, then `length` bytes of payload. All of it is little-endian, with fixed-layout structs (`#[repr(C)]` on the Rust side, plain structs in C++, no padding surprises: use explicit `u32` padding fields). A packet whose length doesn't match its header closes the connection.
- **File descriptors** travel as `SCM_RIGHTS` on the packet of the message that needs them, in the order the message lists them.
- **Timestamps** are `CLOCK_MONOTONIC` in nanoseconds.

## Messages, remoted → plugin

| Type | Name | Payload | fds |
|---|---|---|---|
| 1 | HELLO | `u32 version` (the highest remoted speaks; 4) | |
| 2 | START | `char output[32]` (NUL-padded, e.g. `REMOTE-1`), `u32 ring_size` (3; clamped to 1–8), `u32 flags` (bit 0: prefer LINEAR, for tests) | |
| 3 | STOP | (none) | |
| 4 | RELEASE | `u32 generation`, `u32 buffer_id` | optional release sync_file |
| 5 | FRAME_CLOCK | `i64 phase_offset_ns`, `u32 refresh_mhz`, `u32 grace_ns` | |
| 64 | INPUT_POINTER_ABS | `f64 x`, `f64 y` (buffer pixels), `u64 t_ns` | |
| 65 | INPUT_POINTER_REL | `f64 dx`, `f64 dy`, `f64 dx_unaccel`, `f64 dy_unaccel` (logical pixels), `u64 t_ns` | |
| 66 | INPUT_BUTTON | `u32 button` (evdev `BTN_*`), `u32 state` (0 up, 1 down), `u64 t_ns` | |
| 67 | INPUT_AXIS | `u32 source` (0 wheel, 1 finger, 2 continuous, 3 wheel tilt), `u32 axis` (0 vertical, 1 horizontal), `f64 value` (logical pixels), `i32 value120`, `u32 flags` (bit 0: stop, bit 1: more), `u64 t_ns` | |
| 68 | INPUT_KEY | `u32 keycode` (evdev `KEY_*`), `u32 state` (0 up, 1 down), `u64 t_ns` | |
| 69 | INPUT_GESTURE | `u32 kind` (1 swipe, 2 pinch, 3 hold), `u32 phase` (0 begin, 1 update, 2 end, 3 cancel), `u32 fingers`, `u32 pad`, `f64 dx`, `f64 dy`, `f64 scale`, `f64 rotation`, `u64 t_ns` | |
| 70 | INPUT_RESET | (none) | |
| 71 | LOCAL_INPUT | `u32 mode` (0 normal, 1 the desk's input ignored, 2 the same, but desk keys go to the privacy screen), `u32 pad` (v4) | |
| 72–95 | | Reserved | |

## Messages, plugin → remoted

| Type | Name | Payload | fds |
|---|---|---|---|
| 129 | HELLO_ACK | `u32 version` (the one both speak: min(HELLO's, the plugin's)), `char hyprland[32]` (version string, e.g. `v0.57.0`) | |
| 130 | BUFFERS | `u32 generation`, `u32 count`, `u32 width`, `u32 height`, `u32 drm_format`, `u32 n_planes`, `u64 modifier`, then `n_planes × {u32 offset, u32 stride}` | `count × n_planes` DMA-BUF fds, buffer-major |
| 131 | FRAME | `u32 generation`, `u32 buffer_id`, `u64 seq`, `u64 t_rendered_ns`, `u32 n_damage`, `u32 flags` (bit 0: has acquire fence), then `n_damage × {i32 x, y, w, h}` in buffer pixels, top-left origin | optional acquire sync_file |
| 132 | CURSOR_SHAPE | `u64 hash`, `u32 width`, `u32 height`, `i32 hot_x`, `i32 hot_y` (ARGB8888, premultiplied) | sealed memfd with the pixels |
| 133 | CURSOR_POS | `i32 x`, `i32 y` (buffer pixels), `u32 visible`, `u32 pad`, `u64 t_ns` | |
| 134 | HINT | `u32 kind` (1 anim start, 2 anim settle, 3 window move, 4 scroll), `u32 detail`, `i32 x, y, w, h`, `i32 dx, dy`, `u64 seq` | |
| 135 | STATS | `u64 frames`, `u64 dropped_no_buffer`, `u64 dropped_no_client` | |
| 136 | STOPPED | `u32 reason`, `u32 pad` | |
| 137 | CARET | `u32 visible`, `u32 purpose` (text-input-v3 content purpose; 8 password, 9 PIN), `i32 x, y, w, h` (buffer pixels), `u64 t_ns` | |
| 138 | POINTER_LOCK | `u32 state` (0 none, 1 locked, 2 confined), `u32 pad`, `i32 x, y, w, h` (buffer pixels) | |

Sizes (payload bytes): HELLO 4, START 40, RELEASE 8, FRAME_CLOCK 16, INPUT_POINTER_ABS 24, INPUT_POINTER_REL 40, INPUT_BUTTON 16, INPUT_AXIS 32, INPUT_KEY 16, INPUT_GESTURE 56, INPUT_RESET 0, LOCAL_INPUT 8; HELLO_ACK 36, BUFFERS 32 + 8 per plane, FRAME 32 + 16 per rect, CURSOR_SHAPE 24, CURSOR_POS 24, HINT 40, STATS 24, STOPPED 8, CARET 32, POINTER_LOCK 24. `Protocol.hpp` asserts every one.

## Rules

- **Session.**
  - HELLO is answered with HELLO_ACK, whose `version` is the lower of the two: that is the version spoken on this connection, and neither side sends messages of a higher one (v4; a v3 plugin answers 3). A HELLO with version 0 gets the plugin's own.
  - START begins capturing the named output: BUFFERS, then CURSOR_SHAPE and CURSOR_POS, then a FRAME of the whole output.
  - A START while capturing starts over (new buffers, new generation). STOP, or closing the socket, ends the capture and frees the buffers.
  - **STOPPED** says why capture ended without a STOP: 1 no such output (at START), 2 the output went away, 3 no exportable buffers (GBM/EGL), 4 unsupported (not the GL renderer, a transformed or mirroring output).
- **Buffers.**
  - **Format and size:** `DRM_FORMAT_XRGB8888`, sRGB-encoded, the output's size in pixels. The plugin converts in its copy from Hyprland's work buffer, whatever it is: 8-bit, or FP16 with the sRGB transfer function or linear (`render:fp16_sdr_tf = linear`, mapped back from Hyprland's 0.2–80 nit encoding).
  - **Modifier:** the ones Hyprland's EGL can render XRGB8888 into. With the LINEAR flag the plugin uses LINEAR only where the GPU can render into it; NVIDIA can't (LINEAR is external-only in its EGL, driver 615), so there BUFFERS carries a block-linear modifier anyway. `modifier` is always what you got.
  - All buffers of a generation share one layout, which is why BUFFERS lists the planes once.
  - **A new BUFFERS message** (with a new `generation`) is sent whenever the output's size or format changes. Old-generation buffers are dead after that; RELEASE of one is ignored.
  - **Ownership:** the plugin writes into a buffer only after `remoted` has released it. Every buffer starts out free. A release fence is waited on by the GPU, never by Hyprland's thread.
- **FRAME.**
  - `t_rendered_ns` is when the plugin queued the copy (CPU); the acquire fence signals when the GPU has finished it.
  - **Damage** is the region that changed since the previous FRAME *sent*, in buffer pixels: a frame dropped for want of a buffer or a full socket folds its damage into the next one. It is the whole output after START and after a new generation. More than 32 rects are sent as their bounding box. Damage can be larger than what changed (an app that redraws its whole window), never smaller.
  - **No frames when nothing changed:** cursor-only motion sends CURSOR_POS and no FRAME. The cursor is never drawn into the buffers (the plugin copies the frame before Hyprland draws its software cursor).
  - **A dropped frame is sent late, not lost** (v3): when a frame was dropped (no free buffer, or a full socket) and nothing changes after it, the plugin sends its content as soon as a RELEASE frees a buffer, or retries a few milliseconds later. So the last frame of an animation always arrives.
- **Cursor.**
  - CURSOR_SHAPE is sent at START and whenever the image changes (deduplicated by `hash`). Its pixels are the cursor as Hyprland draws it on this output: `width × height` at the output's scale, hotspot in those pixels.
  - CURSOR_POS is the hotspot's position in buffer pixels, sent when the pointer moves (and at most once per frame otherwise). `visible` is 0 when the pointer is on another output (then `x, y` may lie outside the buffer) or the cursor is hidden. Under a pointer lock (POINTER_LOCK 1) none is sent: the client is in relative mode, and Hyprland moves its cursor by each relative delta for an instant before the lock puts it back. The unlock sends one, where the lock held the pointer (v3).
- **STATS** come once a second while capturing. `dropped_no_buffer`: frames with no free buffer. `dropped_no_client`: FRAMEs not sent because remoted's socket was full.
- **FRAME_CLOCK** (headless outputs only) moves the output's frame schedule so frames finish on a grid: at times `t` with `(t − phase_offset_ns) mod period == 0` on `CLOCK_MONOTONIC`, where `period = 10¹²/refresh_mhz` ns (`refresh_mhz` 0: the output's own). `remoted` maps the client's vsync grid onto the host clock and subtracts its margin to get `phase_offset_ns`. An offset of 0 means the output's natural schedule.
  - While frames follow each other, each one starts its measured render time before its grid time.
  - **After idle** (damage arrives while no frame is scheduled), the frame waits for its grid slot, with one exception: if it is at most `grace_ns` past the start time of the slot just missed, it renders at once for that slot (and finishes up to `grace_ns` late). `grace_ns` 0 always waits. `remoted` sets it to the margin left after the measured jitter.

### Input (64–70)

Input goes into Hyprland's seat through two devices the plugin adds while `remoted` is connected, `sodiumos-remote-pointer` and `sodiumos-remote-keyboard` (`hyprctl devices` lists them). They behave like hardware: Hyprland's keybinds, gestures, focus rules (pointer enter and leave, click to focus), the input-method grab and pointer constraints all apply, and the keyboard gets the host's xkb layout. They are made at the first input message and removed when the connection closes.

- **Timestamps:** `t_ns` is when the event happened on the client, mapped by `remoted` onto this host's `CLOCK_MONOTONIC`. Wayland clients get it in milliseconds; keeping the client's spacing (rather than the arrival times) keeps kinetic scrolling and double clicks right over a jittery network. 0, a time in the future, or a time more than one second old means "now"; times never go backwards per device.
- **INPUT_POINTER_ABS** moves the pointer to a point of the captured output, in buffer pixels (the same space as FRAME and CURSOR_POS). It needs a capture running; without one it is dropped. Under a pointer lock Hyprland keeps the pointer where the lock holds it.
- **INPUT_POINTER_REL** is relative motion for pointer lock (games), in logical pixels like a mouse's libinput deltas: `dx, dy` move the pointer (accelerated by the client), `dx_unaccel, dy_unaccel` go to relative-pointer clients unchanged.
- **INPUT_BUTTON:** evdev codes (`BTN_LEFT` 0x110, `BTN_RIGHT` 0x111, `BTN_MIDDLE` 0x112, `BTN_SIDE`, `BTN_EXTRA`). A press of a button already down, or a release of one that is up, is ignored.
- **INPUT_AXIS:**
  - **Wheel** (source 0, or 3 for tilt): `value120` per notch (±120; high-resolution wheels send fractions), and `value` in logical pixels, 0 meaning `value120 / 8` (15 px a notch, as Hyprland's own wheels).
  - **Trackpad** (source 1, finger): `value` in logical pixels, `value120` 0. End each scroll with an event with flag bit 0 (**stop**) on each axis that moved: clients start kinetic scrolling on `axis_stop`.
  - **Continuous** (source 2) is the same for other smooth devices.
  - **Flag bit 1 (more):** the next INPUT_AXIS belongs to the same `wl_pointer.frame` (both axes of one trackpad event). Without it, the frame ends after this event.
- **INPUT_KEY:** evdev codes; `remoted` maps the client's keys. A press of a key already down is ignored, so the client must not send its own key repeat: Wayland clients repeat keys themselves from the host's `repeat_rate` and `repeat_delay`. Modifiers are tracked from the keys, as a real keyboard's are.
- **INPUT_GESTURE:**
  - **Swipe:** begin (with `fingers`), updates with `dx, dy` (finger motion since the last update, in libinput's touchpad units, about logical pixels), then end, or cancel. Hyprland's gestures use them (sodiumwm's 3-finger space switch: `gestures:workspace_swipe_distance`, 300 by default, is one space), and pointer-gestures clients get them.
  - **Pinch:** the same, plus `scale` (relative to the begin, 1.0 = unchanged) and `rotation` (degrees clockwise since the last update).
  - **Hold:** begin and end (or cancel); no updates.
  - A new begin while a gesture is active cancels the active one first.
- **INPUT_RESET** releases every key and button this connection holds down, stops a trackpad scroll in progress and cancels an active gesture. The plugin runs it by itself when the connection closes, so a dropped client can't leave Ctrl held down.
- **Remote input never powers a monitor up** (v4; home mode Off). With `misc:mouse_move_enables_dpms` or `key_press_enables_dpms` on, Hyprland wakes every monitor on input. The plugin's devices, and keys the input method forwards for them, don't; the desk's own keyboard and mouse still do. Remote input still counts as activity for ext-idle-notify (an idle daemon must not lock or suspend a session in use); a daemon that runs a monitor-less `dpms on` when activity resumes would wake the desk, so give it a monitor.

### LOCAL_INPUT (71, v4): the privacy screen

The desk's own keyboards and pointers, for home mode Privacy.

- **0 normal** (the start of every connection).
- **1 ignored:** every keyboard and pointer except the plugin's devices is ignored, and the input method's virtual keyboard keeps working (it forwards the remote keys it doesn't use). Keybinds don't fire from the desk either. Devices plugged in later (a USB mouse, a virtual pointer) are ignored from their first event. Touchscreens and tablets aren't covered.
- **2 privacy:** as 1, with **two logical keyboard focuses**:
  - **Desk keys never touch the seat's focus.** They go straight to the shell's privacy screen: its own client, a mapped layer surface with namespace **`sodiumshell-privacy`** and keyboard interactivity `none`. As soon as the mode is 2 and such a layer is mapped (whichever comes second) the plugin sends that client's `wl_keyboard`s the keymap and an `enter` with the desk keyboard's current modifiers, so the first desk key arrives after the client has focus (a client that gets the `enter` and a key in one dispatch can drop the key: Qt does). A `wl_keyboard` the client binds later gets the same, and a change of the seat's keyboard focus (which leaves every keyboard) is answered with an `enter` again. From then on the desk's keys and modifiers go only there, and a `leave` follows when the layer unmaps, the session locks or the mode ends.
  - **Remote keys keep the seat's focus,** wherever the remote user put it, so they never reach the privacy screen. Desk keybinds never fire.
  - Without a mapped privacy layer, mode 2 is mode 1.
  - **While the session is locked**, keys are routed by source between the lock client's two surfaces, through the seat: a desk key goes to the desk's lock surface (the main display's, the output that isn't REMOTE-1), a remote key to REMOTE-1's. When the seat's focus is on the other surface, the plugin moves it first (`leave`, `enter`, with the keymap and modifiers of the keyboard that types) and holds the key; the key goes out 15 ms later, in a dispatch of the client's own (the same Qt behaviour as above: a key that comes in the dispatch of the `enter` is dropped, and the lock's form lost the first characters of a password). A key that comes within 15 ms of any change of the seat's focus waits the same way, Hyprland's own moves included (it puts the focus on the lock surface under the remote pointer whenever that surface changes). Whatever arrives meanwhile, keys and modifier changes from either source, waits behind it and goes out in the order it came, each key that needs another move waiting for it in turn; nothing is dropped or reordered. The desk pointer stays ignored, so an unlock at the desk is keyboard-only.
- **Back to 0 3 s after the connection closes**, so a crashed `remoted` can't lock the desk out, and the shell gets the time to lock the desktop first: with the lock up, mode 2 sends the desk's keys to the desk's lock form, so nobody is locked out meanwhile either. A LOCAL_INPUT from a new connection ends the wait at once (and sets its own mode). `hyprctl remotelocalinput` says `lingering` while it lasts: `mode 2, lingering, ignoring 1 keyboards …`. No new message: still v4.
- Desk keys post a Hyprland event (socket2), `remotelocalinput>>keyboard`, whether ignored or delivered to the privacy screen (so the remote owner learns that someone types at the desk); an ignored desk pointer posts `remotelocalinput>>pointer`. At most once a second each, for the shell.
- `hyprctl remotelocalinput` answers the mode and what happened to the desk's input: `mode 2, ignoring 1 keyboards and 0 pointers, privacy layer entered, keys ignored 0, keys to the privacy layer 3, keys to the desk's lock 0, pointer events ignored 0`.

### HINT (134)

What the compositor knows about motion, for rate control. Hints are advice: damage stays the truth about what changed.

- **1 anim start / 2 anim settle** bracket the animations in Hyprland's animation manager whose window, space or layer is on the captured output (window open and close, moves and resizes, fades, space slides, special workspaces, layer surfaces coming in and out), windows drawn through a transformer that move (sodiumwm's overview glide, wobble), and announcements made with `hl.plugin.sodiumwm.remote_animation(seconds)` (for animations the shell draws itself, like the launcher's: the animation runs for the next `seconds`, at most 10; 0 ends it; it can't be left on by a caller that dies). Looping animations (`borderangle` with style `loop`) don't count.
  - **anim start** comes as soon as one begins: `rect` is what animates so far, `seq` the first FRAME that shows it (it arrives before that FRAME).
  - **anim settle** comes after the FRAME that shows the final state: `rect` is the union of everything the animation touched (where to refine), `seq` that FRAME. If a new animation begins before the old one settles, it is the same animation.
  - `detail`: which sources took part: bit 0 windows, 1 spaces, 2 layers, 3 transformed windows, 4 announced. A start names the sources at that moment; one that joins later (the shell announcing a popover's motion while Hyprland's own fade of its layer is running) sends no start of its own, and shows in the settle's `detail`. An announcement alone (nothing else moves) starts with bit 4.
- **3 window move** is a window whose content moved between two FRAMEs without changing size: dragged, animated to a new place, or riding a space slide. `rect` is where it is in FRAME `seq` (clipped to the buffer), and `(dx, dy)` how far it moved since FRAME `seq − 1`, in buffer pixels: the pixels at `rect` in FRAME `seq` were at `rect − (dx, dy)` in the one before.
- **4 scroll** (best effort): the trackpad scroll injected since the last FRAME, for the surface under the pointer: `rect` is that surface, `(dx, dy)` is `−value` in buffer pixels. The app decides how far it really scrolls, and kinetic scrolling after the stop isn't seen, so it is a candidate vector for the encoder's search, not a fact.
- **Motion batches:** kinds 3 and 4 come right before the FRAME they describe, with `detail` = the index in the batch. Index 0 starts a new batch and replaces any earlier batch (one for a FRAME that couldn't be sent); a hint with an empty rect only starts an empty batch. Rects may overlap and may include parts other windows cover.

### CARET (137)

The focused text field's caret, which only the compositor knows for apps that speak text-input-v3 or v1 to it (GTK, Chromium, Firefox, kitty, foot, Qt with `QT_IM_MODULE=wayland`; `remote/docs/ime-and-dnd.md`).

- Sent while capturing, when it changes, and at START. The plugin checks it at every frame of any output, when the keyboard focus changes, and every 100 ms.
- `x, y, w, h` is the caret's rectangle in buffer pixels (the app's `set_cursor_rectangle`, moved with its surface onto the captured output). `w` is often 0.
- `visible` is 0 when no text input is focused and enabled, when the app gave no rectangle, or when the caret is on another output.
- `purpose` is the text-input-v3 content purpose (`zwp_text_input_v3`): 8 password and 9 PIN mean secure input on the client. It is sent whenever a text input is focused and enabled, also with `visible` 0 (a field that gave no rectangle); 0 otherwise.
- Apps on fcitx5's D-Bus frontend (Qt with `QT_IM_MODULE=fcitx`, the live setting) don't tell the compositor their caret; the fcitx5 add-on gets it from fcitx5 and maps it with `hyprctl remotecaret X Y W H SCALE`, which the plugin still answers (`remotecaret` alone gives the text-input caret, as `OUTPUT X Y W H` in that output's pixels, or `none`).

### POINTER_LOCK (138)

The pointer constraint (pointer-constraints-v1) of the surface under the pointer, sent while capturing when it changes, and at START.

- **1 locked:** the pointer doesn't move. The client should hide its cursor, capture the mouse and send INPUT_POINTER_REL. `rect` is the locked surface.
- **2 confined:** the pointer stays inside `rect` (the bounds of the confinement region). Absolute moves still work; the client may keep its own cursor inside it.
- **0 none.** The lock ends when the app releases it, the surface loses focus, or the window closes.
- The rect follows the surface while it moves (one message per change). Hyprland activates a constraint only while its surface has both pointer and keyboard focus. Window-rule confinement (`confine_pointer`) isn't reported, only the app's own constraints.

## Changes

- v4 (for host-session's home modes):
  - **LOCAL_INPUT (71)**: the desk's input ignored for the privacy screen. In mode 2, two logical focuses: desk keys to the `sodiumshell-privacy` client's own keyboards, remote keys through the seat; while locked, routed by source between the lock's two surfaces. Back to normal 3 s after a disconnect (a new connection's LOCAL_INPUT ends the wait sooner); `remotelocalinput` socket2 events and `hyprctl remotelocalinput`.
  - **Remote input never powers a monitor up** (home mode Off), the desk's input still does.
  - **Version negotiation:** HELLO_ACK answers min(HELLO's version, the plugin's), so a v3 `remoted` (which refuses a plugin newer than itself) keeps working with a v4 plugin. A `remoted` that wants LOCAL_INPUT says 4 in HELLO.

- v1: first version.
- v3:
  - INPUT_* messages 64–70, injected at the seat through two plugin devices; INPUT_RESET runs on disconnect too.
    - Refined: INPUT_AXIS flag bit 1 (**more**) groups both axes into one `wl_pointer.frame`; `value` 0 on a wheel means `value120 / 8`; `t_ns` is host `CLOCK_MONOTONIC` with "now" for 0 or out-of-range times; repeated key and button presses are ignored; REL deltas are logical pixels.
  - FRAME_CLOCK's pad becomes `grace_ns`. FRAME_CLOCK is no longer a prototype.
  - CARET (137): the focused text-input caret, which only the compositor sees, with its content purpose.
  - POINTER_LOCK (138): so the client can switch to relative mode. Refined: it also carries the locked surface or the confinement's bounds (`i32 x, y, w, h`; 24 bytes instead of 8).
  - HINT (134), refined: the draft's `pad` is `detail` (the sources of an animation, or the index in a motion batch), and a `u64 seq` is added (40 bytes instead of 32), so a hint names the FRAME it belongs to. Motion hints (3, 4) come in batches right before their FRAME.
  - A dropped frame is now sent late when nothing changes after it (before, the stream could stay on the frame before it until the next change).
  - No CURSOR_POS while the pointer is locked; the unlock sends one. `hl.plugin.sodiumwm.remote_animation(seconds)` announces the shell's own animations.
- v2:
  - START flag bit 0 is now **prefer** LINEAR: NVIDIA can't render into LINEAR XRGB8888, so the plugin falls back to its renderable modifiers and BUFFERS says which.
  - New message **136 STOPPED** with a reason (no output, output removed, allocation failed, unsupported): v1 had no way to say START failed or the output went away.
  - **FRAME_CLOCK**'s phase is an absolute phase on the host's `CLOCK_MONOTONIC` (v1 said "relative to the client's vsync grid", which the plugin can't see); `remoted` does the clock mapping.
  - Clarified, no layout change: CURSOR_POS is the hotspot; CURSOR_SHAPE is at the output's scale and sent on change; damage folds over dropped frames and is never smaller than the change; `t_rendered_ns` is the CPU queue time; STATS once a second and what `dropped_no_client` counts; one plane layout per generation; a START while capturing starts over; `$SODIUMWM_REMOTE_SOCKET`.
