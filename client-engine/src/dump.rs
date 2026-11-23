// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! `--dump-frames DIR --dump-every N`: decoded pictures as PNG, converted on
//! the CPU with the same matrix and range rules as the presenters (BT.709 or
//! 601, full or limited), so an unattended run leaves something to look at.

use crate::decode::{Picture, PixFmt};
use anyhow::{Context, Result};
use std::path::Path;

/// One pixel as RGB8, with the same matrix and range rules as the presenters.
fn rgb_at(p: &Picture, x: usize, y: usize) -> [u8; 3] {
    let (kr, kb) = if p.bt601 { (0.299f32, 0.114f32) } else { (0.2126, 0.0722) };
    let kg = 1.0 - kr - kb;
    let yv = p.plane(0)[y * p.strides[0] + x] as f32;
    let (us, vs) = (p.plane(1), p.plane(2));
    let (cb, cr) = match p.format {
        PixFmt::Nv12 => {
            let i = (y / 2) * p.strides[1] + (x / 2) * 2;
            (us[i] as f32, us[i + 1] as f32)
        }
        PixFmt::Yuv420p => ((us[(y / 2) * p.strides[1] + x / 2]) as f32, (vs[(y / 2) * p.strides[2] + x / 2]) as f32),
        PixFmt::Yuv444p => (us[y * p.strides[1] + x] as f32, vs[y * p.strides[2] + x] as f32),
    };
    let (yn, cbn, crn) = if p.full_range {
        (yv / 255.0, (cb - 128.0) / 255.0, (cr - 128.0) / 255.0)
    } else {
        ((yv - 16.0) / 219.0, (cb - 128.0) / 224.0, (cr - 128.0) / 224.0)
    };
    let r = yn + 2.0 * (1.0 - kr) * crn;
    let b = yn + 2.0 * (1.0 - kb) * cbn;
    let g = (yn - kr * r - kb * b) / kg;
    [(r.clamp(0.0, 1.0) * 255.0 + 0.5) as u8, (g.clamp(0.0, 1.0) * 255.0 + 0.5) as u8, (b.clamp(0.0, 1.0) * 255.0 + 0.5) as u8]
}

/// The picture as tightly packed RGB8.
pub fn to_rgb(p: &Picture) -> Option<Vec<u8>> {
    if p.cuda {
        return None;
    }
    let (w, h) = (p.width as usize, p.height as usize);
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            out.extend_from_slice(&rgb_at(p, x, y));
        }
    }
    Some(out)
}

/// A small copy of the picture, at most `max_w` wide (four samples per output pixel), as packed RGB8 with its size.
/// Cheap enough to run every few seconds on a 4K frame. `None` for a GPU-resident picture.
pub fn thumbnail_rgb(p: &Picture, max_w: u32) -> Option<(u32, u32, Vec<u8>)> {
    if p.cuda || p.width == 0 || p.height == 0 {
        return None;
    }
    let (pw, ph) = (p.width as usize, p.height as usize);
    let tw = max_w.min(p.width).max(1) as usize;
    let th = (ph * tw / pw).max(1);
    let mut out = Vec::with_capacity(tw * th * 3);
    for ty in 0..th {
        for tx in 0..tw {
            // Four samples inside the source rectangle this output pixel covers.
            let x0 = tx * pw / tw;
            let x1 = ((tx + 1) * pw / tw).max(x0 + 1).min(pw);
            let y0 = ty * ph / th;
            let y1 = ((ty + 1) * ph / th).max(y0 + 1).min(ph);
            let mut acc = [0u32; 3];
            for (sx, sy) in [(x0, y0), (x1 - 1, y0), (x0, y1 - 1), (x1 - 1, y1 - 1)] {
                let c = rgb_at(p, sx, sy);
                for i in 0..3 {
                    acc[i] += c[i] as u32;
                }
            }
            out.extend_from_slice(&[(acc[0] / 4) as u8, (acc[1] / 4) as u8, (acc[2] / 4) as u8]);
        }
    }
    Some((tw as u32, th as u32, out))
}

/// A folder only its owner can read (0700); on Windows the profile's ACL is enough.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)
}

/// A file only its owner can read (0600).
fn create_private_file(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)
    }
    #[cfg(not(unix))]
    std::fs::File::create(path)
}

/// Writes RGB8 pixels as a PNG, through a temporary file so a reader never sees half of it.
pub fn write_rgb_png(path: &Path, w: u32, h: u32, rgb: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let tmp = path.with_extension("png.tmp");
    {
        let f = std::io::BufWriter::new(create_private_file(&tmp).with_context(|| tmp.display().to_string())?);
        let mut enc = png::Encoder::new(f, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(rgb)?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Writes `DIR/frame-<id>.png`.
pub fn write_png(dir: &Path, frame_id: u32, p: &Picture) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let rgb = to_rgb(p).context("a GPU-resident picture can't be dumped")?;
    let path = dir.join(format!("frame-{frame_id:06}.png"));
    let f = std::io::BufWriter::new(std::fs::File::create(&path).with_context(|| path.display().to_string())?);
    let mut enc = png::Encoder::new(f, p.width, p.height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    enc.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

#[cfg(all(test, unix))]
mod private_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_thumbnail_and_its_folder_are_private() {
        let dir = std::env::temp_dir().join(format!("thumbperm-{}", std::process::id())).join("thumbs");
        let p = dir.join("a.png");
        write_rgb_png(&p, 1, 1, &[1, 2, 3]).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}
