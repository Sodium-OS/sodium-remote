// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Ning Bao
//! sodium-remote-server: Sodium Remote's rendezvous server (crate `remote-rendezvous`).
//! See lib.rs and docs/reference/rendezvous-api.md.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use remote_rendezvous::hub::unix_now;
use remote_rendezvous::{Config, Enrol, Hub, NoHooks, Notice, RelayRegion, VERSION_TEXT, server, tokens};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tracing::{info, warn};

const DEFAULT_STATE_DIR: &str = "/var/lib/sodium-remote-server";

#[derive(Parser)]
#[command(name = "sodium-remote-server", version = VERSION_TEXT, about = "Sodium Remote's rendezvous server")]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    #[command(flatten)]
    serve: Serve,
}

#[derive(Subcommand)]
enum Cmd {
    /// Enrolment tokens: hosts (and, optionally, clients) register with one.
    Token {
        #[command(subcommand)]
        op: TokenOp,
    },
}

#[derive(Subcommand)]
enum TokenOp {
    /// Mint a token and print it (shown once).
    New {
        /// Registrations it allows; 0 for unlimited.
        #[arg(long, default_value_t = 1)]
        uses: u32,
        /// How long it stays valid: 30m, 24h, 7d … or never.
        #[arg(long, default_value = "7d")]
        expires: String,
        /// The account devices registered with it belong to.
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = DEFAULT_STATE_DIR)]
        state_dir: PathBuf,
    },
    /// List the tokens (IDs only; the tokens themselves aren't kept).
    List {
        #[arg(long, default_value = DEFAULT_STATE_DIR)]
        state_dir: PathBuf,
    },
    /// Delete a token by its ID (or a unique start of it).
    Revoke {
        id: String,
        #[arg(long, default_value = DEFAULT_STATE_DIR)]
        state_dir: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum EnrolArg {
    /// Any device may register as a host.
    Open,
    /// New hosts need an enrolment token (`token new`); allowlisted hosts don't.
    Token,
    /// Only hosts on --hosts-allow.
    Allowlist,
}

#[derive(Args)]
struct Serve {
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:8443")]
    listen: SocketAddr,
    /// Name devices bind their auth signatures to (the server's DNS name, as in
    /// the URL devices use).
    #[arg(long)]
    server_name: Option<String>,
    /// Another name devices reach this server by (repeat for each), such as this
    /// node's own name when a cluster redirects to it.
    #[arg(long = "server-alias")]
    server_aliases: Vec<String>,
    /// TLS certificate chain (PEM).
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<PathBuf>,
    /// TLS private key (PEM).
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<PathBuf>,
    /// Serve without TLS (local testing only).
    #[arg(long, conflicts_with = "tls_cert")]
    plain: bool,
    /// File holding coturn's static-auth-secret.
    #[arg(long)]
    turn_secret_file: Option<PathBuf>,
    /// TURN credential lifetime in seconds.
    #[arg(long, default_value_t = 600)]
    turn_ttl: u64,
    /// A STUN/TURN URL handed out with credentials (repeat for each).
    #[arg(long = "turn-url")]
    turn_urls: Vec<String>,
    /// A relay in a region, REGION=URL (repeat; several URLs per region allowed).
    /// Clients take the region that answers fastest.
    #[arg(long = "relay", value_name = "REGION=URL")]
    relays: Vec<String>,
    /// Directory for state.json and the enrolment tokens.
    #[arg(long, default_value = DEFAULT_STATE_DIR)]
    state_dir: PathBuf,
    /// Who may register as a host.
    #[arg(long, value_enum, default_value = "token")]
    enrol: EnrolArg,
    /// Allow `--enrol open` on an address that isn't loopback or private. Anyone
    /// who can reach the server could then register a host and get TURN credentials.
    #[arg(long)]
    open_enrol_on_public_address: bool,
    /// Host allowlist: one device ID per line. Re-read on SIGHUP.
    #[arg(long)]
    hosts_allow: Option<PathBuf>,
    /// Pairing session lifetime in seconds.
    #[arg(long, default_value_t = 300)]
    pairing_ttl: u64,
    /// How long a host that sent a sleep notice shows as asleep, in seconds; then offline.
    #[arg(long, default_value_t = 24 * 3600)]
    asleep_ttl: u64,
    /// Forget a client no host has paired after this many days without a connection (0: never).
    #[arg(long, default_value_t = 30)]
    client_idle_days: u64,
    /// Address of a reverse proxy to trust: from it, the client address is read from `X-Real-IP`. Repeat for several.
    #[arg(long = "trusted-proxy")]
    trusted_proxies: Vec<std::net::IpAddr>,
    /// Send new registrations to this server instead (wss://…, same domain).
    #[arg(long)]
    redirect: Option<String>,
    /// A notice for every device at authentication: its code …
    #[arg(long, requires = "notice_text")]
    notice_code: Option<String>,
    /// … its text …
    #[arg(long, requires = "notice_code")]
    notice_text: Option<String>,
    /// … and an optional https:// link.
    #[arg(long, requires = "notice_code")]
    notice_url: Option<String>,
}

/// A loopback or private-network address. The unspecified address (all interfaces) is not.
fn is_private_listen(addr: SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_private_listen(SocketAddr::new(IpAddr::V4(v4), addr.port())),
            None => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
        },
    }
}

