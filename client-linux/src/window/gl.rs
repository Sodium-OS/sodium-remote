// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! GLES 3 drawing: the video (Y′CbCr planes → RGB in the shader), the cursor
//! overlay and the HUD, all as textured quads.

use crate::decode::{GlInterop, Picture, PixFmt};
use glow::HasContext;

const VS: &str = r#"#version 300 es
uniform vec4 rect; // x0, y0, x1, y1 in NDC
out vec2 uv;
void main() {
    vec2 p = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1));
    uv = vec2(p.x, 1.0 - p.y);
    gl_Position = vec4(mix(rect.xy, rect.zw, p), 0.0, 1.0);
}
"#;

// Y′CbCr as NVENC writes it (BT.709, full range, sRGB-encoded values: no
// linearisation), or whatever the VUI says.
const FS_VIDEO: &str = r#"#version 300 es
precision highp float;
in vec2 uv;
out vec4 o;
uniform sampler2D ty;
uniform sampler2D tu;
uniform sampler2D tv;
uniform int nv12;
uniform int full;
uniform vec4 m; // R from Cr, G from Cb, G from Cr, B from Cb
void main() {
    float y = texture(ty, uv).r;
    vec2 c = nv12 == 1 ? texture(tu, uv).rg : vec2(texture(tu, uv).r, texture(tv, uv).r);
    if (full == 1) {
        c -= 128.0 / 255.0;
    } else {
        y = (y - 16.0 / 255.0) * (255.0 / 219.0);
        c = (c - 128.0 / 255.0) * (255.0 / 224.0);
    }
    o = vec4(y + m.x * c.y, y - m.y * c.x - m.z * c.y, y + m.w * c.x, 1.0);
}
"#;

// Premultiplied RGBA; `bgra` for ARGB8888 bytes (B, G, R, A in memory).
const FS_RGBA: &str = r#"#version 300 es
precision mediump float;
in vec2 uv;
out vec4 o;
uniform sampler2D t;
uniform int bgra;
void main() {
    vec4 c = texture(t, uv);
    o = bgra == 1 ? c.bgra : c;
}
"#;

pub struct Rgba {
    pub tex: glow::Texture,
    pub w: u32,
    pub h: u32,
    pub bgra: bool,
}

pub struct Renderer {
    pub gl: glow::Context,
    video: glow::Program,
    rgba: glow::Program,
    vao: glow::VertexArray,
    planes: [glow::Texture; 3],
    pub video_size: Option<(u32, u32)>,
    fmt: PixFmt,
    full: bool,
    bt601: bool,
    cuda: Option<GlInterop>,
    cuda_alloc: bool,
}

unsafe fn program(gl: &glow::Context, fs: &str) -> Result<glow::Program, String> {
    unsafe {
        let p = gl.create_program()?;
        for (kind, src) in [(glow::VERTEX_SHADER, VS), (glow::FRAGMENT_SHADER, fs)] {
            let s = gl.create_shader(kind)?;
            gl.shader_source(s, src);
            gl.compile_shader(s);
            if !gl.get_shader_compile_status(s) {
                return Err(gl.get_shader_info_log(s));
            }
            gl.attach_shader(p, s);
        }
        gl.link_program(p);
        if !gl.get_program_link_status(p) {
            return Err(gl.get_program_info_log(p));
        }
        Ok(p)
    }
}

unsafe fn new_texture(gl: &glow::Context, nearest: bool) -> Result<glow::Texture, String> {
    unsafe {
        let t = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(t));
        let f = if nearest { glow::NEAREST } else { glow::LINEAR } as i32;
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, f);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, f);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        Ok(t)
    }
}

impl Renderer {
    pub fn new(gl: glow::Context) -> Result<Renderer, String> {
        unsafe {
            let video = program(&gl, FS_VIDEO)?;
            let rgba = program(&gl, FS_RGBA)?;
            let vao = gl.create_vertex_array()?;
            let planes = [new_texture(&gl, false)?, new_texture(&gl, false)?, new_texture(&gl, false)?];
            gl.use_program(Some(video));
            for (i, n) in ["ty", "tu", "tv"].iter().enumerate() {
                gl.uniform_1_i32(gl.get_uniform_location(video, n).as_ref(), i as i32);
            }
            gl.use_program(Some(rgba));
            gl.uniform_1_i32(gl.get_uniform_location(rgba, "t").as_ref(), 0);
            Ok(Renderer { gl, video, rgba, vao, planes, video_size: None, fmt: PixFmt::Nv12, full: true, bt601: false, cuda: None, cuda_alloc: false })
        }
    }

