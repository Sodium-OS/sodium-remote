// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Keys travel as USB HID usages on the keyboard page, 0x07 (`msg::InputEvent::Key`).
//! Windows gives the client hardware scan codes (set 1) with an "extended"
//! flag (the E0 prefix), in `WM_KEYDOWN`'s lParam (bits 16-23, bit 24) and in
//! the low-level hook's `scanCode` and `LLKHF_EXTENDED`. Layout independent, so
//! the host's own layout decides what a key types.

/// Set-1 scan codes without the E0 prefix: index is the scan code.
const PLAIN: [u8; 0x80] = {
    let mut t = [0u8; 0x80];
    t[0x01] = 0x29; // Esc
    // 1 2 3 4 5 6 7 8 9 0
    let mut i = 0;
    while i < 10 {
        t[0x02 + i] = 0x1E + i as u8;
        i += 1;
    }
    t[0x0B] = 0x27; // 0 is the last of the row on a PC keyboard, the first digit key after 9 in HID
    t[0x0C] = 0x2D; // -
    t[0x0D] = 0x2E; // =
    t[0x0E] = 0x2A; // Backspace
    t[0x0F] = 0x2B; // Tab
    // q w e r t y u i o p
    let row1 = [0x14, 0x1A, 0x08, 0x15, 0x17, 0x1C, 0x18, 0x0C, 0x12, 0x13];
    let mut i = 0;
    while i < 10 {
        t[0x10 + i] = row1[i];
        i += 1;
    }
    t[0x1A] = 0x2F; // [
    t[0x1B] = 0x30; // ]
    t[0x1C] = 0x28; // Enter
    t[0x1D] = 0xE0; // Left Ctrl
    // a s d f g h j k l
    let row2 = [0x04, 0x16, 0x07, 0x09, 0x0A, 0x0B, 0x0D, 0x0E, 0x0F];
    let mut i = 0;
    while i < 9 {
        t[0x1E + i] = row2[i];
        i += 1;
    }
    t[0x27] = 0x33; //;
    t[0x28] = 0x34; // '
    t[0x29] = 0x35; // `
    t[0x2A] = 0xE1; // Left Shift
    t[0x2B] = 0x31; // backslash
    // z x c v b n m
    let row3 = [0x1D, 0x1B, 0x06, 0x19, 0x05, 0x11, 0x10];
    let mut i = 0;
    while i < 7 {
        t[0x2C + i] = row3[i];
        i += 1;
    }
    t[0x33] = 0x36; //,
    t[0x34] = 0x37; // .
    t[0x35] = 0x38; // /
    t[0x36] = 0xE5; // Right Shift
    t[0x37] = 0x55; // keypad *
    t[0x38] = 0xE2; // Left Alt
    t[0x39] = 0x2C; // Space
    t[0x3A] = 0x39; // Caps Lock
    // F1..F10
    let mut i = 0;
    while i < 10 {
        t[0x3B + i] = 0x3A + i as u8;
        i += 1;
    }
    t[0x45] = 0x48; // Pause (NumLock is the extended 0x45)
    t[0x46] = 0x47; // Scroll Lock
    // keypad 7 8 9 - 4 5 6 + 1 2 3 0 .
    let kp = [0x5F, 0x60, 0x61, 0x56, 0x5C, 0x5D, 0x5E, 0x57, 0x59, 0x5A, 0x5B, 0x62, 0x63];
    let mut i = 0;
    while i < 13 {
        t[0x47 + i] = kp[i];
        i += 1;
    }
    t[0x54] = 0x46; // Alt+PrintScreen (SysRq)
    t[0x56] = 0x64; // the extra key left of Z on ISO keyboards
    t[0x57] = 0x44; // F11
    t[0x58] = 0x45; // F12
    // F13..F23
    let mut i = 0;
    while i < 11 {
        t[0x64 + i] = 0x68 + i as u8;
        i += 1;
    }
    t[0x70] = 0x88; // Japanese kana
    t[0x71] = 0x91; // Korean Hanja
    t[0x72] = 0x90; // Korean Hangul
    t[0x73] = 0x87; // Japanese ro (international 1)
    t[0x76] = 0x73; // F24
    t[0x79] = 0x8A; // Japanese henkan
    t[0x7B] = 0x8B; // Japanese muhenkan
    t[0x7D] = 0x89; // Japanese yen
    t[0x7E] = 0x85; // keypad comma
    t
};

