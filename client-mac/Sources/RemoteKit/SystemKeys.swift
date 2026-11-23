// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation

/// What the system-key tap does with a key press.
public enum TapDecision: Equatable {
    /// macOS and Remote's window handle it as usual.
    case pass
    /// The computer gets it; macOS never sees it.
    case take
    /// ⌘Esc: out of the stream (full screen ends, the keyboard goes back to the Mac).
    case leave
    /// ⌘⇧Esc: disconnect.
    case disconnect
}

/// Which keys the event tap takes from macOS.
///
/// In full screen the stream is the whole Mac: every key macOS would act on
/// (⌘ anything, ⌃ anything, the function row, Globe shortcuts) goes to the
/// computer. ⌘Esc and ⌘⇧Esc are the ways out, and Remote's own ⌃⌥⌘ keys stay
/// Remote's. With the Mac's input method on, ⌃Space stays its switch. In a
/// focused window only ⌘Tab, ⌘` and ⌘Space are taken, so the Mac's spaces
/// stay a gesture away.
public enum SystemKeys {
    public static let escape: UInt16 = 0x35
    public static let space: UInt16 = 0x31

    /// ⌘Tab, ⌘`, ⌘Space: taken in a focused window too.
    public static let windowed: Set<UInt16> = [0x30, 0x32, 0x31]

    /// Remote's own ⌃⌥⌘ shortcuts: F, S, M, D, K, L.
    public static let remoteOwn: Set<UInt16> = [0x03, 0x01, 0x2E, 0x02, 0x28, 0x25]

    /// F1–F20, and the keys newer Apple keyboards send in their place
    /// (Mission Control, Launchpad, Dictation, Spotlight, Do Not Disturb).
    public static let functionRow: Set<UInt16> = [
        0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D,  // F1–F10
        0x67, 0x6F, 0x69, 0x6B, 0x71, 0x6A, 0x40, 0x4F, 0x50, 0x5A,  // F11–F20
        0xA0, 0x83, 0xB0, 0xB1, 0xB2,
    ]

    /// Keys that carry the function flag by themselves: arrows, Home, End,
    /// Page Up/Down, Forward Delete, Help.
    public static let navigation: Set<UInt16> = [0x7B, 0x7C, 0x7D, 0x7E, 0x73, 0x77, 0x74, 0x79, 0x75, 0x72]

    /// `flags`: NSEvent's or CGEvent's device-independent modifier bits (the
    /// same values).
    public static func decide(keyCode: UInt16, flags: UInt, fullScreen: Bool, keepInputSwitch: Bool) -> TapDecision {
        let cmd = flags & KeyMods.flagCommand != 0
        let ctrl = flags & KeyMods.flagControl != 0
        let alt = flags & KeyMods.flagOption != 0
        let shift = flags & KeyMods.flagShift != 0
        let fn = flags & KeyMods.flagFunction != 0
        if keyCode == escape && cmd && !ctrl && !alt {
            return shift ? .disconnect : .leave
        }
        if cmd && ctrl && alt && remoteOwn.contains(keyCode) { return .pass }
        guard fullScreen else {
            return cmd && windowed.contains(keyCode) ? .take : .pass
        }
        if keepInputSwitch && ctrl && !cmd && keyCode == space { return .pass }
        if cmd || ctrl || functionRow.contains(keyCode) { return .take }
        if fn && !navigation.contains(keyCode) { return .take }
        return .pass
    }
}
