// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

/// The settings sheet: one set for every
/// computer, six sections down the side, everything applied at
/// once. Every control binds into `AppModel.settings`; the app observes it
/// and applies each change. Buttons go to `AppModel.settingsActions`.
public enum SettingsUI {
    /// The sheet's content, 720 pt wide; the app presents it as an AppKit
    /// sheet from the window's title bar.
    @MainActor
    public static func makeController(model: AppModel) -> NSViewController {
        let c = NSHostingController(rootView: SettingsSheetView(model: model))
        c.preferredContentSize = NSSize(width: SettingsMetrics.width, height: SettingsMetrics.height)
        return c
    }
}

enum SettingsMetrics {
    static let width: CGFloat = 720
    static let height: CGFloat = 560
    static let navWidth: CGFloat = 176
}

enum SettingsTab: CaseIterable, Hashable {
    case display, quality, input, home, audio, server, advanced, about

    var title: String {
        switch self {
        case .display: return "Display"
        case .quality: return "Quality"
        case .input: return "Input"
        case .home: return "Home monitor"
        case .audio: return "Audio"
        case .server: return String(localized: "Server")
        case .advanced: return "Advanced"
        case .about: return String(localized: "About")
        }
    }

    var symbol: String {
        switch self {
        case .display: return "display"
        case .quality: return "sparkles"
        case .input: return "keyboard"
        case .home: return "display.2"
        case .audio: return "speaker.wave.2"
        case .server: return "server.rack"
        case .advanced: return "gearshape"
        case .about: return "info.circle"
        }
    }
}

// MARK: - The sheet

@MainActor
struct SettingsSheetView: View {
    @ObservedObject var model: AppModel
    @State private var tab: SettingsTab

    init(model: AppModel) {
        self.model = model
        // "Server settings…" on the couldn't-reach card opens the sheet on the Server tab.
        // The About Sodium Remote menu item opens it on About.
        // An opened link or imported file to confirm (`pendingServer`) opens it on the Server tab too.
        let wanted: SettingsTabRequest? = model.pendingServer != nil ? SettingsTabRequest.server : model.openSettingsOn
        switch wanted {
        case .server?: _tab = State(initialValue: .server)
        case .about?: _tab = State(initialValue: .about)
        case nil: _tab = State(initialValue: .display)
        }
        model.openSettingsOn = nil
    }

    var body: some View {
        VStack(spacing: 0) {
            SettingsHeader(model: model)
            Rectangle().fill(Theme.hair).frame(height: 1)
            HStack(spacing: 0) {
                SettingsNav(tab: $tab)
                Rectangle().fill(Theme.hair).frame(width: 1)
                ScrollView(.vertical) {
                    SettingsContent(model: model, tab: tab)
                        .padding(EdgeInsets(top: 16, leading: 18, bottom: 18, trailing: 18))
                        .frame(maxWidth: .infinity, alignment: .topLeading)
                }
            }
        }
        .frame(width: SettingsMetrics.width, height: SettingsMetrics.height)
        .background(Theme.dialogBg)
        .onChange(of: model.pendingServer) { _, new in
            if new != nil { tab = .server }
        }
    }
}

@MainActor
private struct SettingsHeader: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(spacing: 12) {
            Text("Settings").font(.system(size: 16, weight: .semibold)).foregroundColor(Theme.text)
            Text("For every computer · changes apply at once").font(.system(size: 12)).foregroundColor(Theme.text2).lineLimit(1)
            Spacer(minLength: 8)
            KitButton("Done", kind: .primary, height: 30) {
                model.settingsActions?.settingsDone()
            }
        }
        .padding(EdgeInsets(top: 16, leading: 20, bottom: 14, trailing: 18))
    }
}

@MainActor
private struct SettingsNav: View {
    @Binding var tab: SettingsTab

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(SettingsTab.allCases, id: \.self) { t in
                SettingsNavItem(tab: t, on: t == tab) {
                    tab = t
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 10)
        .frame(width: SettingsMetrics.navWidth)
        .frame(maxHeight: .infinity, alignment: .top)
    }
}

@MainActor
private struct SettingsNavItem: View {
    let tab: SettingsTab
    let on: Bool
    let select: () -> Void

