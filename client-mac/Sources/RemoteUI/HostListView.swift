// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

// The main window: the welcome, the empty state or the computers, and the
// status strip.

/// The main window's pieces, for the app's window controller.
public enum HostListUI {
    /// The window's content: the welcome (`model.showWelcome`), the empty
    /// state (no computers) or the card grid, with the status strip below.
    /// It draws its own glass background, under the toolbar too.
    @MainActor public static func makeContentView(model: AppModel) -> NSView {
        NSHostingView(rootView: HostListRoot(model: model))
    }

    /// "Sodium Remote" and the Computers / This computer control, for the toolbar's leading item.
    @MainActor public static func makeToolbarTitle(model: AppModel) -> NSView {
        let v = NSHostingView(rootView: ToolbarTitle(model: model))
        v.setFrameSize(v.fittingSize)
        return v
    }

    /// "Pair a computer…", for the toolbar's trailing item.
    @MainActor public static func makeToolbarPairButton(model: AppModel) -> NSView {
        let v = NSHostingView(rootView: ToolbarPairButton(model: model))
        v.setFrameSize(v.fittingSize)
        return v
    }
}

/// A small dialog for AppKit sheets ("In use by X. Take over?").
public enum DialogUI {
    @MainActor public static func makeController(symbol: String, title: String, body: String, cancel: String, confirm: String,
                                                 confirmKind: ButtonKind, onCancel: @escaping () -> Void,
                                                 onConfirm: @escaping () -> Void) -> NSViewController {
        let view = ConfirmDialogView(symbol: symbol, title: title, message: body, cancel: cancel, confirm: confirm, confirmKind: confirmKind,
                                     confirmIsDefault: confirmKind != .danger && confirmKind != .dangerSolid,
                                     onCancel: onCancel, onConfirm: onConfirm)
        let c = NSHostingController(rootView: view)
        c.sizingOptions = [.preferredContentSize]
        return c
    }
}

// MARK: - Toolbar items

@MainActor
private struct ToolbarTitle: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(spacing: 12) {
            // The product's name: not translated.
            Text(verbatim: "Sodium Remote")
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Theme.text)
            PageSegmented(page: $model.page)
        }
        .padding(.leading, 2)
        .fixedSize()
    }
}

@MainActor
private struct ToolbarPairButton: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(spacing: 6) {
            // Hidden, not removed, on This computer: the toolbar item keeps its size.
            KitButton("Pair a computer…", kind: .secondary, icon: "plus", height: 30) {
                model.actions?.openPairing(hostID: nil)
            }
            .opacity(model.page == .computers ? 1 : 0)
            .allowsHitTesting(model.page == .computers)
            IconButton("gearshape", label: "Settings") {
                model.actions?.openSettingsSheet()
            }
        }
        .fixedSize()
    }
}

// MARK: - The root

@MainActor
private struct HostListRoot: View {
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(spacing: 0) {
            HostListContent(model: model)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            StatusStrip(model: model)
        }
        .background(WindowGlass().ignoresSafeArea())
        .overlay(alignment: .bottom) {
            if let t = model.listToast {
                HStack(spacing: 8) {
                    Image(systemName: "checkmark").font(.system(size: 11, weight: .semibold)).foregroundColor(Theme.icon)
                    Text(t).font(.system(size: 12.5)).foregroundColor(Theme.text)
                }
                .padding(.horizontal, 14)
                .frame(height: 34)
                .background(Capsule().fill(Theme.toastGlass))
                .shadow(color: Theme.shadow, radius: 6, x: 0, y: 3)
                .padding(.bottom, 40)
                .transition(.opacity)
            }
        }
        .animation(.easeOut(duration: 0.15), value: model.listToast)
        .sheet(isPresented: forgetShown) {
            ForgetSheet(model: model)
        }
    }

    private var forgetShown: Binding<Bool> {
        Binding(get: { model.forgetAskID != nil }, set: { shown in
            if !shown { model.forgetAskID = nil }
        })
    }
}

