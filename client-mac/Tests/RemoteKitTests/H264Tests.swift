// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import XCTest
@testable import RemoteKit

///: an H.264 keyframe's SPS and PPS go to the format
/// description, everything else (but delimiters) is the sample.
final class H264Tests: XCTestCase {
    func testParameterSetsAndSampleNALs() {
        let aud: [UInt8] = [0x09, 0xF0]
        let sps: [UInt8] = [0x67, 0x42, 0x00, 0x1F]
        let pps: [UInt8] = [0x68, 0xCE, 0x3C, 0x80]
        let idr: [UInt8] = [0x65, 0x88, 0x84]
        let nals = [aud, sps, pps, idr]
        XCTAssertEqual(H264.parameterSets(nals), H264.ParameterSets(sps: sps, pps: pps))
        XCTAssertEqual(H264.sampleNALs(nals), [idr])
        XCTAssertNil(H264.parameterSets([idr]))
    }
}
