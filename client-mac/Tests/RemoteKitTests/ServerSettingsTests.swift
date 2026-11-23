// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import XCTest
@testable import RemoteKit

final class ServerSettingsTests: XCTestCase {
    func testTypedAddressesBecomeWssURLs() {
        // The same cases as the core's `normalize_address`: a bare host gets wss:// and the default path.
        XCTAssertEqual(ServerAddress.normalize(" relay.example.org "), .success("wss://relay.example.org/remote"))
        XCTAssertEqual(ServerAddress.normalize("relay.example.org:8443"), .success("wss://relay.example.org:8443/remote"))
        XCTAssertEqual(ServerAddress.normalize("https://relay.example.org/"), .success("wss://relay.example.org/remote"))
        XCTAssertEqual(ServerAddress.normalize("https://relay.example.org/join"), .success("wss://relay.example.org/join"))
        XCTAssertEqual(ServerAddress.normalize("WSS://relay.example.org:8443/remote"), .success("wss://relay.example.org:8443/remote"))
        // Never upgraded.
        XCTAssertEqual(ServerAddress.normalize("http://relay.example.org"), .failure(.insecure))
        XCTAssertEqual(ServerAddress.normalize("WS://relay.example.org/remote"), .failure(.insecure))
        for bad in ["  ", "ftp://relay.example.org", "https://", "/remote", "relay example.org"] {
            XCTAssertEqual(ServerAddress.normalize(bad), .failure(.invalid), bad)
        }
        XCTAssertEqual(ServerAddress.host(of: "wss://relay.example.org:8443/remote"), "relay.example.org")
    }

    func testLinksAreRecognisedByTheirScheme() {
        XCTAssertTrue(ServerAddress.looksLikeLink(" SODIUM-REMOTE://server?url=wss%3A%2F%2Fa.example"))
        XCTAssertFalse(ServerAddress.looksLikeLink("https://relay.example.org/join"))
    }

    func testRefusalsAreRecognisedByTheirCode() {
        XCTAssertEqual(ServerProblem.parse("rendezvous refused: token_invalid (no such token)"), .tokenInvalid)
        XCTAssertEqual(ServerProblem.parse("rendezvous refused: token_expired (used up)"), .tokenExpired)
        XCTAssertEqual(ServerProblem.parse("rendezvous refused: host_not_allowed (x)"), .tokenNeeded)
        XCTAssertEqual(ServerProblem.parse("rendezvous refused: quota_exceeded (x)"), .quotaExceeded)
        XCTAssertEqual(ServerProblem.parse("rendezvous refused: payment_required (x)"), .paymentRequired)
        XCTAssertNil(ServerProblem.parse("rendezvous refused: rate_limited (x)"))
        XCTAssertNil(ServerProblem.parse("rendezvous unreachable: connection refused"))
        XCTAssertNil(ServerProblem.parse(nil))
    }
}