/// The window's Graphite Glass: macOS's blur behind the window, tinted.
@MainActor
private struct WindowGlass: View {
    var body: some View {
        ZStack {
            BlurView(material: .underWindowBackground, blending: .behindWindow)
            Theme.winGlass
        }
    }
}

@MainActor
private struct HostListContent: View {
    @ObservedObject var model: AppModel

    var body: some View {
        if model.page == .thisComputer {
            HostPaneView(model: model.hostPane)
        } else if model.showWelcome {
            WelcomeView(model: model)
        } else if model.hosts.isEmpty {
            EmptyHostsView(model: model)
        } else {
            GeometryReader { geo in
                HostGridContent(model: model, width: geo.size.width)
            }
        }
    }
}

/// "This Mac: MacBook Air · ID 2HC9-LQ4T" and, only while a server is configured, whether it answers.
@MainActor
private struct StatusStrip: View {
    @ObservedObject var model: AppModel

    var body: some View {
        HStack(spacing: 12) {
            Text(verbatim: leftText).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 8)
            Text(verbatim: rightText).lineLimit(1)
        }
        .font(.system(size: 12))
        .foregroundColor(Theme.text3)
        .padding(.horizontal, 16)
        .frame(height: 28)
        .overlay(alignment: .top) {
            Rectangle().fill(Theme.hair).frame(height: 1)
        }
    }

    private var leftText: String {
        let name = model.thisMacName
        if model.thisMacCode.isEmpty { return String(localized: "This Mac: \(name)") }
        let code = model.thisMacCode
        return String(localized: "This Mac: \(name) · ID \(code)")
    }

    /// Nothing with no server: not having one is a normal state, never a warning.
    private var rightText: String {
        guard let name = model.serverName else { return "" }
        switch model.rendezvous {
        case .online: return String(localized: "\(name) reachable")
        case .offline: return String(localized: "\(name) unreachable")
        case .connecting: return String(localized: "Connecting to \(name)…")
        case .off: return ""
        }
    }
}

// MARK: - The grid

@MainActor
private struct HostGridContent: View {
    @ObservedObject var model: AppModel
    let width: CGFloat

    var body: some View {
        // Cards reflow: 3 columns at 1000 pt, 2 at 560, 1 under 560; under
        // 480 pt each computer is a row with a small picture.
        let narrow: Bool = width < 480
        let cols: Int = width >= 1000 ? 3 : (width >= 560 ? 2 : 1)
        let pad: CGFloat = narrow ? 12 : 22
        let avail: CGFloat = max(width - pad * 2, 200)
        let gaps: CGFloat = CGFloat(cols - 1) * 14
        let cardW: CGFloat = min(300, (avail - gaps) / CGFloat(cols))
        let items: [GridItem] = Array(repeating: GridItem(.fixed(cardW), spacing: 14, alignment: .topLeading), count: cols)
        return ScrollView(.vertical) {
            VStack(alignment: .leading, spacing: 12) {
                if !narrow {
                    GridHeader()
                }
                if narrow {
                    VStack(spacing: 4) {
                        ForEach(model.hosts) { card in
                            HostRow(card: card, model: model)
                        }
                    }
                } else {
                    LazyVGrid(columns: items, alignment: .leading, spacing: 14) {
                        ForEach(model.hosts) { card in
                            HostCard(card: card, model: model, width: cardW)
                        }
                    }
                }
            }
            .padding(.horizontal, pad)
            .padding(.vertical, narrow ? 12 : 18)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

@MainActor
private struct GridHeader: View {
    var body: some View {
        HStack(alignment: .lastTextBaseline) {
            SectionHeader("Computers")
            Spacer(minLength: 8)
            Text("Double-click or Return connects")
                .font(.system(size: 12))
                .foregroundColor(Theme.text3)
        }
        .padding(.horizontal, 2)
    }
}

// MARK: - A card

/// The scrim under the hover Connect button.
private let hoverScrim: Color = Theme.dyn(Theme.rgba(8, 9, 16, 0.28), Theme.rgba(20, 24, 40, 0.12))

@MainActor
private struct HostCard: View {
    let card: HostCardModel
    @ObservedObject var model: AppModel
    let width: CGFloat
    @State private var hovering = false
    @FocusState private var focused: Bool

    var body: some View {
        let ring: Color = focused ? Theme.accentInk : (hovering ? Theme.hairStrong : Color.clear)
        let ringWidth: CGFloat = focused ? 2 : 1
        return VStack(alignment: .leading, spacing: 0) {
            CardPicture(card: card, width: width - 16, hovering: hovering) {
                connect()
            }
            CardInfo(card: card, model: model)
        }
        .padding(EdgeInsets(top: 8, leading: 8, bottom: 12, trailing: 8))
        .frame(width: width, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(hovering ? Theme.hover : Theme.raised2))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.hair2, lineWidth: 1))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(ring, lineWidth: ringWidth))
        .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .onHover { hovering = $0 }
        .onTapGesture {
            if model.renamingID != card.id { connect() }
        }
        .focusable()
        .focusEffectDisabled()
        .focused($focused)
        .onKeyPress(.return) {
            if model.renamingID == card.id { return .ignored }
            connect()
            return .handled
        }
        .contextMenu {
            HostMenuItems(card: card, model: model)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(card.name)
    }

    private func connect() {
        model.actions?.connect(hostID: card.id, fullScreen: false)
    }
}

/// The last frame, its state chip, and Connect on hover.
@MainActor
private struct CardPicture: View {
    let card: HostCardModel
    let width: CGFloat
    let hovering: Bool
    let onConnect: () -> Void

