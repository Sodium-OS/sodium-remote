// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

/// First run: the main window's first content, centred and
/// 440 pt wide. Two permissions, each watched and updated in place; the
/// microphone is asked when it is first switched on, not here.
@MainActor
struct WelcomeView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        GeometryReader { geo in
            ScrollView(.vertical) {
                WelcomeColumn(model: model)
                    .frame(width: 440)
                    .frame(maxWidth: .infinity, minHeight: geo.size.height)
            }
        }
    }
}

@MainActor
private struct WelcomeColumn: View {
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            WelcomeHero()
            KitGroup {
                PermissionRow(symbol: "keyboard", title: "Accessibility",
                              sub: "So ⌘Tab, ⌘Space and other system shortcuts reach the computer while you stream",
                              state: model.accessibility,
                              allow: { model.actions?.requestAccessibility() },
                              openSettings: { model.actions?.openSystemSettings(.accessibility) })
                RowDivider()
                PermissionRow(symbol: "network", title: "Local network",
                              sub: "Finds your computer at home with no server in between: the fastest path",
                              state: model.localNetwork,
                              allow: { model.actions?.requestLocalNetwork() },
                              openSettings: { model.actions?.openSystemSettings(.localNetwork) })
            }
            if model.localNetwork == .denied {
                InfoBanner(text: "Without the local network, Sodium Remote can’t find computers at home by itself.")
                    .padding(.top, 10)
            }
            HStack {
                KitButton("Not now", kind: .ghost, height: 34) {
                    model.actions?.finishWelcome()
                }
                Spacer()
                KitButton("Continue", kind: .primary, height: 34) {
                    model.actions?.finishWelcome()
                }
                .keyboardShortcut(.defaultAction)
            }
            .padding(.top, 16)
        }
        .padding(EdgeInsets(top: 18, leading: 20, bottom: 18, trailing: 20))
    }
}

@MainActor
private struct WelcomeHero: View {
    var body: some View {
        VStack(spacing: 10) {
            AppTile(size: 56)
            Text("Welcome to Remote")
                .font(.system(size: 20, weight: .semibold))
                .tracking(-0.2)
                .foregroundColor(Theme.text)
            Text("Your SodiumOS computer on this Mac. Two permissions make it feel local; you can change them later in Settings.")
                .font(.system(size: 13))
                .foregroundColor(Theme.text2)
                .multilineTextAlignment(.center)
                .lineSpacing(3)
                .frame(maxWidth: 330)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 6)
        .padding(.bottom, 18)
    }
}

/// One permission: what it is for, then Allow…, waiting, Allowed or Open Settings….
@MainActor
private struct PermissionRow: View {
    let symbol: String
    let title: LocalizedStringKey
    let sub: LocalizedStringKey
    let state: PermissionState
    let allow: () -> Void
    let openSettings: () -> Void

    var body: some View {
        let allowed: Bool = state == .allowed
        return HStack(alignment: .center, spacing: 12) {
            Image(systemName: symbol)
                .font(.system(size: 15, weight: .regular))
                .foregroundColor(allowed ? Theme.accentInk : Theme.icon)
                .frame(width: 32, height: 32)
                .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(allowed ? Theme.accentSoft : Theme.raised))
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.system(size: 13))
                    .foregroundColor(Theme.text)
                Text(sub)
                    .font(.system(size: 12))
                    .foregroundColor(Theme.text2)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            trailing
        }
        .padding(EdgeInsets(top: 12, leading: 14, bottom: 12, trailing: 14))
        .frame(minHeight: 58)
    }

    @ViewBuilder private var trailing: some View {
        switch state {
        case .unknown:
            KitButton("Allow…", kind: .secondary, height: 30, action: allow)
        case .waiting:
            HStack(spacing: 6) {
                Spinner(size: 14)
                Text("In System Settings")
                    .font(.system(size: 12))
                    .foregroundColor(Theme.text2)
                    .lineLimit(1)
            }
            .fixedSize()
        case .allowed:
            StatusPill("Allowed", kind: .ok, icon: "checkmark")
        case .denied:
            KitButton("Open Settings…", kind: .secondary, height: 30, action: openSettings)
        }
    }
}

/// kits.banner, the info tone: says in place what a missing permission costs.
@MainActor
private struct InfoBanner: View {
    let text: LocalizedStringKey

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "info.circle")
                .font(.system(size: 14))
                .foregroundColor(Theme.icon)
            Text(text)
                .font(.system(size: 12.5))
                .foregroundColor(Theme.text)
                .lineSpacing(2)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(EdgeInsets(top: 9, leading: 12, bottom: 9, trailing: 12))
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Theme.raised))
    }
}
