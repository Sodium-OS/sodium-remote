// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! USB HID usages (keyboard page 0x07, what the client sends) → Linux evdev codes
//! (`hid_to_evdev`, for the generic Linux backend's libei and portal input), → macOS key codes
//! (`hid_to_mac`, for the Mac backend's `CGEventCreateKeyboardEvent`), and → PC/AT scan code set 1,
//! as Windows' `SendInput` takes them (`KEYEVENTF_SCANCODE`, with
//! `KEYEVENTF_EXTENDEDKEY` for the E0-prefixed keys). Scan codes keep the host's own
//! keyboard layout in charge, as a real keyboard would. Pure data, so it is tested on
//! any OS; the Windows backend uses it.
//!
//! A few keys go by virtual-key code instead, because their scan codes are ambiguous
//! (Pause is the E1 sequence, Num Lock shares 0x45 with it) or have no fixed make code
//! (the Korean and Japanese language keys).

/// How Windows should see one key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinKey {
    /// A set-1 make code; `extended` for the E0 prefix.
    Scan { code: u16, extended: bool },
    /// A virtual-key code with its scan code (for `KEYBDINPUT.wScan`).
    Vk { vk: u16, scan: u16, extended: bool },
}

const fn s(code: u16) -> Option<WinKey> {
    Some(WinKey::Scan { code, extended: false })
}
const fn e(code: u16) -> Option<WinKey> {
    Some(WinKey::Scan { code, extended: true })
}

pub const VK_PAUSE: u16 = 0x13;
pub const VK_NUMLOCK: u16 = 0x90;
/// VK_HANGUL, also VK_KANA.
pub const VK_HANGUL: u16 = 0x15;
/// VK_HANJA, also VK_KANJI.
pub const VK_HANJA: u16 = 0x19;

/// The key for a HID usage, or None for a usage with no PC key.
pub fn hid_to_win(usage: u16) -> Option<WinKey> {
    match usage {
        // Letters A–Z.
        0x04 => s(0x1E),
        0x05 => s(0x30),
        0x06 => s(0x2E),
        0x07 => s(0x20),
        0x08 => s(0x12),
        0x09 => s(0x21),
        0x0A => s(0x22),
        0x0B => s(0x23),
        0x0C => s(0x17),
        0x0D => s(0x24),
        0x0E => s(0x25),
        0x0F => s(0x26),
        0x10 => s(0x32),
        0x11 => s(0x31),
        0x12 => s(0x18),
        0x13 => s(0x19),
        0x14 => s(0x10),
        0x15 => s(0x13),
        0x16 => s(0x1F),
        0x17 => s(0x14),
        0x18 => s(0x16),
        0x19 => s(0x2F),
        0x1A => s(0x11),
        0x1B => s(0x2D),
        0x1C => s(0x15),
        0x1D => s(0x2C),
        // 1–9, 0.
        0x1E..=0x26 => s(usage - 0x1E + 0x02),
        0x27 => s(0x0B),
        0x28 => s(0x1C), // Enter
        0x29 => s(0x01), // Escape
        0x2A => s(0x0E), // Backspace
        0x2B => s(0x0F), // Tab
        0x2C => s(0x39), // Space
        0x2D => s(0x0C), // -
        0x2E => s(0x0D), // =
        0x2F => s(0x1A), // [
        0x30 => s(0x1B), // ]
        0x31 => s(0x2B), // \
        0x32 => s(0x2B), // Non-US # (the ISO key left of Enter)
        0x33 => s(0x27), //;
        0x34 => s(0x28), // '
        0x35 => s(0x29), // `
        0x36 => s(0x33), //,
        0x37 => s(0x34), // .
        0x38 => s(0x35), // /
        0x39 => s(0x3A), // Caps Lock
        // F1–F10, F11, F12.
        0x3A..=0x43 => s(usage - 0x3A + 0x3B),
        0x44 => s(0x57),
        0x45 => s(0x58),
        0x46 => e(0x37), // Print Screen
        0x47 => s(0x46), // Scroll Lock
        0x48 => Some(WinKey::Vk { vk: VK_PAUSE, scan: 0x45, extended: false }),
        0x49 => e(0x52), // Insert
        0x4A => e(0x47), // Home
        0x4B => e(0x49), // Page Up
        0x4C => e(0x53), // Delete
        0x4D => e(0x4F), // End
        0x4E => e(0x51), // Page Down
        0x4F => e(0x4D), // Right
        0x50 => e(0x4B), // Left
        0x51 => e(0x50), // Down
        0x52 => e(0x48), // Up
        0x53 => Some(WinKey::Vk { vk: VK_NUMLOCK, scan: 0x45, extended: true }),
        0x54 => e(0x35), // Keypad /
        0x55 => s(0x37), // Keypad *
        0x56 => s(0x4A), // Keypad -
        0x57 => s(0x4E), // Keypad +
        0x58 => e(0x1C), // Keypad Enter
        0x59 => s(0x4F), // Keypad 1
        0x5A => s(0x50),
        0x5B => s(0x51),
        0x5C => s(0x4B),
        0x5D => s(0x4C),
        0x5E => s(0x4D),
        0x5F => s(0x47),
        0x60 => s(0x48),
        0x61 => s(0x49), // Keypad 9
        0x62 => s(0x52), // Keypad 0
        0x63 => s(0x53), // Keypad .
        0x64 => s(0x56), // Non-US \ (the ISO key left of Z)
        0x65 => e(0x5D), // Application (menu)
        0x66 => e(0x5E), // Power
        0x67 => s(0x59), // Keypad =
        // F13–F23, F24.
        0x68..=0x72 => s(usage - 0x68 + 0x64),
        0x73 => s(0x76),
        0x7F => e(0x20), // Mute
        0x80 => e(0x30), // Volume Up
        0x81 => e(0x2E), // Volume Down
        0x85 => s(0x7E), // Keypad, (Brazil)
        0x87 => s(0x73), // International1: Ro
        0x88 => s(0x70), // International2: Katakana/Hiragana
        0x89 => s(0x7D), // International3: Yen
        0x8A => s(0x79), // International4: Henkan
        0x8B => s(0x7B), // International5: Muhenkan
        0x90 => Some(WinKey::Vk { vk: VK_HANGUL, scan: 0x72, extended: false }), // LANG1: Hangul, Kana
        0x91 => Some(WinKey::Vk { vk: VK_HANJA, scan: 0x71, extended: false }),  // LANG2: Hanja, Eisu
        0xE0 => s(0x1D), // Left Ctrl
        0xE1 => s(0x2A), // Left Shift
        0xE2 => s(0x38), // Left Alt
        0xE3 => e(0x5B), // Left Win
        0xE4 => e(0x1D), // Right Ctrl
        0xE5 => s(0x36), // Right Shift
        0xE6 => e(0x38), // Right Alt (AltGr)
        0xE7 => e(0x5C), // Right Win
        _ => None,
    }
}

