// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! VideoToolbox encoders, used directly.
//!
//! - **Codecs:** HEVC Main only on the hardware encoder (`RequireHardwareAccelerated…`:
//!   decision 5 offers HEVC only with hardware), H.264 Constrained High (hardware on
//!   every Apple silicon Mac; VideoToolbox may fall back to software elsewhere).
//! - **Low latency:** each frame is completed before `encode` returns, `RealTime`, no
//!   frame reordering, `PrioritizeEncodingSpeedOverQuality`, no keyframe interval (`MaxKeyFrameInterval` and its duration 0, "no limit"), a
//!   keyframe only when asked (`kVTEncodeFrameOptionKey_ForceKeyFrame`). Not
//!   `EnableLowLatencyRateControl`: on an M3 Air at 2940x1912 it doubled the encode time
//!   (HEVC p50 23.6 ms against 10.9 ms at 60 fps, 36.8 against 20.4 ms at 10 fps; H.264
//!   22.7 against 17.9 ms) and overshot the target more (103 against 61 Mbit/s for 50),
//!   with larger frames too (max 694 against 486 KB). Measured 2026-11-19.
//! - **Rate:** `AverageBitRate` and `DataRateLimits` (1.5 × the rate over 1 s) follow
//!   core's target per frame, set again only when it moves by more than 10%.
//! - **Pictures:** a ScreenCaptureKit frame (`MacPicture`, an IOSurface-backed NV12
//!   `CVPixelBuffer` at the display's size) goes in as it is, zero copy. A frame of
//!   another size (the stream shrunk to fit the client) is scaled into a buffer from the
//!   session's own pool by a `VTPixelTransferSession` on the GPU; so is any CPU BGRA
//!   picture (the fake backend), after a copy into a pixel buffer.
//! - **Colour:** BT.709 primaries, transfer and matrix, video range, in the VUI: the
//!   same as the Windows encoders.
//! - **Out:** each frame is completed before `encode` returns (`CompleteFrames`), and the
//!   sample buffer's length-prefixed NAL units become Annex B with 4-byte start codes,
//!   the parameter sets (VPS/SPS/PPS, SPS/PPS) in front of every keyframe, as the
//!   Windows encoders send.

