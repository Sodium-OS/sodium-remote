// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The plugin socket client: remote/docs/reference/capture-ipc.md, version 4 (v3's INPUT_*,
//! CARET, POINTER_LOCK and FRAME_CLOCK's grace_ns, plus v4's LOCAL_INPUT). HELLO says
//! 4 and HELLO_ACK answers the version both speak: a v1–v3 plugin is accepted too (the
//! layouts are the same; it ignores what it doesn't know and never sends newer
//! messages), and LOCAL_INPUT is only sent to a v4 plugin.
//!
//! A `SOCK_SEQPACKET` Unix socket; every message is one packet with an 8-byte
//! header `{u32 type, u32 length}` and a little-endian payload; DMA-BUF and
//! sync_file fds ride along as `SCM_RIGHTS`.

use std::{
    io::{IoSlice, IoSliceMut},
    mem::MaybeUninit,
    os::fd::{AsFd, OwnedFd},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use rustix::net::{
    AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags, SocketAddrUnix, SocketType,
};

use super::{CaptureSource, Dmabuf, DmabufPlane, Frame, Rect, monotonic_ns};

pub const VERSION: u32 = 4;

// remoted -> plugin
pub const HELLO: u32 = 1;
pub const START: u32 = 2;
pub const STOP: u32 = 3;
pub const RELEASE: u32 = 4;
pub const FRAME_CLOCK: u32 = 5;
pub const INPUT_POINTER_ABS: u32 = 64;
pub const INPUT_POINTER_REL: u32 = 65;
pub const INPUT_BUTTON: u32 = 66;
pub const INPUT_AXIS: u32 = 67;
pub const INPUT_KEY: u32 = 68;
pub const INPUT_GESTURE: u32 = 69;
pub const INPUT_RESET: u32 = 70;
/// v4: the desk's own keyboards and pointers (`u32 mode, u32 pad`): 0 normal, 1 ignored,
/// 2 ignored but desk keys go to the `sodiumshell-privacy` layer.
pub const LOCAL_INPUT: u32 = 71;
// plugin -> remoted
pub const HELLO_ACK: u32 = 129;
pub const BUFFERS: u32 = 130;
pub const FRAME: u32 = 131;
pub const CURSOR_SHAPE: u32 = 132;
pub const CURSOR_POS: u32 = 133;
pub const HINT: u32 = 134;
pub const STATS: u32 = 135;
pub const STOPPED: u32 = 136;
pub const CARET: u32 = 137;
pub const POINTER_LOCK: u32 = 138;

/// START flags bit 0: ask for linear buffers (tests).
#[allow(dead_code)]
pub const START_LINEAR_ONLY: u32 = 1;
/// FRAME flags bit 0: an acquire sync_file is attached.
pub const FRAME_HAS_FENCE: u32 = 1;

const MAX_PACKET: usize = 64 * 1024;
const MAX_FDS: usize = 32;

pub fn default_socket() -> PathBuf {
    let dir = crate::xdg_runtime_dir();
    Path::new(&dir).join("sodiumos/remote-capture.sock")
}

/// One received packet.
pub struct Packet {
    pub kind: u32,
    pub payload: Vec<u8>,
    pub fds: Vec<OwnedFd>,
}

/// Little-endian reader over a payload.
pub struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader(b)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(self.0.len() >= n, "message too short");
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into()?))
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.take(n)
    }
}

pub fn send(sock: impl AsFd, kind: u32, payload: &[u8], fds: &[std::os::fd::BorrowedFd]) -> Result<()> {
    let mut msg = Vec::with_capacity(8 + payload.len());
    msg.extend_from_slice(&kind.to_le_bytes());
    msg.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    msg.extend_from_slice(payload);
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
    let mut anc = SendAncillaryBuffer::new(&mut space);
    if !fds.is_empty() {
        ensure!(anc.push(SendAncillaryMessage::ScmRights(fds)), "too many fds");
    }
    let n = rustix::net::sendmsg(sock, &[IoSlice::new(&msg)], &mut anc, SendFlags::NOSIGNAL)?;
    ensure!(n == msg.len(), "short send");
    Ok(())
}

