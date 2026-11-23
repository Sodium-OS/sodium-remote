// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Vulkan: DMA-BUF import (with DRM format modifiers), the XRGB -> YUV compute
//! pass, and the output buffers exported to CUDA as opaque fds (Route A).

use std::{
    ffi::CStr,
    os::fd::{FromRawFd, IntoRawFd, OwnedFd},
    time::Instant,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use ash::vk;

use crate::capture::{DRM_FORMAT_ABGR8888, DRM_FORMAT_ARGB8888, DRM_FORMAT_XBGR8888, DRM_FORMAT_XRGB8888, Dmabuf};

static SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rgb_to_yuv.spv"));

/// What the GPU pass writes for the encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutFormat {
    /// 4:2:0, Y plane then interleaved CbCr.
    Nv12,
    /// 4:4:4 planar: Y, Cb, Cr.
    Yuv444,
    /// No conversion: the pixels copied as they are (B,G,R,X bytes); NVENC converts.
    Argb,
}

pub fn vk_format_for(drm: u32) -> Option<vk::Format> {
    match drm {
        DRM_FORMAT_XRGB8888 | DRM_FORMAT_ARGB8888 => Some(vk::Format::B8G8R8A8_UNORM),
        DRM_FORMAT_XBGR8888 | DRM_FORMAT_ABGR8888 => Some(vk::Format::R8G8B8A8_UNORM),
        _ => None,
    }
}

pub struct Gpu {
    _entry: ash::Entry,
    instance: ash::Instance,
    pdev: vk::PhysicalDevice,
    pub device: ash::Device,
    queue: vk::Queue,
    qfi: u32,
    pub uuid: [u8; 16],
    pub name: String,
    mem_props: vk::PhysicalDeviceMemoryProperties,
    ext_mem: ash::khr::external_memory_fd::Device,
    ext_sem: ash::khr::external_semaphore_fd::Device,
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    acquire_sem: vk::Semaphore,
    dsl: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    sampler: vk::Sampler,
    desc_pool: vk::DescriptorPool,
    desc_set: vk::DescriptorSet,
}

/// A source image in Vulkan: an imported DMA-BUF (owned by another queue
/// family, the compositor's) or, in tests, a plain local image.
pub struct SrcImage {
    pub image: vk::Image,
    pub view: vk::ImageView,
    memory: Vec<vk::DeviceMemory>,
    pub width: u32,
    pub height: u32,
    foreign: bool,
}

/// A linear YUV (or ARGB) buffer the encoder reads, exportable to CUDA.
pub struct OutBuffer {
    pub buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    pub alloc_size: u64,
    pub format: OutFormat,
    pub pitch: u32,
    pub off_u: u32,
    pub off_v: u32,
}

/// A host-visible buffer for reading pixels back (source dumps, tests).
pub struct Readback {
    pub buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    ptr: *const u8,
    pub size: usize,
}