    var body: some View {
        SwiftUI.Button(action: select) {
            HStack(spacing: 10) {
                Image(systemName: tab.symbol)
                    .font(.system(size: 13))
                    .foregroundColor(on ? Theme.accentInk : Theme.icon)
                    .frame(width: 18)
                Text(tab.title)
                    .font(.system(size: 13, weight: on ? .medium : .regular))
                    .foregroundColor(on ? Theme.text : Theme.menuText)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 10)
            .frame(height: 32)
            .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(on ? Theme.accentSel : Color.clear))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

@MainActor
private struct SettingsContent: View {
    @ObservedObject var model: AppModel
    let tab: SettingsTab

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            switch tab {
            case .display:
                DisplayTab(model: model)
            case .quality:
                QualityTab(model: model)
            case .input:
                InputTab(model: model)
            case .home:
                if model.hostOffersHomeMonitor {
                    HomeTab(model: model)
                } else {
                    SettingsNoteLine(symbol: "display", text: "The connected computer has no desk monitor mode to set.")
                }
            case .audio:
                AudioTab(model: model)
            case .server:
                ServerTab(model: model)
            case .advanced:
                AdvancedTab(model: model)
            case .about:
                AboutTab(model: model)
            }
        }
    }
}

// MARK: - Shared pieces

/// A radio dot: 16 pt, filled with the accent when chosen.
@MainActor
struct SettingsRadioDot: View {
    let on: Bool

    var body: some View {
        ZStack {
            if on {
                Circle().fill(Theme.accent)
                Circle().fill(Theme.onAccent).frame(width: 6, height: 6)
            } else {
                Circle().fill(Theme.input)
                Circle().strokeBorder(Theme.hairStrong, lineWidth: 1.5)
            }
        }
        .frame(width: 16, height: 16)
    }
}

/// A radio row: dot, title and subtitle (a tap chooses it), then a control.
@MainActor
private struct SettingsRadioRow<Trailing: View>: View {
    let title: String
    let sub: String?
    let on: Bool
    let select: () -> Void
    let trailing: Trailing

    init(_ title: String, sub: String? = nil, on: Bool, select: @escaping () -> Void, @ViewBuilder trailing: () -> Trailing) {
        self.title = title
        self.sub = sub
        self.on = on
        self.select = select
        self.trailing = trailing()
    }