use std::ffi::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{NonNull, null, null_mut};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{
    CMSampleBuffer, CMTime, CMVideoCodecType, CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
    CMVideoFormatDescriptionGetHEVCParameterSetAtIndex, kCMSampleAttachmentKey_NotSync, kCMTimeInvalid, kCMVideoCodecType_H264,
    kCMVideoCodecType_HEVC,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferPool,
    CVPixelBufferUnlockBaseAddress, kCVImageBufferColorPrimaries_ITU_R_709_2,
    kCVImageBufferTransferFunction_ITU_R_709_2, kCVImageBufferYCbCrMatrix_ITU_R_709_2, kCVPixelBufferHeightKey,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_video_toolbox::{
    VTCompressionSession, VTEncodeInfoFlags, VTPixelTransferSession, VTSessionSetProperty, kVTCompressionPropertyKey_AllowFrameReordering,
    kVTCompressionPropertyKey_AverageBitRate, kVTCompressionPropertyKey_ColorPrimaries, kVTCompressionPropertyKey_DataRateLimits,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality,
    kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime, kVTCompressionPropertyKey_TransferFunction,
    kVTCompressionPropertyKey_YCbCrMatrix, kVTEncodeFrameOptionKey_ForceKeyFrame, kVTPixelTransferPropertyKey_DestinationColorPrimaries,
    kVTPixelTransferPropertyKey_DestinationTransferFunction, kVTPixelTransferPropertyKey_DestinationYCbCrMatrix,
    kVTPixelTransferPropertyKey_RealTime, kVTProfileLevel_H264_ConstrainedHigh_AutoLevel, kVTProfileLevel_HEVC_Main_AutoLevel,
    kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
    kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder,
};
use remote_core::msg::Codec;

use super::capture::MacPicture;
use crate::backend::{EncodedFrame, Encoder, EncoderCaps, EncoderConfig, Picture};

pub const HEVC: &str = "videotoolbox_hevc";
pub const H264: &str = "videotoolbox_h264";

/// A CF object handed between threads. VideoToolbox sessions and pixel buffers are
/// thread-safe CF types; the media thread is their only user at a time.
pub struct Cf<T: ?Sized>(pub CFRetained<T>);
unsafe impl<T: ?Sized> Send for Cf<T> {}
unsafe impl<T: ?Sized> Sync for Cf<T> {}

fn num_i32(v: i32) -> CFRetained<CFNumber> {
    CFNumber::new_i32(v)
}

fn dict(pairs: &[(&CFString, &CFType)]) -> CFRetained<CFDictionary<CFString, CFType>> {
    let keys: Vec<&CFString> = pairs.iter().map(|p| p.0).collect();
    let vals: Vec<&CFType> = pairs.iter().map(|p| p.1).collect();
    CFDictionary::from_slices(&keys, &vals)
}

fn set(session: &CFType, key: &CFString, value: &CFType) -> i32 {
    unsafe { VTSessionSetProperty(session, key, Some(value)) }
}

fn codec_type(c: Codec) -> CMVideoCodecType {
    if c == Codec::H264 { kCMVideoCodecType_H264 } else { kCMVideoCodecType_HEVC }
}

/// What the output callback leaves for `encode`, one frame at a time.
#[derive(Default)]
struct Slot {
    called: bool,
    status: i32,
    data: Vec<u8>,
    key: bool,
}

unsafe extern "C-unwind" fn on_encoded(
    _session_ref: *mut c_void,
    frame_ref: *mut c_void,
    status: i32,
    _flags: VTEncodeInfoFlags,
    sample: *mut CMSampleBuffer,
) {
    let Some(slot) = (unsafe { (frame_ref as *mut Slot).as_mut() }) else { return };
    slot.called = true;
    slot.status = status;
    if status != 0 {
        return;
    }
    // A null sample is a dropped frame; leave the slot empty.
    let Some(sb) = (unsafe { sample.as_ref() }) else { return };
    match catch_unwind(AssertUnwindSafe(|| annex_b(sb))) {
        Ok(Some((data, key))) => {
            slot.data = data;
            slot.key = key;
        }
        _ => slot.status = -1,
    }
}

/// The sample's NAL units as Annex B (4-byte start codes), parameter sets first on a
/// keyframe.
fn annex_b(sb: &CMSampleBuffer) -> Option<(Vec<u8>, bool)> {
    let key = !not_sync(sb);
    let fmt = unsafe { sb.format_description() }?;
    let hevc = unsafe { fmt.media_sub_type() } == kCMVideoCodecType_HEVC;
    let block = unsafe { sb.data_buffer() }?;
    let len = unsafe { block.data_length() };
    let mut avcc = vec![0u8; len];
    if len > 0 && unsafe { block.copy_data_bytes(0, len, NonNull::new(avcc.as_mut_ptr().cast())?) } != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(len + 256);
    let mut header_len: c_int = 4;
    let mut count = 1usize;
    let mut i = 0usize;
    while i < count {
        let (mut p, mut size, mut n) = (null(), 0usize, 0usize);
        let r = unsafe {
            if hevc {
                CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(&fmt, i, &mut p, &mut size, &mut n, &mut header_len)
            } else {
                CMVideoFormatDescriptionGetH264ParameterSetAtIndex(&fmt, i, &mut p, &mut size, &mut n, &mut header_len)
            }
        };
        if r != 0 || p.is_null() {
            break;
        }
        count = n;
        if key {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(unsafe { std::slice::from_raw_parts(p, size) });
        }
        i += 1;
    }
    let hl = header_len.clamp(1, 4) as usize;
    let mut pos = 0;
    while pos + hl <= avcc.len() {
        let n = avcc[pos..pos + hl].iter().fold(0usize, |a, &b| (a << 8) | b as usize);
        pos += hl;
        if n == 0 || pos + n > avcc.len() {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&avcc[pos..pos + n]);
        pos += n;
    }
    Some((out, key))
}

/// The sample carries `kCMSampleAttachmentKey_NotSync` = true (not a keyframe).
fn not_sync(sb: &CMSampleBuffer) -> bool {
    let Some(arr) = (unsafe { sb.sample_attachments_array(false) }) else { return false };
    if arr.count() < 1 {
        return false;
    }
    let d = unsafe { arr.value_at_index(0) } as *const CFDictionary;
    let Some(d) = (unsafe { d.as_ref() }) else { return false };
    let key: &CFString = unsafe { kCMSampleAttachmentKey_NotSync };
    let v = unsafe { d.value(key as *const CFString as *const c_void) } as *const CFType;
    let Some(v) = (unsafe { v.as_ref() }) else { return false };
    v.downcast_ref::<CFBoolean>().is_some_and(|b| b.as_bool())
}

/// A compression session as configured here.
fn create_session(codec: Codec, w: u32, h: u32, fps: u32, bps: u64) -> Result<CFRetained<VTCompressionSession>> {
    let yes: &CFType = CFBoolean::new(true);
    let mut spec: Vec<(&CFString, &CFType)> = vec![];
    unsafe {
        if codec == Codec::H264 {
            spec.push((kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder, yes));
        } else {
            spec.push((kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, yes));
        }
    }
    let spec = dict(&spec);
    let (fmt, width, height) = (num_i32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange as i32), num_i32(w as i32), num_i32(h as i32));
    let iosurface = CFDictionary::<CFString, CFType>::empty();
    let attrs = unsafe {
        dict(&[
            (kCVPixelBufferPixelFormatTypeKey, &fmt),
            (kCVPixelBufferWidthKey, &width),
            (kCVPixelBufferHeightKey, &height),
            (kCVPixelBufferIOSurfacePropertiesKey, &iosurface),
        ])
    };
    let mut raw: *mut VTCompressionSession = null_mut();
    let r = unsafe {
        VTCompressionSession::create(
            None,
            w as i32,
            h as i32,
            codec_type(codec),
            Some(spec.as_opaque()),
            Some(attrs.as_opaque()),
            None,
            Some(on_encoded),
            null_mut(),
            NonNull::from(&mut raw),
        )
    };
    if r != 0 || raw.is_null() {
        bail!("VTCompressionSessionCreate {:?} {w}x{h}: OSStatus {r}", codec);
    }
    let s = unsafe { CFRetained::from_raw(NonNull::new_unchecked(raw)) };
    let st: &CFType = &s;
    let no: &CFType = CFBoolean::new(false);
    let zero = num_i32(0);
    let fps_n = num_i32(fps as i32);
    unsafe {
        set(st, kVTCompressionPropertyKey_RealTime, yes);
        set(st, kVTCompressionPropertyKey_AllowFrameReordering, no);
        set(st, kVTCompressionPropertyKey_MaxKeyFrameInterval, &zero);
        set(st, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, &zero);
        set(st, kVTCompressionPropertyKey_ExpectedFrameRate, &fps_n);
        set(st, kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality, yes);
        let profile = if codec == Codec::H264 { kVTProfileLevel_H264_ConstrainedHigh_AutoLevel } else { kVTProfileLevel_HEVC_Main_AutoLevel };
        set(st, kVTCompressionPropertyKey_ProfileLevel, profile);
        set(st, kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2);
        set(st, kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2);
        set(st, kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2);
    }
    set_rate(&s, bps);
    let r = unsafe { s.prepare_to_encode_frames() };
    if r != 0 {
        bail!("VTCompressionSessionPrepareToEncodeFrames: OSStatus {r}");
    }
    Ok(s)
}

