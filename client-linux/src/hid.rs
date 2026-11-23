// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Keys travel as USB HID usages on the keyboard page, 0x07 (
//! `msg::InputEvent::Key`); Wayland gives the client evdev keycodes. This is
//! the keyboard page of the kernel's HID → evdev table (`hid_keyboard[]` in
//! drivers/hid/hid-input.c), both ways.

/// HID usage id (page 7) → evdev KEY_* code; 0 where there is none.
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

/// The keyboard-page usage id for an evdev keycode.
pub fn evdev_to_hid(code: u32) -> Option<u16> {
    if code == 0 {
        return None;
    }
    if let Some(i) = HID_MODS_TO_EVDEV.iter().position(|&e| e as u32 == code) {
        return Some(0xE0 + i as u16);
    }
    HID_TO_EVDEV.iter().position(|&e| e as u32 == code).map(|i| i as u16)
}

/// The evdev keycode for a keyboard-page usage id.
pub fn hid_to_evdev(usage: u16) -> Option<u32> {
    let id = usage as usize;
    let code = match id {
        0xE0..=0xE7 => HID_MODS_TO_EVDEV[id - 0xE0],
        _ => *HID_TO_EVDEV.get(id)?,
    };
    (code != 0).then_some(code as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_keys_both_ways() {
        // KEY_A 30 = 0x04, KEY_ENTER 28 = 0x28, KEY_ESC 1 = 0x29, KEY_UP 103 = 0x52,
        // KEY_F12 88 = 0x45, KEY_LEFTCTRL 29 = 0xE0, KEY_RIGHTMETA 126 = 0xE7.
        for (evdev, hid) in [(30, 0x04), (28, 0x28), (1, 0x29), (103, 0x52), (88, 0x45), (29, 0xE0), (126, 0xE7)] {
            assert_eq!(evdev_to_hid(evdev), Some(hid));
            assert_eq!(hid_to_evdev(hid), Some(evdev));
        }
        assert_eq!(evdev_to_hid(0), None);
        assert_eq!(hid_to_evdev(0x03), None); // ErrorUndefined
    }
}
