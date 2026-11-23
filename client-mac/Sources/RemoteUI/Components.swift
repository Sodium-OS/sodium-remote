// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import RemoteKit
import SwiftUI

// MARK: - Materials

/// macOS's blur behind a view (NSVisualEffectView), for glass surfaces.
public struct BlurView: NSViewRepresentable {
    var material: NSVisualEffectView.Material
    var blending: NSVisualEffectView.BlendingMode

    public init(material: NSVisualEffectView.Material = .hudWindow, blending: NSVisualEffectView.BlendingMode = .withinWindow) {
        self.material = material
        self.blending = blending
    }

    public func makeNSView(context: Context) -> NSVisualEffectView {
        let v = NSVisualEffectView()
        v.material = material
        v.blendingMode = blending
        v.state = .active
        return v
    }

    public func updateNSView(_ v: NSVisualEffectView, context: Context) {
        v.material = material
        v.blendingMode = blending
    }
}

/// A glass surface: blur, the token's tint, a hairline border, a soft shadow.
struct GlassSurface: ViewModifier {
    var tint: Color
    var radius: CGFloat
    var shadow: Bool = true

    func body(content: Content) -> some View {
        content
            .background(
                ZStack {
                    BlurView(material: .hudWindow, blending: .withinWindow)
                    tint
                }
                .clipShape(RoundedRectangle(cornerRadius: radius, style: .continuous))
            )
            .overlay(RoundedRectangle(cornerRadius: radius, style: .continuous).strokeBorder(Theme.glassBorder, lineWidth: 1))
            .shadow(color: shadow ? Theme.shadow : .clear, radius: shadow ? 18 : 0, x: 0, y: shadow ? 10 : 0)
    }
}

extension View {
    func glassSurface(_ tint: Color, radius: CGFloat, shadow: Bool = true) -> some View {
        modifier(GlassSurface(tint: tint, radius: radius, shadow: shadow))
    }
}

// MARK: - Buttons (kits.btn: primary, secondary, ghost, danger, dangerSolid)

public enum ButtonKind {
    case primary, secondary, ghost, danger, dangerSolid
}

struct RemoteButtonStyle: ButtonStyle {
    var kind: ButtonKind
    var height: CGFloat

    func makeBody(configuration: Configuration) -> some View {
        let fg: Color
        let bg: Color
        switch kind {
        case .primary:
            fg = Theme.onAccent
            bg = Theme.accent
        case .secondary:
            fg = Theme.text
            bg = Theme.btn2
        case .ghost:
            fg = Theme.text
            bg = Color.clear
        case .danger:
            fg = Theme.dangerFg
            bg = Theme.dangerBg
        case .dangerSolid:
            fg = Color.white
            bg = Theme.danger
        }
        return configuration.label
            .font(.system(size: 13, weight: .semibold))
            .foregroundColor(fg)
            .padding(.horizontal, 14)
            .frame(height: height)
            .background(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(bg))
            .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).fill(Color.black.opacity(configuration.isPressed ? 0.12 : 0)))
            .contentShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
    }
}

/// A text button in one of the kit's kinds, with an optional SF Symbol.
public struct KitButton: View {
    let title: String
    let kind: ButtonKind
    let icon: String?
    let height: CGFloat
    let action: () -> Void

    public init(_ title: String, kind: ButtonKind = .secondary, icon: String? = nil, height: CGFloat = 30, action: @escaping () -> Void) {
        self.title = title
        self.kind = kind
        self.icon = icon
        self.height = height
        self.action = action
    }

    public var body: some View {
        SwiftUI.Button(action: action) {
            HStack(spacing: 6) {
                if let i = icon {
                    Image(systemName: i).font(.system(size: 13, weight: .medium))
                }
                Text(title).lineLimit(1)
            }
        }
        .buttonStyle(RemoteButtonStyle(kind: kind, height: height))
    }
}

/// A square icon button (the title bar's `wbtn`: 28 pt, radius 8).
public struct IconButton: View {
    let symbol: String
    let label: String
    let on: Bool
    let action: () -> Void
    @State private var hovering = false

    public init(_ symbol: String, label: String, on: Bool = false, action: @escaping () -> Void) {
        self.symbol = symbol
        self.label = label
        self.on = on
        self.action = action
    }

