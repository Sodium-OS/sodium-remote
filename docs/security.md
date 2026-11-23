# Security

What Sodium Remote protects, how, and what it doesn't. This page describes version 0.1.0, a preview, which hasn't had an independent security audit. To report a vulnerability, see [SECURITY.md](../SECURITY.md).

## What it protects, and from whom

Sodium Remote carries a computer's screen, keyboard, pointer, sound and clipboard between your own computers. It is built so that:

- **Anyone on the network path** (your Wi-Fi, your provider, the internet) can't read or change a session, or take part in it.
- **The servers you use** (a rendezvous server, a relay) can't read or change a session either. They are untrusted by design.
- **Only computers you paired** can connect to a host. A stranger who finds your host can't connect, and can't get a pairing code except by guessing it, with three tries.

It does **not** protect against:

- **A compromised endpoint.** Malware on either computer sees what that computer sees. A paired device that is stolen and unlocked can control the hosts that trust it: remove it from the host's list.
- **Someone at the keyboard.** Physical access to an unlocked, signed-in computer is out of scope.
- **Metadata.** A server sees which of your devices talk to each other, and when ([below](#what-a-server-sees)). A network observer sees the packets' size and timing.
- **Denial of service.** Anyone who can reach a host or server can try to make it busy. The server rate-limits what it can.

## Devices and identity

Each device (a host, or a client) has an **Ed25519 key pair**, made on first use. The **device ID** is the public key (32 bytes, shown as 43 characters). Short codes of the form `XXXX-XXXX` are derived from it for comparing by eye. The same key authenticates the device to a server, and (converted to X25519) is the static key for the encrypted session and for sealing signalling messages. So the ID you pinned at pairing is the ID the encrypted session proves.

A host and a client keep separate keys. The key is the device: delete it and the device must be paired again.

### Where keys are stored

| Platform | Where the private key lives |
|---|---|
| Linux (and SodiumOS) | `device.key` in the data directory, a PEM file, mode 0600, in a directory with mode 0700 |
| Windows | `device.key` in `%LOCALAPPDATA%\SodiumRemote\host` or `…\client`, encrypted with the Windows Data Protection API (DPAPI) for your user account: only the same Windows user on the same computer can read it back. A plain key file from an earlier build is encrypted the next time the app starts, and the device keeps its identity |
| macOS | The Keychain, as the items "Sodium Remote device key" and "Sodium Remote host key" under the app's bundle ID. A key file left from an older version is moved into the Keychain and deleted |

Pairings (the other devices' public keys and names) are in `paired.json` in the same place (or the app's store on a Mac). Public keys aren't secret.

## Pairing

Two devices pair once, with a **six-digit code** shown on the one that will be controlled.

- They run **SPAKE2** (a password-authenticated key exchange, over the Ed25519 group) with the code as the password. Only SPAKE2's messages travel; **the code never leaves the device that shows it or the one that types it, not even hashed**, so a server or a network observer can't learn it. A relay or LAN neighbor that tampers with the exchange makes the keys disagree, which looks like a wrong code.
- Both sides then **confirm** the key with HMACs over a transcript that includes both messages and the host's ID, and exchange their device IDs and names under it. Each stores the other's ID. When pairing is by address only, the host's ID comes in the confirmed part of the exchange, so the ID a client stores is one the code vouched for.
- A pairing window is **one use, five minutes, three attempts**. After three wrong codes pairing closes, and no new code opens until someone at the computer opens pairing again. A server enforces the same as a backstop and rate-limits opening, discovering and joining windows; a host's LAN listener limits how often pairing may be tried and how many connections are open at once. A stranger who finds an open window has three guesses at a six-digit code each time you open one: three chances in a million.
- Someone who knows the code while the window is open can pose as the other party. The code is the secret: don't read it out in public, and close pairing when you're done.

After pairing, a host lists the client as authorized, and clients are removed from that list to revoke ([below](#revocation)).

## The session

A connection starts with **Noise IK** (`Noise_IK_25519_ChaChaPoly_BLAKE2s`): the client already holds the host's static key (pinned at pairing) and the host accepts only keys it has pinned, so no one else can start a session. Initiations carry a timestamp the host checks to refuse replays.

After the handshake, every packet is sealed with **ChaCha20-Poly1305**:

- a 64-bit counter per direction doubles as the nonce, and never repeats;
- a 2048-packet replay window rejects duplicates, and a packet is recorded only after its tag verifies;
- the client rekeys every 120 seconds, and keys older than 180 seconds are never used;
- 29 bytes of each packet are the header and tag.

The keys are **path-independent**: the connection can move between a local, direct or relayed path with no new handshake. One device controls a host at a time; a valid new connection from another paired device takes over.

Signalling messages (connection candidates, offers) that pass through a server are **sealed** with Noise's `K` pattern, with both static keys known and the session in the prologue, so only the pinned peer can open them and only for that session. An offer carries a timestamp, and the host takes only one newer than the last it took from that client, so a recorded offer can't be played again. A new connection replaces a live session only once its client has proved itself in it.

The exact formats are in [the protocol reference](reference/protocol.md).

## What a server sees

Using a rendezvous server and relay means trusting them with **metadata only**.

| A rendezvous server | |
|---|---|
| Sees | That a device is online, its role and ID; which paired client asks for which host, and when; that a host is pairing, its display name, and which client attempts, how many times and when; the size of sealed messages; the addresses clients connect from (in its network layer, though its own logs keep no IP addresses) |
| Can't see | The pairing code, screen, keyboard, sound, clipboard, or the contents of signalling blobs. It checks only their size and encoding |
| Can't do | Forge or read sealed blobs, impersonate a device to another, or authorize a client the host didn't sign for |
| Can do | Drop or delay messages, lie about who is online, or refuse service |

| A relay (coturn) | |
|---|---|
| Sees | Encrypted packets: their size and timing, and the addresses of both ends |
| Can't see | Their contents |
| Logs | Session metadata (addresses, a username of the form `expiry:device ID`, byte counts) for seven days in a separate journal, on a server set up with `deploy.sh` |

What authorizes a client is **the host's own signed list of paired clients**, which the server stores but can't forge: it is signed with the host's key. Relay credentials are short-lived (ten minutes) and derived from a secret shared between the rendezvous and coturn, and relays refuse to forward to private networks, so a relay can't be used to probe the server's own network.

### Trusting a server

- By default a device trusts a server's TLS certificate through the operating system's roots. For a private CA, a device **pins** the CA you give it (in `server.toml` or an imported link), and then trusts nothing else. A CA is **never downloaded** from the server it is meant to check.
- Importing a link that carries a CA asks you first, because it changes what the device trusts.
- A signature a device makes to authenticate binds the name of the server it connected to and checked over TLS (never a name the server supplies), so a signature for one server is useless at another.
- Hosts are enrolled with a token or an allowlist, so strangers can't use your server as a host ([self-hosting](self-hosting.md)).
- A server's operator can send a notice to every device; its text is shown as written, its link must be `https://`.

## Revocation

- **Removing a device on the host** uploads a new signed list without it. Its sessions with the host end at once, it is told it was revoked, and it can't connect or get relay credentials through that host again.
- **Relay credentials** can't be recalled: ones already issued can make new allocations for up to ten minutes, and an allocation lives while it is refreshed. The host's end-to-end check of pinned keys is what shuts a revoked client out.
- **An operator** can withdraw a host in allowlist mode.

## On the computer being controlled

- One device at a time.
- **The clipboard** is shared both ways; entries a password manager marks as concealed stay marked on the other side (and out of clipboard history where the OS supports it).
- **Secure screens** (a Windows UAC prompt, the lock screen) aren't captured or controlled: the client is told so instead of getting a picture.
- A SodiumOS host can show a **privacy screen** or turn the monitor off while a session runs, ignore local input, and put the desktop under the session lock when such a session ends.
- Turning off **Allow remote control** disconnects any session and stops new ones; paired devices stay paired.

## No telemetry, no updater

Sodium Remote sends nothing to its authors. It doesn't check for updates. You update yourself ([install](install.md)), and the only servers it contacts are the ones you configured (and the other computers you connect to), with one exception. On Windows, when the computer has no graphics hardware that can encode H.264 and "software video" is on (the default), the host downloads Cisco's OpenH264 codec once, from `ciscobinary.openh264.org` over HTTPS. That happens the first time the host starts on such a computer (the app shows a one-time notice), not at install. The file is checked against a SHA-256 built into the program (and again, unpacked, every time it is loaded), is limited to 8 MiB and 120 seconds, and nothing about you or the computer is sent. A computer with hardware H.264 never downloads it, and turning off "software video" prevents it.

## Source offer (AGPL §13)

Sodium Remote is licensed under the AGPL-3.0, whose section 13 asks that people who use a modified version over a network can get its source. The apps show a link to the source in About (Source code), `--version` prints it, and a rendezvous server includes it in the challenge it sends each device (`source`). The link is `https://github.com/sodium-os/sodium-remote`. If you run a modified server, change it to where your source is.

## Reporting

Report security problems privately: see [SECURITY.md](../SECURITY.md).
