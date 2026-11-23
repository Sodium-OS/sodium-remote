// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The portal session: RemoteDesktop with
//! ScreenCast sources and the Clipboard, on a thread of its own with its own tokio
//! runtime, so the backend's blocking calls work from any thread (the session's tokio
//! tasks included).
//!
//! - **Start** (a device connects): CreateSession, SelectDevices (keyboard and pointer,
//!   persist mode "until revoked", the saved restore token), SelectSources (one
//!   monitor, the cursor as metadata), RequestClipboard, Start. The first Start shows
//!   the system's dialog; the token it returns is saved (`names::RESTORE_TOKEN`, 0600)
//!   and later Starts use it, so no dialog shows. Tokens are single-use: each Start
//!   saves the new one.
//! - Then OpenPipeWireRemote (one per capture: a PipeWire connection can't share a
//!   socket), ConnectToEIS (RemoteDesktop v2) for libei, else the Notify methods.
//! - **End**: the session closes when the device's session ends (`stop`). When the
//!   person at the computer stops sharing, the portal sends `Closed`: the capture ends
//!   with `SharingEnded` and the device sees "Ended at the computer".

use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::mpsc as smpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use ashpd::desktop::clipboard::{Clipboard as ClipPortal, SetSelectionOptions};
use ashpd::desktop::remote_desktop::{Axis as PAxis, DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::{PersistMode, Session};
use futures_util::StreamExt;
use remote_core::msg::{self, ClipFormat};
use tokio::sync::mpsc;

use super::input::Ei;
use super::names;
use crate::backend::{ClipEntry, ClipSink, Consent, Display, NotReady};
use remote_linux_media::video::VideoCapture;

/// How long a Start may wait for the person at the computer.
const DIALOG_TIMEOUT: Duration = Duration::from_secs(180);

/// The running portal session, as the backend sees it.
#[derive(Clone)]
pub struct Active {
    /// Counts sessions, so a capture of an old one knows it ended.
    pub generation: u64,
    /// The ScreenCast stream: its PipeWire node, its place and size in the
    /// compositor's (logical) coordinates, its mapping ID for libei regions.
    pub node: u32,
    pub position: (i32, i32),
    pub logical: (i32, i32),
    pub mapping_id: Option<String>,
    /// The stream's size in pixels (PipeWire's format), once known.
    pub pixels: (u32, u32),
    pub ei: Option<Arc<Ei>>,
    /// The person at the computer didn't allow remote interaction (GNOME's dialog has a
    /// switch for it): the picture only.
    pub view_only: bool,
}

impl Active {
    /// Display pixels → the stream's logical coordinates (relative to the stream).
    pub fn to_logical(&self, x: f64, y: f64) -> (f64, f64) {
        let (lw, lh) = (self.logical.0.max(1) as f64, self.logical.1.max(1) as f64);
        let (pw, ph) = (self.pixels.0.max(1) as f64, self.pixels.1.max(1) as f64);
        (x * lw / pw, y * lh / ph)
    }
}

/// A portal Notify call (the fallback when there is no libei).
#[derive(Clone, Debug)]
pub enum Notify {
    Key { code: i32, down: bool },
    Keysym { sym: i32, down: bool },
    /// Logical coordinates relative to the stream.
    Abs { x: f64, y: f64 },
    Rel { dx: f64, dy: f64 },
    Button { code: i32, down: bool },
    Axis { dx: f64, dy: f64, finish: bool },
    Discrete { vertical: bool, steps: i32 },
}

enum Cmd {
    Start { reply: smpsc::Sender<Result<Active, NotReady>> },
    Stop,
    Remote { reply: smpsc::Sender<Result<OwnedFd>> },
    Notify(Notify),
    ClipOffer(Vec<String>),
    /// Whether the computer is locked (`lock::is_locked`).
    Locked { reply: smpsc::Sender<bool> },
}

#[derive(Default)]
struct State {
    consent: Option<Consent>,
    active: Option<Active>,
    generation: u64,
    /// The generation whose sharing was stopped at the computer.
    ended: u64,
    /// The capture opened at Start to learn the size, for the first `open_capture`.
    prepared: Option<(u64, bool, VideoCapture)>,
    clip_sink: Option<ClipSink>,
    /// The computer's clipboard as last read.
    clip_in: Option<ClipEntry>,
    /// What this host put on the computer's clipboard (the device's), for transfers.
    clip_out: Option<ClipEntry>,
}

type Job = Box<dyn FnOnce() + Send>;

/// The one runtime every `Portal` of this process runs on. ashpd keeps a single D-Bus
/// connection for the whole process, and zbus ties its tasks to the runtime that was
/// current when it was made: a runtime per `Portal`, dropped when the "This computer"
/// switch goes off, would leave the next `Portal` talking to a dead connection (its
/// calls never return, so no dialog would show).
fn runtime() -> Result<&'static mpsc::UnboundedSender<Job>> {
    static RT: OnceLock<Result<mpsc::UnboundedSender<Job>, String>> = OnceLock::new();
    RT.get_or_init(|| {
        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        std::thread::Builder::new()
            .name("portal".into())
            .spawn(move || {
                let local = tokio::task::LocalSet::new();
                rt.block_on(local.run_until(async move {
                    while let Some(job) = rx.recv().await {
                        job();
                    }
                }));
            })
            .map_err(|e| e.to_string())?;
        Ok(tx)
    })
    .as_ref()
    .map_err(|e| anyhow!("the portal thread didn't start: {e}"))
}

