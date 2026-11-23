// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Client input → the plugin's INPUT_* messages (capture-ipc.md 64–70).
//!
//! The client's events (remote-core's `msg::InputEvent`) become the media module's
//! `media::InputEvent`, which `Media::input` writes on the one plugin connection
//! media owns. remoted's part:
//! - **The key table**: the client sends keys exactly as
//!   pressed (Cmd is GUI, Globe is `msg::USAGE_GLOBE`) plus its table
//!   (`Msg::KeyTable`); [`KeyMapper`] applies it here, using the focused window's
//!   class from Hyprland's event socket ([`FocusWatch`]), and juggles the
//!   modifiers itself: Cmd held is Ctrl down; Cmd+← lifts Ctrl, presses Home;
//!   Cmd+C in a terminal adds Shift around C. An empty table is PC passthrough
//!   (Globe with no rule is dropped). The plugin is unchanged.
//! - keys are USB HID usages (keyboard page 0x07) and become evdev keycodes, as
//!   the kernel maps a USB keyboard; the host keeps its own xkb layout;
//! - named buttons become evdev BTN_* codes, scroll and gesture enums their numbers;
//! - `ReleaseAll`, a lost connection and the end of a session send INPUT_RESET
//!   (the plugin also runs it when the socket closes), so a dropped client can't
//!   leave Ctrl held down.
//!
//! The client's IME events (preedit, commit) go to the IME bridge, not here.

