# The Remote client app (SodiumOS "Remote", Windows "Sodium Remote")

The Qt/QML client for other computers, built on the Rust stream engine. One codebase: this is the SodiumOS build; the Windows port keeps the same QML, controller and C ABI and replaces the platform seam below.

```
app/                            the app (CMake target `remote`, app id org.sodiumos.Remote, binary `sodium-remote`; `SodiumRemote.exe` on Windows); it builds on its own: `cmake -S app -B build` (Sodium UI from ../sodium-ui, a vendored copy of the part this app uses, UPSTREAM and MANIFEST say where from); SodiumOS builds it from apps/ with its own ui/
  src/main.cpp                  arguments, the QML engine, the controller as the context property `Remote`
  src/controller.{h,cpp}        RemoteController: engine events -> models and properties; commands -> engine calls
  src/models.{h,cpp}            ComputersModel, NearbyModel, DisplaysModel (QAbstractListModels, ids and roles)
  src/remote_qt.h               the C ABI (hand-written mirror of remote/client-qt/src/lib.rs)
  qml/                          Main (computers window), ComputersPage, ComputerCard, AddSheet, CodeField, SettingsSheet,
                                StreamWindow, StreamBar, ConnectingCard, FailureCard, OsPill, Fmt (words and numbers)
  tests/                        tst_remote (models and event mapping), e2e.sh / matrix.sh (the nested bed), scripts/main.txt
remote/client-qt/               crate remote-qt: libremote_qt.a, the C ABI over remote-engine
  src/lib.rs                    platform-neutral: events as JSON, browse client (list, presence, pairing, by address), commands
  src/linux.rs                  the Linux seam: Platform for the engine, the embedded Wayland surface, session thread
remote/client-linux/src/window  `run_embedded`: the engine's video surface as a subsurface of Qt's toplevel (embed.rs)
remote/client-engine            `Platform::on_status` (SessionStatus) and `Stats::hud_numbers` were added for the app
```

## Build and run

```
ui/build.sh && apps/build.sh        # apps/build.sh builds everything, including `remote` (cargo builds libremote_qt.a first)
cmake --build apps/build --target remote tst_remote        # just this app
QT_QPA_PLATFORM=offscreen apps/build/bin/tst_remote        # the unit tests
sodium-remote [--data DIR] [--lan-only] [--script FILE]
```

`--data` (default `$SODIUM_REMOTE_DATA`, else `~/.local/share/sodium-remote/client`, copied on first start from the pre-rename `sodiumos/remote-client`; never the host half `sodium-remote/host`, so the client key is its own file and identity) holds this device's key, `paired.json` and `app.json` (settings; each computer's own name, system and last route). `--lan-only` skips the rendezvous server. Each computer's last picture is `thumbs/<id>.png` there (written by the engine at the end of a session; `thumbs/` is 0700 and each file 0600, and on Windows the profile's ACL is enough). A computer that is no longer in the paired list (removed here, or revoked from its side) loses its picture the next time the list arrives (`RemoteController::pruneThumbs`). `--script` is for tests and screenshots only (see `qml/Main.qml`, `runStep`) and is ignored unless `SODIUM_REMOTE_TEST=1` is set. A `sodium-remote:` link on the command line is the only argument used; options beside it are ignored, and the Windows handler is registered as `"exe" -- "%1"`. Needs a Wayland session (`QT_QPA_PLATFORM=wayland`) for streaming. Never run it against the live `sodiumos-remoted` from a test.

## The engine through the C ABI

Events go out as **JSON text** through one callback, from engine threads; `RemoteController::trampoline` queues each onto the Qt thread (`handleEvent`). Commands are plain calls that return at once. Events carry ids, enums and numbers, never translated text; `Fmt.qml` and the cards turn them into words.