pub struct Portal {
    data: PathBuf,
    tx: mpsc::UnboundedSender<Cmd>,
    st: Arc<Mutex<State>>,
}

fn token_path(data: &std::path::Path) -> PathBuf {
    data.join(names::RESTORE_TOKEN)
}

fn read_token(data: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(token_path(data)).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

fn save_token(data: &std::path::Path, token: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::create_dir_all(data);
    let tmp = token_path(data).with_extension("tmp");
    let ok = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(token.as_bytes()))
        .and_then(|()| std::fs::rename(&tmp, token_path(data)));
    if let Err(e) = ok {
        eprintln!("sodium-remote-host: can't save the portal's restore token: {e}");
    }
}

impl Portal {
    pub fn start(data: PathBuf) -> Result<Arc<Portal>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let consent = if read_token(&data).is_some() { Consent::Remembered } else { Consent::Ask };
        let st = Arc::new(Mutex::new(State { consent: Some(consent), ..Default::default() }));
        let (st2, data2) = (st.clone(), data.clone());
        runtime()?.send(Box::new(move || {
            tokio::task::spawn_local(run(rx, st2, data2));
        }))
        .map_err(|_| anyhow!("the portal thread stopped"))?;
        Ok(Arc::new(Portal { data, tx, st }))
    }

    pub fn consent(&self) -> Consent {
        self.st.lock().unwrap().consent.clone().unwrap_or(Consent::Ask)
    }

    pub fn forget_consent(&self) {
        let _ = std::fs::remove_file(token_path(&self.data));
        let mut st = self.st.lock().unwrap();
        if matches!(st.consent, Some(Consent::Remembered)) {
            st.consent = Some(Consent::Ask);
        }
    }

    pub fn active(&self) -> Option<Active> {
        self.st.lock().unwrap().active.clone()
    }

    /// Whether the person at the computer stopped sharing session `generation` (or a
    /// newer one replaced it).
    pub fn ended(&self, generation: u64) -> bool {
        let st = self.st.lock().unwrap();
        st.ended >= generation || st.active.as_ref().is_none_or(|a| a.generation != generation)
    }

    /// The one display: the monitor chosen in the dialog.
    pub fn display(&self) -> Option<Display> {
        let a = self.active()?;
        let (w, h) = if a.pixels.0 > 0 { a.pixels } else { (a.logical.0.max(0) as u32, a.logical.1.max(0) as u32) };
        let scale = if a.logical.0 > 0 && w > 0 { w as f32 / a.logical.0 as f32 } else { 1.0 };
        Some(Display { info: msg::DisplayInfo { id: 1, name: String::new(), width: w, height: h, scale, primary: true }, refresh_mhz: 0 })
    }

    /// Starts the portal session unless one runs: the dialog the first time, the
    /// restore token later. Then opens the stream once to learn its size in pixels.
    /// Blocks (the dialog may wait for the person at the computer).
    pub fn ensure_started(&self, dmabuf: bool) -> Result<(), NotReady> {
        if let Some(a) = self.active()
            && !self.ended(a.generation)
        {
            return Ok(());
        }
        let (reply, rx) = smpsc::channel();
        self.tx.send(Cmd::Start { reply }).map_err(|_| NotReady::Other("the portal thread stopped".into()))?;
        let a = match rx.recv_timeout(DIALOG_TIMEOUT + Duration::from_secs(5)) {
            Ok(r) => r?,
            Err(_) => {
                self.stop();
                return Err(NotReady::Other("nobody answered the sharing dialog".into()));
            }
        };
        // The size in pixels comes from PipeWire's format (the portal's is logical).
        match self.remote().and_then(|fd| Ok(VideoCapture::open(fd, a.node, 60, dmabuf)?)) {
            Ok(v) => {
                let until = Instant::now() + Duration::from_secs(3);
                let mut size = None;
                while Instant::now() < until {
                    match v.size() {
                        Ok(Some(s)) => {
                            size = Some(s);
                            break;
                        }
                        Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                        Err(_) => break,
                    }
                }
                let mut st = self.st.lock().unwrap();
                if let Some((w, h, d)) = size
                    && let Some(act) = st.active.as_mut()
                {
                    act.pixels = (w, h);
                    eprintln!(
                        "sodium-remote-host: the stream is {w}x{h} pixels{} ({}x{} logical at {},{})",
                        if d { ", DMA-BUF" } else { ", shared memory" },
                        act.logical.0,
                        act.logical.1,
                        act.position.0,
                        act.position.1
                    );
                }
                st.prepared = Some((a.generation, dmabuf, v));
            }
            Err(e) => eprintln!("sodium-remote-host: the stream didn't open: {e:#}"),
        }
        Ok(())
    }

    /// The capture opened by `ensure_started`, if it matches.
    pub fn take_prepared(&self, generation: u64, dmabuf: bool) -> Option<VideoCapture> {
        let mut st = self.st.lock().unwrap();
        match st.prepared.take() {
            Some((g, d, v)) if g == generation && d == dmabuf => Some(v),
            _ => None,
        }
    }

    /// A new PipeWire remote for a capture (OpenPipeWireRemote).
    pub fn remote(&self) -> Result<OwnedFd> {
        let (reply, rx) = smpsc::channel();
        self.tx.send(Cmd::Remote { reply }).map_err(|_| anyhow!("the portal thread stopped"))?;
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| anyhow!("the portal didn't open a PipeWire remote"))?
    }

    /// Whether the computer is locked, read on the portal's runtime (its D-Bus connections live there).
    pub fn locked(&self) -> bool {
        let (reply, rx) = smpsc::channel();
        self.tx.send(Cmd::Locked { reply }).is_ok() && rx.recv_timeout(Duration::from_secs(3)).unwrap_or(false)
    }

    /// Closes the portal session (the device's session ended).
    pub fn stop(&self) {
        {
            let mut st = self.st.lock().unwrap();
            st.prepared = None;
            if let Some(a) = st.active.take()
                && let Some(ei) = a.ei
            {
                ei.close();
            }
        }
        let _ = self.tx.send(Cmd::Stop);
    }

    pub fn notify(&self, n: Notify) {
        let _ = self.tx.send(Cmd::Notify(n));
    }

    // ---- the clipboard (`clipboard.rs`) ----

    pub fn set_clip_sink(&self, sink: ClipSink) {
        self.st.lock().unwrap().clip_sink = Some(sink);
    }

    pub fn clip_current(&self) -> Option<ClipEntry> {
        self.st.lock().unwrap().clip_in.clone()
    }

    /// Offers the device's clipboard on the computer (SetSelection); the data goes out
    /// on SelectionTransfer.
    pub fn clip_offer(&self, entry: ClipEntry) {
        let mimes = offer_mimes(&entry);
        self.st.lock().unwrap().clip_out = Some(entry);
        let _ = self.tx.send(Cmd::ClipOffer(mimes));
    }
}