pub fn recv(sock: impl AsFd) -> Result<Packet> {
    let mut buf = vec![0u8; MAX_PACKET];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
    let mut anc = RecvAncillaryBuffer::new(&mut space);
    let r = rustix::net::recvmsg(sock, &mut [IoSliceMut::new(&mut buf)], &mut anc, RecvFlags::CMSG_CLOEXEC)?;
    let mut fds = Vec::new();
    for m in anc.drain() {
        if let RecvAncillaryMessage::ScmRights(it) = m {
            fds.extend(it);
        }
    }
    ensure!(r.bytes != 0, "plugin closed the capture socket");
    ensure!(!r.flags.contains(rustix::net::ReturnFlags::TRUNC), "packet truncated");
    let mut rd = Reader::new(&buf[..r.bytes]);
    let kind = rd.u32()?;
    let len = rd.u32()? as usize;
    let payload = rd.bytes(len).context("payload shorter than its header says")?.to_vec();
    Ok(Packet { kind, payload, fds })
}

/// FRAME_CLOCK's payload (v3: the old pad is `grace_ns`).
pub fn frame_clock_payload(phase_offset_ns: i64, refresh_mhz: u32, grace_ns: u32) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&phase_offset_ns.to_le_bytes());
    b[8..12].copy_from_slice(&refresh_mhz.to_le_bytes());
    b[12..16].copy_from_slice(&grace_ns.to_le_bytes());
    b
}

/// Parses BUFFERS into its generation and buffers (the fds are taken from the packet).
pub fn parse_buffers(p: Packet) -> Result<(u32, Vec<Dmabuf>)> {
    let mut rd = Reader::new(&p.payload);
    let generation = rd.u32()?;
    let count = rd.u32()?;
    let width = rd.u32()?;
    let height = rd.u32()?;
    let format = rd.u32()?;
    let n_planes = rd.u32()?;
    let modifier = rd.u64()?;
    ensure!((1..=4).contains(&n_planes), "bad plane count {n_planes}");
    let mut layout = Vec::new();
    for _ in 0..n_planes {
        layout.push((rd.u32()?, rd.u32()?));
    }
    ensure!(
        p.fds.len() == (count * n_planes) as usize,
        "BUFFERS lists {} fds, {} came",
        count * n_planes,
        p.fds.len()
    );
    let mut fds = p.fds.into_iter();
    let mut buffers = Vec::new();
    for _ in 0..count {
        let planes = layout
            .iter()
            .map(|&(offset, stride)| DmabufPlane { fd: fds.next().unwrap(), offset, stride })
            .collect();
        buffers.push(Dmabuf { width, height, format, modifier, planes });
    }
    Ok((generation, buffers))
}

/// Parses FRAME; `t_ready_ns` is stamped now.
pub fn parse_frame(p: Packet) -> Result<Frame> {
    let t_ready_ns = monotonic_ns();
    let mut rd = Reader::new(&p.payload);
    let generation = rd.u32()?;
    let buffer = rd.u32()?;
    let seq = rd.u64()?;
    let t_rendered_ns = rd.u64()?;
    let n_damage = rd.u32()?;
    let flags = rd.u32()?;
    let mut damage = Vec::new();
    for _ in 0..n_damage {
        damage.push(Rect { x: rd.i32()?, y: rd.i32()?, w: rd.i32()?, h: rd.i32()? });
    }
    let acquire = if flags & FRAME_HAS_FENCE != 0 {
        Some(p.fds.into_iter().next().ok_or_else(|| anyhow!("FRAME says fence, no fd came"))?)
    } else {
        None
    };
    Ok(Frame { generation, buffer, seq, t_rendered_ns, t_ready_ns, damage, acquire })
}

/// STOPPED's reason, in words.
pub fn stop_reason(reason: u32) -> &'static str {
    match reason {
        1 => "no such output",
        2 => "the output went away",
        3 => "no exportable buffers",
        4 => "unsupported output or renderer",
        _ => "unknown reason",
    }
}

/// RELEASE's payload.
pub fn release_payload(generation: u32, buffer: u32) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&generation.to_le_bytes());
    b[4..].copy_from_slice(&buffer.to_le_bytes());
    b
}

