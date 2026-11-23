# Input method and drag and drop

How the Mac's own input method and Finder drag and drop reach host apps on a SodiumOS host. Results measured in a nested Hyprland 0.57 with fcitx5 5.1.23, GTK 4.24 and Qt 6.12.

- **Input method:** the fcitx5 addon `remote/fcitx5-remote` works on every frontend the live desktop uses. Preedit shows inline, commits land, the caret comes back in REMOTE-1 pixels, and Deactivate gives every field its own engine back. Carets of text-input-v3 apps are known **only to the compositor**, so the sodiumwm plugin has to answer one small query.
- **S7b, into apps:** recommended and prototyped. `remoted` maps a transparent overlay layer surface, the plugin injects a press on it, the drag starts with that press's serial, and the overlay turns click-through. Drops worked into a GTK4 window and into our Files app (Qt Quick).
- **S7b, out of apps:** feasible without new plugin code, using an edge "catcher" layer surface that becomes the host drag's target while the Mac pointer is outside the window. Not built yet.

## 1. The input method

### How it fits

```
Mac: NSTextInputClient ── marked text / commits ──► remoted ── D-Bus ──► fcitx5 + "remote" engine ──► app
                                                       ▲                    │ preedit inline (text-input-v3,
Mac: candidate window at the caret ◄── CaretRect ──────┘                    │ GTK/Qt fcitx modules)
                                                                            ▼
                                                       Hyprland ◄── remotecaret (sodiumwm plugin hook)
```

During a session the Mac's input method composes the text and the host only displays it. The engine is called `remote`. While a session is on it is the active fcitx5 engine in every field, so Rime never composes at the same time. Keys the Mac's input method doesn't consume pass through it untouched, so shortcuts keep working.

### D-Bus interface

`remoted` talks to the engine on the session bus. The engine sits on fcitx5's own connection, which also owns the name.

- **Bus name:** `org.sodiumos.Remote.Ime`
- **Object:** `/org/sodiumos/Remote/Ime`
- **Interface:** `org.sodiumos.Remote.Ime`

| Member | Signature | Meaning |
|---|---|---|
| `Activate()` | | Session on. Every input context switches to the `remote` engine, and its previous engine is remembered. Contexts created later (new windows) join as well. Calling it again does nothing. |
| `Deactivate()` | | Session off. Every context gets its own engine back, so a field that was on Rime is on Rime again. Calling it with no session does nothing. |
| `SetPreedit(s text, i cursor) → b` | | Shows `text` as the focused field's preedit, underlined and inline in the app. `cursor` counts characters (code points) and is clamped to the text; -1 hides the caret. Empty text clears the preedit. Returns false when no field has focus or the text isn't UTF-8. |
| `Commit(s text) → b` | | Clears the preedit and commits `text` to the focused field. Same return value as `SetPreedit`. |
| `Active` | property `b` | Whether a session is on, for a `remoted` that restarts mid-session. |
| `CaretRect(i x, i y, i w, i h)` | signal | The focused field's caret, in **pixels of REMOTE-1** (the output it is on), top-left origin. Sent only when the value changes. `w` is often 0. |
| `FocusChanged(b focused, b password)` | signal | A text field gained or lost focus. `password` means the Mac should switch to secure input with its input method off. fcitx5 keeps every engine off in password fields anyway. |
| `Reset()` | signal | The app ended the composition we had shown, for example on a click in the field. The Mac should **discard** its marked text, not commit it: Qt commits the visible preedit itself on a click. |

**Mapping from the Mac** (for the client):
- `setMarkedText(_:selectedRange:replacementRange:)` → `SetPreedit(text, selectedRange.location)`. Convert from UTF-16 offsets to characters.
- `insertText` → `Commit`.
- `unmarkText` → `Commit` of the current marked text.
- `firstRect(forCharacterRange:)` → the latest `CaretRect`, mapped from stream pixels to screen points.
- On `Reset` → `NSTextInputContext.discardMarkedText()`.