    var body: some View {
        let height: CGFloat = (width * 10 / 16).rounded()
        return ZStack(alignment: .topLeading) {
            Thumbnail(image: card.thumbnail, state: card.state)
                .frame(width: width, height: height)
                .clipped()
            PictureOverlay(state: card.state, hovering: hovering, onConnect: onConnect)
                .frame(width: width, height: height)
            StateChip(state: card.state)
                .padding(8)
            if let os = card.os {
                OSChip(os: os)
                    .padding(8)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topTrailing)
            }
        }
        .frame(width: width, height: height)
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.hair, lineWidth: 1))
    }
}

@MainActor
private struct PictureOverlay: View {
    let state: HostState
    let hovering: Bool
    let onConnect: () -> Void

    var body: some View {
        if state == .connecting {
            ZStack {
                Theme.scrim
                VStack(spacing: 8) {
                    Spinner(size: 22)
                    Text("Connecting…")
                        .font(.system(size: 12.5, weight: .semibold))
                        .foregroundColor(Theme.text)
                }
            }
        } else if hovering {
            ZStack {
                hoverScrim
                KitButton("Connect", kind: .primary, icon: "display", height: 34, action: onConnect)
            }
        } else {
            Color.clear
        }
    }
}

/// The last frame the Mac saw, dimmed, greyed or blurred by state.
@MainActor
private struct Thumbnail: View {
    let image: NSImage?
    let state: HostState

    var body: some View {
        picture
            .saturation(saturation)
            .brightness(brightness)
            .blur(radius: blur)
            .opacity(opacity)
    }

    @ViewBuilder private var picture: some View {
        if let img = image {
            Image(nsImage: img)
                .resizable()
                .aspectRatio(contentMode: .fill)
        } else {
            // No picture (Show previews is off, or none saved yet): a flat tile, the card keeps its size.
            ZStack {
                Theme.ground
                Image(systemName: "display")
                    .font(.system(size: 22))
                    .foregroundColor(Theme.icon.opacity(0.5))
            }
        }
    }

    private var saturation: Double {
        switch state {
        case .offline: return 0
        case .asleep: return 0.4
        case .busy: return 0.6
        default: return 1
        }
    }

    private var brightness: Double {
        switch state {
        case .offline: return -0.25
        case .asleep: return -0.45
        default: return 0
        }
    }

    private var blur: CGFloat {
        if state == .locked { return 8 }
        return 0
    }

    private var opacity: Double {
        if state == .offline { return 0.55 }
        return 1
    }
}

