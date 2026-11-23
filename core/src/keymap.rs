// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The key table's evaluation, pure
//! and shared by every host: the client's keys (USB HID usages, exactly as
//! pressed) plus its table (`Msg::KeyTable`) plus the foreground app give the
//! keys to inject on the host, again as HID usages. The host turns those into
//! its own key codes (evdev, Win32 scan codes, CGKeyCode).
//!
//! [`KeyMapper`] juggles the modifiers: Cmd held is Ctrl down; Cmd+← lifts Ctrl
//! and presses Home; Cmd+C in a terminal adds Shift around C. An empty table is
//! PC passthrough (Globe with no rule is dropped).
//!
//! [`default_rules`] gives the table a client starts with for a (client OS, host
//! OS) pair. This module is not `keys.rs`, which is the device-key store.

use std::collections::HashMap;

use crate::msg::{KeyChord, KeyCondition, KeyRule, Os, USAGE_GLOBE, mods, usage};

/// The host's foreground app, for the table's `Terminal` and `Class` conditions.
#[derive(Debug, Clone, Copy, Default)]
pub struct ForegroundApp<'a> {
    /// Wayland app ID / X11 class, process name, or bundle ID: what
    /// `KeyCondition::Class` is compared with.
    pub id: &'a str,
    /// The host's own terminal list says this app is a terminal.
    pub terminal: bool,
}

/// One key to inject, as the host's keyboard sees it: a HID usage (modifiers are
/// the left ones, 0xE0 Ctrl, 0xE1 Shift, 0xE2 Alt, 0xE3 GUI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyOut {
    pub usage: u16,
    pub pressed: bool,
}

// The left modifiers the mapper presses, by logical bit.
const OUT_MODS: [(u8, u16); 4] =
    [(mods::CTRL, usage::LEFT_CTRL), (mods::SHIFT, usage::LEFT_SHIFT), (mods::ALT, usage::LEFT_ALT), (mods::GUI, usage::LEFT_GUI)];

/// The logical modifier of a usage (Left/Right Ctrl, Shift, Alt, GUI, Globe).
fn mod_of(usage: u16) -> Option<u8> {
    match usage {
        0xE0 | 0xE4 => Some(mods::CTRL),
        0xE1 | 0xE5 => Some(mods::SHIFT),
        0xE2 | 0xE6 => Some(mods::ALT),
        0xE3 | 0xE7 => Some(mods::GUI),
        USAGE_GLOBE => Some(mods::GLOBE),
        _ => None,
    }
}

/// Applies the client's key table (see the module docs).
#[derive(Default)]
pub struct KeyMapper {
    table: Vec<KeyRule>,
    /// Physical modifier usages the client holds.
    phys: Vec<u16>,
    /// Logical modifiers we hold down on the host's seat.
    out: u8,
    /// The rule behind the last key, while its `from` modifiers stay held: its
    /// `to` modifiers stay down (Alt held through Cmd+Tab, Tab, Tab).
    active: Option<(u8, u8)>,
    /// Non-modifier keys held: the usage pressed for each.
    held: HashMap<u16, u16>,
}

impl KeyMapper {
    pub fn set_table(&mut self, table: Vec<KeyRule>) {
        self.table = table;
    }

    pub fn passthrough(&self) -> bool {
        self.table.is_empty()
    }

    /// Forget every held key (after a reset).
    pub fn clear(&mut self) {
        self.phys.clear();
        self.out = 0;
        self.active = None;
        self.held.clear();
    }

    /// What a physical modifier sends: its remap rule's modifier, itself, or
    /// nothing (Globe with no rule, or a rule with an empty `to`).
    fn remap(&self, usage: u16) -> u8 {
        let rule = self.table.iter().find(|r| r.when == KeyCondition::Any && r.from.mods == 0 && r.from.key == usage);
        match rule {
            Some(r) => r.to.first().and_then(|c| mod_of(c.key)).unwrap_or(0) & !mods::GLOBE,
            None => mod_of(usage).unwrap_or(0) & !mods::GLOBE,
        }
    }

    fn phys_logical(&self) -> u8 {
        self.phys.iter().filter_map(|&u| mod_of(u)).fold(0, |a, m| a | m)
    }

