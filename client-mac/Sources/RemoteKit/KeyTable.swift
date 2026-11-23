// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Foundation

/// Keys travel as USB HID usages on the keyboard page (0x07), exactly as
/// pressed: Cmd is Left/Right GUI, Option is Alt, and
/// Globe (fn), which has no usage, is the reserved `KeyUsage.globe`.
public enum KeyUsage {
    /// `msg::USAGE_GLOBE`: reserved above the keyboard page's modifiers.
    public static let globe: UInt16 = 0x0100

    public static let leftCtrl: UInt16 = 0xE0
    public static let leftShift: UInt16 = 0xE1
    public static let leftAlt: UInt16 = 0xE2
    public static let leftGUI: UInt16 = 0xE3
    public static let rightCtrl: UInt16 = 0xE4
    public static let rightShift: UInt16 = 0xE5
    public static let rightAlt: UInt16 = 0xE6
    public static let rightGUI: UInt16 = 0xE7
    public static let capsLock: UInt16 = 0x39
    /// Bound to nothing on the computer (`ShortcutTap`'s ways out).
    public static let f24: UInt16 = 0x73

    /// macOS virtual key code (Carbon's kVK_*) → HID usage, for every key on
    /// the Mac's ANSI, ISO and JIS keyboards.
    public static let fromMacKeyCode: [UInt16: UInt16] = [
        0x00: 0x04, 0x01: 0x16, 0x02: 0x07, 0x03: 0x09, 0x04: 0x0B, 0x05: 0x0A, 0x06: 0x1D, 0x07: 0x1B,  // A S D F H G Z X
        0x08: 0x06, 0x09: 0x19, 0x0A: 0x64, 0x0B: 0x05, 0x0C: 0x14, 0x0D: 0x1A, 0x0E: 0x08, 0x0F: 0x15,  // C V § B Q W E R
        0x10: 0x1C, 0x11: 0x17, 0x12: 0x1E, 0x13: 0x1F, 0x14: 0x20, 0x15: 0x21, 0x16: 0x23, 0x17: 0x22,  // Y T 1 2 3 4 6 5
        0x18: 0x2E, 0x19: 0x26, 0x1A: 0x24, 0x1B: 0x2D, 0x1C: 0x25, 0x1D: 0x27, 0x1E: 0x30, 0x1F: 0x12,  // = 9 7 - 8 0 ] O
        0x20: 0x18, 0x21: 0x2F, 0x22: 0x0C, 0x23: 0x13, 0x24: 0x28, 0x25: 0x0F, 0x26: 0x0D, 0x27: 0x34,  // U [ I P Return L J '
        0x28: 0x0E, 0x29: 0x33, 0x2A: 0x31, 0x2B: 0x36, 0x2C: 0x38, 0x2D: 0x11, 0x2E: 0x10, 0x2F: 0x37,  // K; \, / N M .
        0x30: 0x2B, 0x31: 0x2C, 0x32: 0x35, 0x33: 0x2A, 0x35: 0x29,                                      // Tab Space ` Delete Esc
        0x36: 0xE7, 0x37: 0xE3, 0x38: 0xE1, 0x39: 0x39, 0x3A: 0xE2, 0x3B: 0xE0, 0x3C: 0xE5, 0x3D: 0xE6,  // rCmd Cmd Shift Caps Opt Ctrl rShift rOpt
        0x3E: 0xE4, 0x3F: KeyUsage.globe,                                                                  // rCtrl Globe
        0x40: 0x6C, 0x41: 0x63, 0x43: 0x55, 0x45: 0x57, 0x47: 0x53, 0x48: 0x80, 0x49: 0x81, 0x4A: 0x7F,  // F17 KP. KP* KP+ Clear Vol+ Vol- Mute
        0x4B: 0x54, 0x4C: 0x58, 0x4E: 0x56, 0x4F: 0x6D, 0x50: 0x6E, 0x51: 0x67, 0x52: 0x62, 0x53: 0x59,  // KP/ KPEnter KP- F18 F19 KP= KP0 KP1
        0x54: 0x5A, 0x55: 0x5B, 0x56: 0x5C, 0x57: 0x5D, 0x58: 0x5E, 0x59: 0x5F, 0x5A: 0x6F, 0x5B: 0x60,  // KP2..KP7 F20 KP8
        0x5C: 0x61, 0x5D: 0x89, 0x5E: 0x87, 0x5F: 0x85,                                                  // KP9 ¥ _ KP,
        0x60: 0x3E, 0x61: 0x3F, 0x62: 0x40, 0x63: 0x3C, 0x64: 0x41, 0x65: 0x42, 0x66: 0x91, 0x67: 0x44,  // F5 F6 F7 F3 F8 F9 Eisu F11
        0x68: 0x90, 0x69: 0x68, 0x6A: 0x6B, 0x6B: 0x69, 0x6D: 0x43, 0x6E: 0x65, 0x6F: 0x45, 0x71: 0x6A,  // Kana F13 F16 F14 F10 Menu F12 F15
        0x72: 0x49, 0x73: 0x4A, 0x74: 0x4B, 0x75: 0x4C, 0x76: 0x3D, 0x77: 0x4D, 0x78: 0x3B, 0x79: 0x4E,  // Help/Ins Home PgUp Del F4 End F2 PgDn
        0x7A: 0x3A, 0x7B: 0x50, 0x7C: 0x4F, 0x7D: 0x51, 0x7E: 0x52,                                      // F1 ← → ↓ ↑
    ]