// ---- MIME types ----

const TEXT_MIMES: [&str; 5] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"];
/// KDE's (and several password managers') mark for a secret.
const CONCEALED_MIME: &str = "x-kde-passwordManagerHint";

fn offer_mimes(e: &ClipEntry) -> Vec<String> {
    let mut v = vec![];
    for (f, _) in &e.items {
        match f {
            ClipFormat::Text => v.extend(TEXT_MIMES.iter().map(|m| m.to_string())),
            ClipFormat::Html => v.push("text/html".into()),
            ClipFormat::Png => v.push("image/png".into()),
            ClipFormat::Mime(m) => v.push(m.clone()),
            _ => {}
        }
    }
    if e.concealed {
        v.push(CONCEALED_MIME.into());
    }
    v
}

/// The data for a MIME type the computer asked for.
fn data_for(e: &ClipEntry, mime: &str) -> Option<Vec<u8>> {
    if mime == CONCEALED_MIME {
        return e.concealed.then(|| b"secret".to_vec());
    }
    let want = if TEXT_MIMES.contains(&mime) {
        ClipFormat::Text
    } else if mime == "text/html" {
        ClipFormat::Html
    } else if mime == "image/png" {
        ClipFormat::Png
    } else {
        ClipFormat::Mime(mime.to_string())
    };
    e.items.iter().find(|(f, _)| *f == want).map(|(_, d)| d.clone())
}

