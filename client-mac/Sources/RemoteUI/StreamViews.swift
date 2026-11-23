// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

/// The Remote tile as the Dock icon (no asset catalog: the Command Line Tools
/// have no actool).
public enum AppIcon {
    @MainActor public static func image(size: CGFloat) -> NSImage? {
        let tile = AppTile(size: size * 0.8).frame(width: size, height: size)
        let r = ImageRenderer(content: tile)
        r.scale = 2
        return r.nsImage
    }
}

/// The stream window's SwiftUI pieces.
/// Each is its own NSView so the app can place it over the Metal view and
/// hide it, so nothing transparent ever takes the stream's clicks.
public enum StreamUI {
    /// The toolbar's leading item: the computer's name and the path chip,
    /// which carries the compact stats while they are on.
    @MainActor public static func makeTitleItem(model: StreamModel) -> NSView {
        let v = NSHostingView(rootView: StreamTitleItem(model: model))
        v.setFrameSize(v.fittingSize)
        return v
    }

    /// The display picker, next to the buttons (only while the host has several physical displays).
    @MainActor public static func makeDisplayItem(model: StreamModel) -> NSView {
        let v = NSHostingView(rootView: StreamDisplayItem(model: model))
        v.setFrameSize(v.fittingSize)
        return v
    }

    /// The toolbar's trailing item: Stats, Settings, Full screen.
    @MainActor public static func makeButtonsItem(model: StreamModel) -> NSView {
        let v = NSHostingView(rootView: StreamButtonsItem(model: model))
        v.setFrameSize(v.fittingSize)
        return v
    }

    /// The in-stream toast, top centre under the computer's bar.
    @MainActor public static func makeToast(model: StreamModel) -> NSView {
        NSHostingView(rootView: StreamToastHost(model: model))
    }

    /// The Reconnecting pill, centred.
    @MainActor public static func makePill(model: StreamModel) -> NSView {
        NSHostingView(rootView: StreamPillHost(model: model))
    }

    /// The cards (connecting, taken over, disconnected, …): fills the stream.
    @MainActor public static func makeCards(model: StreamModel) -> NSView {
        NSHostingView(rootView: StreamCardHost(model: model))
    }

    /// The stats panel, expanded.
    @MainActor public static func makeHudPanel(model: StreamModel) -> NSView {
        NSHostingView(rootView: StreamHudPanel(model: model))
    }

    /// The compact stats pill, for full screen.
    @MainActor public static func makeHudPill(model: StreamModel) -> NSView {
        NSHostingView(rootView: StreamHudPillHost(model: model))
    }

    /// The top-edge menu in full screen.
    @MainActor public static func makeRevealMenu(model: StreamModel) -> NSView {
        NSHostingView(rootView: StreamRevealMenu(model: model))
    }

    /// The size badge while resizing.
    @MainActor public static func makeResizeBadge(model: StreamModel) -> NSView {
        NSHostingView(rootView: StreamResizeBadge(model: model))
    }

    /// The HUD as plain text, for Copy.
    public static func statsText(_ h: HudData) -> String {
        var lines: [String] = []
        if let t = h.frames.totalMs {
            lines.append(String(format: "Input to photon, estimated: %.0f ms", t))
        }
        for s in h.frames.stages {
            let v = s.ms.map { String(format: "%.1f", $0) } ?? "–"
            lines.append("\(s.label): \(v) ms\(s.estimated ? " (estimated)" : "")")
        }
        lines.append("Size: \(h.sizeText)")
        lines.append("Codec: \(h.codecText)")
        lines.append("Frames: \(h.frames.fpsPresented) fps · \(h.frames.late) late · \(h.frames.dropped) dropped")
        lines.append(String(format: "Loss: %.1f %% · FEC %.0f %%", h.lossPercent, h.fecPercent))
        lines.append(String(format: "Bitrate: %.1f Mb/s", h.frames.mbps))
        if let r = h.route { lines.append("Path: \(r.label)" + (h.rttMs.map { String(format: " · %.1f ms", $0) } ?? "")) }
        if let host = h.frames.host {
            lines.append(String(format: "Computer: capture %.1f · convert %.1f · encode %.1f · send %.1f ms · %.0f fps · QP %d",
                                Double(host.captureUs) / 1e3, Double(host.convertUs) / 1e3, Double(host.encodeUs) / 1e3,
                                Double(host.sendUs) / 1e3, Double(host.fps), Int(host.qp)))
        }
        return lines.joined(separator: "\n")
    }
}

// MARK: - Toolbar

@MainActor
struct StreamTitleItem: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        HStack(spacing: 10) {
            Text(model.hostName)
                .font(.system(size: 13, weight: model.active ? .semibold : .regular))
                .foregroundColor(model.active ? Theme.text : Theme.titleInactive)
                .lineLimit(1)
            if let os = model.hostOS {
                // The OS as words, beside the name; not translated.
                Text(verbatim: os.pillText)
                    .font(.system(size: 12))
                    .foregroundColor(Theme.text3)
                    .lineLimit(1)
            }
            if model.secure {
                SecureChip()
            } else if let chip = model.chip {
                if model.hudMode != .off && !chip.lost {
                    StreamStatsChip(hud: model.hud, chip: chip, open: model.hudMode == .expanded) {
                        model.actions?.openHudExpanded()
                    }
                } else {
                    PathChip(chip, open: model.hudMode == .expanded) {
                        model.actions?.openHudExpanded()
                    }
                }
            }
        }
        .padding(.leading, 2)
        .fixedSize()
    }
}

