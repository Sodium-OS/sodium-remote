// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! `remoted run`: the daemon. One process: the session actor (display, IME,
//! audio, clipboard, notifications, the D-Bus API), remote-core's
//! `connect::Host` (pairing, the rendezvous with `device.key`, mDNS, ICE and the
//! protocol) and the media pipeline, joined by `stream`.

use std::{net::IpAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use remote_core::{
    connect::{Host as CoreHost, HostConfig},
    keys::FileStore,
    rendezvous::RendezvousConfig,
    server_config::ServerConfig,
};
use tokio::sync::mpsc;

use crate::{
    audio, capture, dbus, display, ime, input, keys,
    media::MediaConfig,
    session,
    stream::{ChromaPref, CoreRendezvous, Stream},
};

pub struct RunOpts {
    pub display: bool,
    pub audio: bool,
    pub clipboard: bool,
    pub ime: bool,
    pub dev: bool,
    /// Register with the rendezvous server that `server.toml` names (off, or no
    /// server configured: LAN only).
    pub rendezvous: bool,
    /// Tests: this rendezvous instead of `server.toml`'s, with this CA pinned
    /// (none: Web PKI).
    pub rendezvous_url: Option<String>,
    pub rendezvous_ca: Option<PathBuf>,
    /// The LAN listener and its mDNS announcement.
    pub lan: bool,
    pub chroma: ChromaPref,
    /// The plugin's capture socket (tests: a nested Hyprland's).
    pub socket: Option<PathBuf>,
    /// Tests: a directory for host-frames.csv and host-input.csv.
    pub log_dir: Option<PathBuf>,
    /// Tests: relayed candidates only.
    pub relay_only: bool,
    /// Gather host candidates on these addresses only (default: every IPv4 interface).
    pub bind: Option<Vec<IpAddr>>,
}

impl Default for RunOpts {
    fn default() -> Self {
        RunOpts {
            display: true,
            audio: true,
            clipboard: true,
            ime: true,
            dev: false,
            rendezvous: true,
            rendezvous_url: None,
            rendezvous_ca: None,
            lan: true,
            chroma: ChromaPref::Auto,
            socket: None,
            log_dir: None,
            relay_only: false,
            bind: None,
        }
    }
}

pub const RUN_USAGE: &str = "\
remoted run [options]       the daemon (docs/reference/host-session.md)

  --no-display              don't touch Hyprland's outputs (no REMOTE-1)
  --no-audio                no Remote sink or microphone
  --no-clipboard            no clipboard sync
  --no-ime                  don't drive the fcitx5 add-on
  --no-rendezvous           LAN only (ignore server.toml)
  --rendezvous URL          this rendezvous instead of server.toml's (tests)
  --rendezvous-ca FILE      pin this CA for it (default: Web PKI)
  --no-lan                  no LAN listener and no mDNS
  --chroma auto|420|444     HEVC chroma (auto: 4:4:4 when the client decodes it)
  --socket PATH             the plugin's capture socket
  --log-dir DIR             every frame sent and input event injected, as CSV (tests)
  --relay-only              relayed ICE candidates only (tests)
  --bind IP                 gather candidates on this address only (repeatable)
  --dev                     also serve org.sodiumos.Remote.Dev (tests: a stand-in client)
";

pub fn parse_run(args: &[String]) -> Result<RunOpts> {
    let mut o = RunOpts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--no-display" => o.display = false,
            "--no-audio" => o.audio = false,
            "--no-clipboard" => o.clipboard = false,
            "--no-ime" => o.ime = false,
            "--no-rendezvous" => o.rendezvous = false,
            "--rendezvous" => o.rendezvous_url = Some(val()?),
            "--rendezvous-ca" => o.rendezvous_ca = Some(PathBuf::from(val()?)),
            "--no-lan" => o.lan = false,
            "--chroma" => {
                o.chroma = match val()?.as_str() {
                    "auto" => ChromaPref::Auto,
                    "420" => ChromaPref::C420,
                    "444" => ChromaPref::C444,
                    v => bail!("--chroma auto|420|444, not {v}"),
                }
            }
            "--socket" => o.socket = Some(val()?.into()),
            "--log-dir" => o.log_dir = Some(val()?.into()),
            "--relay-only" => o.relay_only = true,
            "--bind" => o.bind.get_or_insert_with(Vec::new).push(val()?.parse().context("--bind IP")?),
            "--dev" => o.dev = true,
            _ => bail!("unknown option {a}\n\n{RUN_USAGE}"),
        }
    }
    Ok(o)
}

pub fn run(o: RunOpts) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(daemon(o))
}

