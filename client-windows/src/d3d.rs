// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Direct3D 11 presenting: a flip-model swapchain with a waitable object, the
//! Y′CbCr → RGB pixel shader (HLSL compiled at run time with `D3DCompile` from
//! `d3dcompiler_47.dll`, which ships with Windows), NV12 and 3-plane uploads,
//! and alpha-blended overlays (HUD, notices). Hardware first, WARP when there
//! is no GPU (the VM).

use anyhow::{Context, Result, anyhow, bail};
use remote_engine::decode::{Picture, PixFmt};
use std::ffi::{CString, c_void};
use windows::Win32::Foundation::{HANDLE, HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_11_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::core::{Interface, PCSTR, PCWSTR};

const HLSL: &str = r#"
cbuffer C : register(b0) { float4 rect; float4 p; };
struct VO { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
VO vs(uint id : SV_VertexID) {
    float2 q = float2(id & 1, id >> 1);
    VO o;
    o.pos = float4(lerp(rect.x, rect.z, q.x), lerp(rect.y, rect.w, q.y), 0, 1);
    o.uv = q;
    return o;
}
Texture2D ty : register(t0);
Texture2D tu : register(t1);
Texture2D tv : register(t2);
SamplerState sm : register(s0);
float4 ps_video(VO i) : SV_Target {
    float y = ty.Sample(sm, i.uv).r;
    float2 c = p.x < 0.5 ? tu.Sample(sm, i.uv).rg : float2(tu.Sample(sm, i.uv).r, tv.Sample(sm, i.uv).r);
    float cb, cr;
    if (p.z > 0.5) { cb = c.x - 128.0 / 255.0; cr = c.y - 128.0 / 255.0; }
    else { y = (y * 255.0 - 16.0) / 219.0; cb = (c.x * 255.0 - 128.0) / 224.0; cr = (c.y * 255.0 - 128.0) / 224.0; }
    float3 rgb;
    if (p.y > 0.5) rgb = float3(y + 1.402 * cr, y - 0.344136 * cb - 0.714136 * cr, y + 1.772 * cb);
    else rgb = float3(y + 1.5748 * cr, y - 0.1873 * cb - 0.4681 * cr, y + 1.8556 * cb);
    return float4(saturate(rgb), 1.0);
}
float4 ps_overlay(VO i) : SV_Target { return ty.Sample(sm, i.uv); }
"#;

type D3DCompileFn = unsafe extern "system" fn(
    *const c_void,
    usize,
    PCSTR,
    *const c_void,
    *mut c_void,
    PCSTR,
    PCSTR,
    u32,
    u32,
    *mut Option<ID3DBlob>,
    *mut Option<ID3DBlob>,
) -> windows::core::HRESULT;

fn compile(src: &str, entry: &str, target: &str) -> Result<ID3DBlob> {
    unsafe {
        // Windows' own copy by full path (System32), never found by name in the folder the program was unpacked into.
        let mut dir = [0u16; 260];
        let n = GetSystemDirectoryW(Some(&mut dir)) as usize;
        if n == 0 || n >= dir.len() {
            bail!("no system folder for d3dcompiler_47.dll");
        }
        let path: Vec<u16> = String::from_utf16_lossy(&dir[..n]).encode_utf16().chain("\\d3dcompiler_47.dll\0".encode_utf16()).collect();
        let lib = LoadLibraryExW(PCWSTR(path.as_ptr()), None, LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32).context("loading d3dcompiler_47.dll")?;
        let f = GetProcAddress(lib, windows::core::s!("D3DCompile")).context("D3DCompile not found")?;
        let f: D3DCompileFn = std::mem::transmute(f);
        let (e, t) = (CString::new(entry)?, CString::new(target)?);
        let (mut code, mut err) = (None, None);
        let hr = f(
            src.as_ptr().cast(),
            src.len(),
            PCSTR::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            PCSTR(e.as_ptr().cast()),
            PCSTR(t.as_ptr().cast()),
            0,
            0,
            &mut code,
            &mut err,
        );
        if hr.is_err() {
            let msg = err.map(|b| String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize())).into_owned());
            bail!("compiling {entry}: {hr:?} {}", msg.unwrap_or_default());
        }
        code.context("no shader code")
    }
}

fn blob_bytes(b: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()) }
}

