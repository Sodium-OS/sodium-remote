// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! sodium-remote-host: Sodium Remote's host engine for Windows, macOS and generic Linux
//! (GNOME, KDE Plasma through the desktop portal), run per user in the graphical
//! session. `docs/reference/host-desktop.md`, `docs/reference/host-mac.md`,
//! `docs/reference/host-linux.md`. In Remote.app the Mac host runs in-process instead (`ffi`).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use host_desktop::backend::Backend;
use host_desktop::session::{self, DEFAULT_PORT, Options};

const USAGE: &str = "\
sodium-remote-host [options]

  --data DIR        device key, pairings, host.txt and pairing-code.txt (and on Linux the
                    portal's restore token). Default on Windows: %LOCALAPPDATA%\\SodiumRemote\\host;
                    on Linux (portal): ~/.local/share/sodium-remote/host; required on the Mac
                    and with the fake backend
  --name NAME       the name devices show (default: this computer's name)
  --port N          the LAN listener's TCP port (default 47311; 0: any free port)
  --pair            open a pairing window now, and a new one whenever it expires
                    (one opens anyway while nothing is paired)
  --no-mdns         don't announce this computer over mDNS
  --rendezvous      also register with the server server.toml names (enrolment: its token)
  --bind IP         gather ICE candidates on this address only (repeatable)
  --secs N          stop after N seconds (tests)

  --backend NAME    windows (the default on Windows), mac (the default on macOS), portal
                    (the default on Linux: GNOME, KDE Plasma), or fake (synthetic frames, tests)
  --encoder NAME    use only this encoder (hevc_nvenc, hevc_amf, hevc_qsv, h264_mf, openh264;
                    on Linux also hevc_vaapi, h264_vaapi, h264_nvenc, libx264, libopenh264;
                    on the Mac videotoolbox_hevc or videotoolbox_h264)
  --forget-consent  Linux: forget the sharing permission (the restore token), then quit
  --fake-size WxH   the fake display's size (default 1920x1080)
  --input-log FILE  fake backend: every injected input event, as CSV
  --list            list the displays and the working encoders, then quit
  --permissions     macOS: print Screen Recording and Accessibility, then quit
  --ask-permissions macOS: show macOS's prompts for the ones missing, then quit
";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")))
        .with_writer(std::io::stderr)
        .init();
    // On the Mac the main thread runs AppKit's run loop (NSScreen, NSCursor and
    // NSWorkspace are read there), so the host runs on another thread.
    #[cfg(target_os = "macos")]
    {
        std::thread::Builder::new()
            .name("host".into())
            .spawn(|| std::process::exit(finish(run())))
            .expect("the host thread");
        host_desktop::mac::run_main_loop();
    }
    #[cfg(not(target_os = "macos"))]
    std::process::exit(finish(run()));
}

fn finish(r: Result<()>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("sodium-remote-host: {e:#}");
            1
        }
    }
}