/// The bar's amber "Secure screen" chip while the host shows its lock screen.
@MainActor
struct SecureChip: View {
    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "lock.fill").font(.system(size: 11, weight: .semibold))
            Text("Secure screen").font(.system(size: 12, weight: .semibold)).fixedSize()
        }
        .foregroundColor(Theme.warning)
        .padding(.leading, 8)
        .padding(.trailing, 10)
        .frame(height: 24)
        .background(Capsule().fill(Theme.warningBg))
    }
}

/// The display picker: "Display 1 ▾", the host's names where it sends them, the main display marked.
@MainActor
struct StreamDisplayItem: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        Menu {
            Picker(selection: Binding(get: { model.currentDisplay }, set: { model.actions?.selectDisplay(id: $0) })) {
                ForEach(Array(model.displays.enumerated()), id: \.element.id) { item in
                    Text(menuTitle(item.element, item.offset)).tag(item.element.id)
                }
            } label: {
                Text("Displays")
            }
            .pickerStyle(.inline)
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "display").font(.system(size: 13))
                Text(buttonTitle).font(.system(size: 12, weight: .medium)).fixedSize()
                Image(systemName: "chevron.down").font(.system(size: 9, weight: .semibold))
            }
            .foregroundColor(Theme.icon)
            .padding(.horizontal, 8)
            .frame(height: 28)
            .contentShape(Rectangle())
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .accessibilityLabel(Text("Displays"))
    }

    private func name(_ d: DisplayChoice, _ index: Int) -> String {
        switch DisplayPicker.label(d, index: index) {
        case .named(let n): return n
        case .numbered(let i): return String(localized: "Display \(i)")
        }
    }

    private var buttonTitle: String {
        let i = model.displays.firstIndex(where: { $0.id == model.currentDisplay }) ?? 0
        guard model.displays.indices.contains(i) else { return "" }
        return name(model.displays[i], i)
    }

    /// "Built-in display · Main · 2940 × 1912", "DELL U2720Q · 2560 × 1440".
    private func menuTitle(_ d: DisplayChoice, _ index: Int) -> String {
        let n = name(d, index)
        let size = DisplayPicker.sizeText(d)
        return d.primary ? String(localized: "\(n) · Main · \(size)") : String(localized: "\(n) · \(size)")
    }
}

/// The path chip grown to carry the compact stats (kitr `stats_chip`).
@MainActor
struct StreamStatsChip: View {
    let hud: HudData
    let chip: LinkView.Chip
    let open: Bool
    let action: () -> Void

    var body: some View {
        let warn = chip.tone != .normal
        let bg: Color = warn ? Theme.warningBg : (open ? Theme.open : Theme.raised)
        return SwiftUI.Button(action: action) {
            StreamStatsLine(hud: hud, chip: chip, separator: Theme.hairStrong)
                .padding(.leading, 8)
                .padding(.trailing, 10)
                .frame(height: 24)
                .background(Capsule().fill(bg))
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
    }
}

/// "● 35 ms | 60 fps | 34 Mb/s | ⟷ Direct".
@MainActor
struct StreamStatsLine: View {
    let hud: HudData
    let chip: LinkView.Chip
    let separator: Color

    var body: some View {
        let warn = chip.tone != .normal
        let total: Int = Int((hud.frames.totalMs ?? 0).rounded())
        let mbps: String = String(format: hud.frames.mbps < 10 ? "%.1f" : "%.0f", hud.frames.mbps)
        return HStack(spacing: 8) {
            QualityDot(warn ? .fair : hud.band)
            Text("\(total) ms").font(.system(size: 12, weight: .semibold))
            Rectangle().fill(separator).frame(width: 1, height: 12)
            Text("\(hud.frames.fpsPresented) fps").font(.system(size: 12))
            Rectangle().fill(separator).frame(width: 1, height: 12)
            Text("\(mbps) Mb/s").font(.system(size: 12))
            Rectangle().fill(separator).frame(width: 1, height: 12)
            HStack(spacing: 5) {
                if let r = chip.route { RouteIcon(r, size: 13) }
                Text(chip.route?.label ?? chip.text).font(.system(size: 12))
            }
            .foregroundColor(warn ? Theme.warning : Theme.text)
        }
        .monospacedDigit()
        .foregroundColor(Theme.text)
        .lineLimit(1)
    }
}

@MainActor
struct StreamButtonsItem: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        HStack(spacing: 2) {
            IconButton("gauge.with.dots.needle.33percent", label: "Stats", on: model.hudMode != .off) {
                model.actions?.cycleHud()
            }
            IconButton("gearshape", label: "Settings") {
                model.actions?.openSettings()
            }
            IconButton(model.fullScreen ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right",
                       label: model.fullScreen ? "Exit full screen" : "Full screen") {
                model.actions?.toggleFullScreen()
            }
        }
        .fixedSize()
    }
}

// MARK: - Toast and pill

@MainActor
struct StreamToastHost: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        Group {
            if let t = model.toast {
                StreamToastView(toast: t, model: model)
            } else {
                Color.clear.frame(width: 0, height: 0)
            }
        }
        .fixedSize()
    }
}