    fn mapped(&self, logical: u8) -> u8 {
        self.phys.iter().filter(|&&u| mod_of(u).is_some_and(|m| logical & m != 0)).fold(0, |a, &u| a | self.remap(u))
    }

    /// The output modifiers for the physical ones held now.
    fn desired(&self) -> u8 {
        let l = self.phys_logical();
        match self.active {
            Some((from, to)) if l & from == from => to | self.mapped(l & !from),
            _ => self.mapped(l),
        }
    }

    fn sync(&mut self, want: u8, out: &mut Vec<KeyOut>) {
        for (m, key) in OUT_MODS {
            let (have, need) = (self.out & m != 0, want & m != 0);
            if have != need {
                out.push(KeyOut { usage: key, pressed: need });
            }
        }
        self.out = want;
    }

    fn rule_for(&self, usage: u16, logical: u8, app: ForegroundApp) -> Option<&KeyRule> {
        let applies = |c: &KeyCondition| match c {
            KeyCondition::Any => true,
            KeyCondition::Terminal => app.terminal,
            KeyCondition::Class(k) => k == app.id,
        };
        let specificity = |c: &KeyCondition| match c {
            KeyCondition::Any => 0,
            KeyCondition::Terminal => 1,
            KeyCondition::Class(_) => 2,
        };
        // A rule with no modifiers is a modifier's remap (`remap`) when its key is a
        // modifier, and a plain key's rule (Home → Cmd+←) otherwise.
        self.table
            .iter()
            .filter(|r| {
                r.from.key == usage
                    && (r.from.mods != 0 || mod_of(usage).is_none())
                    && logical & r.from.mods == r.from.mods
                    && applies(&r.when)
            })
            .max_by_key(|r| (r.from.mods.count_ones(), specificity(&r.when)))
    }

    /// One key from the client → the keys to inject, with `app` the foreground app.
    pub fn key(&mut self, usage: u16, down: bool, app: ForegroundApp, out: &mut Vec<KeyOut>) {
        if self.passthrough() {
            if usage != USAGE_GLOBE {
                out.push(KeyOut { usage, pressed: down });
            }
            return;
        }
        if mod_of(usage).is_some() {
            if down && !self.phys.contains(&usage) {
                self.phys.push(usage);
            } else if !down {
                self.phys.retain(|&u| u != usage);
            }
            if self.active.is_some_and(|(from, _)| self.phys_logical() & from != from) {
                self.active = None;
            }
            let want = self.desired();
            self.sync(want, out);
            return;
        }
        if !down {
            if let Some(k) = self.held.remove(&usage) {
                out.push(KeyOut { usage: k, pressed: false });
                // A plain key's rule holds its modifiers only while its key is down
                // (Home → Cmd+←: Cmd goes up with Home).
                if self.active.is_some_and(|(from, _)| from == 0) {
                    self.active = None;
                    let want = self.desired();
                    self.sync(want, out);
                }
            }
            return;
        }
        let logical = self.phys_logical();
        match self.rule_for(usage, logical, app).cloned() {
            Some(rule) => {
                let extra = self.mapped(logical & !rule.from.mods);
                let Some((last, taps)) = rule.to.split_last() else { return }; // an empty `to` drops it
                for c in taps {
                    self.sync(c.mods | extra, out);
                    out.push(KeyOut { usage: c.key, pressed: true });
                    out.push(KeyOut { usage: c.key, pressed: false });
                }
                self.sync(last.mods | extra, out);
                self.active = Some((rule.from.mods, last.mods));
                out.push(KeyOut { usage: last.key, pressed: true });
                self.held.insert(usage, last.key);
            }
            None => {
                self.active = None;
                let want = self.mapped(logical);
                self.sync(want, out);
                out.push(KeyOut { usage, pressed: true });
                self.held.insert(usage, usage);
            }
        }
    }
}

/// The key table a client starts with for this pair.
/// Empty means passthrough: the same family (Windows and SodiumOS either way,
/// Mac to Mac).
pub fn default_rules(client_os: Os, host_os: Os) -> Vec<KeyRule> {
    match (client_os, host_os) {
        (Os::Mac, Os::Linux | Os::Windows) => KeyRule::mac_defaults(),
        (Os::Linux | Os::Windows, Os::Mac) => pc_to_mac(),
        _ => vec![],
    }
}