use std::{
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use remote_core::{
    keymap,
    msg::{self, InputEvent, KeyRule},
};

pub use crate::media::InputEvent as PluginInput;

/// Where INPUT_* messages go: the media module's plugin connection (`Media::input`).
pub trait PluginLink: Send + Sync {
    fn input(&self, ev: PluginInput) -> Result<()>;
    /// capture-ipc v4 LOCAL_INPUT: 0 normal, 1 the desk's input ignored, 2 the same
    /// with desk keys going to the privacy screen.
    fn local_input(&self, _mode: u32) {}
    /// The session ended: stop the stream (before REMOTE-1 goes).
    fn end(&self) {}
}

/// Releases everything the client held (on disconnect and on a lost connection).
pub fn reset(link: &dyn PluginLink) -> Result<()> {
    link.input(PluginInput::Reset)
}

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;
pub const BTN_SIDE: u32 = 0x113;
pub const BTN_EXTRA: u32 = 0x114;

/// The terminals `KeyCondition::Terminal` means: kitty (also
/// SodiumOS's terminal), foot, Alacritty, wezterm and ghostty.
pub const TERMINAL_CLASSES: &[&str] =
    &["kitty", "foot", "footclient", "Alacritty", "alacritty", "org.wezfurlong.wezterm", "com.mitchellh.ghostty", "ghostty"];

/// Applies the client's key table: remote-core's `keymap::KeyMapper` does the
/// evaluation (rules, modifiers, the focused app); this turns its HID usages
/// into evdev keycodes for the plugin.
#[derive(Default)]
pub struct KeyMapper(keymap::KeyMapper);

impl KeyMapper {
    pub fn set_table(&mut self, table: Vec<KeyRule>) {
        self.0.set_table(table);
    }

    /// Forget every held key (after INPUT_RESET).
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// One key from the client → plugin messages, with `class` the focused app.
    pub fn key(&mut self, usage: u16, down: bool, t_ns: u64, class: &str, out: &mut Vec<PluginInput>) {
        let app = keymap::ForegroundApp { id: class, terminal: TERMINAL_CLASSES.contains(&class) };
        let mut keys = Vec::with_capacity(2);
        self.0.key(usage, down, app, &mut keys);
        for k in keys {
            if let Some(code) = hid_to_evdev(k.usage) {
                out.push(PluginInput::Key { keycode: code as u32, pressed: k.pressed, t_ns });
            }
        }
    }
}

/// A client event → plugin messages. Keys go through `keys`; IME events give
/// nothing here.
pub fn to_plugin(ev: &InputEvent, t_ns: u64, keys: &mut KeyMapper, class: &str) -> Vec<PluginInput> {
    let mut out = Vec::with_capacity(2);
    match *ev {
        InputEvent::PointerAbs { x, y } => out.push(PluginInput::PointerAbs { x, y, t_ns }),
        InputEvent::PointerRel { dx, dy, dx_unaccel, dy_unaccel } => {
            out.push(PluginInput::PointerRel { dx, dy, dx_unaccel, dy_unaccel, t_ns })
        }
        InputEvent::Button { button, down } => {
            let button = match button {
                msg::Button::Left => BTN_LEFT,
                msg::Button::Right => BTN_RIGHT,
                msg::Button::Middle => BTN_MIDDLE,
                // Mac buttons 3 and 4 are Back and Forward: libinput's BTN_SIDE and BTN_EXTRA.
                msg::Button::Back => BTN_SIDE,
                msg::Button::Forward => BTN_EXTRA,
                msg::Button::Other(code) => code,
            };
            out.push(PluginInput::Button { button, pressed: down, t_ns })
        }
        InputEvent::Axis { source, axis, value, value120, stop, more } => out.push(PluginInput::Axis {
            source: source as u32,
            axis: axis as u32,
            value,
            value120,
            stop,
            more,
            t_ns,
        }),
        InputEvent::Key { usage, down } => keys.key(usage, down, t_ns, class, &mut out),
        InputEvent::Gesture { kind, phase, fingers, dx, dy, scale, rotation } => out.push(PluginInput::Gesture {
            kind: kind as u32 + 1,
            phase: phase as u32,
            fingers,
            dx,
            dy,
            scale,
            rotation,
            t_ns,
        }),
        InputEvent::ReleaseAll => {
            keys.clear();
            out.push(PluginInput::Reset)
        }
        InputEvent::ImePreedit { .. } | InputEvent::ImeCommit { .. } => {}
    }
    out
}

/// The focused window's class, from Hyprland's event socket (`activewindowv2`,
/// then the client list for its class), for the key table's conditions.
#[derive(Clone, Default)]
pub struct FocusWatch(Arc<Mutex<String>>);

impl FocusWatch {
    pub fn class(&self) -> String {
        self.0.lock().unwrap().clone()
    }

    /// Follows focus on `hypr`'s event socket on a thread of its own, for as
    /// long as the compositor runs.
    pub fn start(hypr: crate::display::hypr::Hypr) -> FocusWatch {
        let w = FocusWatch::default();
        let me = w.clone();
        let dir = crate::xdg_runtime_dir();
        let path = PathBuf::from(dir).join("hypr").join(&hypr.signature).join(".socket2.sock");
        let class_of = move |addr: &str| -> Option<String> {
            let clients = hypr.json("clients").ok()?;
            let addr = if addr.starts_with("0x") { addr.to_string() } else { format!("0x{addr}") };
            clients.as_array()?.iter().find(|c| c["address"].as_str() == Some(addr.as_str()))?["class"].as_str().map(String::from)
        };
        if let Ok(a) = hypr_active(&path) {
            *w.0.lock().unwrap() = a;
        }
        let _ = std::thread::Builder::new().name("focus-watch".into()).spawn(move || {
            let Ok(s) = UnixStream::connect(&path) else { return };
            for line in BufReader::new(s).lines() {
                let Ok(line) = line else { break };
                if let Some(addr) = line.strip_prefix("activewindowv2>>") {
                    let class = if addr.is_empty() || addr == "," { String::new() } else { class_of(addr).unwrap_or_default() };
                    *me.0.lock().unwrap() = class;
                }
            }
        });
        w
    }
}

fn hypr_active(socket2: &std::path::Path) -> Result<String> {
    let sig = socket2.parent().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let v = crate::display::hypr::Hypr::with_signature(&sig).json("activewindow")?;
    Ok(v["class"].as_str().unwrap_or_default().to_string())
}

const U: u8 = 0;

/// USB HID keyboard page (0x07) → evdev keycode, as the kernel's `hid_keyboard`
/// table in drivers/hid/hid-input.c maps a real USB keyboard.
#[rustfmt::skip]
static HID_KEYBOARD: [u8; 256] = [
      0,  0,  0,  0, 30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38,
     50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44,  2,  3,
      4,  5,  6,  7,  8,  9, 10, 11, 28,  1, 14, 15, 57, 12, 13, 26,
     27, 43, 43, 39, 40, 41, 51, 52, 53, 58, 59, 60, 61, 62, 63, 64,
     65, 66, 67, 68, 87, 88, 99, 70,119,110,102,104,111,107,109,106,
    105,108,103, 69, 98, 55, 74, 78, 96, 79, 80, 81, 75, 76, 77, 71,
     72, 73, 82, 83, 86,127,116,117,183,184,185,186,187,188,189,190,
    191,192,193,194,134,138,130,132,128,129,131,137,133,135,136,113,
    115,114,  U,  U,  U,121,  U, 89, 93,124, 92, 94, 95,  U,  U,  U,
    122,123, 90, 91, 85,  U,  U,  U,  U,  U,  U,  U,111,  U,  U,  U,
      U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,
      U,  U,  U,  U,  U,  U,179,180,  U,  U,  U,  U,  U,  U,  U,  U,
      U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,  U,
      U,  U,  U,  U,  U,  U,  U,  U,111,  U,  U,  U,  U,  U,  U,  U,
     29, 42, 56,125, 97, 54,100,126,164,166,165,163,161,115,114,113,
    150,158,159,128,136,177,178,176,142,152,173,140,  U,  U,  U,  U,
];

pub fn hid_to_evdev(usage: u16) -> Option<u16> {
    let code = *HID_KEYBOARD.get(usage as usize)?;
    (code != 0).then_some(code as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use msg::{USAGE_GLOBE, usage::*};

    const KEY_LEFTCTRL: u32 = 29;
    const KEY_LEFTSHIFT: u32 = 42;
    const KEY_LEFTALT: u32 = 56;
    const KEY_LEFTMETA: u32 = 125;

    fn keys(m: &mut KeyMapper, seq: &[(u16, bool)], class: &str) -> Vec<(u32, bool)> {
        let mut out = vec![];
        for &(u, d) in seq {
            m.key(u, d, 0, class, &mut out);
        }
        out.into_iter()
            .map(|e| match e {
                PluginInput::Key { keycode, pressed, .. } => (keycode, pressed),
                e => panic!("{e:?}"),
            })
            .collect()
    }

    const KEY_C: u32 = 46;
    const KEY_HOME: u32 = 102;
    const KEY_TAB: u32 = 15;
    const KEY_BACKSPACE: u32 = 14;

    #[test]
    fn the_mac_table_juggles_modifiers_by_focus() {
        let mut m = KeyMapper::default();
        m.set_table(KeyRule::mac_defaults());
        // Cmd+C: Ctrl+C in an editor; Ctrl+Shift+C in kitty.
        let editor = keys(&mut m, &[(LEFT_GUI, true), (C, true), (C, false), (LEFT_GUI, false)], "org.gnome.TextEditor");
        assert_eq!(editor, [(KEY_LEFTCTRL, true), (KEY_C, true), (KEY_C, false), (KEY_LEFTCTRL, false)]);
        let term = keys(&mut m, &[(LEFT_GUI, true), (C, true), (C, false), (LEFT_GUI, false)], "kitty");
        assert_eq!(term, [(KEY_LEFTCTRL, true), (KEY_LEFTSHIFT, true), (KEY_C, true), (KEY_C, false), (KEY_LEFTCTRL, false), (KEY_LEFTSHIFT, false)]);
        // Cmd+Shift+←: Ctrl lifts, Shift carries over, Home.
        let sel = keys(&mut m, &[(LEFT_GUI, true), (LEFT_SHIFT, true), (LEFT, true), (LEFT, false)], "x");
        assert_eq!(sel, [(KEY_LEFTCTRL, true), (KEY_LEFTSHIFT, true), (KEY_LEFTCTRL, false), (102, true), (102, false)]);
        m.clear();
        // Cmd+Tab, Tab: Alt stays down between them; Cmd up lets it go.
        let tab = keys(&mut m, &[(LEFT_GUI, true), (TAB, true), (TAB, false), (TAB, true), (TAB, false), (LEFT_GUI, false)], "x");
        assert_eq!(
            tab,
            [(KEY_LEFTCTRL, true), (KEY_LEFTCTRL, false), (KEY_LEFTALT, true), (KEY_TAB, true), (KEY_TAB, false), (KEY_TAB, true), (KEY_TAB, false), (KEY_LEFTALT, false)]
        );
        // Cmd+Backspace: Shift+Home tapped, then Backspace held.
        let del = keys(&mut m, &[(LEFT_GUI, true), (BACKSPACE, true), (BACKSPACE, false), (LEFT_GUI, false)], "x");
        assert_eq!(
            del,
            [
                (KEY_LEFTCTRL, true),
                (KEY_LEFTCTRL, false),
                (KEY_LEFTSHIFT, true),
                (KEY_HOME, true),
                (KEY_HOME, false),
                (KEY_LEFTSHIFT, false),
                (KEY_BACKSPACE, true),
                (KEY_BACKSPACE, false),
            ]
        );
    }

    #[test]
    fn an_empty_table_passes_keys_through_and_drops_globe() {
        let mut m = KeyMapper::default();
        let out = keys(&mut m, &[(LEFT_GUI, true), (USAGE_GLOBE, true), (C, true)], "kitty");
        assert_eq!(out, [(KEY_LEFTMETA, true), (KEY_C, true)]);
        assert_eq!(hid_to_evdev(0x28), Some(28)); // Return → KEY_ENTER
        assert_eq!(hid_to_evdev(0x00), None);
    }
}