/// A HID usage → macOS's virtual key code (`CGKeyCode`, Carbon's `kVK_*`), the inverse
/// of the Mac client's `KeyUsage.fromMacKeyCode` (client-mac RemoteKit/KeyTable.swift),
/// plus the PC's keys a Mac keyboard lacks, as macOS reads a PC keyboard: Print Screen,
/// Scroll Lock and Pause are F13–F15, Insert is Help, Num Lock is Clear, Application is
/// the context-menu key, and a Japanese PC keyboard's Muhenkan/Henkan are Eisu/Kana. The
/// codes are positions: the Mac's own layout gives the character. Globe is fn (0x3F).
pub fn hid_to_mac(usage: u16) -> Option<u16> {
    Some(match usage {
        // Letters A–Z.
        0x04 => 0x00,
        0x05 => 0x0B,
        0x06 => 0x08,
        0x07 => 0x02,
        0x08 => 0x0E,
        0x09 => 0x03,
        0x0A => 0x05,
        0x0B => 0x04,
        0x0C => 0x22,
        0x0D => 0x26,
        0x0E => 0x28,
        0x0F => 0x25,
        0x10 => 0x2E,
        0x11 => 0x2D,
        0x12 => 0x1F,
        0x13 => 0x23,
        0x14 => 0x0C,
        0x15 => 0x0F,
        0x16 => 0x01,
        0x17 => 0x11,
        0x18 => 0x20,
        0x19 => 0x09,
        0x1A => 0x0D,
        0x1B => 0x07,
        0x1C => 0x10,
        0x1D => 0x06,
        // 1–9, 0.
        0x1E => 0x12,
        0x1F => 0x13,
        0x20 => 0x14,
        0x21 => 0x15,
        0x22 => 0x17,
        0x23 => 0x16,
        0x24 => 0x1A,
        0x25 => 0x1C,
        0x26 => 0x19,
        0x27 => 0x1D,
        0x28 => 0x24, // Return
        0x29 => 0x35, // Escape
        0x2A => 0x33, // Backspace (Delete)
        0x2B => 0x30, // Tab
        0x2C => 0x31, // Space
        0x2D => 0x1B, // -
        0x2E => 0x18, // =
        0x2F => 0x21, // [
        0x30 => 0x1E, // ]
        0x31 => 0x2A, // \
        0x32 => 0x2A, // Non-US # (the ISO key left of Return: macOS reads it as \)
        0x33 => 0x29, //;
        0x34 => 0x27, // '
        0x35 => 0x32, // `
        0x36 => 0x2B, //,
        0x37 => 0x2F, // .
        0x38 => 0x2C, // /
        0x39 => 0x39, // Caps Lock
        0x3A => 0x7A, // F1
        0x3B => 0x78,
        0x3C => 0x63,
        0x3D => 0x76,
        0x3E => 0x60,
        0x3F => 0x61,
        0x40 => 0x62,
        0x41 => 0x64,
        0x42 => 0x65,
        0x43 => 0x6D,
        0x44 => 0x67,
        0x45 => 0x6F, // F12
        0x46 => 0x69, // Print Screen: F13
        0x47 => 0x6B, // Scroll Lock: F14
        0x48 => 0x71, // Pause: F15
        0x49 => 0x72, // Insert: Help
        0x4A => 0x73, // Home
        0x4B => 0x74, // Page Up
        0x4C => 0x75, // Delete (forward)
        0x4D => 0x77, // End
        0x4E => 0x79, // Page Down
        0x4F => 0x7C, // Right
        0x50 => 0x7B, // Left
        0x51 => 0x7D, // Down
        0x52 => 0x7E, // Up
        0x53 => 0x47, // Num Lock: Clear
        0x54 => 0x4B, // Keypad /
        0x55 => 0x43, // Keypad *
        0x56 => 0x4E, // Keypad -
        0x57 => 0x45, // Keypad +
        0x58 => 0x4C, // Keypad Enter
        0x59 => 0x53, // Keypad 1
        0x5A => 0x54,
        0x5B => 0x55,
        0x5C => 0x56,
        0x5D => 0x57,
        0x5E => 0x58,
        0x5F => 0x59,
        0x60 => 0x5B,
        0x61 => 0x5C, // Keypad 9
        0x62 => 0x52, // Keypad 0
        0x63 => 0x41, // Keypad .
        0x64 => 0x0A, // Non-US \ (ISO §, left of Z)
        0x65 => 0x6E, // Application (menu)
        0x67 => 0x51, // Keypad =
        0x68 => 0x69, // F13
        0x69 => 0x6B, // F14
        0x6A => 0x71, // F15
        0x6B => 0x6A, // F16
        0x6C => 0x40, // F17
        0x6D => 0x4F, // F18
        0x6E => 0x50, // F19
        0x6F => 0x5A, // F20
        0x7F => 0x4A, // Mute
        0x80 => 0x48, // Volume Up
        0x81 => 0x49, // Volume Down
        0x85 => 0x5F, // Keypad, (JIS)
        0x87 => 0x5E, // International1: Ro (JIS _)
        0x88 => 0x68, // International2: Katakana/Hiragana → Kana
        0x89 => 0x5D, // International3: Yen
        0x8A => 0x68, // International4: Henkan → Kana
        0x8B => 0x66, // International5: Muhenkan → Eisu
        0x90 => 0x68, // LANG1: Kana (Hangul)
        0x91 => 0x66, // LANG2: Eisu (Hanja)
        0xE0 => 0x3B, // Left Control
        0xE1 => 0x38, // Left Shift
        0xE2 => 0x3A, // Left Option
        0xE3 => 0x37, // Left Command
        0xE4 => 0x3E, // Right Control
        0xE5 => 0x3C, // Right Shift
        0xE6 => 0x3D, // Right Option
        0xE7 => 0x36, // Right Command
        remote_core::msg::USAGE_GLOBE => 0x3F, // fn
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_key_code_spot_checks() {
        assert_eq!(hid_to_mac(0x04), Some(0x00)); // A
        assert_eq!(hid_to_mac(0x1D), Some(0x06)); // Z
        assert_eq!(hid_to_mac(0x1E), Some(0x12)); // 1
        assert_eq!(hid_to_mac(0x27), Some(0x1D)); // 0
        assert_eq!(hid_to_mac(0x28), Some(0x24)); // Return
        assert_eq!(hid_to_mac(0x58), Some(0x4C)); // Keypad Enter
        assert_eq!(hid_to_mac(0x3A), Some(0x7A)); // F1
        assert_eq!(hid_to_mac(0x50), Some(0x7B)); // Left
        assert_eq!(hid_to_mac(0xE3), Some(0x37)); // Command
        assert_eq!(hid_to_mac(0xE6), Some(0x3D)); // Right Option
        assert_eq!(hid_to_mac(0x64), Some(0x0A)); // ISO §
        assert_eq!(hid_to_mac(0x46), Some(0x69)); // Print Screen as F13
        assert_eq!(hid_to_mac(remote_core::msg::USAGE_GLOBE), Some(0x3F));
        assert_eq!(hid_to_mac(0x66), None); // Power: not a key on the Mac
    }

    #[test]
    fn spot_checks_against_the_pc_at_set_1_table() {
        let scan = |u| match hid_to_win(u) {
            Some(WinKey::Scan { code, extended }) => (code, extended),
            other => panic!("{u:#x}: {other:?}"),
        };
        assert_eq!(scan(0x04), (0x1E, false)); // A
        assert_eq!(scan(0x1D), (0x2C, false)); // Z
        assert_eq!(scan(0x1E), (0x02, false)); // 1
        assert_eq!(scan(0x27), (0x0B, false)); // 0
        assert_eq!(scan(0x28), (0x1C, false)); // Enter
        assert_eq!(scan(0x58), (0x1C, true)); // Keypad Enter: the same code, extended
        assert_eq!(scan(0x3A), (0x3B, false)); // F1
        assert_eq!(scan(0x45), (0x58, false)); // F12
        assert_eq!(scan(0x50), (0x4B, true)); // Left arrow is extended…
        assert_eq!(scan(0x5C), (0x4B, false)); // …keypad 4 isn't
        assert_eq!(scan(0x4C), (0x53, true)); // Delete
        assert_eq!(scan(0xE4), (0x1D, true)); // Right Ctrl
        assert_eq!(scan(0xE6), (0x38, true)); // AltGr
        assert_eq!(scan(0xE3), (0x5B, true)); // Win
        assert_eq!(scan(0x64), (0x56, false)); // ISO <>
        assert_eq!(scan(0x87), (0x73, false)); // Ro
        assert_eq!(scan(0x73), (0x76, false)); // F24
        assert!(matches!(hid_to_win(0x48), Some(WinKey::Vk { vk: VK_PAUSE, .. })));
        assert!(matches!(hid_to_win(0x53), Some(WinKey::Vk { vk: VK_NUMLOCK, extended: true, .. })));
        assert_eq!(hid_to_win(remote_core::msg::USAGE_GLOBE), None);
    }
}

/// HID usage id (page 7) → evdev KEY_* code; 0 where there is none: the keyboard page of
/// the kernel's table (`hid_keyboard[]` in drivers/hid/hid-input.c), as client-linux has it.
const HID_TO_EVDEV: [u16; 0x95] = [
    0, 0, 0, 0, 30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, // 0x00
    50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44, 2, 3, // 0x10
    4, 5, 6, 7, 8, 9, 10, 11, 28, 1, 14, 15, 57, 12, 13, 26, // 0x20
    27, 43, 43, 39, 40, 41, 51, 52, 53, 58, 59, 60, 61, 62, 63, 64, // 0x30
    65, 66, 67, 68, 87, 88, 99, 70, 119, 110, 102, 104, 111, 107, 109, 106, // 0x40
    105, 108, 103, 69, 98, 55, 74, 78, 96, 79, 80, 81, 75, 76, 77, 71, // 0x50
    72, 73, 82, 83, 86, 127, 116, 117, 183, 184, 185, 186, 187, 188, 189, 190, // 0x60
    191, 192, 193, 194, 134, 138, 130, 132, 128, 129, 131, 137, 133, 135, 136, 113, // 0x70
    115, 114, 0, 0, 0, 121, 0, 89, 93, 124, 92, 94, 95, 0, 0, 0, // 0x80
    122, 123, 90, 91, 85, // 0x90
];
/// Modifiers, HID 0xE0..=0xE7: LCtrl LShift LAlt LMeta RCtrl RShift RAlt RMeta.
const HID_MODS_TO_EVDEV: [u16; 8] = [29, 42, 56, 125, 97, 54, 100, 126];

/// The evdev keycode for a keyboard-page usage id.
pub fn hid_to_evdev(usage: u16) -> Option<u32> {
    let id = usage as usize;
    let code = match id {
        0xE0..=0xE7 => HID_MODS_TO_EVDEV[id - 0xE0],
        _ => *HID_TO_EVDEV.get(id)?,
    };
    (code != 0).then_some(code as u32)
}
