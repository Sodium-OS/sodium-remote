// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Byte-level helpers and the channel numbers of the wire format.
//!
//! Everything on the wire is big-endian. Varints are LEB128 (u64).

/// The first byte of every Remote datagram. RFC 7983 leaves 192..=255 unused by
/// STUN (0..=3), DTLS (20..=63) and TURN ChannelData (64..=79), so one UDP socket
/// can carry ICE checks, TURN traffic and Remote packets side by side.
pub mod kind {
    pub const HANDSHAKE_INIT: u8 = 0xC1;
    pub const HANDSHAKE_RESP: u8 = 0xC2;
    pub const DATA: u8 = 0xC3;

    /// True when a datagram belongs to Remote (and not to STUN or TURN).
    pub fn is_remote(first: u8) -> bool {
        first >= 0xC0
    }
}

/// Channel numbers: the first byte of every decrypted payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Channel {
    Control = 0,
    Video = 1,
    Feedback = 2,
    Cursor = 3,
    Input = 4,
    Audio = 5,
    Mic = 6,
    Probe = 7,
}

impl Channel {
    pub fn from_u8(b: u8) -> Option<Channel> {
        Some(match b {
            0 => Channel::Control,
            1 => Channel::Video,
            2 => Channel::Feedback,
            3 => Channel::Cursor,
            4 => Channel::Input,
            5 => Channel::Audio,
            6 => Channel::Mic,
            7 => Channel::Probe,
            _ => return None,
        })
    }
}

#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn with_capacity(n: usize) -> Self {
        Writer { buf: Vec::with_capacity(n) }
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn varint(&mut self, mut v: u64) -> &mut Self {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(b);
                return self;
            }
            self.buf.push(b | 0x80);
        }
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }
    /// A u16 length prefix, then the bytes.
    pub fn lp_bytes(&mut self, b: &[u8]) -> &mut Self {
        self.u16(b.len() as u16).bytes(b)
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

/// A bounds-checked reader. Every getter returns `None` on truncation, so a
/// malformed packet is dropped with `?` and never panics.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.remaining() < n {
            return None;
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Some(s)
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.pos..];
        self.pos = self.buf.len();
        s
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }
    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    pub fn lp_bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u16()? as usize;
        self.take(n)
    }
}

/// Wrapping comparison for 32-bit sequence numbers (RFC 1982).
pub fn seq32_newer(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < 0x8000_0000
}
