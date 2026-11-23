// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

extension LBanner where Action == EmptyView {
    init(kind: PillKind, symbol: String, message: Text) {
        self.init(kind: kind, symbol: symbol, message: message, action: { EmptyView() })
    }
}

/// The "This computer" page: the switch, the pairing code, paired devices, general settings and
/// the two permissions. Every state of the board: off, on with a code, on without one, being controlled,
/// each permission not allowed, waiting, "reopen to finish", and the view-only state.
@MainActor
struct HostPaneView: View {
    @ObservedObject var model: HostPaneModel

    var body: some View {
        ScrollView(.vertical) {
            VStack(alignment: .leading, spacing: 14) {
                if let c = model.controller {
                    ControlledHero(controller: c, model: model)
                }
                AllowGroup(model: model)
                if let e = model.startError {
                    LBanner(kind: .danger, symbol: "exclamationmark.triangle", message: Text("Couldn’t turn on remote control: \(e)"))
                }
                if model.isOn {
                    PairCard(model: model)
                } else {
                    OffPanel()
                }
                PairedGroup(model: model)
                GeneralGroup(model: model)
                PermissionsGroup(model: model)
                if model.isOn {
                    SecureNote()
                }
            }
            .padding(EdgeInsets(top: 16, leading: 20, bottom: 20, trailing: 20))
            .frame(maxWidth: 736, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
        .sheet(isPresented: revokeShown) {
            RevokeSheet(model: model)
        }
    }

    private var revokeShown: Binding<Bool> {
        Binding(get: { model.revokeAskID != nil }, set: { shown in
            if !shown { model.revokeAskID = nil }
        })
    }
}

// MARK: - The switch

@MainActor
private struct AllowGroup: View {
    @ObservedObject var model: HostPaneModel

    var body: some View {
        LGroup {
            LRow(Text("Allow remote control of this computer"),
                 sub: Text("Computers you’ve paired can see this screen and use its keyboard and mouse.")) {
                HStack(spacing: 12) {
                    if model.isOn {
                        LPill(text: "On", kind: .ok)
                    } else {
                        LPill(text: "Off", kind: .neutral)
                    }
                    LSwitch(label: "Allow remote control of this computer", isOn: Binding(
                        get: { model.isOn },
                        set: { model.actions?.setAllowed($0) }))
                        .disabled(model.starting)
                }
            }
            .padding(.vertical, 3)
        }
    }
}

@MainActor
private struct OffPanel: View {
    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: "lock.shield").font(.system(size: 26)).foregroundColor(Theme.text3)
            Text("Remote control is off").font(.system(size: 14, weight: .semibold)).foregroundColor(Theme.text)
            Text("Nobody can connect to this computer. Paired devices stay paired.")
                .font(.system(size: 12.5))
                .foregroundColor(Theme.text2)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 28)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.raised2))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.hair2, lineWidth: 1))
    }
}

// MARK: - Being controlled

@MainActor
private struct ControlledHero: View {
    let controller: ControllerModel
    @ObservedObject var model: HostPaneModel

    var body: some View {
        HStack(spacing: 14) {
            LeadTile(symbol: "display", bg: Theme.accentSoft, fg: Theme.accentInk, size: 36)
            VStack(alignment: .leading, spacing: 2) {
                Text("\(controller.name) is controlling this computer")
                    .font(.system(size: 13.5, weight: .semibold))
                    .foregroundColor(Theme.text)
                    .fixedSize(horizontal: false, vertical: true)
                Text("Since \(controller.since.formatted(date: .omitted, time: .shortened))")
                    .font(.system(size: 12))
                    .foregroundColor(Theme.text2)
            }
            Spacer(minLength: 8)
            LButton(title: "Disconnect", kind: .danger, height: 32) { model.actions?.disconnect() }
        }
        .padding(EdgeInsets(top: 12, leading: 16, bottom: 12, trailing: 16))
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.accentSoft))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.accentLine, lineWidth: 1))
    }
}

// MARK: - Pairing

@MainActor
private struct PairCard: View {
    @ObservedObject var model: HostPaneModel

    var body: some View {
        LGroup(title: "Pair a device") {
            VStack(alignment: .leading, spacing: 0) {
                if let p = model.pairing {
                    open(p)
                } else {
                    closed
                }
            }
            .padding(EdgeInsets(top: 14, leading: 16, bottom: 14, trailing: 16))
        }
    }

