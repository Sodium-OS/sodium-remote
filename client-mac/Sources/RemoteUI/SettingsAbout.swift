// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

// Settings › About, Server (the state, Add, Change, Remove, Import and Export; sheets in ServerSheets.swift) and the Computers group of Advanced
// (Show previews). Every string here goes through the
// catalog (`Text("…")`, `LocalizedStringKey` or `String(localized:)`; tools/lint-i18n.py checks); the product's name and the license's name are not translated.

// MARK: - About

@MainActor
struct AboutTab: View {
    @ObservedObject var model: AppModel
    @State private var showNotices = false

    private var version: String {
        Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0.1.0"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 14) {
                Image(nsImage: NSApp.applicationIconImage)
                    .resizable()
                    .frame(width: 48, height: 48)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 8) {
                        Text(verbatim: Brand.productName).font(.system(size: 15, weight: .semibold)).foregroundColor(Theme.text)
                        LPill(text: "Preview", kind: .accent)
                    }
                    Text("Version \(version)").font(.system(size: 12)).foregroundColor(Theme.text2)
                }
                Spacer(minLength: 0)
            }
            .padding(14)
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.raised2))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.hair2, lineWidth: 1))

            LGroup {
                LRow(Text("License")) {
                    // "AGPL-3.0" is the license's name: kept as is inside the translated sentence.
                    Text("AGPL-3.0 or commercial license").font(.system(size: 12.5)).foregroundColor(Theme.text2)
                }
                RowDivider()
                LRow(Text("Source code"), sub: Text(verbatim: Brand.sourceURL.replacingOccurrences(of: "https://", with: ""))) {
                    LButton(title: "Open", kind: .secondary, icon: "arrow.up.forward.square") { model.settingsActions?.openSourceCode() }
                }
                RowDivider()
                SwiftUI.Button {
                    showNotices = true
                } label: {
                    LRow(Text("Third-party notices"), sub: Text("The licenses of the software Sodium Remote includes")) {
                        Image(systemName: "chevron.right").font(.system(size: 11, weight: .semibold)).foregroundColor(Theme.text3)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }

            Text("Sodium Remote doesn’t check for updates and doesn’t send usage data.")
                .font(.system(size: 12))
                .foregroundColor(Theme.text2)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 4)
        }
        .sheet(isPresented: $showNotices) {
            NoticesSheet(model: model) { showNotices = false }
        }
    }
}

/// The short list; the full text of every license is in the notices file in the app (cargo-about's output and the
/// acknowledgements, put there by build-app.sh).
@MainActor
private struct NoticesSheet: View {
    @ObservedObject var model: AppModel
    let done: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 3) {
                Text("Third-party notices").font(.system(size: 16, weight: .semibold)).foregroundColor(Theme.text)
                Text("Sodium Remote is built on this software.").font(.system(size: 12)).foregroundColor(Theme.text2)
            }
            LGroup {
                NoticeRow(name: Text(verbatim: "Opus"), license: Text(verbatim: "BSD-3-Clause"))
                RowDivider()
                NoticeRow(name: Text("Rust libraries"), license: Text("MIT, Apache-2.0, others"))
            }
            Text("The full text of every license is in the notices file.")
                .font(.system(size: 12))
                .foregroundColor(Theme.text2)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 4)
            HStack {
                LButton(title: "Open the notices file", kind: .secondary, icon: "doc.text") { model.settingsActions?.openNoticesFile() }
                Spacer(minLength: 8)
                LButton(title: "Done", kind: .primary, action: done)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 440)
        .background(Theme.dialogBg)
    }
}

@MainActor
private struct NoticeRow: View {
    let name: Text
    let license: Text

