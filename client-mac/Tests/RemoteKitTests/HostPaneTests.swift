// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import XCTest
@testable import RemoteKit

final class HostPaneTests: XCTestCase {
    func testDisplayPickerRules() {
        XCTAssertFalse(DisplayPicker.isShown(displays: 1, virtual: false))
        XCTAssertFalse(DisplayPicker.isShown(displays: 3, virtual: true))
        XCTAssertTrue(DisplayPicker.isShown(displays: 2, virtual: false))
        let named = DisplayChoice(id: 1, name: "DELL U2720Q", width: 2560, height: 1440, primary: true)
        let blank = DisplayChoice(id: 2, name: " ", width: 1920, height: 1080, primary: false)
        XCTAssertEqual(DisplayPicker.label(named, index: 0), .named("DELL U2720Q"))
        XCTAssertEqual(DisplayPicker.label(blank, index: 1), .numbered(2))
        XCTAssertEqual(DisplayPicker.sizeText(named), "2560 × 1440")
    }

    func testPermissionRowsNeverSayAllowedBeforeItWorks() {
        XCTAssertEqual(HostPermissionRules.row(.reopenToFinish, settingsOpened: true), .reopenToFinish)
        XCTAssertEqual(HostPermissionRules.row(.notAllowed, settingsOpened: false), .notAllowed)
        XCTAssertEqual(HostPermissionRules.row(.notAllowed, settingsOpened: true), .waiting)
        XCTAssertEqual(HostPermissionRules.row(.allowed, settingsOpened: true), .allowed)
        XCTAssertTrue(HostPermissionRules.viewOnly(screenRecording: .allowed, accessibility: .notAllowed))
        XCTAssertFalse(HostPermissionRules.viewOnly(screenRecording: .notAllowed, accessibility: .notAllowed))
        XCTAssertFalse(HostPermissionRules.viewOnly(screenRecording: .allowed, accessibility: .allowed))
    }

    func testPresenceStateAndAddress() {
        XCTAssertEqual(PresenceState.of(allowed: false, controlled: false), .off)
        XCTAssertEqual(PresenceState.of(allowed: true, controlled: false), .on)
        XCTAssertEqual(PresenceState.of(allowed: true, controlled: true), .controlled)
        XCTAssertEqual(HostAddress.hostPort(ip: "192.0.2.20", port: 47311), "192.0.2.20:47311")
        XCTAssertEqual(HostAddress.string(id: "ID", ip: "192.0.2.20", port: 47311), "ID@192.0.2.20:47311")
        XCTAssertEqual(HostAddress.string(id: "ID", ip: "fe80::1", port: 47311), "ID@[fe80::1]:47311")
        XCTAssertFalse(HostOS.sodiumOS.namesSodiumRemote)
        // "Send Ctrl+Alt+Del" is for SodiumOS hosts only.
        XCTAssertTrue(HostOS.sodiumOS.offersCtrlAltDel)
        XCTAssertFalse(HostOS.windows.offersCtrlAltDel)
        XCTAssertFalse(HostOS.mac.offersCtrlAltDel)
        XCTAssertFalse(HostOS.linux.offersCtrlAltDel)
        XCTAssertTrue(HostOS.linux.namesSodiumRemote)
        XCTAssertEqual(HostOS.linux.pillText, "Linux")
    }
}
