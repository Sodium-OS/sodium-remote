// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! ScreenCaptureKit capture of one display.
//!
//! - **The stream:** an `SCStream` over an `SCContentFilter` of the whole display, at
//!   the display's backing pixels (native size), NV12 video range with the BT.709
//!   matrix (what the encoder takes as it is), `showsCursor` off (the pointer goes as
//!   shape and position), `queueDepth` 5 (the media loop holds at most two frames:
//!   the latest and the one being encoded), `minimumFrameInterval` from the stream's
//!   rate.
//! - **Frames:** each sample's `SCStreamFrameInfo`: only `Complete` frames carry a new
//!   picture (`Idle` means nothing changed, so no frame), with their dirty rects. A
//!   frame waits in a one-slot mailbox; a newer one replaces it and their rects merge.
//! - **The pointer:** its global position (CGEvent) every 16 ms, its shape (NSCursor,
//!   `cursor`) every 50 ms, mapped to this display's pixels; off this display it is
//!   hidden.
//! - **Secure screen:** the lock screen (and another user's session on the console)
//!   from `CGSessionCopyCurrentDictionary`, every 250 ms.
//! - **Ends** when ScreenCaptureKit stops the stream (`stream:didStopWithError:`) or
//!   the display's size changes; the media loop opens a new one.

use std::any::Any;
use std::ffi::c_void;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFString, CFType, CGRect};
use objc2_core_graphics::{CGRectMakeWithDictionaryRepresentation, CGSessionCopyCurrentDictionary, kCGColorSpaceSRGB, kCGDisplayStreamYCbCrMatrix_ITU_R_709_2};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{CVPixelBuffer, CVPixelBufferGetHeight, CVPixelBufferGetWidth, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCFrameStatus, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCStreamFrameInfoDirtyRects, SCStreamFrameInfoScaleFactor, SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType,
};

use super::cursor::CursorPoll;
use super::display::{MacDisplay, geometry};
use super::vt::Cf;
use crate::backend::{Capture, CaptureUpdate, Picture, Rect};

/// A ScreenCaptureKit frame: an IOSurface-backed NV12 pixel buffer, handed to the
/// VideoToolbox encoder as it is (`Picture::as_any`).
pub struct MacPicture {
    pub buffer: Cf<CVPixelBuffer>,
    pub width: u32,
    pub height: u32,
}

impl Picture for MacPicture {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    fn bgra(&self) -> Option<(&[u8], usize)> {
        None
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The latest frame, waiting for `next`.
#[derive(Default)]
struct Mailbox {
    frame: Option<(Cf<CVPixelBuffer>, Instant)>,
    /// Dirty rects since the last `next` (display pixels); `full` when unknown.
    rects: Vec<Rect>,
    full: bool,
    stopped: Option<String>,
    /// The first dirty rects logged (SODIUM_REMOTE_HOST_DEBUG), to check their units.
    logged: u32,
}

#[derive(Default)]
pub(super) struct VideoSink {
    mail: Mutex<Mailbox>,
    cv: Condvar,
}

/// Where a stream's samples go: a display's frames, or the system audio.
pub(super) enum Sink {
    Video(Arc<VideoSink>),
    Audio(Arc<super::audio::AudioSink>),
}

define_class!(
    /// The stream's output and delegate, called on the stream's dispatch queue.
    #[unsafe(super(NSObject))]
    #[ivars = Sink]
    pub(super) struct StreamOutput;

    unsafe impl NSObjectProtocol for StreamOutput {}

    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output_sample(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            autoreleasepool(|_| match (self.ivars(), kind) {
                (Sink::Video(v), SCStreamOutputType::Screen) => on_frame(v, sample),
                (Sink::Audio(a), SCStreamOutputType::Audio) => a.on_sample(sample),
                _ => {}
            });
        }
    }