    var body: some View {
        HStack(spacing: 12) {
            name.font(.system(size: 13)).foregroundColor(Theme.text)
            Spacer(minLength: 8)
            license.font(.system(size: 12.5)).foregroundColor(Theme.text2)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .frame(minHeight: 40)
    }
}

// MARK: - Server

@MainActor
struct ServerTab: View {
    @ObservedObject var model: AppModel
    @State private var request: ServerSheetRequest?

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if let server = model.server {
                configured(server)
            } else {
                none
            }
        }
        .sheet(item: $request) { r in
            switch r.kind {
            case .remove:
                RemoveServerSheet(name: model.serverName ?? "", remove: {
                    request = nil
                    model.serverActions?.removeServer()
                }, cancel: { request = nil })
            default:
                AddServerSheet(model: model, kind: r.kind) { request = nil }
            }
        }
        .onAppear { takePending() }
        .onChange(of: model.pendingServer) { _, _ in takePending() }
    }

    /// A link that was opened (or a file that was imported) is confirmed here, before anything is saved.
    private func takePending() {
        guard let c = model.pendingServer else { return }
        model.pendingServer = nil
        request = ServerSheetRequest(kind: .confirm(c))
    }

    // MARK: No server

    private var none: some View {
        VStack(alignment: .leading, spacing: 14) {
            LGroup(foot: Text("Sodium Remote doesn’t contact any server until you add one. A server helps computers on different networks find each other, and relays the picture when they can’t connect directly.")) {
                LRow(Text("No server"), sub: Text("Computers on this network and by address still work.")) {
                    EmptyView()
                }
            }
            HStack(spacing: 10) {
                LButton(title: "Add a server…", kind: .primary, icon: "plus", height: 32) { request = ServerSheetRequest(kind: .add) }
                SettingsLink("How to run your own server") { model.serverActions?.openServerGuide() }
                Spacer(minLength: 8)
                LButton(title: "Import…", kind: .secondary, height: 32) { model.serverActions?.importServerFile() }
            }
        }
    }

    // MARK: A server

    private func configured(_ server: ServerCandidate) -> some View {
        let name = server.label
        let problem = ServerProblem.parse(offlineError)
        return VStack(alignment: .leading, spacing: 16) {
            LGroup {
                LRow(Text(verbatim: name), sub: status(name), lead: {
                    LeadTile(symbol: "server.rack", bg: Theme.accentSoft, fg: Theme.accentInk, size: 40)
                }, trailing: {
                    HStack(spacing: 8) {
                        LPill(text: pillText, kind: pillKind)
                        LButton(title: "Change…", kind: .secondary) { request = ServerSheetRequest(kind: .change) }
                        LButton(title: "Remove", kind: .danger) { request = ServerSheetRequest(kind: .remove) }
                    }
                })
            }
            if let p = problem {
                problemCard(p, name: name)
            } else if case .offline = model.rendezvous {
                LBanner(kind: .warn, symbol: "exclamationmark.triangle",
                        message: Text("Couldn’t reach \(name). Computers on this network and by address still work. Sodium Remote keeps trying.")) {
                    LButton(title: "Try now", kind: .secondary, height: 28) { model.serverActions?.retryServer() }
                }
            }
            if model.serverWaitsForSession {
                Text("The new server is used when this session ends.")
                    .font(.system(size: 12)).foregroundColor(Theme.text2).padding(.horizontal, 4)
            }
            LGroup(title: "Server") {
                if !server.official {
                    LRow(Text("Address"), sub: nil) {
                        Text(verbatim: server.url).font(.system(size: 12, design: .monospaced)).foregroundColor(Theme.text)
                            .fixedSize(horizontal: false, vertical: true).multilineTextAlignment(.trailing)
                    }
                    RowDivider()
                }
                LRow(Text("Certificate"), sub: nil) {
                    Text(certificateValue(server)).font(.system(size: 12.5)).foregroundColor(Theme.text2)
                }
                if case .online = model.rendezvous {
                    RowDivider()
                    LRow(Text("This computer"), sub: nil) { LPill(text: "Enrolled", kind: .ok, icon: "checkmark") }
                }
            }
            HStack(spacing: 10) {
                LButton(title: "Import…", kind: .secondary) { model.serverActions?.importServerFile() }
                LButton(title: "Export…", kind: .secondary) { model.serverActions?.exportServerFile() }
                Spacer(minLength: 8)
                SettingsLink("How to run your own server") { model.serverActions?.openServerGuide() }
            }
            if !server.token.isEmpty {
                Text("The exported file includes the enrollment token.")
                    .font(.system(size: 12)).foregroundColor(Theme.text2).padding(.horizontal, 4)
            }
        }
    }

    /// the apps' own words for what the server said; its text is never shown for these codes.
    @ViewBuilder
    private func problemCard(_ p: ServerProblem, name: String) -> some View {
        switch p {
        case .tokenNeeded:
            LBanner(kind: .warn, symbol: "key", message: Text("This server needs an enrollment token. Enter the one you were given.")) { changeButton }
        case .tokenInvalid:
            LBanner(kind: .warn, symbol: "key", message: Text("That enrollment token isn’t valid. Check it with the person who runs the server.")) { changeButton }
        case .tokenExpired:
            LBanner(kind: .warn, symbol: "key", message: Text("That enrollment token has expired. Ask the person who runs the server for a new one.")) { changeButton }
        case .quotaExceeded:
            LBanner(kind: .warn, symbol: "exclamationmark.triangle",
                    message: Text("\(name) has reached its limit for now. Computers on this network and by address still work.")) { EmptyView() }
        case .paymentRequired:
            LBanner(kind: .warn, symbol: "exclamationmark.triangle",
                    message: Text("\(name) needs a payment before this computer can use it. Computers on this network and by address still work.")) { EmptyView() }
        }
    }

    private func certificateValue(_ server: ServerCandidate) -> LocalizedStringKey {
        server.pinsCA ? "Pinned" : "Public"
    }

    private var changeButton: some View {
        LButton(title: "Change…", kind: .secondary, height: 28) { request = ServerSheetRequest(kind: .change) }
    }

    private var offlineError: String? {
        if case .offline(let e) = model.rendezvous { return e }
        return nil
    }

    private func status(_ name: String) -> Text {
        switch model.rendezvous {
        case .online: return Text("\(name) reachable")
        case .offline: return Text("\(name) unreachable")
        case .connecting: return Text("Checking…")
        case .off: return Text(verbatim: "")
        }
    }

    private var pillText: LocalizedStringKey {
        switch model.rendezvous {
        case .online: return "Reachable"
        case .offline: return "Unreachable"
        default: return "Checking…"
        }
    }

    private var pillKind: PillKind {
        switch model.rendezvous {
        case .online: return .ok
        case .offline: return .warn
        default: return .neutral
        }
    }
}

// MARK: - Advanced › Computers

@MainActor
struct ComputersGroup: View {
    @ObservedObject var model: AppModel

    var body: some View {
        LGroup(title: "Computers") {
            LRow(Text("Show previews"),
                 sub: Text("Shows a picture of each computer in the list. The pictures stay on this computer, and turning this off deletes them.")) {
                KitSwitch("Show previews", isOn: $model.showPreviews)
            }
        }
    }
}
