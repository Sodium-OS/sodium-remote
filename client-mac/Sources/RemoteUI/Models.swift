// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import Combine
import RemoteKit
import SwiftUI

// The state the views draw and the actions they call. The app (target
// Remote) owns the objects, fills them from the core and implements the
// actions; the views never see the core's types.

// MARK: - Settings (one set for every computer; applied at once)

public enum ResolutionChoice: String, Codable, CaseIterable {
    case matchWindow, matchDisplay, fixed, custom
}

public enum UISizeChoice: String, Codable, CaseIterable {
    case matchMac, larger, smaller, sameAsDesk
}

public enum CodecPick: String, Codable, CaseIterable {
    /// HEVC 4:4:4 if this Mac decodes it in hardware, else 4:2:0.
    case auto
    /// HEVC 4:2:0 whatever this Mac decodes: fewer bytes than 4:4:4. Sent as
    /// `CodecChoice.hevc420` in `Settings`, so it applies at once.
    case hevc
    case av1
}

public enum KeyboardMode: String, Codable, CaseIterable {
    /// The key table: ⌘ acts as Ctrl.
    case mac
    /// PC passthrough: an empty table.
    case pc
}

public enum InputMethodChoice: String, Codable, CaseIterable {
    /// This Mac's input method through NSTextInputClient.
    case mac
    /// Keys go raw; the computer's own (fcitx5) composes.
    case computer
}

public enum SystemShortcutsChoice: String, Codable, CaseIterable {
    case never, fullScreen, fullScreenAndFocused

    public var label: String {
        switch self {
        case .never: return "Never"
        case .fullScreen: return "Full screen only"
        case .fullScreenAndFocused: return "Full screen and focused"
        }
    }
}

public enum HomeMode: String, Codable, CaseIterable {
    case mirror, privacy, off
}

public enum RememberChoice: String, Codable, CaseIterable {
    case thisMac, thisSession
}

public enum RelayChoice: String, Codable, CaseIterable {
    /// The fastest measured path.
    case automatic
    /// Share only relayed candidates (core's `relay_only`), from the next
    /// connection (`RemoteClient.setRelayOnly`).
    case alwaysRelay
}

public enum HudMode: String, Codable, CaseIterable {
    case off, compact, expanded
}

public struct RemoteSettings: Codable, Equatable {
    // Display
    public var resolution: ResolutionChoice = .matchWindow
    public var fixedSize = PixelSize(width: 1920, height: 1200)
    public var customSize = PixelSize(width: 2048, height: 1280)
    public var fixedResolution = false
    /// Hz; 0 = the display's rate (the default).
    public var refreshHz = 0
    public var uiSize: UISizeChoice = .matchMac
    public var fullScreenBesideCamera = false
    // Quality
    public var codec: CodecPick = .auto
    public var bandwidthLimited = false
    public var limitMbps: Double = 40
    public var optimisedLook = true
    // Input
    /// PC keys: ⌘ is the computer's Win key (the user's call, 2026-11-17).
    public var keyboard: KeyboardMode = .pc
    /// The edited key table (nil: the defaults).
    public var keyTable: KeyTable?
    public var inputMethod: InputMethodChoice = .mac
    public var systemShortcuts: SystemShortcutsChoice = .fullScreenAndFocused
    // Home monitor
    public var homeMode: HomeMode = .mirror
    public var rememberHome: RememberChoice = .thisMac
    // Audio
    public var playSound = true
    /// Core Audio device UID; nil = the system default output.
    public var outputDevice: String?
    // Advanced
    public var relay: RelayChoice = .automatic
    /// Sessions of stats files to keep (0: all).
    public var keepSessions = 50
    // The HUD
    public var hud: HudMode = .off

    public init() {}

    /// The bandwidth cap in kbit/s for `StreamSettings` (0 = Auto).
    public var bandwidthLimitKbps: UInt32 {
        bandwidthLimited ? UInt32(max(1, limitMbps) * 1000) : 0
    }
}