    public var body: some View {
        SwiftUI.Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 14, weight: .regular))
                .foregroundColor(Theme.icon)
                .frame(width: 28, height: 28)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(on ? Theme.open : (hovering ? Theme.hover2 : Color.clear)))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(label)
        .accessibilityLabel(label)
    }
}

// MARK: - Pills, dots, bars

public enum PillKind {
    case ok, neutral, warn, accent, danger
}

/// kits.pill: a small rounded label, optionally with an icon.
public struct StatusPill: View {
    let text: String
    let kind: PillKind
    let icon: String?

    public init(_ text: String, kind: PillKind, icon: String? = nil) {
        self.text = text
        self.kind = kind
        self.icon = icon
    }

    public var body: some View {
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
            Text(text).font(.system(size: 11.5, weight: .semibold)).lineLimit(1)
        }
        .foregroundColor(colors.1)
        .padding(.horizontal, 8)
        .frame(height: 20)
        .background(Capsule().fill(colors.0))
    }
}

/// The quality dot (green good; amber fair, poor or a TCP relay).
public struct QualityDot: View {
    let band: QualityBand
    let size: CGFloat

    public init(_ band: QualityBand, size: CGFloat = 6) {
        self.band = band
        self.size = size
    }

    public var body: some View {
        Circle().fill(QualityDot.color(band)).frame(width: size, height: size)
    }

    static func color(_ b: QualityBand) -> Color {
        switch b {
        case .good: return Theme.success
        case .fair, .poor: return Theme.warning
        case .none: return Theme.text3
        }
    }
}

/// Three rising bars, like Wi-Fi strength; coloured only when not good.
public struct QualityBars: View {
    let band: QualityBand

    public init(_ band: QualityBand) {
        self.band = band
    }

    public var body: some View {
        let c: Color = band == .good ? Theme.icon : (band == .none ? Theme.text3 : Theme.warning)
        return HStack(alignment: .bottom, spacing: 2) {
            ForEach(0..<3, id: \.self) { i in
                RoundedRectangle(cornerRadius: 1.5)
                    .fill(c.opacity(i < band.bars ? 1 : 0.28))
                    .frame(width: 3, height: CGFloat(5 + i * 3))
            }
        }
        .frame(height: 12, alignment: .bottom)
    }
}

/// The glyph for a route kind (kitr PATHS).
public struct RouteIcon: View {
    let kind: RouteKind
    let size: CGFloat

    public init(_ kind: RouteKind, size: CGFloat = 13) {
        self.kind = kind
        self.size = size
    }

    public var body: some View {
        Group {
            switch kind {
            case .direct:
                DirectGlyph().stroke(style: StrokeStyle(lineWidth: 1.333 * size / 16, lineCap: .round, lineJoin: .round))
            case .relayUDP, .relayTCP, .relayTLS:
                RelayGlyph().stroke(style: StrokeStyle(lineWidth: 1.333 * size / 16, lineCap: .round, lineJoin: .round))
            case .lan:
                Image(systemName: "cable.connector").font(.system(size: size * 0.85))
            case .vpn, .tailscale:
                Image(systemName: "lock.shield").font(.system(size: size * 0.85))
            }
        }
        .frame(width: size, height: size)
    }
}

/// sodiumshell-path-direct: two ends joined by a line (16 × 16 artboard).
struct DirectGlyph: Shape {
    func path(in r: CGRect) -> Path {
        let s = r.width / 16
        var p = Path()
        p.addEllipse(in: CGRect(x: (3 - 1.667) * s, y: (8 - 1.667) * s, width: 3.333 * s, height: 3.333 * s))
        p.addEllipse(in: CGRect(x: (13 - 1.667) * s, y: (8 - 1.667) * s, width: 3.333 * s, height: 3.333 * s))
        p.move(to: CGPoint(x: 4.667 * s, y: 8 * s))
        p.addLine(to: CGPoint(x: 11.333 * s, y: 8 * s))
        return p
    }
}

/// sodiumshell-path-relay: two ends and the relay between them.
struct RelayGlyph: Shape {
    func path(in r: CGRect) -> Path {
        let s = r.width / 16
        var p = Path()
        p.addEllipse(in: CGRect(x: (2.667 - 1.333) * s, y: (8 - 1.333) * s, width: 2.667 * s, height: 2.667 * s))
        p.addEllipse(in: CGRect(x: (13.333 - 1.333) * s, y: (8 - 1.333) * s, width: 2.667 * s, height: 2.667 * s))
        p.addRoundedRect(in: CGRect(x: 5.667 * s, y: 5.667 * s, width: 4.667 * s, height: 4.667 * s), cornerSize: CGSize(width: 1.333 * s, height: 1.333 * s))
        p.move(to: CGPoint(x: 4 * s, y: 8 * s))
        p.addLine(to: CGPoint(x: 5.667 * s, y: 8 * s))
        p.move(to: CGPoint(x: 10.333 * s, y: 8 * s))
        p.addLine(to: CGPoint(x: 12 * s, y: 8 * s))
        return p
    }
}

