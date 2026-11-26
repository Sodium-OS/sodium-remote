// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Software H.264: OpenH264 from Cisco's prebuilt DLL, fetched on first need.
//!
//! The encoder order is the hardware encoders (`h264_nvenc`, `h264_amf`, `h264_qsv`), then Media
//! Foundation's GPU MFT, then OpenH264. OpenH264 is not shipped and not linked. On Windows
//! [`SoftwareVideo`] downloads Cisco's `.dll.bz2` once ([`crate::openh264_pin`]: version, URL and
//! SHA-256 in one place), checks it, unpacks it into `%LOCALAPPDATA%\SodiumRemote\codecs\` and
//! loads it by path ([`hd_openh264_load`](crate::encode)). Elsewhere it is the system's library
//! and nothing is downloaded.
//!
//! The setting "software video" (on by default) is `settings.json` next to `paired.json`. The UI
//! follows along through [`CodecEvent`]s:
//!
//! | state | when |
//! |---|---|
//! | `needed` | once ever: the host has no hardware H.264 encoder, software video is on and the DLL is not there yet. The one-time dialog. |
//! | `downloading` | with `percent` 0 to 100, from the start |
//! | `ready` | the DLL is loaded (also at a start where it was already downloaded) |
//! | `failed` | the download, the checksum, the unpacking or the load failed; `error` has the text for the log and `reason` the typed cause (below); retry with [`SoftwareVideo::retry`]. An unpinned build fails here without touching the network. |
//! | `off_blocked` | software video is off and there is no hardware encoder: connections fail |
//!
//! A `failed` event's `reason`: `network` (the fetch failed or took too long), `rejected` (the file arrived but its size, checksum,
//! unpacking or load was refused: a retry deletes it first), `unpinned` (this build has no pinned checksum, so it can't download at
//! all) or `cancelled` (software video was switched off meanwhile: no banner).

use std::ffi::{CString, c_char, c_int};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::openh264_pin as pin;

unsafe extern "C" {
    fn hd_openh264_load(path: *const c_char, err: *mut c_char, errlen: c_int) -> c_int;
    fn hd_openh264_loaded() -> c_int;
}

/// The `settings.json` keys.
const KEY_ON: &str = "software_video";
const KEY_SHOWN: &str = "codec_notice_shown";
/// Cisco's DLL is under a megabyte: anything near this is not it.
const MAX_DOWNLOAD: usize = 8 << 20;
const MAX_DLL: usize = 32 << 20;
/// The whole download, from the request to the last byte (a file of under a megabyte): a server that drips bytes, each
/// inside WinHTTP's per-read timeout, can't hold the worker longer than this.
const DOWNLOAD_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// Why software video failed, typed where the failure is produced (the app words each one itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The download didn't complete: no connection, a server error, too slow.
    Network,
    /// A file arrived (or was already there) but can't be used: too large, a different checksum, not unpackable, not loadable.
    Rejected,
    /// This build has no pinned checksum, so it never downloads.
    Unpinned,
    /// Software video was switched off while it downloaded.
    Cancelled,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Network => "network",
            Reason::Rejected => "rejected",
            Reason::Unpinned => "unpinned",
            Reason::Cancelled => "cancelled",
        }
    }
}

/// A failure: the typed cause and the English text for the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub reason: Reason,
    pub text: String,
}