/// The state chip on the picture, on glass.
@MainActor
private struct StateChip: View {
    let state: HostState

    var body: some View {
        pill
            .background(Capsule().fill(Theme.toastGlass))
            .shadow(color: Theme.shadow, radius: 6, x: 0, y: 3)
    }

    @ViewBuilder private var pill: some View {
        switch state {
        case .online: StatusPill("Online", kind: .ok)
        case .locked: StatusPill("Locked", kind: .neutral, icon: "lock.fill")
        case .busy: StatusPill("In use", kind: .warn)
        case .offline: StatusPill("Offline", kind: .neutral)
        case .asleep: StatusPill("Asleep", kind: .neutral, icon: "moon.fill")
        case .connecting: StatusPill("Connecting…", kind: .accent)
        }
    }
}

/// The computer's OS as words on its picture (no vendor logos; not translated).
@MainActor
private struct OSChip: View {
    let os: HostOS

    var body: some View {
        StatusPill(verbatim: os.pillText, kind: .neutral)
            .background(Capsule().fill(Theme.toastGlass))
            .shadow(color: Theme.shadow, radius: 6, x: 0, y: 3)
    }
}

/// The name (or its rename field), the ⋯ menu, the line and the path.
@MainActor
private struct CardInfo: View {
    let card: HostCardModel
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                if model.renamingID == card.id {
                    RenameField(card: card, model: model)
                } else {
                    Text(card.name)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(Theme.text)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                Spacer(minLength: 4)
                MoreMenu(card: card, model: model)
            }
            Text(card.line)
                .font(.system(size: 12))
                .foregroundColor(Theme.text2)
                .lineLimit(1)
                .truncationMode(.tail)
                .padding(.top, 1)
            if let p = card.pathText {
                PathLine(band: card.band, text: p)
                    .padding(.top, 4)
            }
        }
        .padding(EdgeInsets(top: 10, leading: 6, bottom: 0, trailing: 4))
    }
}

@MainActor
private struct PathLine: View {
    let band: QualityBand
    let text: String

    var body: some View {
        HStack(spacing: 7) {
            QualityBars(band)
            Text(text)
                .font(.system(size: 12))
                .foregroundColor(Theme.text2)
                .lineLimit(1)
                .truncationMode(.tail)
        }
    }
}

/// Rename in place: Return keeps it, Esc undoes.
@MainActor
private struct RenameField: View {
    let card: HostCardModel
    @ObservedObject var model: AppModel
    @State private var text = ""
    @FocusState private var focused: Bool

    var body: some View {
        TextField("Name", text: $text)
            .textFieldStyle(.plain)
            .font(.system(size: 14, weight: .semibold))
            .foregroundColor(Theme.text)
            .padding(.horizontal, 8)
            .frame(height: 28)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.input))
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Theme.accentInk, lineWidth: 1.5))
            .focused($focused)
            .onAppear {
                text = card.name
                focused = true
            }
            .onSubmit {
                commit()
            }
            .onExitCommand {
                model.renamingID = nil
            }
            .accessibilityLabel("Name")
    }

    private func commit() {
        let name = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !name.isEmpty && name != card.name {
            model.actions?.rename(hostID: card.id, to: name)
        }
        model.renamingID = nil
    }
}

/// The ⋯ button and its menu.
@MainActor
private struct MoreMenu: View {
    let card: HostCardModel
    @ObservedObject var model: AppModel

    var body: some View {
        Menu {
            HostMenuItems(card: card, model: model)
        } label: {
            Image(systemName: "ellipsis")
                .font(.system(size: 14, weight: .regular))
                .foregroundColor(Theme.text2)
                .frame(width: 28, height: 28)
                .contentShape(Rectangle())
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .fixedSize()
        .accessibilityLabel("More for \(card.name)")
    }
}

/// The computer menu: also on right-click.
@MainActor
private struct HostMenuItems: View {
    let card: HostCardModel
    let model: AppModel

