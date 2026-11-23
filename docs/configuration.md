# Configuration

Every file Sodium Remote reads or writes, where it lives on each platform, and the environment variables that change it. A computer needs no configuration to work on its network; the only file you may write yourself is `server.toml`.

## Where things live

| | Linux | Windows | macOS |
|---|---|---|---|
| **Data, host half** | `$XDG_DATA_HOME/sodium-remote/host` (default `~/.local/share/sodium-remote/host`) | `%LOCALAPPDATA%\SodiumRemote\host` | `~/Library/Application Support/com.sodiumos.remote/Host` (the host key is in the Keychain) |
| **Data, client half** | `$XDG_DATA_HOME/sodium-remote/client` | `%LOCALAPPDATA%\SodiumRemote\client` | `~/Library/Application Support/com.sodiumos.remote` (the key is in the Keychain) |
| **Configuration** | `$XDG_CONFIG_HOME/sodium-remote/` (default `~/.config/sodium-remote/`), then `/etc/sodium-remote/` | `%APPDATA%\SodiumRemote\` | The app's preferences: `~/Library/Preferences/com.sodiumos.remote.plist` |

- The host and the client keep **separate** data folders, which share no file, so a computer that is both has two device keys and two identities.
- Where there are two configuration directories (Linux), the **first** that has the file wins; they're not merged.
- A test build of the Mac app uses the bundle ID `com.sodiumos.remote.dev` and `…/com.sodiumos.remote.dev/` folders.
- **Moving from an older name.** The first start moves or copies the data from the directories an earlier name used (on Linux, `~/.local/share/sodiumos/remote` and `…/remote-client`; on a Mac, the old app's folder and Keychain items), so pairings survive. The old directory is left as it was.

## Files in the data directory

| File | What |
|---|---|
| `device.key` | This device's Ed25519 private key. On Linux a PKCS#8 PEM file, mode 0600; on Windows the same key encrypted with DPAPI for your user account (the file starts with `SODIUM-REMOTE-DPAPI-1`); on a Mac the key is in the Keychain instead. It *is* the device's identity: deleting it makes a new device that must be paired again. Never share it |
| `paired.json` | The paired devices: ID, name, when paired, when last connected, and an address if paired over the LAN. A host also stores its paired-list sequence number here |
| `app.json` | The client app's settings: each computer's own name, system and last route |
| `thumbs/<id>.png` | The last picture of each computer, for its card (the folder is 0700, each file 0600). Deleted when you forget the computer, or turn **Show previews** off |
| `settings.json`, `recent.json` | The SodiumOS host's settings and recent connections |
| `host.txt`, `pairing-code.txt` | Written by the desktop host program: its ID, LAN port and name; the code while pairing is open (removed once a client pairs) |

On Linux and Mac the files are mode 0600 and the folder is 0700. On Windows the folder is under your own profile, protected by the profile's own access list, and `device.key` is also encrypted with DPAPI.

## `server.toml`

Which server to use. **Nothing is built in**: with no `server.toml` (and no imported link), a device doesn't contact any server.

```toml
[server]
url = "wss://relay.example.com:8443/remote"
ca_file = "relay-ca.pem"
token = "…"
```

| Key | Required | Meaning |
|---|---|---|
| `url` | yes | The server: `wss://host[:port]/remote`. Must start with `wss://` |
| `ca_file` | no | A CA certificate (PEM) to pin, relative to the file's directory. When set, **only** this CA is trusted, for the rendezvous and for TURN over TLS. Leave it out for a server with a public certificate: the system's roots are used |
| `token` | no | The enrolment token, sent when this device registers (needed on hosts, if the server asks for one) |

Unknown keys inside `[server]` are an error, so a typo is noticed. A file without a `[server]` table configures nothing. A broken file is logged and the device works as if there were no server. Linux: `~/.config/sodium-remote/server.toml`, then `/etc/sodium-remote/server.toml`. Windows: `%APPDATA%\SodiumRemote\server.toml`. The Mac keeps the same three values in its preferences (`RemoteServerURL` and friends), with import and export in Settings › Server.