    unsafe impl SCStreamDelegate for StreamOutput {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            let why = format!("ScreenCaptureKit stopped the stream: {}", error.localizedDescription());
            match self.ivars() {
                Sink::Video(v) => {
                    v.mail.lock().unwrap().stopped = Some(why);
                    v.cv.notify_all();
                }
                Sink::Audio(a) => a.stopped(why),
            }
        }
    }
);

impl StreamOutput {
    pub(super) fn new(sink: Sink) -> Retained<StreamOutput> {
        let this = Self::alloc().set_ivars(sink);
        unsafe { msg_send![super(this), init] }
    }
}

/// The frame info dictionary of a sample (toll-free bridged to NSDictionary).
fn frame_info(sample: &CMSampleBuffer) -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
    let arr = unsafe { sample.sample_attachments_array(false) }?;
    if arr.count() < 1 {
        return None;
    }
    let d = unsafe { arr.value_at_index(0) } as *mut NSDictionary<NSString, AnyObject>;
    unsafe { Retained::retain(d) }
}

fn on_frame(sink: &VideoSink, sample: &CMSampleBuffer) {
    let info = frame_info(sample);
    let number = |key: &NSString| info.as_ref().and_then(|d| d.objectForKey(key)).and_then(|o| o.downcast::<NSNumber>().ok());
    let status = number(unsafe { SCStreamFrameInfoStatus }).map_or(SCFrameStatus::Complete, |n| SCFrameStatus(n.integerValue()));
    if status != SCFrameStatus::Complete {
        // Idle: nothing changed. Blank, Suspended, Started, Stopped: no picture.
        return;
    }
    let Some(buffer) = (unsafe { sample.image_buffer() }) else { return };
    // Dirty rects: ScreenCaptureKit's are taken as output pixels (docs/reference/host-mac.md,
    // first-run checklist: SODIUM_REMOTE_HOST_DEBUG logs the first ones to confirm).
    let mut rects = vec![];
    let mut known = false;
    if let Some(list) = info.as_ref().and_then(|d| d.objectForKey(unsafe { SCStreamFrameInfoDirtyRects })) {
        if let Ok(list) = list.downcast::<NSArray>() {
            known = true;
            for item in list.iter() {
                let mut r = CGRect::default();
                let cf = Retained::as_ptr(&item) as *const CFDictionary;
                if unsafe { CGRectMakeWithDictionaryRepresentation(cf.as_ref(), &mut r) } {
                    rects.push(Rect { x: r.origin.x.floor() as i32, y: r.origin.y.floor() as i32, w: r.size.width.ceil() as i32, h: r.size.height.ceil() as i32 });
                }
            }
        }
    }
    let mut m = sink.mail.lock().unwrap();
    if std::env::var_os("SODIUM_REMOTE_HOST_DEBUG").is_some() && m.logged < 5 && !rects.is_empty() {
        m.logged += 1;
        let scale = number(unsafe { SCStreamFrameInfoScaleFactor }).map_or(0.0, |n| n.doubleValue());
        eprintln!(
            "sodium-remote-host: capture {}x{} scale factor {scale}, dirty rects {:?}",
            CVPixelBufferGetWidth(&buffer),
            CVPixelBufferGetHeight(&buffer),
            &rects[..rects.len().min(4)]
        );
    }
    if known && !m.full {
        m.rects.extend(rects);
    } else {
        m.full = true;
    }
    m.frame = Some((Cf(buffer), Instant::now()));
    drop(m);
    sink.cv.notify_all();
}

/// Waits for a completion handler's answer.
fn wait<T>(rx: mpsc::Receiver<T>, what: &str) -> Result<T> {
    rx.recv_timeout(Duration::from_secs(5)).map_err(|_| anyhow!("{what}: no answer from ScreenCaptureKit in 5 s"))
}

struct SendPtr<T>(T);
unsafe impl<T> Send for SendPtr<T> {}

