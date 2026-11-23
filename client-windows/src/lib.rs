// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Sodium Remote's Windows client as a library: the stream window (`win`), Direct3D 11 presenting
//! (`d3d`), the clipboard (`clip`), audio out (`audio`) and the `Platform` (`platform`). `remote-client.exe`
//! (main.rs) is a CLI on it; the Qt app (remote-qt) runs the same window as a child of its own
//! (`embed`, docs/reference/client-app.md).

pub mod scancode;

#[cfg(windows)]
pub mod audio;
#[cfg(windows)]
pub mod clip;
#[cfg(windows)]
pub mod d3d;
pub mod embed;
#[cfg(windows)]
pub mod platform;
#[cfg(windows)]
pub mod win;