/// sodiumshell-remote: a display with an arrow leaving it (16 × 16 artboard).
struct RemoteGlyph: Shape {
    func path(in r: CGRect) -> Path {
        let s = r.width / 16
        var p = Path()
        p.addRoundedRect(in: CGRect(x: 1.667 * s, y: 2.333 * s, width: 12.667 * s, height: 8.667 * s), cornerSize: CGSize(width: 1.667 * s, height: 1.667 * s))
        p.move(to: CGPoint(x: 5.333 * s, y: 13.667 * s))
        p.addLine(to: CGPoint(x: 10.667 * s, y: 13.667 * s))
        p.move(to: CGPoint(x: 8 * s, y: 11 * s))
        p.addLine(to: CGPoint(x: 8 * s, y: 13.667 * s))
        p.move(to: CGPoint(x: 6.333 * s, y: 8.333 * s))
        p.addLine(to: CGPoint(x: 9.667 * s, y: 5 * s))
        p.move(to: CGPoint(x: 7 * s, y: 5 * s))
        p.addLine(to: CGPoint(x: 9.667 * s, y: 5 * s))
        p.addLine(to: CGPoint(x: 9.667 * s, y: 7.667 * s))
        return p
    }
}

/// The Remote app tile: our squircle, indigo, the remote glyph in white.
public struct AppTile: View {
    let size: CGFloat

    public init(size: CGFloat) {
        self.size = size
    }

    public var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
                .fill(LinearGradient(colors: [Theme.tileTop, Theme.tileBottom], startPoint: .topLeading, endPoint: .bottomTrailing))
            RemoteGlyph()
                .stroke(Color.white, style: StrokeStyle(lineWidth: 1.333 * size * 0.55 / 16, lineCap: .round, lineJoin: .round))
                .frame(width: size * 0.55, height: size * 0.55)
        }
        .frame(width: size, height: size)
        .shadow(color: Color.black.opacity(0.25), radius: size * 0.08, x: 0, y: size * 0.04)
    }
}

/// The path chip: quality dot, route glyph, "Direct · 9 ms"; amber when warned.
public struct PathChip: View {
    let chip: LinkView.Chip
    let open: Bool
    let height: CGFloat
    let action: () -> Void

    public init(_ chip: LinkView.Chip, open: Bool = false, height: CGFloat = 24, action: @escaping () -> Void) {
        self.chip = chip
        self.open = open
        self.height = height
        self.action = action
    }

    public var body: some View {
        let warn = chip.tone != .normal
        let red = chip.tone == .red
        let fg: Color = red ? Theme.dangerFg : (warn ? Theme.warning : Theme.text)
        let bg: Color = red ? Theme.dangerBg : (warn ? Theme.warningBg : (open ? Theme.open : Theme.raised))
        return SwiftUI.Button(action: action) {
            HStack(spacing: 6) {
                if red {
                    Circle().fill(Theme.danger).frame(width: 6, height: 6)
                } else {
                    QualityDot(warn ? .fair : QualityBand.of(rttMs: chip.rttMs, route: chip.route))
                }
                if let r = chip.route, !red {
                    RouteIcon(r, size: 14)
                }
                Text(chip.text).font(.system(size: 12, weight: .medium)).lineLimit(1)
            }
            .foregroundColor(fg)
            .padding(.leading, 8)
            .padding(.trailing, 10)
            .frame(height: height)
            .background(Capsule().fill(bg))
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Path: " + chip.text)
    }
}

// MARK: - Segmented control, switch, rows and groups (kits: segc, sw, row, group)

/// The kit's segmented control.
public struct Segmented<T: Hashable>: View {
    let options: [(T, String)]
    @Binding var selection: T
    var disabled: Bool = false

    public init(_ options: [(T, String)], selection: Binding<T>, disabled: Bool = false) {
        self.options = options
        self._selection = selection
        self.disabled = disabled
    }

