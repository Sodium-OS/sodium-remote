// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! "This computer" on Windows and generic Linux (feature `linux-host`): `host-desktop`'s in-process `Session` behind
//! a second small C ABI (`rh_*`). Events share the client's callback shape (JSON text, `host*` names); commands
//! return at once. On Linux the desktop asks the person at the computer before sharing (the portal's dialog): its
//! state comes as `host_consent` events, `rh_consent` reads it and `rh_forget_consent` forgets a remembered "Allow".

use crate::{EventFn, Sink, UserPtr, cstr};
use host_desktop::backend::Consent;
use host_desktop::session::{self, Controller, Note, Notes, Options, Session, Status};
use host_desktop::softvideo::SoftwareVideo;
use remote_core::keys::{DeviceId, FileStore};
use serde_json::{Value, json};
use std::ffi::{CString, c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The runtime, dropped with `shutdown_background`: the last `Arc<Rh>` may go in a task on the runtime's own thread
/// (a finished `disable`), where tokio's plain drop would panic.
struct Rt(Option<tokio::runtime::Runtime>);

impl std::ops::Deref for Rt {
    type Target = tokio::runtime::Runtime;
    fn deref(&self) -> &tokio::runtime::Runtime {
        self.0.as_ref().expect("the runtime")
    }
}

impl Drop for Rt {
    fn drop(&mut self) {
        if let Some(rt) = self.0.take() {
            rt.shutdown_background();
        }
    }
}

pub struct Rh {
    rt: Rt,
    /// The stop in flight (`disable`): `rh_free` waits for it, so the matched display is restored before the app ends.
    stopping: Mutex<Option<tokio::task::JoinHandle<()>>>,
    data: PathBuf,
    sink: Sink,
    gate: Arc<crate::Gate>,
    session: Mutex<Option<Session>>,
    /// Software H.264 (OpenH264 from Cisco's DLL, downloaded on first need): the setting and its events.
    soft: Arc<SoftwareVideo>,
}

fn controller_json(c: &Controller) -> Value {
    json!({"id": c.id.to_string(), "name": c.name, "os": crate::os_name(c.os, &c.os_flavor), "since": c.since, "view_only": c.view_only})
}

/// `{"ev": "host_consent", "state": ask|asking|remembered|declined|unavailable, "detail"?}`.
fn consent_json(c: &Consent) -> Value {
    let (state, detail) = match c {
        Consent::Ask => ("ask", None),
        Consent::Asking => ("asking", None),
        Consent::Remembered => ("remembered", None),
        Consent::Declined => ("declined", None),
        Consent::Unavailable(d) => ("unavailable", Some(d.clone())),
    };
    json!({"ev": "host_consent", "state": state, "detail": detail})
}

/// `{"ev": "host", "state": "error", "detail"}` (English, for the log) and, when another host already serves from the
/// data directory (Windows: runs on this computer), `"code": "in_use"`: the page shows its own translated line for it.
fn host_error_json(e: &anyhow::Error) -> Value {
    #[cfg(target_os = "linux")]
    if e.downcast_ref::<host_desktop::linux::DataDirInUse>().is_some() {
        return json!({"ev": "host", "state": "error", "code": "in_use", "detail": format!("{e:#}")});
    }
    #[cfg(windows)]
    if e.downcast_ref::<host_desktop::win::HostInUse>().is_some() {
        return json!({"ev": "host", "state": "error", "code": "in_use", "detail": format!("{e:#}")});
    }
    json!({"ev": "host", "state": "error", "detail": format!("{e:#}")})
}

fn note_json(n: Note) -> Value {
    match n {
        Note::Status(Status::Waiting) => json!({"ev": "host_status", "controlled": false}),
        Note::Status(Status::Controlled { client, secure, held }) => {
            json!({"ev": "host_status", "controlled": true, "client": controller_json(&client), "secure": secure, "held": held})
        }
        Note::PairingOpen { code, expires_in } => json!({"ev": "host_pairing", "state": "open", "code": code, "expires_in": expires_in.as_secs()}),
        Note::PairingAttempt => json!({"ev": "host_pairing", "state": "attempt"}),
        Note::PairingFailed { attempts_left } => json!({"ev": "host_pairing", "state": "wrong", "left": attempts_left}),
        Note::PairingClosed { reason } => json!({"ev": "host_pairing", "state": "closed", "reason": reason}),
        Note::Paired(d) => json!({"ev": "host_paired", "id": d.id.to_string(), "name": d.name}),
        Note::PairedChanged => json!({"ev": "host_paired"}),
        Note::Consent(c) => consent_json(&c),
        Note::MatchedDisplayFailed => json!({"ev": "host_matched", "state": "failed"}),
    }
}

fn paired_json(data: &std::path::Path, live: Option<&Session>) -> Value {
    let list = match live {
        Some(s) => s.paired(),
        None => session::stored_paired(data, &FileStore::new(data.to_path_buf())),
    };
    Value::Array(
        list.into_iter()
            .map(|p| {
                json!({"id": p.device.id.to_string(), "code": p.device.id.code(), "name": p.device.name, "paired_at": p.device.paired_at,
                       "last_connected": p.device.last_connected, "os": p.os.map(|o| crate::os_name(o, &p.os_flavor))})
            })
            .collect(),
    )
}

impl Rh {
    fn enable(self: &Arc<Self>, name: String) {
        if self.session.lock().unwrap().is_some() {
            return;
        }
        let me = self.clone();
        self.rt.spawn(async move {
            let notes: Notes = {
                let sink = me.sink.clone();
                Arc::new(move |n| sink(note_json(n)))
            };
            let backend = match host_desktop::platform_backend(&me.data, None, me.soft.clone()) {
                Ok(b) => b,
                Err(e) => return (me.sink)(host_error_json(&e)),
            };
            let o = Options { data: me.data.clone(), name: if name.is_empty() { None } else { Some(name) }, store: None, port: session::DEFAULT_PORT, pair: false, mdns: true, rendezvous: true, server: None, bind: None, secs: None };
            match Session::start(backend, o, Some(notes)).await {
                Ok(s) => {
                    let (id, port) = (s.host().id().to_string(), s.host().lan_port().unwrap_or(0));
                    *me.session.lock().unwrap() = Some(s);
                    (me.sink)(json!({"ev": "host", "state": "on", "id": id, "port": port}));
                }
                Err(e) => (me.sink)(host_error_json(&e)),
            }
        });
    }

    fn disable(self: &Arc<Self>) {
        let s = self.session.lock().unwrap().take();
        let me = self.clone();
        let h = self.rt.spawn(async move {
            if let Some(s) = s {
                s.shutdown().await;
            }
            (me.sink)(json!({"ev": "host", "state": "off"}));
        });
        *self.stopping.lock().unwrap() = Some(h);
    }

    fn with(&self, f: impl FnOnce(&Session) + Send + 'static) {
        if let Some(s) = self.session.lock().unwrap().clone() {
            f(&s);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_new(data_dir: *const c_char, cb: EventFn, user: *mut c_void) -> *mut Arc<Rh> {
    let gate = crate::Gate::new();
    let sink = crate::gated_sink(cb, UserPtr(user), gate.clone());
    let Ok(rt) = tokio::runtime::Builder::new_multi_thread().worker_threads(3).enable_all().build() else { return std::ptr::null_mut() };
    let data = PathBuf::from(unsafe { cstr(data_dir) });
    // Windows downloads Cisco's OpenH264 on first need; Linux uses the system's library.
    #[cfg(windows)]
    let soft = {
        let sink = sink.clone();
        SoftwareVideo::downloaded(&data, host_desktop::softvideo::codec_dir_for(&data), host_desktop::softvideo::platform_fetcher(), Arc::new(move |e| sink(e.json())))
    };
    #[cfg(not(windows))]
    let soft = SoftwareVideo::system();
    // A matched display left up by a host that stopped (a crash, a sign-out): the displays as they were, before anything else.
    host_desktop::recover_matched_display(&data);
    Box::into_raw(Box::new(Arc::new(Rh { rt: Rt(Some(rt)), stopping: Mutex::new(None), data, sink, gate, session: Mutex::new(None), soft })))
}

unsafe fn rh<'a>(p: *mut Arc<Rh>) -> &'a Arc<Rh> {
    unsafe { &*p }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_free(p: *mut Arc<Rh>) {
    if p.is_null() {
        return;
    }
    let b = unsafe { Box::from_raw(p) };
    if let Some(h) = b.stopping.lock().unwrap().take() {
        let _ = b.rt.block_on(h);
    }
    if let Some(s) = b.session.lock().unwrap().take() {
        b.rt.block_on(s.shutdown());
    }
    b.gate.close(); // no callback runs once the caller deletes its object
}

/// Starts (1) or stops (0) hosting; `host` events follow. `name` is the name shown to others ("" for the computer's).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_enable(p: *mut Arc<Rh>, on: c_int, name: *const c_char) {
    let r = unsafe { rh(p) };
    if on != 0 {
        r.enable(unsafe { cstr(name) });
    } else {
        r.disable();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_open_pairing(p: *mut Arc<Rh>) {
    let r = unsafe { rh(p) };
    if let Some(s) = r.session.lock().unwrap().clone() {
        r.rt.spawn(async move {
            s.open_pairing().await;
        });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_close_pairing(p: *mut Arc<Rh>) {
    unsafe { rh(p) }.with(|s| s.close_pairing());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_disconnect(p: *mut Arc<Rh>) {
    unsafe { rh(p) }.with(|s| s.disconnect());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_set_name(p: *mut Arc<Rh>, name: *const c_char) {
    let n = unsafe { cstr(name) };
    unsafe { rh(p) }.with(move |s| s.set_name(&n));
}

/// Removes a paired device (and ends its session). Works with hosting off too.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_revoke(p: *mut Arc<Rh>, id: *const c_char) {
    let r = unsafe { rh(p) };
    let Ok(id) = unsafe { cstr(id) }.parse::<DeviceId>() else { return };
    let live = r.session.lock().unwrap().clone();
    let me = r.clone();
    r.rt.spawn(async move {
        match live {
            Some(s) => {
                s.revoke(id).await;
            }
            None => {
                let _ = session::stored_revoke(&FileStore::new(me.data.clone()), &id);
                #[cfg(target_os = "linux")]
                if host_desktop::linux::forget_stored_consent_if_unpaired(&me.data) {
                    (me.sink)(consent_json(&Consent::Ask));
                }
                (me.sink)(json!({"ev": "host_paired"}));
            }
        }
    });
}

/// The paired devices as JSON (free with `rq_free_string`); the host's own ID and LAN port in `rh_self`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_paired(p: *mut Arc<Rh>) -> *mut c_char {
    let r = unsafe { rh(p) };
    let live = r.session.lock().unwrap().clone();
    CString::new(paired_json(&r.data, live.as_ref()).to_string()).unwrap().into_raw()
}

/// `{"id","port"}` of the running host, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_self(p: *mut Arc<Rh>) -> *mut c_char {
    let r = unsafe { rh(p) };
    let v = match r.session.lock().unwrap().as_ref() {
        Some(s) => json!({"id": s.host().id().to_string(), "port": s.host().lan_port().unwrap_or(0)}),
        None => Value::Null,
    };
    CString::new(v.to_string()).unwrap().into_raw()
}

/// The sharing permission as `host_consent` JSON (free with `rq_free_string`): the running host's, else what is saved
/// in the data directory (Linux: a restore token means "remembered"). `null` where the OS doesn't ask per session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_consent(p: *mut Arc<Rh>) -> *mut c_char {
    let r = unsafe { rh(p) };
    CString::new(rh_consent_value(r).to_string()).unwrap().into_raw()
}

/// Forgets a remembered "Allow" (Linux): the next connection shows the desktop's sharing dialog again. Works with
/// hosting off too.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_forget_consent(p: *mut Arc<Rh>) {
    let r = unsafe { rh(p) };
    match r.session.lock().unwrap().clone() {
        Some(s) => s.forget_consent(),
        None => {
            #[cfg(target_os = "linux")]
            host_desktop::linux::forget_stored_consent(&r.data);
        }
    }
    let c = rh_consent_value(r);
    (r.sink)(c);
}

fn rh_consent_value(r: &Rh) -> Value {
    let live = r.session.lock().unwrap().as_ref().and_then(|s| s.consent());
    #[cfg(target_os = "linux")]
    let live = live.or_else(|| Some(host_desktop::linux::stored_consent(&r.data)));
    live.as_ref().map_or(Value::Null, consent_json)
}

/// The host's default data directory on this OS (Linux: `~/.local/share/sodium-remote/host`), for `rh_new`. Free with
/// `rq_free_string`; null where the app chooses its own.
#[unsafe(no_mangle)]
pub extern "C" fn rh_default_data_dir() -> *mut c_char {
    #[cfg(target_os = "linux")]
    if let Ok(d) = host_desktop::linux::default_data_dir() {
        return CString::new(d.to_string_lossy().into_owned()).unwrap_or_default().into_raw();
    }
    std::ptr::null_mut()
}

/// Turns "software video" on (nonzero) or off (0), and keeps it in `settings.json` beside `paired.json`
/// (default on). Works with hosting off too. Off stops a download under way. `codec` events follow
/// (see the block above `rh_software_video`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_set_software_video(p: *mut Arc<Rh>, on: c_int) {
    unsafe { rh(p) }.soft.set_enabled(on != 0);
}

/// The setting "software video": 1 on, 0 off.
///
/// `codec` events (JSON text through the callback given to `rh_new`, like every `rh_*` event; the
/// state is worked out when hosting starts, once the encoders are probed):
///
///     {"ev":"codec","event":"codec","state":S,"percent":N,"notice":"OpenH264 Video Codec provided by Cisco Systems, Inc."[,"error":"..."]}
///
/// - `needed`: sent once ever (remembered in settings.json): this computer has no hardware H.264
///   encoder, software video is on and the codec is not downloaded yet. Show the one-time dialog;
///   `downloading` follows at once. `percent` 0.
/// - `downloading`: `percent` 0..99 while it runs.
/// - `ready`: the codec is loaded (also at a later start where it was already there). `percent` 100.
/// - `failed`: download, checksum, unpacking or loading failed; `error` is English, for the log.
///   Offer "Try again" (`rh_codec_retry`). A build whose checksum pin is still the placeholder
///   fails here at once without touching the network.
/// - `off_blocked`: software video is off and there is no hardware encoder: clients cannot connect.
///   Offer to turn it on (`rh_set_software_video(h, 1)`; the download starts by itself).
///
/// `notice` is Cisco's required wording and goes into every card that mentions the codec.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_software_video(p: *mut Arc<Rh>) -> c_int {
    unsafe { rh(p) }.soft.enabled() as c_int
}

/// After `failed`: tries the download again (replacing a bad file). Does nothing otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_codec_retry(p: *mut Arc<Rh>) {
    unsafe { rh(p) }.soft.retry();
}

/// "Match the display to the connecting computer" (`settings.json` `matched_display`, default off): 1 on, 0 off.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_matched_display(p: *mut Arc<Rh>) -> c_int {
    host_desktop::vdisplay::wanted(&unsafe { rh(p) }.data) as c_int
}

/// Saves "Match the display to the connecting computer". The app installs (on) or removes (off) the driver first, in an
/// elevated copy of itself (`rh_matched_display_admin`), and saves only when that worked. A running host uses the setting
/// from its next connection. When a connection with it on can't get the virtual display, the session shows the physical
/// display and this event comes:
///
///     {"ev":"host_matched","state":"failed"}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_set_matched_display(p: *mut Arc<Rh>, on: c_int) {
    host_desktop::vdisplay::set_wanted(&unsafe { rh(p) }.data, on != 0);
}

/// The elevated side of the setting (`SodiumRemote.exe --matched-display install|uninstall --log FILE`, Windows):
/// `action` "install" checks the driver files beside the program against their pins, imports the certificate, creates the
/// device and installs the driver; "uninstall" removes them. 0: done; 1: failed (`log`, when not empty, gets the reason in
/// English); 2: not on this OS or an unknown action.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rh_matched_display_admin(action: *const c_char, log: *const c_char) -> c_int {
    let (action, log) = (unsafe { cstr(action) }, unsafe { cstr(log) });
    #[cfg(windows)]
    {
        use host_desktop::win::vdd_install;
        let r = match action.as_str() {
            "install" => vdd_install::install(None),
            "uninstall" => vdd_install::uninstall(),
            _ => return 2,
        };
        let line = match &r {
            Ok(()) => format!("{action}: done\n"),
            Err(e) => format!("{action}: {e:#}\n"),
        };
        if !log.is_empty() {
            let _ = std::fs::write(&log, line);
        }
        return if r.is_ok() { 0 } else { 1 };
    }
    #[allow(unreachable_code)]
    {
        let _ = (action, log);
        2
    }
}

#[allow(dead_code)]
fn unused(_: Duration) {}