/// Which of the offered types to read, and as what.
fn wanted(mimes: &[String]) -> Vec<(ClipFormat, String)> {
    let mut v = vec![];
    if let Some(t) = TEXT_MIMES.iter().find(|t| mimes.iter().any(|m| m == *t)) {
        v.push((ClipFormat::Text, t.to_string()));
    }
    if mimes.iter().any(|m| m == "text/html") {
        v.push((ClipFormat::Html, "text/html".into()));
    }
    if mimes.iter().any(|m| m == "image/png") {
        v.push((ClipFormat::Png, "image/png".into()));
    }
    v
}

// ---- the portal thread ----

struct Live {
    generation: u64,
    /// The ScreenCast stream's node (NotifyPointerMotionAbsolute names it).
    node: u32,
    rd: Arc<RemoteDesktop>,
    sc: Screencast,
    cb: Option<Arc<ClipPortal>>,
    session: Arc<Session<RemoteDesktop>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

async fn run(mut rx: mpsc::UnboundedReceiver<Cmd>, st: Arc<Mutex<State>>, data: PathBuf) {
    let mut live: Option<Live> = None;
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::Start { reply } => {
                if let Some(l) = live.take() {
                    close(l).await;
                }
                let generation = {
                    let mut s = st.lock().unwrap();
                    s.generation += 1;
                    s.consent = Some(Consent::Asking);
                    s.generation
                };
                match start_session(&st, &data, generation).await {
                    Ok((l, a)) => {
                        st.lock().unwrap().active = Some(a.clone());
                        live = Some(l);
                        let _ = reply.send(Ok(a));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }
            Cmd::Stop => {
                if let Some(l) = live.take() {
                    close(l).await;
                }
            }
            Cmd::Remote { reply } => {
                let r = match &live {
                    Some(l) => l.sc.open_pipe_wire_remote(&l.session, Default::default()).await.context("OpenPipeWireRemote"),
                    None => Err(anyhow!("no portal session")),
                };
                let _ = reply.send(r);
            }
            Cmd::Notify(n) => {
                if let Some(l) = &live
                    && let Err(e) = notify(l, n).await
                {
                    eprintln!("sodium-remote-host: portal input: {e}");
                }
            }
            Cmd::Locked { reply } => {
                // Off the command loop: the re-read waits.
                tokio::task::spawn_local(async move {
                    let _ = reply.send(super::lock::is_locked().await);
                });
            }
            Cmd::ClipOffer(mimes) => {
                if let Some(l) = &live
                    && let Some(cb) = &l.cb
                {
                    let refs: Vec<&str> = mimes.iter().map(String::as_str).collect();
                    match cb.set_selection(&l.session, SetSelectionOptions::default().set_mime_types(&refs)).await {
                        Ok(()) if std::env::var_os("SODIUM_REMOTE_HOST_DEBUG").is_some() => eprintln!("sodium-remote-host: clipboard offered: {mimes:?}"),
                        Ok(()) => {}
                        Err(e) => eprintln!("sodium-remote-host: portal clipboard: {e}"),
                    }
                }
            }
        }
    }
}

async fn close(l: Live) {
    for t in &l.tasks {
        t.abort();
    }
    let _ = l.session.close().await;
    eprintln!("sodium-remote-host: portal session {} closed", l.generation);
}

fn not_ready(e: ashpd::Error) -> NotReady {
    match e {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => NotReady::Declined,
        e => NotReady::Other(format!("the desktop portal: {e}")),
    }
}

async fn start_session(st: &Arc<Mutex<State>>, data: &std::path::Path, generation: u64) -> Result<(Live, Active), NotReady> {
    let set_consent = |c: Consent| st.lock().unwrap().consent = Some(c);
    let unavailable = |what: &str, e: ashpd::Error| {
        let text = format!("{what}: {e}");
        set_consent(Consent::Unavailable(text.clone()));
        NotReady::Other(format!("this desktop can't share its screen ({text})"))
    };
    let rd = RemoteDesktop::new().await.map_err(|e| unavailable("no RemoteDesktop portal", e))?;
    let sc = Screencast::new().await.map_err(|e| unavailable("no ScreenCast portal", e))?;
    let cb = ClipPortal::new().await.ok().map(Arc::new);
    let token = read_token(data);
    let result: Result<(Live, Active), ashpd::Error> = async {
        let session = Arc::new(rd.create_session(Default::default()).await?);
        rd.select_devices(
            &session,
            SelectDevicesOptions::default()
                .set_devices(DeviceType::KEYBOARD | DeviceType::POINTER)
                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                .set_restore_token(token.as_deref()),
        )
        .await?
        .response()?;
        sc.select_sources(
            &session,
            SelectSourcesOptions::default().set_cursor_mode(CursorMode::METADATA).set_sources(SourceType::MONITOR).set_multiple(false),
        )
        .await?
        .response()?;
        if let Some(cb) = &cb
            && let Err(e) = cb.request(&session, Default::default()).await
        {
            eprintln!("sodium-remote-host: no portal clipboard: {e}");
        }
        eprintln!("sodium-remote-host: starting the portal session{}", if token.is_some() { " with the restore token" } else { " (the sharing dialog)" });
        let started = rd.start(&session, None, Default::default()).await?.response()?;
        match started.restore_token() {
            Some(t) => {
                save_token(data, t);
                set_consent(Consent::Remembered);
            }
            None => {
                let _ = std::fs::remove_file(token_path(data));
                set_consent(Consent::Ask);
            }
        }
        let stream = started.streams().first().cloned().ok_or(ashpd::Error::NoResponse)?;
        // SODIUM_REMOTE_NO_EIS=1 skips ConnectToEIS, so the Notify methods carry the input (a test switch:
        // both GNOME and KDE offer libei, so nothing else reaches that fallback).
        let ei = if std::env::var_os("SODIUM_REMOTE_NO_EIS").is_some() {
            eprintln!("sodium-remote-host: SODIUM_REMOTE_NO_EIS is set; using the portal's Notify methods");
            None
        } else if rd.version() >= 2 {
            match rd.connect_to_eis(&session, Default::default()).await {
                Ok(fd) => match Ei::start(fd) {
                    Ok(ei) => Some(ei),
                    Err(e) => {
                        eprintln!("sodium-remote-host: libei: {e:#}; using the portal's Notify methods");
                        None
                    }
                },
                Err(e) => {
                    eprintln!("sodium-remote-host: ConnectToEIS: {e}; using the portal's Notify methods");
                    None
                }
            }
        } else {
            eprintln!("sodium-remote-host: RemoteDesktop v{} has no ConnectToEIS; using its Notify methods", rd.version());
            None
        };
        let clipboard = started.is_clipboard_enabled();
        let devices = started.devices();
        let input = devices.contains(DeviceType::KEYBOARD) || devices.contains(DeviceType::POINTER);
        let active = Active {
            generation,
            node: stream.pipe_wire_node_id(),
            position: stream.position().unwrap_or((0, 0)),
            logical: stream.size().unwrap_or((0, 0)),
            mapping_id: stream.mapping_id().map(str::to_string),
            pixels: (0, 0),
            ei,
            view_only: !input,
        };
        eprintln!(
            "sodium-remote-host: sharing node {} ({:?}), devices {:?}, {}, clipboard {}",
            active.node,
            stream.id(),
            devices,
            if !input { "view only" } else if active.ei.is_some() { "libei input" } else { "portal input" },
            if clipboard { "on" } else { "off" }
        );
        let rd = Arc::new(rd);
        let mut tasks = vec![];
        // Sharing stopped at the computer.
        {
            let (st, session) = (st.clone(), session.clone());
            tasks.push(tokio::spawn(async move {
                let Ok(closed) = session.receive_closed().await else { return };
                let mut closed = Box::pin(closed);
                if closed.next().await.is_some() {
                    eprintln!("sodium-remote-host: the portal closed the session (sharing stopped at this computer)");
                    let mut s = st.lock().unwrap();
                    s.ended = s.ended.max(generation);
                }
            }));
        }
        let cb = if clipboard { cb } else { None };
        if let Some(cb) = &cb {
            tasks.extend(clipboard_tasks(st.clone(), cb.clone(), session.clone()).await?);
        }
        Ok((Live { generation, node: active.node, rd, sc, cb, session, tasks }, active))
    }
    .await;
    match result {
        Ok(x) => Ok(x),
        Err(e) => {
            let mut nr = not_ready(e);
            // A locked GNOME refuses a new session with a portal error: say "locked" (the token stays).
            if matches!(&nr, NotReady::Other(_)) && super::lock::is_locked().await {
                nr = NotReady::Locked;
            }
            match &nr {
                NotReady::Declined => {
                    eprintln!("sodium-remote-host: the person at this computer declined sharing");
                    let _ = std::fs::remove_file(token_path(data));
                    set_consent(Consent::Declined);
                }
                other => {
                    eprintln!("sodium-remote-host: the portal session didn't start: {other:?}");
                    set_consent(if read_token(data).is_some() { Consent::Remembered } else { Consent::Ask });
                }
            }
            Err(nr)
        }
    }
}

async fn clipboard_tasks(st: Arc<Mutex<State>>, cb: Arc<ClipPortal>, session: Arc<Session<RemoteDesktop>>) -> Result<Vec<tokio::task::JoinHandle<()>>, ashpd::Error> {
    let mut tasks = vec![];
    {
        let (st, cb, session) = (st.clone(), cb.clone(), session.clone());
        tasks.push(tokio::spawn(async move {
            let Ok(owner) = cb.receive_selection_owner_changed::<RemoteDesktop>().await else { return };
            let mut owner = Box::pin(owner);
            while let Some((_, ev)) = owner.next().await {
                if std::env::var_os("SODIUM_REMOTE_HOST_DEBUG").is_some() {
                    eprintln!("sodium-remote-host: clipboard owner changed: ours {:?}, {:?}", ev.session_is_owner(), ev.mime_types());
                }
                if ev.session_is_owner() == Some(true) {
                    continue;
                }
                let mimes = ev.mime_types().to_vec();
                let mut entry = ClipEntry { items: vec![], concealed: mimes.iter().any(|m| m == CONCEALED_MIME) };
                for (fmt, mime) in wanted(&mimes) {
                    match read_selection(&cb, &session, &mime).await {
                        Ok(d) if !d.is_empty() => entry.items.push((fmt, d)),
                        Ok(_) => {}
                        Err(e) => eprintln!("sodium-remote-host: reading the clipboard's {mime}: {e:#}"),
                    }
                }
                if entry.items.is_empty() {
                    continue;
                }
                let sink = {
                    let mut s = st.lock().unwrap();
                    s.clip_in = Some(entry.clone());
                    s.clip_sink.clone()
                };
                if let Some(sink) = sink {
                    sink(entry);
                }
            }
        }));
    }
    tasks.push(tokio::spawn(async move {
        let Ok(transfer) = cb.receive_selection_transfer::<RemoteDesktop>().await else { return };
        let mut transfer = Box::pin(transfer);
        while let Some((_, mime, serial)) = transfer.next().await {
            if std::env::var_os("SODIUM_REMOTE_HOST_DEBUG").is_some() {
                eprintln!("sodium-remote-host: clipboard transfer {serial}: {mime}");
            }
            let data = st.lock().unwrap().clip_out.as_ref().and_then(|e| data_for(e, &mime));
            let ok = match data {
                Some(d) => match cb.selection_write(&session, serial).await {
                    Ok(fd) => write_fd(fd.into(), d).await.is_ok(),
                    Err(_) => false,
                },
                None => false,
            };
            let _ = cb.selection_write_done(&session, serial, ok).await;
        }
    }));
    Ok(tasks)
}

/// The most one clipboard item may hold, as on every other path (an item over it is dropped).
const MAX_CLIP: usize = 32 << 20;

/// Reads to the end but never more than `cap` bytes: a source that keeps writing is an error, not memory.
fn read_capped(r: impl std::io::Read, cap: usize) -> std::io::Result<Vec<u8>> {
    let mut v = vec![];
    std::io::Read::read_to_end(&mut std::io::Read::take(r, cap as u64 + 1), &mut v)?;
    if v.len() > cap {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "the clipboard item is too large"));
    }
    Ok(v)
}

