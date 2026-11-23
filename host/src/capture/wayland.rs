// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The fallback capture source: ext-image-copy-capture-v1 into DMA-BUFs.
//!
//! remoted allocates a ring of DMA-BUFs with GBM (on the device and with the
//! modifiers the compositor offers, intersected with what Vulkan can sample),
//! hands them to the compositor as wl_buffers, and asks for one frame at a time.
//! While the pipeline converts and encodes frame N, the compositor already
//! copies frame N+1 into the next buffer.
//!
//! Hyprland 0.57 sends full damage on this path (its screenshare damage is a TODO).

use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};

use anyhow::{Context, Result, anyhow, bail, ensure};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_registry::WlRegistry},
};
use wayland_protocols::{
    ext::{
        image_capture_source::v1::client::{
            ext_image_capture_source_v1::ExtImageCaptureSourceV1,
            ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
        },
        image_copy_capture::v1::client::{
            ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
            ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
            ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
        },
    },
    wp::linux_dmabuf::zv1::client::{
        zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
        zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    },
};

use super::{
    CaptureSource, DRM_FORMAT_ARGB8888, DRM_FORMAT_XRGB8888, Dmabuf, Frame, Rect,
    gbm::{GbmDevice, render_node_for},
    monotonic_ns,
};

#[derive(Default)]
struct SessionInfo {
    size: Option<(u32, u32)>,
    device: Option<u64>,
    formats: Vec<(u32, Vec<u64>)>,
    done: bool,
    stopped: bool,
}

#[derive(Default)]
struct FrameState {
    damage: Vec<Rect>,
    presented_ns: Option<u64>,
    ready: bool,
    failed: Option<u32>,
}

#[derive(Default)]
struct State {
    outputs: Vec<(WlOutput, Option<String>)>,
    session: SessionInfo,
    frame: FrameState,
}

pub struct WaylandSource {
    _conn: Connection,
    queue: EventQueue<State>,
    state: State,
    qh: QueueHandle<State>,
    session: ExtImageCopyCaptureSessionV1,
    _source: ExtImageCaptureSourceV1,
    pub output_name: String,
    pub gbm_node: std::path::PathBuf,
    bufs: Vec<Dmabuf>,
    wl_bufs: Vec<WlBuffer>,
    held: Vec<bool>,
    pending: Option<(u32, ExtImageCopyCaptureFrameV1, u64)>,
    seq: u64,
}

impl WaylandSource {
    /// Connects to `$WAYLAND_DISPLAY` and opens a capture session on `output`
    /// (the first output when `None`). `vk_modifiers(format)` lists the modifiers
    /// the importer can sample for a DRM format.
    pub fn new(output: Option<&str>, ring: u32, vk_modifiers: impl Fn(u32) -> Vec<u64>) -> Result<Self> {
        let conn = Connection::connect_to_env().context("connecting to $WAYLAND_DISPLAY")?;
        let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
        let qh = queue.handle();
        let mut state = State::default();

        let src_mgr: ExtOutputImageCaptureSourceManagerV1 =
            globals.bind(&qh, 1..=1, ()).context("compositor lacks ext_output_image_capture_source_manager_v1")?;
        let cap_mgr: ExtImageCopyCaptureManagerV1 =
            globals.bind(&qh, 1..=1, ()).context("compositor lacks ext_image_copy_capture_manager_v1")?;
        let dmabuf: ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=5, ()).context("compositor lacks zwp_linux_dmabuf_v1")?;
        let names: Vec<(u32, u32)> = globals.contents().with_list(|l| {
            l.iter().filter(|g| g.interface == "wl_output").map(|g| (g.name, g.version)).collect()
        });
        for (name, version) in names {
            let o: WlOutput = globals.registry().bind(name, version.min(4), &qh, ());
            state.outputs.push((o, None));
        }
        queue.roundtrip(&mut state)?;

        let (wl_output, output_name) = match output {
            Some(want) => state
                .outputs
                .iter()
                .find(|(_, n)| n.as_deref() == Some(want))
                .cloned()
                .ok_or_else(|| anyhow!("no output named {want}"))?,
            None => state.outputs.first().cloned().ok_or_else(|| anyhow!("no outputs"))?,
        };
        let output_name = output_name.unwrap_or_default();