Clause highlighting (the selected range's length) isn't carried yet: the whole preedit is underlined.

### Where the caret comes from

Only the compositor knows where a surface is, and on one of the two paths only the compositor knows the caret at all.

| App talks… | Who knows the caret | How the engine gets REMOTE-1 pixels |
|---|---|---|
| text-input-v3 or v1 to Hyprland: GTK by default (the live setting), Chromium, Firefox, kitty, foot, Qt with `QT_IM_MODULE=wayland` | **Hyprland only.** fcitx5's input-method-v2 never carries a cursor rectangle: its input context stays `0,0 0x0` even while Rime's popup shows. | It asks Hyprland: `remotecaret`. This runs 50 ms after a preedit change, a commit, focus, or a surrounding-text update, so the app has drawn the change. |
| fcitx5's D-Bus frontend: Qt with `QT_IM_MODULE=fcitx` (the live setting, including quickshell), GTK with `GTK_IM_MODULE=fcitx`, SDL | fcitx5, relative to the app's focused surface, in that surface's pixels (Qt: device pixels at 1.6; GTK: at its integer scale 2) | It asks Hyprland to map it: `remotecaret X Y W H SCALE`, relative to the keyboard-focused surface. |

**What the plugin must provide: one hyprctl command, `remotecaret`.** About 40 lines.

**Done in M1 (merged 7fd96e9):**
- sodiumwm's `remote` module registers `hyprctl remotecaret` (both forms) and also pushes the caret to `remoted` as capture-ipc v3's CARET message, with the content purpose (8 = password).
- **Never load a standalone caret plug-in alongside sodiumwm:** both register `remotecaret`.

- **No arguments:** returns the focused text input's cursor box (`g_pInputManager->m_relay.getFocusedTextInput()->cursorBox()`, relative to the surface).
- **`X Y W H SCALE`:** maps that box relative to `g_pSeatManager->m_state.keyboardFocus`.
- **How it maps:** both forms add the surface's global box (`CWLSurface::getSurfaceBoxGlobal()`, which counts client-side shadows correctly), then convert to the pixels of the monitor that contains the point.
- **Reply:** `OUTPUT X Y W H`, or `none`.

The engine reads it over Hyprland's command socket with a 100 ms timeout, so a stuck compositor can never freeze typing. The production plugin should also push a change on every text-input commit instead of being polled; the Mac only needs the caret while it composes.

### Engine switching, exactly

- **Joining the group.** fcitx5 only switches a context to an engine that is in the current group. On `Activate` the engine adds itself to the end of the user's group in memory, then sets every context to `remote` locally. Changing the group resets every context's state, so the engine records each context's engine first (`Instance::inputMethod`). `Deactivate` removes it from the group again, then puts each recorded engine back.
- **One small loss.** A context that was inactive (English) comes back inactive. Its remembered "last input method" for the next toggle becomes the group default; on the live desktop that is Rime, the same thing.
- **Mid-session saves.** fcitx5 saves its profile when it quits, and a config tool can save it too. If either happens mid-session, the saved group lists `remote`. When the engine loads, it removes any leftover `remote` from every group once the profile has loaded. This was checked by quitting fcitx5 mid-session: the saved profile had `Name=remote`, and the next start logged `removed a leftover "remote" from group Default`, leaving the group as `keyboard-us, rime`.
- **Password fields.** fcitx5 itself keeps every engine off in them (`AllowInputMethodForPassword` is off). `SetPreedit` and `Commit` still act on whatever field has focus. Not sending text into password fields is the Mac's job, through secure input.

### Results (nested Hyprland, REMOTE-1 2560×1664 at scale 1.6)

The demo script plays `remoted` against four windows: GTK4 and Qt6, each on text-input-v3 and on its fcitx module. Before the session, the GTK field was on Rime (in the test bed's own fcitx5) and the other three were on keyboard-us. All 16 scripted checks passed, and the four carets were checked on the screenshots:

| | GTK4 · text-input-v3 (live) | Qt6 · fcitx module (live) | GTK4 · fcitx module | Qt6 · text-input-v3 |
|---|---|---|---|---|
| `Activate` → engine `remote` | ✓ | ✓ | ✓ | ✓ |
| "ni hao" preedit drawn inline | ✓ underlined | ✓ dashed (Qt's style) | ✓ | ✓ |
| `CaretRect` on the drawn caret | ✓ 259,123 0×102 | ✓ 1509,132 16×77 | ✓ 2139,931 0×102 | ✓ 1509,939 16×77 |
| `Commit("你好")` in the field | ✓ | ✓ | ✓ | ✓ |
| `Deactivate` → own engine | ✓ rime | ✓ keyboard-us | ✓ keyboard-us | ✓ keyboard-us |

More checks:
- **Rime is really back:** after `Deactivate`, typing "nihao" in the GTK field brings up Rime's 你好 candidates.
- **The caret follows a click** into the middle of the text mid-composition. GTK moves the preedit there rather than resetting, and `CaretRect` follows it.
- **`Reset()` fires** on a click in the Qt field mid-composition. Qt commits the visible preedit itself.
- **Password fields:** a GTK4 password entry gives `FocusChanged(true, true)` and engine keyboard-us.
- **Headless test:** `remote/fcitx5-remote/test/testremote.cpp` (fcitx5's test frontend) covers activate and restore (a field created mid-session included), preedit and commit, character-based cursors, and the boundaries (bad UTF-8, no focused field, `Deactivate` with no session).

The evidence files (in the session's scratchpad `remote/ime/run/`) are:
- `*-1-caret.png`: the red box is the `CaretRect` as reported;
- `*-1-preedit.png` and `*-2-commit.png`;
- `after-deactivate.png`, `gtk-3-rime-back-crop.png` and `gtk-click-caret-overlay.png`;
- `ime-demo.txt`, `signals.log` and `fcitx5.log`.

### Applying it live (with the user present)

1. **Build:** `cmake -S remote/fcitx5-remote -B remote/fcitx5-remote/build -DCMAKE_INSTALL_PREFIX=$HOME/.local -DBUILD_TESTING=OFF && cmake --build remote/fcitx5-remote/build && cmake --install remote/fcitx5-remote/build`. This installs `~/.local/lib/fcitx5/libremote.so` and `~/.local/share/fcitx5/addon/remote.conf`.
2. **Let fcitx5 find the library:** add `hl.env("FCITX_ADDON_DIRS", os.getenv("HOME") .. "/.local/lib/fcitx5:/usr/lib/fcitx5")` next to the other fcitx5 variables in `compositor/lua/chinese-input.lua`. The other option is a system install into `/usr/lib/fcitx5` through a PKGBUILD.
3. **Restart fcitx5.** This is the user's call: it restarts their input method. The engine stays inert until `Activate`.
4. **Move the caret hook** into the sodiumwm plugin .

**What's missing:**
- carets of X11 apps (absolute coordinates over XIM), which aren't mapped;
- clause highlighting;
- a push-based caret from the plugin.

## 2. Drag and drop into apps (Finder → host)

### The problem

The spec says that after a Finder drop on the stream window, the files land in a staging folder and `remoted` "performs a real Wayland drag-and-drop at the drop point". But `wl_data_device.start_drag` takes the serial of an implicit grab: a button press on a surface of the dragging client itself. A daemon has no such press.

### Approaches

| Approach | Plugin work | Verdict |
|---|---|---|
| **A. Overlay layer surface + injected press** (prototyped). Map a transparent, full-output `zwlr_layer_surface_v1` on the overlay layer with no keyboard interactivity. Inject pointer motion and a button press at the drop point; the press lands on the overlay, so the app below sees no click. `start_drag` with that serial, offering `text/uri-list`. Set an empty input region, so the app under the pointer becomes the target. Nudge the pointer, then release. | None new: the seat injection the host already plans (absolute motion, button) | **Recommended.** Standard protocol, works with every toolkit, and stays out of Hyprland's internals. |
| B. Plugin-side drag: the plugin implements Hyprland's `IDataSource` and starts the drag from inside the compositor | A data source, the drag start, and the button-release handling, all against Hyprland internals that change every release | It needs no surface or press, but it's deep coupling for no user-visible gain. Keep it as the fallback if A ever breaks. |
| C. No drag: drop on Files or the desktop puts the files in that folder; anywhere else they go to `~/Downloads` with a toast | none | Already the spec's fallback. Keep it for apps that reject the drop. |
| D. Clipboard: put `text/uri-list` on the clipboard and send Ctrl+V | none | Only file managers paste files. No. |
| E. Per-app D-Bus (FileManager1, portals) | none | Too narrow. No. |

### Results (nested)

The drop demo dropped `report final.pdf` and `笔记.md`, a name with a space and a CJK name, sent percent-encoded:

- **GTK4 drop target** (`Gtk.DropTarget`, `Gdk.FileList`): it got `enter`, highlighted while hovered, accepted `text/uri-list` and received both paths. Steps logged: overlay mapped → press (serial) → `start_drag` → target accepts → release → target reads → dropped → finished. Press to finish took about 0.5 s with no hold.
- **Files (`apps/files`, Qt Quick `DropArea`)**: it showed its own hover hint ("Move to Inbox · hold Ctrl to copy"), and both files appeared in the folder.
- **A finding about the Files app:** Files *moved* the files out of the source folder although the source offered only `copy`. It ignores the offer's allowed actions. That's harmless for Remote, whose staging folder is scratch space, but `remoted` must not expect staged files to survive a drop.
- **No `wl_data_source.action` event arrived** with either target. Don't rely on it; rely on `dnd_finished` and `cancelled`.

Evidence: `dnd-{gtk,files}-1-hover.png`, `dnd-{gtk,files}-2-dropped.png`, `dnd-*.log` and `dnd-demo.txt`.

### What `remoted` does in production (approach A)

1. The Finder drop arrives: the client sends the drop point and the file list, and the files stream into the staging folder.
2. `remoted` maps its overlay and asks the plugin to inject motion and a press at the drop point (`INPUT_*` in `capture-ipc.md`).
3. It starts the drag with the press serial it receives on its overlay. It sets the overlay click-through and injects a small motion, then the release.
4. It serves `text/uri-list` when asked.
5. On `dnd_finished` it unmaps the overlay. On `cancelled` it falls back to approach C.

If `send` comes before a file has fully arrived, `remoted` can hold the write: GTK and Qt read the offer asynchronously. The overlay lives for about half a second and never takes keyboard focus.

## 3. Drag and drop out of apps (host → Finder): feasibility

**The situation.** A host app starts a drag after a press that came from the Mac. The Mac pointer then leaves the stream window with the button still down: AppKit keeps sending `mouseDragged` to the window that got the `mouseDown`. The client then needs three things:
1. to know a host drag is active, and its MIME types;
2. the data, such as file paths from `text/uri-list` (then the files themselves), text or images;
3. a way to end the host drag cleanly once the Mac drop has happened.

**Recommended: an edge catcher, with no plugin code.**
- **Mapping it.** When the client reports "pointer left the window with the button down", `remoted` maps a small transparent layer surface (overlay layer) at the edge point where the host pointer now rests, and injects a 1 px move onto it. The compositor recomputes the drag target on motion, so the catcher gets `wl_data_device.enter` with the offer, and the MIME types come with it.
- **Getting the data early.** `wl_data_offer.receive("text/uri-list")` works during the hover, before any drop: GTK and Qt serve offers on request. So `remoted` learns the paths at once. The client then starts an `NSDraggingSession` with `NSFilePromiseProvider` items, or with plain pasteboard items for text and images.
- **The Finder drop.** The promise's writer fetches the files from the host over the control channel. Then `remoted` accepts the host drag on the catcher with **copy only** (`set_actions(copy, copy)`) and injects the release. The host app gets `dnd_finished` with copy and leaves its file alone.
- **A cancel on the Mac.** `remoted` injects the release without accepting, so the host drag is cancelled.
- **Coming back.** If the pointer re-enters the stream window, `remoted` unmaps the catcher and the host drag carries on into host apps.
- **Unknowns.** Drags from XWayland apps go through Hyprland's XDND bridge; whether the catcher sees them is untested.

**Alternative: a plugin hook on drag start.** For example, a function hook on Hyprland's data-device drag start, or polling its DnD state every frame. The plugin would read the source's MIME types and tell `remoted`, so the client knows *before* the pointer leaves and can show the drag image right at the edge. It can also request data through the source (fd passing). It's worth adding later as "drag active" and "drag ended" events for that polish, but it isn't needed to work.

**The risk to remember:** the host app may offer *move*, as Files does within one filesystem. The catcher must only ever accept *copy*, or a drag to Finder would delete the host file.

## 4. What the sodiumwm plugin needs to provide

| For | What | Size |
|---|---|---|
| Input method | `hyprctl remotecaret` (and `remotecaret X Y W H SCALE`), as in `caret-plugin/caret.cpp`. Later, push a caret change on every text-input commit. | ~40 lines |
| Drops into apps | Seat pointer injection (absolute motion, button), already planned as `INPUT_*` | nothing extra |
| Drags out of apps | Nothing is required. Optional: "drag active (MIME types)" and "drag ended" events. | ~60 lines, optional |

## 5. Running the tests

- **Headless:**
  ```
  cmake -S remote/fcitx5-remote -B /tmp/fr && cmake --build /tmp/fr && ctest --test-dir /tmp/fr
  ```
  It reaches no compositor and no bus.
- **Nested:** the test bed's scripts (`nested.sh start`, `ime-demo.sh`, `dnd-demo.sh`, `nested.sh stop`) are not part of this repository.
  - The test bed is one nested Hyprland with a headless REMOTE-1 output and the caret plugin loaded, a private session bus with no activatable services, and its own fcitx5. That fcitx5 has its own config, data and Rime under `$OUT`, and no X11, ibus, clipboard or tray addons.
  - The Files demo needs `ui/build.sh` and `apps/build.sh --target files` first.
  - The live fcitx5, the live bus and the live screen are never touched.

## 6. Open questions

1. **Live hover for Finder drags.** Should a Finder drag hovering over the stream already show host drop targets? The host drag would start when the Mac drag enters the window, the app could highlight, and the Mac could show accept or deny. The spec drops at the release point, after the transfer, which is what is built. Live hover is the same mechanism started earlier.
2. **Drags out always copy.** Should a drag from the host to Finder always copy, even when the host app would move (Files on one filesystem)? The proposal is always copy.
3. **The staging folder.** Where should it live and how long are files kept? Files moves dropped files out of it, so it mostly empties itself.