/// START's payload.
pub fn start_payload(output: &str, ring_size: u32, flags: u32) -> Result<[u8; 40]> {
    ensure!(output.len() < 32, "output name too long");
    let mut start = [0u8; 40];
    start[..output.len()].copy_from_slice(output.as_bytes());
    start[32..36].copy_from_slice(&ring_size.to_le_bytes());
    start[36..40].copy_from_slice(&flags.to_le_bytes());
    Ok(start)
}

/// Connects to the plugin socket and says HELLO; returns the socket, the plugin's
/// capture-ipc version and its Hyprland version.
pub fn hello(path: &Path) -> Result<(OwnedFd, u32, String)> {
    let sock = rustix::net::socket(AddressFamily::UNIX, SocketType::SEQPACKET, None)?;
    rustix::net::connect(&sock, &SocketAddrUnix::new(path)?).with_context(|| format!("connecting to {}", path.display()))?;
    send(&sock, HELLO, &VERSION.to_le_bytes(), &[])?;
    let ack = recv(&sock)?;
    ensure!(ack.kind == HELLO_ACK, "expected HELLO_ACK, got {}", ack.kind);
    let mut rd = Reader::new(&ack.payload);
    let version = rd.u32()?;
    ensure!((1..=VERSION).contains(&version), "plugin speaks capture-ipc v{version}, remoted v{VERSION}");
    let name = rd.bytes(32)?;
    let hyprland = String::from_utf8_lossy(name.split(|&b| b == 0).next().unwrap_or(&[])).into_owned();
    Ok((sock, version, hyprland))
}

pub struct PluginSource {
    sock: OwnedFd,
    pub hyprland_version: String,
    generation: u32,
    buffers: Vec<Dmabuf>,
    /// The last STATS message: frames, dropped (no buffer), dropped (no client).
    pub stats: Option<(u64, u64, u64)>,
}

impl PluginSource {
    /// Connects, says HELLO and asks the plugin to START capturing `output`.
    pub fn connect(path: &Path, output: &str, ring_size: u32, flags: u32) -> Result<Self> {
        let (sock, _version, hyprland_version) = hello(path)?;
        send(&sock, START, &start_payload(output, ring_size, flags)?, &[])?;
        Ok(PluginSource { sock, hyprland_version, generation: 0, buffers: Vec::new(), stats: None })
    }

    /// FRAME_CLOCK: frames finish at `t` where `(t - phase_offset_ns) mod period == 0` on the
    /// host's CLOCK_MONOTONIC; remoted maps the client's vsync grid onto it.
    #[allow(dead_code)]
    pub fn frame_clock(&self, phase_offset_ns: i64, refresh_mhz: u32) -> Result<()> {
        send(&self.sock, FRAME_CLOCK, &frame_clock_payload(phase_offset_ns, refresh_mhz, 0), &[])
    }
}

impl CaptureSource for PluginSource {
    fn buffers(&self) -> (u32, &[Dmabuf]) {
        (self.generation, &self.buffers)
    }

    fn next_frame(&mut self) -> Result<Frame> {
        loop {
            let p = recv(&self.sock)?;
            match p.kind {
                BUFFERS => (self.generation, self.buffers) = parse_buffers(p)?,
                FRAME => {
                    let f = parse_frame(p)?;
                    if f.generation != self.generation {
                        // A frame from a dead generation: hand it straight back.
                        self.release(&f)?;
                        continue;
                    }
                    ensure!((f.buffer as usize) < self.buffers.len(), "FRAME names buffer {}", f.buffer);
                    return Ok(f);
                }
                STATS => {
                    let mut rd = Reader::new(&p.payload);
                    self.stats = Some((rd.u64()?, rd.u64()?, rd.u64()?));
                }
                // The cursor, hints, caret and pointer lock are media's, not this test path's.
                CURSOR_SHAPE | CURSOR_POS | HINT | CARET | POINTER_LOCK => {}
                STOPPED => {
                    let reason = Reader::new(&p.payload).u32()?;
                    bail!("the plugin stopped capturing: {} ({reason})", stop_reason(reason));
                }
                k => bail!("unknown message {k} from the plugin"),
            }
        }
    }