/// Windows or Linux keyboard on a Mac host: Ctrl acts as Cmd, Alt as Option, Win
/// as Control, Alt+Tab is Cmd+Tab; the physical Ctrl stays Control in terminals.
/// With: in terminals Ctrl+Shift+C/V are Cmd+C/V;
/// outside them the PC's editing keys do what they do on a PC (Ctrl+←/→ by word as
/// Option+←/→, Home/End to the line's ends as Cmd+←/→, Ctrl+Home/End to the
/// document's as Cmd+↑/↓), while terminals keep those keys as they are.
fn pc_to_mac() -> Vec<KeyRule> {
    use KeyCondition::{Any, Terminal};
    use usage::*;
    let rule = |when, from: (u8, u16), to: &[(u8, u16)]| KeyRule {
        when,
        from: KeyChord { mods: from.0, key: from.1 },
        to: to.iter().map(|&(mods, key)| KeyChord { mods, key }).collect(),
    };
    // On the Mac host, GUI is Cmd and ALT is Option.
    let mut r = vec![
        rule(Any, (0, LEFT_CTRL), &[(0, LEFT_GUI)]),
        rule(Any, (0, RIGHT_CTRL), &[(0, LEFT_GUI)]),
        rule(Any, (0, LEFT_ALT), &[(0, LEFT_ALT)]),
        rule(Any, (0, RIGHT_ALT), &[(0, LEFT_ALT)]),
        rule(Any, (0, LEFT_GUI), &[(0, LEFT_CTRL)]),
        rule(Any, (0, RIGHT_GUI), &[(0, LEFT_CTRL)]),
        rule(Any, (mods::ALT, TAB), &[(mods::GUI, TAB)]),
    ];
    // Terminals keep Ctrl as Control: Ctrl+<key> goes through as Control+<key>
    // (the letters, Space, Backspace, the brackets and backslash).
    for k in (0x04..=0x1D).chain([SPACE, BACKSPACE, 0x2F, 0x30, 0x31]) {
        r.push(rule(Terminal, (mods::CTRL, k), &[(mods::CTRL, k)]));
    }
    // Copy and paste in a terminal: a PC terminal's Ctrl+Shift+C/V is the Mac's Cmd+C/V.
    for k in [C, V] {
        r.push(rule(Terminal, (mods::CTRL | mods::SHIFT, k), &[(mods::GUI, k)]));
    }
    // The PC's editing keys, outside terminals. Shift carries over (Shift+Home selects
    // to the line's start). Terminals get the keys themselves: Home and End as they
    // are, Ctrl+arrows and Ctrl+Home/End as Control+… (Ctrl stays Control there).
    for (from, to) in [
        ((mods::CTRL, LEFT), (mods::ALT, LEFT)),
        ((mods::CTRL, RIGHT), (mods::ALT, RIGHT)),
        ((0, HOME), (mods::GUI, LEFT)),
        ((0, END), (mods::GUI, RIGHT)),
        ((mods::CTRL, HOME), (mods::GUI, UP)),
        ((mods::CTRL, END), (mods::GUI, DOWN)),
    ] {
        r.push(rule(Any, from, &[to]));
        r.push(rule(Terminal, from, &[from]));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use usage::*;

    const APP: ForegroundApp = ForegroundApp { id: "x", terminal: false };
    const TERM: ForegroundApp = ForegroundApp { id: "kitty", terminal: true };

    fn keys(m: &mut KeyMapper, seq: &[(u16, bool)], app: ForegroundApp) -> Vec<(u16, bool)> {
        let mut out = vec![];
        for &(u, d) in seq {
            m.key(u, d, app, &mut out);
        }
        out.into_iter().map(|e| (e.usage, e.pressed)).collect()
    }

    #[test]
    fn the_mac_table_juggles_modifiers_by_focus() {
        let mut m = KeyMapper::default();
        m.set_table(default_rules(Os::Mac, Os::Linux));
        // Cmd+C: Ctrl+C in an editor; Ctrl+Shift+C in a terminal.
        let editor = keys(&mut m, &[(LEFT_GUI, true), (C, true), (C, false), (LEFT_GUI, false)], APP);
        assert_eq!(editor, [(LEFT_CTRL, true), (C, true), (C, false), (LEFT_CTRL, false)]);
        let term = keys(&mut m, &[(LEFT_GUI, true), (C, true), (C, false), (LEFT_GUI, false)], TERM);
        assert_eq!(term, [(LEFT_CTRL, true), (LEFT_SHIFT, true), (C, true), (C, false), (LEFT_CTRL, false), (LEFT_SHIFT, false)]);
        // Cmd+Shift+←: Ctrl lifts, Shift carries over, Home.
        let sel = keys(&mut m, &[(LEFT_GUI, true), (LEFT_SHIFT, true), (LEFT, true), (LEFT, false)], APP);
        assert_eq!(sel, [(LEFT_CTRL, true), (LEFT_SHIFT, true), (LEFT_CTRL, false), (HOME, true), (HOME, false)]);
        m.clear();
        // Cmd+Tab, Tab: Alt stays down between them; Cmd up lets it go.
        let tab = keys(&mut m, &[(LEFT_GUI, true), (TAB, true), (TAB, false), (TAB, true), (TAB, false), (LEFT_GUI, false)], APP);
        assert_eq!(tab, [(LEFT_CTRL, true), (LEFT_CTRL, false), (LEFT_ALT, true), (TAB, true), (TAB, false), (TAB, true), (TAB, false), (LEFT_ALT, false)]);
        // Cmd+Backspace: Shift+Home tapped, then Backspace held.
        let del = keys(&mut m, &[(LEFT_GUI, true), (BACKSPACE, true), (BACKSPACE, false), (LEFT_GUI, false)], APP);
        assert_eq!(
            del,
            [(LEFT_CTRL, true), (LEFT_CTRL, false), (LEFT_SHIFT, true), (HOME, true), (HOME, false), (LEFT_SHIFT, false), (BACKSPACE, true), (BACKSPACE, false)]
        );
    }

    fn pc_on_mac() -> KeyMapper {
        let mut m = KeyMapper::default();
        m.set_table(default_rules(Os::Windows, Os::Mac));
        m
    }

    #[test]
    fn pc_on_mac_terminal_copy_and_paste_are_cmd_c_and_v() {
        let mut m = pc_on_mac();
        for k in [C, V] {
            let seq = [(LEFT_CTRL, true), (LEFT_SHIFT, true), (k, true), (k, false), (LEFT_SHIFT, false), (LEFT_CTRL, false)];
            // Ctrl lands as Cmd, Shift joins it, and the rule lifts Shift for the key.
            assert_eq!(keys(&mut m, &seq, TERM), [(LEFT_GUI, true), (LEFT_SHIFT, true), (LEFT_SHIFT, false), (k, true), (k, false), (LEFT_GUI, false)]);
        }
        // Outside a terminal Ctrl+Shift+C stays Cmd+Shift+C.
        let out = keys(&mut m, &[(LEFT_CTRL, true), (LEFT_SHIFT, true), (C, true)], APP);
        assert_eq!(out, [(LEFT_GUI, true), (LEFT_SHIFT, true), (C, true)]);
    }

    #[test]
    fn pc_on_mac_ctrl_arrows_move_by_word_outside_terminals() {
        let mut m = pc_on_mac();
        let out = keys(&mut m, &[(LEFT_CTRL, true), (LEFT, true), (LEFT, false), (LEFT_CTRL, false)], APP);
        assert_eq!(out, [(LEFT_GUI, true), (LEFT_ALT, true), (LEFT_GUI, false), (LEFT, true), (LEFT, false), (LEFT_ALT, false)]);
        // Ctrl+Shift+→ selects by word.
        let out = keys(&mut m, &[(LEFT_CTRL, true), (LEFT_SHIFT, true), (RIGHT, true)], APP);
        assert_eq!(out, [(LEFT_GUI, true), (LEFT_SHIFT, true), (LEFT_ALT, true), (LEFT_GUI, false), (RIGHT, true)]);
        m.clear();
        // In a terminal it stays Control+←.
        let out = keys(&mut m, &[(LEFT_CTRL, true), (LEFT, true), (LEFT, false), (LEFT_CTRL, false)], TERM);
        assert_eq!(out, [(LEFT_GUI, true), (LEFT_CTRL, true), (LEFT_GUI, false), (LEFT, true), (LEFT, false), (LEFT_CTRL, false)]);
    }

    #[test]
    fn pc_on_mac_home_and_end_go_to_the_line_ends_outside_terminals() {
        let mut m = pc_on_mac();
        // Cmd is held only while Home is.
        let out = keys(&mut m, &[(HOME, true), (HOME, false), (END, true), (END, false)], APP);
        assert_eq!(
            out,
            [(LEFT_GUI, true), (LEFT, true), (LEFT, false), (LEFT_GUI, false), (LEFT_GUI, true), (RIGHT, true), (RIGHT, false), (LEFT_GUI, false)]
        );
        // Shift+Home selects to the start; Shift stays down after.
        let out = keys(&mut m, &[(LEFT_SHIFT, true), (HOME, true), (HOME, false)], APP);
        assert_eq!(out, [(LEFT_SHIFT, true), (LEFT_GUI, true), (LEFT, true), (LEFT, false), (LEFT_GUI, false)]);
        m.clear();
        // A terminal gets Home itself.
        assert_eq!(keys(&mut m, &[(HOME, true), (HOME, false)], TERM), [(HOME, true), (HOME, false)]);
    }

    #[test]
    fn pc_on_mac_ctrl_home_and_end_go_to_the_document_ends() {
        let mut m = pc_on_mac();
        let out = keys(&mut m, &[(LEFT_CTRL, true), (HOME, true), (HOME, false), (END, true), (END, false), (LEFT_CTRL, false)], APP);
        // Ctrl is already Cmd: Cmd+↑, then Cmd+↓, with Cmd held throughout.
        assert_eq!(out, [(LEFT_GUI, true), (UP, true), (UP, false), (DOWN, true), (DOWN, false), (LEFT_GUI, false)]);
    }

    #[test]
    fn an_empty_table_passes_keys_through_and_drops_globe() {
        let mut m = KeyMapper::default();
        let out = keys(&mut m, &[(LEFT_GUI, true), (USAGE_GLOBE, true), (C, true)], TERM);
        assert_eq!(out, [(LEFT_GUI, true), (C, true)]);
    }

    #[test]
    fn defaults_per_os_pair() {
        // Mac to SodiumOS or Windows: the Mac table, unchanged.
        assert_eq!(default_rules(Os::Mac, Os::Linux), KeyRule::mac_defaults());
        assert_eq!(default_rules(Os::Mac, Os::Windows), KeyRule::mac_defaults());
        // Same family: passthrough.
        for (c, h) in [(Os::Windows, Os::Linux), (Os::Linux, Os::Windows), (Os::Linux, Os::Linux), (Os::Windows, Os::Windows), (Os::Mac, Os::Mac)] {
            assert!(default_rules(c, h).is_empty(), "{c:?} -> {h:?}");
        }
        // PC to Mac: Ctrl acts as Cmd, Win as Control, Alt+Tab is Cmd+Tab; a terminal keeps Ctrl.
        for c in [Os::Windows, Os::Linux] {
            let mut m = KeyMapper::default();
            m.set_table(default_rules(c, Os::Mac));
            let copy = keys(&mut m, &[(LEFT_CTRL, true), (C, true), (C, false), (LEFT_CTRL, false)], APP);
            assert_eq!(copy, [(LEFT_GUI, true), (C, true), (C, false), (LEFT_GUI, false)]);
            let win = keys(&mut m, &[(LEFT_GUI, true), (LEFT_GUI, false)], APP);
            assert_eq!(win, [(LEFT_CTRL, true), (LEFT_CTRL, false)]);
            let tab = keys(&mut m, &[(LEFT_ALT, true), (TAB, true), (TAB, false), (LEFT_ALT, false)], APP);
            assert_eq!(tab, [(LEFT_ALT, true), (LEFT_ALT, false), (LEFT_GUI, true), (TAB, true), (TAB, false), (LEFT_GUI, false)]);
            let term = keys(&mut m, &[(LEFT_CTRL, true), (C, true), (C, false), (LEFT_CTRL, false)], TERM);
            // Ctrl first lands as Cmd for a moment, then the rule swaps it for Control.
            assert_eq!(term, [(LEFT_GUI, true), (LEFT_CTRL, true), (LEFT_GUI, false), (C, true), (C, false), (LEFT_CTRL, false)]);
        }
    }
}
