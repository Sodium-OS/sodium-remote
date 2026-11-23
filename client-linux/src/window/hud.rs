// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! The stats HUD's text, rasterised on the CPU into a premultiplied RGBA image.
//! PROVISIONAL: a plain monospace panel for the reference client; the Mac's HUD
//! follows the Mac app's.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};

pub struct Hud {
    font: FontVec,
    scale: PxScale,
}

impl Hud {
    /// The system's monospace font (fontconfig's choice) at `px` pixels.
    pub fn load(px: f32) -> Option<Hud> {
        let out = std::process::Command::new("fc-match").args(["-f", "%{file}", "monospace"]).output().ok()?;
        let path = String::from_utf8(out.stdout).ok()?;
        let font = FontVec::try_from_vec(std::fs::read(path.trim()).ok()?).ok()?;
        Some(Hud { font, scale: PxScale::from(px) })
    }

    /// (width, height, RGBA premultiplied) for these lines on a dark panel.
    pub fn render(&self, lines: &[String]) -> (u32, u32, Vec<u8>) {
        let f = self.font.as_scaled(self.scale);
        let pad = (self.scale.y * 0.5).ceil();
        let line_h = (f.ascent() - f.descent() + f.line_gap()).ceil();
        let width = lines
            .iter()
            .map(|l| l.chars().map(|c| f.h_advance(f.glyph_id(c))).sum::<f32>())
            .fold(0.0f32, f32::max);
        let (w, h) = ((width + 2.0 * pad).ceil() as u32, (line_h * lines.len() as f32 + 2.0 * pad).ceil() as u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for p in px.chunks_exact_mut(4) {
            p[3] = 166; // black at 65%
        }
        for (n, line) in lines.iter().enumerate() {
            let mut x = pad;
            let base = pad + f.ascent() + n as f32 * line_h;
            for c in line.chars() {
                let id = f.glyph_id(c);
                let g = id.with_scale_and_position(self.scale, point(x, base));
                x += f.h_advance(id);
                let Some(o) = self.font.outline_glyph(g) else { continue };
                let b = o.px_bounds();
                o.draw(|gx, gy, cov| {
                    let (px_x, px_y) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                    if px_x < 0 || px_y < 0 || px_x >= w as i32 || px_y >= h as i32 {
                        return;
                    }
                    let i = ((px_y as u32 * w + px_x as u32) * 4) as usize;
                    let c = cov.clamp(0.0, 1.0);
                    let v = (255.0 * c) as u8;
                    px[i] = px[i].max(v);
                    px[i + 1] = px[i + 1].max(v);
                    px[i + 2] = px[i + 2].max(v);
                    px[i + 3] = (255.0 * c + 166.0 * (1.0 - c)).max(px[i + 3] as f32) as u8;
                });
            }
        }
        (w, h, px)
    }
}