    private func open(_ p: HostPairing) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("On the other computer, choose Add a computer and type this code.")
                .font(.system(size: 12.5))
                .foregroundColor(Theme.text2)
                .fixedSize(horizontal: false, vertical: true)
            HStack(alignment: .center, spacing: 16) {
                CodeDigits(code: p.code)
                HStack(spacing: 6) {
                    Spinner(size: 14)
                    Countdown(until: p.expiresAt)
                }
            }
            HStack(spacing: 8) {
                LButton(title: "New code", kind: .secondary, icon: "arrow.clockwise") { model.actions?.openPairing() }
                LButton(title: "Close pairing", kind: .secondary) { model.actions?.closePairing() }
            }
        }
    }

    private var closed: some View {
        HStack(spacing: 14) {
            LeadTile(symbol: "lock.shield", size: 36)
            VStack(alignment: .leading, spacing: 2) {
                if model.pairingClosedByWrongCodes {
                    Text("Pairing closed").font(.system(size: 13.5, weight: .semibold)).foregroundColor(Theme.text)
                    Text("Someone typed three wrong codes, so pairing closed. Nobody can pair until you open it again.")
                        .font(.system(size: 12))
                        .foregroundColor(Theme.text2)
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    Text("No pairing code is open").font(.system(size: 13.5, weight: .semibold)).foregroundColor(Theme.text)
                    Text("A code works once, for 5 minutes, and three wrong tries withdraw it.")
                        .font(.system(size: 12))
                        .foregroundColor(Theme.text2)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 8)
            LButton(title: "Pair a device…", kind: .primary, height: 32) { model.actions?.openPairing() }
        }
    }
}

/// The 6 digits as two groups of three boxes that never shrink.
@MainActor
private struct CodeDigits: View {
    let code: String

    var body: some View {
        let digits = Array(code)
        return HStack(spacing: 14) {
            ForEach(0..<2, id: \.self) { g in
                HStack(spacing: 6) {
                    ForEach(0..<3, id: \.self) { i in
                        let k = g * 3 + i
                        Text(k < digits.count ? String(digits[k]) : "")
                            .font(.system(size: 30, weight: .semibold, design: .monospaced))
                            .foregroundColor(Theme.text)
                            .frame(width: 38, height: 52)
                            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Theme.input))
                            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Theme.hairStrong, lineWidth: 1))
                    }
                }
            }
        }
        .fixedSize()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: code))
    }
}

/// "Expires in 4:32", counting down.
@MainActor
private struct Countdown: View {
    let until: Date

    var body: some View {
        TimelineView(.periodic(from: Date(), by: 1)) { ctx in
            let left = max(0, Int(until.timeIntervalSince(ctx.date).rounded(.up)))
            Text("Expires in \(Self.clock(left))")
                .font(.system(size: 12))
                .foregroundColor(Theme.text2)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    static func clock(_ seconds: Int) -> String {
        let f = DateComponentsFormatter()
        f.allowedUnits = [.minute, .second]
        f.zeroFormattingBehavior = .pad
        f.unitsStyle = .positional
        return f.string(from: TimeInterval(seconds)) ?? "\(seconds / 60):\(String(format: "%02d", seconds % 60))"
    }
}

// MARK: - Paired devices

@MainActor
private struct PairedGroup: View {
    @ObservedObject var model: HostPaneModel

    var body: some View {
        LGroup(title: "Paired devices") {
            if model.devices.isEmpty {
                Text("No paired devices yet")
                    .font(.system(size: 12.5))
                    .foregroundColor(Theme.text2)
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 18)
            } else {
                ForEach(Array(model.devices.enumerated()), id: \.element.id) { item in
                    if item.offset > 0 { RowDivider() }
                    DeviceRow(device: item.element, model: model)
                }
            }
        }
    }
}

@MainActor
private struct DeviceRow: View {
    let device: PairedDeviceModel
    @ObservedObject var model: HostPaneModel

    var body: some View {
        LRow(Text(verbatim: device.name), sub: sub, lead: {
            LeadTile(symbol: symbol)
        }, trailing: {
            HStack(spacing: 6) {
                if device.connectedNow { LPill(text: "Connected now", kind: .accent) }
                LButton(title: "Remove", kind: .danger, height: 30) { model.revokeAskID = device.id }
            }
        })
    }

    private var symbol: String {
        switch device.os {
        case .mac: return "laptopcomputer"
        case .windows: return "desktopcomputer"
        case .sodiumOS: return "display"
        case .linux: return "display"
        case nil: return "questionmark.square.dashed"
        }
    }

    /// "macOS · Last used 2 hours ago", "Windows · Not used yet".
    private var sub: Text {
        let when = device.lastUsed.map { Self.relative($0) }
        switch (device.os, when) {
        case (let os?, let w?): return Text("\(os.pillText) · Last used \(w)")
        case (let os?, nil): return Text("\(os.pillText) · Not used yet")
        case (nil, let w?): return Text("Last used \(w)")
        case (nil, nil): return Text("Not used yet")
        }
    }