@MainActor
struct StreamToastView: View {
    let toast: Toast
    let model: StreamModel

    var body: some View {
        let tint: Color
        switch toast.tone {
        case .accent: tint = Theme.accentInk
        case .neutral: tint = Theme.text2
        case .warning: tint = Theme.warning
        case .success: tint = Theme.success
        }
        return HStack(spacing: 10) {
            Image(systemName: toast.symbol).font(.system(size: 14)).foregroundColor(tint)
            HStack(spacing: 6) {
                Text(toast.text).foregroundColor(Theme.text)
                if let s = toast.sub {
                    Text(s).foregroundColor(Theme.text2)
                }
            }
            .font(.system(size: 13))
            .lineLimit(1)
            if let a = toast.action, let id = toast.actionID {
                SwiftUI.Button(a) { model.actions?.toastAction(id) }
                    .buttonStyle(.plain)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundColor(Theme.accentInk)
                    .padding(.horizontal, 10)
                    .frame(height: 28)
            }
            SwiftUI.Button {
                model.actions?.dismissToast()
            } label: {
                Image(systemName: "xmark").font(.system(size: 10, weight: .semibold)).foregroundColor(Theme.text2)
                    .frame(width: 28, height: 28)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Dismiss")
        }
        .padding(.leading, 14)
        .padding(.trailing, 6)
        .frame(height: 40)
        .glassSurface(Theme.toastGlass, radius: 12)
    }
}

@MainActor
struct StreamPillHost: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        Group {
            if let p = model.pill {
                HStack(spacing: 8) {
                    Spinner(size: 16)
                    Text(p.text).font(.system(size: 13, weight: .medium)).foregroundColor(Theme.text)
                    if p == .reconnecting {
                        Text("\(Int(model.pillSince)) s").font(.system(size: 13)).foregroundColor(Theme.text2).monospacedDigit()
                    }
                }
                .padding(.leading, 12)
                .padding(.trailing, 14)
                .frame(height: 36)
                .glassSurface(Theme.toastGlass, radius: 18)
            } else {
                Color.clear.frame(width: 0, height: 0)
            }
        }
        .fixedSize()
    }
}

@MainActor
struct StreamResizeBadge: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        Group {
            if let b = model.resizeBadge {
                Text(b)
                    .font(.system(size: 12.5, weight: .semibold))
                    .monospacedDigit()
                    .foregroundColor(Theme.text)
                    .padding(.horizontal, 12)
                    .frame(height: 30)
                    .glassSurface(Theme.toastGlass, radius: 15, shadow: false)
            } else {
                Color.clear.frame(width: 0, height: 0)
            }
        }
        .fixedSize()
    }
}

// MARK: - Cards