fn set_rate(s: &VTCompressionSession, bps: u64) {
    let st: &CFType = s;
    let avg = CFNumber::new_i64(bps as i64);
    // One pair: at most 1.5 × the rate in any second (bytes, seconds).
    let bytes = CFNumber::new_f64(bps as f64 * 1.5 / 8.0);
    let secs = CFNumber::new_f64(1.0);
    let limits = objc2_core_foundation::CFArray::<CFNumber>::from_objects(&[&bytes, &secs]);
    unsafe {
        set(st, kVTCompressionPropertyKey_AverageBitRate, &avg);
        set(st, kVTCompressionPropertyKey_DataRateLimits, &limits);
    }
}

pub struct VtEncoder {
    session: Cf<VTCompressionSession>,
    codec: Codec,
    name: &'static str,
    width: u32,
    height: u32,
    bps: u64,
    epoch: Instant,
    last_pts: i64,
    transfer: Option<Cf<VTPixelTransferSession>>,
    /// The CPU path's staging buffer (BGRA at the picture's size).
    staging: Option<Cf<CVPixelBuffer>>,
}

impl VtEncoder {
    pub fn open(cfg: &EncoderConfig) -> Result<VtEncoder> {
        let session = create_session(cfg.codec, cfg.width, cfg.height, cfg.fps, cfg.bitrate_bps)?;
        Ok(VtEncoder {
            session: Cf(session),
            codec: cfg.codec,
            name: if cfg.codec == Codec::H264 { H264 } else { HEVC },
            width: cfg.width,
            height: cfg.height,
            bps: cfg.bitrate_bps,
            epoch: Instant::now(),
            last_pts: -1,
            transfer: None,
            staging: None,
        })
    }