fn relays(args: &[String]) -> anyhow::Result<Vec<RelayRegion>> {
    let mut out: Vec<RelayRegion> = Vec::new();
    for a in args {
        let Some((region, url)) = a.split_once('=').filter(|(r, u)| !r.is_empty() && !u.is_empty()) else {
            bail!("--relay wants REGION=URL, got {a:?}");
        };
        match out.iter_mut().find(|r| r.region == region) {
            Some(r) => r.urls.push(url.to_string()),
            None => out.push(RelayRegion { region: region.to_string(), urls: vec![url.to_string()] }),
        }
    }
    Ok(out)
}

fn token_cmd(op: TokenOp) -> anyhow::Result<()> {
    let dir = |d: &PathBuf| d.join("tokens");
    match op {
        TokenOp::New { uses, expires, account, state_dir } => {
            let now = unix_now();
            let expires = tokens::parse_duration(&expires)?.map(|s| now + s);
            let (token, rec) = tokens::mint(&dir(&state_dir), (uses > 0).then_some(uses), expires, account, now)?;
            println!("{token}");
            eprintln!(
                "token {}: {} use(s), {}",
                rec.id(),
                if uses == 0 { "unlimited".to_string() } else { uses.to_string() },
                match rec.expires {
                    Some(e) => format!("expires in {} h", (e - now).div_ceil(3600)),
                    None => "never expires".into(),
                }
            );
        }
        TokenOp::List { state_dir } => {
            let now = unix_now();
            for r in tokens::list(&dir(&state_dir))? {
                let state = if r.expires.is_some_and(|e| now >= e) || r.uses_left == Some(0) { "spent" } else { "valid" };
                println!(
                    "{}  {state}  uses left {}  expires {}  account {}",
                    r.id(),
                    r.uses_left.map_or("unlimited".into(), |u| u.to_string()),
                    r.expires.map_or("never".into(), |e| e.to_string()),
                    r.account.as_deref().unwrap_or("-"),
                );
            }
        }
        TokenOp::Revoke { id, state_dir } => {
            let r = tokens::revoke(&dir(&state_dir), &id)?;
            eprintln!("revoked token {}", r.id());
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "remote_rendezvous=info,sodium_remote_server=info".into()),
        )
        .without_time() // journald stamps lines
        .with_target(false)
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    if let Some(Cmd::Token { op }) = cli.cmd {
        return token_cmd(op);
    }
    let args = cli.serve;
    let Some(server_name) = args.server_name else { bail!("--server-name is required") };
    let Some(secret_file) = args.turn_secret_file else { bail!("--turn-secret-file is required") };
    let relays = relays(&args.relays)?;
    if args.turn_urls.is_empty() && relays.is_empty() {
        bail!("give at least one --turn-url or --relay");
    }
    if args.redirect.as_deref().is_some_and(|r| !r.starts_with("wss://")) {
        bail!("--redirect must be a wss:// URL");
    }
    let enrol = match args.enrol {
        EnrolArg::Open => Enrol::Open,
        EnrolArg::Token => Enrol::Token,
        EnrolArg::Allowlist => Enrol::Allowlist,
    };
    if enrol == Enrol::Open && !is_private_listen(args.listen) && !args.open_enrol_on_public_address {
        bail!(
            "--enrol open lets anyone who reaches {} register a host and use the TURN relay; \
             listen on a loopback or private address, use --enrol token, or pass --open-enrol-on-public-address",
            args.listen
        );
    }
    if enrol == Enrol::Allowlist && args.hosts_allow.is_none() {
        bail!("--enrol allowlist needs --hosts-allow FILE");
    }
    let secret = std::fs::read_to_string(&secret_file)
        .with_context(|| format!("reading {}", secret_file.display()))?
        .trim()
        .as_bytes()
        .to_vec();
    if secret.len() < 32 {
        bail!("the TURN secret is shorter than 32 characters");
    }
    let tls = match (&args.tls_cert, &args.tls_key, args.plain) {
        (Some(c), Some(k), _) => Some(server::load_tls(c, k)?),
        (None, None, true) => None,
        _ => bail!("give --tls-cert and --tls-key, or --plain for local tests"),
    };
    let notice = args.notice_code.map(|code| Notice {
        code,
        text: args.notice_text.unwrap_or_default(),
        url: args.notice_url.filter(|u| u.starts_with("https://")),
    });
    if enrol == Enrol::Open {
        warn!(listen = %args.listen, "open enrolment: any device that reaches this server can register a host");
    }
    let mut cfg = Config::new(server_name, args.state_dir, secret);
    cfg.server_aliases = args.server_aliases;
    cfg.enrol = enrol;
    cfg.hosts_allow = args.hosts_allow;
    cfg.turn_ttl = Duration::from_secs(args.turn_ttl);
    cfg.turn_urls = args.turn_urls;
    cfg.relays = relays;
    cfg.redirect = args.redirect;
    cfg.trusted_proxies = args.trusted_proxies;
    cfg.notice = notice;
    cfg.hooks = Arc::new(NoHooks);
    cfg.pairing_ttl = Duration::from_secs(args.pairing_ttl);
    cfg.asleep_ttl = Duration::from_secs(args.asleep_ttl);
    cfg.client_idle_ttl = Duration::from_secs(args.client_idle_days.saturating_mul(24 * 3600));
    let hub = Hub::new(cfg)?;

    let ticker = hub.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_secs(5));
        loop {
            every.tick().await;
            ticker.tick();
        }
    });
    let reloader = hub.clone();
    let mut hup = signal(SignalKind::hangup())?;
    tokio::spawn(async move {
        while hup.recv().await.is_some() {
            reloader.reload_allowlist();
        }
    });

    let mut term = signal(SignalKind::terminate())?;
    let shutdown = async move {
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        info!("shutting down");
    };
    let listener = TcpListener::bind(args.listen).await.with_context(|| format!("binding {}", args.listen))?;
    info!(listen = %args.listen, tls = tls.is_some(), enrol = ?enrol, "listening");
    server::serve(listener, tls, server::router(hub), shutdown).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_private_listen;

    #[test]
    fn open_enrolment_needs_a_private_listen_address() {
        for ok in ["127.0.0.1:8443", "10.1.2.3:8443", "192.168.1.5:8443", "[::1]:8443", "[fd00::1]:8443", "[::ffff:10.0.0.1]:8443"] {
            assert!(is_private_listen(ok.parse().unwrap()), "{ok}");
        }
        for bad in ["0.0.0.0:8443", "[::]:8443", "203.0.113.7:8443", "[2001:db8::1]:8443"] {
            assert!(!is_private_listen(bad.parse().unwrap()), "{bad}");
        }
    }
}
