# Sodium Remote for Mac: both directions

Sodium Remote for Mac (`remote/client-mac/`): the client, plus this Mac as a host. [BUILD.md](../../client-mac/BUILD.md) is the build path; this file is what the app consists of and a checklist for checking a build on a Mac.

## What changed

| Area | Where | What |
|---|---|---|
| Packaging | `build-app.sh`, `Package.swift`, `App/Info.plist` | One static library, `libremote_ffi.a` (host-desktop with `ffi`, which includes core), from `host-desktop/build-swift.sh mac`; both Swift files in `Generated/RemoteCore/`; one `module.modulemap` listing both headers; the host's frameworks; `CFBundleName`/`CFBundleDisplayName` "Sodium Remote" and bundle ID `com.sodiumos.remote` for a release (the executable stays `Remote`), output `build/Sodium Remote.app`. **`build-app.sh --test-id` is the build for every test:** bundle ID `com.sodiumos.remote.dev`, name "Sodium Remote (test)" (untranslated), output `build/Sodium Remote (test).app`; Info.plist carries `__BUNDLE_ID__` and `__NAME__` which the script fills in. `--install` refuses to run (two apps with one bundle ID confuse LaunchServices; installing comes back for a release, once a release exists). |
| Strings | `App/Localizable.xcstrings`, `Tools/xcstrings-to-lproj.py` | 85 strings in ten languages (8 added after the first 77: the control-off and permission cards, "Copy full address"); the script writes `<lang>.lproj/Localizable.strings` into the app at build time and exits 1, naming the key, on any entry with plural, device or substitution variations (`Tools/test_xcstrings_to_lproj.py` tests it with Python alone). |
| Pure logic | `RemoteKit/HostPane.swift`, `Tests/.../HostPaneTests.swift` | `HostOS`, the display picker rules, the permission row mapping and view-only rule, `PresenceState`, `HostAddress`. |
| This computer | `RemoteUI/HostPaneModels.swift`, `HostPaneParts.swift`, `HostPaneView.swift` | The page and its model; no core types. |
| Identity | `RemoteKit/Brand.swift`, `Remote/Support.swift` (`AppIdentity`), `Migration.swift`, `CoreBridge.swift`, `Store.swift` | `Brand` mirrors `core/src/brand.rs`. The bundle ID is read at run time; one ending in `.dev` is the test build. The data folder (`Application Support/<bundle ID>`, the host's `Host`, `thumbs`), `Logs/<bundle ID>`, the Keychain service, the unified-log subsystem, queue labels and toolbar and notification identifiers all follow the bundle ID; UserDefaults and TCC do by themselves. `Migration.run()` (first thing in `main.swift`) moves the old folder (`Remote`, test `Remote-old`) once and copies the old preferences' `Remote…` keys; `KeychainStore` imports keys on first read (below). |
| Host binding | `Remote/HostController.swift` | `ThisComputer` and its listener on the main actor; the app keeps the switch, the name and Start at login. The host's `start()` runs at launch only when the saved switch is on. |
| Menu bar, notification | `Remote/PresenceController.swift`, `RemoteUI/PresenceViews.swift` | `NSStatusItem` that exists only while the switch is on (removed while off): solid, or accent with the dot while controlled, and a menu; `UNUserNotification` with Disconnect, time-sensitive only when `timeSensitiveSetting` is `.enabled` and the normal level otherwise; the board's own banner when notifications are denied. |
| Main window | `RemoteUI/HostListView.swift`, `Remote/HostListWindowController.swift`, `AppController.swift` | The bar holds "Sodium Remote", Computers / This computer, Pair and a gear. OS text pill on each card (from the remembered `Welcome.host_os`). |
| Stream window | `RemoteUI/StreamViews.swift`, `Remote/StreamWindowController.swift`, `StreamSession.swift`, `Support.swift` | Display picker (toolbar item and full-screen bar), the secure-screen card and amber chip and the "Back on …" toast, the cards for a host with its control switched off and for a Mac without Screen Recording or Accessibility (both typed, see below), the key table by host OS. |

### Release changes

| Area | Where | What |
|---|---|---|
| Identifiers | `Brand.swift`, `Support.swift`, `Info.plist`, `build-app.sh`, `Package.swift` | Bundle IDs `com.sodiumos.remote` / `.dev`; mDNS `_sodium-remote._tcp` (Info.plist `NSBonjourServices` and `LocalNetworkProbe`); `libremote_ffi.a`. |
| First-start move | `Migration.swift` | Data folder moved (only when it holds `paired.json`, `thumbs` or `Host`, so a foreign folder called "Remote" is never taken), `Logs` moved with it, old preferences' `Remote…` keys copied. Not movable: TCC grants (Screen Recording, Accessibility, Local Network), notifications, the login item. |
| Keys | `CoreBridge.swift` (`KeychainStore`), `HostController.swift`; Rust: `host-desktop/src/{ffi,session}.rs` | The client key is `device.key` and the host key `host/device.key` in the Keychain under the bundle ID. On the first read: a key file in the folder is saved to the Keychain and deleted (the host's was a file); a key in the app's earlier Keychain service is copied, the old item is left. `ThisComputer(dataDir:name:listener:keyStore:)` takes the new `HostKeyStore`; `session::Options.store` carries it. |
| No server | `AppController.startCore`, `Store.serverURL`, `AppModel.serverHost/serverName` | With no server configured the core is started with `rendezvous: false` (LAN and address only) and every relay string is gone: the status strip's right half is empty, Advanced has no Network group, the connecting card says "on this network", "Couldn't reach" says where to look and offers Server settings…, the offline card names no server. With a server (`Store.serverURL`, preferences key `RemoteServerURL`) the same places name its host ("Sodium Remote service" for `remote.sodiumos.com`). |
| Settings sheet | `SettingsSheet.swift`, `SettingsAbout.swift` | Tabs: Display, Quality, Input, Home monitor, Audio, **Server**, Advanced, **About**. Server shows the state only (no server / the server and whether it answers). The "About Sodium Remote" menu item and the card's "Server settings…" open the sheet on About and Server (`AppModel.openSettingsOn`). |
| About | `SettingsAbout.swift` (`AboutTab`, `NoticesSheet`) | Version (from Info.plist), "Preview" pill, "AGPL-3.0 or commercial license", Source code (opens `Brand.sourceURL`, AGPL §13), Third-party notices (the short list and "Open the notices file" → `THIRD-PARTY-NOTICES.txt` in the app's Resources, made by `build-app.sh` from `App/Acknowledgements.md` and cargo-about's output), the no-updates and no-telemetry line. |
| Show previews | `SettingsAbout.swift` (`ComputersGroup`), `Store`, `AppController.previewsChanged` | Advanced › Computers, on by default, `RemoteShowPreviews` in preferences. Off: no thumbnail is saved or shown, the saved ones are deleted, the cards keep their size with a flat tile and the list shows "Previews are off. The saved pictures were deleted." for 4 s. |
| Strings | `Localizable.xcstrings` (129 keys), `Tools/check-xcstrings-languages.py` | Every new string through `String(localized:)`, `Text("…")` or a `LocalizedStringKey`, in all ten languages (German and Japanese from the design boards; the others written to the same sense; en-GB and en-AU "licence"). The checker fails the build when a key lacks a language or changes its placeholders; `build-app.sh` runs it. The older English literals are still debt (below). |

### Rules the code follows

- A Windows or Mac host is named "Sodium Remote" in copy, untranslated and passed in; a SodiumOS host is "Settings › Remote", never "Sodium Remote". The Mac-permission card and the control-switched-off card name a host's app: the first always "Sodium Remote" (the host is a Mac), the second by the remembered host OS (`Welcome.host_os`): "Settings › Remote" for SodiumOS, "Sodium Remote" for Windows or Mac, and no place when the OS isn't known yet.
- Disconnect cards come from typed reasons only, never from text: `DisconnectReason.controlOff` is "`%@` isn’t allowing control"; `.permissionMissing(screenRecording:accessibility:)` says which permission is missing (Screen Recording, Accessibility, or both). The old `screen-recording:` prefix match is gone.
- No display picker with one display or for a Virtual host; names where the host sends them, else "Display N"; the choice is never saved.
- The key table is the Mac table for SodiumOS and Windows hosts and empty for a Mac. Core's keymap tests assert `default_rules(Mac, Linux)` and `(Mac, Windows)` both equal `KeyRule::mac_defaults()`, so the existing `mac_default_key_table()` is that table.
- The pane shows `IP:PORT` (a bare address pairs) with Copy and a "…" menu with "Copy full address" for `ID@IP:PORT`, as the Windows page does. The port is `ThisComputer.lanPort()`; nothing reads `host.txt`. "View only" appears from Screen Recording allowed and Accessibility not (the `Welcome` flag and the client chip are not wired yet).
- The Mac client has no "Send Ctrl+Alt+Del"; `HostOS.offersCtrlAltDel` (true for SodiumOS hosts only) is the rule for when it gets one.

## The generated APIs relied on

From `build/swift-host/` (made with `host-desktop/build-swift.sh bindings`; spelling checked there).

`SodiumRemoteHost.swift`: `ThisComputer(dataDir:name:listener:) throws`; `start() async throws`; `stop() async`; `state()`; `isOn()`; `openPairing() async throws -> PairingCode(code, expiresInSecs: UInt64)`; `closePairing()`; `devices() -> [AllowedDevice(id, code, name, os: Os?, pairedAt, lastUsed: UInt64?)]`; `revoke(id:) async throws`; `name()`, `setName(name:)`; `disconnect()`; `deviceId() throws`, `deviceCode() throws`; `permissions() -> HostPermissions(screenRecording, accessibility)`; `requestScreenRecording()`, `requestAccessibility() -> PermissionState` (`.allowed`, `.notAllowed`, `.reopenToFinish`); `ThisComputerState` (`.off`, `.waiting`, `.beingControlled(by:)`, `.secureScreen(by:)`); `ControllingDevice(id, name, os: Os, since: UInt64, reconnecting)`; `ThisComputerEvent` (`.state(state:)`, `.pairingOpened(pairing:)`, `.pairingAttempt`, `.pairingFailed(attemptsLeft:)`, `.pairingClosed(reason:)`, `.devicesChanged`); `lanPort() -> UInt16?` (the bound LAN port; `nil` while off; spelled `lanPort()` in the generated Swift, from Rust `lan_port`); `ThisComputerError` (`.Off`, `.UnknownDevice`, `.Failed(message:)`); `protocol ThisComputerListener: AnyObject, Sendable { func onEvent(event:) }`.

`RemoteCore.swift`: `Os` (`.linux`, `.mac`, `.windows`); `Welcome.hostOs`, `.displayMode` (`.virtual`, `.physical`), `.displays: [DisplayInfo(id, name, width, height, scale, primary)]`, `.currentDisplay: UInt32`, `.liveResize`, `.scales`, `.homeMonitor`; `Msg.selectDisplay(UInt32)`, `Msg.secureScreen(Bool)`, `Msg.keyTable([KeyRule])`; `Hello.clientOs`; `DisconnectReason.error(String)`, `.permissionMissing(screenRecording:accessibility:)`, `.controlOff` (a case with no payload, index 11, appended last); `RemoteClient.sendControl(msg:)`.

**Spellings the code relies on:** `ThisComputer.lanPort() -> UInt16?` (non-throwing); `DisconnectReason.controlOff` and `.permissionMissing(let screenRecording, let accessibility)` as exhaustive switch cases (the `switch` in `disconnectedCard` has no `default`); `UNNotificationSettings.timeSensitiveSetting == .enabled` and `UNNotificationInterruptionLevel.active`; `NSStatusBar.system.removeStatusItem(_:)`; `Bundle.main.bundleIdentifier`; SwiftUI `Menu { SwiftUI.Button(...) } label: { Image }` with `.menuStyle(.button)`, `.buttonStyle(.plain)`, `.menuIndicator(.hidden)` (the host list's `MoreMenu` pattern); `String(localized: "…\(string)…")` with a `String` interpolation (catalog key `%@`), also with the `static let app = "Sodium Remote"` in `PermissionCardCopy`.

Name clashes avoided: core's `PermissionState` (host) and RemoteUI's older `PermissionState`: the app target writes `RemoteCore.PermissionState`. The app's own `DisplayInfo` (pixels, Hz) shadows core's by module-local lookup, as before.

## Checking a build

Build with `./build-app.sh --test-id` (bundle ID `com.sodiumos.remote.dev`, name "Sodium Remote (test)"), as [BUILD.md](../../client-mac/BUILD.md) describes. [host-mac.md](host-mac.md) has the host checklist: permissions, displays, pairing a second device, and the input checks.

## Design choices

- **The data folder is named by the bundle ID** (`Application Support/com.sodiumos.remote`, test `….dev`). An older folder from before the rename is moved, not copied (one set of pairings), only when it looks like ours. The old Keychain item is left in place.
- **The host key lives in the Keychain.** A host-desktop `HostKeyStore` foreign trait (the same shape as core's `KeyStore`), an optional `session::Options.store`, and `ThisComputer.new(..., key_store)` carry it. `sodium-remote-host` run as a program is unchanged (files).
- **The server settings live in the app's preferences** (`RemoteServerURL`).
- **"About Sodium Remote" opens Settings › About;** Source code is the repository URL from `Brand`.
- **One notices file:** the acknowledgements and cargo-about's list are concatenated into `THIRD-PARTY-NOTICES.txt` in the app's Resources.

## Known limits

- The app has no automatic updates.
- Time-sensitive notifications need a notarized build.
- The address shown is the first `en*` IPv4 address; with more than one interface, the list of every `IP:PORT` in the "…" menu (the Windows page has it) is not drawn here.
- The client doesn't stop sending input during the secure screen (the host drops it).
- The menu-bar menu is a plain `NSMenu`.
