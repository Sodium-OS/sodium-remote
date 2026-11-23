// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The Wayland clipboard for clipboard sync, through ext-data-control-v1
//! (Hyprland has it since 0.49), on a thread of its own with its own Wayland
//! connection. remoted uses it on the host; the SodiumOS client
//! (`remote-client`'s `clip`) on the controlling side.
//!
//! - **Out:** every new selection that isn't ours is read in the formats the
//!   other side takes (text, rich text, PNG) and handed to `on_change`.
//! - **In:** [`Clipboard::set`] offers the other side's entry as the selection and
//!   serves it to whoever pastes.
//! - **Sensitive entries:** a password manager marks its copies with the MIME type
//!   `x-kde-passwordManagerHint` = `secret`. Such an entry is sent with
//!   `concealed` set (the Mac pastes it as `org.nspasteboard.ConcealedType`), and a
//!   concealed entry from the other side is offered here with the same hint, so the
//!   shell's clipboard history skips it. Neither side writes clipboard data
//!   anywhere or logs it; a concealed entry served here is held in memory only
//!   while it owns the selection, and wiped when it loses it.
//! - Our own offers carry a private marker type, so they are never echoed back.
//! - The primary selection is ignored (middle click never pastes).

use std::{
    collections::HashMap,
    io::{Read, Write},
    os::fd::{AsFd, OwnedFd},
    sync::{Arc, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use rustix::event::{PollFd, PollFlags, poll};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child,
    backend::ObjectId,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
    ext_data_control_source_v1::{self, ExtDataControlSourceV1},
};

pub const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";
/// Marks offers made here.
/// Marks our own offers (`brand::CLIPBOARD_MIME`); the pre-rename marker still counts as ours.
const OURS: &str = remote_core::brand::CLIPBOARD_MIME;
const OURS_OLD: &str = remote_core::brand::OLD_CLIPBOARD_MIME;
const MAX_BYTES: usize = remote_core::msg::CLIPBOARD_MAX;

/// The formats synced, in order of preference within each kind.
const TEXT: &[&str] = &["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"];
const RICH: &[&str] = &["text/html"];
const IMAGE: &[&str] = &["image/png"];

/// One clipboard entry: each format's bytes, and whether it is sensitive.
#[derive(Clone, PartialEq)]
pub struct ClipEntry {
    /// MIME type → data. Text is always under `text/plain;charset=utf-8`.
    pub items: Vec<(String, Vec<u8>)>,
    pub concealed: bool,
}

impl std::fmt::Debug for ClipEntry {
    // Never the content: logs keep metadata only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kinds: Vec<_> = self.items.iter().map(|(m, d)| format!("{m}:{}B", d.len())).collect();
        write!(f, "ClipEntry({kinds:?}, concealed: {})", self.concealed)
    }
}

impl ClipEntry {
    fn wipe(&mut self) {
        for (_, d) in &mut self.items {
            d.fill(0);
        }
        self.items.clear();
    }
}

enum Cmd {
    Set(ClipEntry),
    Quit,
}

pub struct Clipboard {
    tx: mpsc::Sender<Cmd>,
    wake: Arc<OwnedFd>,
    thread: Option<JoinHandle<()>>,
}