@MainActor
struct StreamCardHost: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        ZStack {
            if let card = model.card {
                StreamCardBackdrop(card: card)
                StreamCardView(card: card, model: model)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// What's behind a card: the last frame blurred and dimmed, or nothing.
@MainActor
struct StreamCardBackdrop: View {
    let card: StreamCard

    var body: some View {
        switch card {
        case .connecting:
            ZStack {
                BlurView(material: .hudWindow, blending: .withinWindow)
                Theme.streamDim.opacity(0.7)
            }
        case .takenOver, .disconnected, .askTakeover, .secureScreen:
            ZStack {
                BlurView(material: .hudWindow, blending: .withinWindow).opacity(0.5)
                Theme.streamDim.opacity(0.85)
            }
        default:
            Theme.letterbox
        }
    }
}

@MainActor
struct StreamCardView: View {
    let card: StreamCard
    @ObservedObject var model: StreamModel

    var host: String { model.hostName }

    var body: some View {
        Group {
            switch card {
            case .connecting:
                StreamConnectingCard(model: model)
            case .askTakeover(let by):
                StreamCardParts(symbol: "laptopcomputer", title: "In use by \(by). Take over?",
                                text: "\(by) is streaming \(host) right now. Taking over ends that stream; nothing on the computer closes.") {
                    KitButton("Cancel", kind: .secondary, height: 32) { model.actions?.closeCard() }
                    KitButton("Take over", kind: .primary, height: 32) { model.actions?.takeOver() }
                }
            case .takenOver(let by, let at):
                StreamCardParts(symbol: "laptopcomputer", title: "Taken over by \(by)",
                                text: "You connected from \(by) at \(at), so this Mac was disconnected. Only one device streams at a time.") {
                    KitButton("Close", kind: .secondary, height: 32) { model.actions?.closeCard() }
                    KitButton("Take it back", kind: .primary, height: 32) { model.actions?.takeOver() }
                }
            case .offline(let lastSeen, let retryIn):
                if model.serverName == nil {
                    // No server: nothing answered, and the card says where to look and offers Server settings…
                    StreamCardParts(symbol: "network.slash", title: String(localized: "Couldn’t reach \(host)"),
                                    text: String(localized: "Nothing answered. Check the address and that the computer is on. If it’s on another network, add a server in Settings › Server."),
                                    retry: retryIn) {
                        KitButton("Close", kind: .secondary, height: 32) { model.actions?.closeCard() }
                        KitButton(String(localized: "Server settings…"), kind: .secondary, height: 32) { model.actions?.openServerSettings() }
                        KitButton("Try now", kind: .primary, height: 32) { model.actions?.reconnect() }
                    }
                } else {
                    StreamCardParts(symbol: "network.slash", title: "\(host) isn’t online",
                                    text: (lastSeen.map { "Last seen \($0). " } ?? "") + "It may be turned off or without internet. Remote keeps trying while this window is open.",
                                    retry: retryIn) {
                        KitButton("Close", kind: .secondary, height: 32) { model.actions?.closeCard() }
                        KitButton("Try now", kind: .primary, height: 32) { model.actions?.reconnect() }
                    }
                }
            case .asleep(let since, let retryIn):
                StreamCardParts(symbol: "moon.fill", title: "\(host) is asleep",
                                text: "Asleep since \(since). Remote keeps trying while this window is open.", retry: retryIn) {
                    KitButton("Close", kind: .secondary, height: 32) { model.actions?.closeCard() }
                    KitButton("Try now", kind: .primary, height: 32) { model.actions?.reconnect() }
                }
            case .disconnected(let title, let text):
                StreamCardParts(symbol: "wifi.slash", title: title, text: text) {
                    KitButton("Close", kind: .secondary, height: 32) { model.actions?.closeCard() }
                    KitButton("Reconnect", kind: .primary, height: 32) { model.actions?.reconnect() }
                }
            case .versionMismatch(let h, let c, let updateThisMac):
                // The side to update comes from the core (the one with the older version), never from comparing here.
                StreamCardParts(symbol: "arrow.down.circle",
                                title: updateThisMac ? String(localized: "Update Sodium Remote on this Mac") : String(localized: "Update Sodium Remote on \(host)"),
                                text: updateThisMac
                                    ? String(localized: "\(host) speaks Sodium Remote protocol \(h), which this Mac’s Sodium Remote (protocol \(c)) can’t talk to. Install the new version on this Mac.")
                                    : String(localized: "\(host) speaks Sodium Remote protocol \(h); this Mac’s Sodium Remote speaks \(c). Update Sodium Remote on that computer.")) {
                    KitButton("Close", kind: .secondary, height: 32) { model.actions?.closeCard() }
                    KitButton("How to update", kind: .secondary, height: 32) { model.actions?.howToUpdate() }
                }
            case .keyChanged:
                StreamCardParts(symbol: "xmark.octagon", title: "\(host)’s key changed",
                                text: "The computer answering as \(host) isn’t the one this Mac paired with. If it was reinstalled, forget it here, then pair it again.",
                                danger: true) {
                    KitButton("Forget this computer", kind: .danger, height: 32) { model.actions?.forgetHost() }
                }
            case .removed:
                StreamCardParts(symbol: "lock.fill", title: "This Mac was removed",
                                text: "\(host) no longer lists this Mac, so it won’t answer. Pair again with a new code from the computer.") {
                    KitButton("Forget it", kind: .secondary, height: 32) { model.actions?.forgetHost() }
                    KitButton("Pair again…", kind: .primary, height: 32) { model.actions?.pairAgain() }
                }
            case .macOffline:
                StreamCardParts(symbol: "wifi.slash", title: "This Mac is offline",
                                text: model.serverName.map { String(localized: "Sodium Remote can’t reach \($0) or find \(host) on this network. Check the Wi-Fi.") }
                                    ?? String(localized: "Sodium Remote can’t find \(host) on this network. Check the Wi-Fi.")) {
                    KitButton("Close", kind: .secondary, height: 32) { model.actions?.closeCard() }
                    KitButton("Try again", kind: .primary, height: 32) { model.actions?.reconnect() }
                }
            case .secureScreen(let os):
                SecureScreenCard(host: host, os: os) { model.actions?.disconnect() }
            case .hostNeedsPermission(let screen, let access):
                StreamCardParts(symbol: "display.trianglebadge.exclamationmark",
                                title: PermissionCardCopy.title(host: host, screenRecording: screen),
                                text: PermissionCardCopy.text(screenRecording: screen, accessibility: access)) {
                    KitButton("Close", kind: .secondary, height: 32) { model.actions?.closeCard() }
                    KitButton("Try again", kind: .primary, height: 32) { model.actions?.reconnect() }
                }
            case .controlOff(let os):
                StreamCardParts(symbol: "lock.fill", title: String(localized: "\(host) isn’t allowing control"),
                                text: PermissionCardCopy.controlOffText(os: os)) {
                    KitButton("Close", kind: .secondary, height: 32) { model.actions?.closeCard() }
                    KitButton("Try again", kind: .primary, height: 32) { model.actions?.reconnect() }
                }
            case .failed(let message):
                StreamCardParts(symbol: "exclamationmark.triangle", title: "Couldn’t connect to \(host)", text: message) {
                    KitButton("Close", kind: .secondary, height: 32) { model.actions?.closeCard() }
                    KitButton("Try again", kind: .primary, height: 32) { model.actions?.reconnect() }
                }
            }
        }
        .frame(width: 400)
    }
}

/// The words of the two cards a host's own switches cause. A Mac host's copy names "Sodium Remote" (untranslated,
/// passed in;); a SodiumOS computer's names its page "Settings › Remote".
enum PermissionCardCopy {
    private static let app = "Sodium Remote"

    static func title(host: String, screenRecording: Bool) -> String {
        if screenRecording { return String(localized: "\(host) can’t share its screen yet") }
        return String(localized: "\(host) can’t be controlled yet")
    }

    /// Which permission is missing: Screen Recording, Accessibility, or both.
    static func text(screenRecording: Bool, accessibility: Bool) -> String {
        if screenRecording && accessibility {
            return String(localized: "On that Mac, allow Screen Recording and Accessibility for \(app) in System Settings, then try again.")
        }
        if screenRecording {
            return String(localized: "On that Mac, allow Screen Recording for \(app) in System Settings, then try again.")
        }
        return String(localized: "On that Mac, allow Accessibility for \(app) in System Settings, then try again.")
    }

    /// Where to switch "Allow remote control" on, by what the computer runs (nil: not known yet).
    static func controlOffText(os: HostOS?) -> String {
        guard let os = os else { return String(localized: "Allow control on that computer, then try again.") }
        if os == .sodiumOS { return String(localized: "Open Settings › Remote on that computer and allow control, then try again.") }
        return String(localized: "Open \(app) on that computer and allow control, then try again.")
    }
}

/// The secure-screen notice: the last frame blurred behind, the card, Disconnect only. The
/// session resumes by itself; typing a password into the lock screen is the next milestone.
@MainActor
struct SecureScreenCard: View {
    let host: String
    let os: HostOS
    let disconnect: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(alignment: .top, spacing: 14) {
                CardIcon("lock.shield", bg: Theme.warningBg, fg: Theme.warning)
                VStack(alignment: .leading, spacing: 4) {
                    Text("\(host) is showing a secure screen")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Theme.text)
                        .fixedSize(horizontal: false, vertical: true)
                    message(for: os)
                        .font(.system(size: 13))
                        .foregroundColor(Theme.text2)
                        .lineSpacing(3)
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 8) {
                        Spinner(size: 14)
                        Text("Waiting for the desktop…").font(.system(size: 12)).foregroundColor(Theme.text2)
                    }
                    .padding(.top, 8)
                }
            }
            HStack {
                Spacer()
                LButton(title: "Disconnect", kind: .secondary, height: 32, action: disconnect)
            }
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 22)
        .glassSurface(Theme.sheetGlass, radius: 18)
    }

    private func message(for os: HostOS) -> Text {
        switch os {
        case .mac:
            return Text("Its lock screen can’t be shown or controlled from here. The session resumes when the desktop is back.")
        case .windows, .sodiumOS, .linux:
            return Text("Its lock, sign-in or UAC screen can’t be shown or controlled from here. The session resumes when the desktop is back.")
        }
    }
}

