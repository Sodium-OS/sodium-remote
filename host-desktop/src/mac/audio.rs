// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The Mac's sound, from ScreenCaptureKit: a stream over the main
//! display with `capturesAudio` on, `excludesCurrentProcessAudio` on (Remote.app's own
//! sounds stay out), 48 kHz stereo, and a 2 × 2 picture at 1 fps that is ignored. The
//! samples (Float32, usually one buffer per channel) are interleaved into a queue that
//! `read` takes 10 ms at a time; more than 200 ms waiting is dropped from the front, so
//! a stall never plays late. Silence still flows while nothing plays.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use objc2::AllocAnyThread;
use objc2::rc::autoreleasepool;
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved};
use objc2_core_media::{CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMSampleBuffer, CMTime, kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment};
use objc2_core_graphics::CGMainDisplayID;
use objc2_foundation::NSArray;
use objc2_screen_capture_kit::{SCContentFilter, SCStreamConfiguration, SCStreamOutputType};

use super::capture::{Sink, Stream, find_display, shareable_content};
use crate::backend::AudioSource;

const RATE: f64 = 48_000.0;
/// 200 ms of interleaved stereo.
const MAX_QUEUED: usize = 19_200;

#[derive(Default)]
struct Queue {
    samples: VecDeque<f32>,
    stopped: Option<String>,
    warned_format: bool,
}

#[derive(Default)]
pub(super) struct AudioSink {
    q: Mutex<Queue>,
    cv: Condvar,
}

impl AudioSink {
    pub(super) fn stopped(&self, why: String) {
        self.q.lock().unwrap().stopped = Some(why);
        self.cv.notify_all();
    }

    pub(super) fn on_sample(&self, sample: &CMSampleBuffer) {
        let Some(fmt) = (unsafe { sample.format_description() }) else { return };
        let asbd = unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&fmt).as_ref() };
        let Some(asbd) = asbd.copied() else { return };
        let float = asbd.mFormatFlags & kAudioFormatFlagIsFloat != 0 && asbd.mBitsPerChannel == 32;
        if !float || (asbd.mSampleRate - RATE).abs() > 1.0 {
            let mut q = self.q.lock().unwrap();
            if !q.warned_format {
                q.warned_format = true;
                eprintln!(
                    "sodium-remote-host: system audio comes as {} Hz, flags {:#x}, {} bits: not the 48 kHz float asked for; dropped",
                    asbd.mSampleRate, asbd.mFormatFlags, asbd.mBitsPerChannel
                );
            }
            return;
        }
        let planar = asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0;
        let channels = asbd.mChannelsPerFrame.max(1) as usize;
        // The buffer list, sized for one buffer per channel.
        let mut need = 0usize;
        unsafe {
            sample.audio_buffer_list_with_retained_block_buffer(&mut need, std::ptr::null_mut(), 0, None, None, 0, std::ptr::null_mut());
        }
        let words = need.div_ceil(8).max(4);
        let mut storage = vec![0u64; words];
        let list = storage.as_mut_ptr() as *mut AudioBufferList;
        let mut block: *mut CMBlockBuffer = std::ptr::null_mut();
        let r = unsafe {
            sample.audio_buffer_list_with_retained_block_buffer(
                std::ptr::null_mut(),
                list,
                words * 8,
                None,
                None,
                kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
                &mut block,
            )
        };
        if r != 0 {
            return;
        }
        // Release the block buffer the call retained for us, once we're done reading.
        let _block = std::ptr::NonNull::new(block).map(|b| unsafe { objc2_core_foundation::CFRetained::from_raw(b) });
        let n_buffers = unsafe { (*list).mNumberBuffers } as usize;
        let buffers: &[AudioBuffer] = unsafe { std::slice::from_raw_parts((*list).mBuffers.as_ptr(), n_buffers) };
        let plane = |i: usize| -> &[f32] {
            let b = &buffers[i.min(n_buffers - 1)];
            if b.mData.is_null() {
                return &[];
            }
            unsafe { std::slice::from_raw_parts(b.mData as *const f32, b.mDataByteSize as usize / 4) }
        };
        if n_buffers == 0 {
            return;
        }
        let mut q = self.q.lock().unwrap();
        if planar {
            let (l, r) = (plane(0), plane(if channels > 1 { 1 } else { 0 }));
            for i in 0..l.len().min(r.len()) {
                q.samples.push_back(l[i]);
                q.samples.push_back(r[i]);
            }
        } else {
            let data = plane(0);
            for frame in data.chunks_exact(channels) {
                q.samples.push_back(frame[0]);
                q.samples.push_back(if channels > 1 { frame[1] } else { frame[0] });
            }
        }
        let extra = q.samples.len().saturating_sub(MAX_QUEUED);
        q.samples.drain(..extra);
        drop(q);
        self.cv.notify_all();
    }
}

pub struct ScAudio {
    sink: Arc<AudioSink>,
    _stream: Stream,
}

impl ScAudio {
    pub fn open() -> Result<ScAudio> {
        let sink = Arc::new(AudioSink::default());
        let stream = autoreleasepool(|_| -> Result<Stream> {
            let content = shareable_content()?;
            let scd = find_display(&content, CGMainDisplayID())?;
            let none = NSArray::new();
            let filter = unsafe { SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &scd, &none) };
            let config = unsafe { SCStreamConfiguration::new() };
            unsafe {
                config.setCapturesAudio(true);
                config.setExcludesCurrentProcessAudio(true);
                config.setSampleRate(RATE as isize);
                config.setChannelCount(2);
                config.setWidth(2);
                config.setHeight(2);
                config.setMinimumFrameInterval(CMTime::new(1, 1));
                config.setQueueDepth(3);
            }
            Stream::start(&filter, &config, Sink::Audio(sink.clone()), SCStreamOutputType::Audio, "com.sodiumos.remote.host.audio")
        })?;
        Ok(ScAudio { sink, _stream: stream })
    }
}

impl AudioSource for ScAudio {
    fn read(&mut self, out: &mut [f32; 960], timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        let mut q = self.sink.q.lock().unwrap();
        loop {
            if let Some(why) = q.stopped.take() {
                bail!("{why}");
            }
            if q.samples.len() >= out.len() {
                for (o, s) in out.iter_mut().zip(q.samples.drain(..960)) {
                    *o = s;
                }
                return Ok(true);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(false);
            }
            q = self.sink.cv.wait_timeout(q, left).unwrap().0;
        }
    }
}
