// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The capture-IPC contract (docs/capture-ipc.md) has one canonical C header,
//! `capture-ipc/capture_ipc.h`. This test checks that
//!
//! - every struct in it has the size the document lists (natural C layout),
//! - its message numbers and version are the ones `capture::plugin` uses,
//! - the sodiumwm plugin's own `Protocol.hpp`, when the SodiumOS tree is around,
//!   declares the same structs, enums and constants.

use remoted::capture::plugin;
use std::collections::BTreeMap;

const HEADER: &str = include_str!("../capture-ipc/capture_ipc.h");

/// `(type, field names)` in order, per struct.
type Structs = BTreeMap<String, Vec<(String, String)>>;

fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        rest = &rest[i + 2..];
        rest = rest.split_once("*/").map_or("", |(_, r)| r);
    }
    out.push_str(rest);
    out.lines().map(|l| l.split("//").next().unwrap_or("").trim_end()).collect::<Vec<_>>().join("\n")
}

/// Structs (`struct X { ... }` or `typedef struct X { ... } X;`), fields as (type, name)
/// with `a, b` split and array sizes kept in the name.
fn structs(src: &str) -> Structs {
    let text = strip_comments(src);
    let mut out = Structs::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find("struct ") {
        rest = &rest[i + 7..];
        let Some((head, after)) = rest.split_once('{') else { break };
        let name = head.trim().to_string();
        if name.is_empty() || name.contains(|c: char| !c.is_alphanumeric() && c != '_') {
            continue;
        }
        let (body, tail) = after.split_once('}').unwrap();
        let mut fields = Vec::new();
        for decl in body.split(';').map(str::trim).filter(|d| !d.is_empty()) {
            let (ty, names) = decl.split_once(char::is_whitespace).unwrap();
            for n in names.split(',') {
                fields.push((ty.to_string(), n.trim().to_string()));
            }
        }
        out.insert(name, fields);
        rest = tail;
    }
    out
}

/// Enum constants (`NAME = value`) and `#define`d / `constexpr` integer constants.
fn constants(src: &str) -> BTreeMap<String, u64> {
    let text = strip_comments(src);
    let mut out = BTreeMap::new();
    let num = |v: &str| -> Option<u64> {
        let v = v.trim().trim_start_matches('(').trim_end_matches(')').trim().trim_end_matches('u');
        if let Some((a, b)) = v.split_once("<<") {
            return Some(a.trim().trim_end_matches('u').parse::<u64>().ok()? << b.trim().parse::<u32>().ok()?);
        }
        v.parse().ok()
    };
    for line in text.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix("#define ") {
            if let Some((n, v)) = r.split_once(' ') {
                if let Some(v) = num(v) {
                    out.insert(n.to_string(), v);
                }
            }
        } else if let Some(r) = l.strip_prefix("constexpr ") {
            // constexpr uint32_t NAME = value;
            if let Some((lhs, v)) = r.trim_end_matches(';').split_once('=') {
                if let (Some(n), Some(v)) = (lhs.split_whitespace().last(), num(v)) {
                    out.insert(n.to_string(), v);
                }
            }
        } else if let Some((n, v)) = l.trim_end_matches(',').split_once('=') {
            let n = n.trim();
            if !n.is_empty() && !n.contains(char::is_whitespace) {
                if let Some(v) = num(v) {
                    out.insert(n.to_string(), v);
                }
            }
        }
    }
    out
}

fn scalar(ty: &str) -> (u64, u64) {
    match ty {
        "uint32_t" | "int32_t" => (4, 4),
        "uint64_t" | "int64_t" | "double" => (8, 8),
        "char" => (1, 1),
        t => panic!("unknown field type {t}"),
    }
}