| Call | Does | Events back |
|---|---|---|
| `rq_new(data, name, rendezvous, cb, user)`, `rq_free` | creates/destroys the engine; starts the *browse* client | `self`, `rendezvous` |
| `rq_refresh` | paired computers with presence, and computers with an open pairing window | `hosts` |
| `rq_pair(host, code, addr)` | SPAKE2 pairing | `pair` (`checking`, `wrong` + `left`, `withdrawn`, `closed`, `failed`, `ok`) |
| `rq_resolve_address(text)` | "Connect by address" | `address` (`found` + host id, `unreachable`) |
| `rq_forget(host)` | removes a pairing | `hosts` |
| `rq_decode_support()` | what this machine decodes (probed, never assumed) | (returns JSON) |
| `rq_start(opts, wl_display, wl_surface)` | a session: `remote_engine::client::run` on its own thread | `status`, `welcome`, `stream`, `secure`, `capture`, `leave_chord`, `session_over` |
| `rq_stop`, `rq_view`, `rq_hud_rects`, `rq_capture`, `rq_select_display`, `rq_key_passthrough`, `rq_send_cad`, `rq_stats` | session commands | |

**Two halves.** The *browse* half is a long-lived `remote_core::connect::Client` (list, presence, pairing). The *session* half is the engine's own `client::run`, whose own `Client` registers the same device identity; to keep one registration the browse client is shut down while a session runs and started again when it ends (`session_over`).

**Connect failures** arrive as `status`/`failed` with the engine's `ConnectError` (`busy` + `by`, `offline`, `not_paired`, `refused` + the host's reason, `timeout`, `failed`) and endings as `status`/`ended` with the `DisconnectReason`. Only reasons a host reports get their own card (in use, taken over, version mismatch, not trusted, host shut down or asleep, ended at the computer, hold expired); everything else is "Couldn't reach it" with the firewall hint line.

**Reason JSON** (`reason_json` in `client-qt/src/lib.rs`, the shape inside `ended` and `refused`): `{"kind": "control_off"}` for `DisconnectReason::ControlOff` (the host's switch went off; no text is ever matched), and `{"kind": "permission_missing", "screen_recording": bool, "accessibility": bool}` for `PermissionMissing` (a Mac host without a permission; the flags say which are missing, both may be true). `FailureCard.qml` shows one card for `permission_missing` today and ignores the flags; a card per permission can be worded from them. `last_connected` in `paired.json` is stamped by core when `Welcome` arrives, not by the app.

## The video surface

Video never goes through Qt's scene graph. On Wayland:

1. Qt owns the toplevel (a frameless transparent `Window`, alpha buffer on). The controller reads Qt's `wl_display` and the window's `wl_surface` from `QPlatformNativeInterface` (`nativeResourceForIntegration("wl_display")`, `nativeResourceForWindow("surface", window)`) and passes them to `rq_start`.
2. The engine wraps Qt's display (`Backend::from_foreign_display`), binds its own globals on its own event queue, creates a surface, and makes it a **desynchronised `wl_subsurface` placed below Qt's surface** at the video area's origin. The existing EGL/GLES presenter, `wp_presentation` stamps, vsync reports and local cursor draw into it unchanged. The subsurface has an empty input region, so the pointer and keyboard focus stay on Qt's surface.
3. The QML HUD (bar, menus, cards, toasts) is drawn by Qt above it; everywhere else the window is transparent, so the picture shows through.
4. **Input stays in the engine**, through its own `wl_seat` listeners on the same display (hi-res scroll with value120, relative pointer under lock, HID usages by evdev code, key-ups on focus loss: all as in the reference client). Events from Qt's surface are accepted as ours. `rq_hud_rects` tells the engine which rectangles belong to the app (the bar, or the whole window while a menu or a card is up): the pointer over them is neither sent nor locked.
5. **The local leave chord** Ctrl+Alt+Shift+Esc is matched by evdev scan code in the engine's key handler and never sent; it sends key-ups for held keys, turns capture off (`capture` event), and the QML leaves full screen with the bar shown. A button press in the video turns capture back on. "Send Ctrl+Alt+Del" (bar's keyboard menu) sends the HID chord Ctrl, Alt, Delete.