        let source = src_mgr.create_source(&wl_output, &qh, ());
        let session = cap_mgr.create_session(&source, Options::empty(), &qh, ());
        while !state.session.done && !state.session.stopped {
            queue.blocking_dispatch(&mut state)?;
        }
        ensure!(!state.session.stopped, "capture session stopped at once");
        let (w, h) = state.session.size.ok_or_else(|| anyhow!("no buffer_size from the compositor"))?;
        let dev = state.session.device.ok_or_else(|| anyhow!("compositor offers no DMA-BUF device"))?;

        // XRGB8888 first (the capture contract's format), ARGB8888 otherwise.
        let (format, offered) = [DRM_FORMAT_XRGB8888, DRM_FORMAT_ARGB8888]
            .iter()
            .find_map(|f| state.session.formats.iter().find(|(g, _)| g == f).cloned())
            .ok_or_else(|| anyhow!("no XRGB8888/ARGB8888 DMA-BUF format offered"))?;
        let ours = vk_modifiers(format);
        let mods: Vec<u64> = offered.into_iter().filter(|m| ours.contains(m)).collect();
        ensure!(!mods.is_empty(), "no modifier both the compositor and Vulkan accept");

        let gbm_node = render_node_for(dev)?;
        let gbm = GbmDevice::open(&gbm_node)?;
        let mut bufs = Vec::new();
        let mut wl_bufs = Vec::new();
        for _ in 0..ring {
            let b = gbm.alloc(w, h, format, &mods)?;
            let params = dmabuf.create_params(&qh, ());
            for (i, p) in b.planes.iter().enumerate() {
                params.add(p.fd.as_fd(), i as u32, p.offset, p.stride, (b.modifier >> 32) as u32, b.modifier as u32);
            }
            let wb = params.create_immed(w as i32, h as i32, format, zwp_linux_buffer_params_v1::Flags::empty(), &qh, ());
            params.destroy();
            bufs.push(b);
            wl_bufs.push(wb);
        }
        queue.roundtrip(&mut state)?;

        Ok(WaylandSource {
            _conn: conn,
            queue,
            state,
            qh,
            session,
            _source: source,
            output_name,
            gbm_node,
            held: vec![false; bufs.len()],
            bufs,
            wl_bufs,
            pending: None,
            seq: 0,
        })
    }

    fn request(&mut self) -> Result<()> {
        if self.pending.is_some() {
            return Ok(());
        }
        let Some(slot) = (0..self.bufs.len()).find(|&i| !self.held[i]) else {
            return Ok(()); // every buffer is with the pipeline; ask again on release
        };
        self.state.frame = FrameState::default();
        let f = self.session.create_frame(&self.qh, ());
        let (w, h) = (self.bufs[slot].width as i32, self.bufs[slot].height as i32);
        f.attach_buffer(&self.wl_bufs[slot]);
        f.damage_buffer(0, 0, w, h);
        f.capture();
        self.pending = Some((slot as u32, f, monotonic_ns()));
        self.queue.flush()?;
        Ok(())
    }
}

impl CaptureSource for WaylandSource {
    fn buffers(&self) -> (u32, &[Dmabuf]) {
        (0, &self.bufs)
    }

    fn next_frame(&mut self) -> Result<Frame> {
        self.request()?;
        let Some((slot, _, t_req)) = &self.pending else {
            bail!("every capture buffer is held by the pipeline");
        };
        let (slot, t_req) = (*slot, *t_req);
        while !self.state.frame.ready && self.state.frame.failed.is_none() {
            ensure!(!self.state.session.stopped, "capture session stopped");
            self.queue.blocking_dispatch(&mut self.state)?;
        }
        let t_ready_ns = monotonic_ns();
        let (_, f, _) = self.pending.take().unwrap();
        f.destroy();
        if let Some(reason) = self.state.frame.failed {
            bail!("capture failed (reason {reason}: 0 unknown, 1 buffer constraints, 2 stopped)");
        }
        self.held[slot as usize] = true;
        let fs = std::mem::take(&mut self.state.frame);
        self.seq += 1;
        let acquire = export_sync_file(&self.bufs[slot as usize].planes[0].fd);
        // Start copying the next frame while this one is converted and encoded.
        self.request()?;
        Ok(Frame {
            generation: 0,
            buffer: slot,
            seq: self.seq,
            t_rendered_ns: fs.presented_ns.unwrap_or(t_req),
            t_ready_ns,
            damage: fs.damage,
            acquire,
        })
    }

