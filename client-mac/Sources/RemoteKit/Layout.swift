// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation

/// A size in pixels.
public struct PixelSize: Equatable, Hashable, Codable, CustomStringConvertible {
    public var width: Int
    public var height: Int

    public init(width: Int, height: Int) {
        self.width = width
        self.height = height
    }

    public static let zero = PixelSize(width: 0, height: 0)
    public var isEmpty: Bool { width <= 0 || height <= 0 }
    public var description: String { "\(width) × \(height)" }
}

/// Where the stream goes in the view's backing pixels.
public struct Placement: Equatable {
    public var x: Double
    public var y: Double
    public var w: Double
    public var h: Double

    public init(x: Double, y: Double, w: Double, h: Double) {
        self.x = x
        self.y = y
        self.w = w
        self.h = h
    }
}

/// Where the stream goes in the window, and where the pointer goes in the
/// stream.
///
/// The host rounds REMOTE-1 down to a multiple of its scale's pixel period, so
/// the stream can be up to p−1 px smaller than the window on each axis. It is
/// then drawn 1:1, centred in whole pixels with the odd pixel to the right and
/// bottom; the band is the window's background. Anything else (Fixed
/// resolution, a size still in flight) is fitted with its aspect kept.
public enum StreamLayout {
    /// Within this many pixels per axis the stream is drawn 1:1 (the largest
    /// pixel period the host lists, 8, is well inside it).
    public static let oneToOneBand = 64

    /// The largest pixel period a listed scale may have. The host
    /// keeps the same limit, so a resize step is at most 8 px and a band at
    /// most 7 px.
    public static let maxPixelPeriod = 8

    public static func place(window: PixelSize, stream: PixelSize) -> Placement {
        let bw = window.width, bh = window.height
        let sw = stream.width, sh = stream.height
        if sw <= 0 || sh <= 0 {
            return Placement(x: 0, y: 0, w: Double(bw), h: Double(bh))
        }
        if sw <= bw && sh <= bh && bw - sw <= oneToOneBand && bh - sh <= oneToOneBand {
            // Whole pixels; the odd one goes right and bottom.
            return Placement(x: Double((bw - sw) / 2), y: Double((bh - sh) / 2), w: Double(sw), h: Double(sh))
        }
        let k = min(Double(bw) / Double(sw), Double(bh) / Double(sh))
        let w = Double(sw) * k
        let h = Double(sh) * k
        return Placement(x: ((Double(bw) - w) / 2).rounded(.down), y: ((Double(bh) - h) / 2).rounded(.down), w: w, h: h)
    }

    /// A point in backing pixels (top-left origin) as stream pixels; a point
    /// in the band clamps to the nearest stream edge.
    public static func toStream(_ p: Placement, stream: PixelSize, x: Double, y: Double) -> (x: Double, y: Double) {
        let sw = Double(stream.width), sh = Double(stream.height)
        guard p.w > 0, p.h > 0 else { return (0, 0) }
        let sx = ((x - p.x) * (sw / p.w)).clamped(0, max(sw - 1, 0))
        let sy = ((y - p.y) * (sh / p.h)).clamped(0, max(sh - 1, 0))
        return (sx, sy)
    }

    /// The inverse, for the caret: stream pixels → backing pixels.
    public static func fromStream(_ p: Placement, stream: PixelSize, x: Double, y: Double) -> (x: Double, y: Double) {
        guard stream.width > 0, stream.height > 0 else { return (x, y) }
        return (p.x + x * p.w / Double(stream.width), p.y + y * p.h / Double(stream.height))
    }

    /// The scale as k/120 (Hyprland's valid scales).
    public static func scaleNumerator(_ scale: Double) -> Int {
        max(1, Int((scale * 120).rounded()))
    }

    /// The scale's pixel period p = k / gcd(k, 120): 8 at 1.6, 2 at 2.0, 4 at
    /// 1.333, 3 at 1.5, 32 at 2.133.
    public static func pixelPeriod(scale: Double) -> Int {
        let k = scaleNumerator(scale)
        return k / gcd(k, 120)
    }

    /// What `NSWindow.contentResizeIncrements` should be, in points: p over the
    /// backing scale (1 pt at 2.0 and 4 pt at 1.6 on a 2× display).
    public static func resizeIncrement(scale: Double, backingScale: Double) -> Double {
        Double(pixelPeriod(scale: scale)) / max(backingScale, 1)
    }

    /// REMOTE-1's size for a window: the largest whole multiple of p at or
    /// below it on each axis (what the host does; for the HUD and tests).
    public static func roundDown(_ window: PixelSize, scale: Double) -> PixelSize {
        let p = pixelPeriod(scale: scale)
        return PixelSize(width: window.width / p * p, height: window.height / p * p)
    }

    /// Valid scales for a display size: k/120 with both sides whole logical
    /// pixels, between 1 and 3, whose pixel period is at most 8 (the host's
    /// list; used before `Welcome`).
    public static func validScales(display: PixelSize) -> [Double] {
        guard !display.isEmpty else { return [] }
        let w: Int = display.width * 120
        let h: Int = display.height * 120
        var scales: [Double] = []
        for k in 121...360 {
            let whole: Bool = w % k == 0 && h % k == 0
            let period: Int = k / gcd(k, 120)
            if whole && period <= maxPixelPeriod {
                scales.append(Double(k) / 120)
            }
        }
        return scales
    }

    static func gcd(_ a: Int, _ b: Int) -> Int {
        var a = abs(a), b = abs(b)
        while b != 0 { (a, b) = (b, a % b) }
        return a
    }
}

extension Double {
    func clamped(_ lo: Double, _ hi: Double) -> Double {
        Swift.min(Swift.max(self, lo), hi)
    }
}