/// card_parts: an icon tile, the title, the text, buttons on the right.
@MainActor
struct StreamCardParts<Buttons: View>: View {
    let symbol: String
    let title: String
    let text: String
    var retry: Int? = nil
    var danger = false
    let buttons: Buttons

    init(symbol: String, title: String, text: String, retry: Int? = nil, danger: Bool = false, @ViewBuilder buttons: () -> Buttons) {
        self.symbol = symbol
        self.title = title
        self.text = text
        self.retry = retry
        self.danger = danger
        self.buttons = buttons()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(alignment: .top, spacing: 14) {
                CardIcon(symbol, bg: danger ? Theme.dangerBg : Theme.raised, fg: danger ? Theme.dangerFg : Theme.icon)
                VStack(alignment: .leading, spacing: 4) {
                    Text(title).font(.system(size: 16, weight: .semibold)).foregroundColor(Theme.text)
                    Text(text).font(.system(size: 13)).foregroundColor(Theme.text2).lineSpacing(3).fixedSize(horizontal: false, vertical: true)
                    if let r = retry {
                        HStack(spacing: 8) {
                            Spinner(size: 14)
                            Text(r > 0 ? "Trying again in \(r) s" : "Trying again…").font(.system(size: 12)).foregroundColor(Theme.text2)
                        }
                        .padding(.top, 8)
                    }
                }
            }
            HStack(spacing: 8) {
                Spacer()
                buttons
            }
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 22)
        .glassSurface(Theme.sheetGlass, radius: 18)
    }
}

/// The connecting card: shown only after 600 ms.
@MainActor
struct StreamConnectingCard: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        let p = model.progress
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                AppTile(size: 36)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Connecting to \(model.hostName)").font(.system(size: 15, weight: .semibold)).foregroundColor(Theme.text)
                    Text(p.network.isEmpty ? "Usually under 2 s" : p.network).font(.system(size: 12)).foregroundColor(Theme.text2)
                }
            }
            .padding(.bottom, 14)
            VStack(alignment: .leading, spacing: 8) {
                StreamStep(state: stepState(0, p.step), label: "Find \(model.hostName)", detail: p.findDetail)
                StreamStep(state: stepState(1, p.step), label: "Pick the fastest path", detail: p.pathDetail)
                StreamStep(state: stepState(2, p.step), label: "Check its key", detail: p.keyDetail)
                StreamStep(state: stepState(3, p.step), label: "Start the stream", detail: p.sizeDetail)
            }
            HStack {
                Spacer()
                KitButton("Cancel", kind: .secondary, height: 30) { model.actions?.cancelConnect() }
            }
            .padding(.top, 16)
        }
        .padding(.horizontal, 24)
        .padding(.vertical, 22)
        .frame(width: 420)
        .glassSurface(Theme.sheetGlass, radius: 18)
    }

    func stepState(_ i: Int, _ now: Int) -> Int {
        i < now ? 2 : (i == now ? 1 : 0)
    }
}

