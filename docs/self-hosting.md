# Self-hosting

Away from your own network, Sodium Remote's computers find each other through a **rendezvous server** and fall back to a **relay**. You can run both on one small server you own. A hosted relay service at remote.sodiumos.com is planned; until then, use your own server or connect on your network or by address.

The server and its deploy scripts are in the repository's `relay/` directory.

## What you're running

| Part | What it does | Port |
|---|---|---|
| `sodium-remote-server` | The rendezvous server: it knows which hosts are online, introduces clients to them, runs pairing windows and hands out relay credentials. It sees metadata, never content ([security](security.md)) | TCP 8443 |
| coturn | The STUN and TURN relay that carries encrypted media when no direct path exists | UDP and TCP 3478, TCP 5349 (TLS), UDP 50000–50099 |

`deploy.sh` installs both as systemd services, sets up TLS, a firewall guard and journald limits, and verifies them. The contract between devices and the server is in [the rendezvous API reference](reference/rendezvous-api.md).

## Requirements

- **A server** with a public IPv4 address (a small virtual machine is enough), running a **Debian-family Linux** (the setup uses `apt` to install coturn) on **amd64**. It needs systemd, `nft` (nftables), `openssl` and `ss` (iproute2).
- **A DNS name** pointing at the server, for example `relay.example.com`. Devices verify the server by this name.
- **A TLS certificate** for that name, from Let's Encrypt or another public CA, or a private CA the deploy script makes for you ([below](#dns-and-tls)).
- **The machine you deploy from**: this repository, Rust (to build `sodium-remote-server` in release mode), `ssh`, and an account on the server that is root or has passwordless `sudo`. The binary is built here and must run on the server, so the server's glibc must be at least as new as the build machine's needs; `deploy.sh` checks and stops if not.
- **Open ports** on the server's firewall and your cloud's security group: TCP 8443, UDP and TCP 3478, TCP 5349, and UDP 50000–50099 (adjustable).

## DNS and TLS

Create an `A` record (and `AAAA` if you use IPv6) for the server's name. The name is also what the server binds device signatures to, so use the same name in every device's configuration.

### Option 1: a public CA (the default)

Get a certificate for the name with any ACME client, for example `certbot certonly --standalone -d relay.example.com` run on the server. Then put the **full chain** and the **private key** where `deploy.sh` expects them:

```
/etc/sodium-remote-server/tls/server.crt   # the full chain
/etc/sodium-remote-server/tls/server.key
```

Both are root-only on the server; systemd hands copies to the services. `deploy.sh` stops with a message until the files exist, and refuses an expired certificate. It never creates or renews one: set up renewal yourself and, after each renewal, copy or link the files and run `sudo systemctl restart sodium-remote-server sodium-remote-coturn` (the services read the files when they start). Devices trust the certificate through the operating system's roots, so a link or `server.toml` needs no CA.

### Option 2: a private CA