/// What the settings sheet shows about this Mac and the session.
public struct SettingsInfo: Equatable {
    /// Backing pixels: "2236 × 1348 px" (the stream now), "2940 × 1848 px"
    /// (full screen, below the camera).
    public var windowPixels = ""
    public var displayPixels = ""
    /// The display's refresh rate, Hz.
    public var displayHz = 60
    /// "scale 2.0" when known.
    public var scaleText = ""
    /// This Mac decodes HEVC 4:4:4 in hardware.
    public var hevc444Hardware = false
    public var av1Hardware = false
    public var accessibility: PermissionState = .unknown
    /// Output devices: (UID, name).
    public var outputs: [AudioOutputChoice] = []
    /// AirDrop and Handoff (AWDL) are active on Wi-Fi.
    public var awdl = false
    public var statsFolder = ""
    public init() {}
}

public struct AudioOutputChoice: Equatable, Hashable, Identifiable {
    public var uid: String
    public var name: String
    public var id: String { uid }
    public init(uid: String, name: String) {
        self.uid = uid
        self.name = name
    }
}

public enum PermissionState: Equatable {
    case unknown
    /// Allow… was pressed; waiting for System Settings.
    case waiting
    case allowed
    case denied
}

// MARK: - The host list

public struct HostCardModel: Identifiable {
    /// The full device ID (base64url).
    public var id: String
    public var name: String
    /// `XXXX-XXXX`.
    public var code: String
    public var state: HostState
    public var line: String
    public var band: QualityBand
    public var route: RouteKind?
    /// "Direct · 9 ms" or "Last time: LAN · 2 ms".
    public var pathText: String?
    /// The last frame this Mac saw of it (kept locally, never sent).
    public var thumbnail: NSImage?
    /// What it runs, as its last `Welcome` said: the text pill on its picture (nil until the first connection).
    public var os: HostOS?

    public init(id: String, name: String, code: String, state: HostState, line: String, band: QualityBand, route: RouteKind?,
                pathText: String?, thumbnail: NSImage?, os: HostOS? = nil) {
        self.os = os
        self.id = id
        self.name = name
        self.code = code
        self.state = state
        self.line = line
        self.band = band
        self.route = route
        self.pathText = pathText
        self.thumbnail = thumbnail
    }
}

/// A computer found with an open pairing window (mDNS or the rendezvous).
public struct NearbyHost: Identifiable, Equatable {
    public var id: String
    public var name: String
    /// "lan", "rendezvous" or "both".
    public var foundVia: String
    public init(id: String, name: String, foundVia: String) {
        self.id = id
        self.name = name
        self.foundVia = foundVia
    }
}

public enum RendezvousStatus: Equatable {
    case connecting
    case online
    case offline(String?)
    /// Switched off (LAN only).
    case off
}

/// A tab the settings sheet can be asked to open on.
public enum SettingsTabRequest {
    case server, about
}

public enum SystemPane {
    case accessibility, localNetwork, microphone
}

@MainActor
public protocol AppActions: AnyObject {
    func connect(hostID: String, fullScreen: Bool)
    /// Open the pairing sheet (with a computer from "On this network" chosen).
    func openPairing(hostID: String?)
    func rename(hostID: String, to name: String)
    func copyID(hostID: String)
    /// Forget after the user confirmed.
    func forget(hostID: String)
    func requestAccessibility()
    func requestLocalNetwork()
    func openSystemSettings(_ pane: SystemPane)
    /// Not now / Continue on the welcome.
    func finishWelcome()
    /// The bar's gear: Settings over the main window.
    func openSettingsSheet()
}

/// The settings sheet's buttons (its values are `AppModel.settings`, which
/// the app observes and applies at once).
@MainActor
public protocol SettingsActions: AnyObject {
    func settingsDone()
    func showStatsFolder()
    func runLatencyTest()
    /// "How to turn them off" for AirDrop and Handoff (AWDL).
    func showAWDLHelp()
    func openSystemSettings(_ pane: SystemPane)
    /// About › Source code (AGPL-3.0 section 13): the repository in the browser.
    func openSourceCode()
    /// About › Third-party notices › Open the notices file.
    func openNoticesFile()
}

/// A server as the Server tab and its sheets show it (what the core made of a link, a `server.toml` or typed values).
public struct ServerCandidate: Equatable {
    public enum Source: Equatable { case link, file, typed }
    public var url: String
    public var host: String
    /// "Sodium Remote service" for the hosted service, else the host.
    public var label: String
    /// The hosted service: Web PKI, nothing to confirm about a certificate.
    public var official: Bool
    /// The CA it pins, PEM ("" for none).
    public var caPEM: String
    public var token: String
    public var source: Source