    static func relative(_ d: Date) -> String {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .full
        f.dateTimeStyle = .named
        return f.localizedString(for: d, relativeTo: Date())
    }
}

@MainActor
private struct RevokeSheet: View {
    @ObservedObject var model: HostPaneModel

    var body: some View {
        let id = model.revokeAskID ?? ""
        let name = model.devices.first(where: { $0.id == id })?.name ?? ""
        return VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Remove \(name)?").font(.system(size: 16, weight: .semibold)).foregroundColor(Theme.text)
                Text("It can’t connect to this computer again until you pair it again. If it’s connected now, it’s disconnected.")
                    .font(.system(size: 13))
                    .foregroundColor(Theme.text2)
                    .lineSpacing(3)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: 8) {
                Spacer()
                LButton(title: "Cancel", kind: .secondary, height: 32) { model.revokeAskID = nil }
                    .keyboardShortcut(.cancelAction)
                LButton(title: "Remove", kind: .dangerSolid, height: 32) {
                    model.revokeAskID = nil
                    if !id.isEmpty { model.actions?.revoke(deviceID: id) }
                }
            }
            .padding(.top, 18)
        }
        .padding(EdgeInsets(top: 22, leading: 24, bottom: 22, trailing: 24))
        .frame(width: 404)
        .background(Theme.dialogBg)
    }
}

// MARK: - General

@MainActor
private struct GeneralGroup: View {
    @ObservedObject var model: HostPaneModel

    var body: some View {
        LGroup {
            LRow(Text("Start at login"), sub: Text("Keeps this computer available after you sign in.")) {
                LSwitch(label: "Start at login", isOn: Binding(
                    get: { model.startAtLogin },
                    set: { model.actions?.setStartAtLogin($0) }))
            }
            RowDivider()
            LRow(Text("Name shown to other computers")) {
                NameField(model: model)
            }
            if let a = model.address, model.isOn {
                RowDivider()
                LRow(Text("Address for connecting directly")) {
                    HStack(spacing: 8) {
                        Text(verbatim: a)
                            .font(.system(size: 12, design: .monospaced))
                            .foregroundColor(Theme.text)
                            .textSelection(.enabled)
                            .multilineTextAlignment(.trailing)
                            .fixedSize(horizontal: false, vertical: true)
                        LButton(title: "Copy", kind: .secondary, height: 28) { model.actions?.copyAddress() }
                        if model.fullAddress != nil {
                            // The "…" menu of the Windows page: the full ID@IP:PORT.
                            Menu {
                                SwiftUI.Button("Copy full address") { model.actions?.copyFullAddress() }
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
                            .accessibilityLabel(Text("Copy full address"))
                        }
                    }
                }
            }
        }
    }
}

@MainActor
private struct NameField: View {
    @ObservedObject var model: HostPaneModel
    @State private var text = ""
    @FocusState private var focused: Bool

    var body: some View {
        TextField(text: $text, prompt: Text(verbatim: model.sharingName)) {
            Text("Name shown to other computers")
        }
        .labelsHidden()
        .textFieldStyle(.plain)
        .font(.system(size: 13))
        .foregroundColor(Theme.text)
        .padding(.horizontal, 8)
        .frame(width: 190, height: 28)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.input))
        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(focused ? Theme.accentInk : Theme.hair, lineWidth: focused ? 1.5 : 1))
        .focused($focused)
        .onAppear { text = model.name }
        .onChange(of: model.name) { _, n in if !focused { text = n } }
        .onSubmit { commit() }
        .onChange(of: focused) { _, f in if !f { commit() } }
    }

    private func commit() {
        let n = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if n != model.name { model.actions?.setName(n) }
    }
}

// MARK: - Permissions (the Mac's two steps)

@MainActor
private struct PermissionsGroup: View {
    @ObservedObject var model: HostPaneModel

    var body: some View {
        LGroup(title: "Permissions", foot: footnote) {
            PermissionRowView(kind: .screen, state: model.screenRecording, model: model)
            RowDivider()
            PermissionRowView(kind: .accessibility, state: model.accessibility, model: model)
            if model.viewOnly {
                ViewOnlyNote()
            }
        }
    }

    private var footnote: Text? {
        if model.isOn && model.screenRecording != .allowed {
            return Text("Until Screen Recording is allowed, computers that connect are told this Mac can’t share its screen yet.")
        }
        return nil
    }
}