impl Readback {
    pub fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }

    /// The buffer as u32 words (change flags).
    pub fn words(&self) -> &[u32] {
        unsafe { std::slice::from_raw_parts(self.ptr as *const u32, self.size / 4) }
    }

    /// Zeroes the buffer from the host (coherent memory; the next submit sees it).
    pub fn clear(&self) {
        unsafe { std::ptr::write_bytes(self.ptr as *mut u8, 0, self.size) }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ConvertTiming {
    /// CPU time to record and submit.
    pub record_ms: f64,
    /// Submit until the fence signals (includes waiting on the acquire fence).
    pub gpu_ms: f64,
}

const fn align(v: u32, a: u32) -> u32 {
    v.div_ceil(a) * a
}

impl Gpu {
    /// Opens the NVIDIA GPU (the first discrete GPU otherwise).
    pub fn new() -> Result<Self> {
        unsafe { Self::new_inner() }
    }

    unsafe fn new_inner() -> Result<Self> {
        let entry = unsafe { ash::Entry::load() }.context("loading libvulkan.so.1")?;
        let app = vk::ApplicationInfo::default().application_name(c"remoted").api_version(vk::API_VERSION_1_3);
        let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None)? };

        let pdevs = unsafe { instance.enumerate_physical_devices()? };
        let pick = |want: &dyn Fn(&vk::PhysicalDeviceProperties) -> bool| {
            pdevs.iter().copied().find(|&p| want(&unsafe { instance.get_physical_device_properties(p) }))
        };
        let pdev = pick(&|p| p.vendor_id == 0x10de)
            .or_else(|| pick(&|p| p.device_type == vk::PhysicalDeviceType::DISCRETE_GPU))
            .ok_or_else(|| anyhow!("no suitable Vulkan device"))?;
        let mut id = vk::PhysicalDeviceIDProperties::default();
        let mut props2 = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
        unsafe { instance.get_physical_device_properties2(pdev, &mut props2) };
        let name = props2.properties.device_name_as_c_str().unwrap_or(c"?").to_string_lossy().into_owned();
        let uuid = id.device_uuid;

        let qfi = unsafe { instance.get_physical_device_queue_family_properties(pdev) }
            .iter()
            .position(|q| q.queue_flags.contains(vk::QueueFlags::COMPUTE))
            .ok_or_else(|| anyhow!("no compute queue"))? as u32;

        let exts: [&CStr; 5] = [
            ash::khr::external_memory_fd::NAME,
            ash::ext::external_memory_dma_buf::NAME,
            ash::ext::image_drm_format_modifier::NAME,
            ash::khr::external_semaphore_fd::NAME,
            ash::ext::queue_family_foreign::NAME,
        ];
        let ext_ptrs: Vec<_> = exts.iter().map(|e| e.as_ptr()).collect();
        let prio = [1.0f32];
        let qci = [vk::DeviceQueueCreateInfo::default().queue_family_index(qfi).queue_priorities(&prio)];
        let device = unsafe {
            instance.create_device(
                pdev,
                &vk::DeviceCreateInfo::default().queue_create_infos(&qci).enabled_extension_names(&ext_ptrs),
                None,
            )?
        };
        let queue = unsafe { device.get_device_queue(qfi, 0) };
        let mem_props = unsafe { instance.get_physical_device_memory_properties(pdev) };
        let ext_mem = ash::khr::external_memory_fd::Device::new(&instance, &device);
        let ext_sem = ash::khr::external_semaphore_fd::Device::new(&instance, &device);

        let pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(qfi)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?
        };
        let cmd = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default().command_pool(pool).command_buffer_count(1),
            )?[0]
        };
        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None)? };
        let acquire_sem = unsafe { device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)? };

        // The conversion pipeline.
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            // the previous output and the per-cell change flags (change detection)
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE),
        ];
        let dsl = unsafe {
            device.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None)?
        };
        let pcr = [vk::PushConstantRange::default().stage_flags(vk::ShaderStageFlags::COMPUTE).size(32)];
        let dsls = [dsl];
        let layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&dsls).push_constant_ranges(&pcr),
                None,
            )?
        };
        let code = ash::util::read_spv(&mut std::io::Cursor::new(SHADER))?;
        let module = unsafe { device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None)? };
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(c"main");
        let pipeline = unsafe {
            device
                .create_compute_pipelines(
                    vk::PipelineCache::null(),
                    &[vk::ComputePipelineCreateInfo::default().stage(stage).layout(layout)],
                    None,
                )
                .map_err(|(_, e)| e)?[0]
        };
        unsafe { device.destroy_shader_module(module, None) };
        let sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::NEAREST)
                    .min_filter(vk::Filter::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?
        };
        let sizes = [
            vk::DescriptorPoolSize { ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER, descriptor_count: 1 },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 3 },
        ];
        let desc_pool = unsafe {
            device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(1).pool_sizes(&sizes), None)?
        };
        let desc_set = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::default().descriptor_pool(desc_pool).set_layouts(&dsls),
            )?[0]
        };

        Ok(Gpu {
            _entry: entry,
            instance,
            pdev,
            device,
            queue,
            qfi,
            uuid,
            name,
            mem_props,
            ext_mem,
            ext_sem,
            pool,
            cmd,
            fence,
            acquire_sem,
            dsl,
            layout,
            pipeline,
            sampler,
            desc_pool,
            desc_set,
        })
    }

    fn mem_type(&self, bits: u32, want: vk::MemoryPropertyFlags) -> Result<u32> {
        (0..self.mem_props.memory_type_count)
            .find(|&i| bits & (1 << i) != 0 && self.mem_props.memory_types[i as usize].property_flags.contains(want))
            .ok_or_else(|| anyhow!("no memory type {want:?} in {bits:#x}"))
    }

    /// DRM modifiers of `drm_format` this device can import from a DMA-BUF and
    /// sample (single memory plane).
    pub fn sampleable_modifiers(&self, drm_format: u32) -> Vec<u64> {
        let Some(format) = vk_format_for(drm_format) else { return Vec::new() };
        unsafe {
            let mut list = vk::DrmFormatModifierPropertiesListEXT::default();
            let mut fp = vk::FormatProperties2::default().push_next(&mut list);
            self.instance.get_physical_device_format_properties2(self.pdev, format, &mut fp);
            let n = list.drm_format_modifier_count as usize;
            let mut props = vec![vk::DrmFormatModifierPropertiesEXT::default(); n];
            let mut list = vk::DrmFormatModifierPropertiesListEXT::default().drm_format_modifier_properties(&mut props);
            let mut fp = vk::FormatProperties2::default().push_next(&mut list);
            self.instance.get_physical_device_format_properties2(self.pdev, format, &mut fp);
            props
                .iter()
                .filter(|p| {
                    p.drm_format_modifier_plane_count == 1
                        && p.drm_format_modifier_tiling_features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
                        && self.importable(format, p.drm_format_modifier)
                })
                .map(|p| p.drm_format_modifier)
                .collect()
        }
    }

    unsafe fn importable(&self, format: vk::Format, modifier: u64) -> bool {
        let mut drm = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(modifier)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let mut ext = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC)
            .push_next(&mut ext)
            .push_next(&mut drm);
        let mut ext_props = vk::ExternalImageFormatProperties::default();
        let mut props = vk::ImageFormatProperties2::default().push_next(&mut ext_props);
        let ok = unsafe { self.instance.get_physical_device_image_format_properties2(self.pdev, &info, &mut props) }.is_ok();
        ok && ext_props
            .external_memory_properties
            .external_memory_features
            .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
    }

    /// Imports one DMA-BUF of the capture ring as a sampled image.
    pub fn import(&self, buf: &Dmabuf) -> Result<SrcImage> {
        let format = vk_format_for(buf.format).ok_or_else(|| anyhow!("DRM format {:#x} not supported", buf.format))?;
        ensure!(!buf.planes.is_empty(), "DMA-BUF without planes");
        // Several planes in one DMA-BUF (same file) are fine; disjoint planes aren't (not needed for XRGB).
        let ino = |fd: &OwnedFd| rustix::fs::fstat(fd).map(|s| s.st_ino);
        let first = ino(&buf.planes[0].fd)?;
        for p in &buf.planes[1..] {
            ensure!(ino(&p.fd)? == first, "disjoint multi-plane DMA-BUFs are not supported");
        }
        unsafe {
            let layouts: Vec<vk::SubresourceLayout> = buf
                .planes
                .iter()
                .map(|p| vk::SubresourceLayout {
                    offset: p.offset as u64,
                    size: 0,
                    row_pitch: p.stride as u64,
                    array_pitch: 0,
                    depth_pitch: 0,
                })
                .collect();
            let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
                .drm_format_modifier(buf.modifier)
                .plane_layouts(&layouts);
            let mut ext =
                vk::ExternalMemoryImageCreateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
            let image = self.device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D { width: buf.width, height: buf.height, depth: 1 })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
                    .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE)
                    .initial_layout(vk::ImageLayout::UNDEFINED)
                    .push_next(&mut ext)
                    .push_next(&mut explicit),
                None,
            )?;
            let fd = buf.planes[0].fd.try_clone()?;
            let mut fdp = vk::MemoryFdPropertiesKHR::default();
            self.ext_mem.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                std::os::fd::AsRawFd::as_raw_fd(&fd),
                &mut fdp,
            )?;
            let mut req2 = vk::MemoryRequirements2::default();
            self.device
                .get_image_memory_requirements2(&vk::ImageMemoryRequirementsInfo2::default().image(image), &mut req2);
            let req = req2.memory_requirements;
            let idx = self.mem_type(req.memory_type_bits & fdp.memory_type_bits, vk::MemoryPropertyFlags::empty())?;
            let raw = fd.into_raw_fd();
            let mut import = vk::ImportMemoryFdInfoKHR::default()
                .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
                .fd(raw);
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let mem = match self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(idx)
                    .push_next(&mut import)
                    .push_next(&mut dedicated),
                None,
            ) {
                Ok(m) => m,
                Err(e) => {
                    drop(OwnedFd::from_raw_fd(raw)); // not taken on failure
                    self.device.destroy_image(image, None);
                    bail!("importing the DMA-BUF: {e}");
                }
            };
            self.device.bind_image_memory(image, mem, 0)?;
            let view = self.view(image, format)?;
            Ok(SrcImage { image, view, memory: vec![mem], width: buf.width, height: buf.height, foreign: true })
        }
    }

    unsafe fn view(&self, image: vk::Image, format: vk::Format) -> Result<vk::ImageView> {
        Ok(unsafe {
            self.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(color_range()),
                None,
            )?
        })
    }

    pub fn destroy_src(&self, img: SrcImage) {
        unsafe {
            self.device.destroy_image_view(img.view, None);
            self.device.destroy_image(img.image, None);
            for m in img.memory {
                self.device.free_memory(m, None);
            }
        }
    }

    /// An output buffer for `format`, in device memory exportable as an opaque fd.
    pub fn create_out(&self, width: u32, height: u32, format: OutFormat) -> Result<OutBuffer> {
        let (pitch, size, off_u, off_v) = match format {
            OutFormat::Nv12 => {
                let p = align(width, 256);
                (p, p * height + p * height.div_ceil(2), p * height, 0)
            }
            OutFormat::Yuv444 => {
                let p = align(width, 256);
                (p, 3 * p * height, p * height, 2 * p * height)
            }
            OutFormat::Argb => {
                let p = align(width * 4, 256);
                (p, p * height, 0, 0)
            }
        };
        unsafe {
            let mut ext = vk::ExternalMemoryBufferCreateInfo::default()
                .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let buffer = self.device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size as u64)
                    .usage(
                        vk::BufferUsageFlags::STORAGE_BUFFER
                            | vk::BufferUsageFlags::TRANSFER_SRC
                            | vk::BufferUsageFlags::TRANSFER_DST,
                    )
                    .push_next(&mut ext),
                None,
            )?;
            let req = self.device.get_buffer_memory_requirements(buffer);
            let idx = self.mem_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
            let mut export =
                vk::ExportMemoryAllocateInfo::default().handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
            let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().buffer(buffer);
            let memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(idx)
                    .push_next(&mut export)
                    .push_next(&mut dedicated),
                None,
            )?;
            self.device.bind_buffer_memory(buffer, memory, 0)?;
            Ok(OutBuffer { buffer, memory, alloc_size: req.size, format, pitch, off_u, off_v })
        }
    }

    /// The output buffer's memory as an opaque fd, for cuImportExternalMemory.
    pub fn export_fd(&self, out: &OutBuffer) -> Result<OwnedFd> {
        let fd = unsafe {
            self.ext_mem.get_memory_fd(
                &vk::MemoryGetFdInfoKHR::default()
                    .memory(out.memory)
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD),
            )?
        };
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    pub fn destroy_out(&self, out: OutBuffer) {
        unsafe {
            self.device.destroy_buffer(out.buffer, None);
            self.device.free_memory(out.memory, None);
        }
    }

    pub fn create_readback(&self, size: usize) -> Result<Readback> {
        self.create_host_buffer(size, vk::BufferUsageFlags::TRANSFER_DST)
    }

    /// A host-visible buffer the conversion shader writes change flags into: one u32 per cell.
    pub fn create_flags(&self, cells: usize) -> Result<Readback> {
        self.create_host_buffer(cells.max(1) * 4, vk::BufferUsageFlags::STORAGE_BUFFER)
    }

    fn create_host_buffer(&self, size: usize, usage: vk::BufferUsageFlags) -> Result<Readback> {
        unsafe {
            let buffer =
                self.device.create_buffer(&vk::BufferCreateInfo::default().size(size as u64).usage(usage), None)?;
            let req = self.device.get_buffer_memory_requirements(buffer);
            let idx = self.mem_type(
                req.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            )?;
            let memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(idx),
                None,
            )?;
            self.device.bind_buffer_memory(buffer, memory, 0)?;
            let ptr = self.device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *const u8;
            Ok(Readback { buffer, memory, ptr, size })
        }
    }

    /// Copies an output buffer (owned by CUDA between frames) into `rb` and waits:
    /// what the encoder reads, for quality checks of frames encoded without a new
    /// conversion (refinement).
    pub fn dump_out(&mut self, out: &OutBuffer, rb: &Readback) -> Result<()> {
        let d = &self.device;
        unsafe {
            d.reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())?;
            d.begin_command_buffer(
                self.cmd,
                &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let acq = vk::BufferMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                .dst_queue_family_index(self.qfi)
                .buffer(out.buffer)
                .size(vk::WHOLE_SIZE);
            d.cmd_pipeline_barrier(
                self.cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[acq],
                &[],
            );
            let n = (rb.size as u64).min(out.alloc_size);
            d.cmd_copy_buffer(self.cmd, out.buffer, rb.buffer, &[vk::BufferCopy::default().size(n)]);
            let rel = vk::BufferMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                .dst_access_mask(vk::AccessFlags::empty())
                .src_queue_family_index(self.qfi)
                .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                .buffer(out.buffer)
                .size(vk::WHOLE_SIZE);
            d.cmd_pipeline_barrier(
                self.cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[rel],
                &[],
            );
            d.end_command_buffer(self.cmd)?;
            let cmds = [self.cmd];
            d.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&cmds)], self.fence)?;
            d.wait_for_fences(&[self.fence], true, 1_000_000_000)?;
            d.reset_fences(&[self.fence])?;
        }
        Ok(())
    }

    pub fn destroy_readback(&self, r: Readback) {
        unsafe {
            self.device.unmap_memory(r.memory);
            self.device.destroy_buffer(r.buffer, None);
            self.device.free_memory(r.memory, None);
        }
    }

    /// Converts `src` into `out` and waits for the GPU. `acquire` is a sync_file
    /// the GPU waits on first (the compositor's write fence). `src_dump` gets a
    /// tightly packed copy of the source pixels; `out_dump` a copy of `out`.
    pub fn convert(
        &mut self,
        src: &SrcImage,
        out: &OutBuffer,
        acquire: Option<OwnedFd>,
        src_dump: Option<&Readback>,
        out_dump: Option<&Readback>,
    ) -> Result<ConvertTiming> {
        self.convert_diff(src, out, acquire, src_dump, out_dump, None)
    }

    /// [`Gpu::convert`] with change detection: `diff` = (the previous frame's output,
    /// the flags buffer, cell size, cells per row). Each cell whose converted pixels
    /// differ from `prev` gets a non-zero u32 in `flags` (cleared here first).
    pub fn convert_diff(
        &mut self,
        src: &SrcImage,
        out: &OutBuffer,
        acquire: Option<OwnedFd>,
        src_dump: Option<&Readback>,
        out_dump: Option<&Readback>,
        diff: Option<(&OutBuffer, &Readback, u32, u32)>,
    ) -> Result<ConvertTiming> {
        let t0 = Instant::now();
        if let Some((_, flags, _, _)) = diff {
            flags.clear();
        }
        let d = &self.device;
        unsafe {
            d.reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())?;
            d.begin_command_buffer(
                self.cmd,
                &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let (src_qf, dst_qf) = if src.foreign {
                (vk::QUEUE_FAMILY_FOREIGN_EXT, self.qfi)
            } else {
                (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED)
            };
            // Take the image from the compositor's queue family (contents preserved: GENERAL, not UNDEFINED),
            // and the output buffer back from CUDA.
            let img_acq = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::TRANSFER_READ)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(src_qf)
                .dst_queue_family_index(dst_qf)
                .image(src.image)
                .subresource_range(color_range());
            let buf_acq = vk::BufferMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::TRANSFER_WRITE)
                .src_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                .dst_queue_family_index(self.qfi)
                .buffer(out.buffer)
                .size(vk::WHOLE_SIZE);
            let mut bufs = vec![buf_acq];
            if let Some((prev, _, _, _)) = diff {
                // the previous output, read for change detection, also comes back from CUDA
                bufs.push(
                    vk::BufferMemoryBarrier::default()
                        .src_access_mask(vk::AccessFlags::empty())
                        .dst_access_mask(vk::AccessFlags::SHADER_READ)
                        .src_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                        .dst_queue_family_index(self.qfi)
                        .buffer(prev.buffer)
                        .size(vk::WHOLE_SIZE),
                );
            }
            d.cmd_pipeline_barrier(
                self.cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &bufs,
                &[img_acq],
            );

            if out.format == OutFormat::Argb {
                let region = vk::BufferImageCopy::default()
                    .buffer_row_length(out.pitch / 4)
                    .image_subresource(color_layers())
                    .image_extent(vk::Extent3D { width: src.width, height: src.height, depth: 1 });
                d.cmd_copy_image_to_buffer(self.cmd, src.image, vk::ImageLayout::GENERAL, out.buffer, &[region]);
            } else {
                let img_info = [vk::DescriptorImageInfo::default()
                    .sampler(self.sampler)
                    .image_view(src.view)
                    .image_layout(vk::ImageLayout::GENERAL)];
                let buf_info = [vk::DescriptorBufferInfo::default().buffer(out.buffer).range(vk::WHOLE_SIZE)];
                // Without change detection the shader never touches bindings 2 and 3; they
                // still need a valid buffer.
                let prev_info = [vk::DescriptorBufferInfo::default()
                    .buffer(diff.map_or(out.buffer, |(p, _, _, _)| p.buffer))
                    .range(vk::WHOLE_SIZE)];
                let flags_info = [vk::DescriptorBufferInfo::default()
                    .buffer(diff.map_or(out.buffer, |(_, f, _, _)| f.buffer))
                    .range(vk::WHOLE_SIZE)];
                d.update_descriptor_sets(
                    &[
                        vk::WriteDescriptorSet::default()
                            .dst_set(self.desc_set)
                            .dst_binding(0)
                            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                            .image_info(&img_info),
                        vk::WriteDescriptorSet::default()
                            .dst_set(self.desc_set)
                            .dst_binding(1)
                            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                            .buffer_info(&buf_info),
                        vk::WriteDescriptorSet::default()
                            .dst_set(self.desc_set)
                            .dst_binding(2)
                            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                            .buffer_info(&prev_info),
                        vk::WriteDescriptorSet::default()
                            .dst_set(self.desc_set)
                            .dst_binding(3)
                            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                            .buffer_info(&flags_info),
                    ],
                    &[],
                );
                d.cmd_bind_pipeline(self.cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline);
                d.cmd_bind_descriptor_sets(
                    self.cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    self.layout,
                    0,
                    &[self.desc_set],
                    &[],
                );
                let mode = (out.format == OutFormat::Yuv444) as u32;
                let (cell, cols) = diff.map_or((0, 0), |(_, _, c, n)| (c, n));
                let pc: [u32; 8] = [src.width, src.height, out.pitch, out.off_u, out.off_v, mode, cell, cols];
                let bytes: Vec<u8> = pc.iter().flat_map(|v| v.to_le_bytes()).collect();
                d.cmd_push_constants(self.cmd, self.layout, vk::ShaderStageFlags::COMPUTE, 0, &bytes);
                d.cmd_dispatch(self.cmd, src.width.div_ceil(64), src.height.div_ceil(16), 1);
            }

            if let Some(rb) = src_dump {
                let region = vk::BufferImageCopy::default()
                    .image_subresource(color_layers())
                    .image_extent(vk::Extent3D { width: src.width, height: src.height, depth: 1 });
                d.cmd_copy_image_to_buffer(self.cmd, src.image, vk::ImageLayout::GENERAL, rb.buffer, &[region]);
            }
            if let Some(rb) = out_dump {
                let b = vk::BufferMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .buffer(out.buffer)
                    .size(vk::WHOLE_SIZE);
                d.cmd_pipeline_barrier(
                    self.cmd,
                    vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[b],
                    &[],
                );
                let n = (rb.size as u64).min(out.alloc_size);
                d.cmd_copy_buffer(self.cmd, out.buffer, rb.buffer, &[vk::BufferCopy::default().size(n)]);
            }

            // Hand the buffer to CUDA and the image back to the compositor.
            let buf_rel = vk::BufferMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::empty())
                .src_queue_family_index(self.qfi)
                .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                .buffer(out.buffer)
                .size(vk::WHOLE_SIZE);
            let img_rel = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::SHADER_READ | vk::AccessFlags::TRANSFER_READ)
                .dst_access_mask(vk::AccessFlags::empty())
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(dst_qf)
                .dst_queue_family_index(src_qf)
                .image(src.image)
                .subresource_range(color_range());
            let mut rels = vec![buf_rel];
            if let Some((prev, flags, _, _)) = diff {
                rels.push(
                    vk::BufferMemoryBarrier::default()
                        .src_access_mask(vk::AccessFlags::SHADER_READ)
                        .dst_access_mask(vk::AccessFlags::empty())
                        .src_queue_family_index(self.qfi)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                        .buffer(prev.buffer)
                        .size(vk::WHOLE_SIZE),
                );
                d.cmd_pipeline_barrier(
                    self.cmd,
                    vk::PipelineStageFlags::COMPUTE_SHADER,
                    vk::PipelineStageFlags::HOST,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[vk::BufferMemoryBarrier::default()
                        .src_access_mask(vk::AccessFlags::SHADER_WRITE)
                        .dst_access_mask(vk::AccessFlags::HOST_READ)
                        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                        .buffer(flags.buffer)
                        .size(vk::WHOLE_SIZE)],
                    &[],
                );
            }
            d.cmd_pipeline_barrier(
                self.cmd,
                vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &rels,
                &[img_rel],
            );
            d.end_command_buffer(self.cmd)?;

            let mut wait = Vec::new();
            if let Some(fd) = acquire {
                self.ext_sem.import_semaphore_fd(
                    &vk::ImportSemaphoreFdInfoKHR::default()
                        .semaphore(self.acquire_sem)
                        .flags(vk::SemaphoreImportFlags::TEMPORARY)
                        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
                        .fd(fd.into_raw_fd()),
                )?;
                wait.push(self.acquire_sem);
            }
            let stages = [vk::PipelineStageFlags::ALL_COMMANDS];
            let cmds = [self.cmd];
            let submit = vk::SubmitInfo::default()
                .wait_semaphores(&wait)
                .wait_dst_stage_mask(&stages[..wait.len()])
                .command_buffers(&cmds);
            let t1 = Instant::now();
            d.queue_submit(self.queue, &[submit], self.fence)?;
            d.wait_for_fences(&[self.fence], true, 1_000_000_000)?;
            let t2 = Instant::now();
            d.reset_fences(&[self.fence])?;
            Ok(ConvertTiming {
                record_ms: (t1 - t0).as_secs_f64() * 1e3,
                gpu_ms: (t2 - t1).as_secs_f64() * 1e3,
            })
        }
    }

    /// A local (non-imported) image filled with `rgba` pixels (R,G,B,A bytes), for tests.
    #[cfg(test)]
    pub fn upload_test_image(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<SrcImage> {
        ensure!(rgba.len() == (width * height * 4) as usize);
        unsafe {
            let format = vk::Format::R8G8B8A8_UNORM;
            let image = self.device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D { width, height, depth: 1 })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST,
                    ),
                None,
            )?;
            let req = self.device.get_image_memory_requirements(image);
            let mem = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(req.size)
                    .memory_type_index(self.mem_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?),
                None,
            )?;
            self.device.bind_image_memory(image, mem, 0)?;

            // staging
            let staging = self.device.create_buffer(
                &vk::BufferCreateInfo::default().size(rgba.len() as u64).usage(vk::BufferUsageFlags::TRANSFER_SRC),
                None,
            )?;
            let sreq = self.device.get_buffer_memory_requirements(staging);
            let smem = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default().allocation_size(sreq.size).memory_type_index(self.mem_type(
                    sreq.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                )?),
                None,
            )?;
            self.device.bind_buffer_memory(staging, smem, 0)?;
            let p = self.device.map_memory(smem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8;
            std::ptr::copy_nonoverlapping(rgba.as_ptr(), p, rgba.len());
            self.device.unmap_memory(smem);

            let d = &self.device;
            d.reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())?;
            d.begin_command_buffer(self.cmd, &vk::CommandBufferBeginInfo::default())?;
            let to_dst = vk::ImageMemoryBarrier::default()
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(color_range());
            d.cmd_pipeline_barrier(
                self.cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_dst],
            );
            let region = vk::BufferImageCopy::default()
                .image_subresource(color_layers())
                .image_extent(vk::Extent3D { width, height, depth: 1 });
            d.cmd_copy_buffer_to_image(self.cmd, staging, image, vk::ImageLayout::GENERAL, &[region]);
            let done = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(color_range());
            d.cmd_pipeline_barrier(
                self.cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COMPUTE_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[done],
            );
            d.end_command_buffer(self.cmd)?;
            let cmds = [self.cmd];
            d.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&cmds)], self.fence)?;
            d.wait_for_fences(&[self.fence], true, 1_000_000_000)?;
            d.reset_fences(&[self.fence])?;
            d.destroy_buffer(staging, None);
            d.free_memory(smem, None);
            let view = self.view(image, format)?;
            Ok(SrcImage { image, view, memory: vec![mem], width, height, foreign: false })
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_descriptor_pool(self.desc_pool, None);
            self.device.destroy_sampler(self.sampler, None);
            self.device.destroy_pipeline(self.pipeline, None);
            self.device.destroy_pipeline_layout(self.layout, None);
            self.device.destroy_descriptor_set_layout(self.dsl, None);
            self.device.destroy_semaphore(self.acquire_sem, None);
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1)
}

