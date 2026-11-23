// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

// Settings › Server's sheets: Add a server (a pasted link or the details by hand), the confirmation every addition
// from a link or an imported file passes before anything is saved (a typed server is saved
// directly), Change the server, and Remove. The sheets never see
// the core: `AppModel.serverActions` (the app target) checks, saves and removes.

/// What the Server tab asks its sheet to be.
struct ServerSheetRequest: Identifiable {
    enum Kind {
        /// Start at the link field.
        case add
        /// The details of the current server, to change.
        case change
        /// A server from an opened link or an imported file: straight to the confirmation.
        case confirm(ServerCandidate)
        case remove
    }

    let id = UUID()
    let kind: Kind
}

// MARK: - The sheet's state

@MainActor
final class ServerSheetModel: ObservableObject {
    enum Stage { case link, manual, confirm }

    @Published var stage: Stage
    @Published var link = ""
    @Published var url = ""
    @Published var caName = ""
    @Published var caPEM = ""
    @Published var token = ""
    @Published var candidate: ServerCandidate?
    @Published var problem: ServerInputError?
    let isChange: Bool
    private weak var actions: ServerActions?
    private let finish: () -> Void

    init(kind: ServerSheetRequest.Kind, actions: ServerActions?, finish: @escaping () -> Void) {
        self.actions = actions
        self.finish = finish
        switch kind {
        case .change:
            isChange = true
            stage = .manual
            if let c = actions?.currentServer() {
                url = c.url
                caPEM = c.caPEM
                caName = c.caPEM.isEmpty ? "" : String(localized: "Current certificate")
                token = c.token
            }
        case .confirm(let c):
            isChange = false
            stage = .confirm
            candidate = c
        case .add, .remove:
            isChange = false
            stage = .link
        }
    }

    var canSubmit: Bool {
        switch stage {
        case .link: return !link.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        case .manual: return !url.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        case .confirm: return candidate != nil
        }
    }

    func paste() {
        guard let t = actions?.clipboardText() else { return }
        link = t.trimmingCharacters(in: .whitespacesAndNewlines)
        problem = nil
    }

    func chooseFile() {
        guard let f = actions?.chooseCertificateFile() else { return }
        caName = f.name
        caPEM = f.pem
        problem = nil
    }

    func clearFile() {
        caName = ""
        caPEM = ""
    }

    func goManual() {
        problem = nil
        stage = .manual
    }

    func goLink() {
        problem = nil
        stage = .link
    }

    /// Add / Save / the confirmation's Add.
    func submit() {
        guard let actions = actions else { return }
        switch stage {
        case .link:
            switch actions.inspectServerLink(link) {
            case .success(let c):
                problem = nil
                candidate = c
                stage = .confirm
            case .failure(let e):
                problem = e
            }
        case .manual:
            let address: String
            switch ServerAddress.normalize(url) {
            case .success(let a): address = a
            case .failure(.insecure):
                problem = .insecureAddress
                return
            case .failure(.invalid):
                problem = .badAddress
                return
            }
            switch actions.inspectServer(url: address, caPEM: caPEM, token: token.trimmingCharacters(in: .whitespacesAndNewlines)) {
            case .success(var c):
                // Add and Change save what was typed: the person has just entered every value. Only a link
                // or an imported file is confirmed first.
                problem = nil
                c.source = .typed
                actions.saveServer(c)
                finish()
            case .failure(let e):
                problem = e
            }
        case .confirm:
            if let c = candidate {
                actions.saveServer(c)
            }
            finish()
        }
    }
}

// MARK: - Add and Change

@MainActor
struct AddServerSheet: View {
    @StateObject private var sheet: ServerSheetModel
    let cancel: () -> Void