    /// The modifier keys' macOS key codes, and the device-dependent flag bit
    /// (IOKit's NX_DEVICE*KEYMASK) that says whether that side is down.
    public static let modifierDeviceBits: [UInt16: UInt] = [
        0x3B: 0x0001,  // left Control
        0x3E: 0x2000,  // right Control
        0x38: 0x0002,  // left Shift
        0x3C: 0x0004,  // right Shift
        0x37: 0x0008,  // left Command
        0x36: 0x0010,  // right Command
        0x3A: 0x0020,  // left Option
        0x3D: 0x0040,  // right Option
    ]

    public static func isModifier(_ usage: UInt16) -> Bool {
        (0xE0...0xE7).contains(usage) || usage == globe
    }
}

/// `msg::mods` bits: either side's modifiers in a chord.
public enum KeyMods {
    public static let ctrl: UInt8 = 1
    public static let shift: UInt8 = 2
    public static let alt: UInt8 = 4
    public static let gui: UInt8 = 8
    public static let globe: UInt8 = 16

    /// NSEvent.ModifierFlags' device-independent bits (raw values).
    public static let flagShift: UInt = 1 << 17
    public static let flagControl: UInt = 1 << 18
    public static let flagOption: UInt = 1 << 19
    public static let flagCommand: UInt = 1 << 20
    public static let flagFunction: UInt = 1 << 23

    /// The chord bits for an event's modifier flags. The function flag also
    /// rides on arrows and F-keys, so it only counts as Globe for the Globe
    /// key itself (`withGlobe`).
    public static func from(flags: UInt, withGlobe: Bool = false) -> UInt8 {
        var m: UInt8 = 0
        if flags & flagControl != 0 { m |= ctrl }
        if flags & flagShift != 0 { m |= shift }
        if flags & flagOption != 0 { m |= alt }
        if flags & flagCommand != 0 { m |= gui }
        if withGlobe && flags & flagFunction != 0 { m |= globe }
        return m
    }
}

/// `msg::KeyCondition`.
public enum KeyConditionModel: Codable, Equatable, Hashable {
    case any
    case terminal
    case appClass(String)
}

/// `msg::KeyChord`: modifiers plus one key (a HID usage, or a modifier's own
/// usage for a modifier remap).
public struct KeyChordModel: Codable, Equatable, Hashable {
    public var mods: UInt8
    public var key: UInt16

    public init(mods: UInt8, key: UInt16) {
        self.mods = mods
        self.key = key
    }

    /// A bare modifier (or Globe): a modifier remap when it is a rule's `from`.
    public var isBareModifier: Bool { mods == 0 && KeyUsage.isModifier(key) }
}

/// `msg::KeyRule`: in `when`, `from` is sent as `to` (every chord but the last
/// tapped, the last held while `from` is). An empty `to` drops the key.
public struct KeyRuleModel: Codable, Equatable, Hashable, Identifiable {
    public var when: KeyConditionModel
    public var from: KeyChordModel
    public var to: [KeyChordModel]