/// A BGRA premultiplied overlay texture (HUD, notice) and its size.
pub struct Overlay {
    pub w: u32,
    pub h: u32,
    srv: ID3D11ShaderResourceView,
}

struct Planes {
    w: u32,
    h: u32,
    fmt: PixFmt,
    tex: [Option<ID3D11Texture2D>; 3],
    srv: [Option<ID3D11ShaderResourceView>; 3],
}

pub struct Gfx {
    pub device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    pub warp: bool,
    sc: IDXGISwapChain1,
    waitable: HANDLE,
    rtv: Option<ID3D11RenderTargetView>,
    size: (u32, u32),
    vs: ID3D11VertexShader,
    ps_video: ID3D11PixelShader,
    ps_overlay: ID3D11PixelShader,
    cb: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    blend: ID3D11BlendState,
    planes: Option<Planes>,
}

fn create_device(driver: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let (mut dev, mut ctx) = (None, None);
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_0]),
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            Some(&mut ctx),
        )?;
    }
    Ok((dev.context("no device")?, ctx.context("no context")?))
}

/// Does the GPU decode AV1 (VLD profile 0)? The FFmpeg here has no software AV1.
pub fn hardware_av1() -> bool {
    unsafe {
        let (mut dev, mut ctx) = (None, None);
        if D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            Some(&mut ctx),
        )
        .is_err()
        {
            return false;
        }
        let Some(dev) = dev else { return false };
        let Ok(vd) = dev.cast::<ID3D11VideoDevice>() else { return false };
        // D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0
        let av1 = windows::core::GUID::from_u128(0xb8be4ccb_cf53_46ba_8d59_d6b8a6da5d2a);
        (0..vd.GetVideoDecoderProfileCount()).any(|i| vd.GetVideoDecoderProfile(i).is_ok_and(|g| g == av1))
    }
}

impl Gfx {
    pub fn new(hwnd: HWND, w: u32, h: u32) -> Result<Gfx> {
        let (device, ctx, warp) = match create_device(D3D_DRIVER_TYPE_HARDWARE) {
            Ok((d, c)) => (d, c, false),
            Err(e) => {
                eprintln!("remote-client: no hardware D3D11 device ({e}); using WARP");
                let (d, c) = create_device(D3D_DRIVER_TYPE_WARP)?;
                (d, c, true)
            }
        };
        unsafe {
            let dxgi: IDXGIDevice = device.cast()?;
            let adapter = dxgi.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w.max(1),
                Height: h.max(1),
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
            };
            let sc = factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)?;
            let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            let sc2: IDXGISwapChain2 = sc.cast()?;
            sc2.SetMaximumFrameLatency(1)?;
            let waitable = sc2.GetFrameLatencyWaitableObject();