fn color_layers() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1)
}

/// BT.709 full range on sRGB-encoded values: the shader's reference.
#[cfg(test)]
pub fn rgb_to_yuv_ref(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    (y * 255.0, ((b - y) / 1.8556 + 0.5) * 255.0, ((r - y) / 1.5748 + 0.5) * 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The compute pass against a CPU reference, both outputs, on an odd-sized
    /// image (the edge blocks are the boundary case).
    #[test]
    fn convert_matches_reference() {
        let mut gpu = Gpu::new().expect("needs the NVIDIA Vulkan driver");
        let (w, h) = (70u32, 38u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let mut s = 1u32;
        for px in rgba.chunks_mut(4) {
            for c in &mut px[..3] {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                *c = (s >> 24) as u8;
            }
            px[3] = 255;
        }
        let src = gpu.upload_test_image(w, h, &rgba).unwrap();
        let px = |x: u32, y: u32| {
            let i = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
            rgb_to_yuv_ref(rgba[i], rgba[i + 1], rgba[i + 2])
        };
        for format in [OutFormat::Nv12, OutFormat::Yuv444] {
            let out = gpu.create_out(w, h, format).unwrap();
            let rb = gpu.create_readback(out.alloc_size as usize).unwrap();
            gpu.convert(&src, &out, None, None, Some(&rb)).unwrap();
            let b = rb.bytes();
            let p = out.pitch as usize;
            let close = |got: u8, want: f64, what: &str| {
                assert!((got as f64 - want).abs() <= 1.0, "{format:?} {what}: got {got}, want {want:.2}");
            };
            for y in 0..h {
                for x in 0..w {
                    let (yy, cb, cr) = px(x, y);
                    let at = y as usize * p + x as usize;
                    close(b[at], yy, "Y");
                    if format == OutFormat::Yuv444 {
                        close(b[out.off_u as usize + at], cb, "Cb");
                        close(b[out.off_v as usize + at], cr, "Cr");
                    }
                }
            }
            if format == OutFormat::Nv12 {
                for cy in 0..h / 2 {
                    for cx in 0..w / 2 {
                        let s: Vec<_> = [(0, 0), (1, 0), (0, 1), (1, 1)]
                            .iter()
                            .map(|&(dx, dy)| px(2 * cx + dx, 2 * cy + dy))
                            .collect();
                        let cb = s.iter().map(|v| v.1).sum::<f64>() / 4.0;
                        let cr = s.iter().map(|v| v.2).sum::<f64>() / 4.0;
                        let at = out.off_u as usize + cy as usize * p + 2 * cx as usize;
                        close(b[at], cb, "Cb");
                        close(b[at + 1], cr, "Cr");
                    }
                }
            }
            gpu.destroy_readback(rb);
            gpu.destroy_out(out);
        }
        gpu.destroy_src(src);
    }
}