/// The displays ScreenCaptureKit can capture (this asks for Screen Recording).
pub(super) fn shareable_content() -> Result<Retained<SCShareableContent>> {
    let (tx, rx) = mpsc::channel::<SendPtr<Result<Retained<SCShareableContent>, String>>>();
    let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        let r = match unsafe { Retained::retain(content) } {
            Some(c) => Ok(c),
            None => Err(unsafe { error.as_ref() }.map_or("no content".into(), |e| e.localizedDescription().to_string())),
        };
        let _ = tx.send(SendPtr(r));
    });
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    wait(rx, "SCShareableContent")?.0.map_err(|e| anyhow!("ScreenCaptureKit's displays: {e}"))
}

pub(super) fn find_display(content: &SCShareableContent, id: u32) -> Result<Retained<SCDisplay>> {
    unsafe { content.displays() }
        .iter()
        .find(|d| unsafe { d.displayID() } == id)
        .ok_or_else(|| anyhow!("ScreenCaptureKit has no display {id}"))
}

/// A running stream with one output; stopped on drop.
pub(super) struct Stream {
    stream: Retained<SCStream>,
    output: Retained<StreamOutput>,
    kind: SCStreamOutputType,
    _queue: DispatchRetained<DispatchQueue>,
}

unsafe impl Send for Stream {}

impl Stream {
    pub(super) fn start(filter: &SCContentFilter, config: &SCStreamConfiguration, sink: Sink, kind: SCStreamOutputType, label: &str) -> Result<Stream> {
        let output = StreamOutput::new(sink);
        let delegate = ProtocolObject::<dyn SCStreamDelegate>::from_ref(&*output);
        let stream = unsafe { SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), filter, config, Some(delegate)) };
        let queue = DispatchQueue::new(label, None);
        let out = ProtocolObject::<dyn SCStreamOutput>::from_ref(&*output);
        unsafe { stream.addStreamOutput_type_sampleHandlerQueue_error(out, kind, Some(&queue)) }
            .map_err(|e| anyhow!("SCStream addStreamOutput: {}", e.localizedDescription()))?;
        let (tx, rx) = mpsc::channel::<Option<String>>();
        let block = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.send(unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string()));
        });
        unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };
        if let Some(e) = wait(rx, "SCStream startCapture")? {
            bail!("ScreenCaptureKit didn't start: {e}");
        }
        Ok(Stream { stream, output, kind, _queue: queue })
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let (tx, rx) = mpsc::channel::<()>();
        let block = RcBlock::new(move |_error: *mut NSError| {
            let _ = tx.send(());
        });
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&block)) };
        let _ = rx.recv_timeout(Duration::from_secs(2));
        let out = ProtocolObject::<dyn SCStreamOutput>::from_ref(&*self.output);
        let _ = unsafe { self.stream.removeStreamOutput_type_error(out, self.kind) };
    }
}

/// The lock screen is up, or this user's session isn't the one on the screen.
pub fn screen_locked() -> bool {
    let Some(d) = CGSessionCopyCurrentDictionary() else { return true };
    let get = |name: &'static str| -> Option<bool> {
        let key = CFString::from_static_str(name);
        let v = unsafe { d.value(&*key as *const CFString as *const c_void) } as *const CFType;
        unsafe { v.as_ref() }.and_then(|v| v.downcast_ref::<CFBoolean>()).map(|b| b.as_bool())
    };
    get("CGSSessionScreenIsLocked").unwrap_or(false) || !get("kCGSSessionOnConsoleKey").unwrap_or(true)
}

pub struct ScCapture {
    display: MacDisplay,
    sink: Arc<VideoSink>,
    _stream: Stream,
    first: bool,
    cursor: CursorPoll,
    secure: bool,
    announce_secure: bool,
    secure_checked: Instant,
    geometry_checked: Instant,
}