Known edges: Qt and the engine both set the pointer's cursor on enter; the engine re-applies its shape on the first motion. Qt reads the shared socket too, so a read there queues our events with no wake-up here; the video thread polls at 4 ms (embedded mode only). Fractional scales: the subsurface uses `ceil(devicePixelRatio)` as its buffer scale; `Hello` carries the real value as `pixels_per_point`.

An engine-drawn HUD as a fallback was not needed.

## Languages

All ten (en_US source; en_GB, en_AU, zh_CN, zh_TW, ja, ko, es, fr, de): catalog `remote` (`i18n/ts/remote_*.ts`, from `scripts/i18n.sh update remote`; `.qm` from `release`), launcher names in `desktop-entries` and the glossary term `app_remote` (ja リモート, de Fernzugriff, zh_CN 远程控制, zh_TW 遠端控制, ko 원격 제어, es Acceso remoto, fr Accès à distance, English Remote). Review state is `machine` (machine-translated, to be reviewed); en_GB and en_AU equal the source. OS names (SodiumOS, macOS, Windows), codec names and the route words LAN, VPN and Relay are not translated. The page glossary entry `page_remote` (Settings › Remote) equals `app_remote` in every language), so no label is Microsoft's product name; the old term (リモートデスクトップ, Remotedesktop, 远程桌面, 遠端桌面, 원격 데스크톱, Escritorio remoto, Bureau à distance) stays only as a search keyword on the page and in the launcher.

## Testing

- **Unit** (`tst_remote`): the models keep rows in place, `hosts`/`pair`/`status`/`welcome` events fill the models and properties, a user disconnect is not a card, failure kinds. Offscreen, no engine.
- **End to end, nested only** (`app/tests/e2e.sh`, `matrix.sh`): `tests/e2e/run.sh DIR --host-only` is the nested SodiumOS host (its own Hyprland with the plugin, a private bus, remoted); `e2e.sh` starts a second nested Hyprland with a headless 1600×1000 output, runs the app in it with `--script`, pairs through the app's own UI (the script types the code through `Remote.pair`, as the code field does), streams, and takes `grim` screenshots on the script's `log shot NAME` markers. `matrix.sh` does every language and theme, each against a freshly started nested host (a session that fails leaves the nested capture broken until the host restarts). The screenshots are `build/e2e/<lang>-<theme>/shots/`.
- `SODIUMOS_I18N_AUDIT=1` is on in the bed: the overflow audit logs to `app.log`.

## The Windows port (done)

`SodiumRemote.exe`, built by `packaging/windows/stage-app.sh` (see [the Windows packaging README](../../packaging/windows/README.md)).

