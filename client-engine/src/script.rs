// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Scripted input for unattended tests: `--script FILE`
//! sends a timed list of inputs through the same path the keyboard and mouse
//! use. One step per line, `#` starts a comment, times are milliseconds from
//! the start (the first picture plus a second):
//!
//! ```text
//! 500   move 640 360              # the pointer, in stream pixels
//! 700   click                     # left, at the pointer (or: click right 100 200)
//! 900   key ctrl+a                # chord: modifiers down, key, release
//! 1000  key enter up              # one edge only: `down` or `up`
//! 1200  text Hello, world         # committed text (the rest of the line)
//! 1500  scroll 0 3                # wheel notches: dx dy (positive dy is down, as on the wire)
//! 1700  clipboard Some text       # sets the local clipboard (the sync sends it)
//! 2000  clipboard-show           # logs what the local clipboard holds
//! 2500  quit
//! ```
//!
//! Key names: `a`..`z`, `0`..`9`, `f1`..`f12`, `enter esc tab space backspace
//! delete insert home end pageup pagedown up down left right`, the modifiers
//! `shift ctrl alt win` (left ones; `rshift` etc. for the right), or `0xNN` for a
//! HID usage.

use crate::App;
use crate::msg::{self, Axis, AxisSource, Button, ClipFormat, InputEvent};
use anyhow::{Context, Result, bail};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, PartialEq)]
pub enum Action {
    Move(f64, f64),
    Button(Button, Option<bool>),
    Key(Vec<u16>, Option<bool>),
    Text(String),
    Scroll(i32, i32),
    Clipboard(String),
    ClipboardShow,
    Quit,
}

#[derive(Debug, PartialEq)]
pub struct Step {
    pub at_ms: u64,
    pub action: Action,
}

/// The HID keyboard-page usage for a key name.
pub fn usage_by_name(name: &str) -> Option<u16> {
    let n = name.to_ascii_lowercase();
    if let Some(hex) = n.strip_prefix("0x") {
        return u16::from_str_radix(hex, 16).ok();
    }
    let b = n.as_bytes();
    if b.len() == 1 {
        match b[0] {
            c @ b'a'..=b'z' => return Some(0x04 + (c - b'a') as u16),
            b'1'..=b'9' => return Some(0x1E + (b[0] - b'1') as u16),
            b'0' => return Some(0x27),
            _ => {}
        }
    }
    if let Some(f) = n.strip_prefix('f').and_then(|d| d.parse::<u16>().ok()) {
        return match f {
            1..=12 => Some(0x3A + f - 1),
            _ => None,
        };
    }
    Some(match n.as_str() {
        "enter" | "return" => 0x28,
        "esc" | "escape" => 0x29,
        "backspace" => 0x2A,
        "tab" => 0x2B,
        "space" => 0x2C,
        "minus" => 0x2D,
        "equal" => 0x2E,
        "insert" => 0x49,
        "home" => 0x4A,
        "pageup" => 0x4B,
        "delete" => 0x4C,
        "end" => 0x4D,
        "pagedown" => 0x4E,
        "right" => 0x4F,
        "left" => 0x50,
        "down" => 0x51,
        "up" => 0x52,
        "ctrl" | "lctrl" => 0xE0,
        "shift" | "lshift" => 0xE1,
        "alt" | "lalt" => 0xE2,
        "win" | "meta" | "super" | "lwin" => 0xE3,
        "rctrl" => 0xE4,
        "rshift" => 0xE5,
        "ralt" => 0xE6,
        "rwin" => 0xE7,
        _ => return None,
    })
}

fn button_by_name(n: &str) -> Option<Button> {
    Some(match n {
        "left" => Button::Left,
        "right" => Button::Right,
        "middle" => Button::Middle,
        "back" => Button::Back,
        "forward" => Button::Forward,
        _ => return None,
    })
}

pub fn parse(src: &str) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let err = |what: &str| format!("script line {}: {what}: {raw:?}", i + 1);
        let (t, rest) = line.split_once(char::is_whitespace).with_context(|| err("a time and an action"))?;
        let at_ms: u64 = t.parse().with_context(|| err("the time (ms)"))?;
        let rest = rest.trim();
        let (verb, args) = rest.split_once(char::is_whitespace).map_or((rest, ""), |(v, a)| (v, a.trim()));
        let words: Vec<&str> = args.split_whitespace().collect();
        let num = |s: &str| s.parse::<f64>().with_context(|| err("a number"));
        let action = match verb {
            "move" if words.len() == 2 => Action::Move(num(words[0])?, num(words[1])?),
            "click" | "down" | "up" => {
                let edge = match verb {
                    "down" => Some(true),
                    "up" => Some(false),
                    _ => None,
                };
                let (b, xy) = match words.first().and_then(|w| button_by_name(w)) {
                    Some(b) => (b, &words[1..]),
                    None => (Button::Left, &words[..]),
                };
                if xy.len() == 2 {
                    steps.push(Step { at_ms, action: Action::Move(num(xy[0])?, num(xy[1])?) });
                } else if !xy.is_empty() {
                    bail!(err("click [button] [x y]"));
                }
                Action::Button(b, edge)
            }
            "key" if !words.is_empty() => {
                let chord = words[0];
                let edge = match words.get(1).copied() {
                    Some("down") => Some(true),
                    Some("up") => Some(false),
                    None => None,
                    _ => bail!(err("key NAME [down|up]")),
                };
                let keys = chord
                    .split('+')
                    .map(|k| usage_by_name(k).with_context(|| err(&format!("unknown key {k:?}"))))
                    .collect::<Result<Vec<_>>>()?;
                Action::Key(keys, edge)
            }
            "text" => Action::Text(args.to_string()),
            "scroll" if words.len() == 2 => Action::Scroll(num(words[0])? as i32, num(words[1])? as i32),
            "clipboard" => Action::Clipboard(args.to_string()),
            "clipboard-show" => Action::ClipboardShow,
            "quit" => Action::Quit,
            "wait" => continue,
            _ => bail!(err("unknown action")),
        };
        steps.push(Step { at_ms, action });
    }
    steps.sort_by_key(|s| s.at_ms);
    Ok(steps)
}