    var body: some View {
        HStack(spacing: 10) {
            HStack(spacing: 10) {
                SettingsRadioDot(on: on)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(.system(size: 13)).foregroundColor(Theme.text)
                    if let s = sub {
                        Text(s).font(.system(size: 12)).foregroundColor(Theme.text2).fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 8)
            }
            .contentShape(Rectangle())
            .onTapGesture {
                select()
            }
            trailing
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
        .frame(minHeight: 40)
        .accessibilityElement(children: .contain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

/// A small accent link ("Allow…", "Add a key…").
@MainActor
struct SettingsLink: View {
    let title: String
    let symbol: String?
    let action: () -> Void

    init(_ title: String, symbol: String? = nil, action: @escaping () -> Void) {
        self.title = title
        self.symbol = symbol
        self.action = action
    }

    var body: some View {
        SwiftUI.Button(action: action) {
            HStack(spacing: 4) {
                if let s = symbol {
                    Image(systemName: s).font(.system(size: 11, weight: .semibold))
                }
                Text(title).font(.system(size: 12.5, weight: .medium))
            }
            .foregroundColor(Theme.accentInk)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// A line of note text with an icon (kits.note_line).
@MainActor
private struct SettingsNoteLine: View {
    let symbol: String
    let text: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: symbol).font(.system(size: 11)).foregroundColor(Theme.text2)
            Text(text).font(.system(size: 12)).foregroundColor(Theme.text2).fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 4)
    }
}

// MARK: - Display (MSe1)

@MainActor
private struct DisplayTab: View {
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            ResolutionGroup(model: model)
            DisplayOptionsGroup(model: model)
        }
    }
}

@MainActor
private struct ResolutionGroup: View {
    @ObservedObject var model: AppModel

    static let fixedSizes: [PixelSize] = [
        PixelSize(width: 1280, height: 800),
        PixelSize(width: 1680, height: 1050),
        PixelSize(width: 1920, height: 1200),
        PixelSize(width: 2560, height: 1600),
    ]

    private var windowSub: String? {
        let w = model.settingsInfo.windowPixels
        return w.isEmpty ? nil : "Now \(w); follows every resize"
    }

    private var displaySub: String? {
        let d = model.settingsInfo.displayPixels
        return d.isEmpty ? nil : "\(d), the full screen size"
    }

    var body: some View {
        let res: ResolutionChoice = model.settings.resolution
        return KitGroup("Resolution") {
            SettingsRadioRow("Match the window", sub: windowSub, on: res == .matchWindow, select: { model.settings.resolution = .matchWindow }) {
                EmptyView()
            }
            RowDivider()
            SettingsRadioRow("Match the display", sub: displaySub, on: res == .matchDisplay, select: { model.settings.resolution = .matchDisplay }) {
                EmptyView()
            }
            RowDivider()
            SettingsRadioRow("A fixed size", on: res == .fixed, select: { model.settings.resolution = .fixed }) {
                Picker("Fixed size", selection: $model.settings.fixedSize) {
                    ForEach(ResolutionGroup.fixedSizes, id: \.self) { s in
                        Text("\(s.width) × \(s.height)").tag(s)
                    }
                }
                .labelsHidden()
                .pickerStyle(.menu)
                .frame(width: 150)
                .disabled(res != .fixed)
                .opacity(res == .fixed ? 1 : 0.45)
            }
            RowDivider()
            SettingsRadioRow("Custom", on: res == .custom, select: { model.settings.resolution = .custom }) {
                CustomSizeFields(model: model, enabled: res == .custom)
            }
        }
    }
}

/// Width × height for Custom, applied on Return (a half-typed number never
/// reaches the computer).
@MainActor
private struct CustomSizeFields: View {
    @ObservedObject var model: AppModel
    let enabled: Bool
    @State private var w = ""
    @State private var h = ""

    var body: some View {
        HStack(spacing: 6) {
            TextField("Width", text: $w)
                .frame(width: 64)
                .onSubmit {
                    commit()
                }
            Text("×").font(.system(size: 12.5)).foregroundColor(Theme.text3)
            TextField("Height", text: $h)
                .frame(width: 64)
                .onSubmit {
                    commit()
                }
        }
        .textFieldStyle(.roundedBorder)
        .font(.system(size: 12.5))
        .disabled(!enabled)
        .opacity(enabled ? 1 : 0.45)
        .onAppear {
            w = String(model.settings.customSize.width)
            h = String(model.settings.customSize.height)
        }
    }

    private func commit() {
        let digitsW = String(w.filter { $0.isNumber })
        let digitsH = String(h.filter { $0.isNumber })
        guard let width = Int(digitsW), let height = Int(digitsH) else { return }
        let cw = min(max(width, 640), 7680)
        let ch = min(max(height, 400), 4320)
        w = String(cw)
        h = String(ch)
        model.settings.customSize = PixelSize(width: cw, height: ch)
    }
}

@MainActor
private struct DisplayOptionsGroup: View {
    @ObservedObject var model: AppModel

    static let rates: [Int] = [30, 60, 90, 120, 144]

    /// 0 means the display's rate: show it chosen, and store 0 when picked.
    private var refreshBinding: Binding<Int> {
        let m = model
        let display = model.settingsInfo.displayHz
        return Binding<Int>(
            get: { m.settings.refreshHz == 0 ? display : m.settings.refreshHz },
            set: { v in m.settings.refreshHz = (v == display ? 0 : v) }
        )
    }

    private var foot: String {
        let s = model.settingsInfo.scaleText
        let scale = s.isEmpty ? "this Mac’s scale" : s
        return "UI size Match Mac runs the computer at \(scale), so text is the size of this Mac’s own."
    }

    var body: some View {
        let rateOptions: [(Int, String)] = DisplayOptionsGroup.rates.map { ($0, String($0)) }
        let sizeOptions: [(UISizeChoice, String)] = [
            (UISizeChoice.matchMac, "Match Mac"),
            (UISizeChoice.larger, "Larger"),
            (UISizeChoice.smaller, "Smaller"),
            (UISizeChoice.sameAsDesk, "Same as desk"),
        ]
        return KitGroup(foot: foot) {
            KitRow("Fixed resolution", sub: "Keep the computer’s size when this window resizes; the picture scales") {
                KitSwitch("Fixed resolution", isOn: $model.settings.fixedResolution)
            }
            RowDivider()
            KitRow("Refresh rate", sub: "This display is \(model.settingsInfo.displayHz) Hz; faster streams reach it sooner") {
                Segmented(rateOptions, selection: refreshBinding)
            }
            if model.hostOffersUISize {
                RowDivider()
                KitRow("UI size") {
                    Segmented(sizeOptions, selection: $model.settings.uiSize)
                }
            }
            RowDivider()
            KitRow("Full screen beside the camera", sub: "The computer’s bar would sit under the camera housing") {
                KitSwitch("Use the area beside the camera", isOn: $model.settings.fullScreenBesideCamera)
            }
        }
    }
}

// MARK: - Quality (MSe2)

@MainActor
private struct QualityTab: View {
    @ObservedObject var model: AppModel

    private var codecSub: String {
        if model.settings.codec == .hevc {
            return "HEVC 4:2:0: fewer bytes than 4:4:4"
        }
        return model.settingsInfo.hevc444Hardware
            ? "Auto: HEVC 4:4:4, which this Mac decodes in hardware"
            : "Auto: HEVC 4:2:0 (this Mac doesn’t decode 4:4:4 in hardware)"
    }

    var body: some View {
        let codecs: [(CodecPick, String)] = [(CodecPick.auto, "Auto"), (CodecPick.hevc, "HEVC"), (CodecPick.av1, "AV1")]
        let bandwidth: [(Bool, String)] = [(false, "Auto"), (true, "Limit")]
        let limited: Bool = model.settings.bandwidthLimited
        return VStack(alignment: .leading, spacing: 16) {
            KitGroup {
                KitRow("Codec", sub: codecSub) {
                    Segmented(codecs, selection: $model.settings.codec)
                }
                RowDivider()
                KitRow("Bandwidth", sub: "Auto measures the network every frame") {
                    Segmented(bandwidth, selection: $model.settings.bandwidthLimited)
                }
                RowDivider()
                KitRow("Limit to", sub: "For hotspots with a data cap") {
                    SettingsLimitSlider(model: model)
                        .disabled(!limited)
                        .opacity(limited ? 1 : 0.45)
                }
            }
            KitGroup("On slow networks", foot: "Decided: text sharp when still, motion smooth when moving; quality drops before frame rate.") {
                KitRow("Remote-optimised look", sub: "Under 15 Mb/s the computer swaps live blur for a cached one and pauses grain; above 25 Mb/s it comes back") {
                    KitSwitch("Remote-optimised look", isOn: $model.settings.optimisedLook)
                }
            }
        }
    }
}

@MainActor
private struct SettingsLimitSlider: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(spacing: 10) {
            Slider(value: $model.settings.limitMbps, in: 2...100, step: 1)
                .frame(width: 170)
                .tint(Theme.accent)
                .controlSize(.small)
            Text("\(Int(model.settings.limitMbps.rounded())) Mb/s")
                .font(.system(size: 12.5).monospacedDigit())
                .foregroundColor(Theme.text)
                .frame(width: 64, alignment: .leading)
        }
    }
}

