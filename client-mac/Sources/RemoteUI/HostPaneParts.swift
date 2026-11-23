// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

// Small pieces for the new screens. They take `LocalizedStringKey` (or a `Text`), so a literal at the call
// site is looked up in Localizable.xcstrings (docs/translating.md: Remote for Mac). The older components in
// Components.swift take `String` and stay as they are.

/// A text button in one of the kit's kinds; the title is localised.
struct LButton: View {
    let title: LocalizedStringKey
    var kind: ButtonKind = .secondary
    var icon: String?
    var height: CGFloat = 30
    let action: () -> Void

    var body: some View {
        SwiftUI.Button(action: action) {
            HStack(spacing: 6) {
                if let i = icon {
                    Image(systemName: i).font(.system(size: 13, weight: .medium))
                }
                Text(title).fixedSize()
            }
        }
        .buttonStyle(RemoteButtonStyle(kind: kind, height: height))
    }
}

/// kits.pill with a localised label.
struct LPill: View {
    let text: LocalizedStringKey
    let kind: PillKind
    var icon: String?

    var body: some View {
        let colors: (Color, Color)
        switch kind {
        case .ok: colors = (Theme.successBg, Theme.success)
        case .neutral: colors = (Theme.raised, Theme.text2)
        case .warn: colors = (Theme.warningBg, Theme.warning)
        case .accent: colors = (Theme.accentSoft, Theme.accentInk)
        case .danger: colors = (Theme.dangerBg, Theme.dangerFg)
        }
        return HStack(spacing: 4) {
            if let i = icon { Image(systemName: i).font(.system(size: 10, weight: .semibold)) }
            Text(text).font(.system(size: 11.5, weight: .semibold)).fixedSize()
        }
        .foregroundColor(colors.1)
        .padding(.horizontal, 8)
        .frame(minHeight: 20)
        .background(Capsule().fill(colors.0))
    }
}

/// kits.group: an optional uppercase header, a raised panel of rows, an optional footnote.
struct LGroup<Content: View>: View {
    var title: LocalizedStringKey?
    var foot: Text?
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let t = title {
                Text(t)
                    .textCase(.uppercase)
                    .font(.system(size: 12, weight: .semibold))
                    .tracking(0.3)
                    .foregroundColor(Theme.text3)
                    .padding(.horizontal, 4)
            }
            VStack(spacing: 0) {
                content
            }
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.raised2))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.hair2, lineWidth: 1))
            if let f = foot {
                f.font(.system(size: 12)).foregroundColor(Theme.text2).padding(.horizontal, 4).fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// kits.row: a lead tile, a title and a sub line (both `Text`), a control on the right. Nothing elides:
/// long text wraps and the row grows.
struct LRow<Lead: View, Trailing: View>: View {
    let title: Text
    var sub: Text?
    let lead: Lead
    let trailing: Trailing

    init(_ title: Text, sub: Text? = nil, @ViewBuilder lead: () -> Lead, @ViewBuilder trailing: () -> Trailing) {
        self.title = title
        self.sub = sub
        self.lead = lead()
        self.trailing = trailing()
    }

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            lead
            VStack(alignment: .leading, spacing: 2) {
                title.font(.system(size: 13)).foregroundColor(Theme.text).fixedSize(horizontal: false, vertical: true)
                if let s = sub {
                    s.font(.system(size: 12)).foregroundColor(Theme.text2).fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 8)
            trailing
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .frame(minHeight: 46)
    }
}

extension LRow where Lead == EmptyView {
    init(_ title: Text, sub: Text? = nil, @ViewBuilder trailing: () -> Trailing) {
        self.init(title, sub: sub, lead: { EmptyView() }, trailing: trailing)
    }
}

/// A 32 pt tile with a symbol (kits.lead_icon).
struct LeadTile: View {
    let symbol: String
    var bg: Color = Theme.raised
    var fg: Color = Theme.icon
    var size: CGFloat = 32

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: size * 0.46, weight: .regular))
            .foregroundColor(fg)
            .frame(width: size, height: size)
            .background(RoundedRectangle(cornerRadius: size * 0.3, style: .continuous).fill(bg))
    }
}

/// A switch in the accent, with a localised accessibility label.
struct LSwitch: View {
    let label: LocalizedStringKey
    @Binding var isOn: Bool

    var body: some View {
        Toggle(isOn: $isOn) { Text(label) }
            .toggleStyle(.switch)
            .labelsHidden()
            .tint(Theme.accent)
            .controlSize(.small)
    }
}

/// The two-page control in the bar ("Computers", "This computer"); sizes to its labels.
struct PageSegmented: View {
    @Binding var page: MainPage

    var body: some View {
        HStack(spacing: 2) {
            segment("Computers", .computers)
            segment("This computer", .thisComputer)
        }
        .padding(2)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.segBg))
    }

    private func segment(_ title: LocalizedStringKey, _ p: MainPage) -> some View {
        let on = page == p
        return SwiftUI.Button {
            page = p
        } label: {
            Text(title)
                .font(.system(size: 12, weight: on ? .semibold : .regular))
                .foregroundColor(on ? Theme.segSelText : Theme.menuText)
                .fixedSize()
                .padding(.horizontal, 10)
                .frame(height: 24)
                .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(on ? Theme.segSel : Color.clear))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// kits.banner: a tinted strip with a message and an optional button.
struct LBanner<Action: View>: View {
    let kind: PillKind
    let symbol: String
    let message: Text
    @ViewBuilder var action: Action

    var body: some View {
        let colors: (Color, Color) = kind == .warn ? (Theme.warningBg, Theme.warning) : (kind == .danger ? (Theme.dangerBg, Theme.dangerFg) : (Theme.accentSoft, Theme.accentInk))
        return HStack(alignment: .center, spacing: 10) {
            Image(systemName: symbol).font(.system(size: 14)).foregroundColor(colors.1)
            message.font(.system(size: 12.5)).foregroundColor(Theme.text).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 8)
            action
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(colors.0))
    }
}