fn key(app: &App, usage: u16, down: bool) {
    app.send_input(InputEvent::Key { usage, down });
}

/// Runs the script against the live app: once the first picture is in (plus a
/// second), each step at its time. Logs every step to stderr.
pub async fn run(app: Arc<App>, steps: Vec<Step>) {
    while !app.first_picture.load(std::sync::atomic::Ordering::SeqCst) {
        if app.quitting() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let t0 = std::time::Instant::now();
    for s in steps {
        let at = Duration::from_millis(s.at_ms);
        if let Some(d) = at.checked_sub(t0.elapsed()) {
            tokio::time::sleep(d).await;
        }
        if app.quitting() {
            return;
        }
        eprintln!("remote-client: script {:>6} ms {:?}", s.at_ms, s.action);
        match s.action {
            Action::Move(x, y) => {
                app.send_input(InputEvent::PointerAbs { x, y });
            }
            Action::Button(b, edge) => match edge {
                Some(down) => {
                    app.send_input(InputEvent::Button { button: b, down });
                }
                None => {
                    app.send_input(InputEvent::Button { button: b, down: true });
                    tokio::time::sleep(Duration::from_millis(40)).await;
                    app.send_input(InputEvent::Button { button: b, down: false });
                }
            },
            Action::Key(keys, edge) => {
                let (last, mods) = keys.split_last().expect("a key");
                match edge {
                    Some(down) => key(&app, *last, down),
                    None => {
                        for m in mods {
                            key(&app, *m, true);
                        }
                        key(&app, *last, true);
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        key(&app, *last, false);
                        for m in mods.iter().rev() {
                            key(&app, *m, false);
                        }
                    }
                }
            }
            Action::Text(t) => {
                for ev in InputEvent::commit_chunks(&t) {
                    app.send_input(ev);
                }
            }
            Action::Scroll(dx, dy) => {
                for (axis, n) in [(Axis::Horizontal, dx), (Axis::Vertical, dy)] {
                    if n != 0 {
                        // One notch is value120 120 (and 15 px of `value`, a typical wheel step).
                        app.send_input(InputEvent::Axis {
                            source: AxisSource::Wheel,
                            axis,
                            value: n as f64 * 15.0,
                            value120: n * 120,
                            stop: false,
                            more: false,
                        });
                    }
                }
            }
            Action::Clipboard(t) => match &app.clipboard {
                Some(c) => {
                    c.set_local(vec![(ClipFormat::Text, t.clone().into_bytes())], false);
                    c.offer(vec![(ClipFormat::Text, t.into_bytes())]);
                }
                None => eprintln!("remote-client: script: this platform has no clipboard"),
            },
            Action::ClipboardShow => match &app.clipboard {
                Some(c) => {
                    for (f, d) in c.peek() {
                        let t = if f == ClipFormat::Text { format!(" {:?}", String::from_utf8_lossy(d.get(..80.min(d.len())).unwrap_or(&[]))) } else { String::new() };
                        eprintln!("remote-client: script: local clipboard has {f:?}, {} bytes{t}", d.len());
                    }
                }
                None => eprintln!("remote-client: script: this platform has no clipboard"),
            },
            Action::Quit => {
                app.quit();
                return;
            }
        }
    }
}

#[allow(unused)]
fn _uses(_: msg::Os) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steps_and_chords() {
        let s = parse("# c\n900 key ctrl+a\n500 click right 10 20\n700 text Hi, you\n1000 scroll 0 -3\n").unwrap();
        assert_eq!(s[0].action, Action::Move(10.0, 20.0));
        assert_eq!(s[1].action, Action::Button(Button::Right, None));
        assert_eq!(s[2].action, Action::Text("Hi, you".into()));
        assert_eq!(s[3].action, Action::Key(vec![0xE0, 0x04], None));
        assert_eq!(s[4].action, Action::Scroll(0, -3));
        assert!(parse("1 key nosuchkey").is_err());
        assert_eq!(usage_by_name("f12"), Some(0x45));
        assert_eq!(usage_by_name("0"), Some(0x27));
    }
}