            let vs_b = compile(HLSL, "vs", "vs_4_0")?;
            let pv_b = compile(HLSL, "ps_video", "ps_4_0")?;
            let po_b = compile(HLSL, "ps_overlay", "ps_4_0")?;
            let mut vs = None;
            device.CreateVertexShader(blob_bytes(&vs_b), None, Some(&mut vs))?;
            let mut ps_video = None;
            device.CreatePixelShader(blob_bytes(&pv_b), None, Some(&mut ps_video))?;
            let mut ps_overlay = None;
            device.CreatePixelShader(blob_bytes(&po_b), None, Some(&mut ps_overlay))?;
            let mut cb = None;
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: 32,
                    Usage: D3D11_USAGE_DYNAMIC,
                    BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                    CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut cb),
            )?;
            let mut sampler = None;
            device.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                },
                Some(&mut sampler),
            )?;
            let mut rt = D3D11_RENDER_TARGET_BLEND_DESC::default();
            rt.BlendEnable = true.into();
            rt.SrcBlend = D3D11_BLEND_ONE;
            rt.DestBlend = D3D11_BLEND_INV_SRC_ALPHA;
            rt.BlendOp = D3D11_BLEND_OP_ADD;
            rt.SrcBlendAlpha = D3D11_BLEND_ONE;
            rt.DestBlendAlpha = D3D11_BLEND_INV_SRC_ALPHA;
            rt.BlendOpAlpha = D3D11_BLEND_OP_ADD;
            rt.RenderTargetWriteMask = D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8;
            let mut blend = None;
            device.CreateBlendState(&D3D11_BLEND_DESC { RenderTarget: [rt; 8], ..Default::default() }, Some(&mut blend))?;

            let mut g = Gfx {
                device,
                ctx,
                warp,
                sc,
                waitable,
                rtv: None,
                size: (w.max(1), h.max(1)),
                vs: vs.context("vs")?,
                ps_video: ps_video.context("ps")?,
                ps_overlay: ps_overlay.context("ps")?,
                cb: cb.context("cb")?,
                sampler: sampler.context("sampler")?,
                blend: blend.context("blend")?,
                planes: None,
            };
            g.make_rtv()?;
            Ok(g)
        }
    }

    pub fn waitable(&self) -> HANDLE {
        self.waitable
    }

    fn make_rtv(&mut self) -> Result<()> {
        unsafe {
            let bb: ID3D11Texture2D = self.sc.GetBuffer(0)?;
            let mut rtv = None;
            self.device.CreateRenderTargetView(&bb, None, Some(&mut rtv))?;
            self.rtv = rtv;
        }
        Ok(())
    }

    pub fn resize(&mut self, w: u32, h: u32) -> Result<()> {
        let (w, h) = (w.max(1), h.max(1));
        if (w, h) == self.size {
            return Ok(());
        }
        unsafe {
            self.ctx.OMSetRenderTargets(None, None);
            self.rtv = None;
            self.ctx.Flush();
            self.sc.ResizeBuffers(0, w, h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT)?;
        }
        self.size = (w, h);
        self.make_rtv()
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The video size last uploaded.
    pub fn video_size(&self) -> Option<(u32, u32)> {
        self.planes.as_ref().map(|p| (p.w, p.h))
    }

    fn tex(&self, w: u32, h: u32, fmt: DXGI_FORMAT) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
        unsafe {
            let mut t = None;
            self.device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: fmt,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut t),
            )?;
            let t = t.context("texture")?;
            let mut s = None;
            self.device.CreateShaderResourceView(&t, None, Some(&mut s))?;
            Ok((t, s.context("srv")?))
        }
    }

    /// Uploads a decoded picture (system memory, NV12 or 3 planes).
    pub fn upload(&mut self, p: &Picture) -> Result<()> {
        if p.cuda {
            bail!("a GPU-resident picture");
        }
        let need_new = self.planes.as_ref().is_none_or(|q| (q.w, q.h, q.fmt) != (p.width, p.height, p.format));
        if need_new {
            let (w, h) = (p.width, p.height);
            let mut q = Planes { w, h, fmt: p.format, tex: [None, None, None], srv: [None, None, None] };
            let (t, s) = self.tex(w, h, DXGI_FORMAT_R8_UNORM)?;
            q.tex[0] = Some(t);
            q.srv[0] = Some(s);
            match p.format {
                PixFmt::Nv12 => {
                    let (t, s) = self.tex(w.div_ceil(2), h.div_ceil(2), DXGI_FORMAT_R8G8_UNORM)?;
                    q.tex[1] = Some(t);
                    q.srv[1] = Some(s);
                }
                PixFmt::Yuv420p | PixFmt::Yuv444p => {
                    let (cw, ch) = if p.format == PixFmt::Yuv444p { (w, h) } else { (w.div_ceil(2), h.div_ceil(2)) };
                    for i in 1..3 {
                        let (t, s) = self.tex(cw, ch, DXGI_FORMAT_R8_UNORM)?;
                        q.tex[i] = Some(t);
                        q.srv[i] = Some(s);
                    }
                }
            }
            self.planes = Some(q);
        }
        let q = self.planes.as_ref().unwrap();
        let n = if p.format == PixFmt::Nv12 { 2 } else { 3 };
        for i in 0..n {
            let data = p.plane(i);
            if data.is_empty() {
                bail!("plane {i} has no data");
            }
            unsafe {
                self.ctx.UpdateSubresource(q.tex[i].as_ref().unwrap(), 0, None, data.as_ptr().cast(), p.strides[i] as u32, 0);
            }
        }
        Ok(())
    }

    pub fn overlay(&self, w: u32, h: u32, bgra_premultiplied: &[u8]) -> Result<Overlay> {
        unsafe {
            let mut t = None;
            let init = D3D11_SUBRESOURCE_DATA { pSysMem: bgra_premultiplied.as_ptr().cast(), SysMemPitch: w * 4, SysMemSlicePitch: 0 };
            self.device.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: w,
                    Height: h,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_IMMUTABLE,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    ..Default::default()
                },
                Some(&init),
                Some(&mut t),
            )?;
            let t = t.context("texture")?;
            let mut s = None;
            self.device.CreateShaderResourceView(&t, None, Some(&mut s))?;
            Ok(Overlay { w, h, srv: s.context("srv")? })
        }
    }

    fn set_cb(&self, rect_px: (f32, f32, f32, f32), p: [f32; 4]) -> Result<()> {
        let (bw, bh) = (self.size.0 as f32, self.size.1 as f32);
        let (x0, y0, x1, y1) = rect_px;
        // Pixels → NDC; y grows downward in pixels, upward in NDC.
        let v: [f32; 8] = [x0 / bw * 2.0 - 1.0, 1.0 - y0 / bh * 2.0, x1 / bw * 2.0 - 1.0, 1.0 - y1 / bh * 2.0, p[0], p[1], p[2], p[3]];
        unsafe {
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(&self.cb, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut m))?;
            std::ptr::copy_nonoverlapping(v.as_ptr() as *const u8, m.pData as *mut u8, 32);
            self.ctx.Unmap(&self.cb, 0);
        }
        Ok(())
    }

    /// Draws the video at `video` (x, y, w, h in buffer pixels; None: no picture
    /// yet) and the overlays (rect in buffer pixels), then presents.
    /// `bt601`, `full_range` describe the picture. Returns the present call's
    /// `GetLastPresentCount` for the frame statistics.
    pub fn draw(&mut self, video: Option<(f64, f64, f64, f64)>, bt601: bool, full_range: bool, overlays: &[(&Overlay, (f32, f32, f32, f32))]) -> Result<u32> {
        let rtv = self.rtv.clone().context("no render target")?;
        unsafe {
            self.ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
            self.ctx.ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, 1.0]);
            self.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: self.size.0 as f32,
                Height: self.size.1 as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            self.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            self.ctx.VSSetShader(&self.vs, None);
            self.ctx.VSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            self.ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            self.ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.ctx.OMSetBlendState(None, None, 0xffff_ffff);
            if let (Some(v), Some(q)) = (video, self.planes.as_ref()) {
                let mode = if q.fmt == PixFmt::Nv12 { 0.0 } else { 1.0 };
                self.set_cb(
                    (v.0 as f32, v.1 as f32, (v.0 + v.2) as f32, (v.1 + v.3) as f32),
                    [mode, bt601 as u8 as f32, full_range as u8 as f32, 0.0],
                )?;
                self.ctx.PSSetShader(&self.ps_video, None);
                self.ctx.PSSetShaderResources(0, Some(&[q.srv[0].clone(), q.srv[1].clone(), q.srv[2].clone()]));
                self.ctx.Draw(4, 0);
            }
            if !overlays.is_empty() {
                self.ctx.OMSetBlendState(&self.blend, None, 0xffff_ffff);
                self.ctx.PSSetShader(&self.ps_overlay, None);
                for (o, r) in overlays {
                    self.set_cb((r.0, r.1, r.0 + r.2, r.1 + r.3), [0.0; 4])?;
                    self.ctx.PSSetShaderResources(0, Some(&[Some(o.srv.clone())]));
                    self.ctx.Draw(4, 0);
                }
            }
            self.ctx.PSSetShaderResources(0, Some(&[None, None, None]));
            let hr = self.sc.Present(1, DXGI_PRESENT(0));
            if hr.is_err() && hr.0 != 0x087A0001u32 as i32 {
                return Err(anyhow!("Present: {hr:?}"));
            }
            Ok(self.sc.GetLastPresentCount().unwrap_or(0))
        }
    }

    /// DXGI's frame statistics: (the present count last shown, its QPC time).
    pub fn frame_stats(&self) -> Option<(u32, i64)> {
        let mut s = DXGI_FRAME_STATISTICS::default();
        unsafe { self.sc.GetFrameStatistics(&mut s).ok()? };
        (s.SyncQPCTime != 0).then_some((s.PresentCount, s.SyncQPCTime))
    }
}
