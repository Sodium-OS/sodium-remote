// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Message schemas between a host and a client:
//! what travels inside the session's channels and the sealed signalling blobs.
//!
//! - **Control** (`Msg`): reliable and ordered, as control kind [`CONTROL_KIND`].
//!   Session setup, resize, settings, home monitor mode, takeover notices,
//!   clipboard, caret and focus, pointer lock, stats, disconnect.
//! - **Input** (`InputEvent`): one event per input-channel event, unreliable and
//!   redundant, delivered in order. IME preedit and commit ride here too, so a
//!   commit can never overtake or fall behind the keys typed around it.
//! - **Cursor image** (`CursorImage`): the payload of the core's reliable cursor
//!   shape message (control kind 250, cached by hash). Positions use the cursor
//!   channel: the hotspot in stream pixels.
//! - **Signalling** (`Signal`): the session offer and answer, carried sealed
//!   through the rendezvous or the LAN listener (see `connect.rs`).
//!
//! **Encoding:** serde with `postcard` (compact binary, varints). **Versioning**
//! (from v6 on): every release speaks its own version
//! [`VERSION`] and the one before ([`MIN_VERSION`]). The client says which it
//! speaks in its `Offer` and again in `Hello` (`min_version`..=`version`); the host
//! picks the highest version both speak ([`negotiate`]) and names it in `Welcome`.
//! With none in common the host refuses with `VersionMismatch`, whose numbers say
//! which side to update ([`DisconnectReason::update_side`]). Compatible additions
//! are new enum variants appended at the end (an older peer can't decode them and
//! skips the message: [`decode`] returns `None`) and fields appended to `Offer`.
//! Changing a variant's fields is incompatible: bump [`VERSION`].
//!
//! **Frozen layouts:** so that any two versions can at least refuse each other
//! with the right words, `Offer` keeps its v5 fields first and `min_version`
//! after them (newer fields go after that, and postcard ignores trailing bytes),
//! and `Hello` always starts with `version, min_version`.
//!
//! **Units:** stream pixels are REMOTE-1's buffer pixels, top-left origin.
//! Refresh rates are in millihertz (60 Hz = 60000), like capture-ipc.

use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Schema version. Bump on any incompatible change.
///
/// 2: `ClientDisplay` carries the display's own size
/// Input events carry the client's event time and `Axis.more`;
/// new `KeyTable`, `KeyframeRequest` and `Vsync` messages and the
/// `EndedAtComputer` reason.
///
/// 3: `CodecChoice` gains
/// `Hevc420`, so the codec segment "HEVC" applies live through `Settings`.
///
/// 4: `HostStats` carries the controller's
/// target, C_est and the probe ceiling.
///
/// 5: `Codec::H264`; `DecodeSupport`
/// gains `hevc` and `h264`; `Hello.client_os` and `Welcome.host_os`; `Welcome`
/// gains `display_mode`, `live_resize`, `displays` and `current_display`, its
/// `scales` may be empty and `home_monitor` is optional; new `SelectDisplay` and
/// `SecureScreen` messages; `Welcome.view_only` and
/// `DisconnectReason::PermissionMissing`. No v4
/// compatibility.
///
/// 6: version ranges. `Hello.min_version` (second field) and
/// `Offer.min_version` (last field); the host picks the highest common version;
/// mDNS `_sodium-remote._tcp`. No v5 compatibility: a v5 peer is refused with words
/// that name the side to update.
pub const VERSION: u16 = 6;
/// The oldest version this build speaks. From v6 on every release speaks
/// `VERSION - 1` too; v6 is the first with ranges, so it speaks only itself.
pub const MIN_VERSION: u16 = 6;

/// The versions one side speaks, inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionRange {
    pub min: u16,
    pub max: u16,
}

impl VersionRange {
    /// This build's range.
    pub const OURS: VersionRange = VersionRange { min: MIN_VERSION, max: VERSION };

    /// What a peer said: `version` is the highest it speaks. Before v6 there was no
    /// minimum: such a peer speaks exactly its `version`.
    pub fn from_peer(version: u16, min_version: u16) -> VersionRange {
        if version < 6 { VersionRange { min: version, max: version } } else { VersionRange { min: min_version.min(version), max: version } }
    }
}

/// The highest version both sides speak, or the refusal the host sends.
///
/// The refusal's `host` is the host's lowest version when the client is too old
/// and its highest when the host is too old, so `host > client` always means
/// "update the client" (what v5 clients already show).
pub fn negotiate(host: VersionRange, client: VersionRange) -> Result<u16, DisconnectReason> {
    let v = host.max.min(client.max);
    if v >= host.min && v >= client.min {
        return Ok(v);
    }
    let host_says = if client.max < host.min { host.min } else { host.max };
    Err(DisconnectReason::VersionMismatch { host: host_says, client: client.max })
}

/// Which side a `VersionMismatch` asks the user to update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum UpdateSide {
    /// The computer being connected to.
    Host,
    /// This device.
    Client,
}
/// The control-channel kind (`Session::send_control`) that carries `Msg`.
pub const CONTROL_KIND: u8 = 1;
/// The largest clipboard chunk in one `ClipboardData`, so other control
/// messages are never stuck behind one huge message.
pub const CLIPBOARD_CHUNK: usize = 64 * 1024;
/// The largest clipboard either side takes from the other: every item of one
/// offer together. The platform clipboards cap a single item at the same.
pub const CLIPBOARD_MAX: usize = 32 << 20;
/// Text inside one `InputEvent` should stay under this (the event must fit one
/// datagram); `InputEvent::commit_chunks` splits longer commits.
pub const INPUT_TEXT_MAX: usize = 512;

/// Encode any schema value (postcard).
pub fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    postcard::to_allocvec(v).expect("postcard encoding into a Vec can't fail")
}

/// Decode a schema value. `None` for a malformed message or one from a newer
/// peer (an unknown variant): skip it.
pub fn decode<T: DeserializeOwned>(b: &[u8]) -> Option<T> {
    postcard::from_bytes(b).ok()
}