    init(model: AppModel, kind: ServerSheetRequest.Kind, done: @escaping () -> Void) {
        _sheet = StateObject(wrappedValue: ServerSheetModel(kind: kind, actions: model.serverActions, finish: done))
        cancel = done
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            switch sheet.stage {
            case .link: LinkStage(sheet: sheet)
            case .manual: ManualStage(sheet: sheet)
            case .confirm: ConfirmStage(sheet: sheet)
            }
            HStack(spacing: 8) {
                if sheet.stage == .manual && !sheet.isChange {
                    SettingsLink("Back to the link") { sheet.goLink() }
                }
                Spacer(minLength: 8)
                LButton(title: "Cancel", kind: .secondary, action: cancel)
                    .keyboardShortcut(.cancelAction)
                LButton(title: primaryTitle, kind: .primary) { sheet.submit() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!sheet.canSubmit)
                    .opacity(sheet.canSubmit ? 1 : 0.5)
            }
            .padding(.top, 18)
        }
        .padding(EdgeInsets(top: 20, leading: 22, bottom: 20, trailing: 22))
        .frame(width: 448, alignment: .leading)
        .background(Theme.dialogBg)
    }

    private var primaryTitle: LocalizedStringKey {
        sheet.isChange && sheet.stage == .manual ? "Save" : "Add"
    }
}

private struct SheetTitle: View {
    let title: Text
    var sub: Text?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            title.font(.system(size: 16, weight: .semibold)).foregroundColor(Theme.text)
            if let s = sub {
                s.font(.system(size: 13)).foregroundColor(Theme.text2).fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.bottom, 14)
    }
}

private struct FieldLabel: View {
    let text: LocalizedStringKey

    var body: some View {
        Text(text).font(.system(size: 12, weight: .semibold)).foregroundColor(Theme.text2).padding(.horizontal, 2).padding(.bottom, 6)
    }
}

private struct HelpLine: View {
    let text: LocalizedStringKey

    var body: some View {
        Text(text).font(.system(size: 12)).foregroundColor(Theme.text2).fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, 2).padding(.top, 6)
    }
}

private struct ErrorLine: View {
    let text: LocalizedStringKey

    var body: some View {
        HStack(alignment: .top, spacing: 6) {
            Image(systemName: "exclamationmark.circle").font(.system(size: 12)).foregroundColor(Theme.danger)
            Text(text).font(.system(size: 12.5)).foregroundColor(Theme.danger).fixedSize(horizontal: false, vertical: true)
        }
        .padding(.top, 8)
    }
}

@MainActor
private struct LinkStage: View {
    @ObservedObject var sheet: ServerSheetModel

    var body: some View {
        SheetTitle(title: Text("Add a server"), sub: Text("Paste the link from the person who runs the server."))
        FieldLabel(text: "Server link")
        HStack(alignment: .center, spacing: 8) {
            TextField("sodium-remote://server?…", text: $sheet.link)
                .textFieldStyle(.roundedBorder)
                .font(.system(size: 12.5, design: .monospaced))
                .onSubmit { sheet.submit() }
            LButton(title: "Paste", kind: .secondary, icon: "doc.on.clipboard") { sheet.paste() }
        }
        if sheet.problem == .notALink {
            ErrorLine(text: "That isn’t a Sodium Remote server link.")
        }
        HStack {
            Spacer(minLength: 0)
            SettingsLink("Enter the details yourself…") { sheet.goManual() }
        }
        .padding(.top, 12)
    }
}

@MainActor
private struct ManualStage: View {
    @ObservedObject var sheet: ServerSheetModel

    var body: some View {
        SheetTitle(title: sheet.isChange ? Text("Change the server") : Text("Add a server"),
                   sub: sheet.isChange ? Text("Enter the details of the new server. Paired computers stay paired.")
                                       : Text("Enter the details the person who runs the server gave you."))
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 0) {
                FieldLabel(text: "Server address")
                TextField("relay.example.org", text: $sheet.url)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 12.5, design: .monospaced))
                if sheet.problem == .insecureAddress {
                    ErrorLine(text: "Sodium Remote only connects to a server over an encrypted connection. Use an address that starts with https:// or wss://, or just the server’s name.")
                } else if sheet.problem == .badAddress {
                    ErrorLine(text: "That isn’t a valid server address.")
                }
            }
            VStack(alignment: .leading, spacing: 0) {
                FieldLabel(text: "Certificate authority file")
                HStack(spacing: 8) {
                    if sheet.caName.isEmpty {
                        Text("None chosen").font(.system(size: 13)).foregroundColor(Theme.text3)
                    } else {
                        Text(verbatim: sheet.caName).font(.system(size: 13)).foregroundColor(Theme.text)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 8)
                    if !sheet.caName.isEmpty {
                        SwiftUI.Button { sheet.clearFile() } label: {
                            Image(systemName: "xmark.circle.fill").foregroundColor(Theme.text3)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel(Text("Remove"))
                    }
                    LButton(title: "Choose a file…", kind: .secondary, height: 28) { sheet.chooseFile() }
                }
                .padding(.leading, 12).padding(.trailing, 6).padding(.vertical, 4)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Theme.input))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(sheet.problem == .badCertificate ? Theme.warning : Theme.hairStrong, lineWidth: 1))
                if sheet.problem == .badCertificate {
                    ErrorLine(text: "That file doesn’t hold a certificate. Choose the certificate authority’s PEM file.")
                } else {
                    HelpLine(text: "Only if the server uses its own certificate.")
                }
            }
            VStack(alignment: .leading, spacing: 0) {
                FieldLabel(text: "Enrollment token")
                SecureField("Optional", text: $sheet.token)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 12.5, design: .monospaced))
                HelpLine(text: "Only if the server asks for one.")
            }
        }
    }
}