    var body: some View {
        SwiftUI.Button("Connect") {
            model.actions?.connect(hostID: card.id, fullScreen: false)
        }
        SwiftUI.Button("Connect in full screen") {
            model.actions?.connect(hostID: card.id, fullScreen: true)
        }
        Divider()
        SwiftUI.Button("Rename…") {
            model.renamingID = card.id
        }
        SwiftUI.Button("Copy ID") {
            model.actions?.copyID(hostID: card.id)
        }
        Divider()
        SwiftUI.Button("Forget this computer…", role: .destructive) {
            model.forgetAskID = card.id
        }
    }
}

// MARK: - A row, in narrow windows

@MainActor
private struct HostRow: View {
    let card: HostCardModel
    @ObservedObject var model: AppModel
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            Thumbnail(image: card.thumbnail, state: card.state)
                .frame(width: 88, height: 55)
                .clipShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
            RowText(card: card, model: model)
            Spacer(minLength: 4)
            MoreMenu(card: card, model: model)
        }
        .padding(8)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(hovering ? Theme.accentSel : Color.clear))
        .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .onHover { hovering = $0 }
        .onTapGesture {
            if model.renamingID != card.id {
                model.actions?.connect(hostID: card.id, fullScreen: false)
            }
        }
        .contextMenu {
            HostMenuItems(card: card, model: model)
        }
    }
}

@MainActor
private struct RowText: View {
    let card: HostCardModel
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 8) {
                if model.renamingID == card.id {
                    RenameField(card: card, model: model)
                } else {
                    Text(card.name)
                        .font(.system(size: 13.5, weight: .semibold))
                        .foregroundColor(Theme.text)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
                StateChip(state: card.state)
            }
            Text(card.line)
                .font(.system(size: 12))
                .foregroundColor(Theme.text2)
                .lineLimit(1)
                .truncationMode(.tail)
            if let p = card.pathText {
                PathLine(band: card.band, text: p)
            }
        }
    }
}

// MARK: - Empty: nothing paired yet

@MainActor
private struct EmptyHostsView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        ScrollView(.vertical) {
            VStack(spacing: 18) {
                EmptyHero()
                HowToPair(model: model)
                if !model.nearby.isEmpty {
                    NearbyGroup(model: model)
                        .frame(maxWidth: 520)
                }
            }
            .padding(EdgeInsets(top: 6, leading: 28, bottom: 20, trailing: 28))
            .frame(maxWidth: .infinity)
        }
    }
}

@MainActor
private struct EmptyHero: View {
    var body: some View {
        VStack(spacing: 10) {
            AppTile(size: 52)
            Text("No computers yet")
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(Theme.text)
            Text("Pair this Mac with a SodiumOS computer to stream it here.")
                .font(.system(size: 13))
                .foregroundColor(Theme.text2)
                .multilineTextAlignment(.center)
        }
        .padding(.top, 26)
        .padding(.bottom, 4)
    }
}

@MainActor
private struct HowToPair: View {
    let model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            PairStep(number: 1, text: Text("On the computer, open **Settings › Remote**."))
            PairStep(number: 2, text: Text("Choose **Pair a device**. It shows a 6-digit code for 5 minutes."))
            PairStep(number: 3, text: Text("Type the code here."))
            HStack {
                Spacer()
                KitButton("Enter a code…", kind: .primary, height: 34) {
                    model.actions?.openPairing(hostID: nil)
                }
                Spacer()
            }
            .padding(.top, 4)
        }
        .padding(EdgeInsets(top: 14, leading: 16, bottom: 14, trailing: 16))
        .frame(width: 380)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.raised2))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.hair2, lineWidth: 1))
    }
}

@MainActor
private struct PairStep: View {
    let number: Int
    let text: Text

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Text(verbatim: String(number))
                .font(.system(size: 11.5, weight: .bold))
                .foregroundColor(Theme.text)
                .frame(width: 20, height: 20)
                .background(Circle().fill(Theme.raised))
            text
                .font(.system(size: 13))
                .foregroundColor(Theme.text)
                .lineSpacing(2)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 1)
        }
    }
}

/// Computers with an open pairing window, found over mDNS (or through the configured server).
@MainActor
private struct NearbyGroup: View {
    @ObservedObject var model: AppModel

