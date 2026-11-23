// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import XCTest
@testable import RemoteKit

///: the one Reconnecting timeline's thresholds.
final class TimelineTests: XCTestCase {
    func testTheThresholds() {
        let v = { (since: Double, network: Bool) in
            ReconnectTimeline.view(since: since, route: .lan, rttMs: 2, amber: false, network: network)
        }
        XCTAssertEqual(v(0.999, true).chip?.tone, .normal)
        XCTAssertNil(v(0.999, true).pill)
        XCTAssertEqual(v(1.0, true).pill, .reconnecting)
        XCTAssertEqual(v(1.0, true).chip?.tone, .amber)
        XCTAssertEqual(v(1.0, true).chip?.text, "LAN")
        XCTAssertEqual(v(1.0, false).pill, .waitingForNetwork)
        XCTAssertEqual(v(29.9, true).pill, .reconnecting)
        let lost = v(30, true)
        XCTAssertEqual(lost.chip?.lost, true)
        XCTAssertEqual(lost.chip?.tone, .red)
        XCTAssertEqual(lost.chip?.text, "Lost")
        XCTAssertEqual(lost.pill, .lost)
    }

    func testLiveHedgingThenLostThenEnded() {
        var t = ReconnectTimeline()
        // Hedging: the hedge route, amber at once, no pill.
        let hedge = LinkSample(route: .direct, routeRttMs: 9, hedgeRoute: .relayUDP, hedging: true, healthy: false, silence: 0.3)
        let a = t.now(live: hedge, now: 100, network: true)
        XCTAssertEqual(a?.chip?.route, .relayUDP)
        XCTAssertEqual(a?.chip?.tone, .amber)
        XCTAssertNil(a?.pill)
        // The core declares the loss 4 s after the last packet; the chip names the last route.
        t.reconnecting(sinceSeconds: 4, now: 104)
        let b = t.now(live: nil, now: 104, network: true)
        XCTAssertEqual(b?.chip?.route, .direct)
        XCTAssertEqual(b?.pill, .reconnecting)
        XCTAssertEqual(t.now(live: nil, now: 130, network: true)?.pill, .lost)
        // A TCP relay is always amber.
        var u = ReconnectTimeline()
        let tcp = LinkSample(route: .relayTCP, routeRttMs: 38, hedgeRoute: nil, hedging: false, healthy: true, silence: 0)
        XCTAssertEqual(u.now(live: tcp, now: 0, network: true)?.chip?.tone, .amber)
        // Disconnected: nothing more.
        t.ended = true
        XCTAssertNil(t.now(live: nil, now: 140, network: true))
    }

    func testQualityBands() {
        XCTAssertEqual(QualityBand.of(rttMs: 19.9, route: .direct), .good)
        XCTAssertEqual(QualityBand.of(rttMs: 20, route: .direct), .fair)
        XCTAssertEqual(QualityBand.of(rttMs: 40, route: .relayUDP), .fair)
        XCTAssertEqual(QualityBand.of(rttMs: 40.1, route: .relayUDP), .poor)
        XCTAssertEqual(QualityBand.of(rttMs: 8, route: .relayTCP), .poor)
    }
}
