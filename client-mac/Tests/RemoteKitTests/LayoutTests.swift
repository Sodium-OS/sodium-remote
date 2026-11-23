// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import XCTest
@testable import RemoteKit

///: the scale's pixel period, the window's resize increments, the
/// letterbox and the pointer in the band (the cases of client-linux's test).
final class LayoutTests: XCTestCase {
    func testPixelPeriodsAndIncrements() {
        XCTAssertEqual(StreamLayout.pixelPeriod(scale: 1.6), 8)
        XCTAssertEqual(StreamLayout.pixelPeriod(scale: 2.0), 2)
        XCTAssertEqual(StreamLayout.pixelPeriod(scale: 4.0 / 3.0), 4)
        XCTAssertEqual(StreamLayout.pixelPeriod(scale: 1.5), 3)
        XCTAssertEqual(StreamLayout.pixelPeriod(scale: 256.0 / 120.0), 32)
        // The Air's 2× backing: 1 pt steps at 2.0, 4 pt at 1.6.
        XCTAssertEqual(StreamLayout.resizeIncrement(scale: 2.0, backingScale: 2), 1)
        XCTAssertEqual(StreamLayout.resizeIncrement(scale: 1.6, backingScale: 2), 4)
        // The host's rounding: 2236 × 1349 at 1.6 → 2232 × 1344.
        XCTAssertEqual(StreamLayout.roundDown(PixelSize(width: 2236, height: 1349), scale: 1.6), PixelSize(width: 2232, height: 1344))
        // A fractional client at the panel's pixels still lists 1.6 (2560 × 1664 → 1600 × 1040).
        XCTAssertTrue(StreamLayout.validScales(display: PixelSize(width: 2560, height: 1664)).contains(1.6))
    }

    func testTheScaleListKeepsOnlyPeriodsUpTo8() {
        // The 13" Air's backing pixels: 2.0 with 1.75 and 2.333 beside
        // it, not 1.867 (p = 28) or 2.1 (p = 21).
        let list = StreamLayout.validScales(display: PixelSize(width: 2940, height: 1848))
        XCTAssertEqual(list.map { StreamLayout.scaleNumerator($0) }, [140, 144, 160, 168, 180, 210, 240, 280, 360])
        XCTAssertTrue(list.allSatisfy { StreamLayout.pixelPeriod(scale: $0) <= 8 })
    }

    func testOneToOneWithAnOddBandAndThePointerInIt() {
        let stream = PixelSize(width: 2232, height: 1344)
        // 2237 × 1349 window: 5 px each way, 2 left/top and 3 right/bottom.
        let p = StreamLayout.place(window: PixelSize(width: 2237, height: 1349), stream: stream)
        XCTAssertEqual(p, Placement(x: 2, y: 2, w: 2232, h: 1344))
        var s = StreamLayout.toStream(p, stream: stream, x: 102.5, y: 52)
        XCTAssertEqual(s.x, 100.5)
        XCTAssertEqual(s.y, 50)
        // In the band: clamped to the nearest edge.
        s = StreamLayout.toStream(p, stream: stream, x: 0, y: 1)
        XCTAssertEqual(s.x, 0)
        XCTAssertEqual(s.y, 0)
        s = StreamLayout.toStream(p, stream: stream, x: 2236, y: 700)
        XCTAssertEqual(s.x, 2231)
        XCTAssertEqual(s.y, 698)
        // The band limit: 64 px is still 1:1, 65 is fitted.
        XCTAssertEqual(StreamLayout.place(window: PixelSize(width: 2296, height: 1408), stream: stream).w, 2232)
        XCTAssertGreaterThan(StreamLayout.place(window: PixelSize(width: 2297, height: 1409), stream: stream).w, 2232)
        // Fixed resolution in a bigger window: aspect fit.
        XCTAssertEqual(StreamLayout.place(window: PixelSize(width: 2560, height: 1664), stream: PixelSize(width: 1280, height: 832)),
                       Placement(x: 0, y: 0, w: 2560, h: 1664))
    }
}