/// nothing is saved before this: the host's name, and a plain sentence when the link carries its own certificate.
@MainActor
private struct ConfirmStage: View {
    @ObservedObject var sheet: ServerSheetModel

    var body: some View {
        if let c = sheet.candidate {
            SheetTitle(title: Text("Add \(c.label)?"), sub: nil)
            LGroup(title: groupTitle(c)) {
                if !c.official {
                    LRow(Text("Address"), sub: nil) {
                        Text(verbatim: c.url).font(.system(size: 12, design: .monospaced)).foregroundColor(Theme.text)
                            .fixedSize(horizontal: false, vertical: true)
                            .multilineTextAlignment(.trailing)
                    }
                    RowDivider()
                }
                LRow(Text("Certificate"), sub: nil) {
                    Text(certificateValue(c)).font(.system(size: 12.5)).foregroundColor(c.pinsCA ? Theme.warning : Theme.text2)
                }
                if !c.token.isEmpty {
                    RowDivider()
                    LRow(Text("Enrollment token"), sub: nil) {
                        Text(tokenValue(c)).font(.system(size: 12.5)).foregroundColor(Theme.text2)
                    }
                }
            }
            if c.pinsCA {
                Note(symbol: "exclamationmark.triangle", warn: true,
                     text: c.source == .link
                        ? Text("This link carries its own certificate. Sodium Remote will trust it for this server only, so continue only if you trust whoever sent the link.")
                        : Text("Sodium Remote will trust this certificate for this server only."))
                    .padding(.top, 10)
            }
            Note(symbol: "info.circle", warn: false, text: Text("Only add a server you trust. It can see which computers connect, and when."))
                .padding(.top, 10)
        }
    }

    private func groupTitle(_ c: ServerCandidate) -> LocalizedStringKey {
        switch c.source {
        case .link: return "Server in the link"
        case .file: return "Server in the file"
        case .typed: return "Server"
        }
    }

    private func tokenValue(_ c: ServerCandidate) -> LocalizedStringKey {
        c.source == .link ? "Included in the link" : "Included"
    }

    private func certificateValue(_ c: ServerCandidate) -> LocalizedStringKey {
        guard c.pinsCA else { return "Public" }
        return c.source == .link ? "Pinned by the link" : "Pinned by the file"
    }
}

private struct Note: View {
    let symbol: String
    let warn: Bool
    let text: Text

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: symbol).font(.system(size: 13)).foregroundColor(warn ? Theme.warning : Theme.text3)
            text.font(.system(size: 12)).foregroundColor(warn ? Theme.text : Theme.text2).fixedSize(horizontal: false, vertical: true)
        }
    }
}

// MARK: - Remove

@MainActor
struct RemoveServerSheet: View {
    let name: String
    let remove: () -> Void
    let cancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Remove \(name)?").font(.system(size: 16, weight: .semibold)).foregroundColor(Theme.text)
            Text("This computer stops using the server. Paired computers stay paired, and computers on this network and by address still work.")
                .font(.system(size: 13)).foregroundColor(Theme.text2).fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                Spacer(minLength: 8)
                LButton(title: "Cancel", kind: .secondary, action: cancel)
                    .keyboardShortcut(.cancelAction)
                LButton(title: "Remove", kind: .dangerSolid, action: remove)
            }
        }
        .padding(EdgeInsets(top: 20, leading: 22, bottom: 20, trailing: 22))
        .frame(width: 440, alignment: .leading)
        .background(Theme.dialogBg)
    }
}
