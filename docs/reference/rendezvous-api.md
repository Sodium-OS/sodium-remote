# Rendezvous API (v1)

The contract between Sodium Remote's devices and `sodium-remote-server` (crate `remote-rendezvous`), the server that finds hosts, relays signalling, runs the pairing rendezvous and hands out TURN credentials. The code is `relay/`; deployment is `relay/deploy/` (any host: the server name, SSH target and ports are parameters).

**Versioning:** API v1 changes only by addition (new fields, frames, codes and requests); devices ignore what they don't know.

## 1. What the server is trusted with

- **Nothing secret.** The server is untrusted, as the spec requires.
  - Signalling and pairing payloads are opaque `blob`s. The server checks only their size and base64url encoding, and never parses them.
  - Endpoints must seal every blob end to end with keys from pairing (the core's job), so the server learns only metadata: who talks to whom, when, and how many bytes.
- **What it does decide:**
  - who may use its own resources: signalling to a host, and TURN credentials;
  - the authority for that is the host's own signed list of paired clients.
- **What a malicious server could still do:** drop or delay messages, lie about presence, or refuse service. It can't read or forge sealed blobs, impersonate a device to another device, or authorise a client the host didn't sign for.
- **Pairing:** the 6-digit code never reaches the server, not even hashed (section 7). So the server can't take part in SPAKE2 any better than a stranger guessing codes, and the host allows 3 guesses per window.

## 2. Transport

| | |
|---|---|
| Endpoint | Whatever the device is configured with: `wss://host[:port]/remote` (the server listens on 8443 by default). Nothing is compiled in. |
| TLS | TLS 1.2/1.3, ALPN `http/1.1`. See "Which server, and trust" below. |
| Health | `GET https://host:port/healthz` → `ok` |
| Frames | WebSocket **text** frames, one JSON object each, at most 64 KiB. Binary frames are refused. |
| Encoding | Binary values are base64url **without padding** (RFC 4648 §5). The one exception is the TURN `credential`, which is standard base64 because coturn wants it that way. |
| Keepalive | The server pings every 20 s and drops a connection that has sent nothing (pongs count) for 60 s. |
| Unknown fields | Ignored, so new fields can be added without a version bump. |

### Which server, and trust

- **No server is built in.** A device uses one only when its configuration names one; with none, hosts and clients still find each other on the LAN (mDNS) and by address, and Settings › Server says so.
- **`server.toml`** (`[server] url, ca_file, token`), first match wins:
  - Linux: `~/.config/sodium-remote/server.toml`, then `/etc/sodium-remote/server.toml`;
  - Windows: `%APPDATA%\SodiumRemote\server.toml`;
  - Mac: the same three values in the app's preferences, with import and export.
- **Links and QR codes:** `sodium-remote://server?url=<percent-encoded wss URL>&ca=<base64url DER>&token=<token>`. Importing one with a `ca` asks the user before the CA is trusted; it is then saved as `server.toml` plus `server-ca.pem`.
- **Trust:** Web PKI through the platform's root certificates by default. A `ca_file` (or a link's `ca`) pins that CA instead, and then nothing else is trusted, for the rendezvous and TURN over TLS alike. A CA is never fetched at run time: a client that downloaded its trust anchor from the server it's meant to check would check nothing.
- **The hosted service:** a planned service at `remote.sodiumos.com` (`OFFICIAL_SERVER_HOST`) is never dialled unless configured. Links and files naming it are trusted through Web PKI without a prompt (a link can't pin a CA for it) and are labelled "Sodium Remote service".
- **Operators** choose the certificate: one from a public CA (Let's Encrypt or any other; the default in `deploy.sh`), or a private CA made on the server whose certificate devices pin (`SODIUM_REMOTE_TLS_MODE=private-ca`, `deploy.sh fetch-ca`, `deploy.sh rotate-leaf`; leaves are 1-year and rotate without client changes, because clients pin the CA).

## 3. Identity

- **Device ID** = the device's Ed25519 public key: 32 bytes, base64url, 43 characters.
  - The ID is self-certifying: whoever can sign for it *is* that device.
  - **The host's key** is `~/.local/share/sodium-remote/host/device.key` on Linux (`%LOCALAPPDATA%\SodiumRemote\host\device.key` on Windows): an Ed25519 private key in PKCS#8 PEM, mode 0600, written by the host on first start.
    - `openssl genpkey -algorithm ed25519` writes the same format, and `ed25519-dalek` reads and writes it with its `pkcs8` feature.
    - `deploy.sh host-id` and `allow-host` read this file.
- **Role:** `host` or `client`. A device registers with its role on first contact, and the role is pinned: the same key can't come back with the other role (`role_mismatch`).
- **Enrolment** decides who may register as a host (otherwise anyone could become a "host" and get TURN credentials). The server's `--enrol`:

  | Mode | A new host registers when | Afterwards |
  |---|---|---|
  | `token` (default) | its `auth` carries a valid enrolment token, or it is on `--hosts-allow` | it authenticates without the token |
  | `open` | always | — |
  | `allowlist` | its ID is on `--hosts-allow` | checked at every authentication; removing it (SIGHUP) drops its connection |

  - **Tokens:** `sodium-remote-server token new [--uses N] [--expires 30m|24h|7d|never] [--account LABEL]` prints a token (default: one use, 7 days). `token list` and `token revoke ID` manage them; the server keeps only their SHA-256. The host's user puts the token in `server.toml` (`token = "…"`) or imports a link that carries it. `deploy.sh token-new` runs it on the server.
  - **Errors:** no token where one is needed: `host_not_allowed` ("this server needs an enrolment token"); an unknown or revoked token: `token_invalid`; past its expiry or used up: `token_expired`. Failed tokens count against the registration limit (5 an hour per address), so guessing is slow. When that limit is reached the answer is `rate_limited`; a client may read an optional `retry_after` (seconds) in it. A host treats `token_invalid`, `token_expired` and `host_not_allowed` as final until its settings change, and waits at least five minutes after `rate_limited`.
  - **Clients** need no enrolment and the server ignores a token one sends (only a host redeems a token, so the app's own client connection, which reads the same `server.toml`, can't spend the host's one-use token; the apps send it on the host connection only). A client can do nothing useful until some host's signed list includes it, apart from joining a pairing session.
  - **Allowlist commands:** `deploy.sh allow-host [id]` and `disallow-host [id]` (with no argument they read the host key above and derive the device ID locally; only the public ID goes to the server).
  - Settings › Remote shows "not enrolled" for `host_not_allowed`, and the token states for the token codes.

## 4. Connecting and authenticating

```
server → {"t":"challenge","v":1,"server":"relay.example.com","nonce":"<32 bytes>",
          "source":"https://github.com/sodium-os/sodium-remote"}
device → {"t":"auth","device":"<id>","role":"host"|"client","sig":"<64 bytes>","token":"<optional>"}
server → {"t":"auth_ok","device":"<id>","role":"host","registered":true|false,
          "account":"<optional>","notice":{"code":"…","text":"…","url":"https://…"}}
     or → {"t":"redirect","url":"wss://node2.relay.example.com/remote"}
```

- **What is signed:** `sig` is Ed25519 over these bytes, concatenated:

  ```
  "remote-rendezvous/v1/auth" 0x00 server 0x00 role 0x00 nonce
  ```

  - `server` is the host name in the URL the device connected to and checked over TLS (after a redirect, the redirect's), lowercase, without a port, as UTF-8. It is never taken from the challenge, whose `server` is only informational: a server that passed another server's challenge on to a device would get a signature that is useless at that other server. The server accepts its `--server-name` and each `--server-alias` (for a cluster node reached by its own name after a redirect, or another DNS name).
  - `role` is the ASCII string `host` or `client`.
  - `nonce` is the 32 raw bytes.
  - Verification is strict (`verify_strict`: no small-order keys, no malleable signatures).
- **Timeout:** the device has 10 s to answer the challenge.
- **On failure** the server sends `{"t":"error","id":null,"code":...}` and closes. Codes: `auth_failed`, `host_not_allowed`, `token_invalid`, `token_expired`, `quota_exceeded`, `payment_required`, `role_mismatch`, `rate_limited`, `server_full`, `bad_request`.
- **Registration:** `registered: true` means this authentication registered the device just now. `token` is read only then (section 3).
- **Additive fields:**
  - `source` in the challenge: where the server's source code is offered (AGPL-3.0 section 13).
  - `account` in `auth_ok`: the account of the token the device registered with, if any.
  - `notice` in `auth_ok`, and the event `{"t":"notice","code","text","url"}` at any time: a message from the operator (`--notice-code`, `--notice-text`, `--notice-url`). `text` is shown as written; `url` is only ever `https://`.
- **Redirect:** a server may answer a device it doesn't know with `redirect` instead of `auth_ok` (`--redirect URL`, for example a cluster sending new devices to their node) and close. The device follows it once, only to `wss://` and only to the host it was configured with or a subdomain of it (`sub.example.com` from `example.com`; never the parent, a sibling or another domain), and an IP address only to itself, with the same trust and token. Registered devices are never redirected.
- **One connection per device.** A new authenticated connection replaces the old one:
  - the old connection gets `{"t":"replaced"}` and is closed;
  - its sessions and pairings end with reason `peer_offline`.
- **Presence:** a host is "online" exactly while it has an authenticated connection. Keeping the WebSocket open *is* host presence.
- **Asleep** (Wake-on-LAN groundwork):
  - **Going to sleep:** a host about to sleep (logind `PrepareForSleep`) sends `{"t":"sleep","id":n}`. The server records the time, replies `{"since":<Unix seconds>}`, and closes the connection; core's `sleep_notice()` closes its side too.
  - **What clients see:** clients the host authorises get `state: "asleep"` with that `since`, instead of `offline`, in `presence` replies and events. The host card shows "Asleep since 23:10".
  - **Waking:** the host's next authentication clears the mark, and its clients get `online`.
  - **Never back:** a host asleep for 24 hours counts as `offline` again. Queries apply this at once; housekeeping (every 5 s) pushes the `presence` change to online clients and clears the mark.
  - **Restarts:** the mark is stored with the device, so it survives a server restart.

## 5. Requests and replies

- **Request:** every request after `auth_ok` is `{"t":"<request>","id":<u64>, ...fields}`. The device picks `id`; the reply echoes it.
- **Reply:** exactly one per request, in order on the connection:

  ```
  {"t":"result","id":7, ...fields}
  {"t":"error","id":7,"code":"not_authorised","message":"this host has not paired this client"}
  ```

  - A frame that isn't JSON, or has no numeric `id`, gets an error with `"id":null`.
- **Events** are unsolicited frames from the server. They never carry `id` (section 6).
- **Ordering:** a reply to a request is always sent before any event that the same request caused on this connection.

### Host requests

| Request | Fields | Result | Notes |
|---|---|---|---|
| `paired_list` | `list`, `sig` | `seq`, `clients` (count) | Replaces the host's paired-client list. See below. |
| `get_paired_list` | | `seq`, `list`, `sig` (all `null` if none) | Exactly what the server holds, to compare with the host's own copy |
| `pair_open` | `name` (display name, 1–64 bytes) | `pairing`, `expires_in` (s) | Opens a 5-minute pairing window (section 7). One per host at a time. |
| `signal` | `session`, `blob` | `{}` | Opaque blob to the client of `session` |
| `close` | `session` | `{}` | Ends a signalling session |
| `pair_msg` | `pairing`, `blob` | `{}` | Opaque SPAKE2 message to the client making the current attempt |
| `pair_fail` | `pairing` | `attempts_left` | The current attempt's SPAKE2 failed: the client is let go, and the window closes when no attempts are left |
| `pair_close` | `pairing` | `{}` | Closes the window: paired (one use) or cancelled |
| `turn_credentials` | | `username`, `credential`, `ttl`, `urls` | Section 9 |
| `unregister` | | `{}` | Forgets this device and its list. The server then closes the connection. |
| `sleep` | | `since` (Unix seconds) | The host is going to sleep. Its clients see it `asleep` until it authenticates again (at most 24 h). The server then closes the connection. |

**The paired-client list.**

- **The payload:** the host signs this exact JSON and uploads its bytes as `list` (base64url):

  ```json
  {"v":2,"host":"<this host's ID>","server":"relay.example.com","seq":42,"clients":["<client ID>", "..."]}
  ```

- **The signature:** `sig` is Ed25519 by the host over `"remote-rendezvous/v1/paired-list" 0x00 || payload bytes`.
  - The payload names the server it is for (`server`: the host name the host connected to, as it signs at authentication), so a server can't take a list it holds, such as an older one that still has a revoked client, to another server where the host's `seq` is lower.
- **Checks:**
  - the signature verifies;
  - `v` is 2 (a v1 list, which named no server, is refused);
  - `host` is the connection's own ID;
  - `server` is one of this server's names (`--server-name`, `--server-alias`);
  - at most 64 clients;
  - the payload is at most 8 KiB;
  - `seq` is **strictly greater** than the stored list's. A replayed or older list gets `stale_seq`.
  - The host must therefore keep `seq` monotonic across restarts: persist a counter, or use a clock that never goes backwards.
- **Storage:** the server stores the list verbatim (it survives restarts). This list is **the only thing that authorises a client** for this host.
- **Effects of an upload:**
  - **Clients removed: revocation, immediately.** Their sessions with this host end with `session_closed` reason `revoked`, and an online removed client gets `{"t":"revoked","host":...}`.
  - **Clients added:** if online, each gets a `presence` event for this host.

### Client requests

| Request | Fields | Result | Notes |
|---|---|---|---|
| `presence` | `hosts` (≤ 32 IDs) | `hosts: [{host, state, last_seen, since}]` | `state`: `online`, `asleep`, `offline` or `not_authorised`. `last_seen` (Unix seconds, when it disconnected) when `offline` or `asleep`. `since` (Unix seconds, its sleep notice) only when `asleep`. Otherwise both are `null`. |
| `connect` | `host` | `session` | Asks for the host by ID. The host gets `incoming`. Errors: `not_authorised`, `host_offline`, `too_many` (8 open sessions). |
| `signal` | `session`, `blob` | `{}` | Opaque blob to the host |
| `close` | `session` | `{}` | |
| `pair_discover` | | `hosts: [{pairing, host, name, expires_in}]` | Open pairing windows nobody is attempting right now, newest first, at most 8. Any registered client may ask (that's how it gets paired). |
| `pair_join` | `pairing` | `pairing`, `host`, `attempt`, `expires_in` | Starts an attempt in that window |
| `pair_msg` | `pairing`, `blob` | `{}` | Opaque SPAKE2 message to the host |
| `pair_close` | `pairing` | `{}` | Leaves the attempt (the host gets `pair_left`; the window stays open) |
| `turn_credentials` | | as for hosts | Only while some host's list includes this client |
| `unregister` | | `{}` | |

## 6. Events

| Event | Fields | To | When |
|---|---|---|---|
| `incoming` | `session`, `client` | host | A paired client called `connect` |
| `signal` | `session`, `blob` | the other side | The peer sent `signal` |
| `session_closed` | `session`, `reason` | both sides | `closed` (either side), `revoked`, `peer_offline` (disconnected or replaced), `unregistered` |
| `pair_joined` | `pairing`, `attempt` | host | A client started attempt 1, 2 or 3. The client's ID is deliberately not given: the host must learn and pin it from the SPAKE2-protected exchange, not from the server. |
| `pair_msg` | `pairing`, `blob` | the other side | |
| `pair_left` | `pairing` | host | The client left its attempt or disconnected; the window stays open |
| `pair_closed` | `pairing`, `reason` | client of the attempt, or both sides when the window closes | To the client: `failed` (the host's `pair_fail`) or `closed`. When the window closes: `closed`, `expired`, `attempts_used`, `peer_offline` (the host went away), `unregistered`. |
| `presence` | `host`, `state`, `last_seen`, `since` | online clients the host authorises | The host connected, disconnected or went to sleep; it slept over 24 h and now counts as offline; or a new list added this client. Same fields as a `presence` reply entry. |
| `revoked` | `host` | client | The host's new list dropped this client, or the host unregistered |
| `replaced` | | the old connection | The same device authenticated again elsewhere |

**Signalling sessions:**

- They carry anything the endpoints want: ICE candidates, session offers, ICE restarts.
- They stay open until closed, revoked, or until either side's connection drops.
- A session doesn't survive a reconnect. After a network change the client calls `connect` again and the endpoints match the new session to their media session by what's inside their sealed blobs.
- Blobs up to 16 KiB each.

## 7. Pairing rendezvous

**The 6-digit code never reaches the server**, in any form, hashed or otherwise. A hash of 10⁶ possible codes is brute-forced in moments, and would let the server sit in the middle of SPAKE2. Instead, the host opens a window and clients find it by discovery.

1. **The host opens a window.**
   - The user clicks "Pair a device" in Settings → Remote, which shows the code (and QR).
   - The host sends `pair_open` with its display name, authenticated as itself by the connection.
   - The window lasts 300 s. One window per host at a time (`too_many` otherwise).
2. **The client finds it.**
   - An unpaired client calls `pair_discover`; on the LAN it also browses mDNS.
   - Usually there is exactly one window: the client shows its name, or lets the user pick if there are more.
   - Each entry has `pairing`, `host` (ID), `name` and `expires_in`.
   - Windows with an attempt in progress, or with no attempts left, aren't listed.
3. **One attempt.**
   - The client sends `pair_join` with the `pairing` ID; the host gets `pair_joined` with `attempt` 1, 2 or 3.
   - The two run SPAKE2 with the code the user typed, through `pair_msg` blobs (at most 4 KiB each), and pin each other's keys.
   - Only one client attempts at a time.
4. **The outcome.**
   - **Success:** the host sends `pair_close` (the window is one use) and then uploads a new paired list that includes the client.
   - **Failure** (key confirmation didn't match): the host sends `pair_fail`. The client gets `pair_closed` (`failed`) and the window reopens for the next attempt.
5. **The end of a window.**
   - At most 3 attempts per window. The host counts failed SPAKE2 runs and closes the window; the server also stops after the third attempt as a backstop: `pair_fail` with no attempts left closes the window (`attempts_used`).
   - The window also ends at expiry (`expired`), when the host closes it, or when the host disconnects.
   - A client that leaves (`pair_close`) or disconnects ends only its own attempt (the host gets `pair_left`). It still counts as one of the 3.

**Limits:**
- 32 messages per window, all attempts and both directions together.
- `pair_open`: 10 per 10 minutes per host.
- `pair_discover`: 10 per minute per address (IPv6 per /64).
- `pair_join`: 5 per minute per device and 10 per 10 minutes per address.
- A join to a missing, expired or busy window gets `not_found`; to a window with no attempts left, `too_many`.

**What the server learns:** that a host is pairing, its display name, and which client attempted, how many times and when. All of that is metadata the spec allows. A stranger who discovers the window gets 3 guesses at a 6-digit code, a 3-in-10⁶ chance.

## 8. Revocation

| What | How | Effect |
|---|---|---|
| A host drops a client | Upload a new list without it (`seq` + 1) | At once: its sessions with the host close (`revoked`), it gets `revoked`, `connect` and `turn_credentials` refuse it (`not_authorised`) unless another host still lists it |
| A device leaves | `unregister` | Its record, and a host's list, are deleted. Its sessions close; a host's pairing window closes, a client's pairing attempt ends. A host's clients get `revoked`. |
| The operator withdraws a host | `deploy.sh disallow-host <id>` (allowlist mode) | The host's connection drops and it can't authenticate again |
| TURN | Credentials can't be recalled | Ones already issued can make new allocations until they expire (≤ 10 min). An allocation made before revocation keeps working for as long as its owner refreshes it (section 9). The host's end-to-end check of pinned keys is what really shuts a revoked client out. |

## 9. TURN credentials

```json
{"t":"result","id":9,"username":"1793788614:<device ID>","credential":"<base64>","ttl":600,
 "urls":["stun:relay.example.com:3478","turn:relay.example.com:3478?transport=udp",
         "turn:relay.example.com:3478?transport=tcp","turns:relay.example.com:5349?transport=tcp"],
 "relays":[{"region":"sg","urls":["turn:sg.relay.example.com:3478?transport=udp","…"]},
           {"region":"us","urls":["turn:us.relay.example.com:3478?transport=udp","…"]}]}
```

- **Format:** coturn's `use-auth-secret` (the TURN REST API).
  - `username` = expiry in Unix seconds, a colon, then the device ID.
  - `credential` = base64(HMAC-SHA1(shared secret, username)).
  - Use them as the TURN username and password. The realm is the server's.
- **Regions (optional, `--relay REGION=URL`):** `relays` lists relays by region; every relay shares the secret. A device measures each region's round trip (a TCP connect to its first URL) and uses the fastest; there is no picker. `urls` stays for older devices: the plain `--turn-url` list, or the first region's.
- **Who gets them:** enrolled hosts, and clients that some host's list includes. A device may ask 10 times a minute.
- **Lifetime:** `ttl` is 600 s. That bounds when an allocation can be *created*, not how long it lives.
  - An existing allocation keeps refreshing (and its permissions keep renewing) after its credential expires; measured: still relaying 181 s past expiry, with refreshes every 100 s.
  - So a long session needs no new credentials until it allocates again, for example after an ICE restart.
- **What the coturn template (`relay/deploy/turnserver.conf.in`) sets up:**

  | Listener | Use |
  |---|---|
  | UDP and TCP 3478 | STUN and TURN |
  | TLS 5349 | TURN over TLS, same certificate as the rendezvous. No DTLS. |
  | UDP 50000–50099 | Relayed addresses (parameters) |

  - **Relaying:** UDP only (no RFC 6062 TCP relaying).
  - **Quotas:** `user-quota` allocations per username (12), `total-quota` in all (60), and `max-bps` per allocation (5 MB/s, 40 Mbit/s); all parameters.
  - **Lifetime:** at most 3600 s, unless refreshed.
- **Peers coturn refuses** with 403: private, loopback, link-local, carrier-grade NAT, multicast and reserved ranges, and the cloud's internal resolvers.
  - coturn does grant its own relay address: that's where it sends relay-to-relay traffic. An nft table on the server lets the turnserver user reach only the relay ports there, and drops everything else it sends to private space.
- **Relay-to-relay works.** Both sides may use relay candidates on the same server: coturn delivers between its own allocations internally, so it doesn't need the cloud NAT to hairpin. A relay candidate paired with the other side's server-reflexive or peer-reflexive candidate works too, through any NAT, because TURN permissions are per IP address.
- **Quota hooks:** the server calls `Hooks::register`, `turn_credentials` and `connect` (in `relay/src/hooks.rs`), which may refuse with `quota_exceeded` or `payment_required`. They are off by default (`NoHooks`); the open-source server has no accounts or billing.

## 10. Limits and error codes

| Limit | Value |
|---|---|
| New connections | 30 per minute per address (HTTP 429 instead of the upgrade) |
| New device registrations | 5 per hour per address; an `unregister` gives one back. 10,000 devices at most. |
| Requests | 20 per second per device, bursts of 100 |
| `pair_open` / `pair_discover` / `pair_join` | 10 per 10 minutes per host / 10 per minute per address / 5 per minute per device and 10 per 10 minutes per address |
| `turn_credentials` | 10 per minute per device |
| Frame / signal blob / pairing blob / list payload | 64 KiB / 16 KiB / 4 KiB / 8 KiB |

| Code | Meaning |
|---|---|
| `bad_request` | Malformed frame, field, ID or encoding |
| `auth_failed` | The challenge signature didn't verify |
| `host_not_allowed` | The host isn't enrolled on this server |
| `role_mismatch` | This key registered with the other role |
| `server_full` | No room for new devices |
| `forbidden` | The request isn't allowed for this role |
| `not_authorised` | No host's list includes this client (or not this host's) |
| `host_offline` | The host has no connection right now |
| `not_found` | No such session or pairing window, or not yours, or it's busy, or nobody is attempting right now |
| `rate_limited` | Slow down; see the limits above |
| `stale_seq` | The list's `seq` isn't higher than the stored one |
| `bad_signature` | The list's signature didn't verify |
| `too_large` | A blob or list is over its limit |
| `too_many` | Too many clients, sessions, pairing windows, attempts, pairing messages or hosts in a query |
| `token_invalid` | The enrolment token isn't one this server knows (or it was revoked) |
| `token_expired` | The enrolment token is past its expiry or used up: ask for a new one |
| `quota_exceeded` | The device's account is over a quota (a hook; off by default) |
| `payment_required` | The device's account needs a payment (a hook; off by default) |

## 11. What the server logs and stores

- **Logs** (journald): metadata only, with device IDs shortened to 12 characters.
  - Events logged: authentications, registrations, list uploads (seq and client count), session open and close (signal count and bytes), pairing windows opened, discovered, attempted, failed and closed, TURN issuance, revocations, sleep notices (and a host counted offline after 24 h asleep) and disconnects.
  - Never logged: blobs, display names, signatures, nonces or credentials. The rendezvous logs no IP addresses.
- **coturn's logs:**
  - It logs its own sessions with `simple-log`, never `verbose`: client and peer addresses, the username (expiry:device ID), and byte counts.
  - They go to a journal namespace of their own, `sodium-remote` (`LogNamespace=sodium-remote` in sodium-remote-coturn.service).
  - `/etc/systemd/journald@sodium-remote.conf` keeps them **7 days** (`MaxRetentionSec=7day`), at most 200 MB (`SystemMaxUse=200M`), and forwards them nowhere (not to syslog, kmsg, the console or wall).
  - Read them with `journalctl --namespace=sodium-remote -u sodium-remote-coturn`. The system journal doesn't get them.
- **State** (`/var/lib/sodium-remote-server/state.json`): registered devices (role, first and last seen, a sleeping host's `asleep_since`, and the account of the token it registered with) and each host's signed list. Enrolment tokens are `tokens/<id>.json` beside it: the token's SHA-256, uses left, expiry and account; never the token itself.
- **Memory only:** pairing windows (with display names), signalling sessions and rate limits.

## 12. Example

```
C→S {"t":"auth","device":"PAJQ…","role":"client","sig":"…"}
S→C {"t":"auth_ok","device":"PAJQ…","role":"client","registered":false}
C→S {"t":"connect","id":1,"host":"5i5p…"}
S→H {"t":"incoming","session":"rJwgWMgX…","client":"PAJQ…"}
S→C {"t":"result","id":1,"session":"rJwgWMgX…"}
C→S {"t":"signal","id":2,"session":"rJwgWMgX…","blob":"<sealed offer>"}
S→C {"t":"result","id":2}
S→H {"t":"signal","session":"rJwgWMgX…","blob":"<sealed offer>"}
H→S {"t":"signal","id":5,"session":"rJwgWMgX…","blob":"<sealed answer>"}
S→C {"t":"signal","session":"rJwgWMgX…","blob":"<sealed answer>"}
C→S {"t":"turn_credentials","id":3}
S→C {"t":"result","id":3,"username":"1793788614:PAJQ…","credential":"…","ttl":600,"urls":[…]}
```

A runnable client is `relay/tests/common/mod.rs`; `relay/examples/probe.rs` drives a deployed server through every step above (`relay/deploy/verify.sh`).