impl Failure {
    fn new(reason: Reason, text: impl Into<String>) -> Failure {
        Failure { reason, text: text.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecEvent {
    pub state: &'static str,
    pub percent: u8,
    /// Why, for `failed` (English, for the log; the UI shows its own text).
    pub error: Option<String>,
    /// The typed cause, for `failed`.
    pub reason: Option<Reason>,
}

impl CodecEvent {
    fn new(state: &'static str, percent: u8) -> CodecEvent {
        CodecEvent { state, percent, error: None, reason: None }
    }
    fn failed(f: Failure) -> CodecEvent {
        CodecEvent { state: "failed", percent: 0, error: Some(f.text), reason: Some(f.reason) }
    }
    /// The JSON the app gets: `{"ev":"codec","event":"codec","state","percent","notice"[,"error","reason"]}`.
    pub fn json(&self) -> Value {
        let mut v = json!({"ev": "codec", "event": "codec", "state": self.state, "percent": self.percent, "notice": pin::NOTICE});
        if let Some(e) = &self.error {
            v["error"] = json!(e);
        }
        if let Some(r) = self.reason {
            v["reason"] = json!(r.as_str());
        }
        v
    }
}

pub type EventSink = Arc<dyn Fn(CodecEvent) + Send + Sync>;

/// Gets a file over HTTP(S). `progress(done, total)` returns false to stop.
pub trait Fetch: Send + Sync {
    fn get(&self, url: &str, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<Vec<u8>, String>;
}

/// The fetcher of this platform: WinHTTP on Windows, none elsewhere.
pub fn platform_fetcher() -> Arc<dyn Fetch> {
    #[cfg(windows)]
    {
        Arc::new(crate::winhttp::WinHttp)
    }
    #[cfg(not(windows))]
    {
        Arc::new(NoFetch)
    }
}

#[cfg(not(windows))]
struct NoFetch;
#[cfg(not(windows))]
impl Fetch for NoFetch {
    fn get(&self, _: &str, _: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<Vec<u8>, String> {
        Err("downloads are for Windows".into())
    }
}

enum Source {
    /// Cisco's DLL in this folder.
    Download { dir: PathBuf },
    /// The system's libopenh264; no setting, no download.
    System,
}

struct Inner {
    enabled: bool,
    shown: bool,
    /// Does this computer have a hardware H.264 encoder? Unknown until the probe ran.
    hardware: Option<bool>,
    busy: bool,
    cancel: Arc<AtomicBool>,
    /// The reason of the last `failed`: a retry deletes a refused file first.
    last_failure: Option<Reason>,
}

pub struct SoftwareVideo {
    source: Source,
    /// Where `settings.json` is (the host's data folder).
    data: PathBuf,
    fetcher: Arc<dyn Fetch>,
    sink: EventSink,
    inner: Mutex<Inner>,
    /// `hd_openh264_load` is not meant to race.
    load: Mutex<()>,
}

/// `<parent>/codecs` for a host folder `<parent>/host` (on Windows `%LOCALAPPDATA%\SodiumRemote\codecs`),
/// else `<data>/codecs`.
pub fn codec_dir_for(data: &Path) -> PathBuf {
    match (data.file_name().and_then(|n| n.to_str()), data.parent()) {
        (Some(n), Some(p)) if n == remote_core::brand::HOST_SUBDIR => p.join("codecs"),
        _ => data.join("codecs"),
    }
}

impl SoftwareVideo {
    /// The system's OpenH264 (Linux, tests): always on, nothing downloaded.
    pub fn system() -> Arc<SoftwareVideo> {
        Self::make(Source::System, PathBuf::new(), platform_fetcher(), Arc::new(|_| {}), true, true)
    }

    /// Cisco's DLL, downloaded into `codec_dir`; the setting lives in `data/settings.json`.
    pub fn downloaded(data: &Path, codec_dir: PathBuf, fetcher: Arc<dyn Fetch>, sink: EventSink) -> Arc<SoftwareVideo> {
        let s = crate::settings::read(data);
        let enabled = s.get(KEY_ON).and_then(Value::as_bool).unwrap_or(true);
        let shown = s.get(KEY_SHOWN).and_then(Value::as_bool).unwrap_or(false);
        Self::make(Source::Download { dir: codec_dir }, data.to_path_buf(), fetcher, sink, enabled, shown)
    }

    /// What a host started without an app uses: the standard folders on Windows, the system's library elsewhere.
    pub fn platform_default() -> Arc<SoftwareVideo> {
        #[cfg(windows)]
        if let Some(base) = std::env::var_os("LOCALAPPDATA") {
            let root = PathBuf::from(base).join(remote_core::brand::WINDOWS_DIR_NAME);
            let host = root.join(remote_core::brand::HOST_SUBDIR);
            return Self::downloaded(&host, root.join("codecs"), platform_fetcher(), Arc::new(|_| {}));
        }
        Self::system()
    }

    fn make(source: Source, data: PathBuf, fetcher: Arc<dyn Fetch>, sink: EventSink, enabled: bool, shown: bool) -> Arc<SoftwareVideo> {
        Arc::new(SoftwareVideo {
            source,
            data,
            fetcher,
            sink,
            inner: Mutex::new(Inner { enabled, shown, hardware: None, busy: false, cancel: Arc::new(AtomicBool::new(false)), last_failure: None }),
            load: Mutex::new(()),
        })
    }

    /// The setting "software video".
    pub fn enabled(&self) -> bool {
        self.inner.lock().unwrap().enabled
    }

    /// Turns software video on or off (persisted). Off stops a download under way; either way
    /// the state is worked out again (`off_blocked`, or the download starts).
    pub fn set_enabled(self: &Arc<Self>, on: bool) {
        if matches!(self.source, Source::System) {
            return;
        }
        {
            let mut i = self.inner.lock().unwrap();
            i.enabled = on;
            if !on {
                i.cancel.store(true, Ordering::SeqCst);
            }
        }
        crate::settings::write(&self.data, KEY_ON, json!(on));
        self.reconcile();
    }

    /// The probe's verdict: is there a hardware H.264 encoder?
    pub fn note_hardware(self: &Arc<Self>, has: bool) {
        self.inner.lock().unwrap().hardware = Some(has);
        self.reconcile();
    }

    /// Try again after `failed`: the file, if any, is replaced.
    pub fn retry(self: &Arc<Self>) {
        let Source::Download { dir } = &self.source else { return };
        let refused = {
            let i = self.inner.lock().unwrap();
            if i.busy || !i.enabled || i.hardware != Some(false) {
                return;
            }
            i.last_failure == Some(Reason::Rejected)
        };
        // A refused file is deleted first, so the retry fetches a new one instead of loading the same.
        if refused && !loaded() {
            let _ = std::fs::remove_file(dir.join(pin::DLL_NAME));
        }
        self.reconcile();
    }

    /// Is software H.264 usable now: on, and the library loaded?
    pub fn usable(&self) -> bool {
        match &self.source {
            Source::System => self.try_load().is_ok(),
            Source::Download { .. } => self.enabled() && self.try_load().is_ok(),
        }
    }

    fn emit(&self, e: CodecEvent) {
        self.inner.lock().unwrap().last_failure = e.reason;
        (self.sink)(e);
    }

    /// Loads the library if it is there already.
    fn try_load(&self) -> Result<(), Failure> {
        let _g = self.load.lock().unwrap();
        if loaded() {
            return Ok(());
        }
        // The folder is the user's to write, and the host may run elevated: the file's hash is checked on every load, and
        // the handle that was hashed stays open (share-read only, on Windows) until the load is done.
        let mut _held = None;
        let path = match &self.source {
            Source::System => None,
            Source::Download { dir } => {
                let p = dir.join(pin::DLL_NAME);
                if !p.is_file() {
                    return Err(Failure::new(Reason::Rejected, format!("{} is not downloaded", pin::DLL_NAME)));
                }
                _held = Some(open_verified(&p, &pin::pinned_dll().map_err(|e| Failure::new(Reason::Unpinned, e))?)?);
                Some(CString::new(p.to_string_lossy().as_bytes()).map_err(|e| Failure::new(Reason::Rejected, e.to_string()))?)
            }
        };
        let mut err = [0 as c_char; 256];
        let r = unsafe { hd_openh264_load(path.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()), err.as_mut_ptr(), err.len() as c_int) };
        if r != 0 {
            return Err(Failure::new(Reason::Rejected, unsafe { std::ffi::CStr::from_ptr(err.as_ptr()) }.to_string_lossy().into_owned()));
        }
        Ok(())
    }

    /// Works out what the app should be told and what to do, from the setting, the hardware and the file.
    fn reconcile(self: &Arc<Self>) {
        let Source::Download { dir } = &self.source else { return };
        let (cancel, show) = {
            let mut i = self.inner.lock().unwrap();
            if i.hardware != Some(false) {
                return;
            }
            if !i.enabled {
                drop(i);
                self.emit(CodecEvent::new("off_blocked", 0));
                return;
            }
            if i.busy {
                return;
            }
            // Already downloaded: nothing to ask or fetch.
            if dir.join(pin::DLL_NAME).is_file() {
                drop(i);
                match self.try_load() {
                    Ok(()) => self.emit(CodecEvent::new("ready", 100)),
                    Err(e) => self.emit(CodecEvent::failed(e)),
                }
                return;
            }
            i.busy = true;
            i.cancel = Arc::new(AtomicBool::new(false));
            let show = !i.shown;
            i.shown = true;
            (i.cancel.clone(), show)
        };
        if show {
            crate::settings::write(&self.data, KEY_SHOWN, json!(true));
            self.emit(CodecEvent::new("needed", 0));
        }
        let me = self.clone();
        let dir = dir.clone();
        std::thread::spawn(move || {
            me.emit(CodecEvent::new("downloading", 0));
            let mut last = 0u8;
            let r = install(me.fetcher.as_ref(), pin::pinned(), pin::URL, &dir, pin::DLL_NAME, &cancel, &mut |p| {
                if p != last {
                    last = p;
                    me.emit(CodecEvent::new("downloading", p));
                }
            })
            .and_then(|()| me.try_load());
            let cancelled = cancel.load(Ordering::SeqCst);
            let again = {
                let mut i = me.inner.lock().unwrap();
                i.busy = false;
                cancelled && i.enabled
            };
            match r {
                _ if cancelled => {}
                Ok(()) => me.emit(CodecEvent::new("ready", 100)),
                Err(e) => {
                    eprintln!("sodium-remote-host: software video: {}", e.text);
                    me.emit(CodecEvent::failed(e));
                }
            }
            if again {
                me.reconcile();
            }
        });
    }
}

/// Opens `path` for reading with writing and deleting by others refused (Windows; elsewhere the OS has no such lock), reads
/// it through to the size cap and compares its SHA-256 with `want`. Keep the returned file open until the library is loaded.
fn open_verified(path: &Path, want: &[u8; 32]) -> Result<std::fs::File, Failure> {
    use std::io::Read;
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut o, 1); // FILE_SHARE_READ
    let f = o.open(path).map_err(|e| Failure::new(Reason::Rejected, format!("{}: {e}", path.display())))?;
    let mut bytes = Vec::new();
    (&f).take(MAX_DLL as u64 + 1).read_to_end(&mut bytes).map_err(|e| Failure::new(Reason::Rejected, format!("{}: {e}", path.display())))?;
    if bytes.len() > MAX_DLL || Sha256::digest(&bytes)[..] != want[..] {
        return Err(Failure::new(Reason::Rejected, format!("{} does not match the pinned checksum", path.display())));
    }
    Ok(f)
}

fn loaded() -> bool {
    unsafe { hd_openh264_loaded() != 0 }
}

/// Fetches, checks, unpacks and stores the DLL as `dir/dll_name`. Fails closed: with no valid
/// pin nothing is fetched; a checksum that differs writes nothing. Progress is a percentage.
pub fn install(
    fetcher: &dyn Fetch,
    pin: Result<[u8; 32], String>,
    url: &str,
    dir: &Path,
    dll_name: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u8),
) -> Result<(), Failure> {
    install_within(fetcher, pin, url, dir, dll_name, cancel, progress, DOWNLOAD_DEADLINE)
}

#[allow(clippy::too_many_arguments)]
fn install_within(
    fetcher: &dyn Fetch,
    pin: Result<[u8; 32], String>,
    url: &str,
    dir: &Path,
    dll_name: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u8),
    deadline: std::time::Duration,
) -> Result<(), Failure> {
    let want = pin.map_err(|e| Failure::new(Reason::Unpinned, e))?;
    let started = std::time::Instant::now();
    let mut late = false;
    // Cisco's file is about 1 MB; anything past the cap is cut off before it is held, let
    // alone hashed (an announced size over the cap stops it at the first chunk).
    let mut too_big = false;
    let got = fetcher.get(url, &mut |done, total| {
        if done > MAX_DOWNLOAD as u64 || total.is_some_and(|t| t > MAX_DOWNLOAD as u64) {
            too_big = true;
        }
        if let Some(t) = total.filter(|t| *t > 0) {
            progress((done.min(t) * 99 / t) as u8);
        }
        late = started.elapsed() > deadline;
        !cancel.load(Ordering::SeqCst) && !too_big && !late
    });
    if cancel.load(Ordering::SeqCst) {
        return Err(Failure::new(Reason::Cancelled, "cancelled"));
    }
    if late {
        return Err(Failure::new(Reason::Network, format!("the download took longer than {} s", deadline.as_secs())));
    }
    if too_big {
        return Err(Failure::new(Reason::Rejected, format!("the download is larger than the {} MiB allowed", MAX_DOWNLOAD >> 20)));
    }
    let bz = got.map_err(|e| Failure::new(Reason::Network, e))?;
    if bz.len() > MAX_DOWNLOAD || Sha256::digest(&bz)[..] != want[..] {
        return Err(Failure::new(Reason::Rejected, "the download does not match the pinned checksum"));
    }
    let dll = crate::bunzip::decompress(&bz, MAX_DLL).map_err(|e| Failure::new(Reason::Rejected, e))?;
    let io = |e: std::io::Error| Failure::new(Reason::Rejected, format!("{}: {e}", dir.display()));
    std::fs::create_dir_all(dir).map_err(io)?;
    let part = dir.join(format!("{dll_name}.part"));
    std::fs::write(&part, &dll).and_then(|()| std::fs::rename(&part, dir.join(dll_name))).map_err(io)?;
    progress(100);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO_BZ: &[u8] = include_bytes!("../tests/data/hello.bz2");

    struct Canned(Vec<u8>);
    impl Fetch for Canned {
        fn get(&self, _: &str, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<Vec<u8>, String> {
            let n = self.0.len() as u64;
            progress(n / 2, Some(n));
            progress(n, Some(n));
            Ok(self.0.clone())
        }
    }
    struct Offline;
    impl Fetch for Offline {
        fn get(&self, _: &str, _: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<Vec<u8>, String> {
            Err("offline".into())
        }
    }

    struct Never;
    impl Fetch for Never {
        fn get(&self, _: &str, _: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<Vec<u8>, String> {
            panic!("the network must not be touched");
        }
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("softvideo-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn install_checks_the_hash_then_unpacks() {
        let dir = temp("ok");
        let want: [u8; 32] = Sha256::digest(HELLO_BZ).into();
        let mut seen = vec![];
        install(&Canned(HELLO_BZ.to_vec()), Ok(want), "u", &dir, "x.dll", &AtomicBool::new(false), &mut |p| seen.push(p)).unwrap();
        assert_eq!(std::fs::read(dir.join("x.dll")).unwrap(), b"hello, bzip2\n");
        assert!(!dir.join("x.dll.part").exists());
        assert!(seen.windows(2).all(|w| w[0] <= w[1]) && seen.last() == Some(&100), "{seen:?}");
        // A different file under the same pin writes nothing.
        let other = temp("bad");
        let e = install(&Canned(b"BZh9other".to_vec()), Ok(want), "u", &other, "x.dll", &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(e.text.contains("checksum"), "{e:?}");
        assert_eq!(e.reason, Reason::Rejected);
        assert!(!other.join("x.dll").exists());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn a_download_over_the_cap_is_refused_before_it_is_hashed() {
        // Announces more than the cap: stopped at the first chunk, nothing written.
        struct Huge;
        impl Fetch for Huge {
            fn get(&self, _: &str, progress: &mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<Vec<u8>, String> {
                if progress(1, Some(MAX_DOWNLOAD as u64 + 1)) { Ok(vec![0; 16]) } else { Err("stopped".into()) }
            }
        }
        let dir = temp("huge");
        let e = install(&Huge, Ok([7; 32]), "u", &dir, "x.dll", &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(e.text.contains("larger"), "{e:?}");
        assert!(!dir.exists());
        // A body past the cap that lied about its size is refused the same way.
        let e = install(&Canned(vec![0; MAX_DOWNLOAD + 1]), Ok([7; 32]), "u", &dir, "x.dll", &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(e.text.contains("checksum") || e.text.contains("larger"), "{e:?}");
    }

    #[test]
    fn a_download_past_its_deadline_is_stopped() {
        let dir = temp("late");
        let want: [u8; 32] = Sha256::digest(HELLO_BZ).into();
        let e = install_within(&Canned(HELLO_BZ.to_vec()), Ok(want), "u", &dir, "x.dll", &AtomicBool::new(false), &mut |_| {}, std::time::Duration::ZERO).unwrap_err();
        assert!(e.text.contains("took longer"), "{e:?}");
        assert_eq!(e.reason, Reason::Network);
        assert!(!dir.join("x.dll").exists());
    }

    #[test]
    fn an_unpinned_build_fails_closed_without_the_network() {
        let dir = temp("unpinned");
        let unpinned = Err("OpenH264 is not pinned in this build: run packaging/windows/pin-openh264.sh".to_string());
        let e = install(&Never, unpinned, "u", &dir, "x.dll", &AtomicBool::new(false), &mut |_| {}).unwrap_err();
        assert!(e.text.contains("pin-openh264.sh"), "{e:?}");
        assert_eq!(e.reason, Reason::Unpinned);
        assert!(!dir.exists());
    }

    #[test]
    fn the_setting_and_the_events() {
        let data = temp("settings");
        let events: Arc<Mutex<Vec<&'static str>>> = Default::default();
        let sink: EventSink = {
            let e = events.clone();
            Arc::new(move |ev| e.lock().unwrap().push(ev.state))
        };
        let codecs = data.join("codecs");
        let sv = SoftwareVideo::downloaded(&data, codecs.clone(), Arc::new(Never), sink.clone());
        assert!(sv.enabled(), "on by default");
        // Off with no hardware encoder: connections would fail.
        sv.set_enabled(false);
        assert!(events.lock().unwrap().is_empty(), "nothing to say before the probe ran");
        sv.note_hardware(false);
        assert_eq!(*events.lock().unwrap(), ["off_blocked"]);
        assert!(!SoftwareVideo::downloaded(&data, codecs.clone(), Arc::new(Never), sink.clone()).enabled(), "persisted in settings.json");
        let s: Value = serde_json::from_slice(&std::fs::read(data.join("settings.json")).unwrap()).unwrap();
        assert_eq!(s[KEY_ON], json!(false));
        // Hardware present: software video is never needed.
        let hw = SoftwareVideo::downloaded(&data, codecs, Arc::new(Never), sink);
        hw.note_hardware(true);
        assert_eq!(*events.lock().unwrap(), ["off_blocked"]);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn needed_fires_once_and_an_offline_download_then_fails() {
        let data = temp("needed");
        let events: Arc<Mutex<Vec<&'static str>>> = Default::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let sink: EventSink = {
            let e = events.clone();
            Arc::new(move |ev| {
                e.lock().unwrap().push(ev.state);
                if ev.state == "failed" {
                    let _ = tx.send(ev);
                }
            })
        };
        let sv = SoftwareVideo::downloaded(&data, data.join("codecs"), Arc::new(Offline), sink.clone());
        sv.note_hardware(false);
        let failed = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(failed.reason, Some(Reason::Network));
        assert!(failed.error.unwrap().contains("offline"));
        assert_eq!(*events.lock().unwrap(), ["needed", "downloading", "failed"]);
        // A second start (the notice was shown) goes straight to the download.
        events.lock().unwrap().clear();
        let again = SoftwareVideo::downloaded(&data, data.join("codecs"), Arc::new(Offline), sink);
        again.note_hardware(false);
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(*events.lock().unwrap(), ["downloading", "failed"]);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_library_is_hashed_before_it_is_loaded() {
        let dir = temp("verified");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.dll");
        std::fs::write(&p, b"MZ original").unwrap();
        let want: [u8; 32] = Sha256::digest(b"MZ original").into();
        assert!(open_verified(&p, &want).is_ok());
        std::fs::write(&p, b"MZ planted").unwrap();
        assert!(open_verified(&p, &want).unwrap_err().text.contains("checksum"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_codecs_folder_is_beside_host() {
        assert_eq!(codec_dir_for(Path::new("/a/SodiumRemote/host")), Path::new("/a/SodiumRemote/codecs"));
        assert_eq!(codec_dir_for(Path::new("/a/data")), Path::new("/a/data/codecs"));
    }
}