// MARK: - Input (MSe4, MSe5)

@MainActor
private struct InputTab: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let keyboards: [(KeyboardMode, String)] = [(KeyboardMode.mac, "Mac keys"), (KeyboardMode.pc, "PC keys")]
        let keyboardSub: String = model.settings.keyboard == .mac ? "⌘ acts as Ctrl, as in the table below" : "⌘ is Win; keys pass through"
        return VStack(alignment: .leading, spacing: 16) {
            KitGroup {
                KitRow("Keyboard", sub: keyboardSub) {
                    Segmented(keyboards, selection: $model.settings.keyboard)
                }
            }
            KeyTableEditor(model: model)
            InputMethodGroup(model: model)
        }
    }
}

@MainActor
private struct InputMethodGroup: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let methods: [(InputMethodChoice, String)] = [(InputMethodChoice.mac, "This Mac’s"), (InputMethodChoice.computer, "The computer’s")]
        return KitGroup {
            KitRow("Input method", sub: "Type Chinese with this Mac’s input method; its candidates show at the caret") {
                Segmented(methods, selection: $model.settings.inputMethod)
            }
            RowDivider()
            SettingsShortcutsRow(model: model)
            RowDivider()
            KitRow("Release the keyboard", sub: "Until the next click in the stream") {
                HStack(spacing: 3) {
                    Keycap("⌃")
                    Keycap("⌥")
                    Keycap("⌘")
                    Keycap("K")
                }
            }
        }
    }
}

