// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import XCTest
@testable import RemoteKit

final class BrandTests: XCTestCase {
    func testTestBuildIsToldByTheBundleID() {
        XCTAssertFalse(Brand.isTestBuild(bundleID: Brand.bundleID))
        XCTAssertTrue(Brand.isTestBuild(bundleID: Brand.bundleIDDev))
    }

    func testLabelsFollowTheBundleID() {
        XCTAssertEqual(Brand.label("decode", bundleID: Brand.bundleIDDev), "com.sodiumos.remote.dev.decode")
    }
}