fn run() -> Result<()> {
    let mut o = Options { data: PathBuf::new(), store: None, name: None, port: DEFAULT_PORT, pair: false, mdns: true, rendezvous: false, server: None, bind: None, secs: None };
    let mut backend_name = if cfg!(windows) {
        "windows".to_string()
    } else if cfg!(target_os = "macos") {
        "mac".to_string()
    } else if cfg!(all(target_os = "linux", feature = "linux-portal")) {
        "portal".to_string()
    } else {
        "fake".to_string()
    };
    let mut encoder = None;
    let mut fake_size = (1920, 1080);
    let mut input_log = None;
    let mut list = false;
    let mut permissions = None;
    let mut forget = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--data" => o.data = val()?.into(),
            "--name" => o.name = Some(val()?),
            "--port" => o.port = val()?.parse().context("--port N")?,
            "--pair" => o.pair = true,
            "--no-mdns" => o.mdns = false,
            "--rendezvous" => o.rendezvous = true,
            "--bind" => o.bind.get_or_insert_with(Vec::new).push(val()?.parse().context("--bind IP")?),
            "--secs" => o.secs = Some(val()?.parse().context("--secs N")?),
            "--backend" => backend_name = val()?,
            "--encoder" => encoder = Some(val()?),
            "--fake-size" => {
                let v: Vec<u32> = val()?.split('x').map(|p| p.parse().unwrap_or(0)).collect();
                if v.len() != 2 || v[0] < 2 || v[1] < 2 {
                    bail!("--fake-size WxH");
                }
                fake_size = (v[0], v[1]);
            }
            "--input-log" => input_log = Some(PathBuf::from(val()?)),
            "--list" => list = true,
            "--permissions" => permissions = Some(false),
            "--ask-permissions" => permissions = Some(true),
            "--forget-consent" => forget = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            _ => bail!("unknown option {a}\n\n{USAGE}"),
        }
    }
    if let Some(ask) = permissions {
        return show_permissions(ask);
    }
    if o.data.as_os_str().is_empty() {
        #[cfg(all(target_os = "linux", feature = "linux-portal"))]
        if backend_name == "portal" {
            // the host half, never migrated from remoted's pre-rename directory: that
            // identity belongs to the SodiumOS host.
            o.data = host_desktop::linux::default_data_dir()?;
        }
        if o.data.as_os_str().is_empty() {
            if !cfg!(windows) {
                // Never the SodiumOS host's (remoted's) own data directory.
                bail!("--data DIR is required here\n\n{USAGE}");
            }
            o.data = remote_core::keys::FileStore::host_default()?.dir;
        }
    }
    let backend: Arc<dyn Backend> = match backend_name.as_str() {
        "fake" => Arc::new(host_desktop::fake::FakeBackend::new(encoder.as_deref(), fake_size, input_log)),
        #[cfg(windows)]
        "windows" => Arc::new(host_desktop::win::WinBackend::new(encoder.as_deref())?),
        #[cfg(target_os = "macos")]
        "mac" => Arc::new(host_desktop::mac::MacBackend::new(encoder.as_deref())?),
        #[cfg(all(target_os = "linux", feature = "linux-portal"))]
        "portal" => {
            let b = if list || forget {
                host_desktop::linux::PortalBackend::new_unlocked(o.data.clone(), encoder.as_deref())?
            } else {
                host_desktop::linux::PortalBackend::new(o.data.clone(), encoder.as_deref())?
            };
            if list {
                // The display is chosen in the system's dialog when a device connects.
                println!("display: chosen in the desktop's sharing dialog when a device connects");
                println!("consent: {:?}", b.consent().unwrap_or(host_desktop::backend::Consent::Ask));
                println!("encoders (best first): {}", b.working_encoders().join(", "));
                let caps = b.encoder_caps();
                println!("encoders: hardware HEVC {}, H.264 {}", caps.hevc, caps.h264);
                return Ok(());
            }
            if forget {
                b.forget_consent();
                println!("the sharing permission is forgotten: the next connection shows the dialog");
                return Ok(());
            }
            Arc::new(b)
        }
        b => bail!("no backend {b:?} in this build"),
    };
    let _ = forget;
    if list {
        for d in backend.displays() {
            println!("display {}", session::describe(&d));
        }
        let caps = backend.encoder_caps();
        println!("encoders: hardware HEVC {}, H.264 {}", caps.hevc, caps.h264);
        return Ok(());
    }
    session::run(backend, o)
}

#[cfg(target_os = "macos")]
fn show_permissions(ask: bool) -> Result<()> {
    use host_desktop::mac::permissions as p;
    if ask {
        if !p::screen_recording() {
            p::request_screen_recording();
        }
        if !p::accessibility() {
            p::request_accessibility();
        }
    }
    println!("screen recording: {}", if p::screen_recording() { "allowed" } else { "not allowed" });
    println!("accessibility: {}", if p::accessibility() { "allowed" } else { "not allowed" });
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn show_permissions(_ask: bool) -> Result<()> {
    bail!("--permissions is for macOS")
}
