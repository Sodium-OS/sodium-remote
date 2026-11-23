// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import XCTest
@testable import RemoteKit

///: keys go as HID usages exactly as pressed (Cmd = GUI, Option =
/// Alt, Globe = 0x0100), and the table the user edits round-trips.
final class KeyTableTests: XCTestCase {
    func testMacKeysAsHIDUsages() {
        // kVK_ANSI_A, kVK_ANSI_C, kVK_Return, kVK_Escape, kVK_UpArrow, kVK_F12.
        XCTAssertEqual(KeyUsage.fromMacKeyCode[0x00], 0x04)
        XCTAssertEqual(KeyUsage.fromMacKeyCode[0x08], 0x06)
        XCTAssertEqual(KeyUsage.fromMacKeyCode[0x24], 0x28)
        XCTAssertEqual(KeyUsage.fromMacKeyCode[0x35], 0x29)
        XCTAssertEqual(KeyUsage.fromMacKeyCode[0x7E], 0x52)
        XCTAssertEqual(KeyUsage.fromMacKeyCode[0x6F], 0x45)
        // Modifiers as pressed: Cmd is GUI, Option is Alt, Globe the reserved usage.
        XCTAssertEqual(KeyUsage.fromMacKeyCode[0x37], KeyUsage.leftGUI)
        XCTAssertEqual(KeyUsage.fromMacKeyCode[0x36], KeyUsage.rightGUI)
        XCTAssertEqual(KeyUsage.fromMacKeyCode[0x3A], KeyUsage.leftAlt)
        XCTAssertEqual(KeyUsage.fromMacKeyCode[0x3F], 0x0100)
        // No two Mac keys share a usage.
        let usages = Array(KeyUsage.fromMacKeyCode.values)
        XCTAssertEqual(Set(usages).count, usages.count)
    }

    func testRecordingAChord() {
        let cmd = KeyMods.flagCommand
        // ⌘C → (GUI, C); ⌃⇧C → (CTRL|SHIFT, C); a bare ⌘ → its own usage.
        XCTAssertEqual(KeyRecorder.chord(keyCode: 0x08, flags: cmd), KeyChordModel(mods: KeyMods.gui, key: 0x06))
        XCTAssertEqual(KeyRecorder.chord(keyCode: 0x08, flags: KeyMods.flagControl | KeyMods.flagShift),
                       KeyChordModel(mods: KeyMods.ctrl | KeyMods.shift, key: 0x06))
        XCTAssertEqual(KeyRecorder.chord(keyCode: 0x37, flags: cmd), KeyChordModel(mods: 0, key: KeyUsage.leftGUI))
        // The function flag rides on arrows: it isn't Globe there.
        XCTAssertEqual(KeyRecorder.chord(keyCode: 0x7B, flags: cmd | KeyMods.flagFunction), KeyChordModel(mods: KeyMods.gui, key: 0x50))
        XCTAssertNil(KeyRecorder.chord(keyCode: 0x34, flags: 0))
    }

    func testTheTableRoundTripsAndMarksChanges() {
        let copy = KeyRuleModel(when: .terminal, from: KeyChordModel(mods: KeyMods.gui, key: 0x06),
                                to: [KeyChordModel(mods: KeyMods.ctrl | KeyMods.shift, key: 0x06)])
        let home = KeyRuleModel(when: .any, from: KeyChordModel(mods: KeyMods.gui, key: 0x2A),
                                to: [KeyChordModel(mods: KeyMods.shift, key: 0x4A), KeyChordModel(mods: 0, key: 0x2A)])
        let defaults = KeyTable(rules: [copy, home])
        XCTAssertEqual(KeyTable.decoded(defaults.encoded()), defaults)
        XCTAssertEqual(KeyNames.from(copy), "⌘C in a terminal")
        XCTAssertEqual(KeyNames.to(copy), "Ctrl + Shift + C")
        XCTAssertEqual(KeyNames.to(home), "Shift + Home, ⌫")
        var edited = defaults
        edited.upsert(KeyRuleModel(when: .terminal, from: copy.from, to: [KeyChordModel(mods: KeyMods.ctrl, key: 0x06)]))
        XCTAssertEqual(edited.rules.count, 2)
        XCTAssertEqual(edited.changedIDs(against: defaults), [copy.id])
    }
}