    public init(when: KeyConditionModel, from: KeyChordModel, to: [KeyChordModel]) {
        self.when = when
        self.from = from
        self.to = to
    }

    /// Rules are unique by where they apply and what they match.
    public var id: String {
        let w: String
        switch when {
        case .any: w = "any"
        case .terminal: w = "terminal"
        case .appClass(let c): w = "class:" + c
        }
        return "\(w)/\(from.mods)/\(from.key)"
    }
}

/// The table the user edits (Settings › Input), persisted as JSON. Mac keys
/// sends `rules`; PC keys sends an empty table (passthrough).
public struct KeyTable: Codable, Equatable {
    public var rules: [KeyRuleModel]

    public init(rules: [KeyRuleModel]) {
        self.rules = rules
    }

    /// Replace (or add) the rule matching `rule.id`.
    public mutating func upsert(_ rule: KeyRuleModel) {
        if let i = rules.firstIndex(where: { $0.id == rule.id }) {
            rules[i] = rule
        } else {
            rules.append(rule)
        }
    }

    public mutating func remove(id: String) {
        rules.removeAll { $0.id == id }
    }

    /// Rows that differ from `defaults` (for the "· changed" mark).
    public func changedIDs(against defaults: KeyTable) -> Set<String> {
        var out = Set<String>()
        for r in rules {
            if defaults.rules.first(where: { $0.id == r.id }) != r { out.insert(r.id) }
        }
        return out
    }

    public func encoded() -> Data {
        (try? JSONEncoder().encode(self)) ?? Data()
    }

    public static func decoded(_ data: Data) -> KeyTable? {
        try? JSONDecoder().decode(KeyTable.self, from: data)
    }
}

/// Recording a chord in the key table editor (MSe5: "Press the keys to send…").
public enum KeyRecorder {
    /// A key press with these modifier flags (NSEvent raw values) as a chord.
    public static func chord(keyCode: UInt16, flags: UInt) -> KeyChordModel? {
        guard let usage = KeyUsage.fromMacKeyCode[keyCode] else { return nil }
        if KeyUsage.isModifier(usage) {
            // A modifier alone: its own usage, no modifier bits.
            return KeyChordModel(mods: 0, key: usage)
        }
        return KeyChordModel(mods: KeyMods.from(flags: flags), key: usage)
    }
}

/// Names for the table: the Mac side in its own glyphs, the computer's side in
/// PC words (the board's KEYMAP).
public enum KeyNames {
    static let letters: [UInt16: String] = {
        var m: [UInt16: String] = [:]
        let a = Array("ABCDEFGHIJKLMNOPQRSTUVWXYZ")
        for (i, c) in a.enumerated() { m[UInt16(0x04 + i)] = String(c) }
        let d = Array("1234567890")
        for (i, c) in d.enumerated() { m[UInt16(0x1E + i)] = String(c) }
        for i in 0..<12 { m[UInt16(0x3A + i)] = "F\(i + 1)" }
        for i in 0..<12 { m[UInt16(0x68 + i)] = "F\(i + 13)" }
        let punct: [UInt16: String] = [0x2D: "-", 0x2E: "=", 0x2F: "[", 0x30: "]", 0x31: "\\", 0x33: ";", 0x34: "'",
                                       0x35: "`", 0x36: ",", 0x37: ".", 0x38: "/", 0x64: "§"]
        for (k, v) in punct { m[k] = v }
        return m
    }()

    static func unnamedKey(_ usage: UInt16) -> String {
        let hex = String(format: "%02X", usage)
        return String(localized: "Key \(hex)")
    }