@MainActor
private struct SettingsShortcutsRow: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let allowed: Bool = model.settingsInfo.accessibility == .allowed
        return HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Send system shortcuts").font(.system(size: 13)).foregroundColor(Theme.text)
                HStack(spacing: 6) {
                    Text("⌘Tab, ⌘Space and the like · Accessibility: \(allowed ? "allowed" : "not allowed")")
                        .font(.system(size: 12))
                        .foregroundColor(Theme.text2)
                        .fixedSize(horizontal: false, vertical: true)
                    if !allowed {
                        SettingsLink("Allow…") {
                            model.settingsActions?.openSystemSettings(.accessibility)
                        }
                    }
                }
            }
            Spacer(minLength: 8)
            Picker("Send system shortcuts", selection: $model.settings.systemShortcuts) {
                ForEach(SystemShortcutsChoice.allCases, id: \.self) { c in
                    Text(c.label).tag(c)
                }
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .frame(width: 196)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .frame(minHeight: 46)
    }
}

// MARK: - Home monitor (MSe3)

@MainActor
private struct HomeTab: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let remember: [(RememberChoice, String)] = [(RememberChoice.thisMac, "This Mac"), (RememberChoice.thisSession, "This session")]
        let mode: HomeMode = model.settings.homeMode
        return VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 8) {
                SectionHeader("The monitor at the desk").padding(.horizontal, 4)
                HStack(alignment: .top, spacing: 10) {
                    SettingsHomeCard(kind: .mirror, title: "Mirror", sub: "The desk monitor shows this stream, with bars", on: mode == .mirror) {
                        model.settings.homeMode = .mirror
                    }
                    SettingsHomeCard(kind: .privacy, title: "Privacy screen", sub: "“In use remotely”; the desk’s keyboard and mouse do nothing",
                             on: mode == .privacy) {
                        model.settings.homeMode = .privacy
                    }
                    SettingsHomeCard(kind: .off, title: "Off", sub: "Asleep until you disconnect", on: mode == .off) {
                        model.settings.homeMode = .off
                    }
                }
            }
            KitGroup {
                KitRow("Remember for", sub: "The computer’s own default is in Settings › Remote") {
                    Segmented(remember, selection: $model.settings.rememberHome)
                }
            }
            SettingsNoteLine(symbol: "lock.fill", text: "Typing your password at the desk always ends the remote session and unlocks there.")
        }
    }
}

@MainActor
private struct SettingsHomeCard: View {
    let kind: HomeMode
    let title: String
    let sub: String
    let on: Bool
    let select: () -> Void

    var body: some View {
        SwiftUI.Button(action: select) {
            VStack(alignment: .leading, spacing: 8) {
                SettingsMonitorPicture(kind: kind)
                    .frame(maxWidth: .infinity)
                HStack(spacing: 8) {
                    SettingsRadioDot(on: on)
                    Text(title).font(.system(size: 13, weight: .semibold)).foregroundColor(Theme.text).lineLimit(1)
                }
                Text(sub)
                    .font(.system(size: 12))
                    .foregroundColor(Theme.text2)
                    .multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .topLeading)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Theme.raised2))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(on ? Theme.accentInk : Theme.hair2, lineWidth: on ? 2 : 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(title)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

/// The desk monitor, small: the mirrored stream with bars, the privacy
/// screen, or dark (kitr `monitor_pic`).
@MainActor
private struct SettingsMonitorPicture: View {
    let kind: HomeMode

    private static func c(_ v: UInt32) -> Color {
        Color(nsColor: Theme.hex(v))
    }

    var body: some View {
        ZStack {
            switch kind {
            case .mirror:
                Color.black
                LinearGradient(colors: [SettingsMonitorPicture.c(0x3A2C5E), SettingsMonitorPicture.c(0x251E45), SettingsMonitorPicture.c(0x151229)],
                               startPoint: .top, endPoint: .bottom)
                    .clipShape(RoundedRectangle(cornerRadius: 2))
                    .padding(.horizontal, 12)
                    .padding(.vertical, 3)
                VStack(spacing: 0) {
                    Rectangle().fill(Color.white.opacity(0.18)).frame(height: 4)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 3)
            case .privacy:
                LinearGradient(colors: [SettingsMonitorPicture.c(0x121733), SettingsMonitorPicture.c(0x2B2557), SettingsMonitorPicture.c(0x6E4A78)],
                               startPoint: .top, endPoint: .bottom)
                Text("In use remotely")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundColor(Color.white.opacity(0.9))
            case .off:
                SettingsMonitorPicture.c(0x050608)
                VStack {
                    Spacer(minLength: 0)
                    HStack {
                        Spacer(minLength: 0)
                        Circle().fill(SettingsMonitorPicture.c(0xE0873A)).frame(width: 5, height: 5)
                    }
                }
                .padding(EdgeInsets(top: 0, leading: 0, bottom: 5, trailing: 6))
            }
        }
        .frame(maxWidth: 150)
        .frame(height: 78)
        .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        .padding(3)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.hairStrong))
        .accessibilityHidden(true)
    }
}