    public init(url: String, host: String, label: String, official: Bool, caPEM: String, token: String, source: Source) {
        self.url = url
        self.host = host
        self.label = label
        self.official = official
        self.caPEM = caPEM
        self.token = token
        self.source = source
    }

    public var pinsCA: Bool { !caPEM.isEmpty }
}

/// Why the core refused what was typed or pasted; the sheet puts the message on the field that caused it.
public enum ServerInputError: Error, Equatable {
    /// `insecure`: an `http://` or `ws://` address, refused with its own words (never upgraded).
    case notALink, badAddress, insecureAddress, badCertificate
}

/// The Server tab's buttons. The app target asks the core and keeps the settings.
@MainActor
public protocol ServerActions: AnyObject {
    /// A pasted or opened `sodium-remote://server?…` link; nothing is saved.
    func inspectServerLink(_ text: String) -> Result<ServerCandidate, ServerInputError>
    /// Typed values (address, CA as PEM or "", token or ""); nothing is saved.
    func inspectServer(url: String, caPEM: String, token: String) -> Result<ServerCandidate, ServerInputError>
    /// After the person confirmed: keep it, and use it (now, or after a session that is running).
    func saveServer(_ server: ServerCandidate)
    func removeServer()
    /// The current server, for Change….
    func currentServer() -> ServerCandidate?
    /// "Choose a file…" for the CA: the file's name and its text; nil when cancelled.
    func chooseCertificateFile() -> (name: String, pem: String)?
    func importServerFile()
    func exportServerFile()
    func clipboardText() -> String?
    /// "How to run your own server".
    func openServerGuide()
    /// "Try now".
    func retryServer()
}

/// The app-wide state: the host list, the welcome, permissions, settings.
@MainActor
public final class AppModel: ObservableObject {
    @Published public var hosts: [HostCardModel] = []
    @Published public var nearby: [NearbyHost] = []
    @Published public var thisMacName = ""
    @Published public var thisMacCode = ""
    @Published public var thisMacID = ""
    @Published public var rendezvous: RendezvousStatus = .connecting
    /// The host of the configured server; nil when none is (LAN and address only: nothing mentions a relay).
    @Published public var serverHost: String?
    /// "Show previews" (Settings › Advanced › Computers): the pictures on the computers' cards.
    @Published public var showPreviews = true
    /// A short note over the list ("Previews are off. The saved pictures were deleted."), cleared by its owner.
    @Published public var listToast: String?
    /// Open the settings sheet on this tab (the next time it opens), then cleared.
    public var openSettingsOn: SettingsTabRequest?
    /// The welcome is the main window's first content.
    @Published public var showWelcome = false
    @Published public var accessibility: PermissionState = .unknown
    @Published public var localNetwork: PermissionState = .unknown
    /// The card being renamed in place.
    @Published public var renamingID: String?
    /// The card whose Forget is being asked.
    @Published public var forgetAskID: String?
    @Published public var settings = RemoteSettings()
    @Published public var settingsInfo = SettingsInfo()
    /// What the connected host offers (`Welcome`): the UI size and home monitor settings only show when it does.
    @Published public var hostOffersUISize = true
    @Published public var hostOffersHomeMonitor = true
    /// the table from the core, for Reset and the "· changed" marks.
    @Published public var defaultKeyTable = KeyTable(rules: [])
    /// The bar's "Computers" and "This computer".
    @Published public var page: MainPage = .computers
    /// The "This computer" page's state; the app's `HostController` fills it.
    public let hostPane = HostPaneModel()
    public weak var actions: AppActions?
    public weak var settingsActions: SettingsActions?
    public weak var serverActions: ServerActions?
    /// The configured server (nil: none). `serverHost` follows it.
    @Published public var server: ServerCandidate?
    /// A server to confirm (from a `sodium-remote://` link or an imported file): the Server tab opens its sheet on it.
    @Published public var pendingServer: ServerCandidate?
    /// A new server was saved while a session was running: it is used when the session ends.
    @Published public var serverWaitsForSession = false