    fn transfer(&mut self) -> Result<&VTPixelTransferSession> {
        if self.transfer.is_none() {
            let mut raw: *mut VTPixelTransferSession = null_mut();
            let r = unsafe { VTPixelTransferSession::create(None, NonNull::from(&mut raw)) };
            if r != 0 || raw.is_null() {
                bail!("VTPixelTransferSessionCreate: OSStatus {r}");
            }
            let t = unsafe { CFRetained::from_raw(NonNull::new_unchecked(raw)) };
            let tt: &CFType = &t;
            unsafe {
                set(tt, kVTPixelTransferPropertyKey_RealTime, CFBoolean::new(true));
                set(tt, kVTPixelTransferPropertyKey_DestinationColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2);
                set(tt, kVTPixelTransferPropertyKey_DestinationTransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2);
                set(tt, kVTPixelTransferPropertyKey_DestinationYCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2);
            }
            self.transfer = Some(Cf(t));
        }
        Ok(&self.transfer.as_ref().unwrap().0)
    }

    /// A buffer the session takes as it is: the capture's own when it is the encoder's
    /// size, else a scaled (and converted) copy from the session's pool.
    fn input(&mut self, picture: &dyn Picture) -> Result<CFRetained<CVPixelBuffer>> {
        let src: CFRetained<CVPixelBuffer> = if let Some(p) = picture.as_any().downcast_ref::<MacPicture>() {
            p.buffer.0.clone()
        } else if let Some((bgra, stride)) = picture.bgra() {
            self.stage(bgra, stride, picture.size())?
        } else {
            bail!("{}: a picture this encoder can't read", self.name);
        };
        let (w, h) = (CVPixelBufferGetWidth(&src) as u32, CVPixelBufferGetHeight(&src) as u32);
        if (w, h) == (self.width, self.height) && CVPixelBufferGetPixelFormatType(&src) == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange {
            return Ok(src);
        }
        let pool = unsafe { self.session.0.pixel_buffer_pool() }.ok_or_else(|| anyhow!("{}: the session has no pixel buffer pool", self.name))?;
        let mut raw: *mut CVPixelBuffer = null_mut();
        let r = unsafe { CVPixelBufferPool::create_pixel_buffer(None, &pool, NonNull::from(&mut raw)) };
        if r != 0 || raw.is_null() {
            bail!("CVPixelBufferPoolCreatePixelBuffer: CVReturn {r}");
        }
        let dst = unsafe { CFRetained::from_raw(NonNull::new_unchecked(raw)) };
        let r = unsafe { self.transfer()?.transfer_image(&src, &dst) };
        if r != 0 {
            bail!("VTPixelTransferSessionTransferImage {w}x{h} → {}x{}: OSStatus {r}", self.width, self.height);
        }
        Ok(dst)
    }

    /// A CPU BGRA picture copied into a pixel buffer of its size.
    fn stage(&mut self, bgra: &[u8], stride: usize, (w, h): (u32, u32)) -> Result<CFRetained<CVPixelBuffer>> {
        let fits = self.staging.as_ref().is_some_and(|b| CVPixelBufferGetWidth(&b.0) as u32 == w && CVPixelBufferGetHeight(&b.0) as u32 == h);
        if !fits {
            let iosurface = CFDictionary::<CFString, CFType>::empty();
            let attrs = unsafe { dict(&[(kCVPixelBufferIOSurfacePropertiesKey, &iosurface)]) };
            let mut raw: *mut CVPixelBuffer = null_mut();
            let r = unsafe { CVPixelBufferCreate(None, w as usize, h as usize, kCVPixelFormatType_32BGRA, Some(attrs.as_opaque()), NonNull::from(&mut raw)) };
            if r != 0 || raw.is_null() {
                bail!("CVPixelBufferCreate {w}x{h}: CVReturn {r}");
            }
            self.staging = Some(Cf(unsafe { CFRetained::from_raw(NonNull::new_unchecked(raw)) }));
        }
        let b = self.staging.as_ref().unwrap().0.clone();
        unsafe {
            CVPixelBufferLockBaseAddress(&b, CVPixelBufferLockFlags::empty());
            let base = CVPixelBufferGetBaseAddress(&b) as *mut u8;
            let row = CVPixelBufferGetBytesPerRow(&b);
            let n = (w as usize * 4).min(stride).min(row);
            for y in 0..h as usize {
                std::ptr::copy_nonoverlapping(bgra.as_ptr().add(y * stride), base.add(y * row), n);
            }
            CVPixelBufferUnlockBaseAddress(&b, CVPixelBufferLockFlags::empty());
        }
        Ok(b)
    }
}