// ---------------------------------------------------------------------------
// Control messages

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Msg {
    /// Client → host, the first control message of every connection.
    Hello(Hello),
    /// Host → client, the answer to `Hello`: the stream as the host set it up.
    Welcome(Welcome),
    /// Client → host: the window (or display) changed size. Send every change;
    /// the host debounces (150 ms after the last one).
    Resize(ClientDisplay),
    /// Client → host: new stream settings. The host answers with `Format` when
    /// the stream changes.
    Settings(StreamSettings),
    /// Host → client: the stream's format changed (after a resize, a UI size or
    /// refresh change, or a codec change). The next frame is a keyframe.
    Format(StreamFormat),
    /// Client → host: switch the home monitor mode. Host → client: the mode in
    /// effect now (also after the user changed it at the desk).
    HomeMonitor(HomeMonitorMode),
    /// Host → client: the desktop locked or unlocked ("locked: unlock in the
    /// stream").
    Locked(bool),
    /// Host → the client being replaced: another paired device took over. The
    /// host closes this connection right after; show "Taken over by <name>".
    TakenOver { by: String },
    /// Host → the new client: it took over a held or reconnecting session from
    /// another device, silently. Show the toast "Took over from <name>".
    TookOver { from: String },
    /// Either way: new clipboard contents. Small items come inline.
    ClipboardOffer(ClipboardOffer),
    /// Either way: send me this format of offer `serial`.
    ClipboardRequest { serial: u32, format: ClipFormat },
    /// Either way: one chunk of a requested (or not inlined) item.
    ClipboardData(ClipboardData),
    /// Host → client: the focused text field's caret, for the candidate window.
    Caret(Caret),
    /// Host → client: a text field gained or lost focus. `password`: switch to
    /// secure input with the input method off.
    FocusChanged { focused: bool, password: bool },
    /// Host → client: the app ended the composition. Discard the marked text;
    /// don't commit it.
    ImeReset,
    /// Host → client: pointer lock (games) changed. Locked: send relative motion.
    PointerLock(PointerLock),
    /// Either way, about once a second: numbers for the stats HUD.
    Stats(Stats),
    /// Either way: the session ends now, and why.
    Disconnect(DisconnectReason),
    /// Client → host, at connect and after every edit: the key table (SPEC 8.2,
    ///). The client sends keys exactly as pressed; the host applies
    /// the table using the focused app. Empty = PC passthrough.
    KeyTable(Vec<KeyRule>),
    /// Client → host: my decoder was reset (or failed); send a keyframe.
    KeyframeRequest,
    /// Client → host, about once a second: where frames land against the
    /// client's display refresh, for the phase lock.
    Vsync(VsyncReport),
    /// Client → host: show this display (`DisplayInfo::id`). One display at a
    /// time; the host answers with `Format` for the new display's size.
    SelectDisplay(u32),
    /// Host → client: a lock, login or UAC screen is up that the host can't
    /// capture or drive (`true`), or it went away (`false`). Show a notice; the
    /// session resumes by itself afterwards.
    SecureScreen(bool),
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        encode(self)
    }
    /// Decode a control message. A `Hello` or `Welcome` from a build before `os_flavor`
    /// ends one field early: postcard has no field defaults, so it is read again
    /// with the empty string that field would have been.
    pub fn decode(b: &[u8]) -> Option<Msg> {
        decode(b).or_else(|| {
            let mut longer = b.to_vec();
            longer.push(0);
            match decode::<Msg>(&longer)? {
                m @ (Msg::Hello(_) | Msg::Welcome(_)) => Some(m),
                _ => None,
            }
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Hello {
    /// The highest version the client speaks.
    pub version: u16,
    /// The lowest version the client speaks (v6 on; always the second field).
    #[cfg_attr(feature = "ffi", uniffi(default = 6))]
    pub min_version: u16,
    /// The device's display name ("MacBook Air").
    pub name: String,
    pub display: ClientDisplay,
    pub decode: DecodeSupport,
    pub settings: StreamSettings,
    /// The client's operating system, for the key table defaults (`keymap`).
    pub client_os: Os,
    /// The Linux distribution's os-release `ID`, lowercase ("sodiumos", "fedora"); empty
    /// elsewhere and from older builds. Last, so an older `Hello` still decodes
    /// ([`Msg::decode`]). Only the UI reads it.
    #[serde(default)]
    #[cfg_attr(feature = "ffi", uniffi(default = ""))]
    pub client_os_flavor: String,
}

/// An operating system, for key table defaults and the UI.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Os {
    Linux,
    Mac,
    Windows,
}

/// What the client shows the stream in.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ClientDisplay {
    /// The stream size the client wants, in pixels: the window's backing pixels
    /// (windowed) or the display's (full screen), or a fixed choice.
    pub width: u32,
    pub height: u32,
    /// The client's own UI density: stream pixels per "looks like" point. The
    /// Mac streams its window's backing pixels, so this is its backing scale,
    /// 2.0 on every Retina Mac; a client that
    /// streams panel pixels sends the panel's ratio instead (2560 ÷ 1470 ≈
    /// 1.741 on the 13" Air). "Match Mac" snaps this.
    pub pixels_per_point: f32,
    /// The display's refresh rate (mHz), for the default stream rate.
    pub refresh_mhz: u32,
    pub fullscreen: bool,
    /// The display's own size in pixels (what full screen would stream). The
    /// UI scale list comes from it, never from a window's size.
    pub display_width: u32,
    pub display_height: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DecodeSupport {
    /// HEVC 4:2:0 decode (every client so far; false: ask for H.264).
    pub hevc: bool,
    /// H.264 (8-bit 4:2:0) decode: the universal baseline.
    pub h264: bool,
    /// Hardware HEVC 4:4:4 decode (probed on the Mac).
    pub hevc_444: bool,
    pub av1: bool,
    /// The largest frame the decoder takes (0: no limit known).
    pub max_width: u32,
    pub max_height: u32,
}

/// The client settings the host acts on. Resolution mode and
/// "Fixed resolution" stay on the client: they decide what `Resize` says.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StreamSettings {
    /// 30, 60, 90, 120 or 144 Hz in mHz; 0 = the display's rate.
    pub refresh_mhz: u32,
    pub ui_size: UiSize,
    pub codec: CodecChoice,
    /// A cap for hotspots with data limits, in kbit/s; 0 = Auto.
    pub bandwidth_limit_kbps: u32,
    /// Lighter effects on slow networks; on by default.
    pub optimised_look: bool,
}

impl Default for StreamSettings {
    fn default() -> Self {
        StreamSettings {
            refresh_mhz: 0,
            ui_size: UiSize::MatchMac,
            codec: CodecChoice::Auto,
            bandwidth_limit_kbps: 0,
            optimised_look: true,
        }
    }
}

/// "UI size: Match Mac · Larger · Smaller · Same as desk".
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum UiSize {
    #[default]
    MatchMac,
    /// One step up the valid scale list from Match Mac.
    Larger,
    /// One step down.
    Smaller,
    /// The desk monitor's own scale.
    SameAsDesk,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum CodecChoice {
    /// HEVC 4:4:4 if the client decodes it in hardware, else HEVC 4:2:0.
    #[default]
    Auto,
    /// AV1 4:2:0.
    Av1,
    /// HEVC 4:2:0 whatever the client decodes: fewer bytes than 4:4:4
    /// Last, so the older values keep their encoding.
    Hevc420,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Codec {
    Hevc420,
    Hevc444,
    Av1,
    /// H.264, 8-bit 4:2:0 (added in v5).
    H264,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum HomeMonitorMode {
    /// The desk monitor mirrors the stream (default).
    #[default]
    Mirror,
    /// "In use remotely" on the desk monitor; local input ignored.
    Privacy,
    /// The desk monitor is powered down until disconnect.
    Off,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Welcome {
    /// The version the host picked: the highest both sides speak.
    pub version: u16,
    /// The host's display name.
    pub name: String,
    pub format: StreamFormat,
    /// The valid scales for this size, ascending, for the UI size menu. Empty:
    /// the host offers no UI size (hide the control).
    pub scales: Vec<f32>,
    /// `None`: the host has no home monitor mode (hide the control).
    pub home_monitor: Option<HomeMonitorMode>,
    pub locked: bool,
    /// Set when this connection silently took over a held or reconnecting
    /// session from another device (also sent as `TookOver`).
    pub took_over_from: Option<String>,
    /// True when this connection resumed this client's own held session.
    pub resumed: bool,
    pub host_os: Os,
    pub display_mode: DisplayMode,
    /// The host follows `Resize` (a virtual display). False: never send it, and
    /// fit the picture by aspect (letterbox).
    pub live_resize: bool,
    /// The displays the client can pick between (`SelectDisplay`).
    pub displays: Vec<DisplayInfo>,
    /// The `DisplayInfo::id` being streamed.
    pub current_display: u32,
    /// Input can't be injected (a Mac without Accessibility): the client shows a "View only"
    /// chip and need not send input. False for hosts that can always inject.
    pub view_only: bool,
    /// The host's os-release `ID`, lowercase; empty on Windows and macOS and from older
    /// builds. Last, so an older `Welcome` still decodes ([`Msg::decode`]).
    #[serde(default)]
    #[cfg_attr(feature = "ffi", uniffi(default = ""))]
    pub host_os_flavor: String,
}

/// Whether the host streams a display it made for the session, or a real one.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum DisplayMode {
    /// The host makes a virtual display and sizes it to the client (SodiumOS).
    Virtual,
    /// A physical display at its native size; the client letterboxes.
    Physical,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DisplayInfo {
    pub id: u32,
    pub name: String,
    /// Pixels.
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub primary: bool,
}

impl DecodeSupport {
    /// Whether the stream should be H.264: the client decodes it, the host
    /// encodes it, and HEVC isn't available on both sides (HEVC is preferred
    /// when it is).
    pub fn use_h264(decode: Option<DecodeSupport>, host_hevc: bool, host_h264: bool) -> bool {
        let Some(d) = decode else { return false };
        d.h264 && host_h264 && !(d.hevc && host_hevc)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StreamFormat {
    /// REMOTE-1's size in pixels: the stream's frame size.
    pub width: u32,
    pub height: u32,
    /// The scale Hyprland uses on REMOTE-1 (snapped).
    pub scale: f32,
    pub refresh_mhz: u32,
    pub codec: Codec,
}

// ---------------------------------------------------------------------------
// Clipboard

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum ClipFormat {
    /// UTF-8 plain text.
    Text,
    /// HTML (rich text from browsers and most apps).
    Html,
    /// RTF (rich text from Cocoa apps).
    Rtf,
    Png,
    Jpeg,
    /// Anything else, by MIME type.
    Mime(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ClipboardOffer {
    /// Increases with every new clipboard on the sending side; requests and
    /// data name it, so stale ones are dropped.
    pub serial: u32,
    pub items: Vec<ClipItem>,
    /// The sender marked it sensitive (a password manager's hint): put it on
    /// the pasteboard with `org.nspasteboard.ConcealedType`, never store it.
    pub concealed: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ClipItem {
    pub format: ClipFormat,
    /// Total size in bytes.
    pub size: u64,
    /// The bytes, when small enough to send with the offer (at most
    /// `CLIPBOARD_CHUNK`); otherwise ask with `ClipboardRequest`.
    pub inline: Option<Vec<u8>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ClipboardData {
    pub serial: u32,
    pub format: ClipFormat,
    /// Where this chunk starts in the item, and the item's total size.
    pub offset: u64,
    pub total: u64,
    pub bytes: Vec<u8>,
}

impl ClipboardData {
    /// Cut an item into `CLIPBOARD_CHUNK`-sized messages.
    pub fn chunks(serial: u32, format: &ClipFormat, data: &[u8]) -> Vec<Msg> {
        let total = data.len() as u64;
        if data.is_empty() {
            return vec![Msg::ClipboardData(ClipboardData { serial, format: format.clone(), offset: 0, total, bytes: vec![] })];
        }
        data.chunks(CLIPBOARD_CHUNK)
            .enumerate()
            .map(|(i, c)| {
                Msg::ClipboardData(ClipboardData {
                    serial,
                    format: format.clone(),
                    offset: (i * CLIPBOARD_CHUNK) as u64,
                    total,
                    bytes: c.to_vec(),
                })
            })
            .collect()
    }

    pub fn is_last(&self) -> bool {
        self.offset + self.bytes.len() as u64 >= self.total
    }
}

/// A clipboard arriving from the peer: one offer's items, filled in by its
/// `ClipboardData`. Every receiver (hosts and clients) assembles through this, so
/// the limits hold in both directions:
/// - one item per format (later duplicates are skipped);
/// - [`CLIPBOARD_MAX`] for all items together (an item that doesn't fit is skipped);
/// - an inline item must be exactly its declared size;
/// - a chunk must start where the item's bytes end and stay within its size.
#[derive(Debug)]
pub struct ClipIncoming {
    pub serial: u32,
    pub concealed: bool,
    /// Format, declared size, bytes so far.
    items: Vec<(ClipFormat, u64, Vec<u8>)>,
}

impl ClipIncoming {
    /// Take an offer, keeping only the formats `keep` accepts. Returns the
    /// assembly and the formats to ask for with `ClipboardRequest` (those not
    /// inline), or `None` when nothing in it is taken.
    pub fn from_offer(o: &ClipboardOffer, keep: impl Fn(&ClipFormat) -> bool) -> Option<(ClipIncoming, Vec<ClipFormat>)> {
        let mut items: Vec<(ClipFormat, u64, Vec<u8>)> = Vec::new();
        let mut requests = Vec::new();
        let mut budget = CLIPBOARD_MAX as u64;
        for it in &o.items {
            if !keep(&it.format) || items.iter().any(|(f, _, _)| *f == it.format) || it.size > budget {
                continue;
            }
            match &it.inline {
                Some(d) if d.len() as u64 == it.size => items.push((it.format.clone(), it.size, d.clone())),
                Some(_) => continue,
                None => {
                    items.push((it.format.clone(), it.size, Vec::with_capacity(it.size as usize)));
                    requests.push(it.format.clone());
                }
            }
            budget -= it.size;
        }
        (!items.is_empty()).then(|| (ClipIncoming { serial: o.serial, concealed: o.concealed, items }, requests))
    }

    /// A chunk of one of the items. False (and nothing changes) for another
    /// offer's, an unknown format's, one out of order or one past the item's size.
    pub fn add(&mut self, d: &ClipboardData) -> bool {
        if d.serial != self.serial {
            return false;
        }
        let Some((_, size, buf)) = self.items.iter_mut().find(|(f, _, _)| *f == d.format) else { return false };
        let end = d.offset.checked_add(d.bytes.len() as u64);
        if d.offset != buf.len() as u64 || end.is_none_or(|e| e > *size) {
            return false;
        }
        buf.extend_from_slice(&d.bytes);
        true
    }

    /// Every item has all its bytes.
    pub fn is_complete(&self) -> bool {
        self.items.iter().all(|(_, size, b)| b.len() as u64 == *size)
    }

    pub fn into_items(self) -> Vec<(ClipFormat, Vec<u8>)> {
        self.items.into_iter().map(|(f, _, b)| (f, b)).collect()
    }
}

// ---------------------------------------------------------------------------
// Caret, cursor, pointer lock

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Caret {
    pub visible: bool,
    /// In stream pixels. `w` is often 0.
    pub rect: Rect,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum PointerLock {
    None,
    /// Send `InputEvent::PointerRel` and hide the local cursor.
    Locked,
    /// The pointer stays inside the surface; absolute motion still works.
    Confined,
}

/// The cursor image: the payload of the core's cursor shape message.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct CursorImage {
    /// In stream pixels (the cursor as drawn at REMOTE-1's scale).
    pub width: u32,
    pub height: u32,
    pub hot_x: i32,
    pub hot_y: i32,
    /// REMOTE-1's scale, so the client can draw it at the right point size.
    pub scale: f32,
    /// ARGB8888 premultiplied, little-endian (B, G, R, A in memory), row-major,
    /// `width * 4` bytes per row.
    pub pixels: Vec<u8>,
}

impl CursorImage {
    /// The largest side a cursor image may have, in pixels.
    pub const MAX_SIDE: u32 = 256;

    /// Each side in `1..=MAX_SIDE` and exactly `width * height * 4` bytes of pixels.
    /// A client drops an image that isn't.
    pub fn is_valid(&self) -> bool {
        let sides = (1..=Self::MAX_SIDE).contains(&self.width) && (1..=Self::MAX_SIDE).contains(&self.height);
        sides && (self.width as usize).checked_mul(self.height as usize).and_then(|n| n.checked_mul(4)) == Some(self.pixels.len())
    }

    /// The image scaled down (nearest neighbour) so each side fits `MAX_SIDE`, with
    /// its hotspot moved to match. `None` when its pixels don't match its size.
    pub fn fitted(self) -> Option<CursorImage> {
        let (w, h) = (self.width as usize, self.height as usize);
        if w == 0 || h == 0 || w.checked_mul(h).and_then(|n| n.checked_mul(4)) != Some(self.pixels.len()) {
            return None;
        }
        if self.is_valid() {
            return Some(self);
        }
        let k = Self::MAX_SIDE as f64 / w.max(h) as f64;
        let (dw, dh) = (((w as f64 * k).round() as usize).clamp(1, 256), ((h as f64 * k).round() as usize).clamp(1, 256));
        let mut pixels = vec![0u8; dw * dh * 4];
        for y in 0..dh {
            let sy = (y * h / dh).min(h - 1);
            for x in 0..dw {
                let sx = (x * w / dw).min(w - 1);
                let (si, di) = ((sy * w + sx) * 4, (y * dw + x) * 4);
                pixels[di..di + 4].copy_from_slice(&self.pixels[si..si + 4]);
            }
        }
        Some(CursorImage {
            width: dw as u32,
            height: dh as u32,
            hot_x: (self.hot_x as f64 * dw as f64 / w as f64) as i32,
            hot_y: (self.hot_y as f64 * dh as f64 / h as f64) as i32,
            scale: self.scale,
            pixels,
        })
    }
}

// ---------------------------------------------------------------------------
// Stats and disconnect

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Stats {
    /// Host → client: per-stage times (µs, p50 over the last second).
    pub host: Option<HostStats>,
    /// Client → host.
    pub client: Option<ClientStats>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct HostStats {
    pub capture_us: u32,
    pub convert_us: u32,
    pub encode_us: u32,
    /// From the first to the last packet of a frame leaving the pacer.
    pub send_us: u32,
    pub fps: f32,
    pub bitrate_bps: u64,
    /// QP of the last frame, for the HUD's quality line.
    pub qp: u8,
    /// "Remote-optimised look" is in effect.
    pub optimised_look: bool,
    /// The last change that needed refinement: from its frame to readable
    /// (luma at the QP 34 bar) and to lossless (every plane, QP 20), ms; 0 = none
    /// yet.
    pub readable_ms: u32,
    pub lossless_ms: u32,
    /// The last change that needed refinement, when it did not end at the stop: ms to its
    /// settling, and how it ended (1 parked: a pass coded nothing; 2 cap: out of passes);
    /// 0 / 0 when the latest change ended at the stop. The HUD shows
    /// "settled" for it where it showed "lossless".
    pub settled_ms: u32,
    pub settled_end: u8,
    /// The controller's target rate, bit/s (the encoder's rate is it less FEC;
    ///). The HUD shows it beside the text figures.
    pub target_bps: u64,
    /// C_est, the measured bottleneck rate, bit/s (0: none).
    pub c_est_bps: u64,
    /// A refused probe's ceiling, bit/s (0: none).
    pub probe_ceiling_bps: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ClientStats {
    pub decode_us: u32,
    /// From decode done to `presentedTime`.
    pub present_us: u32,
    pub fps: f32,
    pub frames_late: u32,
    pub frames_dropped: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum DisconnectReason {
    /// The user chose Disconnect.
    User,
    /// Another device took over (the host also sends `TakenOver`).
    TakenOver { by: String },
    /// The host is shutting down or `remoted` is stopping.
    HostShutdown,
    /// The host is going to sleep.
    HostSleeping,
    /// This device is no longer paired.
    Revoked,
    /// No common schema version: show the version-mismatch card. `host > client`:
    /// update the client; otherwise update the host (`update_side`, `negotiate`).
    VersionMismatch { host: u16, client: u16 },
    /// The host's hold of a lost session ran out.
    HoldExpired,
    /// Anything else, for the log.
    Error(String),
    /// The password was typed on the privacy screen at the computer: "Ended at
    /// the computer".
    EndedAtComputer,
    /// The host's OS refused it a permission it needs for this stream (a Mac without Screen
    /// Recording): the client shows the permission card. The flags say which are missing.
    PermissionMissing { screen_recording: bool, accessibility: bool },
    /// "Allow remote control" was switched off on the host:
    /// the client shows "Control switched off". Never the text `"off"` in `Error`.
    ControlOff,
    /// The computer is locked (or showing its sign-in screen) and ended the session, or can't start one: the
    /// client shows "Unlock it at that computer, then reconnect". Consent and the restore token stay.
    HostLocked,
}

// ---------------------------------------------------------------------------
// Input (the input channel's events)

/// One input event, client → host. Keys are already mapped by the client's
/// table: the host only turns HID usages into evdev codes.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum InputEvent {
    /// Absolute pointer position, stream pixels (fractional).
    PointerAbs { x: f64, y: f64 },
    /// Relative motion under pointer lock, stream pixels.
    PointerRel { dx: f64, dy: f64, dx_unaccel: f64, dy_unaccel: f64 },
    Button { button: Button, down: bool },
    /// Scroll. Wheels set `value120`; trackpads send `Finger` with phases, and
    /// `stop` at the end, so momentum feels native. `more`: the next
    /// Axis belongs to the same pointer frame (both axes of one trackpad event).
    Axis { source: AxisSource, axis: Axis, value: f64, value120: i32, stop: bool, more: bool },
    /// A key, as a USB HID usage on the keyboard page (0x07), exactly as
    /// pressed: Cmd is Left/Right GUI, Option is Alt, Globe is [`USAGE_GLOBE`].
    /// The host maps it with the client's `KeyTable`.
    Key { usage: u16, down: bool },
    Gesture { kind: GestureKind, phase: GesturePhase, fingers: u32, dx: f64, dy: f64, scale: f64, rotation: f64 },
    /// IME: the marked text being composed; `cursor` in characters (code
    /// points), -1 hides it. Empty text clears the preedit.
    ImePreedit { text: String, cursor: i32 },
    /// IME: commit this text to the focused field.
    ImeCommit { text: String },
    /// Release every key and button the client holds (focus lost, Cmd-Tab away).
    ReleaseAll,
}

impl InputEvent {
    /// The input channel's event: the client's event time `t_us` (its session
    /// clock, µs) and the event. The core stamps it (`Client::send_input`).
    pub fn encode_at(&self, t_us: u64) -> Vec<u8> {
        encode(&(t_us, self))
    }
    /// The time and the event (`None`: malformed, or from a newer peer).
    pub fn decode_at(b: &[u8]) -> Option<(u64, InputEvent)> {
        decode(b)
    }

    /// A commit split into events under `INPUT_TEXT_MAX` bytes, on character
    /// boundaries.
    pub fn commit_chunks(text: &str) -> Vec<InputEvent> {
        let mut out = Vec::new();
        let mut start = 0;
        while start < text.len() {
            let mut end = (start + INPUT_TEXT_MAX).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            out.push(InputEvent::ImeCommit { text: text[start..end].to_string() });
            start = end;
        }
        out
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    /// Any other, as an evdev BTN_* code.
    Other(u32),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum AxisSource {
    Wheel,
    Finger,
    Continuous,
    WheelTilt,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum GestureKind {
    Swipe,
    Pinch,
    Hold,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum GesturePhase {
    Begin,
    Update,
    End,
    Cancel,
}

// ---------------------------------------------------------------------------
// The key table

/// Globe (fn) has no HID usage; this one is reserved for it, above the
/// keyboard page's modifiers (0xE0–0xE7) and outside its byte range.
pub const USAGE_GLOBE: u16 = 0x0100;

/// HID usages the default table refers to (keyboard page 0x07).
pub mod usage {
    pub const C: u16 = 0x06;
    pub const Q: u16 = 0x14;
    pub const S: u16 = 0x16;
    pub const V: u16 = 0x19;
    pub const BACKSPACE: u16 = 0x2A;
    pub const TAB: u16 = 0x2B;
    pub const SPACE: u16 = 0x2C;
    pub const F4: u16 = 0x3D;
    pub const HOME: u16 = 0x4A;
    pub const END: u16 = 0x4D;
    pub const RIGHT: u16 = 0x4F;
    pub const LEFT: u16 = 0x50;
    pub const DOWN: u16 = 0x51;
    pub const UP: u16 = 0x52;
    pub const LEFT_CTRL: u16 = 0xE0;
    pub const LEFT_SHIFT: u16 = 0xE1;
    pub const LEFT_ALT: u16 = 0xE2;
    pub const LEFT_GUI: u16 = 0xE3;
    pub const RIGHT_CTRL: u16 = 0xE4;
    pub const RIGHT_SHIFT: u16 = 0xE5;
    pub const RIGHT_ALT: u16 = 0xE6;
    pub const RIGHT_GUI: u16 = 0xE7;
}

/// Modifier bits of a chord, either side: the client's keyboard in `from`
/// (Cmd is `GUI`), the host's in `to` (`GUI` is Win/Super).
pub mod mods {
    pub const CTRL: u8 = 1;
    pub const SHIFT: u8 = 2;
    pub const ALT: u8 = 4;
    pub const GUI: u8 = 8;
    pub const GLOBE: u8 = 16;
}

/// Where a rule applies: everywhere, in terminals (the host's class list:
/// kitty, foot, Alacritty, wezterm, ghostty and SodiumOS's terminal), or in
/// one app by its Wayland app ID / X11 class.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum KeyCondition {
    Any,
    Terminal,
    Class(String),
}

/// Modifiers (`mods` bits) plus one key: a HID usage, or a modifier's own
/// usage (or `USAGE_GLOBE`) for a modifier remap.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct KeyChord {
    pub mods: u8,
    pub key: u16,
}

/// One row of the table: in `when`, `from` is sent as `to`, a sequence in
/// which every chord but the last is tapped and the last is held while `from`
/// is. Modifiers held beyond `from.mods` (Shift, for selecting) carry over.
/// A rule whose `from` is a bare modifier (or Globe) and whose `to` is one bare
/// modifier remaps that modifier everywhere (Cmd → Ctrl); an empty `to` drops
/// the key.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct KeyRule {
    pub when: KeyCondition,
    pub from: KeyChord,
    pub to: Vec<KeyChord>,
}

impl KeyRule {
    fn new(when: KeyCondition, from: (u8, u16), to: &[(u8, u16)]) -> KeyRule {
        KeyRule {
            when,
            from: KeyChord { mods: from.0, key: from.1 },
            to: to.iter().map(|&(mods, key)| KeyChord { mods, key }).collect(),
        }
    }

    /// the "Mac behaviour" table: the Mac client's default, editable there.
    pub fn mac_defaults() -> Vec<KeyRule> {
        use KeyCondition::{Any, Terminal};
        use mods::*;
        use usage::*;
        let r = KeyRule::new;
        vec![
            // Cmd acts as Ctrl; Globe is Win; Option is already Alt.
            r(Any, (0, LEFT_GUI), &[(0, LEFT_CTRL)]),
            r(Any, (0, RIGHT_GUI), &[(0, LEFT_CTRL)]),
            r(Any, (0, USAGE_GLOBE), &[(0, LEFT_GUI)]),
            // Copy and paste in a terminal, not interrupt.
            r(Terminal, (GUI, C), &[(CTRL | SHIFT, C)]),
            r(Terminal, (GUI, V), &[(CTRL | SHIFT, V)]),
            r(Any, (GUI, SPACE), &[(GUI, S)]),
            r(Any, (GUI, TAB), &[(ALT, TAB)]),
            r(Any, (GUI, Q), &[(ALT, F4)]),
            r(Any, (GUI, LEFT), &[(0, HOME)]),
            r(Any, (GUI, RIGHT), &[(0, END)]),
            r(Any, (GUI, UP), &[(CTRL, HOME)]),
            r(Any, (GUI, DOWN), &[(CTRL, END)]),
            r(Any, (ALT, LEFT), &[(CTRL, LEFT)]),
            r(Any, (ALT, RIGHT), &[(CTRL, RIGHT)]),
            r(Any, (ALT, BACKSPACE), &[(CTRL, BACKSPACE)]),
            r(Any, (GUI, BACKSPACE), &[(SHIFT, HOME), (0, BACKSPACE)]),
        ]
    }
}

// ---------------------------------------------------------------------------
// Phase lock

/// Where frames land against the client's display refresh, over about the
/// last second. Times are the client's session clock (µs).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct VsyncReport {
    /// A recent vsync (a presentation time) on the client's session clock.
    pub vsync_us: u64,
    /// The display's refresh period, ns.
    pub period_ns: u32,
    /// How long before the presenter's deadline for the vsync it was shown at
    /// each frame was ready to show (decoded, on the presenter's side), modulo
    /// the period: the median and the 5th percentile, µs. The deadline is the
    /// time by which the client must have drawn to make that vsync (the Mac's
    /// display link `targetTimestamp`; the vsync itself where the compositor
    /// renders at the vsync, as on the nested headless output). The host moves
    /// REMOTE-1's frame clock so the median sits at its margin; the 5th
    /// percentile's slack becomes `grace_ns`. (integration.md 9)
    pub lead_p50_us: i32,
    pub lead_p05_us: i32,
    /// Frames these come from (0: nothing shown).
    pub frames: u32,
}

impl DisconnectReason {
    /// For `VersionMismatch`: which side needs the newer release.
    pub fn update_side(&self) -> Option<UpdateSide> {
        match self {
            DisconnectReason::VersionMismatch { host, client } => {
                Some(if host > client { UpdateSide::Client } else { UpdateSide::Host })
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Signalling (sealed end to end; the rendezvous and the LAN only carry it)

/// The plaintext inside a sealed signalling blob.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Signal {
    /// Client → host: start a session.
    Offer(Offer),
    /// Host → client: another client is connected right now. Ask "In use by
    /// <name>. Take over?" and send a new `Offer` with `takeover` set to go on.
    Busy { by: String },
    /// Host → client: accepted; here are the host's ICE credentials.
    Answer(Answer),
    /// Either way: one more ICE candidate (trickle).
    Candidate(String),
    /// Host → client: refused (not paired any more, version, shutting down).
    Refused(DisconnectReason),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Offer {
    /// The highest version the client speaks.
    pub version: u16,
    /// The client's display name, for "In use by" and "Taken over by".
    pub name: String,
    /// The user agreed to take over a live session.
    pub takeover: bool,
    pub ice_ufrag: String,
    pub ice_pass: String,
    pub candidates: Vec<String>,
    /// The lowest version the client speaks (v6 on). After the v5 fields, so
    /// a v5 host still reads the rest and refuses with the right words.
    pub min_version: u16,
    /// When the client made this offer, in Unix µs, higher with every offer it makes
    /// ([`Offer::now_us`]). The host takes an offer only when it is newer than the last
    /// one it took from the same client, so a recorded offer can't be played again
    /// (as the Noise IK first message's timestamp). Last (v6).
    pub sent_us: u64,
}

impl Offer {
    /// The time for a new offer: Unix µs, and always above the last one this
    /// process made, whatever the clock does.
    pub fn now_us() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
        static LAST: AtomicU64 = AtomicU64::new(0);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as u64);
        let mut prev = LAST.load(SeqCst);
        loop {
            let next = now.max(prev + 1);
            match LAST.compare_exchange_weak(prev, next, SeqCst, SeqCst) {
                Ok(_) => return next,
                Err(p) => prev = p,
            }
        }
    }

    pub fn versions(&self) -> VersionRange {
        VersionRange::from_peer(self.version, self.min_version)
    }
}

impl Hello {
    pub fn versions(&self) -> VersionRange {
        VersionRange::from_peer(self.version, self.min_version)
    }

    /// The version the session runs at, which `Welcome.version` names (the host
    /// has already refused a client with no common version, so the fallback is
    /// only for a `Hello` that skipped that check).
    pub fn negotiated(&self) -> u16 {
        negotiate(VersionRange::OURS, self.versions()).unwrap_or(VERSION)
    }
}

/// A v5 client's `Offer` (no `min_version`), read only to refuse it properly.
#[derive(Deserialize)]
enum LegacySignal {
    Offer { version: u16, name: String, takeover: bool, ice_ufrag: String, ice_pass: String, candidates: Vec<String> },
}

impl Signal {
    /// Decode a signal; a pre-v6 `Offer` comes back with `min_version = version` and
    /// `sent_us = 0` (only to be refused: by its version, or as not fresh).
    pub fn decode(b: &[u8]) -> Option<Signal> {
        decode(b).or_else(|| match decode::<LegacySignal>(b)? {
            LegacySignal::Offer { version, name, takeover, ice_ufrag, ice_pass, candidates } => {
                Some(Signal::Offer(Offer { version, name, takeover, ice_ufrag, ice_pass, candidates, min_version: version, sent_us: 0 }))
            }
        })
    }
}

/// The start of every `Hello` since v6 (and the first field before): enough to
/// refuse a client whose `Hello` this build can't decode.
#[derive(Deserialize)]
enum HelloHead {
    Hello { version: u16 },
}

/// The version a control message says it is, when it is a `Hello` (even one this
/// build can't decode in full).
pub fn hello_version(b: &[u8]) -> Option<u16> {
    match decode::<HelloHead>(b)? {
        HelloHead::Hello { version } => Some(version),
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Answer {
    pub ice_ufrag: String,
    pub ice_pass: String,
    pub candidates: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_round_trips_compactly() {
        let hello = Msg::Hello(Hello {
            version: VERSION,
            min_version: MIN_VERSION,
            name: "MacBook Air".into(),
            display: ClientDisplay {
                width: 2560,
                height: 1664,
                pixels_per_point: 1.741,
                refresh_mhz: 60000,
                fullscreen: true,
                display_width: 2560,
                display_height: 1664,
            },
            decode: DecodeSupport { hevc: true, h264: true, hevc_444: true, av1: true, max_width: 0, max_height: 0 },
            settings: StreamSettings::default(),
            client_os: Os::Mac,
            client_os_flavor: String::new(),
        });
        let b = hello.encode();
        assert!(b.len() < 60, "{} bytes", b.len());
        assert_eq!(Msg::decode(&b), Some(hello));
    }

    #[test]
    fn a_message_from_before_os_flavor_still_decodes() {
        let mut hello = Hello {
            version: VERSION,
            min_version: MIN_VERSION,
            name: "PC".into(),
            display: ClientDisplay { width: 1, height: 1, pixels_per_point: 1.0, refresh_mhz: 60000, fullscreen: false, display_width: 1, display_height: 1 },
            decode: DecodeSupport { hevc: false, h264: true, hevc_444: false, av1: false, max_width: 0, max_height: 0 },
            settings: StreamSettings::default(),
            client_os: Os::Linux,
            client_os_flavor: String::new(),
        };
        // An older build's Hello is this one without its last field (an empty string, one byte).
        let mut old = Msg::Hello(hello.clone()).encode();
        assert_eq!(old.pop(), Some(0));
        assert_eq!(Msg::decode(&old), Some(Msg::Hello(hello.clone())));
        hello.client_os_flavor = "sodiumos".into();
        let m = Msg::Hello(hello);
        assert_eq!(Msg::decode(&m.encode()), Some(m));
        // Other truncated messages stay refused.
        let mut cut = Msg::SelectDisplay(300).encode();
        cut.pop();
        assert_eq!(Msg::decode(&cut), None);
    }

    #[test]
    fn a_cursor_image_must_match_its_size() {
        let img = |w: u32, h: u32, n: usize| CursorImage { width: w, height: h, hot_x: 0, hot_y: 0, scale: 1.0, pixels: vec![0; n] };
        assert!(img(256, 256, 256 * 256 * 4).is_valid());
        assert!(!img(257, 1, 257 * 4).is_valid(), "a side over 256");
        assert!(!img(0, 0, 0).is_valid());
        assert!(!img(4, 4, 63).is_valid(), "short pixels");
        assert!(!img(u32::MAX, u32::MAX, 0).is_valid(), "no overflow");
        let big = img(512, 128, 512 * 128 * 4).fitted().unwrap();
        assert_eq!((big.width, big.height), (256, 64));
        assert!(big.is_valid());
    }

    #[test]
    fn a_clipboard_is_capped_and_its_chunks_checked() {
        let item = |format: ClipFormat, size: u64, inline: Option<Vec<u8>>| ClipItem { format, size, inline };
        // Duplicate formats, a declared size past the cap and an inline item of the wrong length are skipped.
        let o = ClipboardOffer {
            serial: 7,
            concealed: false,
            items: vec![
                item(ClipFormat::Text, 4, None),
                item(ClipFormat::Text, 4, None),
                item(ClipFormat::Png, u64::MAX, None),
                item(ClipFormat::Html, 3, Some(vec![1, 2])),
            ],
        };
        let (mut inc, asks) = ClipIncoming::from_offer(&o, |_| true).unwrap();
        assert_eq!(asks, vec![ClipFormat::Text]);
        let chunk = |offset: u64, bytes: &[u8]| ClipboardData { serial: 7, format: ClipFormat::Text, offset, total: 4, bytes: bytes.to_vec() };
        assert!(!inc.add(&chunk(1, b"x")), "a chunk out of order");
        assert!(!inc.add(&chunk(0, b"hello")), "a chunk past the item's size");
        assert!(inc.add(&chunk(0, b"he")) && inc.add(&chunk(2, b"ll")));
        assert!(inc.is_complete());
        assert_eq!(inc.into_items(), vec![(ClipFormat::Text, b"hell".to_vec())]);
        // All items together stay under CLIPBOARD_MAX.
        let half = CLIPBOARD_MAX as u64 / 2 + 1;
        let o = ClipboardOffer { serial: 8, concealed: false, items: vec![item(ClipFormat::Png, half, None), item(ClipFormat::Jpeg, half, None)] };
        assert_eq!(ClipIncoming::from_offer(&o, |_| true).unwrap().1, vec![ClipFormat::Png]);
    }

    fn offer(version: u16, min_version: u16) -> Offer {
        Offer { version, name: "Air".into(), takeover: false, ice_ufrag: "u".into(), ice_pass: "p".into(), candidates: vec![], min_version, sent_us: Offer::now_us() }
    }

    /// The v5 `Signal`, as a v5 build declares it.
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    enum V5Signal {
        Offer { version: u16, name: String, takeover: bool, ice_ufrag: String, ice_pass: String, candidates: Vec<String> },
    }

    #[test]
    fn versions_6_and_6_agree_on_6() {
        let o = Signal::decode(&encode(&Signal::Offer(offer(VERSION, MIN_VERSION)))).unwrap();
        let Signal::Offer(o) = o else { panic!() };
        assert_eq!(negotiate(VersionRange::OURS, o.versions()), Ok(6));
    }

    #[test]
    fn a_v5_peer_is_refused_with_words_naming_the_side_to_update() {
        // A v5 client's Offer reaches this (v6) host: read it anyway, refuse, "update the client".
        let v5 = encode(&V5Signal::Offer {
            version: 5,
            name: "Air".into(),
            takeover: false,
            ice_ufrag: "u".into(),
            ice_pass: "p".into(),
            candidates: vec![],
        });
        let Some(Signal::Offer(o)) = Signal::decode(&v5) else { panic!("a v5 offer must still be read") };
        let r = negotiate(VersionRange::OURS, o.versions()).unwrap_err();
        assert_eq!(r, DisconnectReason::VersionMismatch { host: 6, client: 5 });
        assert_eq!(r.update_side(), Some(UpdateSide::Client));

        // This build's Offer reaches a v5 host: it reads its v5 fields (min_version is
        // trailing), sees version 6 != 5 and refuses with {host: 5, client: 6}: "update the host".
        let Ok(V5Signal::Offer { version, name, .. }) = postcard::from_bytes::<V5Signal>(&encode(&Signal::Offer(offer(6, 6)))) else {
            panic!("a v5 host must read a v6 offer")
        };
        assert_eq!((version, name.as_str()), (6, "Air"));
        assert_eq!(DisconnectReason::VersionMismatch { host: 5, client: version }.update_side(), Some(UpdateSide::Host));

        // A v5 Hello that slipped through is still recognised as version 5.
        let mut v5_hello = vec![0u8, 5];
        v5_hello.extend_from_slice(&encode(&"Air".to_string()));
        assert_eq!(hello_version(&v5_hello), Some(5));
        assert_eq!(hello_version(&Msg::KeyframeRequest.encode()), None);
    }

    #[test]
    fn a_v7_peer_and_this_build_agree_on_6() {
        // A v7 build speaks 6 and 7 and may append fields to Offer.
        #[derive(Serialize)]
        struct V7Offer {
            o: Offer,
            later_field: u32,
        }
        let mut b = encode(&0u8); // Signal::Offer's variant index
        b.extend_from_slice(&encode(&V7Offer { o: offer(7, 6), later_field: 99 }));
        let Some(Signal::Offer(o)) = Signal::decode(&b) else { panic!("a v7 offer must be read") };
        assert_eq!(o.versions(), VersionRange { min: 6, max: 7 });
        assert_eq!(negotiate(VersionRange::OURS, o.versions()), Ok(6));
        // A v7 host and this client.
        assert_eq!(negotiate(VersionRange { min: 6, max: 7 }, VersionRange::OURS), Ok(6));
        // Ranges that don't meet: v8 (7..=8) and v6 (6..=6) name the older side.
        let r = negotiate(VersionRange { min: 7, max: 8 }, VersionRange::OURS).unwrap_err();
        assert_eq!(r, DisconnectReason::VersionMismatch { host: 7, client: 6 });
        assert_eq!(r.update_side(), Some(UpdateSide::Client));
        let r = negotiate(VersionRange::OURS, VersionRange { min: 7, max: 8 }).unwrap_err();
        assert_eq!(r, DisconnectReason::VersionMismatch { host: 6, client: 8 });
        assert_eq!(r.update_side(), Some(UpdateSide::Host));
    }

    #[test]
    fn v5_welcome_and_new_messages_round_trip() {
        let w = Msg::Welcome(Welcome {
            version: VERSION,
            name: "PC".into(),
            format: StreamFormat { width: 1920, height: 1080, scale: 1.0, refresh_mhz: 60000, codec: Codec::H264 },
            scales: vec![],
            home_monitor: None,
            locked: false,
            took_over_from: None,
            resumed: false,
            host_os: Os::Windows,
            display_mode: DisplayMode::Physical,
            live_resize: false,
            displays: vec![DisplayInfo { id: 7, name: "DELL".into(), width: 1920, height: 1080, scale: 1.0, primary: true }],
            current_display: 7,
            view_only: true,
            host_os_flavor: String::new(),
        });
        assert_eq!(Msg::decode(&w.encode()), Some(w));
        for m in [Msg::SelectDisplay(7), Msg::SecureScreen(true)] {
            assert_eq!(Msg::decode(&m.encode()), Some(m));
        }
    }

    #[test]
    fn h264_is_used_only_when_the_host_encodes_it_and_hevc_is_not_shared() {
        let d = |hevc, h264| Some(DecodeSupport { hevc, h264, hevc_444: false, av1: false, max_width: 0, max_height: 0 });
        // H.264-only client, host with both: H.264. Host without H.264: no.
        assert!(DecodeSupport::use_h264(d(false, true), true, true));
        assert!(!DecodeSupport::use_h264(d(false, true), true, false));
        // HEVC wins when both sides have it.
        assert!(!DecodeSupport::use_h264(d(true, true), true, true));
        assert!(!DecodeSupport::use_h264(None, true, true));
    }

    #[test]
    fn a_newer_peers_message_is_skipped() {
        // A variant index past the last one we know (what a newer peer's new
        // message looks like to us).
        assert_eq!(Msg::decode(&[200, 1]), None);
        assert_eq!(InputEvent::decode_at(&[5, 99]), None);
        let key = InputEvent::Key { usage: 0x04, down: true };
        assert_eq!(InputEvent::decode_at(&key.encode_at(1234)), Some((1234, key)));
    }

    #[test]
    fn clipboard_chunks_cover_the_item_exactly() {
        let data = vec![7u8; CLIPBOARD_CHUNK * 2 + 1];
        let msgs = ClipboardData::chunks(3, &ClipFormat::Png, &data);
        assert_eq!(msgs.len(), 3);
        let Msg::ClipboardData(last) = &msgs[2] else { panic!() };
        assert!(last.is_last());
        assert_eq!(last.offset as usize + last.bytes.len(), data.len());
    }

    #[test]
    fn long_commits_split_on_character_boundaries() {
        let text = "你".repeat(INPUT_TEXT_MAX); // 3 bytes each
        let parts = InputEvent::commit_chunks(&text);
        let joined: String = parts
            .iter()
            .map(|e| match e {
                InputEvent::ImeCommit { text } => {
                    assert!(text.len() <= INPUT_TEXT_MAX);
                    text.as_str()
                }
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(joined, text);
    }
}