async fn daemon(o: RunOpts) -> Result<()> {
    let dir = keys::data_dir();
    let key = keys::DeviceKey::load_or_create(&dir)?;
    let settings = session::store::Settings::load(&dir);
    {
        let id: remote_core::keys::DeviceId = key.id().parse().map_err(anyhow::Error::msg)?;
        eprintln!("remoted: device {} (code {}, key in {})", key.id(), id.code(), dir.display());
    }

    let bus = zbus::Connection::session().await.context("connecting to the session bus")?;
    let system = match zbus::Connection::system().await {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("remoted: no system bus ({e}): no sleep notice, lock state unknown");
            None
        }
    };

    let logind = dbus::Logind::new(system.clone());

    // The display: put back whatever a crashed remoted left moved; a Privacy or Off
    // leftover under the session lock.
    let display = if o.display {
        let mut dm = display::DisplayManager::new(display::hypr::Hypr::from_env()?);
        if let Some(device) = dm.adopt_leftover()? {
            eprintln!("remoted: a previous run left {device}'s session on REMOTE-1; putting the desktop back");
            if dm.held().is_some_and(|h| matches!(h.mode, display::HomeMode::Privacy | display::HomeMode::Off)) {
                session::lock_desk_first(logind.as_ref(), &std::sync::atomic::AtomicBool::new(false)).await;
            }
            // As at a session's end: what couldn't be put back is logged; remoted serves anyway.
            if let Err(e) = dm.disconnect() {
                eprintln!("remoted: display: {e:#}");
            }
        }
        Some(dm)
    } else {
        None
    };

    let audio = if o.audio {
        match audio::Audio::open() {
            Ok(a) => {
                match audio::recover(&a) {
                    Ok(0) => {}
                    Ok(n) => eprintln!("remoted: put back {n} default device(s) a previous run had changed"),
                    Err(e) => eprintln!("remoted: audio recovery: {e:#}"),
                }
                Some(a)
            }
            Err(e) => {
                eprintln!("remoted: no audio ({e:#})");
                None
            }
        }
    } else {
        None
    };

    let (ime_tx, mut ime_rx) = mpsc::unbounded_channel();
    let ime = if o.ime { Some(ime::ImeBridge::new(&bus, ime_tx).await?) } else { None };

    // remote-core: pairing, the rendezvous, mDNS and every connection.
    remote_core::connect::load_named_networks();
    let mut cfg = HostConfig::new(Arc::new(FileStore::new(dir.clone())), settings.display_name());
    cfg.rendezvous = match (&o.rendezvous_url, o.rendezvous) {
        (_, false) => None,
        (Some(u), true) => {
            let mut r = RendezvousConfig::new(u.clone());
            if let Some(ca) = &o.rendezvous_ca {
                r.ca_pem = Some(std::fs::read(ca).with_context(|| format!("reading {}", ca.display()))?);
            }
            Some(r)
        }
        (None, true) => ServerConfig::load_rendezvous(),
    };
    if cfg.rendezvous.is_none() && o.rendezvous {
        eprintln!("remoted: no server configured ({}): LAN and address only", remote_core::brand::SERVER_TOML);
    }
    let rdv_host = cfg.rendezvous.as_ref().map(|r| r.host());
    cfg.lan = o.lan;
    cfg.mdns = o.lan && settings.mdns;
    cfg.relay_only = o.relay_only;
    cfg.bind = o.bind.clone();
    if let Some(s) = std::env::var("REMOTED_HOLD_SECS").ok().and_then(|v| v.parse().ok()) {
        cfg.hold = Duration::from_secs(s);
    }
    let (core, core_rx) = CoreHost::start(cfg).await.context("starting remote-core's host")?;
    if let Some(p) = core.lan_port() {
        eprintln!("remoted: LAN listener on port {p}{}", if settings.mdns { ", announced over mDNS" } else { "" });
    }
    let rdv = Arc::new(CoreRendezvous::new(core.clone(), rdv_host));

    let parts = session::Parts {
        dir,
        key,
        display,
        plugin: None,
        ime,
        audio,
        clipboard: o.clipboard,
        rendezvous: rdv.clone(),
        notifier: None,
        desk: logind,
    };
    let (mut host, tx, rx, status) = session::Host::new(parts)?;
    host.set_notifier(dbus::DbusNotifier::new(&bus, tx.clone()).await?);

    let focus = match display::hypr::Hypr::from_env() {
        Ok(h) => input::FocusWatch::start(h),
        Err(_) => input::FocusWatch::default(),
    };
    let base = MediaConfig { socket: o.socket.clone().unwrap_or_else(capture::plugin::default_socket), ..MediaConfig::default() };
    let stream = Stream::new(core.clone(), tx.clone(), base, o.chroma, focus, o.log_dir.clone())?;
    host.set_plugin(stream.clone());
    tokio::spawn(stream.clone().run_events(core_rx, rdv));

    {
        let tx = tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = ime_rx.recv().await {
                let _ = tx.send(session::Cmd::Ime(ev));
            }
        });
    }
    dbus::serve(&bus, tx.clone(), status).await?;
    if o.dev {
        let links: session::Links = Default::default();
        bus.object_server().at(dbus::DEV_PATH, dbus::DevApi { tx: tx.clone(), links }).await?;
    }
    if let Some(sys) = system {
        if let Err(e) = dbus::watch_hostname(sys.clone(), tx.clone()).await {
            eprintln!("remoted: not following the computer's name ({e})");
        }
        if let Err(e) = dbus::watch_sleep(sys, tx.clone()).await {
            eprintln!("remoted: no sleep notice ({e})");
        }
    }
    {
        let tx = tx.clone();
        let stopping = host.stopping();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let (mut term, mut int) = (signal(SignalKind::terminate()).unwrap(), signal(SignalKind::interrupt()).unwrap());
            tokio::select! { _ = term.recv() => {}, _ = int.recv() => {} }
            // A wait for the lock that is running now gets the stopping cap too.
            stopping.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = tx.send(session::Cmd::Shutdown);
        });
    }
    eprintln!("remoted: serving {} on the session bus", dbus::NAME);
    host.run(rx).await;
    core.shutdown().await;
    eprintln!("remoted: stopped");
    Ok(())
}