impl Clipboard {
    /// Connects to `$WAYLAND_DISPLAY`. `on_change` runs on the clipboard thread.
    pub fn start(on_change: impl FnMut(ClipEntry) + Send + 'static) -> Result<Self> {
        let conn = Connection::connect_to_env().context("connecting to $WAYLAND_DISPLAY")?;
        let (tx, rx) = mpsc::channel();
        let wake = Arc::new(rustix::event::eventfd(0, rustix::event::EventfdFlags::CLOEXEC | rustix::event::EventfdFlags::NONBLOCK)?);
        let (ready_tx, ready_rx) = mpsc::channel();
        let w = wake.clone();
        let thread = std::thread::Builder::new().name("remote-clipboard".into()).spawn(move || {
            if let Err(e) = run(conn, rx, w, Box::new(on_change), ready_tx.clone()) {
                eprintln!("remote: clipboard: {e:#}");
                let _ = ready_tx.send(Err(e.to_string()));
            }
        })?;
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Clipboard { tx, wake, thread: Some(thread) }),
            Ok(Err(e)) => anyhow::bail!(e),
            Err(_) => anyhow::bail!("the clipboard thread didn't start"),
        }
    }

    /// Makes `entry` the host's selection.
    pub fn set(&self, entry: ClipEntry) {
        let _ = self.tx.send(Cmd::Set(entry));
        let _ = rustix::io::write(self.wake.as_fd(), &1u64.to_ne_bytes());
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
        let _ = rustix::io::write(self.wake.as_fd(), &1u64.to_ne_bytes());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct State {
    offers: HashMap<ObjectId, Vec<String>>,
    selection: Option<ExtDataControlOfferV1>,
    selection_changed: bool,
    source: Option<(ExtDataControlSourceV1, Arc<std::sync::Mutex<ClipEntry>>)>,
    finished: bool,
}

type OnChange = Box<dyn FnMut(ClipEntry) + Send>;

fn run(
    conn: Connection,
    rx: mpsc::Receiver<Cmd>,
    wake: Arc<OwnedFd>,
    mut on_change: OnChange,
    ready: mpsc::Sender<std::result::Result<(), String>>,
) -> Result<()> {
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=8, ()).context("no wl_seat")?;
    let mgr: ExtDataControlManagerV1 =
        globals.bind(&qh, 1..=1, ()).context("the compositor lacks ext_data_control_manager_v1")?;
    let device = mgr.get_data_device(&seat, &qh, ());
    let mut st = State { offers: HashMap::new(), selection: None, selection_changed: false, source: None, finished: false };
    queue.roundtrip(&mut st)?;
    // The selection present at start is not a change.
    st.selection_changed = false;
    let _ = ready.send(Ok(()));

    loop {
        queue.flush()?;
        queue.dispatch_pending(&mut st)?;
        if st.finished {
            return Ok(());
        }
        if st.selection_changed {
            st.selection_changed = false;
            if let Some(offer) = st.selection.clone() {
                let mimes = st.offers.get(&offer.id()).cloned().unwrap_or_default();
                if !mimes.iter().any(|m| m == OURS || m == OURS_OLD)
                    && let Some(entry) = read_offer(&conn, &mut queue, &mut st, &offer, &mimes)?
                {
                    on_change(entry);
                }
            }
        }
        // No guard means events are already queued: dispatch them first.
        if let Some(g) = queue.prepare_read() {
            let fd = g.connection_fd().try_clone_to_owned()?;
            let mut both = [PollFd::new(wake.as_ref(), PollFlags::IN), PollFd::new(&fd, PollFlags::IN)];
            poll(&mut both, None)?;
            if !both[1].revents().is_empty() {
                match g.read() {
                    Ok(_) => {}
                    Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        let mut buf = [0u8; 8];
        let _ = rustix::io::read(wake.as_fd(), &mut buf);
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Quit => return Ok(()),
                Cmd::Set(entry) => {
                    let src = mgr.create_data_source(&qh, ());
                    for (m, _) in &entry.items {
                        src.offer(m.clone());
                        if m == "text/plain;charset=utf-8" {
                            for alias in &TEXT[1..] {
                                src.offer(alias.to_string());
                            }
                        }
                    }
                    if entry.concealed {
                        src.offer(PASSWORD_HINT.into());
                    }
                    src.offer(OURS.into());
                    device.set_selection(Some(&src));
                    if let Some((old, data)) = st.source.replace((src, Arc::new(std::sync::Mutex::new(entry)))) {
                        old.destroy();
                        data.lock().unwrap().wipe();
                    }
                }
            }
        }
    }
}

/// Reads an offer in the synced formats. None if it has none of them.
fn read_offer(
    conn: &Connection,
    queue: &mut wayland_client::EventQueue<State>,
    st: &mut State,
    offer: &ExtDataControlOfferV1,
    mimes: &[String],
) -> Result<Option<ClipEntry>> {
    let has = |m: &str| mimes.iter().any(|x| x == m);
    let mut entry = ClipEntry { items: Vec::new(), concealed: false };
    if has(PASSWORD_HINT) {
        let hint = receive(conn, queue, st, offer, PASSWORD_HINT)?;
        entry.concealed = hint.as_deref().is_some_and(|h| h.trim_ascii() == b"secret");
    }
    for (group, canonical) in [(TEXT, "text/plain;charset=utf-8"), (RICH, "text/html"), (IMAGE, "image/png")] {
        if let Some(m) = group.iter().find(|m| has(m))
            && let Some(data) = receive(conn, queue, st, offer, m)?
        {
            entry.items.push((canonical.to_string(), data));
        }
    }
    Ok((!entry.items.is_empty()).then_some(entry))
}

fn receive(
    conn: &Connection,
    queue: &mut wayland_client::EventQueue<State>,
    st: &mut State,
    offer: &ExtDataControlOfferV1,
    mime: &str,
) -> Result<Option<Vec<u8>>> {
    let (r, w) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
    offer.receive(mime.into(), w.as_fd());
    drop(w);
    conn.flush()?;
    let _ = queue.dispatch_pending(st);
    let mut f = std::fs::File::from(r);
    rustix::fs::fcntl_setfl(&f, rustix::fs::OFlags::NONBLOCK)?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut out = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None); // a source that never answers
        }
        let mut fds = [PollFd::new(&f, PollFlags::IN)];
        let ts = rustix::time::Timespec { tv_sec: left.as_secs() as i64, tv_nsec: left.subsec_nanos() as i64 };
        poll(&mut fds, Some(&ts))?;
        match f.read(&mut buf) {
            Ok(0) => return Ok(Some(out)),
            Ok(n) => {
                out.extend_from_slice(&buf[..n]);
                if out.len() > MAX_BYTES {
                    return Ok(None);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlSeat, ()> for State {
    fn event(_: &mut Self, _: &WlSeat, _: <WlSeat as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ExtDataControlManagerV1, ()> for State {
    fn event(_: &mut Self, _: &ExtDataControlManagerV1, _: <ExtDataControlManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(st: &mut Self, _: &ExtDataControlDeviceV1, ev: ext_data_control_device_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            ext_data_control_device_v1::Event::DataOffer { id } => {
                st.offers.insert(id.id(), Vec::new());
            }
            ext_data_control_device_v1::Event::Selection { id } => {
                if let Some(old) = st.selection.take() {
                    st.offers.remove(&old.id());
                    old.destroy();
                }
                st.selection = id;
                st.selection_changed = st.selection.is_some();
            }
            ext_data_control_device_v1::Event::PrimarySelection { id: Some(o) } => {
                st.offers.remove(&o.id());
                o.destroy();
            }
            ext_data_control_device_v1::Event::Finished => st.finished = true,
            _ => {}
        }
    }

    event_created_child!(State, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for State {
    fn event(st: &mut Self, offer: &ExtDataControlOfferV1, ev: ext_data_control_offer_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = ev {
            st.offers.entry(offer.id()).or_default().push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, ()> for State {
    fn event(st: &mut Self, src: &ExtDataControlSourceV1, ev: ext_data_control_source_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match ev {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                let Some((ours, data)) = &st.source else { return };
                if ours != src {
                    return;
                }
                let data = data.clone();
                // A slow reader must never block the clipboard thread.
                std::thread::spawn(move || {
                    let e = data.lock().unwrap();
                    let want = if TEXT.contains(&mime_type.as_str()) { "text/plain;charset=utf-8" } else { mime_type.as_str() };
                    let bytes: &[u8] = if mime_type == PASSWORD_HINT {
                        b"secret"
                    } else {
                        e.items.iter().find(|(m, _)| m == want).map_or(&[], |(_, d)| d.as_slice())
                    };
                    let _ = std::fs::File::from(fd).write_all(bytes);
                });
            }
            ext_data_control_source_v1::Event::Cancelled if st.source.as_ref().is_some_and(|(s, _)| s == src) => {
                let (s, data) = st.source.take().unwrap();
                data.lock().unwrap().wipe();
                s.destroy();
            }
            _ => {}
        }
    }
}