- **Platform layer:** `client-qt/src/windows.rs` wraps `client-windows` (now a lib plus the bin). A Qt-created stream frame (`app/src/streamframe.cpp`) holds the engine's D3D11 swapchain on a child HWND; `StreamWindow.qml` stays the HUD, an owned transparent non-activating window clipped by `SetWindowRgn` to the HUD rectangles (`Remote.setHudRects`). Input is the engine's Raw Input; the `WH_KEYBOARD_LL` hook is installed only while the frame is foreground and capture is on; the leave chord Ctrl+Alt+Shift+Esc is matched by HID usage.
- **Chrome and glass:** `ui/native/portable/windowchrome.cpp`, used by Sodium UI's `AppWindow` and `WindowControls`: thick frame with the NC area removed, `HTMAXBUTTON` hover and pressed states (snap layouts), Mica on the list and settings windows, immersive dark per theme, an opaque fallback below build 22621 or with transparency off. The stream frame is opaque, with `DWMWCP_DONOTROUND` in full screen.
- **This computer:** `hostcontroller.cpp` (the `Host` context property over `rh_*` in `client-qt/src/host.rs`, host-desktop's `Session`), `qml/ThisComputerPage.qml`, the tray (`tray.cpp`, `qml/TrayMenu.qml`), the notice (`qml/ControlledNotice.qml`), `winapp.cpp` (single instance, `--background`, start at login through the HKCU Run key). State in `%LOCALAPPDATA%\SodiumRemote`: the host half in `host\`, the client half in `client\` (two stores that share no file; `--data DIR` replaces the base, `SODIUM_REMOTE_DATA` the client half). On generic Linux the same page and controller exist when the app is built with `REMOTE_LINUX_HOST` (see [host-linux.md](host-linux.md)); without a tray, start at sign-in or firewall rows. On Linux the client store is `~/.local/share/sodium-remote/client`, apart from the host half `sodium-remote/host`; on the Mac the client is in `Remote/` and the host in `Remote/Host`.
- **Tray menu and notice:** the menu opens next to the icon on the side the taskbar is on (the icon's rectangle against the screen's work area, any edge); the notice still sits at the bottom right. The window bar carries the app icon and "Sodium Remote" before the tabs (the board's Windows chrome). "Address for connecting directly" shows IP:PORT (a bare address pairs); the full ID@IP:PORT is the "Copy full address" item of the ... menu (and its tooltip). With more than one address the same menu lists every IP:PORT as its own "Copy IP:PORT" item first.
- **The notice:** topmost, non-activating and a tool window by `WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` set on the HWND (`winapp.cpp`), so it never takes the focus; Disconnect and Dismiss. The tray menu and the notice use the opaque Graphite base when `WindowChrome.backdropActive` is false (below build 22621, or transparency effects off).
- **Window sizes and frame geometry:** the main window opens at 760x620 logical and the stream window on the main window's screen at 1100x760 logical, each at most 90% of its screen's available area and centred (`Remote.fittedWindowRect`, `RemoteController::fitWindow`). On Windows the frame HWND alone owns the geometry, in physical pixels. Opening places the HUD and the frame together (`Remote.placeStreamWindow`, `StreamFrame::place`). After that any change to the HUD's geometry (Qt around full screen, a monitor's scale) is put back on the frame, and the HUD's own moves and resizes (Qt's `startSystemMove`/`startSystemResize` from the bar, posted as `WM_SYSCOMMAND` `SC_MOVE`/`SC_SIZE`) are re-posted to the frame by a native event filter. `WM_DPICHANGED` uses Windows' suggested rectangle, `WM_GETMINMAXINFO` gives 640x420 logical at the window's DPI, and full screen is the frame's monitor rectangle (leaving restores the saved windowed or maximised rectangle). The picture's child window reaches the frame's client right and bottom edges and follows the frame's size on its own, not only Qt's view; its left, right and bottom 6 px (DPI-scaled) pass a press to the frame as a sizing drag, since a `HTTRANSPARENT` hit-test cannot cross threads.
- **Stream-bar menus:** while one is open, `StreamWindow.qml` turns capture off (the engine releases held keys) and calls `rq_menu_block`, which makes the engine drop everything but key-ups and button-ups (`blocked` in the embed state). A press in the video then goes nowhere but closes the menu (`video_click`), and Escape, the arrows and Enter reach the menu as `menu_key` events (the keyboard has no Qt focus: the frame is foreground and the HUD never activates). The frame losing the foreground closes it too. The HUD's region is the bar plus the open menu's rectangle and shadow, so the picture stays visible around it.
- **Not done:** SendSAS (Ctrl+Alt+Del, needs a service), Windows toasts, the installer.

## Decisions made while building

The design choices: connect by address, the browse and session split, the video-surface input design, the settings scope, the data directory, the OS pill.


## Settings: About, Advanced, previews (release prep)

