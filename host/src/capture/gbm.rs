// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Minimal libgbm FFI: allocates the DMA-BUFs that ext-image-copy-capture
//! copies into (the plugin path gets its buffers from Hyprland instead).

use std::{
    ffi::{c_int, c_uint},
    fs::File,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use super::{Dmabuf, DmabufPlane};

#[repr(C)]
struct RawDevice {
    _p: [u8; 0],
}
#[repr(C)]
struct RawBo {
    _p: [u8; 0],
}

const GBM_BO_USE_RENDERING: u32 = 1 << 2;

unsafe extern "C" {
    fn gbm_create_device(fd: c_int) -> *mut RawDevice;
    fn gbm_device_destroy(d: *mut RawDevice);
    fn gbm_bo_create_with_modifiers2(
        d: *mut RawDevice,
        w: u32,
        h: u32,
        format: u32,
        modifiers: *const u64,
        count: c_uint,
        flags: u32,
    ) -> *mut RawBo;
    fn gbm_bo_destroy(bo: *mut RawBo);
    fn gbm_bo_get_plane_count(bo: *mut RawBo) -> c_int;
    fn gbm_bo_get_fd_for_plane(bo: *mut RawBo, plane: c_int) -> c_int;
    fn gbm_bo_get_stride_for_plane(bo: *mut RawBo, plane: c_int) -> u32;
    fn gbm_bo_get_offset(bo: *mut RawBo, plane: c_int) -> u32;
    fn gbm_bo_get_modifier(bo: *mut RawBo) -> u64;
}

pub struct GbmDevice {
    dev: *mut RawDevice,
    _file: File,
}

impl GbmDevice {
    pub fn open(path: &Path) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        let dev = unsafe { gbm_create_device(file.as_raw_fd()) };
        if dev.is_null() {
            bail!("gbm_create_device failed on {}", path.display());
        }
        Ok(GbmDevice { dev, _file: file })
    }

    /// Allocates one buffer with one of `modifiers` and returns it as a DMA-BUF.
    pub fn alloc(&self, width: u32, height: u32, format: u32, modifiers: &[u64]) -> Result<Dmabuf> {
        let bo = unsafe {
            gbm_bo_create_with_modifiers2(
                self.dev,
                width,
                height,
                format,
                modifiers.as_ptr(),
                modifiers.len() as c_uint,
                GBM_BO_USE_RENDERING,
            )
        };
        if bo.is_null() {
            bail!("gbm_bo_create_with_modifiers2 failed ({width}x{height}, {} modifiers)", modifiers.len());
        }
        let r = (|| {
            let n = unsafe { gbm_bo_get_plane_count(bo) };
            let modifier = unsafe { gbm_bo_get_modifier(bo) };
            let mut planes = Vec::new();
            for i in 0..n {
                let fd = unsafe { gbm_bo_get_fd_for_plane(bo, i) };
                if fd < 0 {
                    bail!("gbm_bo_get_fd_for_plane failed");
                }
                planes.push(DmabufPlane {
                    fd: unsafe { OwnedFd::from_raw_fd(fd) },
                    offset: unsafe { gbm_bo_get_offset(bo, i) },
                    stride: unsafe { gbm_bo_get_stride_for_plane(bo, i) },
                });
            }
            Ok(Dmabuf { width, height, format, modifier, planes })
        })();
        // The DMA-BUF fds keep the memory alive; the bo itself isn't needed.
        unsafe { gbm_bo_destroy(bo) };
        r
    }
}

impl Drop for GbmDevice {
    fn drop(&mut self) {
        unsafe { gbm_device_destroy(self.dev) };
    }
}

/// The render node for a DRM device number (a primary or render node's st_rdev).
pub fn render_node_for(dev: u64) -> Result<PathBuf> {
    let (maj, min) = (rustix::fs::major(dev), rustix::fs::minor(dev));
    let sys = PathBuf::from(format!("/sys/dev/char/{maj}:{min}/device/drm"));
    for e in std::fs::read_dir(&sys).with_context(|| format!("reading {}", sys.display()))? {
        let name = e?.file_name();
        if name.to_string_lossy().starts_with("renderD") {
            return Ok(Path::new("/dev/dri").join(name));
        }
    }
    bail!("no render node for DRM device {maj}:{min}")
}
