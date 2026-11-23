// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
import Carbon

/// macOS secure keyboard entry while a password field on the computer has
/// focus: other apps can't read the keys. Balanced: on/off pairs.
/// (Its own file: Carbon's old QuickDraw names would clash with the core's.)
enum SecureInput {
    private static var enabled = false

    static func set(_ on: Bool) {
        if on == enabled { return }
        enabled = on
        if on {
            EnableSecureEventInput()
        } else {
            DisableSecureEventInput()
        }
    }
}
