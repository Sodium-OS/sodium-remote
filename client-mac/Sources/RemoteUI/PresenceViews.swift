// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

// The menu-bar extra's icon and the "being controlled" banner.

/// The menu-bar icon in its three states: off is dim, on is solid, controlled is the accent with a lit dot
/// (a shape, so it isn't colour alone). Off and on are template images, so macOS tints them for the bar.
public enum PresenceIcon {
    @MainActor public static func image(_ state: PresenceState) -> NSImage? {
        let size: CGFloat = 16
        let stroke = StrokeStyle(lineWidth: 1.333 * size / 16 * 1.15, lineCap: .round, lineJoin: .round)
        switch state {
        case .off:
            return render(RemoteGlyph().stroke(Color.black.opacity(0.5), style: stroke).frame(width: size, height: size), template: true)
        case .on:
            return render(RemoteGlyph().stroke(Color.black, style: stroke).frame(width: size, height: size), template: true)
        case .controlled:
            let v = ZStack(alignment: .topTrailing) {
                RemoteGlyph().stroke(Theme.accent, style: stroke).frame(width: size, height: size)
                Circle()
                    .fill(Theme.accent)
                    .frame(width: 6, height: 6)
                    .overlay(Circle().strokeBorder(Color.black.opacity(0.55), lineWidth: 1))
                    .offset(x: 2, y: -2)
            }
            .frame(width: size + 4, height: size + 4)
            return render(v, template: false)
        }
    }

    @MainActor private static func render<V: View>(_ v: V, template: Bool) -> NSImage? {
        let r = ImageRenderer(content: v)
        r.scale = 2
        let img = r.nsImage
        img?.isTemplate = template
        return img
    }
}

/// The board's own panel for "being controlled", used when notifications are denied (mac_banner).
public enum PresenceUI {
    @MainActor public static func makeBanner(title: String, detail: String, onDisconnect: @escaping () -> Void,
                                             onDismiss: @escaping () -> Void) -> NSView {
        let v = NSHostingView(rootView: ControlledBannerView(title: title, detail: detail, onDisconnect: onDisconnect, onDismiss: onDismiss))
        v.setFrameSize(v.fittingSize)
        return v
    }
}

@MainActor
struct ControlledBannerView: View {
    let title: String
    let detail: String
    let onDisconnect: () -> Void
    let onDismiss: () -> Void

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            AppTile(size: 36)
            VStack(alignment: .leading, spacing: 2) {
                Text(verbatim: title).font(.system(size: 13, weight: .semibold)).foregroundColor(Theme.text).fixedSize(horizontal: false, vertical: true)
                Text(verbatim: detail).font(.system(size: 12)).foregroundColor(Theme.text2).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)
            VStack(spacing: 6) {
                LButton(title: "Disconnect", kind: .danger, height: 28, action: onDisconnect)
                LButton(title: "Dismiss", kind: .secondary, height: 28, action: onDismiss)
            }
        }
        .padding(12)
        .frame(width: 440)
        .glassSurface(Theme.toastGlass, radius: 16)
    }
}