/// One step: 0 to do, 1 now, 2 done.
@MainActor
struct StreamStep: View {
    let state: Int
    let label: String
    let detail: String

    var body: some View {
        HStack(spacing: 10) {
            Group {
                if state == 2 {
                    Image(systemName: "checkmark").font(.system(size: 9, weight: .bold)).foregroundColor(Theme.success)
                        .frame(width: 18, height: 18)
                        .background(Circle().fill(Theme.successBg))
                } else if state == 1 {
                    Spinner(size: 16).frame(width: 18, height: 18)
                } else {
                    Circle().strokeBorder(Theme.text3, lineWidth: 1.5).frame(width: 10, height: 10).frame(width: 18, height: 18)
                }
            }
            Text(label)
                .font(.system(size: 13, weight: state == 1 ? .semibold : .regular))
                .foregroundColor(state == 0 ? Theme.text3 : Theme.text)
            Spacer(minLength: 8)
            if !detail.isEmpty {
                Text(detail).font(.system(size: 12)).foregroundColor(Theme.text2).monospacedDigit().lineLimit(1)
            }
        }
        .frame(minHeight: 24)
    }
}

// MARK: - The stats HUD

@MainActor
struct StreamHudPillHost: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        Group {
            if model.hudMode != .off, let chip = model.chip {
                SwiftUI.Button {
                    model.actions?.cycleHud()
                } label: {
                    StreamStatsLine(hud: model.hud, chip: chip, separator: Theme.hairStrong)
                        .padding(.leading, 10)
                        .padding(.trailing, 12)
                        .frame(height: 28)
                        .glassSurface(Theme.hudGlass, radius: 14)
                }
                .buttonStyle(.plain)
            } else {
                Color.clear.frame(width: 0, height: 0)
            }
        }
        .fixedSize()
    }
}

@MainActor
struct StreamHudPanel: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        let h = model.hud
        return VStack(alignment: .leading, spacing: 0) {
            HStack {
                SectionHeader("Stats")
                Spacer()
                IconButton("doc.on.doc", label: "Copy the stats") { model.actions?.copyStats() }
                IconButton("xmark", label: "Close the stats") { model.actions?.closeHud() }
            }
            StreamHudStages(hud: h)
            StreamHudSection("Bitrate, last 30 s")
            StreamBitrate(values: h.frames.bitrate, mbps: h.frames.mbps)
            StreamHudSection("Stream")
            StreamHudStreamRows(hud: h)
            StreamHudSection("Paths")
            VStack(spacing: 0) {
                ForEach(h.paths) { p in
                    StreamPathRow(path: p)
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 12)
        .padding(.bottom, 14)
        .frame(width: 380)
        .glassSurface(Theme.hudGlass, radius: 16)
    }
}

/// The total, the stage bar, its legend and the stage rows.
@MainActor
struct StreamHudStages: View {
    let hud: HudData

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            StreamHudTotal(hud: hud).padding(.top, 4)
            StreamStageBar(stages: hud.frames.stages).padding(.top, 10)
            StreamStageLegend().padding(.top, 7)
            VStack(spacing: 0) {
                ForEach(hud.frames.stages) { s in
                    StreamStageRow(stage: s)
                }
            }
            .padding(.top, 8)
        }
    }
}

/// Size, codec, frames, loss, text refinement, Wi-Fi.
@MainActor
struct StreamHudStreamRows: View {
    let hud: HudData

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            StreamKV(key: "Size", value: hud.sizeText)
            StreamKV(key: "Codec", value: hud.codecText)
            StreamKV(key: "Frames", value: "\(hud.frames.fpsPresented) fps · \(hud.frames.late) late · \(hud.frames.dropped) dropped")
            StreamKV(key: "Loss", value: String(format: "%.1f %% · FEC %.0f %%", hud.lossPercent, hud.fecPercent))
            if let host = hud.frames.host, host.readableMs > 0 {
                let target = host.targetBps > 0 ? String(format: " · target %.0f Mb/s", Double(host.targetBps) / 1e6) : ""
                if host.settledMs > 0 {
                    StreamKV(key: "Text", value: String(format: "readable %d ms · settled %.1f s", host.readableMs, Double(host.settledMs) / 1000) + target)
                } else {
                    StreamKV(key: "Text", value: "readable \(host.readableMs) ms · lossless \(host.losslessMs) ms" + target)
                }
            }
            if let link = linkText {
                StreamKV(key: "Link", value: link)
            }
            StreamKV(key: "Wi-Fi", value: hud.awdl ? "AirDrop or Handoff is active" : "no AirDrop interference", warn: hud.awdl)
        }
    }

    /// "C_est 19 Mb/s · ceiling 23 Mb/s", each part only while set; nil when neither is.
    private var linkText: String? {
        guard let host = hud.frames.host else { return nil }
        var link: [String] = []
        if host.cEstBps > 0 { link.append(String(format: "C_est %.0f Mb/s", Double(host.cEstBps) / 1e6)) }
        if host.probeCeilingBps > 0 { link.append(String(format: "ceiling %.0f Mb/s", Double(host.probeCeilingBps) / 1e6)) }
        return link.isEmpty ? nil : link.joined(separator: " · ")
    }
}