/// (scan code after E0, usage).
const EXTENDED: &[(u8, u8)] = &[
    (0x1C, 0x58), // keypad Enter
    (0x1D, 0xE4), // Right Ctrl
    (0x35, 0x54), // keypad /
    (0x37, 0x46), // Print Screen
    (0x38, 0xE6), // Right Alt (AltGr)
    (0x45, 0x53), // Num Lock
    (0x47, 0x4A), // Home
    (0x48, 0x52), // Up
    (0x49, 0x4B), // Page Up
    (0x4B, 0x50), // Left
    (0x4D, 0x4F), // Right
    (0x4F, 0x4D), // End
    (0x50, 0x51), // Down
    (0x51, 0x4E), // Page Down
    (0x52, 0x49), // Insert
    (0x53, 0x4C), // Delete
    (0x5B, 0xE3), // Left Win
    (0x5C, 0xE7), // Right Win
    (0x5D, 0x65), // Menu (Application)
];

/// The keyboard-page usage for a scan code and its extended flag.
pub fn scancode_to_hid(scan: u32, extended: bool) -> Option<u16> {
    let scan = scan & 0xFF;
    if extended {
        EXTENDED.iter().find(|(s, _)| *s as u32 == scan).map(|&(_, u)| u as u16)
    } else {
        match PLAIN.get(scan as usize) {
            Some(&u) if u != 0 => Some(u as u16),
            _ => None,
        }
    }
}

/// The scan code and extended flag that produces a usage.
#[allow(dead_code)]
pub fn hid_to_scancode(usage: u16) -> Option<(u32, bool)> {
    if let Some(&(s, _)) = EXTENDED.iter().find(|(_, u)| *u as u16 == usage) {
        return Some((s as u32, true));
    }
    PLAIN.iter().position(|&u| u as u16 == usage && u != 0).map(|s| (s as u32, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_keys_both_ways() {
        // (scan, extended, usage): A, 1, 0, Enter, keypad Enter, Right Ctrl, Up, F12, Left Win, Delete.
        for (scan, ext, hid) in [
            (0x1E, false, 0x04),
            (0x02, false, 0x1E),
            (0x0B, false, 0x27),
            (0x1C, false, 0x28),
            (0x1C, true, 0x58),
            (0x1D, true, 0xE4),
            (0x48, true, 0x52),
            (0x58, false, 0x45),
            (0x5B, true, 0xE3),
            (0x53, true, 0x4C),
            (0x39, false, 0x2C),
        ] {
            assert_eq!(scancode_to_hid(scan, ext), Some(hid), "{scan:#x} {ext}");
            assert_eq!(hid_to_scancode(hid), Some((scan, ext)), "{hid:#x}");
        }
        // The keypad's 7 and the cursor block's Home share a scan code and differ in E0.
        assert_eq!(scancode_to_hid(0x47, false), Some(0x5F));
        assert_eq!(scancode_to_hid(0x47, true), Some(0x4A));
        assert_eq!(scancode_to_hid(0x00, false), None);
    }

    #[test]
    fn the_letter_rows_cover_a_to_z_once() {
        let mut seen = std::collections::HashSet::new();
        for scan in 0..0x80 {
            if let Some(u) = scancode_to_hid(scan, false)
                && (0x04..=0x1D).contains(&u)
            {
                assert!(seen.insert(u));
            }
        }
        assert_eq!(seen.len(), 26);
    }
}