async fn read_selection(cb: &ClipPortal, session: &Session<RemoteDesktop>, mime: &str) -> Result<Vec<u8>> {
    let fd: OwnedFd = cb.selection_read(session, mime).await?.into();
    // GNOME hands out a non-blocking pipe: read it blocking, off this thread.
    rustix::io::ioctl_fionbio(&fd, false)?;
    tokio::time::timeout(Duration::from_secs(3), tokio::task::spawn_blocking(move || {
        read_capped(std::fs::File::from(fd), MAX_CLIP)
    }))
    .await
    .map_err(|_| anyhow!("timed out"))??
    .map_err(Into::into)
}

async fn write_fd(fd: OwnedFd, data: Vec<u8>) -> Result<()> {
    rustix::io::ioctl_fionbio(&fd, false)?;
    tokio::time::timeout(Duration::from_secs(3), tokio::task::spawn_blocking(move || {
        use std::io::Write;
        std::fs::File::from(fd).write_all(&data)
    }))
    .await
    .map_err(|_| anyhow!("timed out"))??
    .map_err(Into::into)
}

async fn notify(l: &Live, n: Notify) -> Result<(), ashpd::Error> {
    let (rd, s) = (&l.rd, &*l.session);
    let state = |down: bool| if down { KeyState::Pressed } else { KeyState::Released };
    match n {
        Notify::Key { code, down } => rd.notify_keyboard_keycode(s, code, state(down), Default::default()).await,
        Notify::Keysym { sym, down } => rd.notify_keyboard_keysym(s, sym, state(down), Default::default()).await,
        Notify::Abs { x, y } => rd.notify_pointer_motion_absolute(s, l.node, x, y, Default::default()).await,
        Notify::Rel { dx, dy } => rd.notify_pointer_motion(s, dx, dy, Default::default()).await,
        Notify::Button { code, down } => rd.notify_pointer_button(s, code, state(down), Default::default()).await,
        Notify::Axis { dx, dy, finish } => {
            rd.notify_pointer_axis(s, dx, dy, ashpd::desktop::remote_desktop::NotifyPointerAxisOptions::default().set_finish(finish)).await
        }
        Notify::Discrete { vertical, steps } => {
            rd.notify_pointer_axis_discrete(s, if vertical { PAxis::Vertical } else { PAxis::Horizontal }, steps, Default::default()).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clipboard_read_stops_at_the_cap() {
        assert_eq!(read_capped(&b"abc"[..], 3).unwrap(), b"abc");
        assert!(read_capped(std::io::repeat(7), 100).is_err(), "an endless source is refused, not read");
    }

    fn dir(name: &str) -> PathBuf {
        let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../build/test-tmp").join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_restore_token_is_kept_reused_and_forgotten() {
        let d = dir("restore-token");
        // Nothing saved: the next connection asks.
        assert_eq!(Portal::start(d.clone()).unwrap().consent(), Consent::Ask);
        assert_eq!(read_token(&d), None);
        // A Start's token is saved privately, and a new host (a restart) reuses it.
        save_token(&d, "6d3f-token\n");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(token_path(&d)).unwrap().permissions().mode() & 0o777, 0o600);
        let p = Portal::start(d.clone()).unwrap();
        assert_eq!(p.consent(), Consent::Remembered);
        assert_eq!(read_token(&d).as_deref(), Some("6d3f-token"));
        // "Forget": gone, and the next connection asks again.
        p.forget_consent();
        assert_eq!(p.consent(), Consent::Ask);
        assert_eq!(read_token(&d), None);
    }
}