    /// Copies a CUDA picture into the plane textures (CUDA → GL, no host copy).
    fn upload_cuda(&mut self, p: &Picture) -> Result<(), String> {
        let (w, h) = (p.width as i32, p.height as i32);
        let (cw, ch) = match p.format {
            PixFmt::Yuv444p => (w, h),
            _ => ((w + 1) / 2, (h + 1) / 2),
        };
        if self.cuda.as_ref().is_some_and(|c| !c.same_context(p)) {
            self.cuda = None;
        }
        if self.cuda.is_none() {
            self.cuda = Some(GlInterop::open(p).map_err(|e| format!("{e:#}"))?);
            self.cuda_alloc = false;
        }
        let realloc = !self.cuda_alloc || self.video_size != Some((p.width, p.height)) || self.fmt != p.format;
        if realloc {
            let gl = &self.gl;
            let n = if p.format == PixFmt::Nv12 { 2 } else { 3 };
            for i in 0..n {
                let (pw, ph, ifmt, fmt) = match (p.format, i) {
                    (_, 0) => (w, h, glow::R8, glow::RED),
                    (PixFmt::Nv12, _) => (cw, ch, glow::RG8, glow::RG),
                    _ => (cw, ch, glow::R8, glow::RED),
                };
                unsafe {
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.planes[i]));
                    gl.tex_image_2d(glow::TEXTURE_2D, 0, ifmt as i32, pw, ph, 0, fmt, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
                }
                self.cuda.as_mut().unwrap().register(i, self.planes[i].0.get()).map_err(|e| format!("{e:#}"))?;
            }
            self.cuda_alloc = true;
        }
        self.cuda.as_mut().unwrap().copy(p).map_err(|e| format!("{e:#}"))?;
        self.video_size = Some((p.width, p.height));
        self.fmt = p.format;
        self.full = p.full_range;
        self.bt601 = p.bt601;
        Ok(())
    }

    /// Uploads a decoded picture into the plane textures.
    pub fn upload(&mut self, p: &Picture) -> Result<(), String> {
        if p.cuda {
            return self.upload_cuda(p);
        }
        // Back from CUDA to system-memory pictures: the textures get plain storage again.
        let was_cuda = std::mem::take(&mut self.cuda_alloc);
        let gl = &self.gl;
        let (w, h) = (p.width as i32, p.height as i32);
        let (cw, ch) = match p.format {
            PixFmt::Yuv444p => (w, h),
            _ => ((w + 1) / 2, (h + 1) / 2),
        };
        let realloc = was_cuda || self.video_size != Some((p.width, p.height)) || self.fmt != p.format;
        unsafe {
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            let n = if p.format == PixFmt::Nv12 { 2 } else { 3 };
            for i in 0..n {
                let (pw, ph, ifmt, fmt, bpp) = match (p.format, i) {
                    (_, 0) => (w, h, glow::R8, glow::RED, 1),
                    (PixFmt::Nv12, _) => (cw, ch, glow::RG8, glow::RG, 2),
                    _ => (cw, ch, glow::R8, glow::RED, 1),
                };
                gl.active_texture(glow::TEXTURE0 + i as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.planes[i]));
                gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, (p.strides[i] / bpp) as i32);
                let data = glow::PixelUnpackData::Slice(Some(p.plane(i)));
                if realloc {
                    gl.tex_image_2d(glow::TEXTURE_2D, 0, ifmt as i32, pw, ph, 0, fmt, glow::UNSIGNED_BYTE, data);
                } else {
                    gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, pw, ph, fmt, glow::UNSIGNED_BYTE, data);
                }
            }
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            gl.active_texture(glow::TEXTURE0);
        }
        self.video_size = Some((p.width, p.height));
        self.fmt = p.format;
        self.full = p.full_range;
        self.bt601 = p.bt601;
        Ok(())
    }

    /// An RGBA (premultiplied) texture, or ARGB8888 bytes with `bgra`.
    pub fn rgba_texture(&self, w: u32, h: u32, pixels: &[u8], bgra: bool, nearest: bool) -> Option<Rgba> {
        let gl = &self.gl;
        unsafe {
            let tex = new_texture(gl, nearest).ok()?;
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                w as i32,
                h as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(pixels)),
            );
            Some(Rgba { tex, w, h, bgra })
        }
    }

    pub fn delete(&self, t: Rgba) {
        unsafe { self.gl.delete_texture(t.tex) };
    }

    fn rect(&self, prog: glow::Program, vw: f32, vh: f32, x: f32, y: f32, w: f32, h: f32) {
        // Window pixels, top-left origin → NDC.
        let x0 = x / vw * 2.0 - 1.0;
        let x1 = (x + w) / vw * 2.0 - 1.0;
        let y0 = 1.0 - (y + h) / vh * 2.0;
        let y1 = 1.0 - y / vh * 2.0;
        unsafe {
            self.gl.uniform_4_f32(self.gl.get_uniform_location(prog, "rect").as_ref(), x0, y0, x1, y1);
            self.gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
    }

    /// Draws one frame into the current framebuffer of `vw`×`vh` pixels: the
    /// video into `video` (x, y, w, h), then the overlays at their rects.
    pub fn draw(&self, vw: u32, vh: u32, video: (f32, f32, f32, f32), overlays: &[(&Rgba, (f32, f32, f32, f32))]) {
        let gl = &self.gl;
        let (fw, fh) = (vw as f32, vh as f32);
        unsafe {
            gl.viewport(0, 0, vw as i32, vh as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.bind_vertex_array(Some(self.vao));
            gl.disable(glow::BLEND);
            if self.video_size.is_some() {
                gl.use_program(Some(self.video));
                let p = self.video;
                gl.uniform_1_i32(gl.get_uniform_location(p, "nv12").as_ref(), (self.fmt == PixFmt::Nv12) as i32);
                gl.uniform_1_i32(gl.get_uniform_location(p, "full").as_ref(), self.full as i32);
                let m = if self.bt601 { [1.402, 0.344136, 0.714136, 1.772] } else { [1.5748, 0.187324, 0.468124, 1.8556] };
                gl.uniform_4_f32(gl.get_uniform_location(p, "m").as_ref(), m[0], m[1], m[2], m[3]);
                for (i, t) in self.planes.iter().enumerate() {
                    gl.active_texture(glow::TEXTURE0 + i as u32);
                    gl.bind_texture(glow::TEXTURE_2D, Some(*t));
                }
                gl.active_texture(glow::TEXTURE0);
                self.rect(p, fw, fh, video.0, video.1, video.2, video.3);
            }
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.use_program(Some(self.rgba));
            for (t, r) in overlays {
                gl.bind_texture(glow::TEXTURE_2D, Some(t.tex));
                gl.uniform_1_i32(gl.get_uniform_location(self.rgba, "bgra").as_ref(), t.bgra as i32);
                self.rect(self.rgba, fw, fh, r.0, r.1, r.2, r.3);
            }
        }
    }
}