    var body: some View {
        KitGroup("On this network",
                 foot: "Found over the local network (mDNS). Pairing still needs the code, so a stranger on the same Wi-Fi gets nowhere.") {
            ForEach(Array(model.nearby.enumerated()), id: \.element.id) { item in
                if item.offset > 0 {
                    RowDivider()
                }
                NearbyRow(host: item.element, model: model)
            }
        }
    }
}

@MainActor
private struct NearbyRow: View {
    let host: NearbyHost
    let model: AppModel

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "desktopcomputer")
                .font(.system(size: 14))
                .foregroundColor(Theme.icon)
                .frame(width: 30, height: 30)
                .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Theme.raised))
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: host.name)
                    .font(.system(size: 13))
                    .foregroundColor(Theme.text)
                    .lineLimit(1)
                Text(verbatim: sub)
                    .font(.system(size: 12))
                    .foregroundColor(Theme.text2)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            KitButton("Pair…", kind: .secondary, height: 30) {
                model.actions?.openPairing(hostID: host.id)
            }
        }
        .padding(EdgeInsets(top: 10, leading: 14, bottom: 10, trailing: 14))
        .frame(minHeight: 46)
    }

    private var sub: String {
        if host.foundVia == "rendezvous", let name = model.serverName {
            return String(localized: "SodiumOS · found through \(name) · not paired yet")
        }
        return String(localized: "SodiumOS · found nearby · not paired yet")
    }
}

// MARK: - Dialogs

@MainActor
private struct ForgetSheet: View {
    @ObservedObject var model: AppModel

    var body: some View {
        let id: String = model.forgetAskID ?? ""
        let name: String = model.hosts.first(where: { $0.id == id })?.name ?? String(localized: "this computer")
        return ConfirmDialogView(
            symbol: nil,
            title: String(localized: "Forget \(name)?"),
            message: String(localized: "This Mac stops trusting its key and can’t connect until you pair again. \(name) still lists this Mac until you remove it there."),
            cancel: String(localized: "Cancel"),
            confirm: String(localized: "Forget"),
            confirmKind: .dangerSolid,
            confirmIsDefault: false,
            onCancel: {
                model.forgetAskID = nil
            },
            onConfirm: {
                model.forgetAskID = nil
                if !id.isEmpty {
                    model.actions?.forget(hostID: id)
                }
            })
    }
}

/// kit.dialog: an optional icon tile, the title, the body, buttons on the right.
@MainActor
private struct ConfirmDialogView: View {
    let symbol: String?
    let title: String
    let message: String
    let cancel: String
    let confirm: String
    let confirmKind: ButtonKind
    let confirmIsDefault: Bool
    let onCancel: () -> Void
    let onConfirm: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 14) {
                if let s = symbol {
                    CardIcon(s)
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text(verbatim: title)
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundColor(Theme.text)
                        .fixedSize(horizontal: false, vertical: true)
                    Text(verbatim: message)
                        .font(.system(size: 13))
                        .foregroundColor(Theme.text2)
                        .lineSpacing(3)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            HStack(spacing: 8) {
                Spacer()
                KitButton(verbatim: cancel, kind: .secondary, height: 32, action: onCancel)
                    .keyboardShortcut(.cancelAction)
                ConfirmButton(title: confirm, kind: confirmKind, isDefault: confirmIsDefault, action: onConfirm)
            }
            .padding(.top, 18)
        }
        .padding(EdgeInsets(top: 22, leading: 24, bottom: 22, trailing: 24))
        .frame(width: 404)
        .background(Theme.dialogBg)
    }
}

@MainActor
private struct ConfirmButton: View {
    let title: String
    let kind: ButtonKind
    let isDefault: Bool
    let action: () -> Void

    var body: some View {
        if isDefault {
            KitButton(verbatim: title, kind: kind, height: 32, action: action)
                .keyboardShortcut(.defaultAction)
        } else {
            KitButton(verbatim: title, kind: kind, height: 32, action: action)
        }
    }
}