impl Encoder for VtEncoder {
    fn name(&self) -> &str {
        self.name
    }
    fn codec(&self) -> Codec {
        self.codec
    }
    fn dynamic_rate(&self) -> bool {
        true
    }
    fn encode(&mut self, picture: &dyn Picture, keyframe: bool, bitrate_bps: u64) -> Result<EncodedFrame> {
        let t0 = Instant::now();
        if bitrate_bps > 0 && (bitrate_bps as f64 / self.bps.max(1) as f64 - 1.0).abs() > 0.1 {
            set_rate(&self.session.0, bitrate_bps);
            self.bps = bitrate_bps;
        }
        let buffer = self.input(picture)?;
        let t1 = Instant::now();
        // Real time in µs: the rate control sees the frames' true spacing (frames come
        // only on damage).
        let pts = (t1.duration_since(self.epoch).as_micros() as i64).max(self.last_pts + 1);
        self.last_pts = pts;
        let pts = unsafe { CMTime::new(pts, 1_000_000) };
        let props = keyframe.then(|| unsafe { dict(&[(kVTEncodeFrameOptionKey_ForceKeyFrame, CFBoolean::new(true))]) });
        let mut slot = Slot::default();
        let r = unsafe {
            self.session.0.encode_frame(
                &buffer,
                pts,
                kCMTimeInvalid,
                props.as_ref().map(|d| d.as_opaque()),
                (&mut slot as *mut Slot).cast(),
                null_mut(),
            )
        };
        if r != 0 {
            bail!("{}: VTCompressionSessionEncodeFrame: OSStatus {r}", self.name);
        }
        let r = unsafe { self.session.0.complete_frames(pts) };
        if r != 0 {
            bail!("{}: VTCompressionSessionCompleteFrames: OSStatus {r}", self.name);
        }
        if slot.status != 0 {
            bail!("{}: the encoder failed the frame: OSStatus {}", self.name, slot.status);
        }
        let t2 = Instant::now();
        Ok(EncodedFrame {
            data: slot.data,
            keyframe: slot.key,
            convert_us: t1.duration_since(t0).as_micros() as u32,
            encode_us: t2.duration_since(t1).as_micros() as u32,
        })
    }
}

impl Drop for VtEncoder {
    fn drop(&mut self) {
        unsafe { self.session.0.invalidate() };
        if let Some(t) = &self.transfer {
            unsafe { t.0.invalidate() };
        }
    }
}

/// Which encoders work here, found once by opening each and encoding one frame.
pub struct Probe {
    only: Option<Codec>,
    ok: OnceLock<EncoderCaps>,
}

impl Probe {
    pub fn new(only: Option<&str>) -> Result<Probe> {
        let only = match only {
            None => None,
            Some(HEVC) => Some(Codec::Hevc420),
            Some(H264) => Some(Codec::H264),
            Some(other) => bail!("no encoder {other:?} on the Mac ({HEVC} or {H264})"),
        };
        Ok(Probe { only, ok: OnceLock::new() })
    }

    pub fn caps(&self) -> EncoderCaps {
        *self.ok.get_or_init(|| {
            let pic: Arc<dyn Picture> = Arc::new(crate::backend::CpuPicture::new(640, 360));
            let mut caps = EncoderCaps::default();
            for codec in [Codec::Hevc420, Codec::H264] {
                if self.only.is_some_and(|c| c != codec) {
                    continue;
                }
                let cfg = EncoderConfig { codec, width: 640, height: 360, fps: 30, bitrate_bps: 2_000_000 };
                let name = if codec == Codec::H264 { H264 } else { HEVC };
                match VtEncoder::open(&cfg).and_then(|mut e| e.encode(pic.as_ref(), true, cfg.bitrate_bps)) {
                    Ok(f) if !f.data.is_empty() => {
                        eprintln!("sodium-remote-host: encoder {name}: works ({} bytes for the test keyframe)", f.data.len());
                        if codec == Codec::H264 {
                            caps.h264 = true;
                        } else {
                            caps.hevc = true;
                        }
                    }
                    Ok(_) => eprintln!("sodium-remote-host: encoder {name}: gave nothing for the test frame"),
                    Err(e) => eprintln!("sodium-remote-host: encoder {name}: {e:#}"),
                }
            }
            caps
        })
    }

    pub fn open(&self, cfg: &EncoderConfig) -> Result<Box<dyn Encoder>> {
        let caps = self.caps();
        let ok = if cfg.codec == Codec::H264 { caps.h264 } else { caps.hevc };
        if !ok {
            bail!("no {:?} encoder on this Mac", cfg.codec);
        }
        Ok(Box::new(VtEncoder::open(cfg)?))
    }
}