    fn release(&mut self, frame: &Frame) -> Result<()> {
        send(&self.sock, RELEASE, &release_payload(frame.generation, frame.buffer), &[])
    }
}

impl Drop for PluginSource {
    fn drop(&mut self) {
        let _ = send(&self.sock, STOP, &[], &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

    /// A tiny fake plugin: the happy path of capture-ipc v1 end to end, with a
    /// two-buffer ring of memfds standing in for DMA-BUFs and a fenced frame.
    #[test]
    fn handshake_buffers_frame_release() {
        let dir = std::env::temp_dir().join(format!("remoted-ipc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("remote-capture.sock");
        let _ = std::fs::remove_file(&path);
        let listener = rustix::net::socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
        rustix::net::bind(&listener, &SocketAddrUnix::new(&path).unwrap()).unwrap();
        rustix::net::listen(&listener, 1).unwrap();

        let server = std::thread::spawn(move || {
            let c = rustix::net::accept(&listener).unwrap();
            let hello = recv(&c).unwrap();
            assert_eq!((hello.kind, hello.payload.as_slice()), (HELLO, &VERSION.to_le_bytes()[..]));
            let mut ack = VERSION.to_le_bytes().to_vec();
            let mut name = [0u8; 32];
            name[..6].copy_from_slice(b"0.57.0");
            ack.extend_from_slice(&name);
            send(&c, HELLO_ACK, &ack, &[]).unwrap();

            let start = recv(&c).unwrap();
            assert_eq!(start.kind, START);
            assert_eq!(&start.payload[..9], b"REMOTE-1\0");
            assert_eq!(u32::from_le_bytes(start.payload[32..36].try_into().unwrap()), 2);

            // BUFFERS: generation 7, 2 buffers of 64x32 XRGB8888, linear, 1 plane
            let mut b = Vec::new();
            for v in [7u32, 2, 64, 32, crate::capture::DRM_FORMAT_XRGB8888, 1] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(&0u64.to_le_bytes());
            b.extend_from_slice(&0u32.to_le_bytes());
            b.extend_from_slice(&256u32.to_le_bytes());
            let m0 = rustix::fs::memfd_create("buf0", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
            let m1 = rustix::fs::memfd_create("buf1", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
            send(&c, BUFFERS, &b, &[m0.as_fd(), m1.as_fd()]).unwrap();

            // FRAME: buffer 1, seq 5, one damage rect, with a "fence"
            let mut f = Vec::new();
            for v in [7u32, 1] {
                f.extend_from_slice(&v.to_le_bytes());
            }
            f.extend_from_slice(&5u64.to_le_bytes());
            f.extend_from_slice(&123u64.to_le_bytes());
            f.extend_from_slice(&1u32.to_le_bytes());
            f.extend_from_slice(&FRAME_HAS_FENCE.to_le_bytes());
            for v in [1i32, 2, 30, 20] {
                f.extend_from_slice(&v.to_le_bytes());
            }
            let fence = rustix::fs::memfd_create("fence", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
            send(&c, FRAME, &f, &[fence.as_fd()]).unwrap();

            let rel = recv(&c).unwrap();
            assert_eq!(rel.kind, RELEASE);
            assert_eq!(rel.payload, [7u32.to_le_bytes(), 1u32.to_le_bytes()].concat());
            assert_eq!(recv(&c).unwrap().kind, STOP);
        });

        let mut src = PluginSource::connect(&path, "REMOTE-1", 2, 0).unwrap();
        assert_eq!(src.hyprland_version, "0.57.0");
        let frame = src.next_frame().unwrap();
        let (generation, bufs) = src.buffers();
        assert_eq!((generation, bufs.len(), bufs[1].width, bufs[1].planes[0].stride), (7, 2, 64, 256));
        assert_eq!((frame.buffer, frame.seq, frame.t_rendered_ns), (1, 5, 123));
        assert_eq!(frame.damage, vec![Rect { x: 1, y: 2, w: 30, h: 20 }]);
        assert!(frame.acquire.is_some());
        src.release(&frame).unwrap();
        drop(src);
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