// MARK: - Audio (MSe6)

@MainActor
private struct AudioTab: View {
    @ObservedObject var model: AppModel

    var body: some View {
        KitGroup("Sound from the computer", foot: "The microphone comes in a later version.") {
            KitRow("Play it on this Mac", sub: "It plays on the connected device: Opus, 48 kHz stereo, about 20 ms behind the picture") {
                KitSwitch("Play sound here", isOn: $model.settings.playSound)
            }
            RowDivider()
            KitRow("Output") {
                Picker("Output", selection: $model.settings.outputDevice) {
                    Text("System default").tag(Optional<String>.none)
                    ForEach(model.settingsInfo.outputs) { o in
                        Text(o.name).tag(Optional(o.uid))
                    }
                }
                .labelsHidden()
                .pickerStyle(.menu)
                .frame(width: 210)
                .disabled(!model.settings.playSound)
            }
            RowDivider()
            KitRow("The computer’s speakers", sub: "Its apps play to a virtual output, “Remote”, which plays here; nothing is muted") {
                Text("Quiet while connected").font(.system(size: 12.5)).foregroundColor(Theme.text2)
            }
        }
    }
}

// MARK: - Advanced (MSe7)

@MainActor
private struct AdvancedTab: View {
    @ObservedObject var model: AppModel

    var body: some View {
        return VStack(alignment: .leading, spacing: 16) {
            ComputersGroup(model: model)
            // The relay row exists only while a server is configured: with none there is nothing to name.
            if let server = model.serverName {
                RelayGroup(model: model, server: server)
            }
            if model.settingsInfo.awdl {
                SettingsAWDLBanner(model: model)
            }
            MeasuringGroup(model: model)
        }
    }
}

@MainActor
private struct RelayGroup: View {
    @ObservedObject var model: AppModel
    let server: String

    var body: some View {
        let down: Bool = { if case .offline = model.rendezvous { return true } else { return false } }()
        return KitGroup(String(localized: "Network")) {
            KitRow(String(localized: "Relay"),
                   sub: down ? String(localized: "\(server) is unreachable. Automatic uses the other paths.")
                             : String(localized: "\(server). Always through the relay applies from the next connection.")) {
                Picker(String(localized: "Relay"), selection: $model.settings.relay) {
                    Text(String(localized: "Automatic: the fastest path")).tag(RelayChoice.automatic)
                    Text(String(localized: "Always through the relay")).tag(RelayChoice.alwaysRelay)
                }
                .labelsHidden()
                .pickerStyle(.menu)
                .frame(width: 230)
                .disabled(down)
            }
        }
    }
}

@MainActor
private struct SettingsAWDLBanner: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            Image(systemName: "exclamationmark.triangle").font(.system(size: 14)).foregroundColor(Theme.warning)
            Text("AirDrop and Handoff are on. On Wi-Fi they can hold packets back for up to 90 ms while streaming.")
                .font(.system(size: 12.5))
                .foregroundColor(Theme.text)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            KitButton("How to turn them off", kind: .secondary, height: 28) {
                model.settingsActions?.showAWDLHelp()
            }
        }
        .padding(EdgeInsets(top: 8, leading: 12, bottom: 8, trailing: 8))
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Theme.warningBg))
    }
}

@MainActor
private struct MeasuringGroup: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let folder: String = model.settingsInfo.statsFolder.isEmpty ? "~/Library/Logs/Remote" : model.settingsInfo.statsFolder
        return KitGroup("Measuring") {
            KitRow("Stats files", sub: "Every session writes one: \(folder)") {
                KitButton("Show in Finder", kind: .secondary, height: 30) {
                    model.settingsActions?.showStatsFolder()
                }
            }
            RowDivider()
            KitRow("Keep") {
                Picker("Keep", selection: $model.settings.keepSessions) {
                    Text("The last 10 sessions").tag(10)
                    Text("The last 50 sessions").tag(50)
                    Text("Every session").tag(0)
                }
                .labelsHidden()
                .pickerStyle(.menu)
                .frame(width: 190)
            }
            RowDivider()
            KitRow("Latency test", sub: "Flips a pattern on the computer and times it to this screen") {
                KitButton("Run the test…", kind: .secondary, height: 30) {
                    model.settingsActions?.runLatencyTest()
                }
            }
        }
    }
}
