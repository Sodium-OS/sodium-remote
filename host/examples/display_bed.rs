// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! Drives remoted's display manager against a nested Hyprland, one step per run
//! (the held session is read back from REMOTED_RUNTIME_DIR in between), for
//! host/tests/display-nested/run.sh:
//!
//!   display_bed SIGNATURE connect W H [mirror|privacy|off]
//!   display_bed SIGNATURE resize W H
//!   display_bed SIGNATURE disconnect
//!
//! W×H are the stream's pixels; the full-screen size is the same, at the 13" Air's UI ratio.

use anyhow::{Context, Result, bail};
use remoted::display::{DisplayManager, HomeMode, Request, UiSize, hypr::Hypr};

fn request(w: &str, h: &str) -> Result<Request> {
    let (width, height) = (w.parse()?, h.parse()?);
    Ok(Request {
        width,
        height,
        refresh_mhz: 60_000,
        ui: UiSize::Match,
        ui_ratio: 2.0,
        display_width: width,
        display_height: height,
    })
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sig = args.first().context("no signature")?;
    if std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok_and(|s| &s == sig) {
        bail!("refusing the live compositor");
    }
    let mut dm = DisplayManager::new(Hypr::with_signature(sig));
    let held = dm.adopt_leftover()?;
    match args.get(1).map(String::as_str) {
        Some("connect") => {
            let mode = args.get(4).map_or(Some(HomeMode::Mirror), |m| HomeMode::parse(m)).context("bad mode")?;
            let a = dm.connect(&request(&args[2], &args[3])?, mode, "bed")?;
            println!("{}x{} at {}", a.width, a.height, a.scale);
        }
        Some("resize") => {
            held.context("not connected")?;
            let a = dm.resize(&request(&args[2], &args[3])?)?;
            println!("{}x{} at {}", a.width, a.height, a.scale);
        }
        Some("disconnect") => {
            held.context("not connected")?;
            dm.disconnect()?;
        }
        _ => bail!("connect W H [mode] | resize W H | disconnect"),
    }
    Ok(())
}
