// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Remote protocol core: ICE path, Noise IK, framing, FEC, pacing, congestion
//! control, clock sync (wire format in docs/reference/protocol.md).
//!
//! Everything below `driver` is sans-IO: state machines that take datagrams and
//! `Instant`s and give back datagrams, events and deadlines. `driver` is the
//! thin tokio layer that owns sockets and timers. The public surface is plain
//! data (byte vectors, numbers, small enums) so a UniFFI wrapper can carry it
//! to Swift later; the wrapper will own the clock and pass `Instant`s in.

pub mod alloc;
pub mod audio;
pub mod brand;
pub mod cc;
pub mod clock;
pub mod connect;
pub mod crypto;
pub mod dirs;
pub mod driver;
pub mod feedback;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod health;
pub mod input;
pub mod keymap;
pub mod keyprotect;
pub mod keys;
pub mod lanlimit;
pub mod mdns;
pub mod msg;
pub mod mtu;
pub mod os_release;
pub mod pairing;
pub mod pacer;
pub mod path;
pub mod phase;
pub mod reliable;
pub mod rendezvous;
pub mod server_config;
pub mod session;
pub mod train;
pub mod video;
pub mod wire;

pub use crypto::StaticKeypair;
pub use session::{Event, OutgoingFrame, Peer, ReceivedFrame, Role, Session, SessionConfig, SessionStats};

// The Swift bindings (core-connect's `ffi` module).
#[cfg(feature = "ffi")]
uniffi::setup_scaffolding!();