@MainActor
struct StreamHudTotal: View {
    let hud: HudData

    var body: some View {
        let total: String = hud.frames.totalMs.map { String(format: "%.0f ms", $0) } ?? "– ms"
        return HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(total).font(.system(size: 30, weight: .semibold)).monospacedDigit().foregroundColor(Theme.text)
            Text("input to photon, estimated").font(.system(size: 12)).foregroundColor(Theme.text2)
        }
    }
}

@MainActor
struct StreamHudSection: View {
    let title: String

    init(_ title: String) {
        self.title = title
    }

    var body: some View {
        Text(title.uppercased())
            .font(.system(size: 11, weight: .semibold))
            .tracking(0.3)
            .foregroundColor(Theme.text3)
            .padding(.top, 12)
            .padding(.bottom, 4)
    }
}

func streamStageColor(_ g: StageGroup) -> Color {
    switch g {
    case .host: return Theme.stageHost
    case .net: return Theme.stageNet
    case .mac: return Theme.stageMac
    }
}

@MainActor
struct StreamStageBar: View {
    let stages: [HudStage]

    var body: some View {
        let known: [HudStage] = stages.filter { ($0.ms ?? 0) > 0 }
        let total: Double = known.reduce(0.0) { $0 + ($1.ms ?? 0) }
        return GeometryReader { geo in
            HStack(spacing: 1) {
                ForEach(Array(known.enumerated()), id: \.offset) { pair in
                    let w: CGFloat = total > 0 ? CGFloat((pair.element.ms ?? 0) / total) * max(geo.size.width - CGFloat(known.count), 0) : 0
                    Rectangle()
                        .fill(streamStageColor(pair.element.group).opacity(pair.offset % 2 == 0 ? 1 : 0.72))
                        .frame(width: w, height: 10)
                }
            }
        }
        .frame(height: 10)
        .clipShape(RoundedRectangle(cornerRadius: 4))
    }
}

@MainActor
struct StreamStageLegend: View {
    var body: some View {
        HStack(spacing: 14) {
            legend(Theme.stageHost, "The computer")
            legend(Theme.stageNet, "Network")
            legend(Theme.stageMac, "This Mac")
        }
        .font(.system(size: 11.5))
        .foregroundColor(Theme.text2)
    }

    func legend(_ c: Color, _ t: String) -> some View {
        HStack(spacing: 5) {
            RoundedRectangle(cornerRadius: 2).fill(c).frame(width: 8, height: 8)
            Text(t)
        }
    }
}

@MainActor
struct StreamStageRow: View {
    let stage: HudStage

    var body: some View {
        let value: String = stage.ms.map { String(format: "%.1f", $0) } ?? "–"
        return HStack(spacing: 8) {
            RoundedRectangle(cornerRadius: 1.5).fill(streamStageColor(stage.group)).frame(width: 3, height: 12)
            Text(stage.label + (stage.estimated ? " (est.)" : "")).foregroundColor(Theme.text2)
            Spacer()
            Text(value).monospacedDigit().foregroundColor(Theme.text)
        }
        .font(.system(size: 12))
        .frame(height: 19)
    }
}

@MainActor
struct StreamBitrate: View {
    let values: [Double]
    let mbps: Double

    var body: some View {
        let top: Double = max(values.max() ?? 1, 1)
        return HStack(alignment: .bottom, spacing: 10) {
            HStack(alignment: .bottom, spacing: 2) {
                ForEach(Array(values.enumerated()), id: \.offset) { pair in
                    RoundedRectangle(cornerRadius: 1)
                        .fill(Theme.stageHost)
                        .frame(height: max(2, CGFloat(pair.element / top) * 28))
                }
            }
            .frame(maxWidth: .infinity, minHeight: 28, maxHeight: 28, alignment: .bottomLeading)
            Text(String(format: mbps < 10 ? "%.1f Mb/s" : "%.0f Mb/s", mbps))
                .font(.system(size: 12))
                .monospacedDigit()
                .foregroundColor(Theme.text)
        }
    }
}

@MainActor
struct StreamKV: View {
    let key: String
    let value: String
    var warn = false

    var body: some View {
        HStack {
            Text(key).foregroundColor(Theme.text2)
            Spacer()
            Text(value).foregroundColor(warn ? Theme.warning : Theme.text).monospacedDigit().lineLimit(1)
        }
        .font(.system(size: 12))
        .frame(height: 19)
    }
}

@MainActor
struct StreamPathRow: View {
    let path: HudPath

    var body: some View {
        let rtt: String = path.rttMs.map { String(format: "%.0f ms", $0) } ?? "—"
        return HStack(spacing: 8) {
            RouteIcon(path.kind, size: 13).foregroundColor(path.inUse ? Theme.accentInk : Theme.text2)
            Text(path.kind.label)
                .font(.system(size: 12, weight: path.inUse ? .semibold : .regular))
                .foregroundColor(path.inUse ? Theme.text : Theme.text2)
            Spacer()
            Text(path.state).font(.system(size: 12)).foregroundColor(path.inUse ? Theme.text : Theme.text3)
            Text(rtt).font(.system(size: 12)).monospacedDigit().frame(width: 42, alignment: .trailing)
        }
        .padding(.horizontal, 6)
        .frame(height: 22)
        .background(RoundedRectangle(cornerRadius: 6).fill(path.inUse ? Theme.accentSoft : Color.clear))
        .padding(.horizontal, -6)
    }
}