    public var body: some View {
        HStack(spacing: 2) {
            ForEach(0..<options.count, id: \.self) { i in
                let on = options[i].0 == selection
                SwiftUI.Button {
                    selection = options[i].0
                } label: {
                    Text(options[i].1)
                        .font(.system(size: 12, weight: on ? .semibold : .regular))
                        .foregroundColor(on ? Theme.segSelText : Theme.menuText)
                        .lineLimit(1)
                        .padding(.horizontal, 10)
                        .frame(height: 24)
                        .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(on ? Theme.segSel : Color.clear))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(2)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Theme.segBg))
        .opacity(disabled ? 0.45 : 1)
        .disabled(disabled)
    }
}

/// A switch in the accent.
public struct KitSwitch: View {
    @Binding var isOn: Bool
    let label: String

    public init(_ label: String, isOn: Binding<Bool>) {
        self.label = label
        self._isOn = isOn
    }

    public var body: some View {
        Toggle(label, isOn: $isOn)
            .toggleStyle(.switch)
            .labelsHidden()
            .tint(Theme.accent)
            .controlSize(.small)
    }
}

/// A Settings-style row: title and subtitle on the left, a control on the right.
public struct KitRow<Trailing: View>: View {
    let title: String
    let sub: String?
    let trailing: Trailing

    public init(_ title: String, sub: String? = nil, @ViewBuilder trailing: () -> Trailing) {
        self.title = title
        self.sub = sub
        self.trailing = trailing()
    }

    public var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.system(size: 13)).foregroundColor(Theme.text)
                if let s = sub {
                    Text(s).font(.system(size: 12)).foregroundColor(Theme.text2).fixedSize(horizontal: false, vertical: true)
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

/// A titled group: a 12 / 600 uppercase header, then a raised panel of rows
/// with hairlines between them, then an optional footnote.
public struct KitGroup<Content: View>: View {
    let title: String
    let foot: String?
    let content: Content

    public init(_ title: String = "", foot: String? = nil, @ViewBuilder content: () -> Content) {
        self.title = title
        self.foot = foot
        self.content = content()
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if !title.isEmpty {
                SectionHeader(title).padding(.horizontal, 4)
            }
            VStack(spacing: 0) {
                content
            }
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Theme.raised2))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Theme.hair2, lineWidth: 1))
            if let f = foot {
                Text(f).font(.system(size: 12)).foregroundColor(Theme.text2).padding(.horizontal, 4).fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// The hairline between rows of a group.
public struct RowDivider: View {
    public init() {}
    public var body: some View {
        Rectangle().fill(Theme.hair2).frame(height: 1).padding(.horizontal, 14)
    }
}

/// An uppercase section header (12 / 600, text3).
public struct SectionHeader: View {
    let text: String

    public init(_ text: String) {
        self.text = text
    }

    public var body: some View {
        Text(text.uppercased())
            .font(.system(size: 12, weight: .semibold))
            .tracking(0.3)
            .foregroundColor(Theme.text3)
    }
}

/// A round 40 pt icon tile for cards (card_parts).
public struct CardIcon: View {
    let symbol: String
    let bg: Color
    let fg: Color

    public init(_ symbol: String, bg: Color = Theme.raised, fg: Color = Theme.icon) {
        self.symbol = symbol
        self.bg = bg
        self.fg = fg
    }

    public var body: some View {
        Image(systemName: symbol)
            .font(.system(size: 18, weight: .regular))
            .foregroundColor(fg)
            .frame(width: 40, height: 40)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(bg))
    }
}

/// A small spinner (process-working).
public struct Spinner: View {
    let size: CGFloat

    public init(size: CGFloat = 14) {
        self.size = size
    }

    public var body: some View {
        ProgressView()
            .progressViewStyle(.circular)
            .controlSize(.small)
            .scaleEffect(size / 16)
            .frame(width: size, height: size)
            .tint(Theme.accentInk)
    }
}

/// A keycap (kits.kbd).
public struct Keycap: View {
    let text: String

    public init(_ text: String) {
        self.text = text
    }

    public var body: some View {
        Text(text)
            .font(.system(size: 12, weight: .medium))
            .foregroundColor(Theme.menuText)
            .frame(minWidth: 22)
            .padding(.horizontal, 4)
            .frame(height: 22)
            .background(RoundedRectangle(cornerRadius: 5, style: .continuous).fill(Theme.btn2))
    }
}