/// (size, alignment, offsets by field) in natural C layout.
fn layout(fields: &[(String, String)]) -> (u64, BTreeMap<String, u64>) {
    let (mut off, mut align_max) = (0u64, 1u64);
    let mut offsets = BTreeMap::new();
    for (ty, name) in fields {
        let (size, align) = scalar(ty);
        let (base, count) = match name.split_once('[') {
            Some((b, n)) => (b, n.trim_end_matches(']').parse::<u64>().unwrap()),
            None => (name.as_str(), 1),
        };
        off = off.next_multiple_of(align);
        offsets.insert(base.to_string(), off);
        off += size * count;
        align_max = align_max.max(align);
    }
    (off.next_multiple_of(align_max), offsets)
}

#[test]
fn the_header_sizes_are_the_documented_ones() {
    let s = structs(HEADER);
    // docs/capture-ipc.md "Sizes" (BUFFERS and FRAME without their trailing arrays).
    let want = [
        ("SHeader", 8),
        ("SHello", 4),
        ("SStart", 40),
        ("SRelease", 8),
        ("SFrameClock", 16),
        ("SInputPointerAbs", 24),
        ("SInputPointerRel", 40),
        ("SInputButton", 16),
        ("SInputAxis", 32),
        ("SInputKey", 16),
        ("SInputGesture", 56),
        ("SLocalInput", 8),
        ("SHelloAck", 36),
        ("SBuffers", 32),
        ("SPlane", 8),
        ("SFrame", 32),
        ("SRect", 16),
        ("SCursorShape", 24),
        ("SCursorPos", 24),
        ("SHint", 40),
        ("SStats", 24),
        ("SStopped", 8),
        ("SCaret", 32),
        ("SPointerLock", 24),
    ];
    for (name, size) in want {
        let f = s.get(name).unwrap_or_else(|| panic!("{name} is not in the header"));
        assert_eq!(layout(f).0, size, "sizeof({name})");
    }
    assert_eq!(layout(&s["SBuffers"]).1["modifier"], 24);
    assert_eq!(layout(&s["SInputAxis"]).1["tNs"], 24);
    assert_eq!(layout(&s["SHint"]).1["seq"], 32);
}

#[test]
fn the_header_numbers_are_the_ones_remoted_uses() {
    let c = constants(HEADER);
    assert!(c.len() > 40, "the parser found {} constants", c.len());
    assert_eq!(c["VERSION"], plugin::VERSION as u64);
    for (name, v) in [
        ("HELLO", plugin::HELLO),
        ("START", plugin::START),
        ("RELEASE", plugin::RELEASE),
        ("FRAME_CLOCK", plugin::FRAME_CLOCK),
        ("INPUT_POINTER_ABS", plugin::INPUT_POINTER_ABS),
        ("INPUT_RESET", plugin::INPUT_RESET),
        ("LOCAL_INPUT", plugin::LOCAL_INPUT),
        ("HELLO_ACK", plugin::HELLO_ACK),
        ("BUFFERS", plugin::BUFFERS),
        ("FRAME", plugin::FRAME),
        ("CURSOR_SHAPE", plugin::CURSOR_SHAPE),
        ("CURSOR_POS", plugin::CURSOR_POS),
        ("HINT", plugin::HINT),
        ("STATS", plugin::STATS),
        ("STOPPED", plugin::STOPPED),
        ("CARET", plugin::CARET),
        ("POINTER_LOCK", plugin::POINTER_LOCK),
        ("START_FLAG_LINEAR", plugin::START_LINEAR_ONLY),
        ("FRAME_FLAG_FENCE", plugin::FRAME_HAS_FENCE),
    ] {
        assert_eq!(c[name], v as u64, "{name}");
    }
}

#[test]
fn the_plugins_protocol_hpp_declares_the_same_contract() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../compositor/plugin/src/remote/Protocol.hpp");
    let Ok(hpp) = std::fs::read_to_string(path) else {
        eprintln!("no sodiumwm plugin beside this tree: nothing to compare");
        return;
    };
    assert_eq!(structs(&hpp), structs(HEADER), "struct fields differ from capture_ipc.h");
    let (a, b) = (constants(&hpp), constants(HEADER));
    // `VERSION`, enums and flags: every name in either file, same value in both.
    assert_eq!(a, b, "constants differ from capture_ipc.h");
}
