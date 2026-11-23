// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Keeping the Mac awake while it is controlled: an IOKit power
//! assertion that stops idle display sleep (and so idle system sleep) for as long as a
//! session lives. `pmset -g assertions` lists it as "Sodium Remote session".

use objc2_core_foundation::CFString;
use objc2_io_kit::{IOPMAssertionCreateWithName, IOPMAssertionID, IOPMAssertionRelease, kIOPMAssertionLevelOn};

/// kIOPMAssertionTypePreventUserIdleDisplaySleep (a string macro in IOPMLib.h).
const PREVENT_IDLE_DISPLAY_SLEEP: &str = "PreventUserIdleDisplaySleep";

pub struct DisplayAwake(IOPMAssertionID);

impl DisplayAwake {
    pub fn new() -> Option<DisplayAwake> {
        let kind = CFString::from_static_str(PREVENT_IDLE_DISPLAY_SLEEP);
        let name = CFString::from_static_str("Sodium Remote session");
        let mut id: IOPMAssertionID = 0;
        let r = unsafe { IOPMAssertionCreateWithName(Some(&kind), kIOPMAssertionLevelOn, Some(&name), &mut id) };
        if r != 0 {
            eprintln!("sodium-remote-host: no power assertion (IOReturn {r:#x}): the display may sleep during a session");
            return None;
        }
        Some(DisplayAwake(id))
    }
}

impl Drop for DisplayAwake {
    fn drop(&mut self) {
        IOPMAssertionRelease(self.0);
    }
}
