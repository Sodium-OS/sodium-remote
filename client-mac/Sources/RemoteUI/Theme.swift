// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import AppKit
import SwiftUI

/// Graphite Glass on the Mac: the Phase 2 kit's tokens (design/phase2/kit.py
/// DARK and LIGHT) plus the Remote kit's (design/remote/kitr.py), as dynamic
/// colours that follow the window's appearance.
///
/// The accent is the periwinkle preset (tokens.json `accent.default`), decided
///: the stream shows the host's wallpaper accent inside, the chrome
/// round it is ours.
public enum Theme {
    static func rgba(_ r: Int, _ g: Int, _ b: Int, _ a: Double) -> NSColor {
        NSColor(srgbRed: CGFloat(r) / 255, green: CGFloat(g) / 255, blue: CGFloat(b) / 255, alpha: CGFloat(a))
    }

    static func hex(_ v: UInt32, _ a: Double = 1) -> NSColor {
        rgba(Int((v >> 16) & 0xFF), Int((v >> 8) & 0xFF), Int(v & 0xFF), a)
    }

    /// A colour with a dark and a light value.
    static func dyn(_ dark: NSColor, _ light: NSColor) -> Color {
        Color(nsColor: dynNS(dark, light))
    }

    public static func dynNS(_ dark: NSColor, _ light: NSColor) -> NSColor {
        NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
        }
    }

    static func white(_ a: Double) -> NSColor { rgba(255, 255, 255, a) }
    static func ink(_ a: Double) -> NSColor { rgba(20, 24, 40, a) }

    // Text and icons.
    public static let text = dyn(hex(0xF2F3F6), hex(0x1A1D27))
    public static let text2 = dyn(hex(0xB8BDC9), hex(0x3D4251))
    public static let text3 = dyn(hex(0x9EA4B2), hex(0x575D6C))
    public static let menuText = dyn(hex(0xD5D9E2), hex(0x3D4251))
    public static let icon = dyn(hex(0xE4E7EE), hex(0x2B2F3C))
    public static let titleInactive = dyn(hex(0x9DA3B0), hex(0x575D6C))

    // Surfaces.
    public static let ground = dyn(hex(0x0E1018), hex(0xEEF0F5))
    public static let hair = dyn(white(0.07), ink(0.08))
    public static let hair2 = dyn(white(0.06), ink(0.07))
    public static let hairStrong = dyn(white(0.12), ink(0.12))
    public static let raised = dyn(white(0.07), ink(0.05))
    public static let raised2 = dyn(white(0.05), ink(0.04))
    public static let hover = dyn(white(0.06), ink(0.05))
    public static let hover2 = dyn(white(0.10), ink(0.07))
    public static let pressed = dyn(white(0.20), ink(0.14))
    public static let open = dyn(white(0.16), ink(0.10))
    public static let input = dyn(white(0.07), ink(0.05))
    public static let segBg = dyn(white(0.08), ink(0.06))
    public static let segSel = dyn(white(0.16), hex(0xFFFFFF))
    public static let segSelText = dyn(hex(0xFFFFFF), hex(0x1A1D27))
    public static let btn2 = dyn(white(0.10), ink(0.07))
    public static let swOff = dyn(white(0.18), ink(0.20))
    public static let meterTrack = dyn(white(0.14), ink(0.12))

    // Glass.
    public static let winGlassNS = dynNS(rgba(22, 24, 33, 0.84), rgba(250, 251, 253, 0.86))
    public static let winGlass = Color(nsColor: winGlassNS)
    public static let sheetGlass = dyn(rgba(26, 28, 38, 0.80), rgba(250, 251, 254, 0.86))
    public static let toastGlass = dyn(rgba(26, 28, 38, 0.84), rgba(250, 251, 254, 0.88))
    public static let hudGlass = dyn(rgba(14, 15, 22, 0.74), rgba(250, 251, 254, 0.84))
    public static let dialogBg = dyn(rgba(32, 34, 46, 0.97), rgba(252, 252, 254, 0.97))
    public static let glassBorder = dyn(white(0.10), ink(0.08))
    public static let scrim = dyn(rgba(8, 9, 16, 0.5), ink(0.22))
    public static let streamDim = dyn(rgba(8, 9, 16, 0.62), rgba(232, 236, 246, 0.66))
    /// The band around a letterboxed stream (the window's background).
    public static let letterboxNS = dynNS(hex(0x07080C), hex(0x1A1D27))
    public static let letterbox = Color(nsColor: letterboxNS)
    public static let shadow = dyn(rgba(0, 0, 0, 0.45), rgba(30, 36, 60, 0.22))

    // States.
    public static let success = dyn(hex(0x8EE0B6), hex(0x1E6D45))
    public static let successBg = dyn(rgba(142, 224, 182, 0.14), rgba(30, 109, 69, 0.12))
    public static let warning = dyn(hex(0xFFC79E), hex(0x8F5124))
    public static let warningBg = dyn(rgba(255, 199, 158, 0.14), rgba(143, 81, 36, 0.10))
    public static let danger = dyn(hex(0xF28F8F), hex(0xB32D2D))
    public static let dangerBg = dyn(rgba(242, 143, 143, 0.16), rgba(179, 45, 45, 0.10))
    public static let dangerFg = dyn(hex(0xF7A8A8), hex(0xB32D2D))

    // The accent (periwinkle): a fill with dark text, and its ink for text and rings.
    public static let accent = Color(nsColor: hex(0x9AA8FF))
    public static let accentInk = dyn(hex(0x9AA8FF), hex(0x344BDB))
    public static let accentSoft = Color(nsColor: hex(0x9AA8FF, 0x2E / 255.0))
    public static let accentSel = Color(nsColor: hex(0x9AA8FF, 0x38 / 255.0))
    public static let accentLine = Color(nsColor: hex(0x9AA8FF, 0x66 / 255.0))
    public static let onAccent = Color(nsColor: hex(0x121420))

    // The HUD's stage colours: the computer, the network, this Mac.
    public static let stageHost = accentInk
    public static let stageNet = dyn(hex(0xFFB27A), hex(0xC96A24))
    public static let stageMac = dyn(hex(0x7FD8C0), hex(0x216C58))

    // The Remote app tile (kitr REMOTE_TILE).
    public static let tileTop = Color(nsColor: hex(0x8E9BFF))
    public static let tileBottom = Color(nsColor: hex(0x4E5CD8))

    /// The 48 pt bar of our windows; macOS's unified toolbar may
    /// make it a little taller, and the bar follows it.
    public static let barHeight: CGFloat = 48
}