    /// A usage's name on the Mac (glyphs where macOS uses them).
    public static func macKey(_ usage: UInt16) -> String {
        switch usage {
        case 0x28: return "↩"
        case 0x29: return "⎋"
        case 0x2A: return "⌫"
        case 0x2B: return "⇥"
        case 0x2C: return String(localized: "Space")
        case 0x4C: return "⌦"
        case 0x4A: return "↖"
        case 0x4D: return "↘"
        case 0x4B: return "⇞"
        case 0x4E: return "⇟"
        case 0x4F: return "→"
        case 0x50: return "←"
        case 0x51: return "↓"
        case 0x52: return "↑"
        case 0x39: return "⇪"
        case KeyUsage.leftCtrl: return "⌃"
        case KeyUsage.rightCtrl: return String(localized: "⌃ (right)")
        case KeyUsage.leftShift: return "⇧"
        case KeyUsage.rightShift: return String(localized: "⇧ (right)")
        case KeyUsage.leftAlt: return "⌥"
        case KeyUsage.rightAlt: return String(localized: "⌥ (right)")
        case KeyUsage.leftGUI: return "⌘"
        case KeyUsage.rightGUI: return String(localized: "⌘ (right)")
        case KeyUsage.globe: return String(localized: "Globe (fn)")
        default: return letters[usage] ?? unnamedKey(usage)
        }
    }

    /// A usage's name on the computer (PC words).
    public static func hostKey(_ usage: UInt16) -> String {
        switch usage {
        case 0x28: return String(localized: "Enter")
        case 0x29: return String(localized: "Esc")
        case 0x2A: return "⌫"
        case 0x2B: return String(localized: "Tab")
        case 0x2C: return String(localized: "Space")
        case 0x4C: return String(localized: "Delete")
        case 0x49: return String(localized: "Insert")
        case 0x4A: return String(localized: "Home")
        case 0x4D: return String(localized: "End")
        case 0x4B: return String(localized: "Page Up")
        case 0x4E: return String(localized: "Page Down")
        case 0x4F: return "→"
        case 0x50: return "←"
        case 0x51: return "↓"
        case 0x52: return "↑"
        case 0x39: return String(localized: "Caps Lock")
        case 0x46: return String(localized: "Print Screen")
        case KeyUsage.leftCtrl, KeyUsage.rightCtrl: return String(localized: "Ctrl")
        case KeyUsage.leftShift, KeyUsage.rightShift: return String(localized: "Shift")
        case KeyUsage.leftAlt, KeyUsage.rightAlt: return String(localized: "Alt")
        case KeyUsage.leftGUI, KeyUsage.rightGUI: return String(localized: "Win")
        case KeyUsage.globe: return String(localized: "Globe")
        default: return letters[usage] ?? unnamedKey(usage)
        }
    }

    /// "⌘C", "⌥←", String(localized: "⌘ (right)").
    public static func mac(_ c: KeyChordModel) -> String {
        var s = ""
        if c.mods & KeyMods.globe != 0 { s += String(localized: "Globe") + " " }
        if c.mods & KeyMods.ctrl != 0 { s += "⌃" }
        if c.mods & KeyMods.alt != 0 { s += "⌥" }
        if c.mods & KeyMods.shift != 0 { s += "⇧" }
        if c.mods & KeyMods.gui != 0 { s += "⌘" }
        return s + macKey(c.key)
    }

    /// "Ctrl + Shift + C", "Win + S".
    public static func host(_ c: KeyChordModel) -> String {
        var parts: [String] = []
        if c.mods & KeyMods.ctrl != 0 { parts.append(String(localized: "Ctrl")) }
        if c.mods & KeyMods.shift != 0 { parts.append(String(localized: "Shift")) }
        if c.mods & KeyMods.alt != 0 { parts.append(String(localized: "Alt")) }
        if c.mods & KeyMods.gui != 0 { parts.append(String(localized: "Win")) }
        if c.mods & KeyMods.globe != 0 { parts.append(String(localized: "Globe")) }
        parts.append(hostKey(c.key))
        return parts.joined(separator: " + ")
    }

    /// A rule's "On this Mac" cell: "⌘C in a terminal".
    public static func from(_ r: KeyRuleModel) -> String {
        let base = mac(r.from)
        switch r.when {
        case .any: return base
        case .terminal: return String(localized: "\(base) in a terminal")
        case .appClass(let c): return String(localized: "\(base) in \(c)")
        }
    }

    /// A rule's "Sent as" cell: "Shift + Home, ⌫"; "Nothing" for a dropped key.
    public static func to(_ r: KeyRuleModel) -> String {
        if r.to.isEmpty { return String(localized: "Nothing") }
        return r.to.map(host).joined(separator: ", ")
    }
}