/// The view-only state: the screen is shared, the keyboard and mouse aren't. The Mac
/// host also reports it to the connected computer (a later builder adds the protocol flag); here it is shown
/// from the two permission states alone.
@MainActor
private struct ViewOnlyNote: View {
    var body: some View {
        VStack(spacing: 0) {
            RowDivider()
            HStack(spacing: 10) {
                LPill(text: "View only", kind: .warn, icon: "eye")
                Text("Connected computers can see this screen but can’t use its keyboard and mouse until Accessibility is allowed.")
                    .font(.system(size: 12))
                    .foregroundColor(Theme.text2)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
        }
    }
}

private enum PermissionKind {
    case screen, accessibility
}

@MainActor
private struct PermissionRowView: View {
    let kind: PermissionKind
    let state: PermissionRowState
    @ObservedObject var model: HostPaneModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                LeadTile(symbol: kind == .screen ? "display" : "keyboard",
                         bg: state == .allowed ? Theme.accentSoft : (state == .reopenToFinish ? Theme.warningBg : Theme.raised),
                         fg: state == .allowed ? Theme.accentInk : (state == .reopenToFinish ? Theme.warning : Theme.icon))
                VStack(alignment: .leading, spacing: 2) {
                    title.font(.system(size: 13)).foregroundColor(Theme.text)
                    sub.font(.system(size: 12)).foregroundColor(Theme.text2).fixedSize(horizontal: false, vertical: true)
                }
                Spacer(minLength: 8)
                pill
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 11)
            .frame(minHeight: 58)
            if state == .notAllowed || state == .waiting {
                Steps(permissionName: permissionName, waiting: state == .waiting) {
                    if kind == .screen { model.actions?.allowScreenRecording() } else { model.actions?.allowAccessibility() }
                }
            }
            if kind == .screen && state == .reopenToFinish {
                LBanner(kind: .warn, symbol: "exclamationmark.triangle", message: Text("Sodium Remote needs to quit and reopen to use Screen Recording.")) {
                    LButton(title: "Quit and Reopen", kind: .secondary, height: 28) { model.actions?.quitAndReopen() }
                }
                .padding(EdgeInsets(top: 0, leading: 58, bottom: 12, trailing: 14))
            }
        }
    }

    private var title: Text {
        kind == .screen ? Text("Screen Recording") : Text("Accessibility")
    }

    private var sub: Text {
        kind == .screen ? Text("Lets other computers see this screen.") : Text("Lets other computers use this Mac’s keyboard and mouse.")
    }

    /// The translated name, for "Privacy & Security › <name>".
    private var permissionName: String {
        kind == .screen ? String(localized: "Screen Recording") : String(localized: "Accessibility")
    }

    @ViewBuilder private var pill: some View {
        switch state {
        case .allowed: LPill(text: "Allowed", kind: .ok, icon: "checkmark")
        case .notAllowed: LPill(text: "Not allowed", kind: .warn)
        case .waiting: LPill(text: "Waiting for System Settings", kind: .accent)
        case .reopenToFinish: LPill(text: "Reopen to finish", kind: .warn, icon: "arrow.clockwise")
        }
    }
}

/// The three steps and the button that opens System Settings at the right list.
@MainActor
private struct Steps: View {
    let permissionName: String
    let waiting: Bool
    let open: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            step(1, Text("Open System Settings › Privacy & Security › \(permissionName)."))
            step(2, Text("Turn on Sodium Remote in the list."))
            step(3, Text("Come back here. This page updates by itself."))
            // The button stays pressable while the row says Waiting (it reopens System Settings).
            HStack(spacing: 12) {
                LButton(title: "Open System Settings", kind: .primary, icon: "gearshape", height: 32, action: open)
                if waiting {
                    HStack(spacing: 8) {
                        Spinner(size: 16)
                        Text("Waiting for System Settings").font(.system(size: 12.5)).foregroundColor(Theme.text2)
                    }
                }
            }
            .padding(.top, 6)
        }
        .padding(EdgeInsets(top: 0, leading: 58, bottom: 16, trailing: 16))
    }

    private func step(_ n: Int, _ text: Text) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Text("\(n)")
                .font(.system(size: 11, weight: .bold))
                .foregroundColor(Theme.text)
                .frame(width: 20, height: 20)
                .background(Circle().fill(Theme.raised))
            text.font(.system(size: 12.5)).foregroundColor(Theme.text).fixedSize(horizontal: false, vertical: true)
        }
    }
}

@MainActor
private struct SecureNote: View {
    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "lock.fill").font(.system(size: 11)).foregroundColor(Theme.text3).padding(.top, 2)
            Text("The lock screen can’t be shown or controlled remotely.")
                .font(.system(size: 12))
                .foregroundColor(Theme.text2)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 4)
    }
}

// MARK: - Entry points for the app

public enum HostPaneUI {
    /// The page's content, to sit in the main window beside the computer list.
    @MainActor public static func makeView(model: HostPaneModel) -> some View {
        HostPaneView(model: model)
    }
}