impl ScCapture {
    pub fn open(id: u32, fps: u32) -> Result<ScCapture> {
        let display = geometry(id).with_context(|| format!("no display {id}"))?;
        let (w, h) = (display.display.info.width, display.display.info.height);
        let sink = Arc::new(VideoSink::default());
        let stream = autoreleasepool(|_| -> Result<Stream> {
            let content = shareable_content()?;
            let scd = find_display(&content, id)?;
            let none = NSArray::new();
            let filter = unsafe { SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &scd, &none) };
            let config = unsafe { SCStreamConfiguration::new() };
            unsafe {
                config.setWidth(w as usize);
                config.setHeight(h as usize);
                config.setPixelFormat(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
                config.setColorMatrix(kCGDisplayStreamYCbCrMatrix_ITU_R_709_2);
                config.setColorSpaceName(kCGColorSpaceSRGB);
                config.setShowsCursor(false);
                config.setQueueDepth(5);
                config.setMinimumFrameInterval(CMTime::new(1, fps.clamp(1, 240) as i32));
            }
            Stream::start(&filter, &config, Sink::Video(sink.clone()), SCStreamOutputType::Screen, "com.sodiumos.remote.host.capture")
        })?;
        let secure = screen_locked();
        eprintln!("sodium-remote-host: capturing display {id} at {w}x{h}, up to {fps} fps{}", if secure { " (the screen is locked)" } else { "" });
        Ok(ScCapture {
            display,
            sink,
            _stream: stream,
            first: true,
            cursor: CursorPoll::default(),
            secure,
            announce_secure: secure,
            secure_checked: Instant::now(),
            geometry_checked: Instant::now(),
        })
    }
}

impl Capture for ScCapture {
    fn next(&mut self, timeout: Duration) -> Result<CaptureUpdate> {
        let deadline = Instant::now() + timeout;
        let mut u = CaptureUpdate::default();
        if std::mem::take(&mut self.announce_secure) {
            u.secure = Some(true);
        }
        loop {
            let now = Instant::now();
            if now.duration_since(self.secure_checked) >= Duration::from_millis(250) {
                self.secure_checked = now;
                let s = screen_locked();
                if s != self.secure {
                    self.secure = s;
                    u.secure = Some(s);
                }
            }
            if now.duration_since(self.geometry_checked) >= Duration::from_secs(1) {
                self.geometry_checked = now;
                let g = geometry(self.display.display.info.id);
                let same = g.as_ref().is_some_and(|g| (g.display.info.width, g.display.info.height) == (self.display.display.info.width, self.display.display.info.height));
                if !same {
                    bail!("display {} changed or went away", self.display.display.info.id);
                }
                if let Some(g) = g {
                    // It may have moved in the arrangement.
                    self.display.bounds = g.bounds;
                }
            }
            {
                let wait = deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(16));
                let mut m = self.sink.mail.lock().unwrap();
                if m.frame.is_none() && m.stopped.is_none() && !wait.is_zero() {
                    m = self.sink.cv.wait_timeout(m, wait).unwrap().0;
                }
                if let Some(why) = m.stopped.take() {
                    bail!("{why}");
                }
                if let Some((buffer, at)) = m.frame.take() {
                    // The first picture, and any whose damage is unknown, is whole.
                    let whole = std::mem::take(&mut m.full) | std::mem::take(&mut self.first);
                    let rects = std::mem::take(&mut m.rects);
                    let rects = if whole { vec![] } else { rects };
                    let (w, h) = (CVPixelBufferGetWidth(&buffer.0) as u32, CVPixelBufferGetHeight(&buffer.0) as u32);
                    u.frame = Some((Arc::new(MacPicture { buffer, width: w, height: h }) as Arc<dyn Picture>, rects));
                    u.at = Some(at);
                }
            }
            let (pointer, shape) = self.cursor.poll(&self.display);
            u.pointer = pointer;
            u.shape = shape;
            let any = u.frame.is_some() || u.pointer.is_some() || u.shape.is_some() || u.secure.is_some();
            if any || Instant::now() >= deadline {
                return Ok(u);
            }
        }
    }
}