For a server with no public certificate (a home server, a LAN), set `SODIUM_REMOTE_TLS_MODE=private-ca`. The setup creates a CA on the server (valid ten years) and signs a one-year certificate for the name. Devices must **pin that CA**: fetch it with `deploy.sh fetch-ca ca.pem` and give it to them as `ca_file` in `server.toml`, or inside a `sodium-remote://` link (see [devices](#connecting-devices)). The setup reissues the certificate 30 days before it expires, and `deploy.sh rotate-leaf` reissues it now. Devices pin the CA, not the certificate, so rotating needs no change on devices.

## Deploying

From the repository's root, on the machine you deploy from:

```sh
SODIUM_REMOTE_SSH=admin@relay.example.com \
SODIUM_REMOTE_SERVER_NAME=relay.example.com \
  relay/deploy/deploy.sh
```

`deploy` builds the server in release mode here, checks the server's architecture and glibc, uploads the binary and templates, and runs `server-setup.sh` as root there. It is **idempotent**: run it again at any time; only what changed is replaced, and only services whose inputs changed restart. It touches only Sodium Remote's own things: the two services, `/etc/sodium-remote-server`, an nftables table `sodium_remote_guard`, and the `coturn` package. If one of the ports is taken by something else, it stops rather than touch it.

When it finishes it lists the listeners and the two services.

### Commands

| Command | What it does |
|---|---|
| `deploy.sh` or `deploy.sh deploy` | Build here, deploy, start |
| `deploy.sh rotate-leaf` | Deploy and issue a new certificate now (private CA only) |
| `deploy.sh status` | Services, listeners, enrolment mode, certificate expiry and recent log lines |
| `deploy.sh token-new [--uses N] [--expires 30m\|24h\|7d] [--account LABEL]` | Mint an enrolment token (see below) |
| `deploy.sh token-list` | List the tokens |
| `deploy.sh token-revoke ID` | Revoke one |
| `deploy.sh host-id` | This machine's host device ID (derived from its host key, which stays here) and its short code, for allowlist mode |
| `deploy.sh allow-host [ID]` | Allowlist mode: let a host register (default: this machine's); reloads the server |
| `deploy.sh disallow-host [ID]` | Remove it; its connection drops |
| `deploy.sh run 'COMMAND'` | Run a shell command on the server as root |
| `deploy.sh fetch-ca FILE` | Private CA: copy the CA certificate (public) here |

### Parameters

Everything is an environment variable. Defaults are in brackets.

| Variable | Meaning |
|---|---|
| `SODIUM_REMOTE_SSH` | **Required** (except for `host-id`). The SSH target, `user@host`. A non-root user needs passwordless sudo |
| `SODIUM_REMOTE_SERVER_NAME` | **Required** for `deploy` and `rotate-leaf`. The server's public DNS name |
| `SODIUM_REMOTE_SSH_KEY` | An SSH identity file (otherwise ssh's own defaults and agent) |
| `SODIUM_REMOTE_SSH_PORT` | The SSH port |
| `SODIUM_REMOTE_PORT` | The rendezvous port [8443] |
| `SODIUM_REMOTE_TURN_PORT` | STUN and TURN, UDP and TCP [3478] |
| `SODIUM_REMOTE_TURNS_PORT` | TURN over TLS [5349] |
| `SODIUM_REMOTE_MIN_PORT`, `SODIUM_REMOTE_MAX_PORT` | coturn's relay port range [50000, 50099]. One port per relay allocation |
| `SODIUM_REMOTE_ENROL` | `open`, `token` or `allowlist` [token]; see [enrolment](#enrolment) |
| `SODIUM_REMOTE_TLS_MODE` | `provided` (your certificate) or `private-ca` [provided] |
| `SODIUM_REMOTE_USER_QUOTA` | TURN allocations per username [12] |
| `SODIUM_REMOTE_TOTAL_QUOTA` | TURN allocations in all [60] |
| `SODIUM_REMOTE_MAX_BPS` | TURN bytes per second per allocation [5000000, which is 5 MB/s or 40 Mbit/s, each direction] |
| `SODIUM_REMOTE_REALM` | coturn's realm [the server name] |
| `SODIUM_REMOTE_PRIVATE_IP`, `SODIUM_REMOTE_PUBLIC_IP` | Override the addresses the setup detects (the public one from DNS, the private one from the default route). If they differ, as on a cloud with a 1:1 public address mapping, coturn is told so |
| `SODIUM_REMOTE_HOST_KEY` | A host's Ed25519 device key (PKCS#8 PEM) for `host-id` and `allow-host` [`~/.local/share/sodium-remote/host/device.key`]. Only the public device ID derived from it leaves the machine |

## What gets installed

- `/usr/local/bin/sodium-remote-server`, and the systemd units `sodium-remote-server.service` and `sodium-remote-coturn.service`. The server runs as a systemd dynamic user, with a locked-down sandbox (no capabilities, a read-only file system, and memory and task limits). coturn runs as `turnserver`. The distribution's own `coturn.service` is masked so it never runs with its default configuration.
- `/etc/sodium-remote-server/`: the TURN shared secret (`turn-secret`, generated on the server, root-only, never printed), `turnserver.conf` (root-only, because it holds the secret), `tls/`, and `hosts.allow` in allowlist mode. systemd hands the secret and the certificate to the services as credentials.
- `/var/lib/sodium-remote-server/`: the server's state (`state.json`, enrolment tokens).
- An nftables table `sodium_remote_guard` and a journald namespace `sodium-remote` for coturn's logs.

## coturn: the template and the limits

`turnserver.conf.in` is the template. What it sets up:

- Listeners on the server's own address: STUN and TURN on UDP and TCP 3478, TURN over TLS on 5349. No DTLS.
- **Authentication** with short-lived credentials: the rendezvous issues each device a username (expiry and device ID) and a password derived from the shared secret, valid for 10 minutes. coturn runs with `use-auth-secret`.
- **Relaying** is UDP only (no TCP relaying), with no multicast peers, no admin interface and no software version in answers.
- **Quotas.** `user-quota` allocations per credential (12 by default), `total-quota` in all (60), and `max-bps` per allocation (5 MB/s). Allocations last at most an hour unless refreshed. An allocation uses one port from the range, and an ICE agent can make up to three (UDP, TCP and TLS), so for more concurrent sessions widen `SODIUM_REMOTE_MIN_PORT`–`MAX_PORT` and raise `SODIUM_REMOTE_TOTAL_QUOTA`.
- **A relay can't reach your other networks.** coturn refuses to relay into private, loopback, link-local, carrier-grade NAT, multicast and reserved ranges, and the cloud's internal resolvers; a second wall, the nftables table `sodium_remote_guard`, drops anything the `turnserver` user sends to those ranges, except coturn's own relay ports. This means a user of your relay can't use it to probe your server's internal network.
- **Logs.** coturn logs sessions in `simple-log` mode (addresses, usernames of the form `expiry:device`, byte counts) to a journal namespace of its own, kept 7 days and at most 200 MB, forwarded nowhere. Read them with `journalctl --namespace=sodium-remote -u sodium-remote-coturn`.

## Enrolment

Enrolment decides who may register as a **host** (a computer that can be controlled) on your server. Without it anyone could become a host and use your relay. Clients (computers that control) need no enrolment; they can do nothing useful until a host they're paired with lists them.

| Mode | Who can register a host |
|---|---|
| `token` (the default) | A device that presents a valid enrolment token. Hosts already registered or on the allowlist don't need one again |
| `allowlist` | Only hosts whose device ID is on `/etc/sodium-remote-server/hosts.allow` |
| `open` | Anyone. Use it for a private network only: the server refuses it unless `--listen` is a loopback or private address, or you pass `--open-enrol-on-public-address` |

### Tokens

Mint a token on the server:

```sh
SODIUM_REMOTE_SSH=admin@relay.example.com relay/deploy/deploy.sh token-new --uses 1 --expires 7d
```

It prints the token **once**; the server keeps only its SHA-256. Options: `--uses N` (default 1, `0` for unlimited), `--expires` (`30m`, `24h`, `7d` … or `never`; default 7 days), `--account LABEL` (devices registered with it belong to that account label). `token-list` shows the IDs, state, uses left and expiry (never the tokens), and `token-revoke ID` deletes one. A token is only needed when a host first registers. Failed tokens count against a registration limit of five an hour per address, so guessing is slow.

On the server itself the same commands are `sodium-remote-server token new|list|revoke` (add `--state-dir /var/lib/sodium-remote-server`, and run it as the service's user so the files stay readable by the service).

### Allowlist

With `SODIUM_REMOTE_ENROL=allowlist`, run `deploy.sh host-id` on a host to get its device ID (the host key never leaves it), then `deploy.sh allow-host ID`. `disallow-host ID` removes it and drops its connection. Edits to `hosts.allow` apply on `systemctl reload sodium-remote-server`.

## Connecting devices

A device needs three things: the server's address, optionally a CA to pin, and, for a host, an enrolment token. Give them as a **link**, a **file** or by typing them in Settings › Server.

### A link

```
sodium-remote://server?url=<percent-encoded wss URL>&ca=<base64url DER>&token=<token>
```

For example, for a public-CA server:

```
sodium-remote://server?url=wss%3A%2F%2Frelay.example.com%3A8443%2Fremote&token=<token>
```

`ca` is the DER of the CA to pin, base64url, only for a private CA. A device opens the link (or you paste it in Settings › Server). If the link carries a CA, the app **asks before trusting it**, then saves it. The link carries a token only if you put one in: tokens are secrets, so share a link with a token with the one person who needs it. Newer links may carry more fields; devices ignore the ones they don't know.

### A file

`server.toml` ([configuration](configuration.md#servertoml)), in the user's configuration directory (`~/.config/sodium-remote/` on Linux, `%APPDATA%\SodiumRemote\` on Windows) or, on Linux, system-wide in `/etc/sodium-remote/`:

```toml
[server]
url = "wss://relay.example.com:8443/remote"
ca_file = "relay-ca.pem"   # optional: only for a private CA; relative to this file
token = "…"                # optional: the host's enrolment token
```

The server's URL is `wss://<name>:<port>/remote`. The Mac app keeps the same three values in its preferences, with import and export.

Without a `ca_file`, devices trust the server through the operating system's roots. With one, they trust **only** that CA, for the rendezvous and for TURN over TLS alike. A device never downloads its trust anchor from the server it is checking.

## Regions, redirects and notices

`deploy.sh` sets up one server. These options of `sodium-remote-server` go beyond it; set them by adding flags to the service's `ExecStart` in a systemd drop-in (`sudo systemctl edit sodium-remote-server`; copy the `ExecStart` from `systemctl cat sodium-remote-server`, clear it with an empty `ExecStart=` first, and add the flags).

- **Several relay regions: `--relay REGION=URL`** (repeat; several URLs per region). The server hands out the relay list with each device's credentials, and each device measures the round trip to each region and uses the fastest, with no picker. Every relay shares the one TURN secret. The plain `--turn-url` list still goes to older devices.
- **Redirect: `--redirect wss://node2.example.com/remote`.** A server can answer a device it doesn't know with a redirect, so a cluster can send new devices to a node. A device follows it once, only to `wss://` and only within the same registrable domain as the server it was given (`sub.example.com` from `example.com`, never `example.net`), with the same trust and token. Registered devices are never redirected. Devices sign the host name they connected to, so start each node with its own name as `--server-name` (or add it with `--server-alias NAME`, which may be repeated).
- **Notices: `--notice-code CODE --notice-text TEXT [--notice-url https://…]`.** The server sends the notice to every device when it authenticates, and the app shows it in Settings › Server. Use it for maintenance windows. The URL must be `https://`.
- **Other flags:** `--listen ADDR` (default `0.0.0.0:8443`), `--pairing-ttl SECONDS` (default 300), `--asleep-ttl SECONDS` (how long a sleeping host shows as asleep, default 24 hours), `--client-idle-days N` (a client that no host has paired is forgotten after N days without a connection, default 30; 0 keeps it forever; it registers again on its next start), `--turn-ttl SECONDS` (default 600), `--hosts-allow FILE`. `--plain` serves without TLS, for local tests only. `sodium-remote-server --help` lists them all, and `--version` prints the source location.

## Verifying

```sh
SODIUM_REMOTE_SSH=admin@relay.example.com \
SODIUM_REMOTE_SERVER_NAME=relay.example.com \
  relay/deploy/verify.sh out-dir
```

`verify.sh` checks the deployed server from your machine and saves one log per check in `out-dir`: TLS chains, the health endpoint (`https://relay.example.com:8443/healthz` answers `ok`), STUN, the whole rendezvous API with a temporary test host, TURN allocations over UDP, TCP and TLS using credentials the server issued, relay-to-relay traffic, refused credentials, refused private peers (the egress guard), and the coturn log settings. It prints `PASS` or `FAIL` for each and exits non-zero on any failure. It needs `turnutils_uclient` and `turnutils_stunclient` (from the coturn package; don't run its service on your workstation), `jq`, `openssl`, `curl` and `cargo`, and takes the same variables as `deploy.sh` (plus `SODIUM_REMOTE_CA_BUNDLE`, `SODIUM_REMOTE_PRIVATE_IP` and `SODIUM_REMOTE_DENY_PEERS`). In token mode it mints a one-use token for the run.

## Logs

- **Server:** the systemd journal. `journalctl -u sodium-remote-server`. It logs metadata only, with device IDs shortened to 12 characters: authentications, registrations, list uploads, session opens and closes, pairing windows, TURN issuance, revocations and disconnects. It never logs blobs, display names, signatures, nonces or credentials, and logs no IP addresses. Set `RUST_LOG` (for example `RUST_LOG=remote_rendezvous=debug`) for more.
- **coturn:** the `sodium-remote` journal namespace, as above. This is the one place that records addresses.
- `deploy.sh status` shows the last lines of both.

## Upgrading

Pull the new version of this repository and run `deploy.sh` again. It rebuilds, replaces the binary and templates that changed, and restarts only what changed. Devices reconnect on their own. The server's API (v1) only changes by addition, so older and newer devices work against the same server ([compatibility](compatibility.md)).

## Firewall

`deploy.sh` reports what the server's firewall must allow, but never rewrites it. Open these inbound: TCP 8443 (rendezvous), UDP and TCP 3478 (STUN and TURN), TCP 5349 (TURN over TLS) and UDP 50000–50099 (relayed media). If `ufw` is active, the deploy says so; for a cloud, add the same to its security group.

## Running the server without `deploy.sh`

`sodium-remote-server` is one binary (`cargo build --release -p remote-rendezvous`). A minimal run:

```sh
sodium-remote-server \
  --server-name relay.example.com \
  --tls-cert fullchain.pem --tls-key privkey.pem \
  --turn-secret-file turn-secret \
  --turn-url stun:relay.example.com:3478 \
  --turn-url turn:relay.example.com:3478?transport=udp \
  --enrol token --state-dir /var/lib/sodium-remote-server
```

You then run coturn yourself with the same `static-auth-secret` as `turn-secret` and `use-auth-secret`. The template in `relay/deploy/turnserver.conf.in` shows the settings the project uses.