    public init() {}

    /// How the UI names the configured server: the host, or "Sodium Remote service" for the hosted one.
    public var serverName: String? {
        guard let h = serverHost, !h.isEmpty else { return nil }
        return h == Brand.officialServerHost ? String(localized: "Sodium Remote service") : h
    }

    /// The table in effect for Mac keys.
    public var keyTable: KeyTable { settings.keyTable ?? defaultKeyTable }
}

// MARK: - Pairing

public enum PairingPhase: Equatable {
    case entering
    case checking
    /// A wrong code; tries left when known.
    case wrong(left: Int?)
    case tooMany
    case expired
    case noComputer
    case failed(String)
    case paired(name: String, code: String)
}

@MainActor
public protocol PairingActions: AnyObject {
    /// The sixth digit was typed (or a code pasted).
    func pairingSubmit(code: String)
    func pairingCancel()
    /// Too many / expired: clear and keep the sheet open.
    func pairingTryNew()
    /// Paired: Done (or Connect now), with the name to keep on this Mac.
    func pairingFinish(name: String, connect: Bool)
}

@MainActor
public final class PairingModel: ObservableObject {
    @Published public var digits = ""
    @Published public var phase: PairingPhase = .entering
    /// The name kept on this Mac (prefilled from the computer's own).
    @Published public var nameHere = ""
    public weak var actions: PairingActions?
    public init() {}
}

// MARK: - The stream window

public enum ToastTone: Equatable {
    case accent, neutral, warning, success
}

public enum ToastAction: Equatable {
    case details
    case allowAccessibility
    case keepFullEffects
}

/// An in-stream toast: top centre, 4 s, one at a time.
public struct Toast: Equatable, Identifiable {
    public let id = UUID()
    public var symbol: String
    public var tone: ToastTone
    public var text: String
    public var sub: String?
    public var action: String?
    public var actionID: ToastAction?
    /// Seconds before it goes (nil: until dismissed).
    public var duration: Double?

    public init(symbol: String, tone: ToastTone, text: String, sub: String? = nil, action: String? = nil, actionID: ToastAction? = nil,
                duration: Double? = 4) {
        self.symbol = symbol
        self.tone = tone
        self.text = text
        self.sub = sub
        self.action = action
        self.actionID = actionID
        self.duration = duration
    }

    public static func == (a: Toast, b: Toast) -> Bool { a.id == b.id }
}

/// The connecting card's steps.
public struct ConnectProgress: Equatable {
    /// 0 find, 1 path, 2 key, 3 stream; 4 done.
    public var step = 0
    /// "on this network" with no server, "through relay.example.org" with one; set by the session.
    public var findDetail = ""
    public var pathDetail = ""
    public var keyDetail = "the one you paired"
    public var sizeDetail = ""
    /// "From Office Wi-Fi · usually under 2 s".
    public var network = ""
    public init() {}
}

/// A card over the stream: one at a time.
public enum StreamCard: Equatable {
    case connecting
    /// only while the other device is streaming right now.
    case askTakeover(by: String)
    case takenOver(by: String, at: String)
    /// not online; retrying on its own.
    case offline(lastSeen: String?, retryIn: Int?)
    case asleep(since: String, retryIn: Int?)
    /// the session is over (the core's `Disconnected`).
    case disconnected(title: String, body: String)
    /// the versions the two sides speak, and which side has to be updated (the core's
    /// `versionMismatchUpdateSide`: `updateThisMac` names this Mac, else the computer.
    case versionMismatch(host: Int, client: Int, updateThisMac: Bool)
    /// the key the computer answered with isn't the pinned one. Drawn, but
    /// unreachable in M1: the device ID is the key, so a computer whose
    /// key changed has a new ID and the handshake never meets a different key. A
    /// reinstalled host is a new card, and the old one is removed by hand; in M2
    /// pairing discovery shows this card (an open pairing window whose name
    /// matches a paired, offline computer with another ID).
    case keyChanged(hostID: String)
    /// the computer removed this Mac.
    case removed
    /// neither the server nor mDNS answers.
    case macOffline
    case failed(String)
    /// The host's lock screen (or UAC, sign-in) is up: it can't be shown or driven.
    case secureScreen(os: HostOS)
    /// A Mac without Screen Recording or Accessibility turned this Mac away (`DisconnectReason.permissionMissing`): the
    /// flags say which is missing, either or both; the app's name is passed in untranslated.
    case hostNeedsPermission(screenRecording: Bool, accessibility: Bool)
    /// "Allow remote control" is off on the computer (`DisconnectReason.controlOff`); `os` names where to switch it on.
    case controlOff(os: HostOS?)
}