- **About** (`qml/AboutPage.qml`, `qml/NoticesSheet.qml`): the version, `AGPL-3.0 or commercial license`, Source code (`brand::SourceUrl`, opened in the browser: the AGPL section 13 offer), Third-party notices (a sheet; "Open the notices file" opens `THIRD-PARTY-NOTICES(.md|.txt)` found beside the program, in `../share/doc/sodium-remote/` or in the source tree; cargo-about writes it at release, see `about.toml`) and the line that the app neither checks for updates nor sends usage data. `sodium-remote --version` prints the version, the license and the source URL.
- **Advanced › Computers › Show previews** (`Remote.showPreviews`, kept in app.json, on by default): off deletes `thumbs/`, stops the engine from writing pictures (`connectTo` leaves `thumb` out of the options) and the cards show their flat tile at the same size; a toast says so.
- **Advanced › Software video** (Windows only): "Download OpenH264 when needed" (`Host.softwareVideo`), the one-time notice dialog and the progress, failure and off banners on This computer, driven by the host's `codec` events (`rh_set_software_video`, `rh_codec_retry`).
- **Files the app finds beside itself:** Sodium UI (`SodiumUI/` next to the program, in `../lib/sodium-remote/`, or the build tree), `translations/remote_<lang>.qm` and `sodiumui_<lang>.qm` (next to the program, in `../share/sodium-remote/translations/`, or `build/i18n/qm` after `i18n/i18n.sh release`; the app points Sodium UI's `SODIUMOS_I18N_DIR` at the folders that exist), and its symbolic icons, which are in the program itself (resources at `:/icons/sodium-remote`, from `app/data/icons/sodium-remote/`) and listed in `QIcon::fallbackSearchPaths()`: Sodium UI's icon provider looks there after the system's SodiumOS theme, where there is one. Nothing changes `XDG_DATA_DIRS`.


## Settings › Server, server links, no-server states (release prep, phase B)

- **The tab** (`qml/ServerPage.qml`; tabs: Display, Quality, Input, Server, Advanced, About): with no server, "No server", what still works, **Add a server…**, "How to run your own server" (`brand::SelfHostingUrl`) and **Import…**; with one, its name (the host, or "Sodium Remote service" for `remote.sodiumos.com`), Reachable / Unreachable / Checking… (from the engine's `rendezvous` event; the engine reports no round trip, so none is shown), Change…, Remove, Address and Certificate rows, "This computer: Enrolled" while online, Import… and Export…. The app owns `server.toml`: `rq_server_*` in `client-qt` read and write the user's file (`dirs::user_config_dir()`: `~/.config/sodium-remote/`, `%APPDATA%\SodiumRemote\`), so the engine and, on Windows, "This computer" (`rendezvous: true` in `host.rs`) use the same server.
- **Saving** (`qml/ServerSheet.qml`): typed settings (address, optional CA file, optional token) are saved at once (D10); a pasted or opened link and an imported `server.toml` first show a confirmation (C3) with the host, the address, whether a certificate comes with it and whether a token is inside. Typed addresses go through `normalize_address` (core: bare host or `https://` become `wss://` plus `brand::DEFAULT_SERVER_PATH`; `http://` and `ws://` are refused, D13). `rq_restart_browse` starts the browse client again when no session runs; otherwise the tab says the new server is used when the session ends. On Windows "This computer" restarts when nobody is controlling it, else when that ends.
- **Links from outside:** the desktop entry has `MimeType=x-scheme-handler/sodium-remote;` and `Exec=sodium-remote %u`; a launch with a link hands it to the running app (`src/linkhandoff.cpp`, a socket in `$XDG_RUNTIME_DIR` named for the data folder) and exits. Windows writes `HKCU\Software\Classes\sodium-remote` at start (`WinApp::registerLinkScheme`) and a second launch sends the link to the first through the tray window (`WM_COPYDATA`). The window opens on Settings › Server with the confirmation.
- **No server, no relay words** : a relay path learned earlier is not shown on the cards, and the "Couldn't reach it" card says to add a server and offers **Server settings…**.
- **C9:** `token_invalid`, `token_expired`, `quota_exceeded`, `payment_required` and `host_not_allowed` (an enrollment token is needed) become the app's own banner in Settings › Server (`serverProblem` in the controller); the server's text is never shown.
- **File choosers** are Qt Quick Dialogs (`qml/ServerFiles.qml`, loaded when first used); the Windows package needs the `QtQuick.Dialogs` module with it.