// MARK: - The top-edge menu (full screen)

@MainActor
struct StreamRevealMenu: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        Group {
            if model.revealMenu {
                StreamRevealBar(model: model)
            } else {
                Color.clear.frame(width: 0, height: 0)
            }
        }
        .fixedSize()
    }
}

@MainActor
struct StreamRevealBar: View {
    @ObservedObject var model: StreamModel

    var body: some View {
        HStack(spacing: 2) {
            HStack(spacing: 8) {
                AppTile(size: 22)
                Text(model.hostName).font(.system(size: 13, weight: .semibold)).foregroundColor(Theme.text)
            }
            .padding(.leading, 4)
            .padding(.trailing, 6)
            if let chip = model.chip {
                PathChip(chip, height: 28) { model.actions?.openHudExpanded() }
            }
            StreamRevealSeparator()
            StreamRevealButton(symbol: "gauge.with.dots.needle.33percent", label: "Stats", text: nil, on: model.hudMode != .off) {
                model.actions?.cycleHud()
            }
            Menu {
                SwiftUI.Button("Mac keys (⌘ acts as Ctrl)") { model.actions?.setKeyboard(.mac) }
                SwiftUI.Button("PC keys (passed through)") { model.actions?.setKeyboard(.pc) }
            } label: {
                StreamRevealLabel(symbol: "keyboard", text: model.keyboard == .mac ? "Mac" : "PC")
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            if model.hostOffersHomeMonitor {
                Menu {
                    SwiftUI.Button("Mirror this stream") { model.actions?.setHomeMode(.mirror) }
                    SwiftUI.Button("Privacy screen") { model.actions?.setHomeMode(.privacy) }
                    SwiftUI.Button("Off") { model.actions?.setHomeMode(.off) }
                } label: {
                    StreamRevealLabel(symbol: "display", text: streamHomeLabel(model.homeMode))
                }
                .menuStyle(.borderlessButton)
                .menuIndicator(.hidden)
                .fixedSize()
            }
            if DisplayPicker.isShown(displays: model.displays.count, virtual: model.hostVirtual) {
                // A Windows or Mac host has no home monitor: its display picker takes that place.
                StreamDisplayItem(model: model)
            }
            StreamRevealButton(symbol: "gearshape", label: "Settings", text: nil, on: false) { model.actions?.openSettings() }
            StreamRevealSeparator()
            StreamRevealButton(symbol: "arrow.down.right.and.arrow.up.left", label: "Exit full screen", text: "Exit full screen", on: false) {
                model.actions?.toggleFullScreen()
            }
            StreamRevealButton(symbol: "xmark", label: "Disconnect", text: "Disconnect", on: false, danger: true) {
                model.actions?.disconnect()
            }
        }
        .padding(.horizontal, 7)
        .frame(height: 46)
        .glassSurface(Theme.sheetGlass, radius: 23)
    }
}

func streamHomeLabel(_ m: HomeMode) -> String {
    switch m {
    case .mirror: return "Mirror"
    case .privacy: return "Privacy"
    case .off: return "Off"
    }
}

@MainActor
struct StreamRevealSeparator: View {
    var body: some View {
        Rectangle().fill(Theme.hairStrong).frame(width: 1, height: 20).padding(.horizontal, 4)
    }
}

@MainActor
struct StreamRevealLabel: View {
    let symbol: String
    let text: String

    var body: some View {
        HStack(spacing: 7) {
            Image(systemName: symbol).font(.system(size: 14))
            Text(text).font(.system(size: 12.5, weight: .medium))
            Image(systemName: "chevron.down").font(.system(size: 9, weight: .semibold)).foregroundColor(Theme.text2)
        }
        .foregroundColor(Theme.text)
        .padding(.leading, 9)
        .padding(.trailing, 10)
        .frame(height: 32)
    }
}

@MainActor
struct StreamRevealButton: View {
    let symbol: String
    let label: String
    let text: String?
    let on: Bool
    var danger = false
    let action: () -> Void
    @State private var hovering = false

    init(symbol: String, label: String, text: String?, on: Bool, danger: Bool = false, action: @escaping () -> Void) {
        self.symbol = symbol
        self.label = label
        self.text = text
        self.on = on
        self.danger = danger
        self.action = action
    }

    var body: some View {
        SwiftUI.Button(action: action) {
            HStack(spacing: 7) {
                Image(systemName: symbol).font(.system(size: 14))
                if let t = text {
                    Text(t).font(.system(size: 12.5, weight: .medium)).lineLimit(1)
                }
            }
            .foregroundColor(danger ? Theme.dangerFg : Theme.text)
            .padding(.horizontal, text == nil ? 0 : 10)
            .frame(width: text == nil ? 34 : nil, height: 32)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(on ? Theme.open : (hovering ? Theme.hover2 : Color.clear)))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(label)
        .accessibilityLabel(label)
    }
}