You can write it by hand, or **import a link**: opening a `sodium-remote://server?url=…&ca=…&token=…` link (or pasting it in Settings › Server) writes `server.toml` (and the CA as `server-ca.pem`, beside it, mode 0600) after the app asks you to trust the CA. See [self-hosting](self-hosting.md#connecting-devices).

A server named `remote.sodiumos.com` (the planned hosted service) is trusted through the public CA system without a CA prompt, and shown as "Sodium Remote service". Nothing contacts it unless a link or file names it.

## `networks.toml`

Optional. Names networks so the connection chip says something friendlier than "VPN" for a direct route into them. It sits beside `server.toml`.

```toml
[networks]
"198.51.100.0/24" = "Office VPN"
"fd00::/8" = "Home mesh"
```

Each key is a network in CIDR form (or a single address) and each value is the label. The most specific match wins. The first `networks.toml` found in the configuration directories is used.

## Environment variables

| Variable | Used by | Meaning |
|---|---|---|
| `SODIUM_REMOTE_DATA` | The Linux and Windows apps and the client engine | Replaces the data directory. (The earlier name, `SODIUMOS_REMOTE_DATA`, is read when this one is unset) |
| `XDG_DATA_HOME`, `XDG_CONFIG_HOME` | Linux | The usual base directories |
| `LOCALAPPDATA`, `APPDATA` | Windows | The usual folders |
| `SODIUM_REMOTE_HOST_DEBUG` | The desktop host | `1` logs the first frames' status and rectangles |
| `SODIUM_REMOTE_PINCH` | The macOS host | `cmd-scroll` sends pinch as ⌘+scroll instead of a pinch gesture |
| `SODIUM_REMOTE_OPUS` | Building the macOS host | Where the static Opus library is |
| `REMOTED_DATA_DIR`, `REMOTED_HOLD_SECS`, `REMOTED_PAIRING_SECS`, `REMOTED_RUNTIME_DIR` | The SodiumOS host | Tests: its data directory, how long a lost session is held, pairing window length, runtime directory |
| `RUST_LOG` | The server and the Rust programs | Log levels in `tracing` filter syntax, for example `RUST_LOG=remote_rendezvous=debug` |
| `SODIUM_REMOTE_*` (`SSH`, `SERVER_NAME`, …) | `relay/deploy/` scripts | See [self-hosting](self-hosting.md#parameters) |

## Command lines

The apps and programs take options. `sodium-remote-server --help` lists its options, and `--version` prints the version, the license and where the source is (AGPL §13). The Mac app writes logs to `~/Library/Logs/com.sodiumos.remote/`.

- **`sodium-remote`** (Linux) / **`SodiumRemote.exe`** (Windows): `--data DIR`, `--lan-only` (don't use the server), and `--background` on Windows.
- **`sodium-remote-host`** (the desktop host program: Windows, macOS): `--data DIR`, `--name NAME`, `--port N` (LAN listener), `--pair`, `--no-mdns`, `--rendezvous URL`, `--bind ADDR`, `--backend windows|mac|fake`, `--encoder NAME`, `--list`.
- **`remoted`** (the SodiumOS host): `remoted run`, `remoted id` (prints the device ID and short code).
- **`sodium-remote-server`**: see [self-hosting](self-hosting.md). One option worth knowing: `--client-idle-days N` (default 30; 0 never) forgets a client no host has paired after N days without a connection, so devices that register and leave can't fill the table. Clients also share a quota of 8,000 of the 10,000 device slots, which leaves room for hosts.

## Server-side files

`/etc/sodium-remote-server/` (`turn-secret`, `turnserver.conf`, `tls/`, `hosts.allow`) and `/var/lib/sodium-remote-server/` (`state.json`, `tokens/`) are described in [self-hosting](self-hosting.md#what-gets-installed).