    fn release(&mut self, frame: &Frame) -> Result<()> {
        self.held[frame.buffer as usize] = false;
        self.request()
    }
}

/// The DMA-BUF's implicit write fence as a sync_file (DMA_BUF_IOCTL_EXPORT_SYNC_FILE),
/// so the GPU waits for the compositor's copy instead of trusting `ready` alone.
fn export_sync_file(fd: &OwnedFd) -> Option<OwnedFd> {
    #[repr(C)]
    struct ExportSyncFile {
        flags: u32,
        fd: i32,
    }
    const DMA_BUF_SYNC_READ: u32 = 1;
    const DMA_BUF_IOCTL_EXPORT_SYNC_FILE: libc::c_ulong = 0xC008_6202; // _IOWR('b', 2, 8 bytes)
    let mut arg = ExportSyncFile { flags: DMA_BUF_SYNC_READ, fd: -1 };
    let r = unsafe { libc::ioctl(fd.as_raw_fd(), DMA_BUF_IOCTL_EXPORT_SYNC_FILE, &mut arg) };
    (r == 0 && arg.fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(arg.fd) })
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as wayland_client::Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlOutput, ()> for State {
    fn event(s: &mut Self, o: &WlOutput, ev: <WlOutput as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wayland_client::protocol::wl_output::Event::Name { name } = ev {
            if let Some(e) = s.outputs.iter_mut().find(|(x, _)| x == o) {
                e.1 = Some(name);
            }
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for State {
    fn event(s: &mut Self, _: &ExtImageCopyCaptureSessionV1, ev: ext_image_copy_capture_session_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use ext_image_copy_capture_session_v1::Event;
        match ev {
            Event::BufferSize { width, height } => s.session.size = Some((width, height)),
            Event::DmabufDevice { device } => {
                if device.len() == 8 {
                    s.session.device = Some(u64::from_ne_bytes(device[..8].try_into().unwrap()));
                }
            }
            Event::DmabufFormat { format, modifiers } => {
                let mods = modifiers.chunks_exact(8).map(|c| u64::from_ne_bytes(c.try_into().unwrap())).collect();
                s.session.formats.push((format, mods));
            }
            Event::Done => s.session.done = true,
            Event::Stopped => s.session.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for State {
    fn event(s: &mut Self, _: &ExtImageCopyCaptureFrameV1, ev: ext_image_copy_capture_frame_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use ext_image_copy_capture_frame_v1::Event;
        match ev {
            Event::Damage { x, y, width, height } => s.frame.damage.push(Rect { x, y, w: width, h: height }),
            Event::PresentationTime { tv_sec_hi, tv_sec_lo, tv_nsec } => {
                let sec = ((tv_sec_hi as u64) << 32) | tv_sec_lo as u64;
                s.frame.presented_ns = Some(sec * 1_000_000_000 + tv_nsec as u64);
            }
            Event::Ready => s.frame.ready = true,
            Event::Failed { reason } => {
                s.frame.failed = Some(match reason {
                    WEnum::Value(r) => r as u32,
                    WEnum::Unknown(r) => r,
                })
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(State: ExtOutputImageCaptureSourceManagerV1);
wayland_client::delegate_noop!(State: ExtImageCopyCaptureManagerV1);
wayland_client::delegate_noop!(State: ExtImageCaptureSourceV1);
wayland_client::delegate_noop!(State: ignore ZwpLinuxDmabufV1);
wayland_client::delegate_noop!(State: ignore ZwpLinuxBufferParamsV1);
wayland_client::delegate_noop!(State: ignore WlBuffer);
