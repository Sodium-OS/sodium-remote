// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import XCTest
@testable import RemoteKit

///, revised 2026-11-17: full screen takes every system key but the
/// ways out.
final class SystemKeysTests: XCTestCase {
    let cmd = KeyMods.flagCommand
    let ctrl = KeyMods.flagControl
    let alt = KeyMods.flagOption
    let shift = KeyMods.flagShift

    private func full(_ code: UInt16, _ flags: UInt, ime: Bool = true) -> TapDecision {
        SystemKeys.decide(keyCode: code, flags: flags, fullScreen: true, keepInputSwitch: ime)
    }

    private func windowed(_ code: UInt16, _ flags: UInt) -> TapDecision {
        SystemKeys.decide(keyCode: code, flags: flags, fullScreen: false, keepInputSwitch: true)
    }

    func testFullScreenTakesSystemKeys() {
        XCTAssertEqual(full(0x30, cmd), .take)               // ⌘Tab
        XCTAssertEqual(full(0x0C, cmd), .take)               // ⌘Q
        XCTAssertEqual(full(0x7B, ctrl), .take)              // ⌃← (Mac spaces)
        XCTAssertEqual(full(0x14, cmd | shift), .take)       // ⌘⇧3
        XCTAssertEqual(full(0x31, ctrl, ime: false), .take)  // ⌃Space, computer's input method
        XCTAssertEqual(full(0xA0, 0), .take)                 // Mission Control key
        XCTAssertEqual(full(0x00, 0), .pass)                 // A: through the Mac's input method
        XCTAssertEqual(full(0x7B, KeyMods.flagFunction), .pass)  // ← carries fn by itself
    }

    func testWaysOut() {
        XCTAssertEqual(full(0x35, cmd), .leave)
        XCTAssertEqual(full(0x35, cmd | shift), .disconnect)
        XCTAssertEqual(windowed(0x35, cmd), .leave)
        XCTAssertEqual(full(0x35, cmd | alt), .take)         // ⌥⌘Esc is the computer's
        XCTAssertEqual(full(0x03, cmd | ctrl | alt), .pass)  // ⌃⌥⌘F stays Remote's
        XCTAssertEqual(full(0x31, ctrl), .pass)              // ⌃Space stays the Mac's input switch
    }

    func testWindowTakesOnlyTheThree() {
        XCTAssertEqual(windowed(0x30, cmd), .take)
        XCTAssertEqual(windowed(0x31, cmd), .take)
        XCTAssertEqual(windowed(0x7B, ctrl), .pass)
        XCTAssertEqual(windowed(0x0C, cmd), .pass)
    }
}