/// The stats HUD's data.
public struct HudData: Equatable {
    public var frames = HudFrames()
    public var route: RouteKind?
    public var rttMs: Double?
    public var band: QualityBand = .none
    public var lossPercent: Double = 0
    public var fecPercent: Double = 0
    /// "2440 × 1510 · 60 Hz".
    public var sizeText = ""
    /// "HEVC 4:4:4 · hardware decode".
    public var codecText = ""
    /// (route, state, rtt) for the Paths list: in use, hedging, standby.
    public var paths: [HudPath] = []
    public var awdl = false
    public var linkText = ""
    public init() {}
}

public struct HudPath: Equatable, Identifiable {
    public var kind: RouteKind
    public var state: String
    public var rttMs: Double?
    public var inUse: Bool
    public var id: String { kind.rawValue + state }
    public init(kind: RouteKind, state: String, rttMs: Double?, inUse: Bool) {
        self.kind = kind
        self.state = state
        self.rttMs = rttMs
        self.inUse = inUse
    }
}

@MainActor
public protocol StreamActions: AnyObject {
    /// Off → compact → expanded → off (⌃⌥⌘S).
    func cycleHud()
    /// The path chip: open the stats expanded.
    func openHudExpanded()
    func closeHud()
    func copyStats()
    func openSettings()
    /// "Server settings…" on the couldn't-reach card: Settings, on the Server tab.
    func openServerSettings()
    func toggleFullScreen()
    func disconnect()
    /// Cancel on the connecting card.
    func cancelConnect()
    /// Close on a card: back to the list.
    func closeCard()
    func takeOver()
    /// Reconnect (Disconnected), Take it back (Taken over), Try now (offline).
    func reconnect()
    func forgetHost()
    func pairAgain()
    func howToUpdate()
    func toastAction(_ a: ToastAction)
    func dismissToast()
    func setKeyboard(_ m: KeyboardMode)
    func setHomeMode(_ m: HomeMode)
    func hideRevealMenu()
    /// The display picker (hosts with a physical display only).
    func selectDisplay(id: UInt32)
}

/// One stream window's state.
@MainActor
public final class StreamModel: ObservableObject {
    @Published public var hostName = ""
    /// How the UI names the configured server (nil: none; the cards then never mention one).
    @Published public var serverName: String?
    /// The path chip (nil until a path is chosen).
    @Published public var chip: LinkView.Chip?
    /// The Reconnecting pill and the seconds since t₀.
    @Published public var pill: LinkView.Pill?
    @Published public var pillSince: Double = 0
    @Published public var hudMode: HudMode = .off
    @Published public var hud = HudData()
    @Published public var toast: Toast?
    @Published public var card: StreamCard?
    @Published public var progress = ConnectProgress()
    @Published public var fullScreen = false
    /// The top-edge menu in full screen.
    @Published public var revealMenu = false
    /// "1102 × 700 pt" while the window is being resized.
    @Published public var resizeBadge: String?
    @Published public var keyboard: KeyboardMode = .mac
    @Published public var homeMode: HomeMode = .mirror
    /// What the connected host offers (`Welcome`): hide the control when it doesn't.
    @Published public var hostOffersUISize = true
    @Published public var hostOffersHomeMonitor = true
    /// The host's displays, whether it is a Virtual host (SodiumOS: no picker) and which one is shown.
    @Published public var displays: [DisplayChoice] = []
    @Published public var hostVirtual = true
    @Published public var currentDisplay: UInt32 = 0
    /// What the host runs: the secure-screen wording and the key table follow it.
    @Published public var hostOS: HostOS?
    /// The secure-screen chip in the bar (the card is `StreamCard.secureScreen`).
    @Published public var secure = false
    @Published public var active = true
    public weak var actions: StreamActions?
    public init() {}
}
